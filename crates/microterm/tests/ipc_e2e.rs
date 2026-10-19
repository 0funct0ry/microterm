//! End-to-end tests of `microterm ctl …` against a hidden `--headless` instance (SPEC §14):
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
        Self::start_with(None)
    }

    /// Starts with `config` as the user's `microterm.toml`.
    fn start_with(config: Option<&str>) -> Instance {
        let home = format!(
            "/tmp/mte-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        std::fs::create_dir_all(format!("{home}/run")).unwrap();
        if let Some(cfg) = config {
            std::fs::create_dir_all(format!("{home}/cfg/microterm")).unwrap();
            std::fs::write(format!("{home}/cfg/microterm/microterm.toml"), cfg).unwrap();
        }
        let mut child = Process::new(assert_cmd::cargo::cargo_bin("microterm"))
            .arg("--headless")
            .envs(Self::env(&home))
            .env_remove("MICROTERM_UNIX_SOCKET")
            .env_remove("MICROTERM_PANE")
            .env_remove("MICROTERM_TAB")
            .env_remove("MICROTERM_WINDOW")
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
            .env_remove("MICROTERM_TAB")
            .env_remove("MICROTERM_WINDOW")
            .timeout(Duration::from_secs(30));
        c
    }

    fn ctl(&self, args: &[&str]) -> String {
        let out = self.cmd(&[&["ctl"][..], args].concat()).output().unwrap();
        assert!(
            out.status.success(),
            "ctl {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn ctl_code(&self, args: &[&str]) -> i32 {
        self.cmd(&[&["ctl"][..], args].concat())
            .output()
            .unwrap()
            .status
            .code()
            .unwrap()
    }

    /// Like `ctl_code`, with stderr.
    fn ctl_err(&self, args: &[&str]) -> (i32, String) {
        let out = self.cmd(&[&["ctl"][..], args].concat()).output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// `tab new -- sh -c SCRIPT` (kept alive by the script): `(tab, pane, window)` ids.
    fn spawn_ids(&self, script: &str, extra: &[&str]) -> (String, String, String) {
        let mut args = vec!["tab", "new"];
        args.extend_from_slice(extra);
        args.extend(["--", "sh", "-c", script]);
        parse_ids(&self.ctl(&args))
    }

    /// Spawns `sh -c SCRIPT` and returns its pane id.
    fn spawn_sh(&self, script: &str, extra: &[&str]) -> String {
        self.spawn_ids(script, extra).1
    }

    fn text(&self, pane: &str, extra: &[&str]) -> String {
        let mut args = vec!["pane", "get-text", "--pane", pane];
        args.extend_from_slice(extra);
        self.ctl(&args)
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
        serde_json::from_str(&self.ctl(&["list", "--json"])).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&self.ctl(&a)).unwrap()
    }

    /// Waits until `f` holds (a pane's exit is applied asynchronously).
    fn until(&self, what: &str, f: impl Fn(&Instance) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `tab=5 pane=6 window=2`.
fn parse_ids(out: &str) -> (String, String, String) {
    let get = |k: &str| {
        out.split_whitespace()
            .find_map(|w| w.strip_prefix(&format!("{k}=")))
            .unwrap_or_else(|| panic!("no {k}= in {out:?}"))
            .to_string()
    };
    (get("tab"), get("pane"), get("window"))
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
        .args(["ctl", "list"])
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
    mt.ctl(&["workspace", "create", "demo"]);
    assert!(mt.ctl(&["workspace", "list"]).contains("demo"));
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
    let ws: Value = mt.json(&["workspace", "list"]);
    let demo = ws
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "demo")
        .unwrap();
    assert_eq!(demo["active"], true, "spawning switches to it");
    for (pane, w) in panes.iter().zip(["alpha", "beta", "gamma"]) {
        let t = mt.wait_text(pane, &format!("word-{w}"));
        assert!(!t.contains("word-beta") || w == "beta");
    }
    // Text output of `list` is a readable tree.
    let tree = mt.ctl(&["list"]);
    assert!(tree.contains("workspace demo (active)") && tree.contains("tab "));
    mt.ctl(&["workspace", "rename", "demo", "demo2"]);
    assert!(mt.ctl(&["workspace", "list"]).contains("demo2"));
    mt.ctl(&["workspace", "switch", "default"]);
    assert_eq!(mt.ctl_code(&["workspace", "switch", "ghost"]), 3);
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
        .cmd(&["ctl", "pane", "get-text"])
        .env("MICROTERM_PANE", &pane)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("pane="));
    // Neither flag nor variable: a read-only verb falls back to the focused pane, an input verb
    // is a usage error.
    assert_eq!(mt.ctl_code(&["pane", "get-text"]), 0);
    assert_eq!(mt.ctl_code(&["pane", "send-text", "x"]), 1);
}

#[test]
fn send_text_pastes_by_default_and_types_with_no_paste() {
    let mt = Instance::start();
    let pane = mt.spawn_sh("read a; echo got:$a; read b; echo then:$b; sleep 60", &[]);
    mt.ctl(&["pane", "send-text", "--pane", &pane, "hello\n"]);
    mt.wait_text(&pane, "got:hello");
    // TEXT omitted: read from stdin.
    let mut c = mt.cmd(&["ctl", "pane", "send-text", "--no-paste", "--pane", &pane]);
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
    mt.ctl(&["pane", "send-text", "--pane", &pane, "abc"]);
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
        mt.ctl_code(&[
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
fn tabs_titles_focus_and_close() {
    let mt = Instance::start();
    let (_, a, _) = mt.spawn_ids("sleep 60", &["--title", "first"]);
    let (_, _b, _) = mt.spawn_ids("sleep 60", &["--title", "second"]);
    let (_, c, _) = mt.spawn_ids("sleep 60", &[]);
    let list = mt.list();
    let tabs = &list["workspaces"][0]["windows"][0]["tabs"];
    assert_eq!(tabs.as_array().unwrap().len(), 3);
    assert_eq!(tabs[0]["title"], "first");
    assert_eq!(tabs[2]["active"], true, "a new tab becomes active");

    let active = |i: usize| mt.list()["workspaces"][0]["windows"][0]["tabs"][i]["active"] == true;
    mt.ctl(&["tab", "focus", "--index", "0"]);
    assert!(active(0));
    mt.ctl(&["tab", "focus", "--relative", "-1"]);
    assert!(active(2), "wraps");
    let tab_id = tabs[1]["id"].as_u64().unwrap().to_string();
    mt.ctl(&["tab", "focus", "--tab", &tab_id]);
    assert!(active(1));
    mt.ctl(&["tab", "focus", "--match", "title:first"]);
    assert!(active(0));

    mt.ctl(&["tab", "rename", "--tab", &tab_id, "renamed"]);
    assert_eq!(
        mt.list()["workspaces"][0]["windows"][0]["tabs"][1]["title"],
        "renamed"
    );
    mt.ctl(&["tab", "rename", "--tab", &tab_id, ""]);

    // No target for an input/destructive verb is a usage error; focus needs one too.
    assert_eq!(mt.ctl_code(&["tab", "focus"]), 1);
    assert_eq!(mt.ctl_code(&["tab", "rename", "x"]), 1);
    assert_eq!(mt.ctl_code(&["tab", "focus", "--index", "9"]), 3);
    assert_eq!(mt.ctl_code(&["tab", "focus", "--tab", "999"]), 3);

    mt.ctl(&["pane", "close", "--pane", &c, "--force"]);
    assert_eq!(tab_count(&mt.list(), "default"), 2);
    assert_eq!(mt.ctl_code(&["pane", "close", "--pane", &c, "--force"]), 3);
    mt.ctl(&["pane", "close", "--pane", &a]);
    assert_eq!(tab_count(&mt.list(), "default"), 1);
    assert_eq!(mt.ctl_code(&["pane", "get-text", "--pane", "424242"]), 3);
}

#[test]
fn list_verbs_have_the_documented_json_shapes() {
    let mt = Instance::start();
    let (tab, pane, window) = mt.spawn_ids("sleep 60", &["--title", "shape", "--cwd", "/tmp"]);
    let panes = mt.json(&["pane", "list"]);
    let p = &panes[0];
    for key in [
        "id",
        "tab",
        "window",
        "workspace",
        "domain",
        "title",
        "cwd",
        "process",
        "size",
        "focused",
        "exited",
        "exit_code",
    ] {
        assert!(p.get(key).is_some(), "pane.{key} missing in {p}");
    }
    assert_eq!(p["id"].to_string(), pane);
    assert_eq!(p["tab"].to_string(), tab);
    assert_eq!(p["window"].to_string(), window);
    assert_eq!(p["workspace"], "default");
    assert_eq!(p["domain"], "local");
    assert_eq!(p["focused"], true);
    assert_eq!(p["exited"], false);
    assert!(p["exit_code"].is_null());
    assert!(p["size"]["cols"].as_u64().unwrap() > 0);
    assert_eq!(mt.json(&["pane", "get", "--pane", &pane])["id"], p["id"]);

    let tabs = mt.json(&["tab", "list"]);
    assert_eq!(tabs[0]["title"], "shape");
    assert_eq!(tabs[0]["panes"][0], p["id"]);
    assert_eq!(tabs[0]["window"].to_string(), window);
    let windows = mt.json(&["window", "list"]);
    assert_eq!(windows[0]["tabs"][0], tabs[0]["id"]);
    assert_eq!(windows[0]["workspace"], "default");
    // Filters.
    assert_eq!(
        mt.json(&["tab", "list", "--window", "99999"])
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        mt.json(&["pane", "list", "--match", "title:nope"])
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        mt.json(&["pane", "list", "--match", "title:sha*"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Inapplicable selector keys are usage errors, not empty matches.
    assert_eq!(mt.ctl_code(&["tab", "list", "--match", "cwd:/tmp"]), 1);
}

#[test]
fn errors_are_json_on_stderr_with_candidates() {
    let mt = Instance::start();
    mt.spawn_sh("sleep 60", &["--title", "build-a"]);
    mt.spawn_sh("sleep 60", &["--title", "build-b"]);
    let out = mt
        .cmd(&["ctl", "tab", "close", "--match", "title:build*", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(out.stdout.is_empty());
    let e: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(e["error"]["code"], "ambiguous");
    assert_eq!(e["error"]["candidates"].as_array().unwrap().len(), 2);
    // The human form lists the candidates too.
    let (code, err) = mt.ctl_err(&["tab", "close", "--match", "title:build*"]);
    assert_eq!(code, 5);
    assert!(err.contains("build-a") && err.contains("build-b"), "{err}");
    // Zero matches: 3, as JSON.
    let out = mt
        .cmd(&["ctl", "tab", "close", "--match", "title:nope", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let e: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(e["error"]["code"], "not_found");
    assert_eq!(tab_count(&mt.list(), "default"), 2, "nothing was closed");
    // --all closes every match.
    mt.ctl(&[
        "tab",
        "close",
        "--match",
        "title:build*",
        "--all",
        "--force",
    ]);
    mt.until("both tabs gone", |m| tab_count(&m.list(), "default") == 0);
}

#[test]
fn close_honours_confirm_close_and_force() {
    // `process`: an idle shell closes; a running foreground process needs --force.
    let mt = Instance::start_with(Some("confirm_close = \"process\"\n"));
    // `set -m` gives sleep its own process group in the PTY's foreground (as an interactive
    // shell would), so the pane counts as busy.
    let (tab, _pane, _) = mt.spawn_ids("set -m; sleep 60; true", &["--title", "busy"]);
    mt.until("the foreground process is seen", |m| {
        !m.json(&["pane", "list"])[0]["process"].is_null()
    });
    let (code, err) = mt.ctl_err(&["tab", "close", "--tab", &tab]);
    assert_eq!(code, 8, "{err}");
    assert!(err.contains("--force"), "{err}");
    assert_eq!(tab_count(&mt.list(), "default"), 1, "nothing was closed");
    let out = mt
        .cmd(&["ctl", "tab", "close", "--tab", &tab, "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(8));
    let e: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(e["error"]["code"], "confirm_required");
    mt.ctl(&["tab", "close", "--tab", &tab, "--force"]);
    assert_eq!(tab_count(&mt.list(), "default"), 0);

    // `always` prompts for everything; `never` for nothing.
    let always = Instance::start_with(Some("confirm_close = \"always\"\n"));
    let (_, pane, window) = always.spawn_ids("sleep 60", &[]);
    assert_eq!(always.ctl_code(&["pane", "close", "--pane", &pane]), 8);
    assert_eq!(
        always.ctl_code(&["window", "close", "--window", &window]),
        8
    );
    always.ctl(&["window", "close", "--window", &window, "--force"]);
    assert_eq!(tab_count(&always.list(), "default"), 0);

    let never = Instance::start_with(Some("confirm_close = \"never\"\n"));
    let (_, pane, _) = never.spawn_ids("sleep 60", &[]);
    never.ctl(&["pane", "close", "--pane", &pane]);
    assert_eq!(tab_count(&never.list(), "default"), 0);
}

#[test]
fn a_command_in_a_pane_closes_its_own_tab_with_no_flags() {
    let mt = Instance::start();
    let bin = assert_cmd::cargo::cargo_bin("microterm");
    let (_, keep, _) = mt.spawn_ids("sleep 60", &["--title", "keep"]);
    let script = format!("'{}' ctl tab close --force; sleep 60", bin.display());
    mt.spawn_ids(&script, &["--title", "doomed"]);
    mt.until("the pane closed its own tab", |m| {
        let tabs = m.json(&["tab", "list"]);
        tabs.as_array().unwrap().len() == 1 && tabs[0]["panes"][0] == keep.parse::<u64>().unwrap()
    });
}

#[test]
fn panes_know_their_own_ids_and_default_to_them() {
    let mt = Instance::start();
    let (tab, pane, window) = mt.spawn_ids(
        "echo ids=$MICROTERM_PANE/$MICROTERM_TAB/$MICROTERM_WINDOW; sleep 60",
        &[],
    );
    let t = mt.wait_text(&pane, "ids=");
    assert!(t.contains(&format!("ids={pane}/{tab}/{window}")), "{t}");
    // Resolution order: explicit id, then --match, then the environment.
    let (_, other, _) = mt.spawn_ids("sleep 60", &["--title", "other"]);
    let get = |args: &[&str]| {
        let out = mt
            .cmd(&[&["ctl", "pane", "get", "--json"][..], args].concat())
            .env("MICROTERM_PANE", &pane)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["id"].to_string()
    };
    assert_eq!(get(&[]), pane, "the environment");
    assert_eq!(get(&["--pane", &other]), other, "an explicit id wins");
    assert_eq!(
        get(&["--match", "title:other"]),
        other,
        "--match beats the environment"
    );
    // Read-only verbs fall back to the focused pane; acting ones do not.
    let focused = mt.json(&["pane", "get"])["id"].to_string();
    assert_eq!(focused, other, "the newest tab is focused");
    assert_eq!(mt.ctl_code(&["pane", "send-text", "x"]), 1);
    assert_eq!(mt.ctl_code(&["pane", "close"]), 1);
    // A stale id in the environment is "not found".
    let out = mt
        .cmd(&["ctl", "tab", "close", "--force"])
        .env("MICROTERM_TAB", "424242")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn windows_new_focus_and_close() {
    let mt = Instance::start();
    let (_, p1, w1) = mt.spawn_ids("sleep 60", &[]);
    let (tab2, p2, w2) = parse_ids(&mt.ctl(&["window", "new", "--", "sh", "-c", "sleep 60"]));
    assert_ne!(w1, w2);
    assert_eq!(mt.json(&["window", "list"]).as_array().unwrap().len(), 2);
    assert_eq!(
        mt.json(&["pane", "get", "--pane", &p2])["tab"].to_string(),
        tab2
    );
    mt.ctl(&["window", "focus", "--window", &w1]);
    assert_eq!(
        mt.json(&["pane", "get"])["id"].to_string(),
        p1,
        "the focused window decides"
    );
    mt.ctl(&["window", "focus", "--window", &w2]);
    assert_eq!(mt.json(&["pane", "get"])["id"].to_string(), p2);
    assert_eq!(mt.ctl_code(&["window", "focus"]), 1);
    assert_eq!(mt.ctl_code(&["window", "focus", "--window", "9999"]), 3);
    mt.ctl(&["window", "close", "--window", &w2, "--force"]);
    assert_eq!(mt.json(&["window", "list"]).as_array().unwrap().len(), 1);
    assert_eq!(
        mt.ctl_code(&["window", "close", "--window", &w2, "--force"]),
        3
    );
}

#[test]
fn workspace_delete_rules() {
    let mt = Instance::start();
    mt.ctl(&["workspace", "create", "tmp"]);
    mt.spawn_sh("sleep 60", &["--workspace", "tmp"]);
    assert_eq!(
        mt.ctl_code(&["workspace", "delete", "default", "--force"]),
        1,
        "protected"
    );
    assert_eq!(
        mt.ctl_code(&["workspace", "delete", "tmp", "--force"]),
        1,
        "active"
    );
    assert_eq!(mt.ctl_code(&["workspace", "delete", "ghost"]), 3);
    mt.ctl(&["workspace", "switch", "default"]);
    mt.ctl(&["workspace", "delete", "tmp"]);
    let names: Vec<String> = mt
        .json(&["workspace", "list"])
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["default"]);
    assert_eq!(tab_count(&mt.list(), "tmp"), 0);
}

#[test]
fn cli_is_gone() {
    let out = Command::cargo_bin("microterm")
        .unwrap()
        .args(["cli", "list"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "the old name is not an alias");
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

    let pane = mt.ctl(&["replay", &cast, "--speed", "20"]);
    let pane = parse_ids(&pane).1;
    let t = mt.wait_text(&pane, "second-line");
    assert!(t.contains("recorded-line"));
    assert_eq!(mt.ctl_code(&["replay", "/nonexistent.cast"]), 3);
    std::fs::write(format!("{}/bad.cast", mt.home), "not json\n").unwrap();
    assert_eq!(
        mt.ctl_code(&["replay", &format!("{}/bad.cast", mt.home)]),
        1
    );
}

#[test]
fn cli_record_by_duration_with_input_opt_in() {
    let mt = Instance::start();
    let pane = mt.spawn_sh("while :; do echo tick; sleep 0.1; done", &[]);
    let cast = format!("{}/d.cast", mt.home);
    mt.ctl(&["record", "--pane", &pane, "-o", &cast, "--duration", "1"]);
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
        mt.ctl_code(&["record", "--pane", "9999", "-o", &cast, "--duration", "1"]),
        3
    );

    // --input records what is sent to the pane.
    let reader_pane = mt.spawn_sh("cat", &[]);
    let cast2 = format!("{}/i.cast", mt.home);
    let m = mt
        .cmd(&[
            "ctl",
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
    mt.ctl(&[
        "pane",
        "send-text",
        "--no-paste",
        "--pane",
        &reader_pane,
        "typed\n",
    ]);
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
        for word in [
            "ctl",
            "workspace",
            "window",
            "tab",
            "pane",
            "send-text",
            "get-text",
            "close",
            "focus",
            "rename",
            "delete",
            "reload-config",
        ] {
            assert!(s.contains(word), "{shell} completions miss {word:?}");
        }
        // fish completes only two subcommand levels deep, so `ctl pane close --force` has no
        // flag completions there.
        if shell != "fish" {
            for flag in ["match", "force"] {
                assert!(s.contains(flag), "{shell} completions miss --{flag}");
            }
        }
        for gone in ["activate-tab", "set-tab-title", "kill-pane"] {
            assert!(!s.contains(gone), "{shell} completions still have {gone:?}");
        }
    }
}
