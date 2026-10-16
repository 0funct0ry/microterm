//! End-to-end tests of `microterm cli …` against a hidden `--headless` instance (SPEC §14):
//! real mux, real PTYs, no windows.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A headless instance with its own `$HOME` / `$XDG_RUNTIME_DIR` (kept short: unix socket paths
/// are limited to ~104 bytes).
struct Instance {
    child: Child,
    home: String,
    socket: String,
}

impl Instance {
    fn start() -> Instance {
        let home = format!(
            "/tmp/mte-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        std::fs::create_dir_all(format!("{home}/run")).unwrap();
        let mut child = Process::new(assert_cmd::cargo::cargo_bin("microterm"))
            .arg("--headless")
            .envs(Self::env(&home))
            .env_remove("MICROTERM_UNIX_SOCKET")
            .env_remove("MICROTERM_PANE")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        Instance {
            child,
            home,
            socket: line.trim().to_string(),
        }
    }

    fn env(home: &str) -> Vec<(String, String)> {
        vec![
            ("HOME".into(), home.into()),
            ("XDG_RUNTIME_DIR".into(), format!("{home}/run")),
            ("XDG_CONFIG_HOME".into(), format!("{home}/cfg")),
            ("SHELL".into(), "/bin/sh".into()),
        ]
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::cargo_bin("microterm").unwrap();
        c.args(args)
            .envs(Self::env(&self.home))
            .env_remove("MICROTERM_UNIX_SOCKET")
            .env_remove("MICROTERM_PANE")
            .timeout(Duration::from_secs(30));
        c
    }

    fn cli(&self, args: &[&str]) -> String {
        let out = self.cmd(&[&["cli"][..], args].concat()).output().unwrap();
        assert!(
            out.status.success(),
            "cli {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn cli_code(&self, args: &[&str]) -> i32 {
        self.cmd(&[&["cli"][..], args].concat())
            .output()
            .unwrap()
            .status
            .code()
            .unwrap()
    }

    /// Spawns `sh -c SCRIPT` (kept alive by the script) and returns its pane id.
    fn spawn_sh(&self, script: &str, extra: &[&str]) -> String {
        let mut args = vec!["spawn"];
        args.extend_from_slice(extra);
        args.extend(["--", "sh", "-c", script]);
        self.cli(&args).trim().to_string()
    }

    fn text(&self, pane: &str, extra: &[&str]) -> String {
        let mut args = vec!["get-text", "--pane", pane];
        args.extend_from_slice(extra);
        self.cli(&args)
    }

    fn wait_text(&self, pane: &str, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let t = self.text(pane, &[]);
            if t.contains(needle) || Instant::now() > deadline {
                assert!(t.contains(needle), "{needle:?} not in {t:?}");
                return t;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn list(&self) -> Value {
        serde_json::from_str(&self.cli(&["list", "--json"])).unwrap()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn tab_count(list: &Value, workspace: &str) -> usize {
    list["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["name"] == workspace)
        .flat_map(|w| w["windows"].as_array().unwrap())
        .map(|w| w["tabs"].as_array().unwrap().len())
        .sum()
}

#[test]
fn no_running_instance_exits_2() {
    let home = format!("/tmp/mte-none-{}", std::process::id());
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::cargo_bin("microterm")
        .unwrap()
        .args(["cli", "list"])
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", format!("{home}/run"))
        .env_remove("MICROTERM_UNIX_SOCKET")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no running"));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn workspace_with_three_tabs_and_get_text() {
    let mt = Instance::start();
    mt.cli(&["workspace", "create", "demo"]);
    assert!(mt.cli(&["workspace", "list"]).contains("demo"));
    let panes: Vec<String> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|w| {
            mt.spawn_sh(
                &format!("echo word-{w}; sleep 60"),
                &["--workspace", "demo"],
            )
        })
        .collect();
    let list = mt.list();
    assert_eq!(tab_count(&list, "demo"), 3);
    assert!(
        mt.cli(&["workspace", "list"]).contains("demo *"),
        "spawning switches to it"
    );
    for (pane, w) in panes.iter().zip(["alpha", "beta", "gamma"]) {
        let t = mt.wait_text(pane, &format!("word-{w}"));
        assert!(!t.contains("word-beta") || w == "beta");
    }
    // Text output of `list` is a readable tree.
    let tree = mt.cli(&["list"]);
    assert!(tree.contains("workspace demo (active)") && tree.contains("tab "));
    mt.cli(&["workspace", "rename", "demo", "demo2"]);
    assert!(mt.cli(&["workspace", "list"]).contains("demo2"));
    mt.cli(&["workspace", "switch", "default"]);
    assert_eq!(mt.cli_code(&["workspace", "switch", "ghost"]), 3);
}

#[test]
fn pane_env_is_exported_and_pane_defaults_to_it() {
    let mt = Instance::start();
    let pane = mt.spawn_sh(
        "echo pane=$MICROTERM_PANE; echo sock=$MICROTERM_UNIX_SOCKET; sleep 60",
        &[],
    );
    let t = mt.wait_text(&pane, "sock=");
    assert!(t.contains(&format!("pane={pane}")), "{t}");
    assert!(t.contains(&format!("sock={}", mt.socket)), "{t}");
    // Without --pane, $MICROTERM_PANE is used.
    let out = mt
        .cmd(&["cli", "get-text"])
        .env("MICROTERM_PANE", &pane)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("pane="));
    // Neither flag nor variable: usage error.
    assert_eq!(mt.cli_code(&["get-text"]), 1);
}

#[test]
fn send_text_pastes_by_default_and_types_with_no_paste() {
    let mt = Instance::start();
    let pane = mt.spawn_sh("read a; echo got:$a; read b; echo then:$b; sleep 60", &[]);
    mt.cli(&["send-text", "--pane", &pane, "hello\n"]);
    mt.wait_text(&pane, "got:hello");
    // TEXT omitted: read from stdin.
    let mut c = mt.cmd(&["cli", "send-text", "--no-paste", "--pane", &pane]);
    c.write_stdin("world\n").assert().success();
    mt.wait_text(&pane, "then:world");
}

#[test]
fn send_text_uses_bracketed_paste_when_the_app_asks() {
    let mt = Instance::start();
    // The shell turns on bracketed paste and dumps what it receives in hex.
    let pane = mt.spawn_sh(
        "printf '\\033[?2004h'; stty raw -echo; head -c 15 | od -An -c; sleep 60",
        &[],
    );
    mt.wait_text(&pane, "");
    std::thread::sleep(Duration::from_millis(300));
    mt.cli(&["send-text", "--pane", &pane, "abc"]);
    let t = mt.wait_text(&pane, "~");
    let squashed: String = t.split_whitespace().collect();
    assert!(
        squashed.contains("033[200~abc033[201~") || squashed.contains("\\033[200~abc\\033[201~"),
        "{t:?}"
    );
}

#[test]
fn get_text_ranges_scrollback_and_escapes() {
    let mt = Instance::start();
    let pane = mt.spawn_sh(
        "printf '\\033[1;31mred\\033[0m\\n'; seq 1 100; echo end-marker; sleep 60",
        &[],
    );
    mt.wait_text(&pane, "end-marker");
    // Into scrollback: line -1 is the row above the screen.
    let history = mt.text(&pane, &["--start-line", "-10", "--end-line", "-1"]);
    assert_eq!(history.lines().count(), 10, "{history:?}");
    assert!(
        history.lines().all(|l| l.trim().parse::<u32>().is_ok()),
        "{history:?}"
    );
    // All the way up to the first printed line.
    let all = mt.text(&pane, &["--start-line", "-1000"]);
    assert!(all.contains("red") && all.contains("end-marker"));
    // --escapes re-encodes SGR; plain output has none.
    let esc = mt.text(&pane, &["--start-line", "-1000", "--escapes"]);
    assert!(esc.contains("\x1b[0;1;31mred\x1b[0m"), "{esc:?}");
    assert!(!all.contains('\x1b'));
    // JSON wraps the text.
    let j: Value = serde_json::from_str(&mt.text(&pane, &["--json"])).unwrap();
    assert!(j["text"].as_str().unwrap().contains("end-marker"));
    assert_eq!(
        mt.cli_code(&[
            "get-text",
            "--pane",
            &pane,
            "--start-line",
            "5",
            "--end-line",
            "2"
        ]),
        1
    );
}

#[test]
fn tabs_titles_activation_and_kill() {
    let mt = Instance::start();
    let a = mt.spawn_sh("sleep 60", &["--title", "first"]);
    let _b = mt.spawn_sh("sleep 60", &["--title", "second"]);
    let c = mt.spawn_sh("sleep 60", &[]);
    let list = mt.list();
    let tabs = &list["workspaces"][0]["windows"][0]["tabs"];
    assert_eq!(tabs.as_array().unwrap().len(), 3);
    assert_eq!(tabs[0]["title"], "first");
    assert_eq!(tabs[2]["active"], true, "a new tab becomes active");

    mt.cli(&["activate-tab", "--tab-index", "0"]);
    assert_eq!(
        mt.list()["workspaces"][0]["windows"][0]["tabs"][0]["active"],
        true
    );
    mt.cli(&["activate-tab", "--tab-relative", "-1"]);
    assert_eq!(
        mt.list()["workspaces"][0]["windows"][0]["tabs"][2]["active"],
        true,
        "wraps"
    );
    let tab_id = tabs[1]["id"].as_u64().unwrap().to_string();
    mt.cli(&["activate-tab", "--tab-id", &tab_id]);
    assert_eq!(
        mt.list()["workspaces"][0]["windows"][0]["tabs"][1]["active"],
        true
    );

    mt.cli(&["set-tab-title", "renamed"]);
    assert_eq!(
        mt.list()["workspaces"][0]["windows"][0]["tabs"][1]["title"],
        "renamed"
    );
    mt.cli(&["set-tab-title", "--tab-id", &tab_id, ""]);

    assert_eq!(mt.cli_code(&["activate-tab"]), 1);
    assert_eq!(mt.cli_code(&["activate-tab", "--tab-index", "9"]), 3);
    assert_eq!(mt.cli_code(&["activate-tab", "--tab-id", "999"]), 3);

    mt.cli(&["kill-pane", "--pane", &c]);
    assert_eq!(tab_count(&mt.list(), "default"), 2);
    assert_eq!(mt.cli_code(&["kill-pane", "--pane", &c]), 3);
    mt.cli(&["kill-pane", "--pane", &a]);
    assert_eq!(tab_count(&mt.list(), "default"), 1);
    assert_eq!(mt.cli_code(&["get-text", "--pane", "424242"]), 3);
}

#[test]
fn a_pane_closes_when_its_process_exits() {
    let mt = Instance::start();
    mt.spawn_sh("sleep 60", &[]);
    mt.spawn_sh("echo bye", &[]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while tab_count(&mt.list(), "default") != 1 {
        assert!(Instant::now() < deadline, "the exited pane was not removed");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn record_command_and_replay_into_a_new_tab() {
    let mt = Instance::start();
    let cast = format!("{}/out.cast", mt.home);
    mt.cmd(&[
        "record",
        "-o",
        &cast,
        "--",
        "sh",
        "-c",
        "echo recorded-line; sleep 0.3; echo second-line",
    ])
    .assert()
    .success();
    let file = std::fs::File::open(&cast).unwrap();
    let reader = mt_record::Reader::new(BufReader::new(file)).unwrap();
    assert_eq!(reader.header().version, 2);
    let out: String = reader
        .filter_map(Result::ok)
        .filter_map(|e| match e.kind {
            mt_record::EventKind::Output(s) => Some(s),
            _ => None,
        })
        .collect();
    assert!(
        out.contains("recorded-line") && out.contains("second-line"),
        "{out:?}"
    );
    assert!(!std::path::Path::new(&format!("{cast}.go")).exists());

    let pane = mt
        .cli(&["replay", &cast, "--speed", "20"])
        .trim()
        .to_string();
    let t = mt.wait_text(&pane, "second-line");
    assert!(t.contains("recorded-line"));
    assert_eq!(mt.cli_code(&["replay", "/nonexistent.cast"]), 3);
    std::fs::write(format!("{}/bad.cast", mt.home), "not json\n").unwrap();
    assert_eq!(
        mt.cli_code(&["replay", &format!("{}/bad.cast", mt.home)]),
        1
    );
}

#[test]
fn cli_record_by_duration_with_input_opt_in() {
    let mt = Instance::start();
    let pane = mt.spawn_sh("while :; do echo tick; sleep 0.1; done", &[]);
    let cast = format!("{}/d.cast", mt.home);
    mt.cli(&["record", "--pane", &pane, "-o", &cast, "--duration", "1"]);
    let events: Vec<_> =
        mt_record::Reader::new(BufReader::new(std::fs::File::open(&cast).unwrap()))
            .unwrap()
            .map(Result::unwrap)
            .collect();
    assert!(events.len() >= 3, "{events:?}");
    assert!(events
        .iter()
        .all(|e| matches!(e.kind, mt_record::EventKind::Output(_))));
    // Stopping twice is a not-found.
    assert_eq!(
        mt.cli_code(&["record", "--pane", "9999", "-o", &cast, "--duration", "1"]),
        3
    );

    // --input records what is sent to the pane.
    let reader_pane = mt.spawn_sh("cat", &[]);
    let cast2 = format!("{}/i.cast", mt.home);
    let m = mt
        .cmd(&[
            "cli",
            "record",
            "--pane",
            &reader_pane,
            "-o",
            &cast2,
            "--duration",
            "2",
            "--input",
        ])
        .spawn_for_test();
    std::thread::sleep(Duration::from_millis(500));
    mt.cli(&["send-text", "--no-paste", "--pane", &reader_pane, "typed\n"]);
    drop(m);
    std::thread::sleep(Duration::from_millis(2200));
    let kinds: Vec<_> =
        mt_record::Reader::new(BufReader::new(std::fs::File::open(&cast2).unwrap()))
            .unwrap()
            .map(|e| e.unwrap().kind)
            .collect();
    assert!(
        kinds.contains(&mt_record::EventKind::Input("typed\n".into())),
        "{kinds:?}"
    );
}

trait SpawnForTest {
    fn spawn_for_test(&mut self) -> Child;
}

impl SpawnForTest for Command {
    fn spawn_for_test(&mut self) -> Child {
        let mut p = Process::new(self.get_program());
        p.args(self.get_args());
        for (k, v) in self.get_envs() {
            match v {
                Some(v) => p.env(k, v),
                None => p.env_remove(k),
            };
        }
        p.spawn().unwrap()
    }
}

#[test]
fn subscribe_streams_mux_events_and_versions_are_checked() {
    let mt = Instance::start();
    let mut s = UnixStream::connect(&mt.socket).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"{\"v\":9,\"id\":1,\"method\":\"mux.list\"}\n")
        .unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["error"]["code"], "version_mismatch");

    s.write_all(b"{\"v\":1,\"id\":2,\"method\":\"subscribe\"}\n")
        .unwrap();
    line.clear();
    r.read_line(&mut line).unwrap();
    assert!(line.contains("\"result\""));
    let pane = mt.spawn_sh("sleep 60", &[]);
    let mut seen = Vec::new();
    for _ in 0..20 {
        line.clear();
        r.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert!(v.get("id").is_none(), "notifications carry no id");
        seen.push(v["event"]["event"].as_str().unwrap().to_string());
        if seen.iter().any(|e| e == "pane_added") {
            break;
        }
    }
    assert!(seen.contains(&"pane_added".to_string()), "{seen:?}");
    let _ = pane;
}

#[test]
fn completions_for_all_shells() {
    for shell in ["bash", "zsh", "fish"] {
        let out = Command::cargo_bin("microterm")
            .unwrap()
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(out.status.success());
        let s = String::from_utf8(out.stdout).unwrap();
        assert!(s.contains("microterm") && s.contains("spawn"), "{shell}");
    }
}
