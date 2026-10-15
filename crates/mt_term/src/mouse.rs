//! Mouse report encoding (SPEC §6.1): X10-style legacy, SGR (1006) and SGR-pixels (1016),
//! for tracking modes 1000 (click), 1002 (drag) and 1003 (motion).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseTracking {
    #[default]
    None,
    /// 1000: press and release.
    Click,
    /// 1002: plus motion while a button is held.
    Drag,
    /// 1003: plus all motion.
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseModes {
    pub tracking: MouseTracking,
    /// DECSET 1006.
    pub sgr: bool,
    /// DECSET 1016 (implies SGR framing, with pixel coordinates).
    pub pixels: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// Motion with no button held.
    Unpressed,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub button: MouseButton,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// 0-based cell.
    pub col: usize,
    pub row: usize,
    /// 0-based pixel offset inside the grid (used for 1016).
    pub px: usize,
    pub py: usize,
}

impl MouseEvent {
    pub fn new(kind: MouseEventKind, button: MouseButton, col: usize, row: usize) -> Self {
        Self {
            kind,
            button,
            shift: false,
            alt: false,
            ctrl: false,
            col,
            row,
            px: 0,
            py: 0,
        }
    }
}

/// Encodes `ev` for the active modes, or `None` when it must not be reported.
pub fn encode_mouse(ev: &MouseEvent, modes: &MouseModes) -> Option<Vec<u8>> {
    if modes.tracking == MouseTracking::None {
        return None;
    }
    let wheel = matches!(
        ev.button,
        MouseButton::WheelUp
            | MouseButton::WheelDown
            | MouseButton::WheelLeft
            | MouseButton::WheelRight
    );
    let sgr = modes.sgr || modes.pixels;
    let mut code: u32 = match ev.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::Unpressed => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    match ev.kind {
        MouseEventKind::Press => {}
        MouseEventKind::Release => {
            if wheel {
                return None;
            }
            if !sgr {
                code = 3;
            }
        }
        MouseEventKind::Motion => {
            let held = ev.button != MouseButton::Unpressed;
            match modes.tracking {
                MouseTracking::Click => return None,
                MouseTracking::Drag if !held => return None,
                _ => {}
            }
            if wheel {
                return None;
            }
            code += 32;
        }
    }
    if ev.shift {
        code += 4;
    }
    if ev.alt {
        code += 8;
    }
    if ev.ctrl {
        code += 16;
    }
    if sgr {
        let (x, y) = if modes.pixels {
            (ev.px, ev.py)
        } else {
            (ev.col, ev.row)
        };
        let end = if ev.kind == MouseEventKind::Release {
            'm'
        } else {
            'M'
        };
        Some(format!("\x1b[<{};{};{}{}", code, x + 1, y + 1, end).into_bytes())
    } else {
        // Legacy encoding is limited to coordinates that fit one byte after the +32 offset.
        if ev.col + 1 > 223 || ev.row + 1 > 223 {
            return None;
        }
        Some(vec![
            0x1b,
            b'[',
            b'M',
            (code + 32) as u8,
            (ev.col + 33) as u8,
            (ev.row + 33) as u8,
        ])
    }
}

