//! Trusted directories for shell-registered and project menu items (SPEC §10.9.7).
//!
//! Trust lives only in the user's own config (`[menu_security] trusted_dirs`). Entries and the
//! pane's working directory are canonicalised (symlinks resolved) before they are compared,
//! component by component, so `/a/bc` is never inside `/a/b`.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::menu::MenuSecurity;

/// Longest label, payload and item count accepted from a project file (same as the OSC).
const MAX_LABEL: usize = 256;
const MAX_PAYLOAD: usize = 1024;
const MAX_ITEMS: usize = 16;

/// File names looked up in a pane's working directory, in order: the first one that exists
/// is used and the others are ignored.
pub const PROJECT_FILES: [&str; 4] = [
    ".microterm-menu.toml",
    ".mt-menu.toml",
    ".menu.toml",
    ".p-menu.toml",
];

/// The project menu file of `dir`: its name and contents.
pub fn read_project_file(dir: &Path) -> Option<(&'static str, String)> {
    PROJECT_FILES.iter().find_map(|name| {
        let p = dir.join(name);
        if p.is_file() {
            std::fs::read_to_string(p).ok().map(|t| (*name, t))
        } else {
            None
        }
    })
}

/// One `trusted_dirs` entry after parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEntry {
    pub dir: PathBuf,
    /// `"/dir/**"`: the directory and everything below it.
    pub subtree: bool,
}

/// Parses one entry: absolute (after `~` expansion), optionally ending in `/**`.
pub fn parse_entry(raw: &str, home: Option<&Path>) -> Result<TrustEntry, String> {
    let (path, subtree) = match raw.strip_suffix("/**") {
        Some(p) => (p, true),
        None => (raw, false),
    };
    if path.contains('*') || path.contains('?') {
        return Err(format!(
            "`{raw}`: only a trailing `/**` is supported (no other wildcards)"
        ));
    }
    let expanded = if path == "~" {
        home.map(Path::to_path_buf)
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.map(|h| h.join(rest))
    } else {
        Some(PathBuf::from(path))
    };
    let dir = expanded.ok_or_else(|| format!("`{raw}`: cannot expand `~` (no home directory)"))?;
    if !dir.is_absolute() {
        return Err(format!(
            "`{raw}`: trusted directories must be absolute paths"
        ));
    }
    if dir.components().any(|c| c == Component::ParentDir) {
        return Err(format!("`{raw}`: `..` is not allowed"));
    }
    if dir.parent().is_none() && subtree {
        return Err(format!("`{raw}`: trusting the whole filesystem is refused"));
    }
    Ok(TrustEntry { dir, subtree })
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Whether `cwd` is trusted by `sec`. Unparsable or missing entries trust nothing.
pub fn is_trusted(sec: &MenuSecurity, cwd: &Path) -> bool {
    let Ok(cwd) = cwd.canonicalize() else {
        return false;
    };
    let home = home();
    sec.trusted_dirs.iter().any(|raw| {
        let Ok(e) = parse_entry(raw, home.as_deref()) else {
            return false;
        };
        let Ok(dir) = e.dir.canonicalize() else {
            return false;
        };
        if e.subtree {
            cwd.starts_with(&dir)
        } else {
            cwd == dir
        }
    })
}

/// A `[[items]]` entry of a project file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectItem {
    pub id: Option<String>,
    pub label: String,
    pub send: Option<String>,
    pub ctl: Option<String>,
    pub spawn: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    #[serde(default)]
    items: Vec<ProjectItem>,
}

/// A project item resolved to the same shape as an OSC registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    pub id: String,
    pub label: String,
    /// `send`, `ctl` or `spawn`.
    pub kind: &'static str,
    /// Text, ctl string, or a JSON argv array.
    pub payload: String,
}

