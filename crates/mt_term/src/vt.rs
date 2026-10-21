//! Sequences alacritty's parser does not model (SPEC §6.1, M10.6): the per-pane colour store
//! (OSC 5/13–19/21 and replies for 4/10–12), OSC 9 notifications and 9;4 progress, OSC 22
//! pointer shape and XTSHIFTESCAPE.
//!
//! [`crate::commands::OscSniffer`] finds them in the raw byte stream (across read boundaries,
//! payloads capped at 100 KiB) and [`crate::commands::drive`] applies them between parser calls,
//! so every feeder (local PTY, daemon-fed remote pane, `Emulator`, `HeadlessPane`) shares one
//! code path. The same bytes still reach the engine, which keeps handling the sets and resets it
//! already understands (OSC 4/10/11/12/104/110–112). The engine's own query replies are
//! suppressed in favour of the ones here, which read the engine's overrides and honour
//! `terminal.osc_color_report`.
//!
//! The interceptor is the hook M14 reuses for APC G and OSC 1337.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;

use crate::events::{ProgressState, Shared, TermEvent};

/// Longest OSC payload buffered (same cap as OSC 52, SPEC §6.3).
pub const MAX_OSC_PAYLOAD: usize = 100 * 1024;
/// Longest OSC 9 notification text kept.
const MAX_NOTIFY: usize = 4096;

/// A decoded sequence handled here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VtSeq {
    /// `OSC code ; payload` (payload may be empty). `st` is true when the request ended in
    /// `ESC \`, false for BEL; replies use the same terminator.
    Osc {
        code: u16,
        payload: String,
        st: bool,
    },
    /// `CSI > Pn s` (XTSHIFTESCAPE).
    ShiftEscape(u8),
    /// `CSI ? … 1016 … h` / `l`: SGR-pixels mouse reporting turned on or off. The engine does
    /// not know the mode, so it is tracked here to answer DECRQM.
    PixelMouse(bool),
    /// `CSI ? 1016 $ p` (DECRQM for SGR-pixels).
    QueryPixelMouse,
    /// `CSI > q` / `CSI > 0 q` (XTVERSION): who is this terminal?
    Xtversion,
}

/// OSC codes the interceptor forwards to [`apply`].
pub(crate) fn wants_osc(code: u16) -> bool {
    matches!(code, 4 | 5 | 9 | 10..=19 | 21 | 22 | 104 | 105 | 110..=119)
}

/// `terminal.osc_color_report`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorReport {
    #[default]
    Bit16,
    Bit8,
    None,
}

impl ColorReport {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "16-bit" => Some(Self::Bit16),
            "8-bit" => Some(Self::Bit8),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// The `[terminal]` config values the interceptor needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VtConfig {
    pub report: ColorReport,
    pub osc_notifications: bool,
    pub allow_shift_capture: bool,
}

impl Default for VtConfig {
    fn default() -> Self {
        VtConfig {
            report: ColorReport::Bit16,
            osc_notifications: true,
            allow_shift_capture: true,
        }
    }
}

/// Per-pane overrides a program set explicitly. Reset by RIS.
#[derive(Clone, Debug, Default)]
pub(crate) struct VtState {
    pub cfg: VtConfig,
    /// OSC 5 / 105: bold, underline, blink, reverse, italic.
    special: [Option<[u8; 3]>; 5],
    /// OSC 13–19 (index = code − 13): pointer fg/bg, Tektronix fg/bg, highlight bg, Tektronix
    /// cursor, highlight fg.
    dynamic: [Option<[u8; 3]>; 7],
    /// Kitty `cursor_text`.
    cursor_text: Option<[u8; 3]>,
    /// Other Kitty keys (`visual_bell`, `transparent_background_colorN`): stored and reported.
    extra: BTreeMap<String, [u8; 3]>,
    shift_capture: bool,
    pointer: Option<String>,
    had_mouse: bool,
    /// DECSET 1016, for DECRQM.
    pixel_mouse: bool,
}

