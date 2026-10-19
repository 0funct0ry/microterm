//! The product docs and demos must use the `ctl` name: M10.1 removed `microterm cli` outright
//! (nothing was released, so there is no alias).

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !matches!(name.as_str(), "node_modules" | "dist" | ".astro" | ".git") {
                walk(&p, out);
            }
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("md" | "mdx" | "astro" | "mjs" | "ts" | "json")
        ) {
            out.push(p);
        }
    }
}

#[test]
fn no_doc_mentions_microterm_cli() {
    let mut files = Vec::new();
    walk(&root().join("site"), &mut files);
    walk(&root().join("docs"), &mut files);
    assert!(!files.is_empty(), "found no docs to check");
    let bad: Vec<String> = files
        .iter()
        .filter(|f| {
            std::fs::read_to_string(f).is_ok_and(|s| {
                s.contains("microterm cli") || s.contains("`cli ") || s.contains("microterm-cli")
            })
        })
        .map(|f| f.display().to_string())
        .collect();
    assert!(
        bad.is_empty(),
        "`microterm cli` is gone, use `microterm ctl`: {bad:?}"
    );
}
