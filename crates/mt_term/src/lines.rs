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

/// The whole grid as logical lines: soft-wrapped rows are joined, trailing blanks and wide-char
/// spacers dropped, trailing empty lines removed. Index 0 is the oldest scrollback line.
pub(crate) fn logical_lines<T>(term: &Term<T>) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for abs in 0..total_lines(term) {
        let Some(cells) = line_cells(term, abs) else {
            break;
        };
        let seg: String = cells.chars.iter().filter(|c| **c != '\0').collect();
        if cells.wrapped {
            cur.push_str(&seg);
        } else {
            cur.push_str(seg.trim_end());
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

/// One row with full styling, by absolute line (for `get-text --escapes`).
pub(crate) fn styled_line<T>(term: &Term<T>, abs: usize) -> Option<Vec<crate::Cell>> {
    if abs >= total_lines(term) {
        return None;
    }
    let g = term.grid();
    let line = grid_line(term, abs);
    let colors = term.colors();
    Some(
        (0..g.columns())
            .map(|c| crate::snapshot::convert_cell(&g[Point::new(line, Column(c))], colors))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use crate::Emulator;

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
