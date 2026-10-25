//! A live terminal: PTY, reader thread and emulator behind one handle.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty::{self, Shell};

use crate::commands::{drive, CommandInfo, CommandTracker, OscSniffer};
use crate::emulator::{take_damage, term_config, CursorDefault, ScrollCmd, Size};
use crate::events::{Listener, Osc52Policy, Palette, Shared, TermEvent};
use crate::input::{encode_key, encode_paste, InputMode, KeyInput};
use crate::linetap::{LineQueue, LineTapCfg};
use crate::shell::resolve_shell;
use crate::snapshot::{self, Modes, RenderSnapshot};
use crate::width_filter::WidthFilter;

pub struct SpawnOptions {
    /// `shell` config value; empty means `$SHELL`, then the passwd entry, then `/bin/sh`.
    pub shell: String,
    /// Run this program and arguments instead of the shell (editor tabs, launch profiles).
    pub command: Option<Vec<String>>,
    pub cwd: Option<PathBuf>,
    /// Extra environment for the child (`[env]` config table).
    pub env: Vec<(String, String)>,
    /// Value of `$TERM`.
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    /// `text.wide_regional_indicators`: count regional indicators as two cells like zsh.
    pub wide_regional_indicators: bool,
    pub cell_width: u16,
    pub cell_height: u16,
    pub scrollback: usize,
    pub osc52: Osc52Policy,
    pub palette: Palette,
    /// Cursor shown until the program sets one (`cursor.*` config).
    pub cursor: CursorDefault,
    /// `[terminal]` settings for VT sequences (colour replies, OSC 9, XTSHIFTESCAPE).
    pub vt: crate::vt::VtConfig,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        SpawnOptions {
            shell: String::new(),
            command: None,
            cwd: None,
            env: Vec::new(),
            term: "xterm-256color".into(),
            cols: 80,
            rows: 24,
            wide_regional_indicators: false,
            cell_width: 8,
            cell_height: 16,
            scrollback: 10_000,
            osc52: Osc52Policy::Write,
            palette: Palette::default(),
            cursor: CursorDefault::default(),
            vt: Default::default(),
        }
    }
}

/// One event of the recording tap (SPEC §10.10.3). `at` is stamped where the bytes arrive, so
/// playback timing does not depend on how busy the consumer is.
#[derive(Debug, Clone)]
pub struct TapEvent {
    pub at: std::time::Instant,
    pub kind: TapKind,
}

#[derive(Debug, Clone)]
pub enum TapKind {
    /// Raw bytes the child (or remote) printed, before any width filtering.
    Output(Vec<u8>),
    /// Bytes sent to the pane. `hidden` is whether the local PTY was in a no-echo prompt
    /// (`ECHO` clear, `ICANON` set) at that moment; `None` for SSH and serial panes, which
    /// cannot be inspected.
    Input {
        bytes: Vec<u8>,
        hidden: Option<bool>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// A marker request (`ctl recording marker`), ordered with the bytes around it.
    /// Empty = the recorder picks "mark N".
    Marker(String),
}

/// Optional copies of the byte streams, for recording (`mt_record`).
#[derive(Default)]
struct Taps {
    /// The M12.1 recording tap: one channel of timestamped events for every kind.
    record: std::sync::Mutex<Option<std::sync::mpsc::Sender<TapEvent>>>,
    output: std::sync::Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    input: std::sync::Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    /// Unix milliseconds of the last PTY read (`pane wait --idle`). Zero before any output.
    last_output_ms: AtomicU64,
}

impl Taps {
    fn send_record(&self, kind: impl FnOnce() -> TapKind) {
        let mut g = self.record.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = g.as_ref() {
            let ev = TapEvent {
                at: std::time::Instant::now(),
                kind: kind(),
            };
            if tx.send(ev).is_err() {
                *g = None;
            }
        }
    }

    /// Output reaching the parser: both the M9 byte tap and the recording tap.
    fn output_bytes(&self, bytes: &[u8]) {
        Self::send(&self.output, bytes);
        self.send_record(|| TapKind::Output(bytes.to_vec()));
    }

    fn send(slot: &std::sync::Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>, bytes: &[u8]) {
        let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = g.as_ref() {
            if tx.send(bytes.to_vec()).is_err() {
                *g = None; // the receiver went away
            }
        }
    }
}

/// `ECHO` clear and `ICANON` set in the PTY master's termios (a password prompt).
fn pty_input_hidden(fd: RawFd) -> Option<bool> {
    // SAFETY: `termios` is plain data and `tcgetattr` only writes into it; `fd` is the master
    // fd the terminal owns for as long as the backend lives.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            return None;
        }
        Some(t.c_lflag & libc::ECHO == 0 && t.c_lflag & libc::ICANON != 0)
    }
}

/// How a terminal reaches its process.
enum Backend {
    /// A local PTY read by our own thread (see [`spawn_local_io`]).
    Local {
        /// Resizes the PTY and signals the child on drop.
        pty: Mutex<tty::Pty>,
        input: std::sync::mpsc::Sender<Cow<'static, [u8]>>,
        pid: u32,
        /// PTY master fd, for foreground-process queries.
        master_fd: RawFd,
    },
    /// A pane owned by a `microterm-mux` daemon: bytes are pushed in with `feed`.
    Remote {
        io: RemoteIo,
        feed: Box<std::sync::Mutex<RemoteFeed>>,
    },
}

/// Outbound side of a daemon-backed terminal.
pub struct RemoteIo {
    /// Keystrokes and terminal replies, in order.
    pub input: Box<dyn Fn(Vec<u8>) + Send + Sync>,
    /// The visible size changed (cols, rows).
    pub resize: Box<dyn Fn(u16, u16) + Send + Sync>,
}

/// Parser state for bytes arriving from a daemon.
pub struct RemoteFeed {
    parser: alacritty_terminal::vte::ansi::Processor,
    filter: WidthFilter,
    sniffer: crate::mouse::PixelModeSniffer,
    osc: OscSniffer,
    out: Vec<u8>,
}

pub struct Terminal {
    taps: Arc<Taps>,
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    backend: Backend,
    scrollback: usize,
    /// `text.wide_regional_indicators`, shared with the PTY reader.
    wide_ri: Arc<AtomicBool>,
    /// DECSET 1016 state, set by the PTY reader's sniffer.
    sgr_pixels: Arc<AtomicBool>,
    /// Per-command OSC 133 records, written by whoever parses the output.
    tracker: Arc<Mutex<CommandTracker>>,
}

