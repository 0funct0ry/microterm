//! Real SSH against an openssh server (SPEC §14). Skipped unless `MT_SSH_TEST_HOST` is set;
//! `scripts/test-ssh.sh` (`make test-ssh`) starts a server (Docker, or a throw-away local
//! sshd with `--local`) and sets the environment:
//!
//! - `MT_SSH_TEST_HOST`, `MT_SSH_TEST_PORT`, `MT_SSH_TEST_USER`
//! - `MT_SSH_TEST_KEY`: private key the server accepts
//! - `MT_SSH_TEST_PASSWORD`: optional; enables the password test
//! - `MICROTERM_MUX_BINARY`: a `microterm-mux` build for the *server's* platform

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mt_ssh::bootstrap;
use mt_ssh::config::{Proxy, Target};
use mt_ssh::dialer::SshDialer;
use mt_ssh::hostkeys;
use mt_ssh::link::{self, Command, LinkConfig, LinkEvent, LinkState};
use mt_ssh::prompt::{BoxFuture, HostKeyPrompt, Prompter, Secret, SecretPrompt};
use mt_ssh::session::{self, ConnectOptions, SshError};

struct Cfg {
    host: String,
    port: u16,
    user: String,
    key: PathBuf,
    password: Option<String>,
}

fn cfg() -> Option<Cfg> {
    Some(Cfg {
        host: std::env::var("MT_SSH_TEST_HOST").ok()?,
        port: std::env::var("MT_SSH_TEST_PORT").ok()?.parse().ok()?,
        user: std::env::var("MT_SSH_TEST_USER").ok()?,
        key: std::env::var("MT_SSH_TEST_KEY").ok()?.into(),
        password: std::env::var("MT_SSH_TEST_PASSWORD").ok(),
    })
}

macro_rules! require {
    () => {
        match cfg() {
            Some(c) => c,
            None => {
                eprintln!("skipped: MT_SSH_TEST_HOST is not set");
                return;
            }
        }
    };
}

/// Answers prompts from a script and counts them.
struct Script {
    trust: bool,
    password: Option<String>,
    host_prompts: AtomicUsize,
}

impl Script {
    fn new(trust: bool, password: Option<String>) -> Arc<Self> {
        Arc::new(Script {
            trust,
            password,
            host_prompts: AtomicUsize::new(0),
        })
    }
}

impl Prompter for Script {
    fn confirm_host_key(&self, p: HostKeyPrompt) -> BoxFuture<bool> {
        assert!(p.fingerprint.starts_with("SHA256:"));
        self.host_prompts.fetch_add(1, Ordering::SeqCst);
        let t = self.trust;
        Box::pin(async move { t })
    }
    fn secret(&self, p: SecretPrompt) -> BoxFuture<Option<Secret>> {
        let pw = match p {
            SecretPrompt::Password { .. } | SecretPrompt::Interactive { .. } => {
                self.password.clone()
            }
            SecretPrompt::Passphrase { .. } => None,
        };
        Box::pin(async move { pw.map(Secret::new) })
    }
}

fn tmp_known_hosts(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mtssh-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("known_hosts")
}

fn target(c: &Cfg, port: u16, key: bool) -> Target {
    Target {
        host: c.host.clone(),
        port,
        user: c.user.clone(),
        identity_files: if key { vec![c.key.clone()] } else { vec![] },
        identities_only: true,
        proxy: Proxy::None,
        keepalive_secs: 2,
    }
}

fn opts(kh: PathBuf, p: Arc<Script>) -> ConnectOptions {
    ConnectOptions {
        known_hosts: kh,
        timeout: Duration::from_secs(10),
        prompter: p,
    }
}

