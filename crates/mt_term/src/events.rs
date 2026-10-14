//! Terminal → UI events, with coalesced wakeups (SPEC §3.4).

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoopSender, Msg};
use alacritty_terminal::term::Osc52;
use alacritty_terminal::vte::ansi::Rgb as EngineRgb;

/// OSC 52 payloads above this size are dropped (SPEC §6.3).
pub const OSC52_MAX_BYTES: usize = 100 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Osc52Policy {
    Write,
    ReadWrite,
    Disabled,
}

impl Osc52Policy {
    /// Parses the `clipboard.osc52` config value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "write" => Some(Self::Write),
            "read-write" => Some(Self::ReadWrite),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    pub(crate) fn engine(self) -> Osc52 {
        match self {
            Self::Write => Osc52::OnlyCopy,
            Self::ReadWrite => Osc52::CopyPaste,
            Self::Disabled => Osc52::Disabled,
        }
    }
}

/// Which clipboard an OSC 52 write targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardKind {
    Clipboard,
    Primary,
}

#[derive(Clone)]
pub enum TermEvent {
    /// New content; re-snapshot.
    Wakeup,
    Title(String),
    ResetTitle,
    Bell,
    ClipboardStore(ClipboardKind, String),
    /// Only emitted when the policy is `read-write`; reply with `Terminal::write`.
    ClipboardLoad(ClipboardKind, Arc<dyn Fn(&str) -> String + Send + Sync>),
    CursorBlinkingChange,
    /// The child exited with this status code (`None` if killed by a signal).
    ChildExit(Option<i32>),
}

impl std::fmt::Debug for TermEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wakeup => write!(f, "Wakeup"),
            Self::Title(t) => write!(f, "Title({t:?})"),
            Self::ResetTitle => write!(f, "ResetTitle"),
            Self::Bell => write!(f, "Bell"),
            Self::ClipboardStore(k, t) => write!(f, "ClipboardStore({k:?}, {} bytes)", t.len()),
            Self::ClipboardLoad(k, _) => write!(f, "ClipboardLoad({k:?})"),
            Self::CursorBlinkingChange => write!(f, "CursorBlinkingChange"),
            Self::ChildExit(c) => write!(f, "ChildExit({c:?})"),
        }
    }
}

/// Colours the terminal needs to answer OSC 4/10/11/12 queries.
#[derive(Clone, Debug)]
pub struct Palette {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub cursor: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

impl Default for Palette {
    fn default() -> Self {
        // One Dark.
        let h = |v: u32| [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        Palette {
            fg: h(0xabb2bf),
            bg: h(0x282c34),
            cursor: h(0x528bff),
            ansi: [
                h(0x282c34),
                h(0xe06c75),
                h(0x98c379),
                h(0xe5c07b),
                h(0x61afef),
                h(0xc678dd),
                h(0x56b6c2),
                h(0xabb2bf),
                h(0x5c6370),
                h(0xe06c75),
                h(0x98c379),
                h(0xe5c07b),
                h(0x61afef),
                h(0xc678dd),
                h(0x56b6c2),
                h(0xffffff),
            ],
        }
    }
}

impl Palette {
    pub(crate) fn lookup(&self, idx: usize) -> Option<[u8; 3]> {
        match idx {
            0..=15 => Some(self.ansi[idx]),
            16..=231 => {
                let i = idx - 16;
                let lvl = |n: usize| if n == 0 { 0 } else { (55 + 40 * n) as u8 };
                Some([lvl(i / 36), lvl((i / 6) % 6), lvl(i % 6)])
            }
            232..=255 => {
                let v = (8 + 10 * (idx - 232)) as u8;
                Some([v, v, v])
            }
            256 => Some(self.fg),
            257 => Some(self.bg),
            258 => Some(self.cursor),
            _ => None,
        }
    }
}

pub(crate) struct Shared {
    queue: Mutex<VecDeque<TermEvent>>,
    /// Set while a wakeup has been delivered and not yet drained.
    armed: AtomicBool,
    wakeup_queued: AtomicBool,
    waker: Box<dyn Fn() + Send + Sync>,
    pty: OnceLock<EventLoopSender>,
    pub(crate) palette: Mutex<Palette>,
    pub(crate) size: Mutex<WindowSize>,
    policy: Osc52Policy,
}

impl Shared {
    pub(crate) fn new(
        waker: Box<dyn Fn() + Send + Sync>,
        palette: Palette,
        size: WindowSize,
        policy: Osc52Policy,
    ) -> Arc<Self> {
        Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            armed: AtomicBool::new(false),
            wakeup_queued: AtomicBool::new(false),
            waker,
            pty: OnceLock::new(),
            palette: Mutex::new(palette),
            size: Mutex::new(size),
            policy,
        })
    }

    pub(crate) fn set_pty(&self, sender: EventLoopSender) {
        let _ = self.pty.set(sender);
    }

    fn write_pty(&self, bytes: Vec<u8>) {
        if let Some(tx) = self.pty.get() {
            let _ = tx.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    fn push(&self, ev: TermEvent) {
        if matches!(ev, TermEvent::Wakeup) && self.wakeup_queued.swap(true, Ordering::AcqRel) {
            return;
        }
        self.queue.lock().unwrap().push_back(ev);
        if !self.armed.swap(true, Ordering::AcqRel) {
            (self.waker)();
        }
    }

    /// Takes every queued event. Call before snapshotting so later wakeups re-arm.
    pub(crate) fn drain(&self) -> Vec<TermEvent> {
        self.armed.store(false, Ordering::Release);
        self.wakeup_queued.store(false, Ordering::Release);
        self.queue.lock().unwrap().drain(..).collect()
    }
}

#[derive(Clone)]
pub(crate) struct Listener(pub(crate) Arc<Shared>);

fn kind(t: alacritty_terminal::term::ClipboardType) -> ClipboardKind {
    match t {
        alacritty_terminal::term::ClipboardType::Clipboard => ClipboardKind::Clipboard,
        alacritty_terminal::term::ClipboardType::Selection => ClipboardKind::Primary,
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let s = &self.0;
        match event {
            Event::Wakeup => s.push(TermEvent::Wakeup),
            Event::Title(t) => s.push(TermEvent::Title(t)),
            Event::ResetTitle => s.push(TermEvent::ResetTitle),
            Event::Bell => s.push(TermEvent::Bell),
            Event::CursorBlinkingChange => s.push(TermEvent::CursorBlinkingChange),
            Event::ClipboardStore(t, text) => {
                if s.policy != Osc52Policy::Disabled && text.len() <= OSC52_MAX_BYTES {
                    s.push(TermEvent::ClipboardStore(kind(t), text));
                }
            }
            Event::ClipboardLoad(t, f) => {
                if s.policy == Osc52Policy::ReadWrite {
                    s.push(TermEvent::ClipboardLoad(kind(t), f));
                }
            }
            Event::PtyWrite(text) => s.write_pty(text.into_bytes()),
            Event::ColorRequest(idx, f) => {
                let rgb = s.palette.lock().unwrap().lookup(idx);
                if let Some([r, g, b]) = rgb {
                    s.write_pty(f(EngineRgb { r, g, b }).into_bytes());
                }
            }
            Event::TextAreaSizeRequest(f) => {
                let size = *s.size.lock().unwrap();
                s.write_pty(f(size).into_bytes());
            }
            Event::ChildExit(status) => s.push(TermEvent::ChildExit(status.code())),
            Event::MouseCursorDirty | Event::Exit => {}
        }
    }
}