impl Terminal {
    /// Spawns the shell on a new PTY. `waker` is called (from the reader thread) when events
    /// are queued; it must be cheap and non-blocking. Wakeups are coalesced, so it fires at
    /// most once until `drain_events` runs.
    pub fn spawn(
        opts: SpawnOptions,
        waker: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Terminal> {
        let window_size = WindowSize {
            num_lines: opts.rows,
            num_cols: opts.cols,
            cell_width: opts.cell_width,
            cell_height: opts.cell_height,
        };
        let shared = Shared::new(
            Box::new(waker),
            opts.palette.clone(),
            window_size,
            opts.osc52,
        );
        shared.vt.lock().unwrap().cfg = opts.vt;
        let listener = Listener(shared.clone());
        let term = Term::new(
            term_config(opts.scrollback, opts.osc52, opts.cursor),
            &Size {
                cols: opts.cols as usize,
                rows: opts.rows as usize,
            },
            listener.clone(),
        );
        let term = Arc::new(FairMutex::new(term));

        let (program, args) = match opts.command.as_deref() {
            Some([program, rest @ ..]) => (program.clone(), rest.to_vec()),
            _ => (
                resolve_shell(&opts.shell),
                // macOS terminals start login shells so PATH matches Terminal.app.
                if cfg!(target_os = "macos") {
                    vec!["-l".to_string()]
                } else {
                    Vec::new()
                },
            ),
        };
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("TERM".into(), opts.term.clone());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "microterm".into());
        env.insert(
            "TERM_PROGRAM_VERSION".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        if std::env::var_os("LANG").is_none() && std::env::var_os("LC_ALL").is_none() {
            env.insert("LANG".into(), "en_US.UTF-8".into());
        }
        env.extend(opts.env);
        let pty_opts = tty::Options {
            shell: Some(Shell::new(program, args)),
            working_directory: opts.cwd,
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&pty_opts, window_size, 0)?;
        let pid = pty.child().id();
        let master_fd = pty.file().as_raw_fd();
        let wide_ri = Arc::new(AtomicBool::new(opts.wide_regional_indicators));
        let sgr_pixels = Arc::new(AtomicBool::new(false));
        let taps = Arc::new(Taps::default());
        let tracker = Arc::new(Mutex::new(CommandTracker::new(
            opts.scrollback,
            opts.cols as usize,
        )));
        let input = spawn_local_io(
            &pty,
            LocalIo {
                term: term.clone(),
                shared: shared.clone(),
                tracker: tracker.clone(),
                reader: FilterReader::new(
                    pty.file().try_clone()?,
                    wide_ri.clone(),
                    sgr_pixels.clone(),
                    taps.clone(),
                ),
                pid,
            },
        )?;
        {
            let tx = input.clone();
            shared.set_sink(Box::new(move |bytes| {
                let _ = tx.send(Cow::Owned(bytes));
            }));
        }
        Ok(Terminal {
            taps,
            term,
            shared,
            backend: Backend::Local {
                pty: Mutex::new(pty),
                input,
                pid,
                master_fd,
            },
            scrollback: opts.scrollback,
            wide_ri,
            sgr_pixels,
            tracker,
        })
    }

    /// A terminal whose process lives in a remote `microterm-mux` daemon. Feed it the daemon's
    /// `Output`/`Replay` bytes with [`Terminal::feed`]; keystrokes go out through `io.input`.
    pub fn remote(
        opts: SpawnOptions,
        io: RemoteIo,
        waker: impl Fn() + Send + Sync + 'static,
    ) -> Terminal {
        let window_size = WindowSize {
            num_lines: opts.rows,
            num_cols: opts.cols,
            cell_width: opts.cell_width,
            cell_height: opts.cell_height,
        };
        let shared = Shared::new(
            Box::new(waker),
            opts.palette.clone(),
            window_size,
            opts.osc52,
        );
        shared.vt.lock().unwrap().cfg = opts.vt;
        let term = Term::new(
            term_config(opts.scrollback, opts.osc52, opts.cursor),
            &Size {
                cols: opts.cols as usize,
                rows: opts.rows as usize,
            },
            Listener(shared.clone()),
        );
        let input: Arc<dyn Fn(Vec<u8>) + Send + Sync> = Arc::from(io.input);
        let reply = input.clone();
        shared.set_sink(Box::new(move |b| reply(b)));
        let io = RemoteIo {
            input: Box::new(move |b| input(b)),
            resize: io.resize,
        };
        let sgr_pixels = Arc::new(AtomicBool::new(false));
        Terminal {
            taps: Arc::new(Taps::default()),
            term: Arc::new(FairMutex::new(term)),
            shared,
            backend: Backend::Remote {
                io,
                feed: Box::new(std::sync::Mutex::new(RemoteFeed {
                    parser: Default::default(),
                    filter: WidthFilter::new(),
                    sniffer: crate::mouse::PixelModeSniffer::new(sgr_pixels.clone()),
                    osc: Default::default(),
                    out: Vec::new(),
                })),
            },
            scrollback: opts.scrollback,
            wide_ri: Arc::new(AtomicBool::new(opts.wide_regional_indicators)),
            sgr_pixels,
            tracker: Arc::new(Mutex::new(CommandTracker::new(
                opts.scrollback,
                opts.cols as usize,
            ))),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.backend, Backend::Remote { .. })
    }

    /// Parses bytes that arrived from the daemon (remote terminals only).
    pub fn feed(&self, bytes: &[u8]) {
        self.feed_inner(bytes, true);
    }

    /// Like [`feed`](Self::feed) for the VT replay the daemon sends after a reconnect: the
    /// screen it redraws is already in any running recording, so only the M9 tap sees it
    /// (SPEC §10.10.2).
    pub fn feed_replay(&self, bytes: &[u8]) {
        self.feed_inner(bytes, false);
    }

    fn feed_inner(&self, bytes: &[u8], record: bool) {
        let Backend::Remote { feed, .. } = &self.backend else {
            return;
        };
        Taps::send(&self.taps.output, bytes);
        if record {
            self.taps.send_record(|| TapKind::Output(bytes.to_vec()));
        }
        let mut f = feed.lock().unwrap_or_else(|e| e.into_inner());
        let f = &mut *f;
        f.sniffer.feed(bytes);
        self.taps
            .last_output_ms
            .store(crate::commands::now_ms(), Ordering::Relaxed);
        let mut term = self.term.lock();
        let mut tracker = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
        let RemoteFeed {
            parser,
            filter,
            osc,
            out,
            ..
        } = f;
        let wide = self.wide_ri.load(Ordering::Relaxed);
        // Replayed bytes (reconnect, attach) never reach the trigger line tap (SPEC §10.11.3).
        tracker.lines.set_replay(!record);
        // Bytes held back by the width filter while it was on are released first.
        if !wide {
            let held = filter.take_pending();
            filter.reset();
            if !held.is_empty() {
                parser.advance(&mut *term, &held);
            }
        }
        drive(
            osc,
            &mut tracker,
            &self.shared,
            &mut term,
            bytes,
            |term, seg| {
                if wide {
                    out.clear();
                    filter.filter(seg, out);
                    parser.advance(term, out);
                } else {
                    parser.advance(term, seg);
                }
            },
            |code| self.shared.notify(TermEvent::CommandFinished(code)),
        );
        if !record {
            let trimmed = tracker.trimmed();
            tracker.lines.settle_replay(&term, trimmed);
        }
        drop(tracker);
        drop(term);
        self.shared.notify(TermEvent::Wakeup);
    }

    /// Reports that a remote process exited (shows up as `TermEvent::ChildExit`).
    pub fn remote_exited(&self, status: Option<i32>) {
        let cleared = self.tracker().clear_private();
        self.shared.notify_cleared(cleared);
        self.shared.notify(TermEvent::ChildExit(status));
    }

    /// Process id of the local child; `0` for remote terminals.
    pub fn pid(&self) -> u32 {
        match &self.backend {
            Backend::Local { pid, .. } => *pid,
            Backend::Remote { .. } => 0,
        }
    }

    /// The foreground process when it is not the shell itself (drives the dirty dot and the
    /// `confirm_close = \"process\"` policy).
    pub fn foreground_process(&self) -> Option<crate::ForegroundProcess> {
        match &self.backend {
            Backend::Local { pid, master_fd, .. } => crate::process::foreground(*master_fd, *pid),
            Backend::Remote { .. } => None,
        }
    }

    /// Working directory of the foreground process, falling back to the shell's.
    pub fn cwd(&self) -> Option<PathBuf> {
        let Backend::Local { pid: shell, .. } = &self.backend else {
            return None;
        };
        let pid = self.foreground_process().map_or(*shell, |p| p.pid);
        crate::process::process_cwd(pid).or_else(|| crate::process::process_cwd(*shell))
    }

    /// Queues bytes for the child. Never blocks on a full PTY buffer.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        Taps::send(&self.taps.input, &bytes);
        let hidden = self.input_hidden();
        self.taps.send_record(|| TapKind::Input {
            bytes: bytes.to_vec(),
            hidden,
        });
        self.write_untapped(bytes);
    }

