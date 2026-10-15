//! `microterm config …` and `microterm themes …` (SPEC §8, §12.1).

use std::path::Path;
use std::process::Command as Process;

use mt_config::{editor, load};
use mt_theme::{Registry, Theme};

use crate::cli::{ConfigCmd, ThemesCmd};

pub fn config(cmd: &ConfigCmd, config_flag: Option<&Path>) -> i32 {
    let path = load::config_path(config_flag);
    match cmd {
        ConfigCmd::Path => {
            println!("{}", path.display());
            0
        }
        ConfigCmd::ShowDefaults => {
            print!("{}", mt_config::template::DEFAULT_TOML);
            0
        }
        ConfigCmd::Check => check(&path),
        ConfigCmd::Edit => edit(&path),
    }
}

/// Same validation path as the GUI's hot reload, plus theme-name resolution.
fn check(path: &Path) -> i32 {
    let name = load::display_name(path);
    match load::load(path) {
        Err(diags) => {
            for d in &diags {
                eprintln!("{}", d.render(&name));
            }
            1
        }
        Ok(loaded) => {
            for d in &loaded.warnings {
                eprintln!("warning: {}", d.render(&name));
            }
            let (registry, theme_errors) = Registry::with_user_dir(&load::themes_dir(path));
            for e in &theme_errors {
                eprintln!("warning: {e}");
            }
            // Unknown action names are only found once the action registry is consulted.
            let specs = mt_config::keys::bindings(&loaded.config).unwrap_or_default();
            let (_, key_warnings) =
                mt_ui::actions::keymap_chips(mt_ui::platform::Platform::current(), &specs);
            for w in &key_warnings {
                eprintln!("warning: {name} — {w}");
            }
            let mut code = 0;
            for t in loaded.config.theme.names() {
                if registry.get(t).is_none() {
                    eprintln!("{name} — unknown theme \"{t}\" (see `microterm themes list`)");
                    code = 1;
                }
            }
            if code == 0 {
                println!("{} is valid", path.display());
            }
            code
        }
    }
}

fn edit(path: &Path) -> i32 {
    match editor::ensure_config_file(path) {
        Ok(true) => eprintln!("created {}", path.display()),
        Ok(false) => {}
        Err(e) => {
            eprintln!("cannot create {}: {e}", path.display());
            return 1;
        }
    }
    let (argv, from_editor) = match editor::editor_argv_from_env(path) {
        Some(a) => (a, true),
        None => (editor::opener_argv(path), false),
    };
    let status = Process::new(&argv[0]).args(&argv[1..]).status();
    match status {
        Ok(s) if s.success() => 0,
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            let what = if from_editor { "editor" } else { "opener" };
            eprintln!("cannot run {what} `{}`: {e}", argv[0]);
            1
        }
    }
}

pub fn themes(cmd: &ThemesCmd, config_flag: Option<&Path>) -> i32 {
    let cfg_path = load::config_path(config_flag);
    let (registry, errors) = Registry::with_user_dir(&load::themes_dir(&cfg_path));
    for e in &errors {
        eprintln!("warning: {e}");
    }
    let bundled = Registry::bundled();
    let source = |t: &Theme| match bundled.get(&t.name) {
        Some(b) if b == t => "bundled",
        _ => "user",
    };
    match cmd {
        ThemesCmd::List { json } => {
            if *json {
                let items: Vec<String> = registry
                    .all()
                    .iter()
                    .map(|t| {
                        format!(
                            "{{\"name\":{},\"appearance\":\"{}\",\"source\":\"{}\",\"license\":{}}}",
                            json_str(&t.name),
                            if t.is_dark() { "dark" } else { "light" },
                            source(t),
                            json_str(&t.license)
                        )
                    })
                    .collect();
                println!("[{}]", items.join(","));
            } else {
                for t in registry.all() {
                    println!(
                        "{:<24} {:<6} {}",
                        t.name,
                        if t.is_dark() { "dark" } else { "light" },
                        source(t)
                    );
                }
            }
            0
        }
        ThemesCmd::Show { name } => match registry.get(name) {
            None => {
                eprintln!("unknown theme \"{name}\" (see `microterm themes list`)");
                1
            }
            Some(t) => {
                show(t, source(t));
                0
            }
        },
    }
}

fn show(t: &Theme, source: &str) {
    println!("name        {}", t.name);
    println!("appearance  {}", if t.is_dark() { "dark" } else { "light" });
    println!("source      {source}");
    println!("author      {}", t.author);
    println!("license     {}", t.license);
    println!("contrast    {:.2}:1", t.contrast());
    println!("foreground  {}", t.foreground);
    println!("background  {}", t.background);
    println!("cursor      {}", t.cursor);
    println!("selection   {}", t.selection_bg);
    let row = |label: &str, cs: &[mt_theme::Rgba]| {
        let v: Vec<String> = cs.iter().map(ToString::to_string).collect();
        println!("{label:<11} {}", v.join(" "));
    };
    row("ansi", &t.ansi[..8]);
    row("brights", &t.ansi[8..]);
    println!("-- ui (derived unless the theme overrides) --");
    let u = &t.ui;
    for (k, v) in [
        ("surface", u.surface),
        ("border", u.border),
        ("text", u.text),
        ("text_muted", u.text_muted),
        ("accent", u.accent),
    ] {
        println!("{k:<11} {v}");
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
