//! Sending an expanded snippet to a pane (SPEC §10.8.5 steps 5-7): sanitise, paste (bracketed
//! or `\n` → `\r`), optionally run, and place the cursor for `{{cursor}}`.

use mt_snippets::{sanitize, Expanded};

/// `{{cursor}}` moves back at most this many characters.
pub const MAX_CURSOR_MOVES: usize = 1_000;

/// What a pane receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub text: String,
    pub run: bool,
    /// Cursor-left presses after the paste.
    pub cursor_left: usize,
}

/// A terminal as far as delivery is concerned.
pub trait Sink {
    fn paste(&self, text: &str);
    fn write(&self, bytes: Vec<u8>);
    /// DECCKM: cursor keys send `SS3` instead of `CSI`.
    fn app_cursor(&self) -> bool;
}

impl Sink for mt_term::Terminal {
    fn paste(&self, text: &str) {
        mt_term::Terminal::paste(self, text);
    }
    fn write(&self, bytes: Vec<u8>) {
        mt_term::Terminal::write(self, bytes);
    }
    fn app_cursor(&self) -> bool {
        self.modes().app_cursor
    }
}

/// Decides what to send. The cursor marker only counts for a single-line insert that does not
/// run (SPEC §10.8.5 step 7).
pub fn plan(expanded: &Expanded, run: bool) -> Delivery {
    let text = sanitize(&expanded.text);
    let cursor_left = match expanded.cursor {
        Some(at) if !run && !text.contains('\n') => sanitize(&expanded.text[at..])
            .chars()
            .count()
            .min(MAX_CURSOR_MOVES),
        _ => 0,
    };
    Delivery {
        text,
        run,
        cursor_left,
    }
}

pub fn deliver(sink: &dyn Sink, d: &Delivery) {
    if !d.text.is_empty() {
        sink.paste(&d.text);
    }
    if d.run {
        sink.write(b"\r".to_vec());
    }
    if d.cursor_left > 0 {
        let one: &[u8] = if sink.app_cursor() {
            b"\x1bOD"
        } else {
            b"\x1b[D"
        };
        sink.write(one.repeat(d.cursor_left));
    }
}

/// The text is multi-line (more than one line once a trailing newline is ignored).
pub fn is_multiline(text: &str) -> bool {
    text.trim_end_matches(['\n', '\r']).contains(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Rec {
        log: RefCell<Vec<String>>,
        app: bool,
    }
    impl Sink for Rec {
        fn paste(&self, text: &str) {
            self.log.borrow_mut().push(format!("paste:{text}"));
        }
        fn write(&self, bytes: Vec<u8>) {
            self.log.borrow_mut().push(format!(
                "write:{}",
                String::from_utf8_lossy(&bytes).escape_debug()
            ));
        }
        fn app_cursor(&self) -> bool {
            self.app
        }
    }

    fn ex(text: &str, cursor: Option<usize>) -> Expanded {
        Expanded {
            text: text.into(),
            cursor,
            has_secret: false,
        }
    }

    #[test]
    fn plain_paste_without_run() {
        let r = Rec::default();
        deliver(&r, &plan(&ex("echo hi", None), false));
        assert_eq!(*r.log.borrow(), ["paste:echo hi"]);
    }

    #[test]
    fn run_sends_enter_after_the_paste() {
        let r = Rec::default();
        deliver(&r, &plan(&ex("ls", None), true));
        assert_eq!(*r.log.borrow(), ["paste:ls", "write:\\r"]);
    }

    #[test]
    fn esc_and_nul_never_reach_the_pane() {
        let d = plan(&ex("a\u{1b}[31m\0b\r\nc", None), false);
        assert_eq!(d.text, "a[31mb\nc");
    }

    #[test]
    fn cursor_moves_left_over_the_tail() {
        let r = Rec::default();
        // "psql " then marker then "-d é" (4 chars after the marker).
        deliver(&r, &plan(&ex("psql -d é", Some(5)), false));
        let moves = format!("write:{}", "\\u{1b}[D".repeat(4));
        assert_eq!(*r.log.borrow(), ["paste:psql -d é".to_string(), moves]);
    }

    #[test]
    fn cursor_uses_ss3_in_application_cursor_mode() {
        let r = Rec {
            app: true,
            ..Default::default()
        };
        deliver(&r, &plan(&ex("ab", Some(1)), false));
        assert_eq!(r.log.borrow()[1], "write:\\u{1b}OD");
    }

    #[test]
    fn cursor_is_ignored_when_running_or_multiline() {
        assert_eq!(plan(&ex("ab", Some(1)), true).cursor_left, 0);
        assert_eq!(plan(&ex("a\nb", Some(1)), false).cursor_left, 0);
        assert_eq!(plan(&ex("ab", None), false).cursor_left, 0);
        assert_eq!(
            plan(&ex("ab", Some(2)), false).cursor_left,
            0,
            "marker at the end"
        );
    }

    #[test]
    fn cursor_moves_are_capped() {
        let long = "x".repeat(2_000);
        assert_eq!(
            plan(&ex(&long, Some(0)), false).cursor_left,
            MAX_CURSOR_MOVES
        );
    }

    #[test]
    fn stripped_characters_do_not_count_toward_the_tail() {
        assert_eq!(plan(&ex("ab\u{1b}\u{1b}c", Some(2)), false).cursor_left, 1);
    }

    #[test]
    fn multiline_detection() {
        assert!(is_multiline("a\nb"));
        assert!(!is_multiline("a\n"));
        assert!(!is_multiline("a"));
    }
}