    /// Like [`write`](Self::write) but invisible to every input tap. Used for snippet inserts
    /// that contain a `secret` (SPEC §10.8.5, §10.10.3).
    pub fn write_untapped(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.note_typed();
        self.send_raw(bytes.into());
    }

    /// Marks the cursor's logical line as typed input (see `CompletedLine::input`), so rules
    /// that match command output skip it even without shell integration.
    fn note_typed(&self) {
        let id = {
            let term = self.term.lock();
            if term
                .mode()
                .contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
            {
                return;
            }
            crate::linetap::cursor_line_start(&term)
        };
        let mut tracker = self.tracker();
        let id = tracker.trimmed() + id as u64;
        tracker.note_typed(id);
    }

    fn send_raw(&self, bytes: Cow<'static, [u8]>) {
        match &self.backend {
            Backend::Local { input, .. } => {
                let _ = input.send(bytes);
            }
            Backend::Remote { io, .. } => (io.input)(bytes.into_owned()),
        }
    }

    /// Copies every byte the child prints (before any width filtering) to `tx`, or stops the
    /// copy with `None`. Used by `record.start`.
    pub fn set_output_tap(&self, tx: Option<std::sync::mpsc::Sender<Vec<u8>>>) {
        *self.taps.output.lock().unwrap_or_else(|e| e.into_inner()) = tx;
    }

    /// Copies every byte written to the child (keystrokes, pastes) to `tx`.
    /// Starts or stops the timestamped recording tap (SPEC §10.10.3).
    pub fn set_record_tap(&self, tx: Option<std::sync::mpsc::Sender<TapEvent>>) {
        *self.taps.record.lock().unwrap_or_else(|e| e.into_inner()) = tx;
    }

    /// Adds a marker to the running recording, if any, at the current time.
    pub fn record_marker(&self, label: Option<&str>) {
        self.taps
            .send_record(|| TapKind::Marker(label.unwrap_or_default().to_string()));
    }

    /// True while a program on the local PTY reads a password: `ECHO` clear with `ICANON`
    /// still set. Line editors (readline, zle) clear both, so ordinary prompts are not
    /// hidden. `None` for remote panes or when the termios query fails.
    pub fn input_hidden(&self) -> Option<bool> {
        match &self.backend {
            Backend::Local { master_fd, .. } => pty_input_hidden(*master_fd),
            Backend::Remote { .. } => None,
        }
    }

    /// Installs (or with `None` removes) the trigger line tap (SPEC §10.11.3): finished lines of
    /// rendered output go to `queue` tagged `pane`, starting from the cursor's current line.
    ///
    /// The PTY thread only records which rows finished; a small `mt-line-tap` thread reads them
    /// out of the grid a couple of milliseconds later (so the text is the line as it stands then)
    /// and scans an unfinished line (a prompt) once after `partial_after_ms` of PTY silence. With
    /// no tap installed the cost is one check per parsed segment.
    pub fn set_line_tap(&self, cfg: Option<LineTapCfg>, queue: Arc<LineQueue>, pane: u64) {
        let term = self.term.lock();
        let mut tracker = self.tracker();
        let Some(cfg) = cfg else {
            tracker.lines.remove();
            return;
        };
        let shared = crate::linetap::TapShared::new();
        let trimmed = tracker.trimmed();
        tracker
            .lines
            .install(cfg, queue, pane, shared.clone(), true, &*term, trimmed);
        drop(tracker);
        drop(term);
        self.spawn_line_thread(cfg.partial_after_ms, shared);
    }

    pub fn line_tap_installed(&self) -> bool {
        self.tracker().lines.installed()
    }

    /// The `mt-line-tap` thread: reads finished lines out of the grid (woken by the PTY thread,
    /// then waiting a moment so one pass covers many lines) and scans an unfinished line once
    /// the PTY has been quiet.
    fn spawn_line_thread(&self, after_ms: u32, shared: Arc<crate::linetap::TapShared>) {
        const COALESCE: Duration = Duration::from_millis(2);
        let term = Arc::downgrade(&self.term);
        let tracker = Arc::downgrade(&self.tracker);
        let taps = Arc::downgrade(&self.taps);
        let idle = if after_ms == 0 {
            Duration::from_secs(3600)
        } else {
            Duration::from_millis(u64::from((after_ms / 4).clamp(20, 100)))
        };
        let thread_shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("mt-line-tap".into())
            .spawn(move || {
                let shared = thread_shared;
                let mut checked = 0u64;
                loop {
                    // Woken by the PTY thread when rows finish; otherwise checks for quiet.
                    if !shared.pending.load(Ordering::Acquire) {
                        std::thread::park_timeout(idle);
                    }
                    if !shared.alive.load(Ordering::Relaxed) {
                        return;
                    }
                    let work = shared.pending.load(Ordering::Acquire);
                    if work {
                        std::thread::sleep(COALESCE);
                    }
                    let (Some(term), Some(tracker), Some(taps)) =
                        (term.upgrade(), tracker.upgrade(), taps.upgrade())
                    else {
                        return;
                    };
                    let last = taps.last_output_ms.load(Ordering::Relaxed);
                    let quiet = crate::commands::now_ms().saturating_sub(last);
                    let want_partial = after_ms > 0
                        && last != 0
                        && last != checked
                        && quiet >= u64::from(after_ms);
                    if !work && !want_partial {
                        continue;
                    }
                    let term = term.lock();
                    let mut tracker = tracker.lock().unwrap_or_else(|e| e.into_inner());
                    tracker.drain_lines(&*term);
                    if want_partial {
                        checked = last;
                        tracker.flush_partial_lines(&*term);
                    }
                }
            });
        match spawned {
            Ok(h) => {
                shared.set_waker(h.thread().clone());
                h.thread().unpark();
            }
            Err(_) => {
                // No extraction thread: finished lines are never read out (taps stay inert).
            }
        }
    }

    pub fn set_input_tap(&self, tx: Option<std::sync::mpsc::Sender<Vec<u8>>>) {
        *self.taps.input.lock().unwrap_or_else(|e| e.into_inner()) = tx;
    }

    pub fn send_key(&self, key: &KeyInput) {
        let mode = self.input_mode();
        if let Some(b) = encode_key(key, &mode) {
            self.write(b);
        }
    }

    /// Pastes text, honouring bracketed paste mode.
    pub fn paste(&self, text: &str) {
        let bracketed = self.modes().bracketed_paste;
        self.write(encode_paste(text, bracketed));
    }

    /// [`paste`](Self::paste) that no input tap sees.
    pub fn paste_untapped(&self, text: &str) {
        let bracketed = self.modes().bracketed_paste;
        self.write_untapped(encode_paste(text, bracketed));
    }

