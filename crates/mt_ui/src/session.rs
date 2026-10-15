//! A live terminal attached to a tab: the `mt_term` handle plus UI-side state, and the
//! translation from GPUI keystrokes to terminal key input.

use std::cell::{Cell, RefCell};
use std::io;
use std::rc::Rc;
use std::sync::Arc;

use gpui::Keystroke;
use mt_term::{Key, KeyEventKind, KeyInput, Mods, RenderSnapshot, SpawnOptions, Terminal};

use crate::terminal_view::LineCache;

/// Defaults until M3 wires the config schema (SPEC §8.3).
pub const DEFAULT_SCROLLBACK: usize = 10_000;
pub const DEFAULT_COLS: u16 = 120;
pub const DEFAULT_ROWS: u16 = 34;
/// `mouse.scroll_multiplier`: lines per wheel notch.
pub const SCROLL_MULTIPLIER: f32 = 3.0;

pub struct TerminalSession {
    pub term: Arc<Terminal>,
    pub snapshot: Arc<RenderSnapshot>,
    pub cache: Rc<RefCell<LineCache>>,
    /// Grid size last pushed to the PTY (set while painting).
    pub size: Rc<Cell<(u16, u16)>>,
    /// Window position of the grid's top-left corner (set while painting), for hit-testing.
    pub grid_origin: Rc<Cell<(f32, f32)>>,
    /// Fractional wheel/trackpad lines carried between events.
    pub scroll_remainder: f32,
    pub default_title: String,
}

impl TerminalSession {
    /// Spawns the shell. `wake` receives a unit whenever the terminal has events to drain;
    /// it is bounded to one pending message so wakeups coalesce.
    pub fn spawn(
        mut opts: SpawnOptions,
        wake: async_channel::Sender<()>,
    ) -> io::Result<TerminalSession> {
        opts.scrollback = if opts.scrollback == 0 {
            DEFAULT_SCROLLBACK
        } else {
            opts.scrollback
        };
        let default_title = std::path::Path::new(&mt_term::resolve_shell(&opts.shell))
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "shell".into());
        let term = Terminal::spawn(opts, move || {
            let _ = wake.try_send(());
        })?;
        let snapshot = Arc::new(term.snapshot());
        Ok(TerminalSession {
            term: Arc::new(term),
            snapshot,
            cache: Rc::default(),
            size: Rc::new(Cell::new((0, 0))),
            grid_origin: Rc::new(Cell::new((0.0, 0.0))),
            scroll_remainder: 0.0,
            default_title,
        })
    }

    pub fn refresh(&mut self) {
        self.snapshot = Arc::new(self.term.snapshot());
    }
}

/// Converts a GPUI keystroke to terminal input. `None` for keys the terminal never sees
/// (platform-modified shortcuts, bare modifiers).
pub fn key_input(ks: &Keystroke, held: bool, event: KeyEventKind) -> Option<KeyInput> {
    let m = &ks.modifiers;
    if m.platform || m.function && ks.key.len() > 1 && !ks.key.starts_with('f') {
        return None;
    }
    let mods = Mods {
        shift: m.shift,
        ctrl: m.control,
        alt: m.alt,
        logo: false,
    };
    let key = match ks.key.as_str() {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "space" => Key::Char(' '),
        k if k.len() > 1 && k.starts_with('f') => {
            Key::F(k[1..].parse::<u8>().ok().filter(|n| (1..=35).contains(n))?)
        }
        k => {
            let mut chars = k.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None; // shift, control, capslock, ...
            }
            Key::Char(c)
        }
    };
    // Alt acts as Meta (ESC prefix), so the layout's composed text (e.g. Option-e → "´") is
    // never used. Without Alt the typed text wins so layouts and dead keys work.
    let text = if mods.alt || mods.ctrl {
        None
    } else {
        ks.key_char.clone().filter(|s| !s.is_empty())
    };
    Some(KeyInput {
        key,
        mods,
        text,
        event: if held && event == KeyEventKind::Press {
            KeyEventKind::Repeat
        } else {
            event
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_term::{encode_key, InputMode};

    fn enc(src: &str, key_char: Option<&str>) -> Option<Vec<u8>> {
        let mut ks = Keystroke::parse(src).unwrap();
        ks.key_char = key_char.map(String::from);
        let input = key_input(&ks, false, KeyEventKind::Press)?;
        encode_key(&input, &InputMode::default())
    }

    #[test]
    fn plain_and_shifted_text() {
        assert_eq!(enc("a", Some("a")).unwrap(), b"a");
        assert_eq!(enc("shift-a", Some("A")).unwrap(), b"A");
        assert_eq!(enc("shift-1", Some("!")).unwrap(), b"!");
        assert_eq!(enc("space", Some(" ")).unwrap(), b" ");
    }

    #[test]
    fn control_and_alt() {
        assert_eq!(enc("ctrl-c", None).unwrap(), [3]);
        assert_eq!(enc("ctrl-a", Some("a")).unwrap(), [1]);
        // Option-e on a US layout composes "´"; Alt-as-meta ignores it.
        assert_eq!(enc("alt-e", Some("´")).unwrap(), b"\x1be");
    }

    #[test]
    fn named_and_function_keys() {
        assert_eq!(enc("enter", None).unwrap(), b"\r");
        assert_eq!(enc("escape", None).unwrap(), [0x1b]);
        assert_eq!(enc("up", None).unwrap(), b"\x1b[A");
        assert_eq!(enc("pageup", None).unwrap(), b"\x1b[5~");
        assert_eq!(enc("f5", None).unwrap(), b"\x1b[15~");
        assert_eq!(enc("shift-tab", None).unwrap(), b"\x1b[Z");
    }

    #[test]
    fn platform_shortcuts_are_not_sent() {
        assert!(enc("cmd-k", Some("k")).is_none());
    }

    #[test]
    fn repeat_is_flagged() {
        let ks = Keystroke::parse("a").unwrap();
        let i = key_input(&ks, true, KeyEventKind::Press).unwrap();
        assert_eq!(i.event, KeyEventKind::Repeat);
    }
}