#[tokio::test]
async fn key_auth_with_unknown_then_known_host_key() {
    let c = require!();
    let kh = tmp_known_hosts("key");
    let p = Script::new(true, None);
    let conn = session::connect(&target(&c, c.port, true), &opts(kh.clone(), p.clone()))
        .await
        .expect("connect with key");
    let (out, st) = conn.exec_capture("echo hello").await.unwrap();
    assert_eq!(
        (String::from_utf8_lossy(&out).trim(), st),
        ("hello", Some(0))
    );
    assert_eq!(
        p.host_prompts.load(Ordering::SeqCst),
        1,
        "unknown key asked once"
    );
    drop(conn);
    let p2 = Script::new(false, None);
    session::connect(&target(&c, c.port, true), &opts(kh, p2.clone()))
        .await
        .expect("second connect");
    assert_eq!(p2.host_prompts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn declined_host_key_fails_and_is_not_recorded() {
    let c = require!();
    let kh = tmp_known_hosts("decline");
    let r = session::connect(
        &target(&c, c.port, true),
        &opts(kh.clone(), Script::new(false, None)),
    )
    .await;
    assert!(
        matches!(r, Err(SshError::HostKeyRejected(_))),
        "{:?}",
        r.err()
    );
    assert!(!kh.exists() || std::fs::read_to_string(&kh).unwrap().trim().is_empty());
}

#[tokio::test]
async fn a_changed_host_key_is_a_hard_failure_without_prompting() {
    let c = require!();
    let kh = tmp_known_hosts("changed");
    let other = {
        use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
        russh::keys::ssh_key::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[9; 32])),
            "",
        )
        .unwrap()
        .public_key()
        .clone()
    };
    hostkeys::trust(&kh, &c.host, c.port, &other).unwrap();
    let p = Script::new(true, None);
    let r = session::connect(&target(&c, c.port, true), &opts(kh, p.clone())).await;
    match r {
        Err(SshError::HostKeyChanged { fingerprint, .. }) => {
            assert!(fingerprint.starts_with("SHA256:"))
        }
        other => panic!("expected HostKeyChanged, got {:?}", other.err()),
    }
    assert_eq!(
        p.host_prompts.load(Ordering::SeqCst),
        0,
        "no override is offered"
    );
}

#[tokio::test]
async fn password_auth() {
    let c = require!();
    let Some(pw) = c.password.clone() else {
        eprintln!("skipped: MT_SSH_TEST_PASSWORD is not set");
        return;
    };
    let kh = tmp_known_hosts("pw");
    let conn = session::connect(
        &target(&c, c.port, false),
        &opts(kh.clone(), Script::new(true, Some(pw))),
    )
    .await
    .expect("password auth");
    assert_eq!(conn.exec_capture("echo ok").await.unwrap().1, Some(0));
    let r = session::connect(
        &target(&c, c.port, false),
        &opts(kh, Script::new(true, Some("wrong".into()))),
    )
    .await;
    assert!(
        matches!(r, Err(SshError::AuthFailed { .. })),
        "{:?}",
        r.err()
    );
}

fn remote_dir(tag: &str) -> String {
    format!("~/.cache/mtssh-it-{tag}-{}", std::process::id())
}

async fn stop_remote_daemon(conn: &session::Connection) {
    let _ = conn
        .exec_capture(
            "d=${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/microterm-mux}; d=${d:-/tmp/microterm-mux-$(id -u)}; kill $(cat $d/lock) 2>/dev/null; true",
        )
        .await;
}

#[tokio::test]
async fn bootstrap_installs_into_an_empty_home_and_is_idempotent() {
    let c = require!();
    let conn = session::connect(
        &target(&c, c.port, true),
        &opts(tmp_known_hosts("boot"), Script::new(true, None)),
    )
    .await
    .unwrap();
    let dir = remote_dir("boot");
    let first = bootstrap::ensure_remote(&conn, &dir, bootstrap::VERSION)
        .await
        .expect("bootstrap");
    assert!(first.uploaded);
    let second = bootstrap::ensure_remote(&conn, &dir, bootstrap::VERSION)
        .await
        .unwrap();
    assert!(!second.uploaded, "present and runnable: nothing to upload");
    let (out, st) = conn
        .exec_capture(&format!("{} --version", bootstrap::sh_path(&first.binary)))
        .await
        .unwrap();
    assert_eq!(st, Some(0));
    assert!(String::from_utf8_lossy(&out).contains(bootstrap::VERSION));
    let _ = conn
        .exec_capture(&format!("rm -rf {}", bootstrap::sh_path(&dir)))
        .await;
}