    pub fn resize(&self, cols: u16, rows: u16, cell_width: u16, cell_height: u16) {
        let ws = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width,
            cell_height,
        };
        *self.shared.size.lock().unwrap() = ws;
        self.taps.send_record(|| TapKind::Resize { cols, rows });
        {
            let mut term = self.term.lock();
            term.resize(Size {
                cols: cols as usize,
                rows: rows as usize,
            });
            self.tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .after_resize(&term);
        }
        match &self.backend {
            Backend::Local { pty, .. } => {
                pty.lock().unwrap_or_else(|e| e.into_inner()).on_resize(ws);
            }
            Backend::Remote { io, .. } => (io.resize)(cols, rows),
        }
    }

    /// Applies the settings that may change while the shell runs: the OSC 52 policy and the
    /// default cursor. Scrollback size is fixed at spawn (SPEC §8.2: new panes only).
    pub fn set_runtime(&self, osc52: Osc52Policy, cursor: CursorDefault) {
        self.shared.set_policy(osc52);
        self.term
            .lock()
            .set_options(term_config(self.scrollback, osc52, cursor));
    }

    /// Switches `text.wide_regional_indicators`; it applies to output that arrives afterwards.
    pub fn set_wide_regional_indicators(&self, on: bool) {
        self.wide_ri.store(on, Ordering::Relaxed);
    }

    pub fn set_palette(&self, palette: Palette) {
        *self.shared.palette.lock().unwrap() = palette;
    }

    pub fn modes(&self) -> Modes {
        let mut m = Modes::from_engine(*self.term.lock().mode());
        m.sgr_pixels = self.sgr_pixels.load(Ordering::Relaxed);
        m
    }

    /// Sends a mouse report if the application asked for one. Returns whether it was sent.
    pub fn send_mouse(&self, ev: &crate::MouseEvent) -> bool {
        match crate::encode_mouse(ev, &self.modes().mouse_modes()) {
            Some(b) => {
                // Mouse reports are not typing: the cursor's line stays output.
                let b: Cow<'static, [u8]> = b.into();
                Taps::send(&self.taps.input, &b);
                let hidden = self.input_hidden();
                self.taps.send_record(|| TapKind::Input {
                    bytes: b.to_vec(),
                    hidden,
                });
                self.send_raw(b);
                true
            }
            None => false,
        }
    }

    pub fn input_mode(&self) -> InputMode {
        self.modes().input_mode()
    }

    pub fn selection_start(
        &self,
        kind: crate::SelectionKind,
        row: usize,
        col: usize,
        right: bool,
        word_chars: &str,
    ) {
        crate::selection::start(&mut self.term.lock(), kind, row, col, right, word_chars);
    }

    pub fn selection_update(&self, row: usize, col: usize, right: bool) {
        crate::selection::update(&mut self.term.lock(), row, col, right);
    }

    pub fn selection_clear(&self) {
        crate::selection::clear(&mut self.term.lock());
    }

    pub fn select_all(&self) {
        crate::selection::select_all(&mut self.term.lock());
    }

    pub fn has_selection(&self) -> bool {
        self.term.lock().selection.is_some()
    }

    /// The selected text, or `None` when nothing (or only blanks) is selected.
    pub fn selection_text(&self, trim_trailing: bool) -> Option<String> {
        crate::selection::text(&self.term.lock(), trim_trailing)
    }

    pub fn scroll_display(&self, cmd: ScrollCmd) {
        self.term.lock().scroll_display(cmd.engine());
    }

    /// Drops scrollback history (the visible screen is kept).
    pub fn clear_scrollback(&self) {
        let mut term = self.term.lock();
        term.grid_mut().clear_history();
        self.tracker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .history_cleared(&term);
    }

    /// Resets modes, charsets, cursor style, text attributes, scroll region, tab stops and
    /// program colour overrides, like RIS, but keeps the screen and the scrollback (RIS would
    /// clear both). The cursor stays where it is.
    pub fn reset(&self) {
        let mut term = self.term.lock();
        let mut seq = String::from("\x1b7\x1b[3g");
        for col in (8..alacritty_terminal::grid::Dimensions::columns(&*term)).step_by(8) {
            seq.push_str(&format!("\x1b[1;{}H\x1bH", col + 1));
        }
        seq.push_str("\x1b8");
        // Charsets, SGR, scroll region, insert/newline modes, DECSET defaults, cursor style
        // and visibility, keypad, kitty keyboard flags, hyperlink.
        seq.push_str(
            "\x0f\x1b(B\x1b)B\x1b[0m\x1b[r\x1b[4l\x1b[20l\x1b[?1l\x1b[?6l\x1b[?7h\x1b[?25h\
             \x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\
             \x1b[?1007l\x1b[?2004l\x1b[?1049l\x1b[?47l\x1b>\x1b[0 q\x1b[<99u\x1b]8;;\x1b\\",
        );
        crate::vt::inject_str(&mut term, &seq);
        crate::vt::reset(&mut term, &self.shared);
    }

    /// Applies the `[terminal]` settings (new output only).
    pub fn set_vt_config(&self, cfg: crate::vt::VtConfig) {
        let old = std::mem::replace(&mut self.shared.vt.lock().unwrap().cfg, cfg).private;
        if old != cfg.private {
            let cleared = self.tracker().private_gate_changed(&old, &cfg.private);
            self.shared.notify_cleared(cleared);
        }
    }

    /// The `7725` status segments this pane's program set (slot-name order).
    pub fn status_segments(&self) -> Vec<crate::private_osc::Segment> {
        self.tracker().status_segments()
    }

    /// The `7720` progress bars this pane's program set (id order).
    pub fn progress_bars(&self) -> Vec<crate::private_osc::Bar> {
        self.tracker().progress_bars()
    }

    /// The `7718` badge this pane's program set.
    pub fn badge(&self) -> Option<crate::private_osc::Badge> {
        self.tracker().badge()
    }

    /// Drops the `7722` marks (`ClearMarkers`); emits `PrivateOscChanged(mark)` when any went.
    pub fn clear_program_marks(&self) -> bool {
        let changed = self.tracker().clear_program_marks();
        if changed {
            self.shared
                .notify_cleared(vec![crate::private_osc::Feature::Mark]);
        }
        changed
    }

    /// The `7722` marks this pane's program set, oldest first.
    pub fn program_marks(&self) -> Vec<crate::private_osc::ProgramMark> {
        self.tracker().program_marks()
    }

    /// The color this pane's program declared for its tab (`7719`), as sent.
    pub fn declared_tab_color(&self) -> Option<String> {
        self.tracker().declared_tab_color()
    }

    /// Drops stale and finished bars; emits `PrivateOscChanged(progress)` when any went.
    pub fn expire_progress(&self) -> bool {
        let changed = self.tracker().expire_progress(crate::commands::now_ms());
        if changed {
            self.shared
                .notify_cleared(vec![crate::private_osc::Feature::Progress]);
        }
        changed
    }

    /// Private-OSC counters (`applied`, `malformed`, `dropped`, `swallowed_*`).
    pub fn private_counters(&self) -> crate::private_osc::Counters {
        self.tracker().private_counters()
    }

    /// Takes queued events. Call this before `snapshot` so new output re-arms the waker.
    pub fn drain_events(&self) -> Vec<TermEvent> {
        self.shared.drain()
    }

    /// A byte stream that, fed to a fresh emulator of the same size, redraws the visible screen
    /// (and at most `max_history` scrollback rows), cursor, modes and title (SPEC D6). Recordings
    /// start with it so they do not begin blank.
    pub fn replay_bytes(&self, max_history: usize) -> Vec<u8> {
        let term = self.term.lock();
        crate::replay::synthesize(
            &term,
            self.shared.title().as_deref(),
            self.sgr_pixels.load(Ordering::Relaxed),
            max_history,
            &self.tracker(),
            &self.shared.vt.lock().unwrap(),
        )
    }

    /// Copies the visible rows; the terminal lock is held only for the copy.
    pub fn snapshot(&self) -> RenderSnapshot {
        let mut term = self.term.lock();
        let damage = take_damage(&mut term);
        let mut snap = snapshot::build(&term, damage, self.wide_ri.load(Ordering::Relaxed));
        snap.modes.sgr_pixels = self.sgr_pixels.load(Ordering::Relaxed);
        snap.vt = self.shared.vt.lock().unwrap().view();
        snap.line_base = self.tracker().trimmed();
        snap
    }

    /// A snapshot that leaves the damage state alone, so a one-off reader (a trigger
    /// `screenshot`) cannot make the renderer miss rows.
    pub fn peek_snapshot(&self) -> RenderSnapshot {
        let term = self.term.lock();
        let mut snap = snapshot::build(
            &term,
            snapshot::Damage::Full,
            self.wide_ri.load(Ordering::Relaxed),
        );
        snap.modes.sgr_pixels = self.sgr_pixels.load(Ordering::Relaxed);
        snap.vt = self.shared.vt.lock().unwrap().view();
        drop(term);
        snap.line_base = self.tracker().trimmed();
        snap
    }

    /// The logical line with stable id `id` plus context (trigger `snapshot`), cut to
    /// `max_chars` per line. `None` once the line left the scrollback.
    pub fn capture_lines(
        &self,
        id: u64,
        before: usize,
        after: usize,
        max_chars: usize,
    ) -> Option<crate::CapturedLines> {
        let term = self.term.lock();
        let trimmed = self.tracker().trimmed();
        crate::linetap::lines_around(&term, trimmed, id, before, after, max_chars)
    }

    /// The `[start, end)` stable-id regions of the prompt and typed input of the retained
    /// commands (empty without shell integration); see `CompletedLine::input`.
    pub fn input_line_regions(&self) -> Vec<(u64, u64)> {
        self.tracker().input_regions()
    }

    /// Lines that left the history for good: `line_base + absolute row` is a row's stable id.
    pub fn line_base(&self) -> u64 {
        self.tracker().trimmed()
    }

    /// Where the viewport sits, without touching the damage state a snapshot consumes:
    /// `(line_base, history size, display offset, rows)`.
    pub fn view_info(&self) -> (u64, usize, usize, usize) {
        let (history, offset, rows) = {
            use alacritty_terminal::grid::Dimensions;
            let term = self.term.lock();
            let g = term.grid();
            (g.history_size(), g.display_offset(), g.screen_lines())
        };
        (self.tracker().trimmed(), history, offset, rows)
    }

    /// Stable id of the cursor's row (`line_base + absolute row`), for markers on event rules.
    pub fn cursor_line_id(&self) -> u64 {
        let term = self.term.lock();
        let (abs, _) = crate::lines::cursor_abs(&term);
        drop(term);
        self.tracker().trimmed() + abs as u64
    }

    /// Scrolls so the row with stable id `id` is visible (centred when it was off screen).
    /// `false` when the row has left the scrollback.
    pub fn scroll_to_line_id(&self, id: u64) -> bool {
        let trimmed = self.tracker().trimmed();
        let Some(abs) = id.checked_sub(trimmed) else {
            return false;
        };
        let mut term = self.term.lock();
        if abs as usize >= crate::lines::total_lines(&term) {
            return false;
        }
        crate::lines::scroll_to_abs(&mut term, abs as usize, true);
        true
    }

    pub fn total_lines(&self) -> usize {
        crate::lines::total_lines(&self.term.lock())
    }

    pub fn history_size(&self) -> usize {
        crate::lines::history_size(&self.term.lock())
    }

    /// One row by absolute line (0 = oldest scrollback row).
    pub fn line_cells(&self, abs: usize) -> Option<crate::LineCells> {
        crate::lines::line_cells(&self.term.lock(), abs)
    }

    /// Several rows under one lock: absolute lines `from..to`, clamped to the grid.
    pub fn lines_range(&self, from: usize, to: usize) -> Vec<crate::LineCells> {
        let t = self.term.lock();
        let to = to.min(crate::lines::total_lines(&t));
        (from..to)
            .filter_map(|a| crate::lines::line_cells(&t, a))
            .collect()
    }

    /// Styled rows, absolute lines `from..to`, clamped to the grid.
    pub fn styled_lines(&self, from: usize, to: usize) -> Vec<Vec<crate::Cell>> {
        let t = self.term.lock();
        let to = to.min(crate::lines::total_lines(&t));
        (from..to)
            .filter_map(|a| crate::lines::styled_line(&t, a))
            .collect()
    }

    /// The terminal cursor as `(absolute line, column)`.
    pub fn cursor_abs(&self) -> (usize, usize) {
        crate::lines::cursor_abs(&self.term.lock())
    }

    pub fn select_abs(
        &self,
        kind: crate::SelectionKind,
        start: (usize, usize),
        end: (usize, usize),
    ) {
        crate::lines::select_abs(&mut self.term.lock(), kind, start, end);
    }

    pub fn scroll_to_abs(&self, abs: usize, center: bool) {
        crate::lines::scroll_to_abs(&mut self.term.lock(), abs, center);
    }

    /// Items shell programs registered for this pane (`OSC 7717`, SPEC §10.9.7), with the
    /// dropped and malformed counters.
    pub fn menu_items(&self) -> (Vec<crate::menu_reg::RegItem>, u64, u64) {
        let t = self.tracker();
        let m = t.menus();
        (m.items().to_vec(), m.dropped(), m.malformed())
    }

    fn tracker(&self) -> std::sync::MutexGuard<'_, CommandTracker> {
        self.tracker.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Absolute lines of OSC 133 prompt starts, ascending; empty without shell integration.
    pub fn prompt_marks(&self) -> Vec<usize> {
        self.tracker().prompt_lines()
    }

    /// Whether the shell ever sent an OSC 133 mark (shell integration is active).
    pub fn has_marks(&self) -> bool {
        self.tracker().has_marks()
    }

    /// A command is running: its `C` mark arrived and its `D` did not.
    pub fn busy(&self) -> bool {
        self.tracker().busy()
    }

    /// Number of commands that finished (OSC 133 `D`) over the terminal's life.
    pub fn commands_finished(&self) -> u64 {
        self.tracker().finished_total()
    }

    /// Sequence number the next command will get; remember it before typing a command line.
    pub fn next_command_seq(&self) -> u64 {
        self.tracker().next_seq()
    }

    /// The `n`-th most recent finished command (`1` = latest) with its output.
    pub fn last_command(&self, n: usize) -> Option<CommandInfo> {
        let term = self.term.lock();
        self.tracker().finished(&term, n)
    }

    /// The finished command with this sequence number, if it is still retained.
    pub fn command_by_seq(&self, seq: u64) -> Option<CommandInfo> {
        let term = self.term.lock();
        self.tracker().finished_seq(&term, seq)
    }

    /// Output of the running command so far.
    pub fn running_output(&self) -> Option<String> {
        let term = self.term.lock();
        self.tracker().running_output(&term)
    }

    /// The working directory last announced with OSC 7.
    pub fn reported_cwd(&self) -> Option<String> {
        self.tracker().cwd().map(str::to_string)
    }

    /// The whole grid as logical lines (soft wraps joined, trailing blanks trimmed, scrollback
    /// first), for `pane wait --match` and `pane run` output.
    pub fn logical_lines(&self) -> Vec<String> {
        crate::lines::logical_lines(&self.term.lock())
    }

    /// Prints `bytes` into the grid as if the program had written them, without touching the
    /// process (the `[process exited with status N]` notice).
    pub fn inject(&self, bytes: &[u8]) {
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        parser.advance(&mut *self.term.lock(), bytes);
        self.shared.notify(TermEvent::Wakeup);
    }

    /// The signal that ended the child, if it was killed by one.
    pub fn exit_signal(&self) -> Option<i32> {
        match self.shared.exit_signal.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n),
        }
    }

    /// Unix milliseconds when output last arrived (`0` before any).
    pub fn last_output_ms(&self) -> u64 {
        self.taps.last_output_ms.load(Ordering::Relaxed)
    }

    pub fn size(&self) -> (usize, usize) {
        let t = self.term.lock();
        use alacritty_terminal::grid::Dimensions;
        (t.columns(), t.screen_lines())
    }
}