/// Watches the PTY byte stream for DECSET/DECRST 1016, which the engine ignores.
#[derive(Debug, Default)]
pub struct PixelModeSniffer {
    state: Sniff,
    param: u32,
    has1016: bool,
    flag: Arc<AtomicBool>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Sniff {
    #[default]
    Ground,
    Esc,
    Csi,
    Params,
}

impl PixelModeSniffer {
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        Self {
            flag,
            ..Default::default()
        }
    }

    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state = match (self.state, b) {
                (_, 0x1b) => Sniff::Esc,
                (Sniff::Esc, b'[') => Sniff::Csi,
                (Sniff::Csi, b'?') => {
                    self.param = 0;
                    self.has1016 = false;
                    Sniff::Params
                }
                (Sniff::Params, b'0'..=b'9') => {
                    self.param = self
                        .param
                        .saturating_mul(10)
                        .saturating_add((b - b'0') as u32);
                    Sniff::Params
                }
                (Sniff::Params, b';') => {
                    self.has1016 |= self.param == 1016;
                    self.param = 0;
                    Sniff::Params
                }
                (Sniff::Params, b'h' | b'l') => {
                    if self.has1016 || self.param == 1016 {
                        self.flag.store(b == b'h', Ordering::Relaxed);
                    }
                    Sniff::Ground
                }
                _ => Sniff::Ground,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MouseButton::*;
    use MouseEventKind::*;

    fn ev(kind: MouseEventKind, button: MouseButton, col: usize, row: usize) -> MouseEvent {
        MouseEvent::new(kind, button, col, row)
    }
    fn modes(t: MouseTracking, sgr: bool, pixels: bool) -> MouseModes {
        MouseModes {
            tracking: t,
            sgr,
            pixels,
        }
    }
    fn enc(e: MouseEvent, m: MouseModes) -> Option<String> {
        encode_mouse(&e, &m).map(|b| {
            b.iter()
                .map(|&c| format!("{:02x}", c))
                .collect::<Vec<_>>()
                .join("")
        })
    }

    #[test]
    fn off_reports_nothing() {
        assert_eq!(
            encode_mouse(&ev(Press, Left, 0, 0), &MouseModes::default()),
            None
        );
    }

    #[test]
    fn legacy_press_release() {
        let m = modes(MouseTracking::Click, false, false);
        assert_eq!(
            encode_mouse(&ev(Press, Left, 0, 0), &m).unwrap(),
            b"\x1b[M !!"
        );
        assert_eq!(
            encode_mouse(&ev(Press, Right, 9, 4), &m).unwrap(),
            b"\x1b[M\"*%"
        );
        // Release carries button 3 in legacy encoding.
        assert_eq!(
            encode_mouse(&ev(Release, Left, 0, 0), &m).unwrap(),
            b"\x1b[M#!!"
        );
    }

    #[test]
    fn legacy_coordinate_limit() {
        let m = modes(MouseTracking::Click, false, false);
        assert!(encode_mouse(&ev(Press, Left, 222, 0), &m).is_some());
        assert_eq!(encode_mouse(&ev(Press, Left, 223, 0), &m), None);
    }

    #[test]
    fn sgr_press_release() {
        let m = modes(MouseTracking::Click, true, false);
        assert_eq!(
            enc(ev(Press, Left, 0, 0), m),
            Some("1b5b3c303b313b314d".into())
        );
        assert_eq!(
            encode_mouse(&ev(Press, Left, 9, 4), &m).unwrap(),
            b"\x1b[<0;10;5M"
        );
        assert_eq!(
            encode_mouse(&ev(Release, Left, 9, 4), &m).unwrap(),
            b"\x1b[<0;10;5m"
        );
        assert_eq!(
            encode_mouse(&ev(Release, Right, 300, 4), &m).unwrap(),
            b"\x1b[<2;301;5m"
        );
    }

    #[test]
    fn modifiers_add_bits() {
        let m = modes(MouseTracking::Click, true, false);
        let mut e = ev(Press, Left, 0, 0);
        e.shift = true;
        e.alt = true;
        e.ctrl = true;
        assert_eq!(encode_mouse(&e, &m).unwrap(), b"\x1b[<28;1;1M");
    }

    #[test]
    fn click_mode_ignores_motion() {
        let m = modes(MouseTracking::Click, true, false);
        assert_eq!(encode_mouse(&ev(Motion, Left, 1, 1), &m), None);
    }

    #[test]
    fn drag_mode_needs_button() {
        let m = modes(MouseTracking::Drag, true, false);
        assert_eq!(
            encode_mouse(&ev(Motion, Left, 1, 1), &m).unwrap(),
            b"\x1b[<32;2;2M"
        );
        assert_eq!(encode_mouse(&ev(Motion, Unpressed, 1, 1), &m), None);
    }

    #[test]
    fn motion_mode_reports_buttonless() {
        let m = modes(MouseTracking::Motion, true, false);
        assert_eq!(
            encode_mouse(&ev(Motion, Unpressed, 1, 1), &m).unwrap(),
            b"\x1b[<35;2;2M"
        );
        let l = modes(MouseTracking::Motion, false, false);
        assert_eq!(
            encode_mouse(&ev(Motion, Unpressed, 1, 1), &l).unwrap(),
            b"\x1b[MC\"\""
        );
    }

    #[test]
    fn wheel_press_only() {
        let m = modes(MouseTracking::Click, true, false);
        assert_eq!(
            encode_mouse(&ev(Press, WheelUp, 0, 0), &m).unwrap(),
            b"\x1b[<64;1;1M"
        );
        assert_eq!(
            encode_mouse(&ev(Press, WheelDown, 0, 0), &m).unwrap(),
            b"\x1b[<65;1;1M"
        );
        assert_eq!(
            encode_mouse(&ev(Press, WheelLeft, 0, 0), &m).unwrap(),
            b"\x1b[<66;1;1M"
        );
        assert_eq!(encode_mouse(&ev(Release, WheelUp, 0, 0), &m), None);
        let l = modes(MouseTracking::Click, false, false);
        assert_eq!(
            encode_mouse(&ev(Press, WheelUp, 0, 0), &l).unwrap(),
            b"\x1b[M`!!"
        );
    }

    #[test]
    fn pixels_use_pixel_coordinates() {
        let m = modes(MouseTracking::Click, true, true);
        let mut e = ev(Press, Left, 3, 2);
        e.px = 41;
        e.py = 37;
        assert_eq!(encode_mouse(&e, &m).unwrap(), b"\x1b[<0;42;38M");
        e.kind = Release;
        assert_eq!(encode_mouse(&e, &m).unwrap(), b"\x1b[<0;42;38m");
    }

    #[test]
    fn sniffer_tracks_1016() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut s = PixelModeSniffer::new(flag.clone());
        s.feed(b"hi \x1b[?10");
        assert!(!flag.load(Ordering::Relaxed));
        s.feed(b"16h");
        assert!(flag.load(Ordering::Relaxed));
        s.feed(b"\x1b[?1006;1016l");
        assert!(!flag.load(Ordering::Relaxed));
        s.feed(b"\x1b[?1000h\x1b[1016h");
        assert!(!flag.load(Ordering::Relaxed));
        s.feed(b"\x1b[?1016;1006h");
        assert!(flag.load(Ordering::Relaxed));
    }
}
