//! Daemon behaviour driven through the wire protocol against an in-process server.

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mt_mux_server::daemon;
use mt_mux_server::server::Server;
use mt_proto::mux::{self, Msg, SpawnReq};
use mt_term::Emulator;

struct Env {
    dir: PathBuf,
    server: Server,
    _bound: daemon::Bound,
}

fn start(tag: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("mtmux-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bound = daemon::bind(&dir).unwrap();
    let server = Server::new();
    let (s2, l) = (server.clone(), bound.listener.try_clone().unwrap());
    std::thread::spawn(move || s2.run(l));
    Env {
        dir,
        server,
        _bound: bound,
    }
}

struct Client {
    w: UnixStream,
    r: BufReader<UnixStream>,
}

impl Client {
    fn connect(env: &Env) -> Client {
        let w = UnixStream::connect(mt_mux_server::paths::socket_path(&env.dir)).unwrap();
        w.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut c = Client {
            r: BufReader::new(w.try_clone().unwrap()),
            w,
        };
        c.send(&mux::hello());
        let m = c.recv();
        assert!(mux::check_hello(&m).is_ok(), "{m:?}");
        c
    }
    fn send(&mut self, m: &Msg) {
        mux::write_frame(&mut self.w, m).unwrap();
    }
    fn recv(&mut self) -> Msg {
        mux::read_frame(&mut self.r)
            .unwrap()
            .expect("connection closed")
    }
    /// Reads until `f` returns `Some`, collecting everything seen.
    fn until<T>(&mut self, mut f: impl FnMut(&Msg) -> Option<T>) -> T {
        let end = Instant::now() + Duration::from_secs(15);
        while Instant::now() < end {
            let m = self.recv();
            if let Some(v) = f(&m) {
                return v;
            }
        }
        panic!("timed out");
    }
}

fn spawn_req(session: &str, script: &str) -> SpawnReq {
    SpawnReq {
        req_id: 1,
        session: session.into(),
        cwd: None,
        command: Some(vec!["/bin/sh".into(), "-c".into(), script.into()]),
        env: vec![],
        cols: 40,
        rows: 6,
        scrollback: 100,
    }
}

fn spawn(c: &mut Client, session: &str, script: &str) -> u64 {
    c.send(&Msg::Spawn(spawn_req(session, script)));
    c.until(|m| match m {
        Msg::Spawned { pane, .. } => Some(*pane),
        Msg::Error { message } => panic!("{message}"),
        _ => None,
    })
}

fn screen_after(replay: &[u8]) -> Vec<String> {
    let mut e = Emulator::new(40, 6, 100);
    e.advance(replay);
    e.screen_text()
}

#[test]
fn spawn_streams_output_with_cumulative_seq_and_reports_exit() {
    let env = start("spawn");
    let mut c = Client::connect(&env);
    c.send(&Msg::Attach {
        session: "s".into(),
        panes: vec![],
    });
    c.until(|m| matches!(m, Msg::Attached { .. }).then_some(()));
    let pane = spawn(&mut c, "s", "printf 'hi there'; exit 4");
    let (mut total, mut text) = (0u64, Vec::new());
    let status = c.until(|m| match m {
        Msg::Output {
            pane: p,
            seq,
            bytes,
        } if *p == pane => {
            total += bytes.len() as u64;
            assert_eq!(*seq, total);
            text.extend_from_slice(bytes);
            None
        }
        Msg::PaneExited { status, .. } => Some(*status),
        _ => None,
    });
    assert_eq!(status, Some(4));
    assert!(String::from_utf8_lossy(&text).contains("hi there"));
}

#[test]
fn input_and_title_round_trip() {
    let env = start("input");
    let mut c = Client::connect(&env);
    let pane = spawn(
        &mut c,
        "s",
        "printf '\\033]2;mytitle\\007'; read x; echo got:$x; sleep 5",
    );
    c.until(|m| matches!(m, Msg::TitleChanged { title, .. } if title == "mytitle").then_some(()));
    c.send(&Msg::Input {
        pane,
        bytes: b"zz\n".to_vec(),
    });
    let mut seen = String::new();
    c.until(|m| {
        if let Msg::Output { bytes, .. } = m {
            seen.push_str(&String::from_utf8_lossy(bytes));
        }
        seen.contains("got:zz").then_some(())
    });
    c.send(&Msg::Kill { pane });
}

#[test]
fn reattach_replays_the_screen_and_keeps_the_process_running() {
    let env = start("reattach");
    let mut a = Client::connect(&env);
    let pane = spawn(
        &mut a,
        "prod",
        "printf 'line one\\r\\nline two'; read x; echo done:$x; sleep 30",
    );
    let mut total = 0;
    a.until(|m| {
        if let Msg::Output { bytes, .. } = m {
            total += bytes.len();
        }
        (total >= "line one\r\nline two".len()).then_some(())
    });
    drop(a); // connection lost; the pane must survive
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(env.server.pane_count(), 1);

    let mut b = Client::connect(&env);
    b.send(&Msg::Attach {
        session: "prod".into(),
        panes: vec![],
    });
    let infos = b.until(|m| match m {
        Msg::Attached { panes } => Some(panes.clone()),
        _ => None,
    });
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].id, pane);
    let (seq, bytes) = b.until(|m| match m {
        Msg::Replay { seq, bytes, .. } => Some((*seq, bytes.clone())),
        _ => None,
    });
    assert!(seq > 0);
    let t = screen_after(&bytes);
    assert_eq!((t[0].as_str(), t[1].as_str()), ("line one", "line two"));

    // The same process is still alive and live output continues after the replay.
    b.send(&Msg::Input {
        pane,
        bytes: b"ok\n".to_vec(),
    });
    let mut seen = String::new();
    let last = b.until(|m| {
        if let Msg::Output { seq, bytes, .. } = m {
            seen.push_str(&String::from_utf8_lossy(bytes));
            if seen.contains("done:ok") {
                return Some(*seq);
            }
        }
        None
    });
    assert!(last > seq);

    // Re-attaching with an up-to-date seq skips the replay.
    drop(b);
    std::thread::sleep(Duration::from_millis(100));
    let mut c = Client::connect(&env);
    c.send(&Msg::Attach {
        session: "prod".into(),
        panes: vec![(pane, last)],
    });
    c.send(&Msg::Ping(9));
    let mut replays = 0;
    c.until(|m| {
        if matches!(m, Msg::Replay { .. }) {
            replays += 1;
        }
        matches!(m, Msg::Pong(9)).then_some(())
    });
    assert_eq!(replays, 0, "nothing changed, so no replay is needed");
    c.send(&Msg::KillSession {
        session: "prod".into(),
    });
}

