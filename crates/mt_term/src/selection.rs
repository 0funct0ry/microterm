//! Selection on top of the engine's grid selection (SPEC §10.2).
//!
//! Coordinates are viewport rows/cols (row 0 is the top visible row). The engine stores
//! selections in absolute grid lines, so a selection keeps following its text as the view
//! scrolls. Word selection is computed here from `selection.word_chars` rather than from the
//! engine's escape-character set, so a word is exactly "alphanumeric or a word char".

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// Character drag.
    Simple,
    /// Rectangular (Alt/Option-drag).
    Block,
    /// Double-click.
    Word,
    /// Triple-click.
    Line,
}

/// The selected region in viewport coordinates. Rows may fall outside `0..rows` when the
/// selection extends into scrollback or below the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectionSpan {
    pub start: (i32, usize),
    pub end: (i32, usize),
    pub block: bool,
}

fn point<T>(term: &Term<T>, row: usize, col: usize) -> Point {
    let g = term.grid();
    let row = row.min(g.screen_lines() - 1) as i32;
    let col = col.min(g.columns() - 1);
    Point::new(Line(row - g.display_offset() as i32), Column(col))
}

fn side(right: bool) -> Side {
    if right {
        Side::Right
    } else {
        Side::Left
    }
}

fn is_word_char(c: char, word_chars: &str) -> bool {
    c.is_alphanumeric() || word_chars.contains(c)
}

/// Normalises a point on a wide-char spacer to the wide cell itself.
fn base_col<T>(term: &Term<T>, line: Line, col: usize) -> usize {
    if col > 0
        && term.grid()[Point::new(line, Column(col))]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    {
        col - 1
    } else {
        col
    }
}

fn word_bounds<T>(term: &Term<T>, p: Point, word_chars: &str) -> (Point, Point) {
    let cols = term.grid().columns();
    let line = p.line;
    let ch = |c: usize| term.grid()[Point::new(line, Column(c))].c;
    let col = base_col(term, line, p.column.0);
    let class = ch(col);
    let same = |c: char| {
        if class == ' ' || class == '\0' {
            c == ' ' || c == '\0'
        } else if is_word_char(class, word_chars) {
            is_word_char(c, word_chars)
        } else {
            // A run of the same punctuation character.
            c == class
        }
    };
    let cls_at = |c: usize| {
        let c = base_col(term, line, c);
        ch(c)
    };
    let mut s = col;
    while s > 0 && same(cls_at(s - 1)) {
        s -= 1;
    }
    s = base_col(term, line, s);
    let mut e = col;
    while e + 1 < cols && same(cls_at(e + 1)) {
        e += 1;
    }
    // Cover the spacer half of a wide character at the end.
    if e + 1 < cols
        && term.grid()[Point::new(line, Column(e + 1))]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    {
        e += 1;
    }
    (Point::new(line, Column(s)), Point::new(line, Column(e)))
}

pub(crate) fn start<T>(
    term: &mut Term<T>,
    kind: SelectionKind,
    row: usize,
    col: usize,
    right: bool,
    word_chars: &str,
) {
    let p = point(term, row, col);
    start_at(term, kind, p, right, word_chars);
}

/// [`start`] at a grid line (a folded view's rows no longer equal viewport rows).
pub(crate) fn start_line<T>(
    term: &mut Term<T>,
    kind: SelectionKind,
    line: i32,
    col: usize,
    right: bool,
    word_chars: &str,
) {
    let p = Point::new(Line(line), Column(col.min(term.grid().columns() - 1)));
    start_at(term, kind, p, right, word_chars);
}

fn start_at<T>(term: &mut Term<T>, kind: SelectionKind, p: Point, right: bool, word_chars: &str) {
    let sel = match kind {
        SelectionKind::Simple => Selection::new(SelectionType::Simple, p, side(right)),
        SelectionKind::Block => Selection::new(SelectionType::Block, p, side(right)),
        SelectionKind::Line => Selection::new(SelectionType::Lines, p, Side::Left),
        SelectionKind::Word => {
            let (s, e) = word_bounds(term, p, word_chars);
            let mut sel = Selection::new(SelectionType::Simple, s, Side::Left);
            sel.update(e, Side::Right);
            sel
        }
    };
    term.selection = Some(sel);
}

pub(crate) fn update<T>(term: &mut Term<T>, row: usize, col: usize, right: bool) {
    let p = point(term, row, col);
    update_at(term, p, right);
}

pub(crate) fn update_line<T>(term: &mut Term<T>, line: i32, col: usize, right: bool) {
    let p = Point::new(Line(line), Column(col.min(term.grid().columns() - 1)));
    update_at(term, p, right);
}

fn update_at<T>(term: &mut Term<T>, p: Point, right: bool) {
    if let Some(sel) = term.selection.as_mut() {
        sel.update(p, side(right));
    }
}

pub(crate) fn clear<T>(term: &mut Term<T>) {
    term.selection = None;
}

pub(crate) fn select_all<T>(term: &mut Term<T>) {
    let g = term.grid();
    let top = Point::new(Line(-(g.history_size() as i32)), Column(0));
    let bottom = Point::new(Line(g.screen_lines() as i32 - 1), Column(g.columns() - 1));
    let mut sel = Selection::new(SelectionType::Simple, top, Side::Left);
    sel.update(bottom, Side::Right);
    term.selection = Some(sel);
}