/// Reads the PTY master, running the bytes through `WidthFilter` while
/// `text.wide_regional_indicators` is on.
///
/// The fd is non-blocking (shared with the master the `Pty` owns), so a read with nothing
/// pending fails with `WouldBlock`.
struct FilterReader {
    file: std::fs::File,
    enabled: Arc<AtomicBool>,
    filter: WidthFilter,
    sniffer: crate::mouse::PixelModeSniffer,
    taps: Arc<Taps>,
    raw: Vec<u8>,
    out: Vec<u8>,
}

impl FilterReader {
    fn new(
        file: std::fs::File,
        enabled: Arc<AtomicBool>,
        pixels: Arc<AtomicBool>,
        taps: Arc<Taps>,
    ) -> Self {
        FilterReader {
            file,
            enabled,
            filter: WidthFilter::new(),
            sniffer: crate::mouse::PixelModeSniffer::new(pixels),
            taps,
            raw: Vec::new(),
            out: Vec::new(),
        }
    }

    fn touch(&self) {
        self.taps
            .last_output_ms
            .store(crate::commands::now_ms(), Ordering::Relaxed);
    }
}

impl io::Read for FilterReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.enabled.load(Ordering::Relaxed) {
            // Switched off: give back anything held, then read straight through.
            let held = self.filter.take_pending();
            self.filter.reset();
            if !held.is_empty() && held.len() <= buf.len() {
                buf[..held.len()].copy_from_slice(&held);
                return Ok(held.len());
            }
            let n = self.file.read(buf)?;
            self.taps.output_bytes(&buf[..n]);
            self.sniffer.feed(&buf[..n]);
            self.touch();
            return Ok(n);
        }
        // Every 4-byte indicator grows by one byte and up to 3 held-back bytes are re-emitted,
        // so reading at most 4/5 of the buffer (minus 4) guarantees the output fits.
        let cap = (buf.len() * 4 / 5).saturating_sub(4);
        if cap == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        loop {
            self.raw.resize(cap, 0);
            let n = self.file.read(&mut self.raw)?;
            if n == 0 {
                let held = self.filter.take_pending();
                buf[..held.len()].copy_from_slice(&held);
                return Ok(held.len());
            }
            self.out.clear();
            self.taps.output_bytes(&self.raw[..n]);
            self.sniffer.feed(&self.raw[..n]);
            self.touch();
            self.filter.filter(&self.raw[..n], &mut self.out);
            if self.out.is_empty() {
                continue; // only a partial UTF-8 sequence so far; read on
            }
            buf[..self.out.len()].copy_from_slice(&self.out);
            return Ok(self.out.len());
        }
    }
}

