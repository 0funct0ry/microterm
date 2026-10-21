//! Terminal → UI events, with coalesced wakeups (SPEC §3.4).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::term::Osc52;

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

/// OSC 9;4 progress state (ConEmu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressState {
    Remove,
    Normal,
    Error,
    Indeterminate,
    Paused,
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
    /// OSC 133 `D`: a command finished with this status (`None` when the shell sent none).
    CommandFinished(Option<i32>),
    /// OSC 9;4 progress report.
    Progress {
        state: ProgressState,
        value: Option<u8>,
    },
    /// OSC 9 notification text (unsanitised; the mux cleans it).
    Notify(String),
    /// The pane's shell-registered menu items changed (`OSC 7717`, SPEC §10.9.7).
    MenuItemsChanged,
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
            Self::CommandFinished(c) => write!(f, "CommandFinished({c:?})"),
            Self::Progress { state, value } => write!(f, "Progress({state:?}, {value:?})"),
            Self::Notify(t) => write!(f, "Notify({t:?})"),
            Self::MenuItemsChanged => write!(f, "MenuItemsChanged"),
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
    /// Where terminal-generated replies (DA, CPR, colour queries) go: the PTY, or the remote
    /// connection for a daemon-backed pane.
    sink: OnceLock<Box<dyn Fn(Vec<u8>) + Send + Sync>>,
    pub(crate) palette: Mutex<Palette>,
    /// Overrides set by sequences the engine does not model (see `vt.rs`).
    pub(crate) vt: Mutex<crate::vt::VtState>,
    pub(crate) size: Mutex<WindowSize>,
    policy: Mutex<Osc52Policy>,
    /// Last OSC 0/2 title, kept for VT replay (`None` after a reset).
    title: Mutex<Option<String>>,
    /// Signal that killed the child (`0` = none), set just before `ChildExit`.
    pub(crate) exit_signal: std::sync::atomic::AtomicI32,
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
            sink: OnceLock::new(),
            palette: Mutex::new(palette),
            vt: Mutex::new(Default::default()),
            size: Mutex::new(size),
            policy: Mutex::new(policy),
            title: Mutex::new(None),
            exit_signal: std::sync::atomic::AtomicI32::new(0),
        })
    }

    pub(crate) fn title(&self) -> Option<String> {
        self.title.lock().unwrap().clone()
    }

    pub(crate) fn set_policy(&self, policy: Osc52Policy) {
        *self.policy.lock().unwrap() = policy;
    }

    pub(crate) fn set_sink(&self, sink: Box<dyn Fn(Vec<u8>) + Send + Sync>) {
        let _ = self.sink.set(sink);
    }

    /// Queues an event for the UI, as the reader does.
    pub(crate) fn notify(&self, ev: TermEvent) {
        self.push(ev);
    }

    pub(crate) fn write_pty(&self, bytes: Vec<u8>) {
        if let Some(sink) = self.sink.get() {
            sink(bytes);
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
            Event::Title(t) => {
                *s.title.lock().unwrap() = Some(t.clone());
                s.push(TermEvent::Title(t));
            }
            Event::ResetTitle => {
                *s.title.lock().unwrap() = None;
                s.push(TermEvent::ResetTitle);
            }
            Event::Bell => s.push(TermEvent::Bell),
            Event::CursorBlinkingChange => s.push(TermEvent::CursorBlinkingChange),
            Event::ClipboardStore(t, text) => {
                let policy = *s.policy.lock().unwrap();
                if policy != Osc52Policy::Disabled && text.len() <= OSC52_MAX_BYTES {
                    s.push(TermEvent::ClipboardStore(kind(t), text));
                }
            }
            Event::ClipboardLoad(t, f) => {
                let policy = *s.policy.lock().unwrap();
                if policy == Osc52Policy::ReadWrite {
                    s.push(TermEvent::ClipboardLoad(kind(t), f));
                }
            }
            // DECRQM for SGR-pixels (1016): the engine says "not recognised"; `vt.rs` answers.
            Event::PtyWrite(text) if text == "\x1b[?1016;0$y" => {}
            Event::PtyWrite(text) => s.write_pty(text.into_bytes()),
            // Colour queries are answered by `vt.rs`, which sees the override state and the
            // reply format setting; the engine's reply would duplicate it.
            Event::ColorRequest(..) => {}
            Event::TextAreaSizeRequest(f) => {
                let size = *s.size.lock().unwrap();
                s.write_pty(f(size).into_bytes());
            }
            Event::ChildExit(status) => s.push(TermEvent::ChildExit(status.code())),
            Event::MouseCursorDirty | Event::Exit => {}
        }
    }
}
