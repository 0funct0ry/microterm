//! Small pieces of state persisted between runs in `$XDG_STATE_HOME/microterm/` (SPEC §8.1):
//! currently the last window size (`window.remember_size`).

use std::path::{Path, PathBuf};

/// `$XDG_STATE_HOME/microterm`, else `~/.local/state/microterm`.
pub fn state_dir() -> Option<PathBuf> {
    state_dir_from(
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

pub(crate) fn state_dir_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = match xdg.filter(|p| p.is_absolute()) {
        Some(x) => x,
        None => home?.join(".local").join("state"),
    };
    Some(base.join("microterm"))
}

const WINDOW_FILE: &str = "window-size";

/// Last saved window size in logical pixels.
pub fn load_window_size(dir: &Path) -> Option<(f32, f32)> {
    let text = std::fs::read_to_string(dir.join(WINDOW_FILE)).ok()?;
    let (w, h) = text.trim().split_once('x')?;
    let (w, h) = (w.parse::<f32>().ok()?, h.parse::<f32>().ok()?);
    (w >= 100.0 && h >= 100.0 && w < 20_000.0 && h < 20_000.0).then_some((w, h))
}

pub fn save_window_size(dir: &Path, width: f32, height: f32) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{WINDOW_FILE}.tmp"));
    std::fs::write(&tmp, format!("{}x{}", width.round(), height.round()))?;
    std::fs::rename(tmp, dir.join(WINDOW_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mt-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn state_dir_prefers_xdg_then_home() {
        assert_eq!(
            state_dir_from(Some("/x".into()), Some("/h".into())),
            Some("/x/microterm".into())
        );
        assert_eq!(
            state_dir_from(None, Some("/h".into())),
            Some("/h/.local/state/microterm".into())
        );
        assert_eq!(
            state_dir_from(Some("relative".into()), Some("/h".into())),
            Some("/h/.local/state/microterm".into())
        );
        assert_eq!(state_dir_from(None, None), None);
    }

    #[test]
    fn window_size_round_trips() {
        let d = temp("rt");
        assert_eq!(load_window_size(&d), None);
        save_window_size(&d, 1234.4, 777.6).unwrap();
        assert_eq!(load_window_size(&d), Some((1234.0, 778.0)));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn garbage_is_ignored() {
        let d = temp("bad");
        std::fs::create_dir_all(&d).unwrap();
        for bad in ["", "abc", "10x10", "99999x500", "1200"] {
            std::fs::write(d.join(WINDOW_FILE), bad).unwrap();
            assert_eq!(load_window_size(&d), None, "{bad:?}");
        }
        let _ = std::fs::remove_dir_all(d);
    }
}
