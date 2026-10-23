//! Atomic, staleness-checked file write-back shared by every file microterm writes
//! (`microterm.toml` workspaces, `snippets.toml`; SPEC §8.5, §10.8.2). No GPUI.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::edit::{content_hash, EditError};

static LAST_WRITTEN: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

fn key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// True when `text` is exactly what we last wrote to `path`, so the watcher's reload is a no-op.
pub fn is_self_write_at(path: &Path, text: &str) -> bool {
    LAST_WRITTEN
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&key(path)))
        .is_some_and(|h| *h == content_hash(text))
}

/// True when `text` is what we last wrote to any file.
pub fn is_self_write_any(text: &str) -> bool {
    let h = content_hash(text);
    LAST_WRITTEN
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|m| m.values().any(|v| *v == h))
}

/// Writes `edit(current_text)` to `path` atomically.
///
/// - `opened_hash` is `content_hash` of the text the caller loaded; a mismatch is `Stale`.
///   `None` skips the check. A missing file reads as the empty string and is created.
/// - Symlinks are written through, the mode is preserved, the previous content is kept in
///   `<file>.bak`, and the write is temp file + fsync + rename.
pub fn write_atomic(
    path: &Path,
    opened_hash: Option<u64>,
    edit: impl FnOnce(&str) -> Result<String, EditError>,
) -> Result<String, EditError> {
    let existing = fs::canonicalize(path).ok();
    let target = existing.clone().unwrap_or_else(|| path.to_path_buf());
    let src = match &existing {
        Some(t) => fs::read_to_string(t)?,
        None => String::new(),
    };
    if opened_hash.is_some_and(|h| content_hash(&src) != h) {
        return Err(EditError::Stale);
    }
    let out = edit(&src)?;
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    if existing.is_some() {
        let mut bak = target.clone().into_os_string();
        bak.push(".bak");
        fs::write(&bak, &src)?;
    }
    let mut tmp = target.clone().into_os_string();
    tmp.push(".tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(out.as_bytes())?;
        f.sync_all()?;
    }
    if existing.is_some() {
        fs::set_permissions(&tmp, fs::metadata(&target)?.permissions())?;
    }
    LAST_WRITTEN
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(key(&target), content_hash(&out));
    fs::rename(&tmp, &target)?;
    Ok(out)
}

/// Writes `bytes` to `path` atomically (temp file + fsync + rename) with `mode` on Unix.
/// Unlike `write_atomic` it keeps no `.bak` and does no staleness check; it is for
/// machine-written files such as recording sidecars (SPEC §10.10.4).
pub fn write_bytes_atomic(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.to_path_buf().into_os_string();
    tmp.push(".tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    fs::rename(&tmp, path)
}