/// A TCP forwarder the test can cut, standing in for "the network drops".
struct Cutter {
    port: u16,
    tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    open: Arc<std::sync::atomic::AtomicBool>,
}

impl Cutter {
    async fn start(to: (String, u16)) -> Cutter {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> = Default::default();
        let open = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (t2, o2) = (tasks.clone(), open.clone());
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = l.accept().await {
                if !o2.load(Ordering::SeqCst) {
                    continue;
                }
                let to = to.clone();
                let h = tokio::spawn(async move {
                    if let Ok(mut out) = tokio::net::TcpStream::connect(to).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut out).await;
                    }
                });
                t2.lock().unwrap().push(h);
            }
        });
        Cutter { port, tasks, open }
    }
    fn cut(&self) {
        self.open.store(false, Ordering::SeqCst);
        for h in self.tasks.lock().unwrap().drain(..) {
            h.abort();
        }
    }
    fn restore(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

async fn until<T>(
    rx: &async_channel::Receiver<LinkEvent>,
    mut f: impl FnMut(&LinkEvent) -> Option<T>,
) -> T {
    loop {
        let e = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("timed out waiting for a link event")
            .unwrap();
        if let Some(v) = f(&e) {
            return v;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killing_the_connection_then_reconnecting_keeps_the_screen_and_the_process() {
    let c = require!();
    let cutter = Cutter::start((c.host.clone(), c.port)).await;
    let dialer = Arc::new(SshDialer {
        target: target(&c, cutter.port, true),
        remote_dir: remote_dir("e2e"),
        multiplexing: true,
        opts: opts(tmp_known_hosts("e2e"), Script::new(true, None)),
    });
    let (h, rx) = link::start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "it".into(),
            scrollback: 1000,
        },
        dialer,
    );
    h.send(Command::Spawn {
        pane: 1,
        cwd: None,
        command: Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'screen one\\r\\nscreen two'; read x; echo \" got:$x\"; sleep 120".into(),
        ]),
        env: vec![],
        cols: 60,
        rows: 8,
    });
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    let mut seen = Vec::new();
    until(&rx, |e| {
        if let LinkEvent::Bytes { bytes, .. } = e {
            seen.extend_from_slice(bytes);
        }
        String::from_utf8_lossy(&seen)
            .contains("screen two")
            .then_some(())
    })
    .await;

    cutter.cut();
    let attempt = until(&rx, |e| match e {
        LinkEvent::State(LinkState::Reconnecting { attempt, .. }) if *attempt >= 2 => {
            Some(*attempt)
        }
        _ => None,
    })
    .await;
    assert!(attempt >= 2, "the overlay counts attempts");

    cutter.restore();
    h.send(Command::ReconnectNow);
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;

    h.send(Command::Input {
        pane: 1,
        bytes: b"alive\n".to_vec(),
    });
    let mut tail = String::new();
    until(&rx, |e| {
        if let LinkEvent::Bytes { bytes, .. } = e {
            tail.push_str(&String::from_utf8_lossy(bytes));
        }
        tail.contains("got:alive").then_some(())
    })
    .await;

    h.send(Command::Close {
        pane: 1,
        kill: true,
    });
    h.send(Command::Shutdown);
    let conn = session::connect(
        &target(&c, c.port, true),
        &opts(tmp_known_hosts("e2e-clean"), Script::new(true, None)),
    )
    .await
    .unwrap();
    stop_remote_daemon(&conn).await;
    let _ = conn
        .exec_capture(&format!(
            "rm -rf {}",
            bootstrap::sh_path(&remote_dir("e2e"))
        ))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killing_the_remote_daemon_ends_its_panes_and_new_panes_still_work() {
    let c = require!();
    let dialer = Arc::new(SshDialer {
        target: target(&c, c.port, true),
        remote_dir: remote_dir("dk"),
        multiplexing: true,
        opts: opts(tmp_known_hosts("dk"), Script::new(true, None)),
    });
    let (h, rx) = link::start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "dk".into(),
            scrollback: 100,
        },
        dialer,
    );
    let sleeper = |pane| Command::Spawn {
        pane,
        cwd: None,
        command: Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'up'; sleep 120".into(),
        ]),
        env: vec![],
        cols: 40,
        rows: 6,
    };
    h.send(sleeper(1));
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    let mut seen = String::new();
    until(&rx, |e| {
        if let LinkEvent::Bytes { bytes, .. } = e {
            seen.push_str(&String::from_utf8_lossy(bytes));
        }
        seen.contains("up").then_some(())
    })
    .await;

    // Kill the daemon on the host (what `pkill microterm-mux` does).
    let conn = session::connect(
        &target(&c, c.port, true),
        &opts(tmp_known_hosts("dk2"), Script::new(true, None)),
    )
    .await
    .unwrap();
    stop_remote_daemon(&conn).await;

    let mut log = Vec::new();
    let exited = tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let e = rx.recv().await.unwrap();
            log.push(format!("{e:?}").chars().take(90).collect::<String>());
            if matches!(e, LinkEvent::Exited { pane: 1, .. }) {
                return;
            }
        }
    })
    .await;
    assert!(
        exited.is_ok(),
        "pane 1 should be reported gone; events: {log:#?}"
    );

    // A new pane works against the restarted daemon.
    h.send(sleeper(2));
    let mut seen = String::new();
    let got = tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            if let LinkEvent::Bytes { pane: 2, bytes, .. } = rx.recv().await.unwrap() {
                seen.push_str(&String::from_utf8_lossy(&bytes));
                if seen.contains("up") {
                    return;
                }
            }
        }
    })
    .await;
    assert!(got.is_ok(), "new pane produced no output");
    h.send(Command::Close {
        pane: 2,
        kill: true,
    });
    h.send(Command::Shutdown);
    stop_remote_daemon(&conn).await;
    let _ = conn
        .exec_capture(&format!("rm -rf {}", bootstrap::sh_path(&remote_dir("dk"))))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_sessions_show_output_in_every_pane_and_report_loss() {
    let c = require!();
    let cutter = Cutter::start((c.host.clone(), c.port)).await;
    let dialer = Arc::new(SshDialer {
        target: target(&c, cutter.port, true),
        remote_dir: remote_dir("plain"),
        multiplexing: false,
        opts: opts(tmp_known_hosts("plain"), Script::new(true, None)),
    });
    let (h, rx) = link::start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "plain".into(),
            scrollback: 100,
        },
        dialer,
    );
    let shell = |pane| Command::Spawn {
        pane,
        cwd: None,
        command: None,
        env: vec![],
        cols: 40,
        rows: 6,
    };
    for p in 1..=3 {
        h.send(shell(p));
    }
    let mut got = std::collections::HashSet::new();
    let mut log = Vec::new();
    let r = tokio::time::timeout(Duration::from_secs(30), async {
        while got.len() < 3 {
            let e = rx.recv().await.unwrap();
            log.push(format!("{e:?}").chars().take(80).collect::<String>());
            if let LinkEvent::Bytes { pane, .. } = e {
                got.insert(pane);
            }
        }
    })
    .await;
    assert!(r.is_ok(), "panes with output: {got:?}; events: {log:#?}");

    cutter.cut();
    let mut log = Vec::new();
    let mut exited = std::collections::HashSet::new();
    cutter.restore();
    h.send(Command::ReconnectNow);
    let r = tokio::time::timeout(Duration::from_secs(40), async {
        while exited.len() < 3 {
            let e = rx.recv().await.unwrap();
            log.push(format!("{e:?}").chars().take(80).collect::<String>());
            if let LinkEvent::Exited { pane, .. } = e {
                exited.insert(pane);
            }
        }
    })
    .await;
    assert!(
        r.is_ok(),
        "after loss, exited: {exited:?}; events: {log:#?}"
    );

    // A new tab after the reconnect gets a shell.
    h.send(shell(4));
    let mut log = Vec::new();
    let r = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let e = rx.recv().await.unwrap();
            log.push(format!("{e:?}").chars().take(80).collect::<String>());
            if let LinkEvent::Bytes { pane: 4, .. } = e {
                return;
            }
        }
    })
    .await;
    assert!(
        r.is_ok(),
        "pane 4 produced no output after reconnect; events: {log:#?}"
    );
    h.send(Command::Shutdown);
}

