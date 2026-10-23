//! The recordings store (SPEC §10.10.4): `<dir>/<id>.cast` plus a rebuildable `<id>.json`
//! sidecar. No GPUI, no terminal emulator. Nothing here touches the disk until it is asked to
//! (`list` on a missing directory is empty and creates nothing).

use std::fs;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{EventKind, Reader};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("recording not found: {0}")]
    NotFound(String),
    #[error("invalid recording id: {0}")]
    BadId(String),
    #[error("{0}")]
    Format(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Origin {
    /// `"local"`, `"ssh"` or `"serial"`.
    pub kind: String,
    pub name: String,
    pub cwd: String,
    pub process: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Seconds since the recording started.
    pub time: f64,
    pub label: String,
}

/// Sidecar metadata. Everything except `title`, `theme`, `font` and `origin` can be rebuilt
/// from the `.cast` alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidecar {
    pub id: String,
    pub title: String,
    /// Unix seconds (UTC).
    pub created_at: u64,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub cols: u16,
    pub rows: u16,
    pub has_input: bool,
    /// `"none"`, `"no_echo"` or `"n/a"`.
    pub input_redacted: String,
    pub origin: Origin,
    pub theme: String,
    pub font: String,
    pub markers: Vec<Marker>,
    pub app_version: String,
    pub recovered: bool,
}

impl Default for Sidecar {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            created_at: 0,
            duration_ms: 0,
            size_bytes: 0,
            cols: 80,
            rows: 24,
            has_input: false,
            input_redacted: "none".into(),
            origin: Origin::default(),
            theme: String::new(),
            font: String::new(),
            markers: Vec::new(),
            app_version: String::new(),
            recovered: false,
        }
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `(yyyymmdd, hhmmss)` in UTC.
fn stamp(unix: u64) -> (String, String) {
    let (y, m, d) = civil((unix / 86_400) as i64);
    let s = unix % 86_400;
    (
        format!("{y:04}{m:02}{d:02}"),
        format!("{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60),
    )
}

/// `2026-10-22 14:15:03` (UTC), used in default titles.
pub fn display_time(unix: u64) -> String {
    let (y, m, d) = civil((unix / 86_400) as i64);
    let s = unix % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

fn random_hex6() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:06x}", h.finish() & 0xff_ffff)
}

/// `r_<UTC yyyymmdd_hhmmss>_<6 hex>`.
pub fn new_id(unix: u64) -> String {
    let (d, t) = stamp(unix);
    format!("r_{d}_{t}_{}", random_hex6())
}

