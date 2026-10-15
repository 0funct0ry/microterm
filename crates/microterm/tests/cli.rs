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

fn temp_cfg(name: &str, contents: Option<&str>) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mt_cli_{}_{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("microterm.toml");
    if let Some(c) = contents {
        std::fs::write(&f, c).unwrap();
    }
    f
}

#[test]
fn config_path_honours_flag_and_env() {
    let f = temp_cfg("path", None);
    microterm()
        .args(["--config", f.to_str().unwrap(), "config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(f.to_str().unwrap()));
    microterm()
        .env("MICROTERM_CONFIG", &f)
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(f.to_str().unwrap()));
}

#[test]
fn config_show_defaults_is_valid_config() {
    let out = microterm()
        .args(["config", "show-defaults"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let f = temp_cfg("defaults", Some(&String::from_utf8(out.stdout).unwrap()));
    microterm()
        .args(["--config", f.to_str().unwrap(), "config", "check"])
        .assert()
        .success();
}

#[test]
fn config_check_reports_spans_and_warnings() {
    let bad = temp_cfg("bad", Some("[window]\nopacity = 3\n"));
    microterm()
        .args(["--config", bad.to_str().unwrap(), "config", "check"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("microterm.toml:2:11"));
    let warn = temp_cfg("warn", Some("fnt = 1\n"));
    microterm()
        .args(["--config", warn.to_str().unwrap(), "config", "check"])
        .assert()
        .success()
        .stderr(predicate::str::contains("did you mean \"font\""));
    let action = temp_cfg(
        "action",
        Some("[[keys]]\nkey = \"ctrl+alt+x\"\naction = \"Frobnicate\"\n"),
    );
    microterm()
        .args(["--config", action.to_str().unwrap(), "config", "check"])
        .assert()
        .success()
        .stderr(predicate::str::contains("unknown action \"Frobnicate\""));
    let theme = temp_cfg("theme", Some("theme = \"Nope\"\n"));
    microterm()
        .args(["--config", theme.to_str().unwrap(), "config", "check"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unknown theme"));
}

#[test]
fn config_edit_creates_file_and_runs_editor() {
    let f = temp_cfg("edit", None);
    assert!(!f.exists());
    microterm()
        .env("VISUAL", "true")
        .args(["--config", f.to_str().unwrap(), "config", "edit"])
        .assert()
        .success();
    assert!(std::fs::read_to_string(&f)
        .unwrap()
        .contains("scrollback_lines"));
}

#[test]
fn themes_list_and_show() {
    let f = temp_cfg("themes", None);
    let cfg = f.to_str().unwrap();
    microterm()
        .args(["--config", cfg, "themes", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Tokyo Night").and(predicate::str::contains("bundled")));
    microterm()
        .args(["--config", cfg, "themes", "list", "--json"])
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("[{\"name\":")
                .and(predicate::str::contains("\"One Dark\"")),
        );
    microterm()
        .args(["--config", cfg, "themes", "show", "nord"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#2e3440"));
    microterm()
        .args(["--config", cfg, "themes", "show", "nope"])
        .assert()
        .code(1);
}

#[test]
fn user_themes_are_listed() {
    let f = temp_cfg("usertheme", None);
    let dir = f.parent().unwrap().join("themes");
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../themes/nord.toml"
    ))
    .unwrap()
    .replace("name = \"Nord\"", "name = \"My Custom\"");
    std::fs::write(dir.join("mine.toml"), src).unwrap();
    microterm()
        .args(["--config", f.to_str().unwrap(), "themes", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("My Custom").and(predicate::str::contains("user")));
}

#[test]
fn ls_fonts_names_the_font_per_character() {
    let dir = std::env::temp_dir().join("microterm-lsfonts-test");
    std::fs::create_dir_all(&dir).unwrap();
    microterm()
        .arg("--config")
        .arg(dir.join("none.toml"))
        .args(["ls-fonts", "--text", "a"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Fallback chain:")
                .and(predicate::str::contains("U+0061"))
                .and(predicate::str::contains("JetBrains Mono  (font.family)")),
        );
}
