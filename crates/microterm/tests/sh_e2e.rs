//! End-to-end tests of `microterm sh` (SPEC §12.7): `-c`, script and stdin modes against a stub IPC
//! server (so requests and exit-code mapping are exact), and parity with `ctl` against a real
//! `--headless` instance.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use assert_cmd::Command;
use mt_proto::{ErrorCode, RpcError};
use serde_json::{json, Value};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn pane(id: u64, title: &str) -> Value {
    json!({
        "id": id, "tab": 3, "window": 1, "workspace": "default", "domain": "local",
        "title": title, "cwd": "/tmp", "process": "zsh",
        "size": {"cols": 80, "rows": 24}, "focused": id == 7, "exited": false
    })
}

type Answer = Result<Value, RpcError>;

/// A canned IPC server: answers by method name and records every call.
struct Stub {
    path: String,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Stub {
    fn start(answers: Vec<(&'static str, Answer)>) -> Stub {
        let path = format!(
            "/tmp/mts-{}-{}.sock",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&calls);
        let answers = Arc::new(answers);
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (log, answers) = (Arc::clone(&log), Arc::clone(&answers));
                std::thread::spawn(move || serve(conn, &log, &answers));
            }
        });
        Stub { path, calls }
    }

    fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }

    fn params(&self, method: &str) -> Option<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
    }

    fn sh(&self) -> Command {
        let mut c = Command::cargo_bin("microterm").unwrap();
        c.arg("sh")
            .args(["--socket", &self.path])
            .env_remove("MICROTERM_UNIX_SOCKET")
            .env_remove("MICROTERM_PANE")
            .env_remove("MICROTERM_TAB")
            .env_remove("MICROTERM_WINDOW")
            .env("NO_COLOR", "1")
            .timeout(Duration::from_secs(30));
        c
    }

    fn ctl(&self, args: &[&str]) -> Command {
        let mut c = Command::cargo_bin("microterm").unwrap();
        c.arg("ctl")
            .args(args)
            .env("MICROTERM_UNIX_SOCKET", &self.path)
            .env_remove("MICROTERM_PANE")
            .env_remove("MICROTERM_TAB")
            .env_remove("MICROTERM_WINDOW")
            .timeout(Duration::from_secs(30));
        c
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn serve(conn: UnixStream, log: &Mutex<Vec<(String, Value)>>, answers: &[(&'static str, Answer)]) {
    let mut writer = conn.try_clone().unwrap();
    for line in BufReader::new(conn).lines().map_while(Result::ok) {
        let v: Value = serde_json::from_str(&line).unwrap();
        let method = v["method"].as_str().unwrap_or("").to_string();
        log.lock()
            .unwrap()
            .push((method.clone(), v["params"].clone()));
        let answer = answers
            .iter()
            .find(|(m, _)| *m == method)
            .map(|(_, a)| a.clone())
            .unwrap_or_else(|| {
                Err(RpcError::new(
                    ErrorCode::BadRequest,
                    format!("stub: unexpected method {method}"),
                ))
            });
        let reply = mt_proto::encode_response(v["id"].as_u64(), answer);
        if writer.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}

fn listing() -> Stub {
    Stub::start(vec![
        (
            "pane.list",
            Ok(json!([pane(7, "zsh"), pane(8, "cargo watch")])),
        ),
        ("pane.get", Ok(pane(7, "zsh"))),
        ("pane.send_text", Ok(json!({}))),
        ("tab.new", Ok(json!({"pane": 12, "tab": 9, "window": 1}))),
    ])
}

fn stdout(c: &mut Command) -> (i32, String, String) {
    let out = c.output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn dash_c_matches_ctl_json_and_plain_output() {
    let stub = listing();
    let (code, sh_json, _) = stdout(stub.sh().args(["-c", "pane list", "--json"]));
    let (_, ctl_json, _) = stdout(&mut stub.ctl(&["pane", "list", "--json"]));
    assert_eq!(code, 0);
    assert_eq!(sh_json, ctl_json);
    let (_, sh_text, _) = stdout(stub.sh().args(["-c", "pane list"]));
    let (_, ctl_text, _) = stdout(&mut stub.ctl(&["pane", "list"]));
    assert_eq!(
        sh_text, ctl_text,
        "piped output is the plain TSV form of ctl"
    );
    assert!(sh_text.contains("cargo watch"));
}

#[test]
fn aliases_and_shorthand_send_the_canonical_request() {
    let stub = listing();
    let (code, _, _) = stdout(stub.sh().args(["-c", "p get 7"]));
    assert_eq!(code, 0);
    assert_eq!(stub.params("pane.get").unwrap()["pane"], 7);
    let (code, _, _) = stdout(stub.sh().args(["-c", "type 7 'echo hi'"]));
    assert_eq!(code, 0);
    let p = stub.params("pane.send_text").unwrap();
    assert_eq!(p["pane"], 7);
    assert_eq!(p["text"], "echo hi");
    let (_, _, _) = stdout(stub.sh().args(["-c", "pane list 'title:*cargo*,!exited'"]));
    assert_eq!(
        stub.params("pane.list").unwrap()["match"],
        "title:*cargo*,!exited"
    );
}

#[test]
fn use_sets_a_sticky_target_but_never_for_send_text() {
    let stub = listing();
    let script = "use pane 7\npane get\ntype hello\n";
    let (code, _, err) = stdout(stub.sh().args(["-c", script]));
    assert_eq!(stub.params("pane.get").unwrap()["pane"], 7);
    // `type hello` is send-text with no target: the sticky pane must not be injected.
    let p = stub.params("pane.send_text");
    assert!(
        p.as_ref().is_none_or(|p| p.get("pane").is_none()),
        "{p:?} {err}"
    );
    let _ = code;
    // An explicit ID beats the context.
    stdout(stub.sh().args(["-c", "use pane 7; pane get 9"]));
    assert_eq!(stub.params("pane.get").unwrap()["pane"], 9);
}

#[test]
fn variables_come_from_new_and_the_last_status() {
    let stub = listing();
    let script = "tab new --title build\npane get $pane\nset X=$?\n";
    let (code, _, err) = stdout(stub.sh().args(["-c", script]));
    assert_eq!(code, 0, "{err}");
    assert_eq!(stub.params("pane.get").unwrap()["pane"], 12);
}

#[test]
fn exit_codes_follow_the_ctl_table() {
    let stub = Stub::start(vec![
        (
            "pane.get",
            Err(RpcError::new(ErrorCode::NotFound, "pane 99 does not exist")),
        ),
        (
            "pane.close",
            Err(RpcError::new(
                ErrorCode::ConfirmRequired,
                "closing would ask for confirmation",
            )),
        ),
        (
            "pane.list",
            Err(RpcError {
                code: ErrorCode::Ambiguous,
                message: "2 panes match".into(),
                candidates: vec!["pane 1".into(), "pane 2".into()],
            }),
        ),
    ]);
    let (code, _, err) = stdout(stub.sh().args(["-c", "pane get 99"]));
    assert_eq!(code, 3);
    assert!(err.contains("pane 99 does not exist"), "{err}");
    let (code, _, err) = stdout(stub.sh().args(["-c", "pane close 5"]));
    assert_eq!(code, 8, "script mode never prompts: {err}");
    let (code, _, err) = stdout(stub.sh().args(["-c", "pane list"]));
    assert_eq!(code, 5);
    assert!(err.contains("pane 1") && err.contains("pane 2"), "{err}");
    let (code, _, err) = stdout(stub.sh().args(["-c", "pane list", "--json"]));
    assert_eq!(code, 5);
    assert!(err.contains(r#""candidates""#), "{err}");
    let (code, _, _) = stdout(stub.sh().args(["-c", "pane frobnicate"]));
    assert_eq!(code, 1);
}

#[test]
fn the_exit_status_is_the_last_commands() {
    let stub = Stub::start(vec![
        ("pane.get", Err(RpcError::new(ErrorCode::NotFound, "nope"))),
        ("pane.list", Ok(json!([]))),
    ]);
    let (code, _, _) = stdout(stub.sh().args(["-c", "pane get 1; pane list"]));
    assert_eq!(code, 0);
    let (code, _, _) = stdout(stub.sh().args(["-c", "pane list; pane get 1"]));
    assert_eq!(code, 3);
    let (code, _, _) = stdout(stub.sh().args(["-c", "pane get 1 && pane list"]));
    assert_eq!(code, 3);
    assert_eq!(
        stub.methods().iter().filter(|m| *m == "pane.list").count(),
        1 + 1
    );
    let (code, _, _) = stdout(stub.sh().args(["-c", "pane get 1 || pane list"]));
    assert_eq!(code, 0);
    let (code, _, _) = stdout(stub.sh().args(["-c", "exit"]));
    assert_eq!(code, 0);
}

#[test]
fn script_files_and_stdin_run_without_a_prompt() {
    let stub = listing();
    let file = format!("{}.sh", stub.path);
    std::fs::write(&file, "# a script\npane list\ntab new \\\n  --title t\n").unwrap();
    let (code, out, err) = stdout(stub.sh().arg(&file));
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("zsh"));
    assert!(stub.methods().contains(&"tab.new".to_string()));
    let _ = std::fs::remove_file(file);
    let (code, out, _) = stdout(stub.sh().write_stdin("pane list\n"));
    assert_eq!(code, 0);
    assert!(out.contains("cargo watch"));
    assert!(!out.contains("microterm>"), "no prompt in script mode");
}

#[test]
fn errexit_aborts_a_script() {
    let stub = Stub::start(vec![
        ("pane.get", Err(RpcError::new(ErrorCode::NotFound, "nope"))),
        ("pane.list", Ok(json!([]))),
    ]);
    let (code, _, _) = stdout(stub.sh().args(["-e", "-c", "pane get 1\npane list"]));
    assert_eq!(code, 3);
    assert!(!stub.methods().contains(&"pane.list".to_string()));
    let (code, _, _) = stdout(stub.sh().args(["-c", "pane get 1\npane list"]));
    assert_eq!(code, 0, "without -e the script goes on");
}

#[test]
fn readonly_rejects_mutating_verbs_on_the_client() {
    let stub = listing();
    for line in [
        "pane close 5",
        "tab new",
        "type 7 hi",
        "pane send-text --pane 7 hi",
    ] {
        let (code, _, err) = stdout(stub.sh().args(["--readonly", "-c", line]));
        assert_eq!(code, 1, "{line}");
        assert!(err.contains("--readonly"), "{err}");
    }
    let (code, _, _) = stdout(stub.sh().args(["--readonly", "-c", "pane list"]));
    assert_eq!(code, 0);
    let methods = stub.methods();
    assert!(
        !methods
            .iter()
            .any(|m| m == "pane.close" || m == "tab.new" || m == "pane.send_text"),
        "{methods:?}"
    );
}

#[test]
fn send_text_never_reads_stdin_inside_sh() {
    let stub = listing();
    let (code, _, err) = stdout(stub.sh().args(["-c", "pane send-text --pane 7"]));
    assert_eq!(code, 1);
    assert!(err.contains("needs TEXT"), "{err}");
    assert!(!stub.methods().contains(&"pane.send_text".to_string()));
}

#[test]
fn redirects_and_host_pipes() {
    let stub = listing();
    let out = format!("{}.out", stub.path);
    let (code, printed, _) = stdout(stub.sh().args(["-c", &format!("pane list > {out}")]));
    assert_eq!(code, 0);
    assert_eq!(printed, "");
    assert!(std::fs::read_to_string(&out)
        .unwrap()
        .contains("cargo watch"));
    let (_, piped, _) = stdout(stub.sh().args(["-c", "pane list | wc -l"]));
    assert_eq!(piped.trim(), "2");
    let (_, bang, _) = stdout(stub.sh().args(["-c", "!echo from-host"]));
    assert_eq!(bang.trim(), "from-host");
    let _ = std::fs::remove_file(out);
}

#[test]
fn no_instance_exits_2() {
    let mut c = Command::cargo_bin("microterm").unwrap();
    c.args(["sh", "--socket", "/tmp/mts-absent.sock", "-c", "list"])
        .env_remove("MICROTERM_UNIX_SOCKET");
    let (code, _, err) = stdout(&mut c);
    assert_eq!(code, 2);
    assert!(err.contains("no running microterm instance"), "{err}");
}

// --- against a real headless instance --------------------------------------------------------

struct Headless {
    child: Child,
    home: String,
}

impl Headless {
    fn start() -> Headless {
        let home = format!(
            "/tmp/mtsh-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        std::fs::create_dir_all(format!("{home}/run")).unwrap();
        let mut child = Process::new(assert_cmd::cargo::cargo_bin("microterm"))
            .arg("--headless")
            .envs(Self::env(&home))
            .env_remove("MICROTERM_UNIX_SOCKET")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        Headless { child, home }
    }

    fn env(home: &str) -> Vec<(String, String)> {
        vec![
            ("HOME".into(), home.into()),
            ("XDG_RUNTIME_DIR".into(), format!("{home}/run")),
            ("XDG_CONFIG_HOME".into(), format!("{home}/cfg")),
            ("XDG_STATE_HOME".into(), format!("{home}/state")),
            ("SHELL".into(), "/bin/sh".into()),
        ]
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::cargo_bin("microterm").unwrap();
        c.args(args)
            .envs(Self::env(&self.home))
            .env_remove("MICROTERM_UNIX_SOCKET")
            .env_remove("MICROTERM_PANE")
            .env_remove("MICROTERM_TAB")
            .env_remove("MICROTERM_WINDOW")
            .env("NO_COLOR", "1")
            .timeout(Duration::from_secs(30));
        c
    }
}

impl Drop for Headless {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
fn headless_json_matches_ctl_for_list_verbs() {
    let h = Headless::start();
    let (code, _, err) =
        stdout(&mut h.cmd(&["ctl", "tab", "new", "--title", "one", "--", "sleep", "30"]));
    assert_eq!(code, 0, "{err}");
    for (sh, ctl) in [
        ("pane list", vec!["pane", "list"]),
        ("tab list", vec!["tab", "list"]),
        ("window list", vec!["window", "list"]),
        ("workspace list", vec!["workspace", "list"]),
        ("list", vec!["list"]),
    ] {
        let mut ctl_args = vec!["ctl"];
        ctl_args.extend(ctl);
        ctl_args.push("--json");
        let (_, want, _) = stdout(&mut h.cmd(&ctl_args));
        let (code, got, err) = stdout(&mut h.cmd(&["sh", "-c", sh, "--json"]));
        assert_eq!(code, 0, "{sh}: {err}");
        assert_eq!(got, want, "{sh}");
    }
}

#[test]
fn headless_variables_and_the_sticky_context_work_together() {
    let h = Headless::start();
    let script = "tab new --title build -- sleep 30\npane get $pane --json\nuse pane $pane\npane get --json\n";
    let (code, out, err) = stdout(&mut h.cmd(&["sh", "-c", script]));
    assert_eq!(code, 0, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    // tab=… pane=… window=…, the JSON, `use`'s one-line context, the JSON again.
    assert_eq!(lines.len(), 4, "{out}");
    assert!(
        lines[0].starts_with("tab=") && lines[2].starts_with("pane "),
        "{out}"
    );
    let first: Value = serde_json::from_str(lines[1]).unwrap();
    let second: Value = serde_json::from_str(lines[3]).unwrap();
    assert_eq!(first["id"], second["id"]);
    let (code, out, _) = stdout(&mut h.cmd(&["sh", "-c", "pane list 'title:build*' --json"]));
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(out.trim())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
