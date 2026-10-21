use gpui::{prelude::*, px, App, SharedString, Window};

use super::theme;

/// A single-line text field *display*: query text, caret and placeholder. Key handling lives in
/// the owning overlay, which forwards characters into its own state.
#[derive(IntoElement)]
pub struct TextInput {
    text: SharedString,
    placeholder: SharedString,
}

impl TextInput {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            placeholder: SharedString::default(),
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
}

impl RenderOnce for TextInput {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let empty = self.text.is_empty();
        gpui::div()
            .flex()
            .flex_1()
            .items_center()
            .h(px(20.))
            .text_size(px(13.))
            .text_color(t.text)
            .when(!empty, |el| el.child(self.text.clone()))
            .child(gpui::div().w(px(1.)).h(px(15.)).bg(t.accent))
            .when(empty, |el| {
                el.child(
                    gpui::div()
                        .ml_1()
                        .text_color(t.text_placeholder)
                        .child(self.placeholder.clone()),
                )
            })
    }
}

/// Editable single-line text state: caret, selection, word/line movement, paste. The owning
/// overlay feeds it keystrokes; `TextInput` only draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextEdit {
    text: String,
    /// Byte offsets; `anchor == caret` means no selection.
    caret: usize,
    anchor: usize,
}

impl TextEdit {
    pub fn new(text: impl Into<String>) -> Self {
        let text: String = text.into();
        let n = text.len();
        Self {
            text,
            caret: n,
            anchor: n,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor)
            .then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((a, b)) => {
                self.text.replace_range(a..b, "");
                self.caret = a;
                self.anchor = a;
                true
            }
            None => false,
        }
    }

    /// Inserts (or pastes) text at the caret, replacing the selection. Newlines are dropped.
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
        self.anchor = self.caret;
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() || self.caret == 0 {
            return;
        }
        let prev = self.prev_char(self.caret);
        self.text.replace_range(prev..self.caret, "");
        self.caret = prev;
        self.anchor = prev;
    }

    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        let to = self.word_left(self.caret);
        self.text.replace_range(to..self.caret, "");
        self.caret = to;
        self.anchor = to;
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() || self.caret >= self.text.len() {
            return;
        }
        let next = self.next_char(self.caret);
        self.text.replace_range(self.caret..next, "");
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

    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
    }

    pub fn left(&mut self, extend: bool) {
        let to = match (self.selection(), extend) {
            (Some((a, _)), false) => a,
            _ => self.prev_char(self.caret),
        };
        self.move_to(to, extend);
    }

    pub fn right(&mut self, extend: bool) {
        let to = match (self.selection(), extend) {
            (Some((_, b)), false) => b,
            _ => self.next_char(self.caret),
        };
        self.move_to(to, extend);
    }

    pub fn word_left_move(&mut self, extend: bool) {
        self.move_to(self.word_left(self.caret), extend);
    }

    pub fn word_right_move(&mut self, extend: bool) {
        self.move_to(self.word_right(self.caret), extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    /// Places the caret at byte offset `i` (snapped to a char boundary), extending or not.
    pub fn set_caret(&mut self, i: usize, extend: bool) {
        let mut i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        self.move_to(i, extend);
    }

    /// Selects the word (or run of spaces) around byte offset `i` (double-click).
    pub fn select_word_at(&mut self, i: usize) {
        self.set_caret(i, false);
        let i = self.caret;
        let space = |c: char| c.is_whitespace();
        let at_space = self.text[i..]
            .chars()
            .next()
            .or_else(|| self.text[..i].chars().next_back())
            .is_some_and(space);
        let mut a = i;
        while a > 0
            && self.text[..a]
                .chars()
                .next_back()
                .is_some_and(|c| space(c) == at_space)
        {
            a = self.prev_char(a);
        }
        let mut b = i;
        while b < self.text.len()
            && self.text[b..]
                .chars()
                .next()
                .is_some_and(|c| space(c) == at_space)
        {
            b = self.next_char(b);
        }
        self.anchor = a;
        self.caret = b;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// Removes and returns the selection (cut).
    pub fn take_selection(&mut self) -> Option<String> {
        let s = self.selected_text()?.to_string();
        self.delete_selection();
        Some(s)
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(a, b)| &self.text[a..b])
    }
}

#[cfg(test)]
mod edit_tests {
    use super::TextEdit;

    #[test]
    fn caret_selection_and_word_movement() {
        let mut e = TextEdit::new("cargo watch");
        e.word_left_move(false);
        assert_eq!(e.caret(), 6);
        e.home(true);
        assert_eq!(e.selected_text(), Some("cargo "));
        e.insert("x");
        assert_eq!(e.text(), "xwatch");
        e.end(false);
        e.delete_word_back();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn mouse_positioning_and_word_selection() {
        let mut e = TextEdit::new("cargo watch -x");
        e.select_word_at(8);
        assert_eq!(e.selected_text(), Some("watch"));
        e.set_caret(2, false);
        e.set_caret(11, true);
        assert_eq!(e.selected_text(), Some("rgo watch"));
        let mut e = TextEdit::new("é");
        e.set_caret(1, false);
        assert_eq!(e.caret(), 0);
    }

    #[test]
    fn cut_removes_the_selection() {
        let mut e = TextEdit::new("cargo watch");
        e.select_word_at(1);
        assert_eq!(e.take_selection().as_deref(), Some("cargo"));
        assert_eq!(e.text(), " watch");
        assert_eq!(e.take_selection(), None);
    }

    #[test]
    fn multibyte_and_paste_are_safe() {
        let mut e = TextEdit::new("é");
        e.left(false);
        assert_eq!(e.caret(), 0);
        e.insert("a\nb");
        assert_eq!(e.text(), "abé");
        e.select_all();
        e.backspace();
        assert_eq!(e.text(), "");
    }
}