/// What the local reader thread needs.
struct LocalIo {
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    tracker: Arc<Mutex<CommandTracker>>,
    reader: FilterReader,
    pid: u32,
}

/// Longest the reader sleeps without checking whether the child exited.
const POLL_MS: i32 = 50;
/// A parse never holds the terminal lock for more than one read of this many bytes.
const READ_CHUNK: usize = 64 * 1024;

#[allow(unsafe_code)]
fn poll_fd(fd: RawFd, events: i16, timeout_ms: i32) -> bool {
    let mut p = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the duration of the call.
    let r = unsafe { libc::poll(&mut p, 1, timeout_ms) };
    r > 0
}

fn write_all_nb(file: &mut std::fs::File, mut buf: &[u8]) -> io::Result<()> {
    use io::Write;
    let fd = file.as_raw_fd();
    while !buf.is_empty() {
        match file.write(buf) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => buf = &buf[n..],
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                poll_fd(fd, libc::POLLOUT, 100);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// How the child ended: `(exit code, terminating signal)`, once it did.
type ExitSlot = Arc<(
    Mutex<Option<(Option<i32>, Option<i32>)>>,
    std::sync::Condvar,
)>;

/// `(exit code, terminating signal)` of the child.
#[allow(unsafe_code)]
fn wait_status(pid: u32) -> (Option<i32>, Option<i32>) {
    let mut status = 0;
    // SAFETY: waitpid on our own child; the Pty's drop then sees ECHILD.
    let r = unsafe { libc::waitpid(pid as i32, &mut status, 0) };
    if r < 0 {
        return (None, None);
    }
    if libc::WIFEXITED(status) {
        (Some(libc::WEXITSTATUS(status)), None)
    } else if libc::WIFSIGNALED(status) {
        (None, Some(libc::WTERMSIG(status)))
    } else {
        (None, None)
    }
}

/// Starts the writer, the child waiter and the reader that parses output. Returns the channel
/// that queues bytes for the child; sends never block, a full PTY only stalls the writer.
///
/// We run this ourselves instead of the engine's `EventLoop` so that OSC 133 marks are applied
/// at the exact byte they arrive on (`commands::drive`).
fn spawn_local_io(
    pty: &tty::Pty,
    io: LocalIo,
) -> io::Result<std::sync::mpsc::Sender<Cow<'static, [u8]>>> {
    let (tx, rx) = std::sync::mpsc::channel::<Cow<'static, [u8]>>();
    let mut writer = pty.file().try_clone()?;
    std::thread::Builder::new()
        .name("PTY writer".into())
        .spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if bytes.is_empty() {
                    continue;
                }
                if write_all_nb(&mut writer, &bytes).is_err() {
                    break;
                }
            }
        })?;

    let exited: ExitSlot = Arc::new((Mutex::new(None), std::sync::Condvar::new()));
    let ex = exited.clone();
    let pid = io.pid;
    std::thread::Builder::new()
        .name("PTY waiter".into())
        .spawn(move || {
            let status = wait_status(pid);
            *ex.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
            ex.1.notify_all();
        })?;

    std::thread::Builder::new()
        .name("PTY reader".into())
        .spawn(move || run_reader(io, exited))?;
    Ok(tx)
}

