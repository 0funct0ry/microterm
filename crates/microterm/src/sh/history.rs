//! Where the `sh` history lives: 0600 under the state directory.

use std::path::PathBuf;

/// `$XDG_STATE_HOME/microterm`, else `~/Library/Application Support/microterm` on macOS and
/// `~/.local/state/microterm` elsewhere.
pub fn history_path() -> Option<PathBuf> {
    history_path_from(
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        cfg!(target_os = "macos"),
    )
}

fn history_path_from(xdg: Option<PathBuf>, home: Option<PathBuf>, macos: bool) -> Option<PathBuf> {
    let dir = match xdg.filter(|p| p.is_absolute()) {
        Some(x) => x.join("microterm"),
        None if macos => home?
            .join("Library")
            .join("Application Support")
            .join("microterm"),
        None => home?.join(".local").join("state").join("microterm"),
    };
    Some(dir.join("sh_history"))
}

/// Creates the history file (and its directory) with mode 0600 if it is missing, and tightens
/// the mode of an existing one.
pub fn prepare(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_follows_xdg_then_the_platform_default() {
        let home = Some(PathBuf::from("/h"));
        assert_eq!(
            history_path_from(Some("/x".into()), home.clone(), true),
            Some("/x/microterm/sh_history".into())
        );
        assert_eq!(
            history_path_from(None, home.clone(), true),
            Some("/h/Library/Application Support/microterm/sh_history".into())
        );
        assert_eq!(
            history_path_from(None, home.clone(), false),
            Some("/h/.local/state/microterm/sh_history".into())
        );
        assert_eq!(
            history_path_from(Some("relative".into()), home, false),
            Some("/h/.local/state/microterm/sh_history".into())
        );
        assert_eq!(history_path_from(None, None, false), None);
    }

    #[test]
    fn the_file_is_created_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mt-sh-history-{}", std::process::id()));
        let path = dir.join("sh_history");
        prepare(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }
}
