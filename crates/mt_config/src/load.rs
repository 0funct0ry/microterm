//! Config location (SPEC §8.1) and loading.

use std::path::{Path, PathBuf};

use crate::diagnostics::{parse_str, Diagnostic, Loaded};

pub const FILE_NAME: &str = "microterm.toml";

/// Pure resolution: `--config` → `$MICROTERM_CONFIG` → `$XDG_CONFIG_HOME` → `~/.config`.
pub fn resolve_path(
    cli: Option<&Path>,
    env_config: Option<&str>,
    xdg_config_home: Option<&str>,
    home: Option<&Path>,
) -> PathBuf {
    if let Some(p) = cli {
        return p.to_path_buf();
    }
    if let Some(p) = env_config.filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(x) = xdg_config_home.filter(|s| !s.is_empty()) {
        return Path::new(x).join("microterm").join(FILE_NAME);
    }
    home.map_or_else(
        || PathBuf::from(FILE_NAME),
        |h| h.join(".config").join("microterm").join(FILE_NAME),
    )
}

/// Resolve the config path from the process environment.
pub fn config_path(cli: Option<&Path>) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_path(
        cli,
        std::env::var("MICROTERM_CONFIG").ok().as_deref(),
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        home.as_deref(),
    )
}

/// Directory holding the config file (and the user `themes/` directory).
pub fn config_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

pub fn themes_dir(path: &Path) -> PathBuf {
    config_dir(path).join("themes")
}

/// Load from disk. A missing file means all defaults.
pub fn load(path: &Path) -> Result<Loaded, Vec<Diagnostic>> {
    match std::fs::read_to_string(path) {
        Ok(src) => parse_str(&src),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => parse_str(""),
        Err(e) => Err(vec![Diagnostic::error_without_position(format!(
            "cannot read {}: {e}",
            path.display()
        ))]),
    }
}

/// Short file name for diagnostics.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| FILE_NAME.into(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_order() {
        let home = Path::new("/h");
        let cli = Path::new("/c/x.toml");
        assert_eq!(
            resolve_path(Some(cli), Some("/e"), Some("/x"), Some(home)),
            cli
        );
        assert_eq!(
            resolve_path(None, Some("/e.toml"), Some("/x"), Some(home)),
            Path::new("/e.toml")
        );
        assert_eq!(
            resolve_path(None, None, Some("/x"), Some(home)),
            Path::new("/x/microterm/microterm.toml")
        );
        assert_eq!(
            resolve_path(None, Some(""), Some(""), Some(home)),
            Path::new("/h/.config/microterm/microterm.toml")
        );
    }

    #[test]
    fn missing_file_is_defaults() {
        let l = load(Path::new("/definitely/not/here/microterm.toml")).unwrap();
        assert_eq!(l.config, crate::Config::default());
    }

    #[test]
    fn themes_dir_is_next_to_config() {
        assert_eq!(
            themes_dir(Path::new("/a/b/microterm.toml")),
            Path::new("/a/b/themes")
        );
    }
}
