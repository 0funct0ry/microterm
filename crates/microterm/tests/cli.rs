use assert_cmd::Command;
use predicates::prelude::*;

fn microterm() -> Command {
    Command::cargo_bin("microterm").unwrap()
}

#[test]
fn version_prints_package_version() {
    microterm()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_lists_subcommands() {
    microterm()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("ssh").and(predicate::str::contains("cli")));
}

#[test]
fn stubs_name_their_milestone() {
    for (args, milestone) in [
        (vec!["ssh", "host"], "M10"),
        (vec!["ls-fonts"], "M4"),
        (vec!["themes", "list"], "M3"),
        (vec!["config", "path"], "M3"),
        (vec!["completions", "zsh"], "M9"),
        (vec!["cli", "list"], "M9"),
        (vec!["record", "-o", "a.cast", "--", "ls"], "M9"),
    ] {
        microterm()
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "not implemented yet ({milestone})"
            )));
    }
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
    microterm().arg("bogus").assert().code(1);
}
