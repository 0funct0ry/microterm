//! The captures directory (SPEC §10.11.7): where `snapshot` and `screenshot` write, and nowhere
//! else. File names are generated from the rule id and the clock, never from user or terminal
//! text, so path traversal and overwriting do not exist.
//!
//! * the directory is created `0700`; a symlink, a non-directory or a directory owned by someone
//!   else is refused (`unsafe captures dir`);
//! * files are created `0600` with `O_CREAT|O_EXCL|O_NOFOLLOW` and never overwrite anything;
//! * per-file, file-count and total-size caps apply, and rotation deletes **only** files that
//!   match our own naming pattern, oldest first.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest text file in bytes (longer text is cut).
    pub max_text_bytes: usize,
    pub max_files: usize,
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_text_bytes: 256 * 1024,
            max_files: 200,
            max_total_bytes: 100 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext {
    Txt,
    Png,
}

impl Ext {
    fn as_str(self) -> &'static str {
        match self {
            Ext::Txt => "txt",
            Ext::Png => "png",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CaptureError {
    /// The directory is a symlink, not a directory, or not owned by the user.
    #[error("unsafe captures dir")]
    UnsafeDir,
    #[error("invalid rule id")]
    BadRuleId,
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for CaptureError {
    fn from(e: std::io::Error) -> Self {
        CaptureError::Io(e.to_string())
    }
}

/// `$XDG_DATA_HOME/microterm/captures`, `~/.local/share/microterm/captures` when unset (on
/// macOS too, as in D13). `None` without `$HOME`.
pub fn default_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(x) => PathBuf::from(x),
        None => {
            PathBuf::from(std::env::var_os("HOME").filter(|v| !v.is_empty())?).join(".local/share")
        }
    };
    Some(base.join("microterm").join("captures"))
}

/// `YYYYMMDD-HHMMSS` in UTC for a Unix time in seconds.
pub fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn valid_rule_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether `name` is one of ours: `<rule-id>-<YYYYMMDD>-<HHMMSS>-<seq>.(txt|png)`.
pub fn is_ours(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext != "txt" && ext != "png" {
        return false;
    }
    let mut parts = stem.rsplitn(4, '-');
    let (seq, time, date, id) = (parts.next(), parts.next(), parts.next(), parts.next());
    matches!((seq, time, date, id), (Some(s), Some(t), Some(d), Some(i))
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
            && t.len() == 6 && t.bytes().all(|b| b.is_ascii_digit())
            && d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit())
            && valid_rule_id(i))
}

static SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct CaptureDir {
    dir: PathBuf,
    limits: Limits,
}