/// What the renderer and mouse code need from [`VtState`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VtView {
    /// OSC 17 / Kitty `selection_background`.
    pub selection_bg: Option<[u8; 3]>,
    /// OSC 19 / Kitty `selection_foreground`.
    pub selection_fg: Option<[u8; 3]>,
    /// OSC 5;0: colour of bold text.
    pub bold_color: Option<[u8; 3]>,
    /// OSC 5;1: colour of underlines.
    pub underline_color: Option<[u8; 3]>,
    /// XTSHIFTESCAPE 1 is active and allowed.
    pub shift_capture: bool,
    /// OSC 22 name (`None` = default).
    pub pointer_shape: Option<String>,
}

impl VtState {
    pub(crate) fn view(&self) -> VtView {
        VtView {
            selection_bg: self.dynamic[4],
            selection_fg: self.dynamic[6],
            bold_color: self.special[0],
            underline_color: self.special[1],
            shift_capture: self.shift_capture && self.cfg.allow_shift_capture,
            pointer_shape: self.pointer.clone(),
        }
    }

    /// Everything but the config goes back to defaults.
    fn reset(&mut self) {
        *self = VtState {
            cfg: self.cfg,
            ..Default::default()
        };
    }

    /// Bytes that restore this state in a fresh terminal (after its RIS).
    pub(crate) fn replay(&self, out: &mut String) {
        for (i, c) in self.special.iter().enumerate() {
            if let Some(c) = c {
                let _ = write!(out, "\x1b]5;{i};{}\x07", fmt8(*c));
            }
        }
        for (i, c) in self.dynamic.iter().enumerate() {
            if let Some(c) = c {
                let _ = write!(out, "\x1b]{};{}\x07", 13 + i, fmt8(*c));
            }
        }
        if let Some(c) = self.cursor_text {
            let _ = write!(out, "\x1b]21;cursor_text={}\x07", fmt8(c));
        }
        for (k, c) in &self.extra {
            let _ = write!(out, "\x1b]21;{k}={}\x07", fmt8(*c));
        }
        if self.shift_capture {
            out.push_str("\x1b[>1s");
        }
        if let Some(p) = &self.pointer {
            let _ = write!(out, "\x1b]22;{p}\x07");
        }
    }
}

fn fmt8(c: [u8; 3]) -> String {
    format!("rgb:{:02x}/{:02x}/{:02x}", c[0], c[1], c[2])
}

fn fmt16(c: [u8; 3]) -> String {
    format!(
        "rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}",
        c[0], c[0], c[1], c[1], c[2], c[2]
    )
}

