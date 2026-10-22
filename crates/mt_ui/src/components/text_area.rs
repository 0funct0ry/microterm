//! Multi-line editable text state (SPEC §10.8.6 editor body): newline, line up/down with a goal
//! column, per-line home/end, word moves, selection, paste and vertical scrolling. The owning
//! view feeds it keystrokes and draws it. Offsets are bytes; columns count characters.

use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextAreaEdit {
    text: String,
    caret: usize,
    anchor: usize,
    /// Column (in characters) the caret returns to when moving vertically.
    goal_col: Option<usize>,
    /// First visible line.
    pub scroll: usize,
}

impl TextAreaEdit {
    pub fn new(text: impl Into<String>) -> Self {
        let text: String = text.into().replace("\r\n", "\n");
        let n = text.len();
        Self {
            text,
            caret: n,
            anchor: n,
            goal_col: None,
            scroll: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor)
            .then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(a, b)| &self.text[a..b])
    }

    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// Byte range of `line` without its newline.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        let mut start = 0;
        for (i, l) in self.text.split('\n').enumerate() {
            if i == line {
                return start..start + l.len();
            }
            start += l.len() + 1;
        }
        self.text.len()..self.text.len()
    }

    pub fn line_of(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .matches('\n')
            .count()
    }

    /// `(line, column in characters)` of a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = self.clamp(offset);
        let line = self.line_of(offset);
        let start = self.line_range(line).start;
        (line, self.text[start..offset].chars().count())
    }

    fn clamp(&self, mut i: usize) -> usize {
        i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((a, b)) => {
                self.text.replace_range(a..b, "");
                self.caret = a;
                self.anchor = a;
                self.goal_col = None;
                true
            }
            None => false,
        }
    }

    /// Inserts (or pastes) text, replacing the selection. `\r\n` becomes `\n`; control
    /// characters other than `\n` and `\t` are dropped.
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let s: String = s
            .replace("\r\n", "\n")
            .chars()
            .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
            .collect();
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
        self.anchor = self.caret;
        self.goal_col = None;
    }

    pub fn newline(&mut self) {
        self.insert("\n");
    }

    fn prev_char(&self, i: usize) -> usize {
        self.text[..i]
            .char_indices()
            .next_back()
            .map_or(0, |(p, _)| p)
    }

    fn next_char(&self, i: usize) -> usize {
        self.text[i..]
            .chars()
            .next()
            .map_or(i, |c| i + c.len_utf8())
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() || self.caret == 0 {
            return;
        }
        let prev = self.prev_char(self.caret);
        self.text.replace_range(prev..self.caret, "");
        self.caret = prev;
        self.anchor = prev;
        self.goal_col = None;
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() || self.caret >= self.text.len() {
            return;
        }
        let next = self.next_char(self.caret);
        self.text.replace_range(self.caret..next, "");
        self.goal_col = None;
    }

    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        let to = self.word_left(self.caret);
        self.text.replace_range(to..self.caret, "");
        self.caret = to;
        self.anchor = to;
        self.goal_col = None;
    }

    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && self.text[..i].ends_with(char::is_whitespace) {
            i = self.prev_char(i);
        }
        while i > 0 && !self.text[..i].ends_with(char::is_whitespace) {
            i = self.prev_char(i);
        }
        i
    }

    fn word_right(&self, mut i: usize) -> usize {
        while i < self.text.len() && self.text[i..].starts_with(char::is_whitespace) {
            i = self.next_char(i);
        }
        while i < self.text.len() && !self.text[i..].starts_with(char::is_whitespace) {
            i = self.next_char(i);
        }
        i
    }

    fn move_to(&mut self, to: usize, extend: bool, keep_goal: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
        if !keep_goal {
            self.goal_col = None;
        }
    }

    pub fn left(&mut self, extend: bool) {
        let to = match (self.selection(), extend) {
            (Some((a, _)), false) => a,
            _ => self.prev_char(self.caret),
        };
        self.move_to(to, extend, false);
    }

    pub fn right(&mut self, extend: bool) {
        let to = match (self.selection(), extend) {
            (Some((_, b)), false) => b,
            _ => self.next_char(self.caret),
        };
        self.move_to(to, extend, false);
    }

    pub fn word_left_move(&mut self, extend: bool) {
        self.move_to(self.word_left(self.caret), extend, false);
    }

    pub fn word_right_move(&mut self, extend: bool) {
        self.move_to(self.word_right(self.caret), extend, false);
    }

    /// Start of the caret's line.
    pub fn home(&mut self, extend: bool) {
        let at = self.line_range(self.line_of(self.caret)).start;
        self.move_to(at, extend, false);
    }

    /// End of the caret's line.
    pub fn end(&mut self, extend: bool) {
        let at = self.line_range(self.line_of(self.caret)).end;
        self.move_to(at, extend, false);
    }

    pub fn doc_home(&mut self, extend: bool) {
        self.move_to(0, extend, false);
    }

    pub fn doc_end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend, false);
    }

    fn vertical(&mut self, delta: isize, extend: bool) {
        let (line, col) = self.line_col(self.caret);
        let goal = *self.goal_col.get_or_insert(col);
        let target = line as isize + delta;
        if target < 0 {
            self.move_to(0, extend, false);
            return;
        }
        let target = target as usize;
        if target >= self.line_count() {
            self.move_to(self.text.len(), extend, false);
            return;
        }
        let r = self.line_range(target);
        let at = self.text[r.clone()]
            .char_indices()
            .nth(goal)
            .map_or(r.end, |(i, _)| r.start + i);
        self.move_to(at, extend, true);
    }

    /// Moves up a line, keeping the goal column. On the first line it goes to the start.
    pub fn up(&mut self, extend: bool) {
        self.vertical(-1, extend);
    }

    /// Moves down a line, keeping the goal column. On the last line it goes to the end.
    pub fn down(&mut self, extend: bool) {
        self.vertical(1, extend);
    }

    pub fn set_caret(&mut self, i: usize, extend: bool) {
        let i = self.clamp(i);
        self.move_to(i, extend, false);
    }

    /// Caret at `col` characters into `line` (mouse clicks resolve to these).
    pub fn set_caret_line_col(&mut self, line: usize, byte_in_line: usize, extend: bool) {
        let r = self.line_range(line.min(self.line_count() - 1));
        self.set_caret((r.start + byte_in_line).min(r.end), extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
        self.goal_col = None;
    }

    pub fn select_word_at(&mut self, i: usize) {
        self.set_caret(i, false);
        let i = self.caret;
        let class = |c: char| c.is_alphanumeric() || c == '_';
        let at_word = self.text[i..]
            .chars()
            .next()
            .or_else(|| self.text[..i].chars().next_back())
            .is_some_and(class);
        let mut a = i;
        while a > 0
            && self.text[..a]
                .chars()
                .next_back()
                .is_some_and(|c| c != '\n' && class(c) == at_word)
        {
            a = self.prev_char(a);
        }
        let mut b = i;
        while b < self.text.len()
            && self.text[b..]
                .chars()
                .next()
                .is_some_and(|c| c != '\n' && class(c) == at_word)
        {
            b = self.next_char(b);
        }
        self.anchor = a;
        self.caret = b;
    }

    pub fn take_selection(&mut self) -> Option<String> {
        let s = self.selected_text()?.to_string();
        self.delete_selection();
        Some(s)
    }

    /// Keeps the caret's line inside a window of `visible` lines.
    pub fn scroll_to_caret(&mut self, visible: usize) {
        let line = self.line_of(self.caret);
        let visible = visible.max(1);
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + visible {
            self.scroll = line + 1 - visible;
        }
        self.scroll = self.scroll.min(self.line_count().saturating_sub(1));
    }

    pub fn scroll_by(&mut self, lines: isize, visible: usize) {
        let max = self.line_count().saturating_sub(visible.max(1));
        self.scroll = (self.scroll as isize + lines).clamp(0, max as isize) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_and_lines() {
        let mut e = TextAreaEdit::new("ab");
        e.newline();
        e.insert("cd");
        assert_eq!(e.text(), "ab\ncd");
        assert_eq!(e.line_count(), 2);
        assert_eq!(e.line_range(0), 0..2);
        assert_eq!(e.line_range(1), 3..5);
        assert_eq!(e.line_col(e.caret()), (1, 2));
        assert_eq!(TextAreaEdit::new("a\r\nb").text(), "a\nb");
    }

    #[test]
    fn up_down_keep_the_goal_column() {
        let mut e = TextAreaEdit::new("abcdef\nab\nabcdef");
        // Caret at the end of the last line (col 6).
        e.up(false);
        assert_eq!(e.line_col(e.caret()), (1, 2), "short line clamps");
        e.up(false);
        assert_eq!(e.line_col(e.caret()), (0, 6), "goal column restored");
        e.up(false);
        assert_eq!(e.caret(), 0, "first line: to the start");
        e.down(false);
        assert_eq!(e.line_col(e.caret()), (1, 0));
        e.down(false);
        e.down(false);
        assert_eq!(e.caret(), e.text().len(), "last line: to the end");
    }

    #[test]
    fn home_end_are_per_line() {
        let mut e = TextAreaEdit::new("one\ntwo three");
        e.set_caret(7, false);
        e.home(false);
        assert_eq!(e.caret(), 4);
        e.end(false);
        assert_eq!(e.caret(), e.text().len());
        e.set_caret(1, false);
        e.end(false);
        assert_eq!(e.caret(), 3);
        e.doc_home(false);
        assert_eq!(e.caret(), 0);
        e.doc_end(true);
        assert_eq!(e.selected_text(), Some("one\ntwo three"));
    }

    #[test]
    fn words_selection_and_editing() {
        let mut e = TextAreaEdit::new("cargo watch\nnext");
        e.word_left_move(false);
        assert_eq!(e.caret(), 12);
        e.word_left_move(false);
        assert_eq!(e.caret(), 6);
        e.word_right_move(true);
        assert_eq!(e.selected_text(), Some("watch\nnext").map(|_| "watch"));
        e.insert("X");
        assert_eq!(e.text(), "cargo X\nnext");
        e.delete_word_back();
        assert_eq!(e.text(), "cargo \nnext");
        e.select_all();
        assert_eq!(e.take_selection().as_deref(), Some("cargo \nnext"));
        assert!(e.is_empty());
    }

    #[test]
    fn insert_filters_control_characters_but_keeps_newlines_and_tabs() {
        let mut e = TextAreaEdit::default();
        e.insert("a\u{1b}[0m\tb\r\nc\0");
        assert_eq!(e.text(), "a[0m\tb\nc");
    }

    #[test]
    fn backspace_and_delete_across_lines_and_multibyte() {
        let mut e = TextAreaEdit::new("é\nb");
        e.set_caret(4, false);
        e.backspace();
        assert_eq!(e.text(), "é\n");
        e.backspace();
        assert_eq!(e.text(), "é");
        e.backspace();
        assert_eq!(e.text(), "");
        let mut e = TextAreaEdit::new("a\nb");
        e.doc_home(false);
        e.delete_forward();
        e.delete_forward();
        assert_eq!(e.text(), "b");
    }

    #[test]
    fn word_selection_stops_at_newlines() {
        let mut e = TextAreaEdit::new("foo bar\nbaz");
        e.select_word_at(5);
        assert_eq!(e.selected_text(), Some("bar"));
        e.select_word_at(9);
        assert_eq!(e.selected_text(), Some("baz"));
    }

    #[test]
    fn scrolling_follows_the_caret() {
        let mut e = TextAreaEdit::new(
            (0..20)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        e.scroll_to_caret(5);
        assert_eq!(e.scroll, 15);
        e.doc_home(false);
        e.scroll_to_caret(5);
        assert_eq!(e.scroll, 0);
        e.scroll_by(100, 5);
        assert_eq!(e.scroll, 15);
        e.scroll_by(-100, 5);
        assert_eq!(e.scroll, 0);
    }

    #[test]
    fn click_positions_resolve_per_line() {
        let mut e = TextAreaEdit::new("abc\ndef");
        e.set_caret_line_col(1, 2, false);
        assert_eq!(e.caret(), 6);
        e.set_caret_line_col(0, 99, false);
        assert_eq!(e.caret(), 3);
        e.set_caret_line_col(9, 0, false);
        assert_eq!(e.caret(), 4, "past the last line clamps to it");
    }
}
