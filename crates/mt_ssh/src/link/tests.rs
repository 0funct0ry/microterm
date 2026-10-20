//! The link against an in-process `microterm-mux` daemon: reconnect with replay, backoff
//! countdown, fatal failures and shutdown, with no SSH involved.

use std::os::unix::net::UnixStream as StdStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use mt_mux_server::{daemon, paths, server::Server};

use super::*;

struct Env {
    dir: PathBuf,
    _bound: daemon::Bound,
}

fn start_daemon(tag: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("mtlink-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bound = daemon::bind(&dir).unwrap();
    let (server, l) = (Server::new(), bound.listener.try_clone().unwrap());
    std::thread::spawn(move || server.run(l));
    Env { dir, _bound: bound }
}

/// Dials the daemon's socket; the test can sever live connections and block new ones.
struct TestDialer {
    socket: PathBuf,
    up: AtomicBool,
    live: Mutex<Vec<StdStream>>,
    dials: AtomicU32,
    fail: Mutex<Option<DialFailure>>,
}

impl TestDialer {
    fn new(env: &Env) -> Arc<Self> {
        Arc::new(TestDialer {
            socket: paths::socket_path(&env.dir),
            up: AtomicBool::new(true),
            live: Mutex::new(vec![]),
            dials: AtomicU32::new(0),
            fail: Mutex::new(None),
        })
    }
    fn sever(&self) {
        self.up.store(false, Ordering::SeqCst);
        for s in self.live.lock().unwrap().drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
    fn restore(&self) {
        self.up.store(true, Ordering::SeqCst);
    }
}

impl Dialer for TestDialer {
    fn dial(&self) -> BoxFuture<Result<Dialed, DialFailure>> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let result = if let Some(f) = self.fail.lock().unwrap().clone() {
            Err(f)
        } else if !self.up.load(Ordering::SeqCst) {
            Err(DialFailure::Transient("network down".into()))
        } else {
            StdStream::connect(&self.socket)
                .map_err(|e| DialFailure::Transient(e.to_string()))
                .and_then(|s| {
                    self.live.lock().unwrap().push(s.try_clone().unwrap());
                    s.set_nonblocking(true).unwrap();
                    let tokio_stream = tokio::net::UnixStream::from_std(s)
                        .map_err(|e| DialFailure::Transient(e.to_string()))?;
                    Ok(Dialed {
                        stream: Box::new(tokio_stream),
                        keepalive: Duration::from_secs(10),
                        keep: Box::new(()),
                        notice: None,
                    })
                })
        };
        Box::pin(async move { result })
    }
}

async fn next(rx: &async_channel::Receiver<LinkEvent>) -> LinkEvent {
    tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("timed out waiting for a link event")
        .unwrap()
}

/// Collects events until `f` matches one.
async fn until<T>(
    rx: &async_channel::Receiver<LinkEvent>,
    mut f: impl FnMut(&LinkEvent) -> Option<T>,
) -> T {
    loop {
        let e = next(rx).await;
        if let Some(v) = f(&e) {
            return v;
        }
    }
}

fn spawn_cmd(pane: u64, script: &str) -> Command {
    Command::Spawn {
        pane,
        cwd: None,
        command: Some(vec!["/bin/sh".into(), "-c".into(), script.into()]),
        env: vec![],
        cols: 40,
        rows: 6,
    }
}

fn screen(bytes: &[u8]) -> Vec<String> {
    let mut e = mt_term::Emulator::new(40, 6, 100);
    e.advance(bytes);
    e.screen_text()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_connection_reconnects_and_the_same_process_and_screen_survive() {
    let env = start_daemon("reconnect");
    let dialer = TestDialer::new(&env);
    let (h, rx) = start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "prod".into(),
            scrollback: 100,
        },
        dialer.clone(),
    );
    h.send(spawn_cmd(1, "printf 'top-like screen\\r\\nline 2'; sleep 1; printf '\\r\\nlate line'; read x; echo \"; got:$x\"; sleep 60"));

    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    let mut seen = Vec::new();
    until(&rx, |e| {
        if let LinkEvent::Bytes {
            bytes,
            replay: false,
            ..
        } = e
        {
            seen.extend_from_slice(bytes);
        }
        String::from_utf8_lossy(&seen)
            .contains("line 2")
            .then_some(())
    })
    .await;

    // The network drops.
    dialer.sever();
    let attempt = until(&rx, |e| match e {
        LinkEvent::State(LinkState::Reconnecting { attempt, next_in })
            if *attempt >= 1 && !next_in.is_zero() =>
        {
            Some(*attempt)
        }
        _ => None,
    })
    .await;
    assert_eq!(attempt, 1);
    // A second failed attempt counts up.
    let attempt = until(&rx, |e| match e {
        LinkEvent::State(LinkState::Reconnecting { attempt, .. }) if *attempt >= 2 => {
            Some(*attempt)
        }
        _ => None,
    })
    .await;
    assert_eq!(attempt, 2);

    // The process prints more while we are away, then the network comes back.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    dialer.restore();
    h.send(Command::ReconnectNow);
    let replay = until(&rx, |e| match e {
        LinkEvent::Bytes {
            bytes,
            replay: true,
            ..
        } => Some(bytes.clone()),
        _ => None,
    })
    .await;
    let t = screen(&replay);
    // Output produced while we were away is part of the replayed screen.
    assert_eq!(
        (t[0].as_str(), t[1].as_str(), t[2].as_str()),
        ("top-like screen", "line 2", "late line")
    );

    // Same process: it is still waiting in `read` and answers.
    h.send(Command::Input {
        pane: 1,
        bytes: b"zz\n".to_vec(),
    });
    let mut seen = String::new();
    until(&rx, |e| {
        if let LinkEvent::Bytes { bytes, .. } = e {
            seen.push_str(&String::from_utf8_lossy(bytes));
        }
        seen.contains("got:zz").then_some(())
    })
    .await;
    h.send(Command::Close {
        pane: 1,
        kill: true,
    });
    h.send(Command::Shutdown);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pane_that_exits_while_disconnected_is_reported_after_reconnect() {
    let env = start_daemon("exitoff");
    let dialer = TestDialer::new(&env);
    let (h, rx) = start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "s".into(),
            scrollback: 100,
        },
        dialer.clone(),
    );
    h.send(spawn_cmd(7, "sleep 1; printf bye; exit 5"));
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    // Wait for the spawn to be acknowledged by the first output-free round trip.
    tokio::time::sleep(Duration::from_millis(300)).await;
    dialer.sever();
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Reconnecting { .. })).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await; // the process exits meanwhile
    dialer.restore();
    h.send(Command::ReconnectNow);
    let (pane, status) = until(&rx, |e| match e {
        LinkEvent::Exited { pane, status } => Some((*pane, *status)),
        _ => None,
    })
    .await;
    assert_eq!((pane, status), (7, Some(5)));
    h.send(Command::Shutdown);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_failures_do_not_retry_until_asked() {
    let env = start_daemon("fatal");
    let dialer = TestDialer::new(&env);
    *dialer.fail.lock().unwrap() = Some(DialFailure::Fatal("host key changed".into()));
    let (h, rx) = start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "s".into(),
            scrollback: 10,
        },
        dialer.clone(),
    );
    let reason = until(&rx, |e| match e {
        LinkEvent::State(LinkState::Disconnected { reason }) => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert_eq!(reason, "host key changed");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 1, "no automatic retry");
    *dialer.fail.lock().unwrap() = None;
    h.send(Command::ReconnectNow);
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    h.send(Command::Shutdown);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_queued_before_the_first_connect_are_applied_and_titles_flow() {
    let env = start_daemon("queued");
    let dialer = TestDialer::new(&env);
    let (h, rx) = start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "s".into(),
            scrollback: 10,
        },
        dialer,
    );
    h.send(spawn_cmd(3, "printf '\\033]2;remote-title\\007'; sleep 5"));
    h.send(Command::Resize {
        pane: 3,
        cols: 100,
        rows: 30,
    });
    let title = until(&rx, |e| match e {
        LinkEvent::Title { pane: 3, title } => Some(title.clone()),
        _ => None,
    })
    .await;
    assert_eq!(title, "remote-title");
    h.send(Command::Close {
        pane: 3,
        kill: true,
    });
    h.send(Command::Shutdown);
}