impl CaptureDir {
    /// Opens (creating `0700` when missing) the captures directory and checks it is safe.
    pub fn open(dir: &Path, limits: Limits) -> Result<CaptureDir, CaptureError> {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::symlink_metadata(dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                DirBuilder::new().mode(0o700).create(dir)?;
            }
            Err(e) => return Err(e.into()),
        }
        let md = fs::symlink_metadata(dir)?;
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if md.file_type().is_symlink() || !md.is_dir() || md.uid() != me {
            return Err(CaptureError::UnsafeDir);
        }
        if md.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(CaptureDir {
            dir: dir.to_path_buf(),
            limits,
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Writes `bytes` as a new capture of rule `rule_id` and returns its path. Text is cut to
    /// the per-file cap on a character boundary. Rotation runs afterwards.
    pub fn write(
        &self,
        rule_id: &str,
        ext: Ext,
        bytes: &[u8],
        now_secs: u64,
    ) -> Result<PathBuf, CaptureError> {
        if !valid_rule_id(rule_id) {
            return Err(CaptureError::BadRuleId);
        }
        let mut data = bytes;
        if ext == Ext::Txt && data.len() > self.limits.max_text_bytes {
            let mut cut = self.limits.max_text_bytes;
            while cut > 0 && std::str::from_utf8(&data[..cut]).is_err() {
                cut -= 1;
            }
            data = &data[..cut];
        }
        let stamp = utc_stamp(now_secs);
        let mut last = None;
        for _ in 0..64 {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = self
                .dir
                .join(format!("{rule_id}-{stamp}-{seq}.{}", ext.as_str()));
            match create_new(&path) {
                Ok(mut f) => {
                    f.write_all(data)?;
                    f.sync_all().ok();
                    drop(f);
                    self.rotate(&path);
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e.into()),
            }
        }
        Err(last.map_or(CaptureError::Io("no free name".into()), Into::into))
    }

    /// Deletes the oldest of our own files while the count or total size is over the caps.
    /// `keep` (the file just written) is never removed.
    fn rotate(&self, keep: &Path) {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = rd
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(is_ours))
            .filter_map(|e| {
                let md = fs::symlink_metadata(e.path()).ok()?;
                md.file_type().is_file().then(|| {
                    (
                        md.modified().unwrap_or(std::time::UNIX_EPOCH),
                        e.path(),
                        md.len(),
                    )
                })
            })
            .collect();
        files.sort();
        let mut count = files.len();
        let mut total: u64 = files.iter().map(|f| f.2).sum();
        for (_, path, len) in files {
            if count <= self.limits.max_files && total <= self.limits.max_total_bytes {
                break;
            }
            if path == keep {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                count -= 1;
                total -= len;
            }
        }
    }
}

/// `O_CREAT|O_EXCL|O_NOFOLLOW`, mode `0600`.
fn create_new(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mt_cap_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn open(d: &Path, l: Limits) -> CaptureDir {
        CaptureDir::open(&d.join("captures"), l).unwrap()
    }

    #[test]
    fn utc_stamps() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(951_782_400), "20000229-000000");
        assert_eq!(utc_stamp(1_792_859_604), "20261024-163324");
        assert_eq!(utc_stamp(4_102_444_799), "20991231-235959");
    }

    #[test]
    fn our_names_and_only_ours_match() {
        for ok in [
            "t_3f9a1c-20261024-163324-7.txt",
            "a-20000101-000000-12345.png",
            "x_y-20261024-163324-1.txt",
        ] {
            assert!(is_ours(ok), "{ok}");
        }
        for no in [
            "notes.txt",
            "README",
            "t_3f9a1c-20261024-163324.txt",
            "t_3f9a1c-2026102-163324-1.txt",
            "t_3f9a1c-20261024-163324-x.txt",
            "t_3f9a1c-20261024-163324-1.sh",
            "../t-20261024-163324-1.txt",
            "a b-20261024-163324-1.txt",
            "-20261024-163324-1.txt",
        ] {
            assert!(!is_ours(no), "{no}");
        }
    }

    #[test]
    fn names_are_generated_modes_are_private_and_nothing_is_overwritten() {
        let d = tmp("modes");
        let c = open(&d, Limits::default());
        let a = c
            .write("t_abc123", Ext::Txt, b"one", 1_792_859_604)
            .unwrap();
        let b = c
            .write("t_abc123", Ext::Txt, b"two", 1_792_859_604)
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read(&a).unwrap(), b"one");
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("t_abc123-20261024-163324-") && is_ours(name),
            "{name}"
        );
        assert_eq!(
            fs::metadata(&a).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(c.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let p = c.write("t_abc123", Ext::Png, &[0x89, b'P'], 1).unwrap();
        assert!(p.extension().is_some_and(|e| e == "png"));
    }

    #[test]
    fn rule_ids_cannot_carry_a_path() {
        let d = tmp("ids");
        let c = open(&d, Limits::default());
        for bad in ["", "../x", "a/b", "a b", ".hidden", "x\0y", &"y".repeat(40)] {
            assert_eq!(
                c.write(bad, Ext::Txt, b"x", 0).unwrap_err(),
                CaptureError::BadRuleId,
                "{bad:?}"
            );
        }
        assert_eq!(fs::read_dir(c.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_symlink_planted_at_a_candidate_name_is_never_followed() {
        let d = tmp("nofollow");
        let c = open(&d, Limits::default());
        let victim = d.join("victim.txt");
        fs::write(&victim, "precious").unwrap();
        // Plant symlinks at the next several candidate names of this process.
        let next = SEQ.load(Ordering::Relaxed);
        let stamp = utc_stamp(5);
        for seq in next..next + 4 {
            symlink(&victim, c.path().join(format!("t_x-{stamp}-{seq}.txt"))).unwrap();
        }
        let out = c.write("t_x", Ext::Txt, b"attack", 5).unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        assert!(!fs::symlink_metadata(&out).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&out).unwrap(), b"attack");
    }

    #[test]
    fn a_symlinked_or_foreign_directory_is_refused() {
        let d = tmp("unsafe");
        let real = d.join("real");
        fs::create_dir(&real).unwrap();
        symlink(&real, d.join("link")).unwrap();
        assert_eq!(
            CaptureDir::open(&d.join("link"), Limits::default()).unwrap_err(),
            CaptureError::UnsafeDir
        );
        fs::write(d.join("file"), "x").unwrap();
        assert_eq!(
            CaptureDir::open(&d.join("file"), Limits::default()).unwrap_err(),
            CaptureError::UnsafeDir
        );
        // An existing directory of ours with loose permissions is tightened.
        let loose = d.join("loose");
        fs::create_dir(&loose).unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
        CaptureDir::open(&loose, Limits::default()).unwrap();
        assert_eq!(
            fs::metadata(&loose).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn text_is_cut_on_a_character_boundary() {
        let d = tmp("cut");
        let c = open(
            &d,
            Limits {
                max_text_bytes: 10,
                ..Limits::default()
            },
        );
        let p = c.write("t_x", Ext::Txt, "ééééééééé".as_bytes(), 0).unwrap();
        let text = fs::read_to_string(p).unwrap();
        assert_eq!(text, "ééééé");
    }

    #[test]
    fn rotation_deletes_only_our_oldest_files() {
        let d = tmp("rotate");
        let c = open(
            &d,
            Limits {
                max_files: 3,
                ..Limits::default()
            },
        );
        fs::write(c.path().join("notes.txt"), "mine").unwrap();
        fs::write(c.path().join("t_x-20000101-000000-1.sh"), "not ours").unwrap();
        let mut written = Vec::new();
        for i in 0..6u64 {
            written.push(
                c.write("t_x", Ext::Txt, format!("{i}").as_bytes(), 100 + i)
                    .unwrap(),
            );
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let ours: Vec<_> = fs::read_dir(c.path())
            .unwrap()
            .flatten()
            .filter(|e| is_ours(e.file_name().to_str().unwrap()))
            .collect();
        assert_eq!(ours.len(), 3);
        assert!(!written[0].exists() && !written[2].exists() && written[5].exists());
        assert!(c.path().join("notes.txt").exists());
        assert!(c.path().join("t_x-20000101-000000-1.sh").exists());
    }

    #[test]
    fn total_size_cap_rotates_and_never_removes_the_new_file() {
        let d = tmp("total");
        let c = open(
            &d,
            Limits {
                max_total_bytes: 25,
                ..Limits::default()
            },
        );
        for i in 0..4u64 {
            c.write("t_x", Ext::Txt, &[b'x'; 10], i).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let size: u64 = fs::read_dir(c.path())
            .unwrap()
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .sum();
        assert!(size <= 25, "{size}");
        // A single file bigger than the whole cap is kept (it is the one just written).
        let big = c.write("t_big", Ext::Png, &[0u8; 100], 9).unwrap();
        assert!(big.exists());
    }

    #[test]
    fn default_dir_follows_xdg_then_home() {
        // Only the shape is checked (the environment is process-wide).
        let p = default_dir().unwrap();
        assert!(p.ends_with("microterm/captures"));
    }
}
