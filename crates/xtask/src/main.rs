//! Repository automation: `check-themes` (M3) and `gen-docs` (arrives in M6).

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "check-themes" => check_themes(),
        "gen-docs" => {
            eprintln!("xtask gen-docs: not implemented yet (M6)");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: cargo run -p xtask -- check-themes | gen-docs");
            ExitCode::from(1)
        }
    }
}

/// `make themes`: every `themes/*.toml` must parse with 16 colours, name a permissive licence
/// and reach 4.5:1 foreground/background contrast; each licence must appear in LICENSES.md and
/// every file must be bundled into the binary.
fn check_themes() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes");
    let licenses = std::fs::read_to_string(root.join("LICENSES.md")).unwrap_or_default();
    let bundled = mt_theme::Registry::bundled();
    let mut failures = 0usize;
    let mut count = 0usize;
    let mut files: Vec<_> = match std::fs::read_dir(&root) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect(),
        Err(e) => {
            eprintln!("cannot read {}: {e}", root.display());
            return ExitCode::from(1);
        }
    };
    files.sort();
    for f in &files {
        count += 1;
        let shown = f.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(f).unwrap_or_default();
        match mt_theme::parse_theme(&src).and_then(|t| t.validate().map(|()| t)) {
            Err(e) => {
                eprintln!("FAIL {shown}: {e}");
                failures += 1;
            }
            Ok(t) => {
                if !licenses.contains(&t.name) {
                    eprintln!(
                        "FAIL {shown}: \"{}\" is not recorded in themes/LICENSES.md",
                        t.name
                    );
                    failures += 1;
                }
                if bundled.get(&t.name).is_none() {
                    eprintln!(
                        "FAIL {shown}: not embedded (add it to crates/mt_theme/src/bundled.rs)"
                    );
                    failures += 1;
                }
            }
        }
    }
    if count != bundled.len() {
        eprintln!(
            "FAIL: {count} theme files but {} bundled themes",
            bundled.len()
        );
        failures += 1;
    }
    if failures == 0 {
        println!("themes: {count} themes OK");
        ExitCode::SUCCESS
    } else {
        eprintln!("themes: {failures} problem(s)");
        ExitCode::from(1)
    }
}
