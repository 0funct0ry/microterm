//! Surgical write-back of `[[workspaces]]` and `startup_workspace` (SPEC §8.5). No GPUI.
//!
//! Only changed, added or removed workspace entries and `startup_workspace` are touched; every
//! other byte of the file is preserved. Output is the nested array-of-tables form.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

use crate::{SplitKind, WorkspaceDef, WorkspacePane, WorkspaceTab};

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("config file does not parse: {0}")]
    Unparsable(String),
    #[error("config file changed on disk since the designer opened")]
    Stale,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// FNV-1a 64-bit content hash used for the staleness check and self-write detection.
pub fn content_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

static LAST_WRITTEN: Mutex<Option<u64>> = Mutex::new(None);

/// True when `text` is exactly what the designer last wrote, so the watcher's reload is a no-op.
pub fn is_self_write(text: &str) -> bool {
    *LAST_WRITTEN.lock().unwrap() == Some(content_hash(text))
}

#[derive(serde::Deserialize, Default)]
struct OnlyWorkspaces {
    #[serde(default)]
    workspaces: Vec<WorkspaceDef>,
}

/// Reads the declared workspaces of `src` (tolerating unrelated content).
pub fn read_workspaces(src: &str) -> Result<Vec<WorkspaceDef>, EditError> {
    toml::from_str::<OnlyWorkspaces>(src)
        .map(|o| o.workspaces)
        .map_err(|e| EditError::Unparsable(e.to_string()))
}

fn leaf_fields(t: &mut Table, cwd: &Option<String>, command: &[String]) {
    if let Some(c) = cwd {
        t["cwd"] = value(c.as_str());
    }
    if !command.is_empty() {
        let mut a = Array::new();
        for c in command {
            a.push(c.as_str());
        }
        t["command"] = value(a);
    }
}

fn split_fields(t: &mut Table, split: SplitKind, percent: Option<u8>, panes: &[WorkspacePane]) {
    t["split"] = value(match split {
        SplitKind::Horizontal => "horizontal",
        SplitKind::Vertical => "vertical",
    });
    if let Some(p) = percent {
        t["percent"] = value(p as i64);
    }
    let mut aot = ArrayOfTables::new();
    for p in panes {
        aot.push(pane_table(p));
    }
    t["panes"] = Item::ArrayOfTables(aot);
}

fn pane_table(p: &WorkspacePane) -> Table {
    let mut t = Table::new();
    leaf_fields(&mut t, &p.cwd, &p.command);
    if let Some(s) = p.split {
        split_fields(&mut t, s, p.percent, &p.panes);
    }
    t
}

fn tab_table(tab: &WorkspaceTab) -> Table {
    let mut t = Table::new();
    if let Some(title) = &tab.title {
        t["title"] = value(title.as_str());
    }
    leaf_fields(&mut t, &tab.cwd, &tab.command);
    if let Some(s) = tab.split {
        split_fields(&mut t, s, tab.percent, &tab.panes);
    }
    t
}

fn workspace_table(w: &WorkspaceDef) -> Table {
    let mut t = Table::new();
    t["name"] = value(w.name.as_str());
    if !w.tabs.is_empty() {
        let mut aot = ArrayOfTables::new();
        for tab in &w.tabs {
            aot.push(tab_table(tab));
        }
        t["tabs"] = Item::ArrayOfTables(aot);
    }
    t
}

/// Rewrites `src` so its declared workspaces equal `workspaces` and `startup_workspace` equals
/// `startup`, touching nothing else. Unchanged workspaces keep their exact bytes.
pub fn patch(src: &str, workspaces: &[WorkspaceDef], startup: &str) -> Result<String, EditError> {
    let old = read_workspaces(src)?;
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| EditError::Unparsable(e.to_string()))?;

    let existing: Vec<Table> = match doc.get("workspaces") {
        Some(Item::ArrayOfTables(a)) if a.len() == old.len() => a.iter().cloned().collect(),
        _ => Vec::new(),
    };
    let mut aot = ArrayOfTables::new();
    for w in workspaces {
        let reuse = old
            .iter()
            .position(|o| o == w)
            .and_then(|i| existing.get(i));
        aot.push(match reuse {
            Some(t) => t.clone(),
            None => workspace_table(w),
        });
    }
    let render = |doc: &DocumentMut| doc.to_string();
    if workspaces.is_empty() {
        doc.remove("workspaces");
    } else {
        doc["workspaces"] = Item::ArrayOfTables(aot);
    }
    let cur_startup = doc
        .get("startup_workspace")
        .and_then(|i| i.as_str())
        .map(str::to_string);
    match cur_startup {
        Some(s) if s == startup => {}
        None if startup == "default" => {}
        _ => doc["startup_workspace"] = value(startup),
    }
    let mut out = render(&doc);
    // Document order of array-of-tables can differ from the requested order; if so, rewrite all.
    if read_workspaces(&out)? != workspaces {
        let all: Vec<Table> = workspaces.iter().map(workspace_table).collect();
        let mut aot = ArrayOfTables::new();
        for t in all {
            aot.push(t);
        }
        doc.remove("workspaces");
        doc["workspaces"] = Item::ArrayOfTables(aot);
        out = render(&doc);
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Same,
    Added,
    Removed,
}

/// Line diff for the designer's TOML preview.
pub fn preview(old: &str, new: &str) -> Vec<(DiffTag, String)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push((DiffTag::Same, a[i].to_string()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push((DiffTag::Removed, a[i].to_string()));
            i += 1;
        } else {
            out.push((DiffTag::Added, b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| (DiffTag::Removed, l.to_string())));
    out.extend(b[j..].iter().map(|l| (DiffTag::Added, l.to_string())));
    out
}

/// Writes the patched file atomically. `opened_hash` is `content_hash` of the text the designer
/// loaded; a mismatch refuses the write. Symlinks are written through; mode is preserved and the
/// previous content is kept in `<file>.bak`.
pub fn save(
    path: &Path,
    opened_hash: u64,
    workspaces: &[WorkspaceDef],
    startup: &str,
) -> Result<String, EditError> {
    let target = fs::canonicalize(path)?;
    let src = fs::read_to_string(&target)?;
    if content_hash(&src) != opened_hash {
        return Err(EditError::Stale);
    }
    let out = patch(&src, workspaces, startup)?;
    let mut bak = target.clone().into_os_string();
    bak.push(".bak");
    fs::write(&bak, &src)?;
    let mut tmp = target.clone().into_os_string();
    tmp.push(".tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(out.as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::metadata(&target)?.permissions())?;
    *LAST_WRITTEN.lock().unwrap() = Some(content_hash(&out));
    fs::rename(&tmp, &target)?;
    Ok(out)
}