/// Parses `rgb:h/h/h` (1–4 hex digits per component) and `#RGB` … `#RRRRGGGGBBBB`. A Kitty
/// `@alpha` suffix is ignored.
pub(crate) fn parse_color(s: &str) -> Option<[u8; 3]> {
    let s = s.split('@').next()?.trim();
    let scale = |digits: &str| -> Option<u8> {
        if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(digits, 16).ok()?;
        let max = (1u32 << (4 * digits.len())) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    if let Some(rest) = s.strip_prefix("rgb:") {
        let mut it = rest.split('/');
        let (r, g, b) = (it.next()?, it.next()?, it.next()?);
        if it.next().is_some() {
            return None;
        }
        return Some([scale(r)?, scale(g)?, scale(b)?]);
    }
    let h = s.strip_prefix('#')?;
    if h.is_empty() || h.len() % 3 != 0 || h.len() > 12 || !h.is_ascii() {
        return None;
    }
    let n = h.len() / 3;
    let msb = |d: &str| -> Option<u8> {
        let v = u32::from_str_radix(d, 16).ok()?;
        Some(((v << (4 * (4 - n))) >> 8) as u8)
    };
    Some([msb(&h[..n])?, msb(&h[n..2 * n])?, msb(&h[2 * n..])?])
}

/// A colour value as OSC 21 takes it: a numeric spec or an X11 name (Kitty, SPEC §6.1).
fn parse_kitty_color(s: &str) -> Option<[u8; 3]> {
    parse_color(s).or_else(|| crate::color_names::lookup(s.split('@').next()?))
}

fn b64_nopad(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for ch in s.as_bytes().chunks(3) {
        let n = ch.len();
        let v = (ch[0] as u32) << 16
            | (*ch.get(1).unwrap_or(&0) as u32) << 8
            | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..=n {
            out.push(T[((v >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

pub(crate) fn inject_str<T: EventListener>(term: &mut Term<T>, s: &str) {
    inject(term, s);
}

fn inject<T: EventListener>(term: &mut Term<T>, s: &str) {
    let mut parser: Processor = Processor::default();
    parser.advance(term, s.as_bytes());
}

/// Current value of colour slot `idx` (0–255 palette, 256 fg, 257 bg, 258 cursor): the program's
/// override if there is one, else the theme.
fn current<T>(term: &Term<T>, shared: &Shared, idx: usize) -> Option<[u8; 3]> {
    if let Some(c) = term.colors()[idx] {
        return Some([c.r, c.g, c.b]);
    }
    shared.palette.lock().unwrap().lookup(idx)
}

fn send(shared: &Shared, report: ColorReport, st: bool, body: &str) {
    if report == ColorReport::None {
        return;
    }
    let t = if st { "\x1b\\" } else { "\x07" };
    shared.write_pty(format!("\x1b]{body}{t}").into_bytes());
}

fn fmt(report: ColorReport, c: [u8; 3]) -> String {
    if report == ColorReport::Bit8 {
        fmt8(c)
    } else {
        fmt16(c)
    }
}

/// Applies one sequence at the byte offset where it ended.
pub(crate) fn apply<T: EventListener>(term: &mut Term<T>, shared: &Shared, seq: VtSeq) {
    match seq {
        VtSeq::ShiftEscape(n) => {
            let mut vt = shared.vt.lock().unwrap();
            if vt.cfg.allow_shift_capture {
                match n {
                    0 => vt.shift_capture = false,
                    1 => vt.shift_capture = true,
                    _ => {}
                }
            }
        }
        VtSeq::PixelMouse(on) => shared.vt.lock().unwrap().pixel_mouse = on,
        VtSeq::QueryPixelMouse => {
            // 1 set, 2 reset (DECRQM). The engine answers 0 for modes it does not know, and
            // `Listener` drops that reply.
            let state = if shared.vt.lock().unwrap().pixel_mouse {
                1
            } else {
                2
            };
            shared.write_pty(format!("\x1b[?1016;{state}$y").into_bytes());
        }
        VtSeq::Xtversion => {
            // `DCS > | name version ST`, the form xterm, Kitty and Ghostty use.
            let reply = format!("\x1bP>|microterm {}\x1b\\", env!("CARGO_PKG_VERSION"));
            shared.write_pty(reply.into_bytes());
        }
        VtSeq::Osc { code, payload, st } => osc(term, shared, code, &payload, st),
    }
}

/// RIS: forget every override, in the store and in the engine.
pub(crate) fn reset<T: EventListener>(term: &mut Term<T>, shared: &Shared) {
    shared.vt.lock().unwrap().reset();
    inject(term, "\x1b]104\x07\x1b]110\x07\x1b]111\x07\x1b]112\x07");
    shared.notify(TermEvent::Progress {
        state: ProgressState::Remove,
        value: None,
    });
}

/// Called after each parsed segment: a pointer shape ends when mouse reporting does.
pub(crate) fn after_segment<T>(term: &Term<T>, shared: &Shared) {
    let on = term.mode().intersects(TermMode::MOUSE_MODE);
    let mut vt = shared.vt.lock().unwrap();
    if vt.had_mouse && !on {
        vt.pointer = None;
    }
    vt.had_mouse = on;
}

fn osc<T: EventListener>(term: &mut Term<T>, shared: &Shared, code: u16, payload: &str, st: bool) {
    let report = shared.vt.lock().unwrap().cfg.report;
    match code {
        4 => {
            let parts: Vec<&str> = payload.split(';').collect();
            for pair in parts.chunks_exact(2) {
                if pair[1] != "?" {
                    continue;
                }
                if let Some(i) = pair[0].parse::<usize>().ok().filter(|i| *i < 256) {
                    if let Some(c) = current(term, shared, i) {
                        send(shared, report, st, &format!("4;{i};{}", fmt(report, c)));
                    }
                }
            }
        }
        5 => {
            let parts: Vec<&str> = payload.split(';').collect();
            for pair in parts.chunks_exact(2) {
                let Some(i) = pair[0].parse::<usize>().ok().filter(|i| *i < 5) else {
                    continue;
                };
                if pair[1] == "?" {
                    let c =
                        shared.vt.lock().unwrap().special[i].or_else(|| current(term, shared, 256));
                    if let Some(c) = c {
                        send(shared, report, st, &format!("5;{i};{}", fmt(report, c)));
                    }
                } else if let Some(c) = parse_color(pair[1]) {
                    shared.vt.lock().unwrap().special[i] = Some(c);
                }
            }
        }
        105 => {
            let mut vt = shared.vt.lock().unwrap();
            if payload.is_empty() {
                vt.special = Default::default();
            } else {
                for p in payload.split(';') {
                    if let Some(i) = p.parse::<usize>().ok().filter(|i| *i < 5) {
                        vt.special[i] = None;
                    }
                }
            }
        }
        10..=19 => {
            for (n, p) in payload.split(';').enumerate() {
                let c = code + n as u16;
                if c > 19 {
                    break;
                }
                if p == "?" {
                    let col = match c {
                        10..=12 => current(term, shared, 256 + (c - 10) as usize),
                        _ => shared.vt.lock().unwrap().dynamic[(c - 13) as usize].or_else(|| {
                            // Unset: foreground for the "foreground" slots, background for
                            // the rest (highlight colours default to reverse video).
                            let idx = match c {
                                13 | 15 | 17 => 256,
                                18 => 258,
                                _ => 257,
                            };
                            current(term, shared, idx)
                        }),
                    };
                    if let Some(col) = col {
                        send(shared, report, st, &format!("{c};{}", fmt(report, col)));
                    }
                } else if c >= 13 {
                    if let Some(col) = parse_color(p) {
                        shared.vt.lock().unwrap().dynamic[(c - 13) as usize] = Some(col);
                    }
                }
            }
        }
        113..=119 => {
            shared.vt.lock().unwrap().dynamic[(code - 113) as usize] = None;
        }
        21 => kitty(term, shared, payload, st, report),
        22 => {
            let name: String = payload
                .split(',')
                .next()
                .unwrap_or("")
                .trim_start_matches(['=', '>', '<'])
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                .take(64)
                .collect();
            shared.vt.lock().unwrap().pointer = if name.is_empty() { None } else { Some(name) };
        }
        9 => osc9(shared, payload),
        _ => {}
    }
}

fn osc9(shared: &Shared, payload: &str) {
    // ConEmu subcommand: `N` or `N;…` with N in 1..=12.
    let head = payload.split(';').next().unwrap_or("");
    let sub = head
        .parse::<u8>()
        .ok()
        .filter(|n| (1..=12).contains(n) && head.len() <= 2);
    match sub {
        Some(4) => {
            let mut it = payload.split(';').skip(1);
            let Some(state) = it.next().and_then(|s| s.parse::<u8>().ok()) else {
                return;
            };
            let value = it
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .map(|v| v.min(100) as u8);
            let (state, value) = match state {
                0 => (ProgressState::Remove, None),
                1 => (ProgressState::Normal, Some(value.unwrap_or(0))),
                2 => (ProgressState::Error, value),
                3 => (ProgressState::Indeterminate, None),
                4 => (ProgressState::Paused, value),
                _ => return,
            };
            shared.notify(TermEvent::Progress { state, value });
        }
        Some(_) => {}
        None => {
            if !shared.vt.lock().unwrap().cfg.osc_notifications || payload.trim().is_empty() {
                return;
            }
            let mut text = payload.to_string();
            if text.len() > MAX_NOTIFY {
                let mut cut = MAX_NOTIFY;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
            }
            shared.notify(TermEvent::Notify(text));
        }
    }
}

/// OSC 21, the Kitty colour protocol.
fn kitty<T: EventListener>(
    term: &mut Term<T>,
    shared: &Shared,
    payload: &str,
    st: bool,
    report: ColorReport,
) {
    let mut reply: Vec<String> = Vec::new();
    for item in payload.split(';') {
        let item: String = item.chars().filter(|c| !c.is_whitespace()).collect();
        if item.is_empty() {
            continue;
        }
        let (key, value) = match item.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (item.clone(), None),
        };
        // Engine slot for keys that live there: palette index, or 256/257/258.
        let slot = match key.as_str() {
            "foreground" => Some(256),
            "background" => Some(257),
            "cursor" => Some(258),
            k => k.parse::<usize>().ok().filter(|i| *i < 256),
        };
        let known = slot.is_some()
            || matches!(
                key.as_str(),
                "selection_foreground" | "selection_background" | "cursor_text" | "visual_bell"
            )
            || key
                .strip_prefix("transparent_background_color")
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7"));
        if !known {
            reply.push(format!("unknown={}", b64_nopad(&key)));
            continue;
        }
        match value.as_deref() {
            // Query.
            Some("?") => {
                let c = match slot {
                    Some(s) => current(term, shared, s),
                    None => {
                        let vt = shared.vt.lock().unwrap();
                        match key.as_str() {
                            "selection_foreground" => vt.dynamic[6],
                            "selection_background" => vt.dynamic[4],
                            "cursor_text" => vt.cursor_text,
                            k => vt.extra.get(k).copied(),
                        }
                    }
                };
                reply.push(format!("{key}={}", c.map(fmt8).unwrap_or_default()));
            }
            // Set.
            Some(v) if !v.is_empty() => {
                let Some(c) = parse_kitty_color(v) else {
                    continue;
                };
                match (slot, key.as_str()) {
                    (Some(256), _) => inject(term, &format!("\x1b]10;{}\x07", fmt8(c))),
                    (Some(257), _) => inject(term, &format!("\x1b]11;{}\x07", fmt8(c))),
                    (Some(258), _) => inject(term, &format!("\x1b]12;{}\x07", fmt8(c))),
                    (Some(i), _) => inject(term, &format!("\x1b]4;{i};{}\x07", fmt8(c))),
                    (None, k) => {
                        let mut vt = shared.vt.lock().unwrap();
                        match k {
                            "selection_foreground" => vt.dynamic[6] = Some(c),
                            "selection_background" => vt.dynamic[4] = Some(c),
                            "cursor_text" => vt.cursor_text = Some(c),
                            k => {
                                vt.extra.insert(k.to_string(), c);
                            }
                        }
                    }
                }
            }
            // `key=` (dynamic) and a bare key (reset to default) both drop the override: a
            // dynamic colour has no fixed value, so there is nothing to render or report.
            _ => match (slot, key.as_str()) {
                (Some(256), _) => inject(term, "\x1b]110\x07"),
                (Some(257), _) => inject(term, "\x1b]111\x07"),
                (Some(258), _) => inject(term, "\x1b]112\x07"),
                (Some(i), _) => inject(term, &format!("\x1b]104;{i}\x07")),
                (None, k) => {
                    let mut vt = shared.vt.lock().unwrap();
                    match k {
                        "selection_foreground" => vt.dynamic[6] = None,
                        "selection_background" => vt.dynamic[4] = None,
                        "cursor_text" => vt.cursor_text = None,
                        k => {
                            vt.extra.remove(k);
                        }
                    }
                }
            },
        }
    }
    if !reply.is_empty() {
        send(shared, report, st, &format!("21;{}", reply.join(";")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_specs() {
        assert_eq!(parse_color("rgb:ff/00/80"), Some([255, 0, 128]));
        assert_eq!(parse_color("rgb:ffff/0000/8080"), Some([255, 0, 128]));
        assert_eq!(parse_color("rgb:f/0/8"), Some([255, 0, 136]));
        assert_eq!(parse_color("#3a7"), Some([0x30, 0xa0, 0x70]));
        assert_eq!(parse_color("#102030"), Some([0x10, 0x20, 0x30]));
        assert_eq!(parse_color("#100020003000"), Some([0x10, 0x20, 0x30]));
        assert_eq!(parse_color("rgb:ff/00"), None);
        assert_eq!(parse_color("rgb:fffff/00/00"), None);
        assert_eq!(parse_color("nonsense"), None);
        assert_eq!(parse_color("#12"), None);
    }

    #[test]
    fn base64_matches_the_kitty_example() {
        assert_eq!(b64_nopad("nonsense"), "bm9uc2Vuc2U");
        assert_eq!(b64_nopad("a"), "YQ");
    }
}
