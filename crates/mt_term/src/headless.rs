//! A PTY-backed pane without a UI: the building block of `microterm-mux` (SPEC §11, D6).
//!
//! The reader thread feeds an authoritative [`Emulator`] and hands every chunk to a sink while
//! still holding the state lock. A concurrent [`HeadlessPane::replay`] therefore sees a
//! `(seq, bytes)` pair that is consistent with the stream: output with `seq` greater than the
//! replay's is exactly what the replay does not contain.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, Shell};

use crate::emulator::Emulator;
use crate::shell::resolve_shell;

pub struct HeadlessOptions {
    /// `None` runs the user's shell.
    pub command: Option<Vec<String>>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: usize,
}

/// What a pane reports to its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneEvent {
    /// New output. `seq` is the total number of bytes produced including this chunk.
    Output {
        seq: u64,
        bytes: Vec<u8>,
    },
    Title(String),
    /// The process ended (`None` when killed by a signal). Always the last event.
    Exited(Option<i32>),
}

struct State {
    emu: Emulator,
    seq: u64,
    title: Option<String>,
}

pub struct HeadlessPane {
    state: Arc<Mutex<State>>,
    pty: Mutex<tty::Pty>,
    input: Sender<Vec<u8>>,
    pid: u32,
    scrollback: usize,
}

impl HeadlessPane {
    /// Spawns the process. `sink` runs on the reader thread, in output order, with the state
    /// lock held: keep it short and never call back into this pane.
    pub fn spawn(
        opts: HeadlessOptions,
        sink: impl Fn(PaneEvent) + Send + 'static,
    ) -> io::Result<HeadlessPane> {
        let window_size = WindowSize {
            num_lines: opts.rows,
            num_cols: opts.cols,
            cell_width: 8,
            cell_height: 16,
        };
        let (program, args) = match opts.command.as_deref() {
            Some([program, rest @ ..]) => (program.clone(), rest.to_vec()),
            // A login shell, like an interactive `ssh host`: the daemon's own environment is the
            // minimal one sshd gives exec channels (no Homebrew PATH, no profile).
            _ => (resolve_shell(""), vec!["-l".to_string()]),
        };
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
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
        let pty = tty::new(
            &tty::Options {
                shell: Some(Shell::new(program, args)),
                // Home, as `ssh host` would start; the daemon itself runs in `/`.
                working_directory: opts
                    .cwd
                    .or_else(|| std::env::var_os("HOME").map(PathBuf::from)),
                drain_on_exit: true,
                env,
            },
            window_size,
            0,
        )?;
        let pid = pty.child().id();
        let reader = pty.file().try_clone()?;
        let mut writer = pty.file().try_clone()?;
        // The engine's PTY is non-blocking; these threads block instead.
        clear_nonblocking(&reader)?;

        let mut emu = Emulator::new(opts.cols as usize, opts.rows as usize, opts.scrollback);
        // The daemon keeps private-OSC state for replay and never answers a program itself;
        // the attached client's gate decides what the replayed bytes show (SPEC §6.4.3).
        emu.set_vt_config(crate::VtConfig {
            private: crate::private_osc::PrivateGate::store_only(),
            ..crate::VtConfig::default()
        });
        let state = Arc::new(Mutex::new(State {
            emu,
            seq: 0,
            title: None,
        }));

        let (input, rx) = channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("mux-pty-writer".into())
            .spawn(move || {
                while let Ok(bytes) = rx.recv() {
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })?;

        let st = state.clone();
        std::thread::Builder::new()
            .name("mux-pty-reader".into())
            .spawn(move || {
                let mut reader = reader;
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break, // EIO once the child side closes (Linux)
                    };
                    let mut s = st.lock().unwrap_or_else(|e| e.into_inner());
                    s.emu.advance(&buf[..n]);
                    s.seq += n as u64;
                    let seq = s.seq;
                    sink(PaneEvent::Output {
                        seq,
                        bytes: buf[..n].to_vec(),
                    });
                    let title = s.emu.title();
                    if title != s.title {
                        s.title = title.clone();
                        if let Some(t) = title {
                            sink(PaneEvent::Title(t));
                        }
                    }
                }
                sink(PaneEvent::Exited(wait_status(pid)));
            })?;

        Ok(HeadlessPane {
            state,
            pty: Mutex::new(pty),
            input,
            pid,
            scrollback: opts.scrollback,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Queues keystrokes for the process; never blocks.
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(bytes);
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let ws = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: 8,
            cell_height: 16,
        };
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.emu.resize(cols as usize, rows as usize);
        self.pty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .on_resize(ws);
    }

    pub fn size(&self) -> (usize, usize) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let snap = s.emu.snapshot();
        (snap.cols, snap.rows)
    }

    pub fn title(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .emu
            .title()
    }

    /// The current state as a VT stream plus the sequence number it corresponds to.
    pub fn replay(&self) -> (u64, Vec<u8>) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (s.seq, s.emu.replay_bytes(self.scrollback))
    }

    /// Runs `f` with the sequence number while output is paused. Used to register a client's
    /// live subscription atomically with its replay.
    pub fn replay_with<R>(&self, f: impl FnOnce(u64, Vec<u8>) -> R) -> R {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(s.seq, s.emu.replay_bytes(self.scrollback))
    }

    /// Visible rows as text (for `microterm-mux list`).
    pub fn screen_text(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .emu
            .screen_text()
    }

    /// Sends SIGHUP to the process group.
    pub fn kill(&self) {
        // SAFETY: plain signal delivery to a pid we spawned.
        #[allow(unsafe_code)]
        unsafe {
            libc::kill(self.pid as i32, libc::SIGHUP);
        }
    }
}

