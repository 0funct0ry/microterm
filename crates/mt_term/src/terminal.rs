//! A live terminal: PTY, reader thread and emulator behind one handle.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite, Shell};

use crate::emulator::{take_damage, term_config, CursorDefault, ScrollCmd, Size};
use crate::events::{Listener, Osc52Policy, Palette, Shared, TermEvent};
use crate::input::{encode_key, encode_paste, InputMode, KeyInput};
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
        }
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    sender: EventLoopSender,
    pid: u32,
    /// PTY master fd, for foreground-process queries. Owned by the event loop.
    master_fd: RawFd,
    scrollback: usize,
    /// `text.wide_regional_indicators`, shared with the PTY reader.
    wide_ri: Arc<AtomicBool>,
    /// DECSET 1016 state, set by the PTY reader's sniffer.
    sgr_pixels: Arc<AtomicBool>,
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
        let pty = FilteredPty::new(pty, wide_ri.clone(), sgr_pixels.clone())?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        shared.set_pty(sender.clone());
        // The loop thread owns the PTY; it exits on Shutdown or child exit.
        let _ = event_loop.spawn();
        Ok(Terminal {
            term,
            shared,
            sender,
            pid,
            master_fd,
            scrollback: opts.scrollback,
            wide_ri,
            sgr_pixels,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The foreground process when it is not the shell itself (drives the dirty dot and the
    /// `confirm_close = \"process\"` policy).
    pub fn foreground_process(&self) -> Option<crate::ForegroundProcess> {
        crate::process::foreground(self.master_fd, self.pid)
    }

    /// Working directory of the foreground process, falling back to the shell's.
    pub fn cwd(&self) -> Option<PathBuf> {
        let pid = self.foreground_process().map_or(self.pid, |p| p.pid);
        crate::process::process_cwd(pid).or_else(|| crate::process::process_cwd(self.pid))
    }

    /// Queues bytes for the child. Never blocks on a full PTY buffer.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
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

    pub fn resize(&self, cols: u16, rows: u16, cell_width: u16, cell_height: u16) {
        let ws = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width,
            cell_height,
        };
        *self.shared.size.lock().unwrap() = ws;
        self.term.lock().resize(Size {
            cols: cols as usize,
            rows: rows as usize,
        });
        let _ = self.sender.send(Msg::Resize(ws));
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
                self.write(b);
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
        self.term.lock().grid_mut().clear_history();
    }

    /// Takes queued events. Call this before `snapshot` so new output re-arms the waker.
    pub fn drain_events(&self) -> Vec<TermEvent> {
        self.shared.drain()
    }

    /// Copies the visible rows; the terminal lock is held only for the copy.
    pub fn snapshot(&self) -> RenderSnapshot {
        let mut term = self.term.lock();
        let damage = take_damage(&mut term);
        let mut snap = snapshot::build(&term, damage, self.wide_ri.load(Ordering::Relaxed));
        snap.modes.sgr_pixels = self.sgr_pixels.load(Ordering::Relaxed);
        snap
    }

    pub fn size(&self) -> (usize, usize) {
        let t = self.term.lock();
        use alacritty_terminal::grid::Dimensions;
        (t.columns(), t.screen_lines())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

/// The PTY handed to the engine's event loop, with the width filter in the read path.
///
/// The loop reads through `reader()`; this wrapper reads from a duplicate of the master fd
/// (same open file, so readiness and `O_NONBLOCK` are shared) and runs the bytes through
/// `WidthFilter` while `text.wide_regional_indicators` is on.
struct FilteredPty {
    inner: tty::Pty,
    reader: FilterReader,
}

struct FilterReader {
    file: std::fs::File,
    enabled: Arc<AtomicBool>,
    filter: WidthFilter,
    sniffer: crate::mouse::PixelModeSniffer,
    raw: Vec<u8>,
    out: Vec<u8>,
}

impl FilteredPty {
    fn new(inner: tty::Pty, enabled: Arc<AtomicBool>, pixels: Arc<AtomicBool>) -> io::Result<Self> {
        let file = inner.file().try_clone()?;
        Ok(Self {
            inner,
            reader: FilterReader {
                file,
                enabled,
                filter: WidthFilter::new(),
                sniffer: crate::mouse::PixelModeSniffer::new(pixels),
                raw: Vec::new(),
                out: Vec::new(),
            },
        })
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
            self.sniffer.feed(&buf[..n]);
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
            self.sniffer.feed(&self.raw[..n]);
            self.filter.filter(&self.raw[..n], &mut self.out);
            if self.out.is_empty() {
                continue; // only a partial UTF-8 sequence so far; read on
            }
            buf[..self.out.len()].copy_from_slice(&self.out);
            return Ok(self.out.len());
        }
    }
}

impl EventedReadWrite for FilteredPty {
    type Reader = FilterReader;
    type Writer = std::fs::File;

    unsafe fn register(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        unsafe { self.inner.register(poll, interest, mode) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        self.inner.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<polling::Poller>) -> io::Result<()> {
        self.inner.deregister(poll)
    }

    fn reader(&mut self) -> &mut FilterReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut std::fs::File {
        self.inner.writer()
    }
}

impl EventedPty for FilteredPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.inner.next_child_event()
    }
}

impl OnResize for FilteredPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.inner.on_resize(window_size);
    }
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
    fn shell_runs_and_exports_env() {
        let (t, rx) = spawn_sh("echo env:$TERM:$COLORTERM:$TERM_PROGRAM:$MT_TEST");
        let rows = wait_for(&t, &rx, "env:xterm-256color:truecolor:microterm:yes");
        assert!(
            rows.iter()
                .any(|l| l.contains("env:xterm-256color:truecolor:microterm:yes")),
            "{rows:?}"
        );
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
}