/// Collects the pane's output until `needle` shows up.
async fn output_contains(
    rx: &async_channel::Receiver<LinkEvent>,
    pane: u64,
    needle: &str,
) -> String {
    let mut seen = String::new();
    let r = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let LinkEvent::Bytes { pane: p, bytes, .. } = rx.recv().await.unwrap() {
                if p == pane {
                    seen.push_str(&String::from_utf8_lossy(&bytes));
                    if seen.contains(needle) {
                        return;
                    }
                }
            }
        }
    })
    .await;
    assert!(
        r.is_ok(),
        "pane {pane} never printed {needle:?}; got {seen:?}"
    );
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resize_sent_before_the_daemon_confirms_the_pane_reaches_it() {
    // The GUI creates a remote terminal at a default size and resizes it to the window on the
    // first paint, typically before `Spawned` comes back.
    let env = start_daemon("early-resize");
    let dialer = TestDialer::new(&env);
    let (h, rx) = start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "s".into(),
            scrollback: 10,
        },
        dialer,
    );
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    h.send(spawn_cmd(1, "sleep 1; echo SIZE=$(stty size); sleep 5"));
    h.send(Command::Resize {
        pane: 1,
        cols: 100,
        rows: 30,
    });
    output_contains(&rx, 1, "SIZE=30 100").await;
    h.send(Command::Close {
        pane: 1,
        kill: true,
    });
    h.send(Command::Shutdown);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pane_queued_before_connecting_starts_at_its_latest_size() {
    let env = start_daemon("queued-size");
    let dialer = TestDialer::new(&env);
    let (h, rx) = start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "s".into(),
            scrollback: 10,
        },
        dialer,
    );
    // Both arrive while the first connection is still being dialled.
    h.send(spawn_cmd(2, "echo SIZE=$(stty size); sleep 5"));
    h.send(Command::Resize {
        pane: 2,
        cols: 132,
        rows: 43,
    });
    output_contains(&rx, 2, "SIZE=43 132").await;
    h.send(Command::Close {
        pane: 2,
        kill: true,
    });
    h.send(Command::Shutdown);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_hints_reach_the_daemon_and_all_three_panes_reattach_after_a_reconnect() {
    use mt_proto::mux::{encode, hello, read_frame};
    use std::io::Write as _;

    let env = start_daemon("splits");
    let dialer = TestDialer::new(&env);
    let (h, rx) = start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "dev".into(),
            scrollback: 100,
        },
        dialer.clone(),
    );
    for (pane, word) in [(1, "alpha"), (2, "bravo"), (3, "charlie")] {
        h.send(spawn_cmd(pane, &format!("printf '{word}'; sleep 60")));
    }
    // 2 split off 1 (side by side, 40%), 3 off 2 (stacked). Sent before the panes exist: the
    // link holds them back until the daemon has confirmed both ends.
    h.send(Command::Split {
        pane: 2,
        parent: 1,
        axis: 0,
        permille: 400,
    });
    h.send(Command::Split {
        pane: 3,
        parent: 2,
        axis: 1,
        permille: 500,
    });

    let mut seen: std::collections::BTreeMap<u64, String> = Default::default();
    until(&rx, |e| {
        if let LinkEvent::Bytes {
            pane,
            bytes,
            replay: false,
        } = e
        {
            seen.entry(*pane)
                .or_default()
                .push_str(&String::from_utf8_lossy(bytes));
        }
        (seen.get(&1).is_some_and(|s| s.contains("alpha"))
            && seen.get(&2).is_some_and(|s| s.contains("bravo"))
            && seen.get(&3).is_some_and(|s| s.contains("charlie")))
        .then_some(())
    })
    .await;

    dialer.sever();
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Reconnecting { .. })).then_some(())
    })
    .await;
    dialer.restore();
    h.send(Command::ReconnectNow);
    // Nothing was printed while we were away, so there is nothing to replay: the link simply
    // re-attaches to all three panes.
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    // A brand-new client sees how the panes were split.
    let socket = paths::socket_path(&env.dir);
    let infos = tokio::task::spawn_blocking(move || {
        let mut s = StdStream::connect(socket).unwrap();
        s.write_all(&encode(&hello()).unwrap()).unwrap();
        s.write_all(
            &encode(&Msg::Attach {
                session: "dev".into(),
                panes: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        loop {
            match read_frame(&mut s).unwrap().unwrap() {
                Msg::Attached { panes } => return panes,
                _ => continue,
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(infos.len(), 3);
    let by_split: Vec<_> = infos
        .iter()
        .map(|i| i.split.map(|s| (s.axis, s.permille)))
        .collect();
    assert_eq!(by_split, [None, Some((0, 400)), Some((1, 500))]);

    for pane in 1..=3 {
        h.send(Command::Close { pane, kill: true });
    }
    h.send(Command::Shutdown);
}