#[test]
fn pane_that_exits_while_detached_is_reported_on_attach_then_forgotten() {
    let env = start("exited");
    let mut a = Client::connect(&env);
    let pane = spawn(&mut a, "s", "sleep 1; printf bye; exit 7");
    drop(a);
    let end = Instant::now() + Duration::from_secs(10);
    // Wait until the daemon has seen the exit (pane stays registered, marked exited).
    std::thread::sleep(Duration::from_millis(1800));
    assert!(Instant::now() < end);
    let mut b = Client::connect(&env);
    b.send(&Msg::Attach {
        session: "s".into(),
        panes: vec![(pane, 0)],
    });
    let infos = b.until(|m| match m {
        Msg::Attached { panes } => Some(panes.clone()),
        _ => None,
    });
    assert_eq!(infos[0].exited.map(|e| e.status), Some(Some(7)));
    let t = b.until(|m| match m {
        Msg::Replay { bytes, .. } => Some(screen_after(bytes)),
        _ => None,
    });
    assert_eq!(t[0], "bye");
    let status = b.until(|m| match m {
        Msg::PaneExited { status, .. } => Some(*status),
        _ => None,
    });
    assert_eq!(status, Some(7));
    b.send(&Msg::List);
    let n = b.until(|m| match m {
        Msg::Listing { panes } => Some(panes.len()),
        _ => None,
    });
    assert_eq!(n, 0);
}

#[test]
fn resize_list_unknown_pane_and_version_mismatch() {
    let env = start("misc");
    let mut c = Client::connect(&env);
    let pane = spawn(&mut c, "s", "sleep 20");
    c.send(&Msg::Resize {
        pane,
        cols: 90,
        rows: 20,
    });
    c.send(&Msg::List);
    let infos = c.until(|m| match m {
        Msg::Listing { panes } => Some(panes.clone()),
        _ => None,
    });
    assert_eq!((infos[0].cols, infos[0].rows), (90, 20));
    c.send(&Msg::Input {
        pane: 999,
        bytes: vec![],
    });
    c.until(|m| matches!(m, Msg::Error { message } if message.contains("999")).then_some(()));
    c.send(&Msg::Kill { pane });

    let mut bad = UnixStream::connect(mt_mux_server::paths::socket_path(&env.dir)).unwrap();
    bad.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    mux::write_frame(
        &mut bad,
        &Msg::Hello {
            proto: 99,
            version: "9".into(),
        },
    )
    .unwrap();
    let mut r = BufReader::new(bad);
    match mux::read_frame(&mut r).unwrap() {
        Some(Msg::Error { message }) => assert!(message.contains("mismatch"), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_second_daemon_in_the_same_dir_is_refused() {
    let env = start("lock");
    assert!(matches!(
        daemon::bind(&env.dir),
        Err(daemon::BindError::AlreadyRunning)
    ));
}
