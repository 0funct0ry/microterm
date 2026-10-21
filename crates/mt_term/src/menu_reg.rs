//! Shell-registered context-menu items: `OSC 7717 ; menu ; <verb> ; …` (SPEC §10.9.7).
//!
//! A program in a pane registers pane-scoped items for the `viewport` menu. The payload is
//! untrusted terminal output, so everything here is defensive: sizes are capped, control
//! characters are stripped from labels, malformed input is ignored and counted, and a fixed
//! one-second window limits how often a pane may change its items.

use std::fmt::Write as _;

/// Items one pane may hold.
pub const MAX_ITEMS: usize = 16;
/// Label length in bytes (after stripping controls).
pub const MAX_LABEL: usize = 256;
/// Decoded payload length in bytes.
pub const MAX_PAYLOAD: usize = 1024;
/// `add`/`clear` commands accepted per pane and second.
pub const RATE_PER_SEC: u32 = 10;
const WINDOW_MS: u64 = 1000;
const MAX_ID: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegKind {
    /// Text typed into the pane, followed by Enter.
    Send,
    /// A `microterm ctl` argv string.
    Ctl,
    /// A JSON argv array run in a new split.
    Spawn,
}

impl RegKind {
    pub fn name(self) -> &'static str {
        match self {
            RegKind::Send => "send",
            RegKind::Ctl => "ctl",
            RegKind::Spawn => "spawn",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "send" => Some(RegKind::Send),
            "ctl" => Some(RegKind::Ctl),
            "spawn" => Some(RegKind::Spawn),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegItem {
    pub id: String,
    pub label: String,
    pub kind: RegKind,
    pub payload: String,
}

/// One decoded `OSC 7717;menu;…` sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegCmd {
    /// `replay` is set on the daemon's synthesized stream: it is exempt from the rate window
    /// (the item cap still applies).
    Add {
        item: RegItem,
        replay: bool,
    },
    Clear(Option<String>),
    Malformed,
}

/// Parses the OSC payload (everything after the OSC number and its `;`): `menu;<verb>;…`.
/// `None` when it is not a menu sequence at all.
pub fn parse(payload: &[u8]) -> Option<RegCmd> {
    let rest = payload.strip_prefix(b"menu;")?;
    Some(parse_verb(rest).unwrap_or(RegCmd::Malformed))
}

fn parse_verb(rest: &[u8]) -> Option<RegCmd> {
    if rest.len() > MAX_PAYLOAD * 2 + 512 {
        return None;
    }
    let s = std::str::from_utf8(rest).ok()?;
    let mut f = s.split(';');
    match f.next()? {
        "add" => {
            let id = f.next()?;
            let label = f.next()?;
            let kind = RegKind::parse(f.next()?)?;
            let payload = f.next()?;
            let replay = match f.next() {
                None => false,
                Some("replay") => true,
                Some(_) => return None,
            };
            if f.next().is_some() || !valid_id(id) {
                return None;
            }
            let label = clean_label(&String::from_utf8(b64_decode(label)?).ok()?);
            let payload = String::from_utf8(b64_decode(payload)?).ok()?;
            if label.is_empty()
                || label.len() > MAX_LABEL
                || payload.is_empty()
                || payload.len() > MAX_PAYLOAD
            {
                return None;
            }
            Some(RegCmd::Add {
                item: RegItem {
                    id: id.to_string(),
                    label,
                    kind,
                    payload,
                },
                replay,
            })
        }
        "clear" => match (f.next(), f.next()) {
            (None, _) | (Some(""), None) => Some(RegCmd::Clear(None)),
            (Some(id), None) if valid_id(id) => Some(RegCmd::Clear(Some(id.to_string()))),
            _ => None,
        },
        _ => None,
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Strips C0/C1 controls, ESC and DEL; the label is shown in app chrome.
fn clean_label(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// The items one pane has registered, with the counters SPEC §10.9.7 names.
#[derive(Clone, Debug, Default)]
pub struct MenuRegistry {
    items: Vec<RegItem>,
    window_start: u64,
    window_count: u32,
    dropped: u64,
    malformed: u64,
}

impl MenuRegistry {
    pub fn items(&self) -> &[RegItem] {
        &self.items
    }

    /// Commands dropped by the rate window.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Sequences ignored as malformed (or refused for exceeding the item cap).
    pub fn malformed(&self) -> u64 {
        self.malformed
    }

    /// Removes everything (shell exit, `clear`, RIS). Counters are kept.
    pub fn reset(&mut self) -> bool {
        let changed = !self.items.is_empty();
        self.items.clear();
        changed
    }

    /// Applies one command at time `now_ms`. Returns whether the item list changed.
    pub fn apply(&mut self, cmd: RegCmd, now_ms: u64) -> bool {
        let exempt = matches!(cmd, RegCmd::Add { replay: true, .. });
        if matches!(cmd, RegCmd::Malformed) {
            self.malformed += 1;
            return false;
        }
        if !exempt {
            if now_ms.saturating_sub(self.window_start) >= WINDOW_MS || self.window_count == 0 {
                self.window_start = now_ms;
                self.window_count = 0;
            }
            if self.window_count >= RATE_PER_SEC {
                self.dropped += 1;
                return false;
            }
            self.window_count += 1;
        }
        match cmd {
            RegCmd::Add { item, .. } => {
                if let Some(slot) = self.items.iter_mut().find(|i| i.id == item.id) {
                    let changed = *slot != item;
                    *slot = item;
                    changed
                } else if self.items.len() >= MAX_ITEMS {
                    self.malformed += 1;
                    false
                } else {
                    self.items.push(item);
                    true
                }
            }
            RegCmd::Clear(None) => self.reset(),
            RegCmd::Clear(Some(id)) => {
                let before = self.items.len();
                self.items.retain(|i| i.id != id);
                self.items.len() != before
            }
            RegCmd::Malformed => false,
        }
    }

    /// The sequences that rebuild this registry, for the daemon's replay stream.
    pub fn replay(&self, out: &mut String) {
        for i in &self.items {
            let _ = write!(
                out,
                "\x1b]7717;menu;add;{};{};{};{};replay\x07",
                i.id,
                b64_encode(i.label.as_bytes()),
                i.kind.name(),
                b64_encode(i.payload.as_bytes())
            );
        }
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for b in s.bytes() {
        let v = ALPHABET.iter().position(|a| *a == b)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(id: &str, label: &str, kind: &str, payload: &str) -> Vec<u8> {
        format!(
            "menu;add;{id};{};{kind};{}",
            b64_encode(label.as_bytes()),
            b64_encode(payload.as_bytes())
        )
        .into_bytes()
    }

    fn item(id: &str) -> RegItem {
        RegItem {
            id: id.into(),
            label: id.into(),
            kind: RegKind::Send,
            payload: "x".into(),
        }
    }

    fn add_cmd(id: &str) -> RegCmd {
        RegCmd::Add {
            item: item(id),
            replay: false,
        }
    }

    #[test]
    fn base64_round_trips() {
        for s in ["", "a", "ab", "abc", "make test; echo \"é\"\n"] {
            assert_eq!(b64_decode(&b64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert!(b64_decode("a*bc").is_none());
        assert!(b64_decode("abcde").is_none());
    }

    #[test]
    fn parses_add_and_clear() {
        let c = parse(&add("tests", "Run tests", "send", "make test")).unwrap();
        assert_eq!(
            c,
            RegCmd::Add {
                item: RegItem {
                    id: "tests".into(),
                    label: "Run tests".into(),
                    kind: RegKind::Send,
                    payload: "make test".into()
                },
                replay: false
            }
        );
        assert_eq!(parse(b"menu;clear"), Some(RegCmd::Clear(None)));
        assert_eq!(
            parse(b"menu;clear;a-1"),
            Some(RegCmd::Clear(Some("a-1".into())))
        );
        assert_eq!(parse(b"other;add"), None);
    }

    #[test]
    fn labels_lose_control_characters() {
        let c = parse(&add("a", "Dep\x1b[31mloy\n\u{85}", "send", "x")).unwrap();
        let RegCmd::Add { item, .. } = c else {
            panic!()
        };
        assert_eq!(item.label, "Dep[31mloy");
    }

    #[test]
    fn malformed_payloads_are_flagged() {
        let bad: Vec<Vec<u8>> = vec![
            b"menu;".to_vec(),
            b"menu;frob".to_vec(),
            b"menu;add;A;eA==;send;eA==".to_vec(), // uppercase id
            b"menu;add;a;eA==;exec;eA==".to_vec(), // unknown kind
            b"menu;add;a;***;send;eA==".to_vec(),  // bad base64
            b"menu;add;a;eA==;send".to_vec(),      // missing payload
            b"menu;add;a;;send;eA==".to_vec(),     // empty label
            b"menu;add;a;eA==;send;eA==;nope".to_vec(),
            b"menu;clear;Bad Id".to_vec(),
            add("a", "x", "send", &"p".repeat(MAX_PAYLOAD + 1)),
            add("a", &"l".repeat(MAX_LABEL + 1), "send", "x"),
            add(&"i".repeat(65), "x", "send", "x"),
        ];
        for b in bad {
            assert_eq!(
                parse(&b),
                Some(RegCmd::Malformed),
                "{}",
                String::from_utf8_lossy(&b)
            );
        }
        assert_eq!(
            parse(
                &vec![b'a'; 100_000]
                    .iter()
                    .fold(b"menu;add;".to_vec(), |mut v, b| {
                        v.push(*b);
                        v
                    })
            ),
            Some(RegCmd::Malformed)
        );
    }

    #[test]
    fn limits_replace_and_count() {
        let mut r = MenuRegistry::default();
        for i in 0..MAX_ITEMS {
            // Spread over time so the rate window does not interfere.
            assert!(r.apply(add_cmd(&format!("i{i}")), i as u64 * 1000));
        }
        assert!(!r.apply(add_cmd("extra"), 99_000));
        assert_eq!(r.items().len(), MAX_ITEMS);
        assert_eq!(r.malformed(), 1);
        // Same id replaces without growing, and an identical add is no change.
        let mut changed = item("i0");
        changed.label = "new".into();
        assert!(r.apply(
            RegCmd::Add {
                item: changed.clone(),
                replay: false
            },
            100_000
        ));
        assert!(!r.apply(
            RegCmd::Add {
                item: changed,
                replay: false
            },
            100_001
        ));
        assert!(r.apply(RegCmd::Clear(Some("i1".into())), 100_002));
        assert_eq!(r.items().len(), MAX_ITEMS - 1);
        assert!(r.apply(RegCmd::Clear(None), 100_003));
        assert!(r.items().is_empty());
        assert!(!r.apply(RegCmd::Malformed, 0));
        assert_eq!(r.malformed(), 2);
    }

    #[test]
    fn rate_window_drops_and_recovers() {
        let mut r = MenuRegistry::default();
        for i in 0..RATE_PER_SEC {
            assert!(r.apply(add_cmd(&format!("i{i}")), 5000 + i as u64));
        }
        assert!(!r.apply(add_cmd("late"), 5500));
        assert_eq!(r.dropped(), 1);
        assert!(r.apply(add_cmd("late"), 6001), "a new window opens");
        // Replay is exempt from the window.
        for i in 0..5 {
            let c = RegCmd::Add {
                item: item(&format!("r{i}")),
                replay: true,
            };
            r.apply(c, 6002);
        }
        assert_eq!(r.dropped(), 1);
    }

    #[test]
    fn replay_stream_rebuilds_the_registry() {
        let mut r = MenuRegistry::default();
        let mut it = item("deploy");
        it.label = "Deploy; staging".into();
        it.payload = "make deploy\n".into();
        r.apply(
            RegCmd::Add {
                item: it.clone(),
                replay: false,
            },
            0,
        );
        let mut s = String::new();
        r.replay(&mut s);
        let body = s
            .strip_prefix("\x1b]7717;")
            .unwrap()
            .strip_suffix('\x07')
            .unwrap();
        let mut r2 = MenuRegistry::default();
        r2.apply(parse(body.as_bytes()).unwrap(), 0);
        assert_eq!(r2.items(), &[it]);
    }
}
