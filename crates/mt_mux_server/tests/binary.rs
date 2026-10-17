//! The real `microterm-mux` binary: on-demand daemon start, detach, proxy, list, kill.

use std::io::BufReader;
use std::process::{Command, Stdio};

use mt_proto::mux::{self, Msg, SpawnReq};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_microterm-mux"))
}

#[test]
fn proxy_starts_a_detached_daemon_that_outlives_it() {
    let rt = std::env::temp_dir().join(format!("mtmux-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&rt);
    std::fs::create_dir_all(&rt).unwrap();
    let run = |args: &[&str]| {
        bin()
            .args(args)
            .env("XDG_RUNTIME_DIR", &rt)
            .output()
            .unwrap()
    };

    let v = run(&["--version"]);
    assert!(String::from_utf8_lossy(&v.stdout).contains(mt_mux_server::VERSION));

    // First proxy: no daemon yet, so it is started and detached.
    let mut p = bin()
        .arg("proxy")
        .env("XDG_RUNTIME_DIR", &rt)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (mut w, mut r) = (
        p.stdin.take().unwrap(),
        BufReader::new(p.stdout.take().unwrap()),
    );
    mux::write_frame(&mut w, &mux::hello()).unwrap();
    assert!(mux::check_hello(&mux::read_frame(&mut r).unwrap().unwrap()).is_ok());
    mux::write_frame(
        &mut w,
        &Msg::Spawn(SpawnReq {
            req_id: 1,
            session: "t".into(),
            cwd: None,
            command: Some(vec!["/bin/sh".into(), "-c".into(), "sleep 60".into()]),
            env: vec![],
            cols: 80,
            rows: 24,
            scrollback: 10,
        }),
    )
    .unwrap();
    let pane = loop {
        if let Some(Msg::Spawned { pane, .. }) = mux::read_frame(&mut r).unwrap() {
            break pane;
        }
    };
    drop(w);
    let _ = p.wait(); // the proxy ends with its stdin; the daemon must not

    let out = String::from_utf8(run(&["list"]).stdout).unwrap();
    assert!(out.contains("running"), "{out}");
    assert!(
        out.lines().any(|l| l.starts_with(&pane.to_string())),
        "{out}"
    );

    assert!(run(&["kill", &pane.to_string()]).status.success());
    std::thread::sleep(std::time::Duration::from_millis(500));
    let out = String::from_utf8(run(&["list"]).stdout).unwrap();
    assert!(!out.contains("running"), "{out}");

    // Clean up the daemon.
    if let Ok(pid) = std::fs::read_to_string(rt.join("microterm-mux/lock")) {
        let _ = Command::new("kill").arg(pid.trim()).status();
    }
    let _ = std::fs::remove_dir_all(&rt);
}
