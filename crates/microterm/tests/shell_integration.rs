//! The shell-integration snippets, run in real shells on a PTY: every command must come out of
//! the OSC 133 marks with its exact text, output and status (SPEC §12.4).
//!
//! A separate test binary: the shells are forked from this process, and a child that has not
//! exec'd yet would otherwise keep other tests' sockets alive.

use std::sync::Arc;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use mt_term::{SpawnOptions, Terminal};

fn snippet(shell: &str) -> String {
    let out = Command::cargo_bin("microterm")
        .unwrap()
        .args(["shell-integration", shell])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn every_snippet_emits_all_four_marks_and_the_working_directory() {
    for shell in ["bash", "zsh", "fish"] {
        let text = snippet(shell);
        for needle in [
            "133;A",
            "133;B",
            "133;C",
            "133;D",
            "]7;file://",
            "]7717;menu;add",
            "mt-menu",
            "mt-key",
            "mt-palette",
            "mt-fold",
            "mt-link",
            "]7721;fold;begin",
            "]7726;link;add",
        ] {
            assert!(text.contains(needle), "{shell} snippet lacks {needle}");
        }
    }
}

#[test]
fn shell_integration_needs_a_shell() {
    let out = Command::cargo_bin("microterm")
        .unwrap()
        .args(["shell-integration", "tcsh"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

/// Starts `shell` on a PTY with the snippet sourced; `None` when the shell is not installed.
fn session(shell: &str, args: &[&str], source_cmd: &str) -> Option<Arc<Terminal>> {
    if std::process::Command::new(shell)
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: {shell} is not installed");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("mt-si-{}-{shell}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("integration");
    std::fs::write(&file, snippet(shell)).unwrap();
    let mut command = vec![shell.to_string()];
    command.extend(args.iter().map(|a| a.to_string()));
    let t = Terminal::spawn(
        SpawnOptions {
            command: Some(command),
            cols: 100,
            rows: 30,
            cwd: Some(dir.clone()),
            env: vec![
                ("HOME".into(), dir.to_string_lossy().into_owned()),
                ("PS1".into(), "$ ".into()),
            ],
            ..Default::default()
        },
        || {},
    )
    .unwrap();
    t.write(format!("{source_cmd} {}\r", file.display()).into_bytes());
    Some(Arc::new(t))
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Runs a few commands in an integrated shell and checks what the tracker made of them.
fn exercise(t: &Terminal) {
    until("the first prompt", || t.has_marks());
    let n0 = t.commands_finished();
    t.write(b"echo hello; echo world\r".to_vec());
    until("the first command", || t.commands_finished() > n0);
    let c = t.last_command(1).unwrap();
    assert_eq!(c.command, "echo hello; echo world");
    assert_eq!(c.output, "hello\nworld\n");
    assert_eq!(c.exit_code, Some(0));

    let n1 = t.commands_finished();
    t.write(b"cd /tmp; false\r".to_vec());
    until("the failing command", || t.commands_finished() > n1);
    let c = t.last_command(1).unwrap();
    assert_eq!(c.exit_code, Some(1));
    assert_eq!(c.output, "");

    // Empty lines do not create commands; the next one still reports its own status.
    let n2 = t.commands_finished();
    t.write(b"\r\rprintf 'a\\nb'; sh -c 'exit 7'\r".to_vec());
    until("the third command", || t.commands_finished() > n2);
    let c = t.last_command(1).unwrap();
    assert_eq!(c.exit_code, Some(7));
    assert!(c.output.starts_with("a\nb"), "{:?}", c.output);
    assert_eq!(t.commands_finished(), n2 + 1);
    // The working directory announced after `cd /tmp` reached the tracker.
    until("the cwd report", || {
        t.reported_cwd()
            .is_some_and(|c| c.ends_with("/tmp") || c.ends_with("/private/tmp"))
    });
}

#[test]
fn bash_snippet_produces_exact_command_records() {
    if let Some(t) = session("bash", &["--noprofile", "--norc", "-i"], ".") {
        exercise(&t);
    }
}

#[test]
fn zsh_snippet_produces_exact_command_records() {
    if let Some(t) = session("zsh", &["-f", "-i"], ".") {
        exercise(&t);
    }
}

#[test]
fn fish_snippet_produces_exact_command_records() {
    if let Some(t) = session("fish", &["--no-config", "-i"], "source") {
        exercise(&t);
    }
}

/// `mt-menu add|clear` in a real shell reach the pane's registry with the exact text.
fn menu_registration(t: &Terminal) {
    use mt_term::menu_reg::RegKind;
    // 7717 is part of the private OSC range: inert until the master switch is on.
    t.set_vt_config(mt_term::VtConfig {
        private: mt_term::private_osc::PrivateGate {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    });
    until("the first prompt", || t.has_marks());
    t.write(b"mt-menu add \"Run tests\" -- make test 'a b'\r".to_vec());
    until("the registration", || !t.menu_items().0.is_empty());
    let (items, dropped, malformed) = t.menu_items();
    assert_eq!((dropped, malformed), (0, 0));
    assert_eq!(items[0].id, "run-tests");
    assert_eq!(items[0].label, "Run tests");
    assert_eq!(items[0].kind, RegKind::Send);
    // Quoted for the shell it is typed into: bash and zsh escape, fish single-quotes.
    assert!(
        ["make test a\\ b", "make test 'a b'"].contains(&items[0].payload.as_str()),
        "{:?}",
        items[0].payload
    );

    t.write(b"mt-menu add --spawn --id sp Spawn -- cargo 'say \"hi\"'\r".to_vec());
    until("the spawn item", || t.menu_items().0.len() == 2);
    let sp = t.menu_items().0.into_iter().find(|i| i.id == "sp").unwrap();
    assert_eq!(sp.kind, RegKind::Spawn);
    assert_eq!(sp.payload, r#"["cargo","say \"hi\""]"#);

    t.write(b"mt-menu clear run-tests\r".to_vec());
    until("the clear", || t.menu_items().0.len() == 1);
    t.write(b"mt-menu clear\r".to_vec());
    until("the clear all", || t.menu_items().0.is_empty());
    // Usage errors register nothing.
    t.write(b"mt-menu add nolabel\r".to_vec());
    std::thread::sleep(Duration::from_millis(300));
    assert!(t.menu_items().0.is_empty());
}

#[test]
fn bash_mt_menu_registers_items() {
    if let Some(t) = session("bash", &["--noprofile", "--norc", "-i"], ".") {
        menu_registration(&t);
    }
}

#[test]
fn zsh_mt_menu_registers_items() {
    if let Some(t) = session("zsh", &["-f", "-i"], ".") {
        menu_registration(&t);
    }
}

#[test]
fn fish_mt_menu_registers_items() {
    if let Some(t) = session("fish", &["--no-config", "-i"], "source") {
        menu_registration(&t);
    }
}

/// `mt-key` and `mt-palette` reach the pane's stores with the exact text.
fn pane_registration(t: &Terminal) {
    use mt_term::menu_reg::RegKind;
    t.set_vt_config(mt_term::VtConfig {
        private: mt_term::private_osc::PrivateGate {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    });
    until("the first prompt", || t.has_marks());
    t.write(b"mt-key add --id t ctrl+alt+t -- make test\r".to_vec());
    until("the key binding", || !t.pane_keys().is_empty());
    let k = &t.pane_keys()[0];
    assert_eq!((k.id.as_str(), k.chord.as_str()), ("t", "ctrl-alt-t"));
    assert_eq!(k.kind, RegKind::Send);
    assert_eq!(k.payload, "make test");

    t.write(b"mt-palette add \"Run tests\" -- make test\r".to_vec());
    until("the palette entry", || !t.pane_palette().is_empty());
    let e = &t.pane_palette()[0];
    assert_eq!(
        (e.id.as_str(), e.label.as_str()),
        ("run-tests", "Run tests")
    );

    // A reserved chord registers nothing.
    t.write(b"mt-key add --id q cmd+q -- exit\r".to_vec());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(t.pane_keys().len(), 1);

    t.write(b"mt-key clear\r".to_vec());
    until("the key clear", || t.pane_keys().is_empty());
    t.write(b"mt-palette clear\r".to_vec());
    until("the palette clear", || t.pane_palette().is_empty());
}

#[test]
fn bash_mt_key_and_palette_register() {
    if let Some(t) = session("bash", &["--noprofile", "--norc", "-i"], ".") {
        pane_registration(&t);
    }
}

#[test]
fn zsh_mt_key_and_palette_register() {
    if let Some(t) = session("zsh", &["-f", "-i"], ".") {
        pane_registration(&t);
    }
}

#[test]
fn fish_mt_key_and_palette_register() {
    if let Some(t) = session("fish", &["--no-config", "-i"], "source") {
        pane_registration(&t);
    }
}