/// Every process below `pid` (children first is not needed: SIGKILL is immediate).
fn descendants(pid: u32) -> Vec<u32> {
    let out = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .unwrap();
    let mut all = Vec::new();
    for child in String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse::<u32>().ok())
    {
        all.push(child);
        all.extend(descendants(child));
    }
    all
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killing_the_servers_session_processes_reconnects_to_the_same_process() {
    let c = require!();
    let Some(sshd) = std::env::var("MT_SSH_TEST_SSHD_PID")
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
    else {
        eprintln!("skipped: MT_SSH_TEST_SSHD_PID is not set (run scripts/test-ssh.sh --local)");
        return;
    };
    let dialer = Arc::new(SshDialer {
        target: target(&c, c.port, true),
        remote_dir: remote_dir("srvkill"),
        multiplexing: true,
        opts: opts(tmp_known_hosts("srvkill"), Script::new(true, None)),
    });
    let (h, rx) = link::start(
        &tokio::runtime::Handle::current(),
        LinkConfig {
            session: "srvkill".into(),
            scrollback: 100,
        },
        dialer,
    );
    h.send(Command::Spawn {
        pane: 1,
        cwd: None,
        command: Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo pid=$$; read x; echo \" got:$x\"; sleep 120".into(),
        ]),
        env: vec![],
        cols: 60,
        rows: 8,
    });
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;
    let mut seen = String::new();
    until(&rx, |e| {
        if let LinkEvent::Bytes { bytes, .. } = e {
            seen.push_str(&String::from_utf8_lossy(bytes));
        }
        seen.contains("pid=").then_some(())
    })
    .await;

    // What `kill -9` of the sshd-session processes does: the server end of the connection dies
    // while the listening sshd keeps running.
    // Kill only the unprivileged `…@notty` process, as `pgrep -fl sshd` + `kill -9` would; its
    // privileged monitor stays alive and may keep the socket open.
    let all = descendants(sshd);
    let notty: Vec<u32> = all
        .iter()
        .copied()
        .filter(|p| {
            let out = std::process::Command::new("ps")
                .args(["-o", "command=", "-p", &p.to_string()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).contains("@notty")
        })
        .collect();
    eprintln!("session processes {all:?}, killing {notty:?}");
    let victims = if notty.is_empty() { all } else { notty };
    assert!(
        !victims.is_empty(),
        "no session processes under sshd {sshd}"
    );
    for v in &victims {
        let _ = std::process::Command::new("kill")
            .args(["-9", &v.to_string()])
            .stderr(std::process::Stdio::null())
            .status();
    }
    let killed_at = std::time::Instant::now();

    let lost = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let LinkEvent::State(LinkState::Reconnecting { .. }) = rx.recv().await.unwrap() {
                return;
            }
        }
    })
    .await;
    assert!(lost.is_ok(), "the link never noticed the server side dying");
    eprintln!("loss noticed after {:?}", killed_at.elapsed());
    until(&rx, |e| {
        matches!(e, LinkEvent::State(LinkState::Connected)).then_some(())
    })
    .await;

    h.send(Command::Input {
        pane: 1,
        bytes: b"still-here\n".to_vec(),
    });
    let mut tail = String::new();
    until(&rx, |e| {
        if let LinkEvent::Bytes { bytes, .. } = e {
            tail.push_str(&String::from_utf8_lossy(bytes));
        }
        tail.contains("got:still-here").then_some(())
    })
    .await;

    h.send(Command::Close {
        pane: 1,
        kill: true,
    });
    h.send(Command::Shutdown);
    let conn = session::connect(
        &target(&c, c.port, true),
        &opts(tmp_known_hosts("srvkill-clean"), Script::new(true, None)),
    )
    .await
    .unwrap();
    stop_remote_daemon(&conn).await;
    let _ = conn
        .exec_capture(&format!(
            "rm -rf {}",
            bootstrap::sh_path(&remote_dir("srvkill"))
        ))
        .await;
}