pub(crate) fn text<T>(term: &Term<T>, trim_trailing: bool) -> Option<String> {
    let s = term.selection_to_string()?;
    if s.is_empty() {
        return None;
    }
    if trim_trailing {
        Some(
            s.split('\n')
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    } else {
        Some(s)
    }
}

pub(crate) fn span<T>(term: &Term<T>) -> Option<SelectionSpan> {
    let sel = term.selection.as_ref()?;
    let r = sel.to_range(term)?;
    let off = term.grid().display_offset() as i32;
    Some(SelectionSpan {
        start: (r.start.line.0 + off, r.start.column.0),
        end: (r.end.line.0 + off, r.end.column.0),
        block: r.is_block,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    const WC: &str = "-_./~:@";

    fn emu(input: &str, cols: usize, rows: usize) -> Emulator {
        let mut e = Emulator::new(cols, rows, 100);
        e.advance(input.as_bytes());
        e
    }

    fn word(e: &mut Emulator, row: usize, col: usize, wc: &str) -> String {
        e.selection_start(SelectionKind::Word, row, col, false, wc);
        e.selection_text(true).unwrap()
    }

    #[test]
    fn word_respects_word_chars() {
        let mut e = emu("cd /usr/local-bin/x_y.rs now", 40, 3);
        assert_eq!(word(&mut e, 0, 6, WC), "/usr/local-bin/x_y.rs");
        assert_eq!(word(&mut e, 0, 0, WC), "cd");
        assert_eq!(word(&mut e, 0, 26, WC), "now");
        // Without the extra chars the path splits at '/'.
        assert_eq!(word(&mut e, 0, 6, ""), "usr");
    }

    #[test]
    fn word_stops_at_brackets_and_quotes() {
        let mut e = emu("foo(bar_baz) \"qux\"", 40, 3);
        assert_eq!(word(&mut e, 0, 5, WC), "bar_baz");
        assert_eq!(word(&mut e, 0, 15, WC), "qux");
    }

    #[test]
    fn word_on_blank_selects_run_of_blanks() {
        let mut e = emu("ab   cd", 20, 2);
        e.selection_start(SelectionKind::Word, 0, 3, false, WC);
        assert!(e.selection_text(false).is_some_and(|t| t == "   "));
    }

    #[test]
    fn word_over_wide_chars() {
        let mut e = emu("a 漢字b c", 20, 2);
        assert_eq!(word(&mut e, 0, 3, WC), "漢字b");
        assert_eq!(word(&mut e, 0, 4, WC), "漢字b");
    }

    #[test]
    fn line_selection() {
        let mut e = emu("one two\r\nthree", 20, 4);
        e.selection_start(SelectionKind::Line, 0, 3, false, WC);
        assert_eq!(e.selection_text(true).unwrap(), "one two\n");
    }

    #[test]
    fn drag_selection_and_trim() {
        let mut e = emu("hello world   \r\nsecond", 20, 4);
        e.selection_start(SelectionKind::Simple, 0, 6, false, WC);
        e.selection_update(1, 2, false);
        assert_eq!(e.selection_text(true).unwrap(), "world\nse");
    }

    #[test]
    fn block_selection_extracts_rectangle() {
        let mut e = emu("abcdef\r\nghijkl\r\nmnopqr", 10, 4);
        e.selection_start(SelectionKind::Block, 0, 1, false, WC);
        e.selection_update(2, 3, true);
        assert_eq!(e.selection_text(true).unwrap(), "bcd\nhij\nnop");
    }

    #[test]
    fn extend_updates_end_only() {
        let mut e = emu("abcdef", 10, 2);
        e.selection_start(SelectionKind::Simple, 0, 1, false, WC);
        e.selection_update(0, 2, true);
        assert_eq!(e.selection_text(true).unwrap(), "bc");
        e.selection_update(0, 4, true);
        assert_eq!(e.selection_text(true).unwrap(), "bcde");
    }

    #[test]
    fn select_all_includes_scrollback() {
        let mut e = emu("l1\r\nl2\r\nl3\r\nl4\r\nl5", 10, 3);
        e.select_all();
        assert_eq!(e.selection_text(true).unwrap(), "l1\nl2\nl3\nl4\nl5");
    }

    #[test]
    fn clear_removes_selection() {
        let mut e = emu("abc", 10, 2);
        e.selection_start(SelectionKind::Word, 0, 0, false, WC);
        assert!(e.snapshot().selection.is_some());
        e.selection_clear();
        assert!(e.snapshot().selection.is_none());
        assert_eq!(e.selection_text(true), None);
    }

    #[test]
    fn span_is_in_viewport_coordinates() {
        let mut e = emu("ab\r\ncd", 10, 3);
        e.selection_start(SelectionKind::Simple, 1, 0, false, WC);
        e.selection_update(1, 1, true);
        let s = e.snapshot().selection.unwrap();
        assert_eq!((s.start, s.end, s.block), ((1, 0), (1, 1), false));
    }

    #[test]
    fn trailing_whitespace_option() {
        let mut e = emu("ab  \r\ncd", 10, 3);
        e.selection_start(SelectionKind::Simple, 0, 0, false, WC);
        e.selection_update(1, 1, true);
        // "ab" then two real spaces are kept only when trimming is off.
        assert_eq!(e.selection_text(true).unwrap(), "ab\ncd");
    }
}
