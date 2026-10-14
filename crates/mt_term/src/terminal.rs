//! A live terminal: PTY, reader thread and emulator behind one handle.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty::{self, Shell};

use crate::emulator::{take_damage, term_config, ScrollCmd, Size};
use crate::events::{Listener, Osc52Policy, Palette, Shared, TermEvent};
use crate::input::{encode_key, encode_paste, InputMode, KeyInput};
use crate::shell::resolve_shell;
use crate::snapshot::{self, Modes, RenderSnapshot};

pub struct SpawnOptions {
    /// `shell` config value; empty means `$SHELL`, then the passwd entry, then `/bin/sh`.
    pub shell: String,
    pub cwd: Option<PathBuf>,
    /// Extra environment for the child (`[env]` config table).
    pub env: Vec<(String, String)>,
    /// Value of `$TERM`.
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    pub scrollback: usize,
    pub osc52: Osc52Policy,
    pub palette: Palette,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        SpawnOptions {
            shell: String::new(),
            cwd: None,
            env: Vec::new(),
            term: "xterm-256color".into(),
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
            scrollback: 10_000,
            osc52: Osc52Policy::Write,
            palette: Palette::default(),
        }
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    sender: EventLoopSender,
    pid: u32,
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
            term_config(opts.scrollback, opts.osc52),
            &Size {
                cols: opts.cols as usize,
                rows: opts.rows as usize,
            },
            listener.clone(),
        );
        let term = Arc::new(FairMutex::new(term));

        let program = resolve_shell(&opts.shell);
        // macOS terminals start login shells so PATH matches Terminal.app.
        let args = if cfg!(target_os = "macos") {
            vec!["-l".to_string()]
        } else {
            Vec::new()
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
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
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

    pub fn set_palette(&self, palette: Palette) {
        *self.shared.palette.lock().unwrap() = palette;
    }

    pub fn modes(&self) -> Modes {
        Modes::from_engine(*self.term.lock().mode())
    }

    pub fn input_mode(&self) -> InputMode {
        self.modes().input_mode()
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
        snapshot::build(&term, damage)
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
