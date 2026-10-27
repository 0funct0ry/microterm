//! Post-install setup for a drag-installed macOS bundle: `microterm` and `mt` links in
//! `~/.local/bin`, and that directory on the login shell's `PATH` (once). Best effort and off the
//! UI thread; panes already get the launcher directory on `PATH` (see `ipc_ops::pane_env`), so
//! this only matters for shells outside microterm. Set `MICROTERM_NO_CLI_SETUP` to skip it.

use std::io;
use std::path::{Path, PathBuf};

const LINKS: [&str; 2] = ["microterm", "mt"];
const MARKER: &str = "cli-setup";

/// Runs the setup in the background when the app runs from `X.app` on macOS.
pub fn spawn_setup() {
    if !cfg!(target_os = "macos") || std::env::var_os("MICROTERM_NO_CLI_SETUP").is_some() {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let Some(bundle_bin) = std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok())
        .and_then(|e| bundle_bin_of(&e))
    else {
        return;
    };
    std::thread::spawn(move || {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        if let Err(e) = setup(
            &bundle_bin,
            &home,
            &shell,
            crate::state::state_dir().as_deref(),
        ) {
            eprintln!("microterm: command-line setup skipped: {e}");
        }
    });
}

/// `X.app/Contents/Resources/bin` for the app binary `X.app/Contents/MacOS/microterm-app`.
fn bundle_bin_of(app_exe: &Path) -> Option<PathBuf> {
    let bundle = app_exe.ancestors().nth(3)?;
    let bin = bundle.join("Contents/Resources/bin");
    (bundle.extension().is_some_and(|e| e == "app") && bin.join("microterm").exists())
        .then_some(bin)
}

fn setup(bundle_bin: &Path, home: &Path, shell: &str, state: Option<&Path>) -> io::Result<()> {
    let bindir = home.join(".local/bin");
    // Links are checked every launch so they follow the app when it moves between folders.
    for name in LINKS {
        link(&bundle_bin.join(name), &bindir.join(name))?;
    }
    // The PATH edit happens once: a user who removes it later is not fought.
    let marker = state.map(|s| s.join(MARKER));
    if marker.as_deref().is_some_and(Path::exists) {
        return Ok(());
    }
    let (rc, line) = rc_for(shell, home);
    ensure_path_line(&rc, &line)?;
    if let Some(m) = marker {
        if let Some(dir) = m.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(m, "")?;
    }
    Ok(())
}

/// Points `dest` at `target`. A real file is left alone, as is a symlink to something that is
/// not a microterm bundle; stale microterm links (dangling or into another `.app`) are replaced.
fn link(target: &Path, dest: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let old = std::fs::read_link(dest)?;
            if old == target {
                return Ok(());
            }
            let ours = !dest.exists() || old.components().any(|c| c.as_os_str() == "microterm.app");
            if !ours {
                return Ok(());
            }
            std::fs::remove_file(dest)?;
        }
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::os::unix::fs::symlink(target, dest)
}

/// The profile file and line that put `~/.local/bin` on `PATH` (same rules as `install.sh`).
fn rc_for(shell: &str, home: &Path) -> (PathBuf, String) {
    if shell.ends_with("/fish") {
        (
            home.join(".config/fish/config.fish"),
            "fish_add_path $HOME/.local/bin".into(),
        )
    } else if shell.ends_with("/zsh") {
        (
            home.join(".zshrc"),
            r#"export PATH="$HOME/.local/bin:$PATH""#.into(),
        )
    } else {
        (
            home.join(".profile"),
            r#"export PATH="$HOME/.local/bin:$PATH""#.into(),
        )
    }
}

/// Appends `line` under a `# microterm` comment unless the file already mentions `.local/bin`.
fn ensure_path_line(rc: &Path, line: &str) -> io::Result<()> {
    let existing = std::fs::read_to_string(rc).unwrap_or_default();
    if existing.contains(".local/bin") {
        return Ok(());
    }
    if let Some(dir) = rc.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("\n# microterm\n{line}\n"));
    std::fs::write(rc, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mt_first_run_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn bundle(root: &Path) -> PathBuf {
        let bin = root.join("microterm.app/Contents/Resources/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for n in LINKS {
            std::fs::write(bin.join(n), "").unwrap();
        }
        bin
    }

    #[test]
    fn links_are_created_and_followed_when_the_app_moves() {
        let root = temp("links");
        let home = root.join("home");
        let bin = bundle(&root);
        setup(&bin, &home, "/bin/zsh", None).unwrap();
        let mt = home.join(".local/bin/mt");
        assert_eq!(std::fs::read_link(&mt).unwrap(), bin.join("mt"));

        let moved = root.join("elsewhere");
        let new_bin = bundle(&moved);
        std::fs::remove_dir_all(root.join("microterm.app")).unwrap();
        setup(&new_bin, &home, "/bin/zsh", None).unwrap();
        assert_eq!(std::fs::read_link(&mt).unwrap(), new_bin.join("mt"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn foreign_files_and_links_are_never_replaced() {
        let root = temp("foreign");
        let home = root.join("home");
        let bindir = home.join(".local/bin");
        std::fs::create_dir_all(&bindir).unwrap();
        std::fs::write(bindir.join("microterm"), "mine").unwrap();
        let other = root.join("tool");
        std::fs::write(&other, "").unwrap();
        std::os::unix::fs::symlink(&other, bindir.join("mt")).unwrap();
        setup(&bundle(&root), &home, "/bin/zsh", None).unwrap();
        assert_eq!(
            std::fs::read_to_string(bindir.join("microterm")).unwrap(),
            "mine"
        );
        assert_eq!(std::fs::read_link(bindir.join("mt")).unwrap(), other);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn path_line_is_added_once_and_only_if_missing() {
        let root = temp("path");
        let home = root.join("home");
        let state = root.join("state");
        let bin = bundle(&root);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".zshrc"), "alias k=kubectl").unwrap();
        setup(&bin, &home, "/bin/zsh", Some(&state)).unwrap();
        let rc = std::fs::read_to_string(home.join(".zshrc")).unwrap();
        assert!(rc.starts_with("alias k=kubectl\n"));
        assert!(rc.ends_with("# microterm\nexport PATH=\"$HOME/.local/bin:$PATH\"\n"));
        // The marker stops a second edit even after the user removes the line.
        std::fs::write(home.join(".zshrc"), "").unwrap();
        setup(&bin, &home, "/bin/zsh", Some(&state)).unwrap();
        assert_eq!(std::fs::read_to_string(home.join(".zshrc")).unwrap(), "");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_rc_that_already_mentions_local_bin_is_untouched() {
        let root = temp("existing");
        let rc = root.join(".zshrc");
        std::fs::write(&rc, "export PATH=$HOME/.local/bin:$PATH\n").unwrap();
        ensure_path_line(&rc, "x").unwrap();
        assert_eq!(
            std::fs::read_to_string(&rc).unwrap(),
            "export PATH=$HOME/.local/bin:$PATH\n"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn shell_picks_the_profile() {
        let h = Path::new("/h");
        assert_eq!(rc_for("/bin/zsh", h).0, Path::new("/h/.zshrc"));
        assert_eq!(
            rc_for("/opt/homebrew/bin/fish", h).0,
            Path::new("/h/.config/fish/config.fish")
        );
        assert_eq!(rc_for("/bin/bash", h).0, Path::new("/h/.profile"));
    }
}