fn run_reader(mut io: LocalIo, exited: ExitSlot) {
    use alacritty_terminal::vte::ansi::Processor;
    use io::Read;
    let fd = io.reader.file.as_raw_fd();
    let mut parser: Processor = Processor::default();
    let mut osc = OscSniffer::default();
    let mut buf = vec![0u8; READ_CHUNK];
    let has_exited = |e: &ExitSlot| e.0.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    loop {
        // Wake for a synchronized-update timeout as well as for output.
        let timeout = parser.sync_timeout().sync_timeout().map_or(POLL_MS, |t| {
            (t.saturating_duration_since(std::time::Instant::now())
                .as_millis() as i32)
                .clamp(0, POLL_MS)
        });
        if !poll_fd(fd, libc::POLLIN, timeout) {
            if parser
                .sync_timeout()
                .sync_timeout()
                .is_some_and(|t| t <= std::time::Instant::now())
            {
                let mut term = io.term.lock();
                parser.stop_sync(&mut *term);
                drop(term);
                io.shared.notify(TermEvent::Wakeup);
            }
            if has_exited(&exited) {
                break;
            }
            continue;
        }
        let n = match io.reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => break, // EIO once the child side closed (Linux)
        };
        {
            let mut term = io.term.lock();
            let mut tracker = io.tracker.lock().unwrap_or_else(|e| e.into_inner());
            let shared = io.shared.clone();
            drive(
                &mut osc,
                &mut tracker,
                &shared,
                &mut term,
                &buf[..n],
                |term, seg| parser.advance(term, seg),
                |code| shared.notify(TermEvent::CommandFinished(code)),
            );
        }
        if parser.sync_bytes_count() < n {
            io.shared.notify(TermEvent::Wakeup);
        }
    }
    // Output is drained; report how the child ended (it may still be shutting down).
    let status = {
        let mut g = exited.0.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while g.is_none() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            g = exited
                .1
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        g.unwrap_or((None, None))
    };
    if let Some(sig) = status.1 {
        io.shared.exit_signal.store(sig, Ordering::Relaxed);
    }
    let cleared = io
        .tracker
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear_private();
    io.shared.notify_cleared(cleared);
    io.shared.notify(TermEvent::ChildExit(status.0));
    io.shared.notify(TermEvent::Wakeup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn spawn_sh(script: &str) -> (Terminal, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let t = Terminal::spawn(
            SpawnOptions {
                shell: "/bin/sh".into(),
                cols: 160,
                rows: 5,
                env: vec![("MT_TEST".into(), "yes".into())],
                ..Default::default()
            },
            move || {
                let _ = tx.send(());
            },
        )
        .unwrap();
        t.write(format!("{script}\n").into_bytes());
        (t, rx)
    }

    fn wait_for(t: &Terminal, rx: &mpsc::Receiver<()>, needle: &str) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let _ = rx.recv_timeout(Duration::from_millis(100));
            t.drain_events();
            let s = t.snapshot();
            let rows: Vec<String> = (0..s.rows).map(|r| s.row_text(r)).collect();
            if rows.iter().any(|l| l.contains(needle)) || Instant::now() > deadline {
                return rows;
            }
        }
    }

    #[test]
    fn record_tap_carries_output_input_echo_and_resize() {
        let (t, rx) = spawn_sh("echo tapready");
        let (tx, trx) = mpsc::channel();
        t.set_record_tap(Some(tx));
        t.write(b"echo second-$((1+1))\n".to_vec());
        t.resize(100, 7, 8, 16);
        wait_for(&t, &rx, "second-2");
        t.set_record_tap(None);
        let evs: Vec<TapEvent> = trx.try_iter().collect();
        assert!(evs.iter().any(|e| matches!(&e.kind, TapKind::Output(b)
            if String::from_utf8_lossy(b).contains("second-2"))));
        assert!(evs.iter().any(|e| matches!(&e.kind,
            TapKind::Input { bytes, hidden: Some(false) } if bytes == b"echo second-$((1+1))\n")));
        assert!(evs
            .iter()
            .any(|e| matches!(e.kind, TapKind::Resize { cols: 100, rows: 7 })));
        assert!(evs.windows(2).all(|w| w[0].at <= w[1].at));
    }

    #[test]
    fn lines_typed_on_are_input_even_without_shell_integration() {
        let (t, rx) = spawn_sh("echo ready-$((1+1)); read x; echo got-$x; sleep 5");
        wait_for(&t, &rx, "ready-2");
        t.write(b"typed-line-x\r".to_vec());
        wait_for(&t, &rx, "got-typed-line-x");
        let base = t.line_base();
        let id_of = |needle: &str| {
            t.lines_range(0, t.total_lines())
                .iter()
                .position(|l| l.text().starts_with(needle))
                .map(|i| base + i as u64)
                .expect(needle)
        };
        let regions = t.input_line_regions();
        assert!(crate::in_regions(&regions, id_of("typed-line-x")));
        assert!(!crate::in_regions(&regions, id_of("ready-2")));
        assert!(!crate::in_regions(&regions, id_of("got-typed-line-x")));
        t.write(vec![3]);
    }

    #[test]
    fn echo_flag_follows_the_pty_and_untapped_writes_are_invisible() {
        // A password prompt: ECHO off while the program holds the tty in canonical mode.
        let (t, rx) = spawn_sh("stty -echo; echo noecho-$((1+1)); sleep 5");
        wait_for(&t, &rx, "noecho-2");
        assert_eq!(t.input_hidden(), Some(true));
        let (tx, trx) = mpsc::channel();
        t.set_record_tap(Some(tx));
        t.write(b"hunter2\n".to_vec());
        t.write_untapped(b"SECRET\n".to_vec());
        t.paste_untapped("PASTED");
        t.write(vec![3]); // ^C ends the sleep
        t.set_record_tap(None);
        let inputs: Vec<(Vec<u8>, Option<bool>)> = trx
            .try_iter()
            .filter_map(|e| match e.kind {
                TapKind::Input { bytes, hidden } => Some((bytes, hidden)),
                _ => None,
            })
            .collect();
        assert_eq!(inputs[0], (b"hunter2\n".to_vec(), Some(true)));
        assert_eq!(inputs.len(), 2);
        assert!(inputs
            .iter()
            .all(|(b, _)| !String::from_utf8_lossy(b).contains("SECRET")
                && !String::from_utf8_lossy(b).contains("PASTED")));
    }

    #[test]
    fn replay_bytes_skip_the_record_tap_only() {
        let t = Terminal::remote(
            SpawnOptions {
                cols: 20,
                rows: 3,
                ..Default::default()
            },
            RemoteIo {
                input: Box::new(|_| {}),
                resize: Box::new(|_, _| {}),
            },
            || {},
        );
        let (tx, trx) = mpsc::channel();
        let (otx, orx) = mpsc::channel();
        t.set_record_tap(Some(tx));
        t.set_output_tap(Some(otx));
        t.feed_replay(b"replayed");
        t.feed(b"live");
        assert_eq!(trx.try_iter().count(), 1);
        assert_eq!(orx.try_iter().count(), 2);
        assert_eq!(t.input_hidden(), None);
    }

    #[test]
    fn reset_clears_modes_and_keeps_scrollback() {
        let (t, rx) = spawn_sh(
            "i=0; while [ $i -lt 20 ]; do echo line$i; i=$((i+1)); done; \
             printf '\\033[?2004h\\033[?1h\\033[3 q'; echo ready",
        );
        wait_for(&t, &rx, "ready");
        std::thread::sleep(Duration::from_millis(100));
        t.drain_events();
        assert!(t.modes().bracketed_paste && t.modes().app_cursor);
        let hist = t.history_size();
        assert!(hist > 0);
        t.reset();
        let m = t.modes();
        assert!(!m.bracketed_paste && !m.app_cursor && !m.alt_screen);
        assert_eq!(t.history_size(), hist, "scrollback is kept");
    }

    #[test]
    fn shell_runs_and_exports_env() {
        let (t, rx) = spawn_sh("echo env:$TERM:$COLORTERM:$TERM_PROGRAM:$MT_TEST");
        let rows = wait_for(&t, &rx, "env:xterm-256color:truecolor:microterm:yes");
        assert!(
            rows.iter()
                .any(|l| l.contains("env:xterm-256color:truecolor:microterm:yes")),
            "{rows:?}"
        );
    }

    #[test]
    fn taps_copy_both_directions_in_order() {
        let (t, rx) = spawn_sh("true");
        let (otx, orx) = mpsc::channel();
        let (itx, irx) = mpsc::channel();
        t.set_output_tap(Some(otx));
        t.set_input_tap(Some(itx));
        t.write(b"echo tap-$((20+22))\n".to_vec());
        wait_for(&t, &rx, "tap-42");
        assert_eq!(irx.try_recv().unwrap(), b"echo tap-$((20+22))\n");
        let out: Vec<u8> = orx.try_iter().flatten().collect();
        assert!(String::from_utf8_lossy(&out).contains("tap-42"));
        t.set_output_tap(None);
        t.write(b"echo more\n".to_vec());
        wait_for(&t, &rx, "more");
        assert!(orx.try_recv().is_err());
    }

    /// Column of the `x` that follows a flag in the row whose text starts with that flag.
    fn x_after_flag(t: &Terminal) -> Option<usize> {
        let s = t.snapshot();
        (0..s.rows).find_map(|r| {
            let row = s.row(r);
            (row.first().is_some_and(|c| c.ch == '\u{1F1EF}'))
                .then(|| row.iter().position(|c| c.ch == 'x'))
                .flatten()
        })
    }

    #[test]
    fn wide_regional_indicators_apply_to_live_pty_output_and_toggle_at_runtime() {
        let (tx, rx) = mpsc::channel();
        let t = Terminal::spawn(
            SpawnOptions {
                shell: "/bin/sh".into(),
                cols: 80,
                rows: 8,
                wide_regional_indicators: true,
                ..Default::default()
            },
            move || {
                let _ = tx.send(());
            },
        )
        .unwrap();
        // U+1F1EF U+1F1F5 then x, written as octal bytes so the echoed command is plain ASCII.
        let flag_x = r"printf '\360\237\207\257\360\237\207\265x\n'";
        t.write(format!("{flag_x}\n").into_bytes());
        let rows = wait_for(&t, &rx, "🇯🇵x");
        assert!(rows.iter().any(|l| l.contains("🇯🇵x")), "{rows:?}");
        assert_eq!(x_after_flag(&t), Some(4), "four cells per flag when on");

        t.set_wide_regional_indicators(false);
        t.write(format!("clear; {flag_x}\n").into_bytes());
        let deadline = Instant::now() + Duration::from_secs(10);
        while x_after_flag(&t) != Some(2) && Instant::now() < deadline {
            let _ = rx.recv_timeout(Duration::from_millis(100));
            t.drain_events();
        }
        assert_eq!(x_after_flag(&t), Some(2), "two cells per flag when off");
    }

    #[test]
    fn resize_updates_stty_size() {
        let (t, rx) = spawn_sh("sleep 0.3; stty size");
        t.resize(50, 7, 8, 16);
        let rows = wait_for(&t, &rx, "7 50");
        assert!(rows.iter().any(|l| l.trim() == "7 50"), "{rows:?}");
        assert_eq!(t.size(), (50, 7));
    }

    #[test]
    fn typed_keys_reach_the_child() {
        let (t, rx) = spawn_sh("cat");
        std::thread::sleep(Duration::from_millis(200));
        t.send_key(&KeyInput::new(
            crate::input::Key::Char('h'),
            Default::default(),
        ));
        t.send_key(&KeyInput::new(
            crate::input::Key::Char('i'),
            Default::default(),
        ));
        t.send_key(&KeyInput::new(crate::input::Key::Enter, Default::default()));
        let rows = wait_for(&t, &rx, "hi");
        assert!(rows.iter().any(|l| l.trim() == "hi"), "{rows:?}");
    }

    #[test]
    fn foreground_process_and_cwd_are_tracked() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let (t, _rx) = spawn_sh(&format!("cd {} && exec sleep 20", dir.display()));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut cwd = None;
        while Instant::now() < deadline {
            cwd = t.cwd().and_then(|p| p.canonicalize().ok());
            if cwd.as_ref() == Some(&dir) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(cwd, Some(dir));
    }

    #[test]
    fn child_of_shell_is_foreground() {
        let (t, _rx) = spawn_sh("sleep 5");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut name = None;
        while name.is_none() && Instant::now() < deadline {
            name = t.foreground_process().map(|p| p.name);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(name.as_deref(), Some("sleep"));
    }

    #[test]
    fn exit_is_reported() {
        let (t, rx) = spawn_sh("exit 3");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut code = None;
        while code.is_none() && Instant::now() < deadline {
            let _ = rx.recv_timeout(Duration::from_millis(100));
            for e in t.drain_events() {
                if let TermEvent::ChildExit(c) = e {
                    code = Some(c);
                }
            }
        }
        assert_eq!(code, Some(Some(3)));
    }

    #[test]
    fn remote_terminal_parses_fed_bytes_and_forwards_input_and_resize() {
        use std::sync::Mutex as M;
        let sent = Arc::new(M::new(Vec::<u8>::new()));
        let sizes = Arc::new(M::new(Vec::<(u16, u16)>::new()));
        let (s2, z2) = (sent.clone(), sizes.clone());
        let t = Terminal::remote(
            SpawnOptions::default(),
            RemoteIo {
                input: Box::new(move |b| s2.lock().unwrap().extend(b)),
                resize: Box::new(move |c, r| z2.lock().unwrap().push((c, r))),
            },
            || {},
        );
        assert!(t.is_remote());
        t.feed(b"hi\x1b[1;31m!\x1b[c");
        let snap = t.snapshot();
        assert_eq!(snap.row_text(0), "hi!");
        // The device-attributes query is answered through the remote input path.
        assert!(sent.lock().unwrap().starts_with(b"\x1b[?"));
        t.write(&b"ls\r"[..]);
        assert!(sent.lock().unwrap().ends_with(b"ls\r"));
        t.resize(100, 30, 8, 16);
        assert_eq!(*sizes.lock().unwrap(), vec![(100, 30)]);
        assert_eq!(t.size(), (100, 30));
        t.remote_exited(Some(2));
        assert!(t
            .drain_events()
            .iter()
            .any(|e| matches!(e, TermEvent::ChildExit(Some(2)))));
        assert!(t.foreground_process().is_none() && t.cwd().is_none());
    }

    fn line_cfg(partial_after_ms: u32) -> LineTapCfg {
        LineTapCfg {
            max_line_chars: 4096,
            scan_alt_screen: false,
            partial_after_ms,
        }
    }

    /// Collects lines until `done` accepts what has arrived (or 10 s pass).
    fn wait_lines(
        q: &LineQueue,
        done: impl Fn(&[crate::CompletedLine]) -> bool,
    ) -> Vec<crate::CompletedLine> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while !done(&got) && Instant::now() < deadline {
            got.extend(q.recv_batch(100, Duration::from_millis(100)));
        }
        got
    }

    #[test]
    fn line_tap_sees_live_pty_lines_and_a_prompt_without_a_newline() {
        let (t, rx) = spawn_sh("echo rd$((1+1))");
        wait_for(&t, &rx, "rd2");
        std::thread::sleep(Duration::from_millis(300));
        let q = LineQueue::new(4096);
        assert!(!t.line_tap_installed());
        t.set_line_tap(Some(line_cfg(150)), q.clone(), 9);
        assert!(t.line_tap_installed());
        t.write(b"echo 'error: build failed'; printf 'Continue? [y/N] '; sleep 2\n".to_vec());
        let got = wait_lines(&q, |l| {
            l.iter().any(|x| x.partial && x.text == "Continue? [y/N]")
                && l.iter().any(|x| x.text == "error: build failed")
        });
        let texts: Vec<_> = got.iter().map(|l| (l.text.as_str(), l.partial)).collect();
        assert!(texts.contains(&("error: build failed", false)), "{texts:?}");
        assert!(texts.contains(&("Continue? [y/N]", true)), "{texts:?}");
        assert!(got.iter().all(|l| l.pane == 9));
        // Removing the tap stops delivery (and the flusher).
        t.set_line_tap(None, q.clone(), 9);
        assert!(!t.line_tap_installed());
        t.write(b"echo after-rem$((1+1))\n".to_vec());
        wait_for(&t, &rx, "after-rem2");
        assert!(q.recv_batch(100, Duration::from_millis(50)).is_empty());
    }

    #[test]
    fn replayed_bytes_do_not_reach_the_line_tap_but_live_ones_do() {
        let t = Terminal::remote(
            SpawnOptions::default(),
            RemoteIo {
                input: Box::new(|_| {}),
                resize: Box::new(|_, _| {}),
            },
            || {},
        );
        let q = LineQueue::new(100);
        t.set_line_tap(Some(line_cfg(0)), q.clone(), 3);
        t.feed_replay(b"old error: replayed\r\nContinue? [y/N] ");
        assert!(q.recv_batch(10, Duration::from_millis(0)).is_empty());
        t.feed(b"\r\nlive error: fresh\r\nx");
        // The lines are read out of the grid a moment later, by the tap's own thread.
        let texts: Vec<_> = wait_lines(&q, |l| !l.is_empty())
            .into_iter()
            .map(|l| l.text)
            .collect();
        // The replayed prompt was not scanned, and finishing it unchanged is not a new scan.
        assert_eq!(texts, ["live error: fresh"]);
    }
}
