//! SPEC §15.5 acceptance: the launcher and the app binary are indistinguishable on the command
//! line. Runs both over a list of argv vectors and compares stdout, stderr and exit status.
#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin(name: &str) -> PathBuf {
    let launcher = PathBuf::from(env!("CARGO_BIN_EXE_microterm-launcher"));
    let p = launcher.with_file_name(name);
    assert!(
        p.exists(),
        "{} not built; run `cargo test --workspace` (or `cargo build -p microterm`)",
        p.display()
    );
    p
}

fn run(exe: &PathBuf, args: &[&str], home: &std::path::Path) -> Output {
    Command::new(exe)
        .args(args)
        .env_remove("MICROTERM_UNIX_SOCKET")
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn")
}

#[test]
fn launcher_matches_app_binary() {
    let launcher = PathBuf::from(env!("CARGO_BIN_EXE_microterm-launcher"));
    let app = bin("microterm");
    let home = std::env::temp_dir().join(format!("mt-launcher-test-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let vectors: &[&[&str]] = &[
        &["--help"],
        &["-h"],
        &["--version"],
        &["-V", "--verbose"],
        &["--verbose"],
        &["--no-such-flag"],
        &["bogus-subcommand"],
        &["ctl", "--help"],
        &["ctl", "pane", "--help"],
        &["ctl", "pane", "list"],
        &["ctl", "list", "--json"],
        &["config", "--help"],
        &["themes", "list", "--json"],
        &["completions", "bash"],
        &["completions", "zsh"],
        &["completions", "fish"],
        &["ssh"],
        &["serial", "--help"],
        &["shell-integration", "zsh"],
    ];
    for v in vectors {
        let a = run(&launcher, v, &home);
        let b = run(&app, v, &home);
        assert_eq!(a.status.code(), b.status.code(), "exit status for {v:?}");
        assert_eq!(
            String::from_utf8_lossy(&a.stdout),
            String::from_utf8_lossy(&b.stdout),
            "stdout for {v:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&a.stderr),
            String::from_utf8_lossy(&b.stderr),
            "stderr for {v:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&home);
}
