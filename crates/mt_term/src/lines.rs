//! Absolute-line access for copy mode, search and quick select (SPEC §10.4–10.6), and OSC 133
//! prompt marks.
//!
//! An *absolute line* counts from the oldest scrollback row: `0` is the top of history and
//! `history_size() + rows - 1` is the bottom visible row. Absolute lines change when old history
//! is evicted, so callers re-query rather than caching them across output.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;

use crate::SelectionKind;

/// One grid row, a char per column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineCells {
    /// Blank cells are `' '`; the spacer half of a wide character is `'\0'`.
    pub chars: Vec<char>,
    /// The row soft-wraps into the next one (no hard newline).
    pub wrapped: bool,
}

impl LineCells {
    /// The text without wide-char spacers, trailing blanks trimmed.
    pub fn text(&self) -> String {
        let s: String = self.chars.iter().filter(|c| **c != '\0').collect();
        s.trim_end().to_string()
    }
}

pub(crate) fn history_size<T>(term: &Term<T>) -> usize {
    term.grid().history_size()
}

pub(crate) fn total_lines<T>(term: &Term<T>) -> usize {
    let g = term.grid();
    g.history_size() + g.screen_lines()
}

fn grid_line<T>(term: &Term<T>, abs: usize) -> Line {
    Line(abs as i32 - term.grid().history_size() as i32)
}

pub(crate) fn line_cells<T>(term: &Term<T>, abs: usize) -> Option<LineCells> {
    if abs >= total_lines(term) {
        return None;
    }
    let g = term.grid();
    let line = grid_line(term, abs);
    let cols = g.columns();
    let mut chars = Vec::with_capacity(cols);
    for c in 0..cols {
        let cell = &g[Point::new(line, Column(c))];
        chars.push(if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            '\0'
        } else if cell.c == '\0' {
            ' '
        } else {
            cell.c
        });
    }
    let wrapped = g[Point::new(line, Column(cols - 1))]
        .flags
        .contains(Flags::WRAPLINE);
    Some(LineCells { chars, wrapped })
}

/// The cursor as `(absolute line, column)`.
pub(crate) fn cursor_abs<T>(term: &Term<T>) -> (usize, usize) {
    let p = term.grid().cursor.point;
    let line = (p.line.0 + term.grid().history_size() as i32).max(0) as usize;
    (line, p.column.0)
}

/// Selects from `start` to `end` (inclusive cells), both `(absolute line, column)`.
pub(crate) fn select_abs<T>(
    term: &mut Term<T>,
    kind: SelectionKind,
    start: (usize, usize),
    end: (usize, usize),
) {
    let cols = term.grid().columns();
    let pt = |(l, c): (usize, usize)| Point::new(grid_line(term, l), Column(c.min(cols - 1)));
    let ty = match kind {
        SelectionKind::Block => SelectionType::Block,
        SelectionKind::Line => SelectionType::Lines,
        _ => SelectionType::Simple,
    };
    let (a, b) = (pt(start), pt(end));
    let mut sel = Selection::new(ty, a, Side::Left);
    sel.update(b, Side::Right);
    term.selection = Some(sel);
}

/// Scrolls the minimum needed to bring `abs` into view; `center` centres it instead.
pub(crate) fn scroll_to_abs<T: alacritty_terminal::event::EventListener>(
    term: &mut Term<T>,
    abs: usize,
    center: bool,
) {
    let g = term.grid();
    let hist = g.history_size() as i32;
    let rows = g.screen_lines() as i32;
    let off = g.display_offset() as i32;
    // Viewport row of the target at the current offset.
    let row = abs as i32 - hist + off;
    let want = if center {
        off + (rows / 2 - row)
    } else if row < 0 {
        off - row
    } else if row >= rows {
        off - (row - rows + 1)
    } else {
        off
    };
    let want = want.clamp(0, hist);
    term.scroll_display(Scroll::Delta(want - off));
}

/// OSC 133;A (prompt start) positions as grid lines that follow their text as history grows.
#[derive(Debug, Default)]
pub(crate) struct PromptMarks {
    lines: Vec<i32>,
    history: usize,
}

impl PromptMarks {
    /// Re-bases the marks after the history length changed.
    pub fn sync(&mut self, history: usize) {
        let d = history as i32 - self.history as i32;
        if d != 0 {
            for l in &mut self.lines {
                *l -= d;
            }
            self.history = history;
        }
        self.lines.retain(|l| *l >= -(history as i32));
    }