/// Parses a project menu file (`.microterm-menu.toml` or an alias). It can hold items and nothing else: in particular it can
/// never name trusted directories.
pub fn parse_project(text: &str) -> Result<Vec<ProjectEntry>, String> {
    let file: ProjectFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if file.items.len() > MAX_ITEMS {
        return Err(format!("more than {MAX_ITEMS} items"));
    }
    let mut out: Vec<ProjectEntry> = Vec::new();
    for (n, it) in file.items.into_iter().enumerate() {
        let at = format!("items[{n}]");
        let (kind, payload) = match (&it.send, &it.ctl, &it.spawn) {
            (Some(s), None, None) => ("send", s.clone()),
            (None, Some(c), None) => ("ctl", c.clone()),
            (None, None, Some(a)) if !a.is_empty() => ("spawn", serde_json_array(a)),
            _ => return Err(format!("{at}: set exactly one of send, ctl, spawn")),
        };
        let label: String = it.label.chars().filter(|c| !c.is_control()).collect();
        let label = label.trim().to_string();
        if label.is_empty() || label.len() > MAX_LABEL {
            return Err(format!("{at}: label must be 1..={MAX_LABEL} bytes"));
        }
        if payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return Err(format!("{at}: payload must be 1..={MAX_PAYLOAD} bytes"));
        }
        let id = it.id.unwrap_or_else(|| slug(&label));
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return Err(format!("{at}: id `{id}` must match [a-z0-9_-]{{1,64}}"));
        }
        if out.iter().any(|o| o.id == id) {
            return Err(format!("{at}: duplicate id `{id}`"));
        }
        out.push(ProjectEntry {
            id,
            label,
            kind,
            payload,
        });
    }
    Ok(out)
}

fn slug(label: &str) -> String {
    let mut s = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    s.trim_end_matches('-').chars().take(64).collect()
}

fn serde_json_array(a: &[String]) -> String {
    let mut out = String::from("[");
    for (i, s) in a.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sec(dirs: &[&Path]) -> MenuSecurity {
        MenuSecurity {
            trusted_dirs: dirs
                .iter()
                .map(|d| d.to_string_lossy().into_owned())
                .collect(),
            ..Default::default()
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mt_trust_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn exact_versus_subtree() {
        let root = tmp("exact");
        let sub = root.join("a/b");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(root.join("ab")).unwrap();
        let exact = sec(&[&root.join("a")]);
        assert!(is_trusted(&exact, &root.join("a")));
        assert!(
            !is_trusted(&exact, &sub),
            "exact entries do not cover children"
        );
        let mut tree = sec(&[]);
        tree.trusted_dirs = vec![format!("{}/**", root.join("a").display())];
        assert!(is_trusted(&tree, &root.join("a")));
        assert!(is_trusted(&tree, &sub));
        assert!(
            !is_trusted(&tree, &root.join("ab")),
            "component-wise, not textual"
        );
        assert!(!is_trusted(&tree, &root));
    }

    #[test]
    fn symlinks_are_resolved_on_both_sides() {
        let root = tmp("links");
        let real = root.join("real");
        let other = root.join("other");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&real, root.join("link")).unwrap();
        // A symlink inside a trusted tree that points outside is not trusted.
        std::os::unix::fs::symlink(&other, real.join("escape")).unwrap();
        let mut s = sec(&[]);
        s.trusted_dirs = vec![format!("{}/**", real.display())];
        assert!(!is_trusted(&s, &real.join("escape")));
        // Trusting through a link equals trusting its target; entering through a link works.
        let via = sec(&[&root.join("link")]);
        assert!(is_trusted(&via, &real));
        assert!(is_trusted(&sec(&[&real]), &root.join("link")));
    }

    #[test]
    fn bad_entries_trust_nothing() {
        let root = tmp("bad");
        for raw in ["rel/dir", "/a/../b", "/a/*/b", "", "/**", "/missing/dir/**"] {
            let mut s = sec(&[]);
            s.trusted_dirs = vec![raw.to_string()];
            assert!(!is_trusted(&s, &root), "{raw}");
        }
        assert!(parse_entry("~/x", Some(Path::new("/h"))).unwrap().dir == Path::new("/h/x"));
        assert!(parse_entry("~/x", None).is_err());
        assert!(!is_trusted(&sec(&[&root]), &root.join("missing")));
    }

    #[test]
    fn project_files_hold_items_only() {
        let ok = parse_project(
            r#"
[[items]]
label = "Run tests"
send = "make test"
[[items]]
id = "lint"
label = "Lint"
spawn = ["cargo", "clippy"]
"#,
        )
        .unwrap();
        assert_eq!(ok[0].id, "run-tests");
        assert_eq!(ok[1].payload, r#"["cargo","clippy"]"#);
        for bad in [
            "[menu_security]\ntrusted_dirs = [\"/\"]",
            "[[items]]\nlabel = \"x\"",
            "[[items]]\nlabel = \"x\"\nsend = \"a\"\nctl = \"b\"",
            "[[items]]\nlabel = \"x\"\nsend = \"a\"\nwhen = \"b\"",
            "[[items]]\nid = \"Bad\"\nlabel = \"x\"\nsend = \"a\"",
        ] {
            assert!(parse_project(bad).is_err(), "{bad}");
        }
    }
}
