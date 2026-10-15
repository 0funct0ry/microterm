//! Opening the config for editing: `$VISUAL` / `$EDITOR`, falling back to the system opener.

use std::io;
use std::path::Path;

use crate::template::DEFAULT_TOML;

/// Create the config file from the commented template if it does not exist yet.
/// Returns `true` when the file was created.
pub fn ensure_config_file(path: &Path) -> io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, DEFAULT_TOML)?;
    Ok(true)
}

/// argv for a terminal editor, from `$VISUAL` then `$EDITOR` (split on whitespace, so
/// `EDITOR="code -w"` works). `None` when neither is set.
pub fn editor_argv(visual: Option<&str>, editor: Option<&str>, path: &Path) -> Option<Vec<String>> {
    let chosen = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())?;
    let mut argv: Vec<String> = chosen.split_whitespace().map(String::from).collect();
    argv.push(path.to_string_lossy().into_owned());
    Some(argv)
}

/// argv for the platform's default-application opener.
pub fn opener_argv(path: &Path) -> Vec<String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    vec![program.into(), path.to_string_lossy().into_owned()]
}

/// The editor argv from the process environment.
pub fn editor_argv_from_env(path: &Path) -> Option<Vec<String>> {
    editor_argv(
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
        path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_beats_editor_and_args_are_split() {
        let p = Path::new("/c/microterm.toml");
        assert_eq!(
            editor_argv(Some("code -w"), Some("vi"), p).unwrap(),
            vec!["code", "-w", "/c/microterm.toml"]
        );
        assert_eq!(
            editor_argv(Some("  "), Some("vi"), p).unwrap(),
            vec!["vi", "/c/microterm.toml"]
        );
        assert!(editor_argv(None, None, p).is_none());
    }

    #[test]
    fn creates_template_once() {
        let dir = std::env::temp_dir().join(format!("mt_editor_{}", std::process::id()));
        let f = dir.join("sub").join("microterm.toml");
        assert!(ensure_config_file(&f).unwrap());
        assert!(!ensure_config_file(&f).unwrap());
        assert!(std::fs::read_to_string(&f)
            .unwrap()
            .contains("scrollback_lines"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