#[allow(unsafe_code)]
fn clear_nonblocking(f: &File) -> io::Result<()> {
    let fd = f.as_raw_fd();
    // SAFETY: fcntl on a valid fd we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[allow(unsafe_code)]
fn wait_status(pid: u32) -> Option<i32> {
    let mut status = 0;
    // SAFETY: waitpid on our own child; the Pty's later wait just sees ECHILD.
    let r = unsafe { libc::waitpid(pid as i32, &mut status, 0) };
    if r < 0 {
        return None;
    }
    if libc::WIFEXITED(status) {
        Some(libc::WEXITSTATUS(status))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn spawn(cmd: &[&str]) -> (HeadlessPane, mpsc::Receiver<PaneEvent>) {
        let (tx, rx) = mpsc::channel();
        let p = HeadlessPane::spawn(
            HeadlessOptions {
                command: Some(cmd.iter().map(|s| s.to_string()).collect()),
                cwd: None,
                env: vec![],
                cols: 40,
                rows: 6,
                scrollback: 100,
            },
            move |e| {
                let _ = tx.send(e);
            },
        )
        .unwrap();
        (p, rx)
    }

    fn collect_until(
        rx: &mpsc::Receiver<PaneEvent>,
        f: impl Fn(&PaneEvent) -> bool,
    ) -> Vec<PaneEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(10)) {
            let done = f(&e);
            out.push(e);
            if done {
                return out;
            }
        }
        panic!("timed out; got {out:?}");
    }

    #[test]
    fn output_exit_status_and_replay_match() {
        let (p, rx) = spawn(&["/bin/sh", "-c", "printf 'hello\\r\\nworld'; exit 3"]);
        let evs = collect_until(&rx, |e| matches!(e, PaneEvent::Exited(_)));
        assert_eq!(evs.last(), Some(&PaneEvent::Exited(Some(3))));
        let mut total = 0;
        for e in &evs {
            if let PaneEvent::Output { seq, bytes } = e {
                total += bytes.len() as u64;
                assert_eq!(*seq, total, "seq counts bytes cumulatively");
            }
        }
        let (seq, replay) = p.replay();
        assert_eq!(seq, total);
        let mut fresh = Emulator::new(40, 6, 100);
        fresh.advance(&replay);
        let t = fresh.screen_text();
        assert_eq!(t[0], "hello");
        assert_eq!(t[1], "world");
    }

    #[test]
    fn daemon_pane_and_local_emulator_agree_on_vt_sequences() {
        let script = "\\033]5;0;rgb:11/22/33\\007\\033]17;#102030\\007\\033[>1s\\033]22;text\\007\\033]4;2;rgb:01/02/03\\007ab\\r\\nline2\\033[2;3H";
        let (p, rx) = spawn(&["/bin/sh", "-c", &format!("printf '{script}'")]);
        collect_until(&rx, |e| matches!(e, PaneEvent::Exited(_)));
        let (_, replay) = p.replay();
        let mut remote = Emulator::new(40, 6, 100);
        remote.advance(&replay);
        let mut local = Emulator::new(40, 6, 100);
        local.advance(
            b"\x1b]5;0;rgb:11/22/33\x07\x1b]17;#102030\x07\x1b[>1s\x1b]22;text\x07\x1b]4;2;rgb:01/02/03\x07ab\r\nline2\x1b[2;3H",
        );
        assert_eq!(remote.screen_text(), local.screen_text());
        assert_eq!(remote.snapshot().vt, local.snapshot().vt);
        assert_eq!(remote.snapshot().cursor, local.snapshot().cursor);
        assert!(remote.snapshot().vt.shift_capture);
        assert_eq!(remote.snapshot().vt.bold_color, Some([0x11, 0x22, 0x33]));
    }

    #[test]
    fn input_reaches_the_process_and_title_is_reported() {
        let (p, rx) = spawn(&[
            "/bin/sh",
            "-c",
            "printf '\\033]2;ttl\\007'; read x; echo got:$x",
        ]);
        collect_until(&rx, |e| matches!(e, PaneEvent::Title(t) if t == "ttl"));
        p.write(b"abc\n".to_vec());
        collect_until(&rx, |e| {
            matches!(e, PaneEvent::Output { .. }) && p.screen_text().iter().any(|l| l == "got:abc")
        });
        assert_eq!(p.title().as_deref(), Some("ttl"));
    }

    #[test]
    fn processes_start_in_home_by_default() {
        let (p, rx) = spawn(&["/bin/pwd"]);
        collect_until(&rx, |e| matches!(e, PaneEvent::Exited(_)));
        let home = std::env::var("HOME").unwrap();
        let real = std::fs::canonicalize(&home).unwrap();
        // Output can still be in flight when the exit event arrives, so poll the screen.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let out = p.screen_text().join("\n");
            if out.contains(&home) || out.contains(real.to_str().unwrap()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pwd printed {out:?}, HOME is {home}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn resize_changes_the_authoritative_size() {
        let (p, rx) = spawn(&["/bin/sh", "-c", "sleep 5"]);
        p.resize(50, 9);
        assert_eq!(p.size(), (50, 9));
        p.kill();
        collect_until(&rx, |e| matches!(e, PaneEvent::Exited(_)));
    }
}
