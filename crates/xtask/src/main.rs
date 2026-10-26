//! Repository automation: `check-themes` (M3) and `gen-docs` (M6).

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "check-themes" => check_themes(),
        "gen-docs" => gen_docs(std::env::args().nth(2).as_deref() == Some("--check")),
        "licenses-assets" => licenses_assets(std::env::args().nth(2)),
        "check-assets" => check_assets(),
        _ => {
            eprintln!(
                "usage: cargo run -p xtask -- check-themes | gen-docs [--check] | \
                 licenses-assets <file> | check-assets"
            );
            ExitCode::from(1)
        }
    }
}

/// `gen-docs`: renders generated docs pages from the registries. With `--check` it writes
/// nothing and fails when a committed page is stale (CI).
fn gen_docs(check: bool) -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pages = [
        (
            mt_ui::actions::REFERENCE_DOC_PATH,
            mt_ui::actions::reference_markdown(),
        ),
        (
            microterm::agent::render::REFERENCE_DOC_PATH,
            microterm::agent::render::reference_markdown(),
        ),
        (
            mt_term::vt_doc::REFERENCE_DOC_PATH,
            mt_term::vt_doc::reference_markdown(),
        ),
        (
            mt_ui::menu::REFERENCE_DOC_PATH,
            mt_ui::menu::reference_markdown(),
        ),
        (
            mt_snippets::docs::REFERENCE_DOC_PATH,
            mt_snippets::docs::reference_markdown(),
        ),
        (
            mt_export::docs::REFERENCE_DOC_PATH,
            mt_export::docs::reference_markdown(),
        ),
        (
            mt_config::docs::REFERENCE_DOC_PATH,
            mt_config::docs::reference_markdown(),
        ),
        (CHANGELOG_DOC_PATH, changelog_markdown(&root)),
    ];
    let mut stale = 0;
    for (rel, content) in pages {
        let path = root.join(rel);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current == content {
            continue;
        }
        if check {
            eprintln!("STALE {rel}: run `cargo run -p xtask -- gen-docs` and commit the result");
            stale += 1;
        } else {
            if let Some(dir) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    eprintln!("cannot create {}: {e}", dir.display());
                    return ExitCode::from(1);
                }
            }
            if let Err(e) = std::fs::write(&path, content) {
                eprintln!("cannot write {rel}: {e}");
                return ExitCode::from(1);
            }
            println!("wrote {rel}");
        }
    }
    if stale == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
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

const CHANGELOG_DOC_PATH: &str = "site/src/content/docs/docs/changelog.md";

/// The site's Changelog page: `CHANGELOG.md` with its title swapped for front matter.
fn changelog_markdown(root: &Path) -> String {
    let src = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
    let body = src.strip_prefix("# Changelog").unwrap_or(&src).trim_start();
    format!(
        "---\ntitle: Changelog\ndescription: What changed in each microterm release.\n---\n\n\
         <!-- Generated by `cargo run -p xtask -- gen-docs` from CHANGELOG.md. Do not edit. -->\n\n{body}"
    )
}

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Every bundled font, icon and theme must be recorded in `assets/LICENSES.md` or
/// `themes/LICENSES.md` (SPEC §15.6).
fn check_assets() -> ExitCode {
    let root = root();
    let assets = std::fs::read_to_string(root.join("assets/LICENSES.md")).unwrap_or_default();
    let mut failures = 0usize;
    let files = |dir: &str, skip: &dyn Fn(&str) -> bool| -> Vec<String> {
        std::fs::read_dir(root.join(dir))
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !skip(n))
                    .collect()
            })
            .unwrap_or_default()
    };
    // Fonts: the family prefix (before the first `_`/`-`) must appear in the table.
    for f in files("assets/fonts", &|n| {
        n.starts_with("LICENSE") || n.starts_with("OFL")
    }) {
        let family = f.split(['_', '-']).next().unwrap_or(&f).to_string();
        if !assets.contains(&format!("fonts/{family}")) {
            eprintln!("FAIL assets/fonts/{f}: not covered by assets/LICENSES.md");
            failures += 1;
        }
    }
    // Icons are one Lucide set covered by a single row plus the licence file.
    if !assets.contains("Lucide") || !root.join("assets/icons/LICENSE-lucide.txt").exists() {
        eprintln!("FAIL assets/icons: Lucide licence missing");
        failures += 1;
    }
    if !assets.contains("microterm logo") {
        eprintln!("FAIL app icon: not recorded in assets/LICENSES.md");
        failures += 1;
    }
    for f in ["LICENSE", "assets/LICENSES.md", "themes/LICENSES.md"] {
        if !root.join(f).exists() {
            eprintln!("FAIL {f} is missing");
            failures += 1;
        }
    }
    if failures == 0 {
        println!("assets: licences cover every bundled font, icon and theme");
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `licenses-assets <file>`: splices the bundled-asset and theme licence tables into the
/// cargo-about output at the `<!--ASSETS-->` marker.
fn licenses_assets(file: Option<String>) -> ExitCode {
    let Some(file) = file else {
        eprintln!("licenses-assets: missing <file>");
        return ExitCode::from(1);
    };
    let root = root();
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();
    let html = std::fs::read_to_string(&file).unwrap_or_default();
    if !html.contains("<!--ASSETS-->") {
        eprintln!("licenses-assets: marker not found in {file}");
        return ExitCode::from(1);
    }
    let block = format!(
        "<h2>Bundled fonts, icons and artwork</h2>\n<pre>{}</pre>\n\
         <h2>Bundled themes</h2>\n<pre>{}</pre>\n",
        html_escape(&read("assets/LICENSES.md")),
        html_escape(&read("themes/LICENSES.md")),
    );
    match std::fs::write(&file, html.replace("<!--ASSETS-->", &block)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("licenses-assets: {e}");
            ExitCode::from(1)
        }
    }
}
