//! Keyboard and paste encoding: legacy xterm and the Kitty keyboard protocol.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KittyFlags {
    pub disambiguate: bool,
    pub report_events: bool,
    pub report_alternates: bool,
    pub report_all_keys: bool,
    pub report_text: bool,
}

impl KittyFlags {
    pub fn any(&self) -> bool {
        self.disambiguate
            || self.report_events
            || self.report_alternates
            || self.report_all_keys
            || self.report_text
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputMode {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub kitty: KittyFlags,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
        logo: false,
    };
    pub const SHIFT: Mods = Mods {
        shift: true,
        ..Mods::NONE
    };
    pub const CTRL: Mods = Mods {
        ctrl: true,
        ..Mods::NONE
    };
    pub const ALT: Mods = Mods {
        alt: true,
        ..Mods::NONE
    };

    /// xterm modifier parameter minus one: shift=1, alt=2, ctrl=4, super=8.
    fn bits(self) -> u32 {
        self.shift as u32
            | (self.alt as u32) << 1
            | (self.ctrl as u32) << 2
            | (self.logo as u32) << 3
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// The unshifted character on the key cap (lower case for letters).
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyEventKind {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Debug)]
pub struct KeyInput {
    pub key: Key,
    pub mods: Mods,
    /// Text the key produced with the active layout (`None` when Option acts as Alt).
    pub text: Option<String>,
    pub event: KeyEventKind,
}

impl KeyInput {
    pub fn new(key: Key, mods: Mods) -> Self {
        KeyInput {
            key,
            mods,
            text: None,
            event: KeyEventKind::Press,
        }
    }

    pub fn with_text(mut self, text: &str) -> Self {
        self.text = Some(text.to_string());
        self
    }
}

/// Encodes a key for the PTY. `None` means nothing should be sent.
pub fn encode_key(input: &KeyInput, mode: &InputMode) -> Option<Vec<u8>> {
    if mode.kitty.any() {
        encode_kitty(input, mode)
    } else {
        encode_legacy(input, mode)
    }
}

fn csi_mod_suffix(mods: Mods) -> Option<u32> {
    (mods.bits() != 0).then(|| mods.bits() + 1)
}

/// Letter/tilde functional keys shared by legacy and kitty encodings.
enum Functional {
    /// `CSI 1 ; m <letter>` / SS3 in application mode.
    Letter(u8),
    /// `CSI <n> ; m ~`
    Tilde(u32),
}

fn functional(key: Key) -> Option<Functional> {
    Some(match key {
        Key::Up => Functional::Letter(b'A'),
        Key::Down => Functional::Letter(b'B'),
        Key::Right => Functional::Letter(b'C'),
        Key::Left => Functional::Letter(b'D'),
        Key::Home => Functional::Letter(b'H'),
        Key::End => Functional::Letter(b'F'),
        Key::Insert => Functional::Tilde(2),
        Key::Delete => Functional::Tilde(3),
        Key::PageUp => Functional::Tilde(5),
        Key::PageDown => Functional::Tilde(6),
        Key::F(1) => Functional::Letter(b'P'),
        Key::F(2) => Functional::Letter(b'Q'),
        Key::F(3) => Functional::Tilde(13),
        Key::F(4) => Functional::Letter(b'S'),
        Key::F(n @ 5..=20) => Functional::Tilde(match n {
            5 => 15,
            6 => 17,
            7 => 18,
            8 => 19,
            9 => 20,
            10 => 21,
            11 => 23,
            12 => 24,
            13 => 25,
            14 => 26,
            15 => 28,
            16 => 29,
            17 => 31,
            18 => 32,
            19 => 33,
            _ => 34,
        }),
        _ => return None,
    })
}

fn encode_legacy(input: &KeyInput, mode: &InputMode) -> Option<Vec<u8>> {
    if input.event == KeyEventKind::Release {
        return None;
    }
    let m = input.mods;
    let mut out: Vec<u8> = Vec::new();
    if let Some(f) = functional(input.key) {
        let suffix = csi_mod_suffix(m);
        match f {
            Functional::Letter(l) => {
                // F1-F4 use SS3 unmodified; arrows use SS3 only in application mode.
                let is_fkey = matches!(input.key, Key::F(_));
                match suffix {
                    Some(s) => out.extend(format!("\x1b[1;{s}").bytes().chain([l])),
                    None if is_fkey || mode.app_cursor => out.extend([0x1b, b'O', l]),
                    None => out.extend([0x1b, b'[', l]),
                }
            }
            Functional::Tilde(n) => match suffix {
                Some(s) => out.extend(format!("\x1b[{n};{s}~").bytes()),
                None => out.extend(format!("\x1b[{n}~").bytes()),
            },
        }
        return Some(out);
    }
    let esc_prefix = |v: &mut Vec<u8>| {
        if m.alt {
            v.push(0x1b);
        }
    };
    match input.key {
        Key::Enter => {
            esc_prefix(&mut out);
            out.push(b'\r');
        }
        Key::Tab => {
            if m.shift {
                out.extend(b"\x1b[Z");
            } else {
                esc_prefix(&mut out);
                out.push(b'\t');
            }
        }
        Key::Backspace => {
            esc_prefix(&mut out);
            out.push(if m.ctrl { 0x08 } else { 0x7f });
        }
        Key::Escape => {
            esc_prefix(&mut out);
            out.push(0x1b);
        }
        Key::Char(c) => {
            if m.ctrl {
                let b = ctrl_byte(c)?;
                esc_prefix(&mut out);
                out.push(b);
            } else {
                let text: String = match (&input.text, m.alt) {
                    (Some(t), _) if !t.is_empty() => t.clone(),
                    _ if m.shift => c.to_uppercase().collect(),
                    _ => c.to_string(),
                };
                esc_prefix(&mut out);
                out.extend(text.bytes());
            }
        }
        _ => return None,
    }
    Some(out)
}

fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

fn encode_kitty(input: &KeyInput, mode: &InputMode) -> Option<Vec<u8>> {
    let k = mode.kitty;
    let m = input.mods;
    let ev = match input.event {
        KeyEventKind::Press => 1,
        KeyEventKind::Repeat => 2,
        KeyEventKind::Release => 3,
    };
    if ev != 1 && !k.report_events {
        // Repeats are sent as presses; releases are dropped.
        if ev == 3 {
            return None;
        }
    }
    let ev = if k.report_events { ev } else { 1 };
    let mods_field = |mods: Mods, ev: u32| -> String {
        let bits = mods.bits() + 1;
        match (bits, ev) {
            (1, 1) => String::new(),
            (b, 1) => format!(";{b}"),
            (b, e) => format!(";{b}:{e}"),
        }
    };

    // Functional keys keep their legacy final byte.
    if let Some(f) = functional(input.key) {
        let s = match f {
            Functional::Letter(l) => {
                let field = mods_field(m, ev);
                if field.is_empty() {
                    format!("\x1b[{}", l as char)
                } else {
                    format!("\x1b[1{field}{}", l as char)
                }
            }
            Functional::Tilde(n) => format!("\x1b[{n}{}~", mods_field(m, ev)),
        };
        return Some(s.into_bytes());
    }

    // Legacy-compatible keys stay legacy unless every key is reported as an escape code.
    let (code, legacy_ok): (u32, bool) = match input.key {
        Key::Enter => (13, true),
        Key::Tab => (9, true),
        Key::Backspace => (127, true),
        Key::Escape => (27, false),
        Key::Char(c) => (c.to_lowercase().next().unwrap_or(c) as u32, true),
        _ => return None,
    };
    let is_text_key = matches!(input.key, Key::Char(_));
    let non_shift_mods = Mods { shift: false, ..m }.bits() != 0;
    let needs_csi_u = k.report_all_keys || !legacy_ok || (non_shift_mods && k.disambiguate);
    // Text keys with only shift (or nothing) are plain text under disambiguate.
    if !needs_csi_u {
        if ev == 3 {
            return None;
        }
        return encode_legacy(
            &KeyInput {
                event: KeyEventKind::Press,
                ..input.clone()
            },
            mode,
        );
    }
    if ev == 3 && !k.report_events {
        return None;
    }
    // Enter/Tab/Backspace releases are only reported with report_all_keys.
    if ev == 3 && !is_text_key && !k.report_all_keys {
        return None;
    }

    let mut s = format!("\x1b[{code}");
    if k.report_alternates && m.shift {
        if let Some(t) = &input.text {
            if let Some(shifted) = t.chars().next() {
                if shifted as u32 != code {
                    s.push_str(&format!(":{}", shifted as u32));
                }
            }
        }
    }
    let field = mods_field(m, ev);
    let text_codes = if k.report_all_keys && k.report_text && ev != 3 {
        input.text.as_ref().filter(|t| !t.is_empty()).map(|t| {
            t.chars()
                .map(|c| (c as u32).to_string())
                .collect::<Vec<_>>()
                .join(":")
        })
    } else {
        None
    };
    match (field.is_empty(), text_codes) {
        (true, None) => {}
        (true, Some(t)) => s.push_str(&format!(";;{t}")),
        (false, None) => s.push_str(&field),
        (false, Some(t)) => s.push_str(&format!("{field};{t}")),
    }
    s.push('u');
    Some(s.into_bytes())
}

/// Wraps pasted text for the PTY.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // Strip ESC so pasted text cannot terminate the bracket early.
        let clean: String = text.chars().filter(|&c| c != '\x1b').collect();
        let mut out = b"\x1b[200~".to_vec();
        out.extend(clean.bytes());
        out.extend(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Whether a paste needs the multi-line confirmation (SPEC §6.3).
pub fn paste_needs_confirmation(text: &str, bracketed: bool, confirm_multiline: bool) -> bool {
    confirm_multiline && !bracketed && text.trim_end_matches(['\n', '\r']).contains(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(key: Key, mods: Mods) -> Vec<u8> {
        encode_key(&KeyInput::new(key, mods), &InputMode::default()).unwrap()
    }

    fn kitty_mode(f: KittyFlags) -> InputMode {
        InputMode {
            kitty: f,
            ..Default::default()
        }
    }

    const DIS: KittyFlags = KittyFlags {
        disambiguate: true,
        report_events: false,
        report_alternates: false,
        report_all_keys: false,
        report_text: false,
    };

    #[test]
    fn legacy_basics() {
        assert_eq!(legacy(Key::Char('a'), Mods::NONE), b"a");
        assert_eq!(legacy(Key::Char('a'), Mods::SHIFT), b"A");
        assert_eq!(legacy(Key::Char('c'), Mods::CTRL), [3]);
        assert_eq!(legacy(Key::Char('a'), Mods::ALT), b"\x1ba");
        assert_eq!(
            legacy(
                Key::Char('c'),
                Mods {
                    ctrl: true,
                    alt: true,
                    ..Mods::NONE
                }
            ),
            [0x1b, 3]
        );
        assert_eq!(legacy(Key::Char(' '), Mods::CTRL), [0]);
        assert_eq!(legacy(Key::Char('['), Mods::CTRL), [0x1b]);
        assert_eq!(legacy(Key::Enter, Mods::NONE), b"\r");
        assert_eq!(legacy(Key::Tab, Mods::NONE), b"\t");
        assert_eq!(legacy(Key::Tab, Mods::SHIFT), b"\x1b[Z");
        assert_eq!(legacy(Key::Backspace, Mods::NONE), [0x7f]);
        assert_eq!(legacy(Key::Backspace, Mods::ALT), [0x1b, 0x7f]);
        assert_eq!(legacy(Key::Escape, Mods::NONE), [0x1b]);
    }

    #[test]
    fn legacy_text_uses_layout_text() {
        let k = KeyInput::new(Key::Char('1'), Mods::SHIFT).with_text("!");
        assert_eq!(encode_key(&k, &InputMode::default()).unwrap(), b"!");
        let k = KeyInput::new(Key::Char('e'), Mods::NONE).with_text("é");
        assert_eq!(
            encode_key(&k, &InputMode::default()).unwrap(),
            "é".as_bytes()
        );
    }

    #[test]
    fn legacy_cursor_keys() {
        assert_eq!(legacy(Key::Up, Mods::NONE), b"\x1b[A");
        let app = InputMode {
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&KeyInput::new(Key::Up, Mods::NONE), &app).unwrap(),
            b"\x1bOA"
        );
        // Modifiers force the CSI form even in application mode.
        assert_eq!(
            encode_key(&KeyInput::new(Key::Left, Mods::CTRL), &app).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(legacy(Key::Right, Mods::SHIFT), b"\x1b[1;2C");
        assert_eq!(legacy(Key::Home, Mods::NONE), b"\x1b[H");
        assert_eq!(legacy(Key::End, Mods::NONE), b"\x1b[F");
    }

    #[test]
    fn legacy_tilde_and_function_keys() {
        assert_eq!(legacy(Key::Delete, Mods::NONE), b"\x1b[3~");
        assert_eq!(legacy(Key::Delete, Mods::CTRL), b"\x1b[3;5~");
        assert_eq!(legacy(Key::PageUp, Mods::NONE), b"\x1b[5~");
        assert_eq!(legacy(Key::PageDown, Mods::NONE), b"\x1b[6~");
        assert_eq!(legacy(Key::Insert, Mods::NONE), b"\x1b[2~");
        assert_eq!(legacy(Key::F(1), Mods::NONE), b"\x1bOP");
        assert_eq!(legacy(Key::F(4), Mods::NONE), b"\x1bOS");
        assert_eq!(legacy(Key::F(1), Mods::SHIFT), b"\x1b[1;2P");
        assert_eq!(legacy(Key::F(5), Mods::NONE), b"\x1b[15~");
        assert_eq!(legacy(Key::F(12), Mods::NONE), b"\x1b[24~");
        assert_eq!(legacy(Key::F(5), Mods::CTRL), b"\x1b[15;5~");
    }

    #[test]
    fn legacy_ignores_release() {
        let mut k = KeyInput::new(Key::Char('a'), Mods::NONE);
        k.event = KeyEventKind::Release;
        assert!(encode_key(&k, &InputMode::default()).is_none());
    }

    #[test]
    fn kitty_disambiguate() {
        let m = kitty_mode(DIS);
        let enc = |k: Key, mods: Mods| encode_key(&KeyInput::new(k, mods), &m).unwrap();
        assert_eq!(enc(Key::Escape, Mods::NONE), b"\x1b[27u");
        // Plain text and shifted text stay text.
        assert_eq!(enc(Key::Char('a'), Mods::NONE), b"a");
        assert_eq!(enc(Key::Char('a'), Mods::SHIFT), b"A");
        // Enter/Tab/Backspace stay legacy.
        assert_eq!(enc(Key::Enter, Mods::NONE), b"\r");
        assert_eq!(enc(Key::Backspace, Mods::NONE), [0x7f]);
        // Ctrl/Alt text keys become CSI u.
        assert_eq!(enc(Key::Char('c'), Mods::CTRL), b"\x1b[99;5u");
        assert_eq!(enc(Key::Char('a'), Mods::ALT), b"\x1b[97;3u");
        assert_eq!(enc(Key::Enter, Mods::CTRL), b"\x1b[13;5u");
        // Functional keys.
        assert_eq!(enc(Key::Up, Mods::NONE), b"\x1b[A");
        assert_eq!(enc(Key::Up, Mods::CTRL), b"\x1b[1;5A");
        assert_eq!(enc(Key::F(3), Mods::NONE), b"\x1b[13~");
        assert_eq!(enc(Key::F(3), Mods::SHIFT), b"\x1b[13;2~");
        assert_eq!(enc(Key::Delete, Mods::NONE), b"\x1b[3~");
    }

    #[test]
    fn kitty_all_keys_and_events() {
        let m = kitty_mode(KittyFlags {
            disambiguate: true,
            report_events: true,
            report_all_keys: true,
            ..Default::default()
        });
        let enc = |k: Key, mods: Mods, ev| {
            let mut i = KeyInput::new(k, mods);
            i.event = ev;
            encode_key(&i, &m)
        };
        assert_eq!(
            enc(Key::Char('a'), Mods::NONE, KeyEventKind::Press).unwrap(),
            b"\x1b[97u"
        );
        assert_eq!(
            enc(Key::Char('a'), Mods::NONE, KeyEventKind::Repeat).unwrap(),
            b"\x1b[97;1:2u"
        );
        assert_eq!(
            enc(Key::Char('a'), Mods::NONE, KeyEventKind::Release).unwrap(),
            b"\x1b[97;1:3u"
        );
        assert_eq!(
            enc(Key::Enter, Mods::NONE, KeyEventKind::Press).unwrap(),
            b"\x1b[13u"
        );
        assert_eq!(
            enc(Key::Up, Mods::CTRL, KeyEventKind::Release).unwrap(),
            b"\x1b[1;5:3A"
        );
    }

    #[test]
    fn kitty_associated_text_and_alternates() {
        let m = kitty_mode(KittyFlags {
            disambiguate: true,
            report_all_keys: true,
            report_text: true,
            report_alternates: true,
            ..Default::default()
        });
        let k = KeyInput::new(Key::Char('a'), Mods::SHIFT).with_text("A");
        assert_eq!(encode_key(&k, &m).unwrap(), b"\x1b[97:65;2;65u");
        let k = KeyInput::new(Key::Char('a'), Mods::NONE).with_text("a");
        assert_eq!(encode_key(&k, &m).unwrap(), b"\x1b[97;;97u");
    }

    #[test]
    fn kitty_release_without_event_flag_dropped() {
        let m = kitty_mode(DIS);
        let mut k = KeyInput::new(Key::Char('c'), Mods::CTRL);
        k.event = KeyEventKind::Release;
        assert!(encode_key(&k, &m).is_none());
    }

    #[test]
    fn paste_encoding() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(encode_paste("a\r\nb", false), b"a\rb");
        assert_eq!(encode_paste("hi", true), b"\x1b[200~hi\x1b[201~");
        assert_eq!(
            encode_paste("x\x1b[201~y", true),
            b"\x1b[200~x[201~y\x1b[201~"
        );
    }

    #[test]
    fn paste_confirmation() {
        assert!(paste_needs_confirmation("a\nb", false, true));
        assert!(!paste_needs_confirmation("a\nb", true, true));
        assert!(!paste_needs_confirmation("a\nb", false, false));
        assert!(!paste_needs_confirmation("single", false, true));
        assert!(!paste_needs_confirmation("single\n", false, true));
    }
}