    pub fn push(&mut self, line: i32) {
        if let Err(i) = self.lines.binary_search(&line) {
            self.lines.insert(i, line);
        }
    }

    /// Ascending absolute lines.
    pub fn abs_lines(&self) -> Vec<usize> {
        self.lines
            .iter()
            .map(|l| (l + self.history as i32).max(0) as usize)
            .collect()
    }
}

/// Finds the `ESC ] 133 ; A` prefix across arbitrary chunk boundaries.
#[derive(Debug, Default, Clone)]
pub struct PromptSniffer {
    matched: usize,
}

const PREFIX: &[u8] = b"\x1b]133;A";

impl PromptSniffer {
    /// Offsets just past each prefix found in `bytes`.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<usize> {
        let mut out = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            if b == PREFIX[self.matched] {
                self.matched += 1;
                if self.matched == PREFIX.len() {
                    out.push(i + 1);
                    self.matched = 0;
                }
            } else {
                self.matched = usize::from(b == PREFIX[0]);
            }
        }
        out
    }
}

pub(crate) fn mark_here<T>(term: &Term<T>, marks: &mut PromptMarks) {
    marks.sync(term.grid().history_size());
    marks.push(term.grid().cursor.point.line.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    #[test]
    fn sniffer_matches_across_chunks() {
        let mut s = PromptSniffer::default();
        assert_eq!(s.feed(b"ab\x1b]13"), Vec::<usize>::new());
        assert_eq!(s.feed(b"3;A\x07x"), vec![3]);
        assert_eq!(s.feed(b"\x1b]133;B"), Vec::<usize>::new());
        assert_eq!(s.feed(b"\x1b\x1b]133;A\x1b]133;A"), vec![8, 15]);
    }

    #[test]
    fn marks_follow_scrolling_history() {
        let mut e = Emulator::new(10, 3, 100);
        e.advance(b"\x1b]133;A\x07prompt1\r\nout\r\n\x1b]133;A\x07prompt2");
        assert_eq!(e.prompt_marks(), vec![0, 2]);
        e.advance(b"\r\nmore\r\nmore2");
        // History grew by 2; marks still point at their text.
        let m = e.prompt_marks();
        assert_eq!(m, vec![0, 2]);
        assert_eq!(e.line_cells(2).unwrap().text(), "prompt2");
    }

    #[test]
    fn line_cells_cover_history_and_wrap() {
        let mut e = Emulator::new(5, 2, 100);
        e.advance(b"abcdefgh\r\nxy\r\nz");
        // Rows: "abcde"(wrapped) "fgh" "xy" "z"
        assert_eq!(e.total_lines(), 4);
        assert_eq!(e.history_size(), 2);
        let l0 = e.line_cells(0).unwrap();
        assert!(l0.wrapped);
        assert_eq!(l0.text(), "abcde");
        assert!(!e.line_cells(1).unwrap().wrapped);
        assert_eq!(e.line_cells(3).unwrap().text(), "z");
        assert!(e.line_cells(4).is_none());
    }

    #[test]
    fn wide_chars_use_spacer() {
        let mut e = Emulator::new(6, 2, 10);
        e.advance("a漢b".as_bytes());
        let l = e.line_cells(0).unwrap();
        assert_eq!(&l.chars[..4], &['a', '漢', '\0', 'b']);
        assert_eq!(l.text(), "a漢b");
    }

    #[test]
    fn select_abs_in_scrollback_and_scroll() {
        let mut e = Emulator::new(10, 2, 100);
        e.advance(b"one\r\ntwo\r\nthree\r\nfour");
        e.select_abs(crate::SelectionKind::Simple, (0, 0), (1, 2));
        assert_eq!(e.selection_text(true).unwrap(), "one\ntwo");
        e.scroll_to_abs(0, false);
        assert_eq!(e.snapshot().display_offset, 2);
        e.scroll_to_abs(3, false);
        assert_eq!(e.snapshot().display_offset, 0);
    }

    #[test]
    fn block_select_abs() {
        let mut e = Emulator::new(10, 3, 100);
        e.advance(b"abcdef\r\nghijkl\r\nmnopqr");
        e.select_abs(crate::SelectionKind::Block, (0, 1), (2, 3));
        assert_eq!(e.selection_text(true).unwrap(), "bcd\nhij\nnop");
    }
}