/// Ids become file names, so only the generated shape (and hand-dropped `.cast` stems made of
/// the same safe characters) is accepted.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `$XDG_DATA_HOME/microterm/recordings` when `XDG_DATA_HOME` is absolute, otherwise
    /// `~/.local/share/microterm/recordings` (also on macOS).
    pub fn default_dir(xdg_data_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
        let base = match xdg_data_home {
            Some(x) if Path::new(x).is_absolute() => PathBuf::from(x),
            _ => PathBuf::from(home.filter(|h| !h.is_empty())?)
                .join(".local")
                .join("share"),
        };
        Some(base.join("microterm").join("recordings"))
    }

    pub fn from_env() -> Option<Self> {
        let xdg = std::env::var("XDG_DATA_HOME").ok();
        let home = std::env::var("HOME").ok();
        Self::default_dir(xdg.as_deref(), home.as_deref()).map(Self::new)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn cast_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.cast"))
    }

    pub fn partial_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.cast.partial"))
    }

    pub fn sidecar_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Creates the directory `0700` (first recording only).
    pub fn ensure_dir(&self) -> io::Result<()> {
        if self.dir.is_dir() {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    pub fn write_sidecar(&self, sc: &Sidecar) -> Result<(), StoreError> {
        let mut json = serde_json::to_vec_pretty(sc).map_err(io::Error::from)?;
        json.push(b'\n');
        mt_config::atomic::write_bytes_atomic(&self.sidecar_path(&sc.id), &json, 0o600)?;
        Ok(())
    }

    /// Builds a sidecar from a `.cast` file alone (SPEC §10.10.4).
    pub fn sidecar_from_cast(&self, id: &str) -> Result<Sidecar, StoreError> {
        let path = self.cast_path(id);
        let meta = fs::metadata(&path)?;
        let mut reader = Reader::new(BufReader::new(fs::File::open(&path)?))
            .map_err(|e| StoreError::Format(e.to_string()))?;
        let h = reader.header().clone();
        let mut sc = Sidecar {
            id: id.to_string(),
            cols: h.width,
            rows: h.height,
            size_bytes: meta.len(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            ..Sidecar::default()
        };
        sc.created_at = h.timestamp.unwrap_or_else(|| {
            meta.modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs())
        });
        sc.title = h
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Recording — {}", display_time(sc.created_at)));
        sc.origin.kind = "local".into();
        let mut last = 0.0f64;
        for ev in reader.by_ref() {
            // A damaged tail must not hide the recording; keep what parsed.
            let Ok(ev) = ev else { break };
            last = last.max(ev.time);
            match ev.kind {
                EventKind::Input(_) => sc.has_input = true,
                EventKind::Marker(label) => sc.markers.push(Marker {
                    time: ev.time,
                    label,
                }),
                _ => {}
            }
        }
        sc.duration_ms = (last * 1000.0).round() as u64;
        if sc.has_input {
            sc.input_redacted = "n/a".into();
        }
        Ok(sc)
    }

    /// Reads the sidecar, regenerating (and rewriting) it when missing or corrupt.
    pub fn load(&self, id: &str) -> Result<Sidecar, StoreError> {
        if !valid_id(id) {
            return Err(StoreError::BadId(id.into()));
        }
        if !self.cast_path(id).is_file() {
            return Err(StoreError::NotFound(id.into()));
        }
        let parsed = fs::read(self.sidecar_path(id))
            .ok()
            .and_then(|b| serde_json::from_slice::<Sidecar>(&b).ok())
            .filter(|s| s.id == id);
        if let Some(mut sc) = parsed {
            // The `.cast` is the source of truth for size.
            if let Ok(m) = fs::metadata(self.cast_path(id)) {
                sc.size_bytes = m.len();
            }
            return Ok(sc);
        }
        let sc = self.sidecar_from_cast(id)?;
        let _ = self.write_sidecar(&sc);
        Ok(sc)
    }

    /// Completes every `.cast.partial` left by a crash (SPEC §10.10.2), then lists the store
    /// newest first. A missing directory is an empty store and is not created.
    pub fn list(&self) -> Result<Vec<Sidecar>, StoreError> {
        self.list_skipping(&[])
    }

    /// [`list`](Self::list) that leaves the ids in `running` alone: their `.partial` files are
    /// live recordings, not crash leftovers.
    pub fn list_skipping(&self, running: &[String]) -> Result<Vec<Sidecar>, StoreError> {
        let rd = match fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let names: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        for n in &names {
            if let Some(id) = n.strip_suffix(".cast.partial") {
                if valid_id(id) && !running.iter().any(|r| r == id) {
                    let _ = self.recover(id);
                }
            }
        }
        let mut out = Vec::new();
        let mut ids: Vec<String> = fs::read_dir(&self.dir)?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".cast").map(str::to_string))
            .filter(|id| valid_id(id))
            .collect();
        ids.sort();
        for id in ids {
            if let Ok(sc) = self.load(&id) {
                out.push(sc);
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(out)
    }

    /// Turns `<id>.cast.partial` into `<id>.cast`: a truncated last line is dropped and the
    /// sidecar is marked `recovered`. A partial with no usable header is deleted.
    pub fn recover(&self, id: &str) -> Result<Option<Sidecar>, StoreError> {
        let partial = self.partial_path(id);
        let mut data = fs::read(&partial)?;
        let keep = data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        data.truncate(keep);
        // A complete-looking last line can still be garbage if the flush was torn.
        loop {
            let start = data[..data.len().saturating_sub(1)]
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1);
            if start == 0 || data.is_empty() {
                break;
            }
            if serde_json::from_slice::<serde_json::Value>(&data[start..]).is_ok() {
                break;
            }
            data.truncate(start);
        }
        let ok = std::str::from_utf8(&data)
            .ok()
            .and_then(|s| s.lines().next())
            .is_some_and(|l| serde_json::from_str::<crate::Header>(l).is_ok());
        if !ok {
            let _ = fs::remove_file(&partial);
            return Ok(None);
        }
        let cast = self.cast_path(id);
        mt_config::atomic::write_bytes_atomic(&cast, &data, 0o600)?;
        fs::remove_file(&partial)?;
        let mut sc = self.sidecar_from_cast(id)?;
        sc.recovered = true;
        self.write_sidecar(&sc)?;
        Ok(Some(sc))
    }

    /// Sets the sidecar title and the `title` in the cast header (SPEC §10.10.5), so an
    /// exported or shared copy carries the new name.
    pub fn rename(&self, id: &str, title: &str) -> Result<Sidecar, StoreError> {
        let mut sc = self.load(id)?;
        self.rewrite_header_title(id, title)?;
        sc.title = title.to_string();
        sc.size_bytes = fs::metadata(self.cast_path(id)).map_or(sc.size_bytes, |m| m.len());
        self.write_sidecar(&sc)?;
        Ok(sc)
    }

    fn rewrite_header_title(&self, id: &str, title: &str) -> Result<(), StoreError> {
        let path = self.cast_path(id);
        let data = fs::read(&path)?;
        let nl = data.iter().position(|b| *b == b'\n').unwrap_or(data.len());
        let mut header: serde_json::Value = serde_json::from_slice(&data[..nl])
            .map_err(|e| StoreError::Format(format!("bad cast header: {e}")))?;
        let obj = header
            .as_object_mut()
            .ok_or_else(|| StoreError::Format("cast header is not an object".into()))?;
        obj.insert("title".into(), title.into());
        let mut out = serde_json::to_vec(&header).map_err(io::Error::from)?;
        out.extend_from_slice(&data[nl..]);
        mt_config::atomic::write_bytes_atomic(&path, &out, 0o600)?;
        Ok(())
    }

    /// Sum of the `.cast` sizes, for the panel header.
    pub fn total_size(&self) -> u64 {
        fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".cast"))
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    }

    /// Copies an asciicast file into the store with a regenerated sidecar. The file stem is
    /// kept as the id when it is safe and free, otherwise a fresh id is generated.
    pub fn import(&self, src: &Path) -> Result<Sidecar, StoreError> {
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        self.ensure_dir()?;
        let id = if valid_id(&stem) && !self.cast_path(&stem).exists() {
            stem
        } else {
            new_id(unix_now())
        };
        let dest = self.cast_path(&id);
        fs::copy(src, &dest)?;
        match self.sidecar_from_cast(&id) {
            Ok(sc) => {
                self.write_sidecar(&sc)?;
                Ok(sc)
            }
            Err(e) => {
                let _ = fs::remove_file(&dest);
                Err(e)
            }
        }
    }

    /// Removes both files. No trash.
    pub fn remove(&self, id: &str) -> Result<(), StoreError> {
        if !valid_id(id) {
            return Err(StoreError::BadId(id.into()));
        }
        if !self.cast_path(id).is_file() {
            return Err(StoreError::NotFound(id.into()));
        }
        fs::remove_file(self.cast_path(id))?;
        match fs::remove_file(self.sidecar_path(id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mtrec-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn id_shape_and_civil_dates() {
        assert_eq!(stamp(0), ("19700101".into(), "000000".into()));
        // 2026-10-22 14:15:03 UTC
        assert_eq!(stamp(1_792_678_503), ("20261022".into(), "141503".into()));
        let id = new_id(1_792_678_503);
        assert!(id.starts_with("r_20261022_141503_"), "{id}");
        assert_eq!(id.len(), "r_20261022_141503_".len() + 6);
        assert!(valid_id(&id));
        assert!(!valid_id("../x") && !valid_id("a/b") && !valid_id(""));
    }

    #[test]
    fn default_dir_resolution() {
        let d = Store::default_dir(Some("/x/data"), Some("/h")).unwrap();
        assert_eq!(d, PathBuf::from("/x/data/microterm/recordings"));
        let d = Store::default_dir(Some("rel"), Some("/h")).unwrap();
        assert_eq!(d, PathBuf::from("/h/.local/share/microterm/recordings"));
        assert!(Store::default_dir(None, None).is_none());
    }

    #[test]
    fn missing_dir_lists_empty_and_creates_nothing() {
        let d = tmp("missing");
        let s = Store::new(&d);
        assert!(s.list().unwrap().is_empty());
        assert!(!d.exists());
    }

    #[test]
    fn bare_cast_regenerates_sidecar_and_corrupt_sidecar_is_rebuilt() {
        let d = tmp("regen");
        let s = Store::new(&d);
        s.ensure_dir().unwrap();
        let cast = "{\"version\":2,\"width\":100,\"height\":30,\"timestamp\":1792678503}\n\
                    [0.5,\"o\",\"hi\"]\n[1.25,\"i\",\"x\"]\n[2.0,\"m\",\"mark 1\"]\n";
        fs::write(d.join("r_a.cast"), cast).unwrap();
        let l = s.list().unwrap();
        assert_eq!(l.len(), 1);
        let sc = &l[0];
        assert_eq!((sc.cols, sc.rows, sc.duration_ms), (100, 30, 2000));
        assert!(sc.has_input);
        assert_eq!(sc.markers.len(), 1);
        assert!(d.join("r_a.json").is_file());
        fs::write(d.join("r_a.json"), "{ not json").unwrap();
        assert_eq!(s.load("r_a").unwrap().cols, 100);
        assert!(s.rename("r_a", "T").unwrap().title == "T");
        assert_eq!(s.load("r_a").unwrap().title, "T");
        s.remove("r_a").unwrap();
        assert!(matches!(s.load("r_a"), Err(StoreError::NotFound(_))));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rename_rewrites_the_cast_header_title() {
        let d = tmp("rename-header");
        let s = Store::new(&d);
        s.ensure_dir().unwrap();
        fs::write(
            d.join("r_h.cast"),
            "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hi\"]\n",
        )
        .unwrap();
        s.rename("r_h", "Demo").unwrap();
        let text = fs::read_to_string(d.join("r_h.cast")).unwrap();
        assert!(text.lines().next().unwrap().contains("\"title\":\"Demo\""));
        assert!(text.contains("[0.5,\"o\",\"hi\"]"));
        assert_eq!(s.load("r_h").unwrap().title, "Demo");
        // The cast alone now rebuilds the same title.
        assert_eq!(s.sidecar_from_cast("r_h").unwrap().title, "Demo");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn import_copies_with_a_fresh_sidecar_and_total_size_sums() {
        let d = tmp("import");
        let src = tmp("import-src");
        fs::create_dir_all(&src).unwrap();
        let s = Store::new(&d);
        let cast = "{\"version\":2,\"width\":90,\"height\":20}\n[1.0,\"o\",\"x\"]\n";
        fs::write(src.join("mine.cast"), cast).unwrap();
        let sc = s.import(&src.join("mine.cast")).unwrap();
        assert_eq!((sc.id.as_str(), sc.cols), ("mine", 90));
        assert!(d.join("mine.json").is_file());
        // The same stem again gets a generated id instead of overwriting.
        let again = s.import(&src.join("mine.cast")).unwrap();
        assert!(again.id.starts_with("r_"), "{}", again.id);
        assert_eq!(s.total_size(), 2 * cast.len() as u64);
        // A non-cast file is rejected and leaves nothing behind.
        fs::write(src.join("bad.cast"), "nope").unwrap();
        assert!(s.import(&src.join("bad.cast")).is_err());
        assert!(!d.join("bad.cast").exists());
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(&src);
    }

    #[test]
    fn partial_is_recovered_with_truncated_line_dropped() {
        let d = tmp("partial");
        let s = Store::new(&d);
        s.ensure_dir().unwrap();
        let p = "{\"version\":2,\"width\":80,\"height\":24}\n[0.1,\"o\",\"ok\"]\n[0.2,\"o\",\"tru";
        fs::write(d.join("r_b.cast.partial"), p).unwrap();
        let l = s.list().unwrap();
        assert_eq!(l.len(), 1);
        assert!(l[0].recovered);
        assert!(!d.join("r_b.cast.partial").exists());
        let text = fs::read_to_string(d.join("r_b.cast")).unwrap();
        assert_eq!(text.lines().count(), 2);
        // A header-less partial disappears instead of poisoning the store.
        fs::write(d.join("r_c.cast.partial"), "{\"vers").unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        assert!(!d.join("r_c.cast.partial").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("perm").join("rec");
        let s = Store::new(&d);
        s.ensure_dir().unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&d), 0o700);
        s.write_sidecar(&Sidecar {
            id: "r_p".into(),
            ..Sidecar::default()
        })
        .unwrap();
        assert_eq!(mode(&d.join("r_p.json")), 0o600);
        let _ = fs::remove_dir_all(d.parent().unwrap());
    }
}
