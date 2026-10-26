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

/// The editor argv from the process environment, or, when neither variable is set there (an app
/// launched from Finder/Dock does not see shell exports), from the user's login shell.
pub fn editor_argv_from_env(path: &Path) -> Option<Vec<String>> {
    let var = |k| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let (visual, editor) = (var("VISUAL"), var("EDITOR"));
    if visual.is_some() || editor.is_some() {
        return editor_argv(visual.as_deref(), editor.as_deref(), path);
    }
    let (visual, editor) = login_shell_editor();
    editor_argv(visual.as_deref(), editor.as_deref(), path)
}

const MARK: &str = "__MT_EDITOR__";

/// `$VISUAL` and `$EDITOR` as an interactive login shell exports them. Cached; bounded to 2 s so
/// a slow rc file cannot hang the UI for long.
fn login_shell_editor() -> (Option<String>, Option<String>) {
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    static CACHE: OnceLock<(Option<String>, Option<String>)> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            let script = format!("printf '{MARK}%s{MARK}%s{MARK}' \"$VISUAL\" \"$EDITOR\"");
            let Ok(mut child) = Command::new(shell)
                .args(["-ilc", &script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            else {
                return (None, None);
            };
            let mut out = child.stdout.take();
            let reader = std::thread::spawn(move || {
                let mut buf = String::new();
                if let Some(o) = out.as_mut() {
                    let _ = io::Read::read_to_string(o, &mut buf);
                }
                buf
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while matches!(child.try_wait(), Ok(None)) {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return (None, None);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            parse_login_output(&reader.join().unwrap_or_default())
        })
        .clone()
}

/// Extracts the two values from `MARK visual MARK editor MARK`, ignoring any rc-file noise.
fn parse_login_output(out: &str) -> (Option<String>, Option<String>) {
    let mut parts = out.rsplitn(4, MARK).collect::<Vec<_>>();
    parts.reverse();
    let non_empty = |s: Option<&&str>| {
        s.map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(String::from)
    };
    match parts.as_slice() {
        [_, v, e, _] => (non_empty(Some(v)), non_empty(Some(e))),
        _ => (None, None),
    }
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
    fn login_shell_output_is_parsed_past_rc_noise() {
        let out = format!("welcome\n{MARK}{MARK}vim{MARK}");
        assert_eq!(parse_login_output(&out), (None, Some("vim".into())));
        let out = format!("{MARK}code -w{MARK}vi{MARK}");
        assert_eq!(
            parse_login_output(&out),
            (Some("code -w".into()), Some("vi".into()))
        );
        assert_eq!(parse_login_output("garbage"), (None, None));
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
