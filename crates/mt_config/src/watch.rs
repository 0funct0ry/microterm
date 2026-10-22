//! File watching with a 150 ms debounce (SPEC §8.2). Editors save via rename, so the parent
//! directory is watched as well as the file; `snippets.toml` (same
//! directory) and the themes directory are watched too. The receiver does not say which file
//! changed; consumers re-read what they own and treat unchanged content as a no-op.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::load::{snippets_path, themes_dir};

pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// Keeps the watcher and debounce thread alive; drop it to stop watching.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    _thread: JoinHandle<()>,
}

fn relevant(path: &Path, config: &Path, snippets: &Path, themes: &Path) -> bool {
    path == config || path == snippets || path.starts_with(themes)
}

/// Start watching. The receiver yields one `()` per debounced burst of relevant changes.
pub fn watch(config_path: &Path) -> notify::Result<(ConfigWatcher, async_channel::Receiver<()>)> {
    watch_with(config_path, DEBOUNCE)
}

pub fn watch_with(
    config_path: &Path,
    debounce: Duration,
) -> notify::Result<(ConfigWatcher, async_channel::Receiver<()>)> {
    // FSEvents/inotify report canonical paths, so compare against the canonical parent.
    let config: PathBuf = match (config_path.parent(), config_path.file_name()) {
        (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => dir
            .canonicalize()
            .map_or_else(|_| config_path.to_path_buf(), |d| d.join(name)),
        _ => config_path.to_path_buf(),
    };
    let themes = themes_dir(&config);
    let snippets = snippets_path(&config);
    let (raw_tx, raw_rx) = mpsc::channel::<()>();
    let (tx, rx) = async_channel::unbounded::<()>();

    let (cfg2, snip2, themes2) = (config.clone(), snippets, themes.clone());
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if ev
                .paths
                .iter()
                .any(|p| relevant(p, &cfg2, &snip2, &themes2))
            {
                let _ = raw_tx.send(());
            }
        }
    })?;

    if let Some(parent) = config.parent().filter(|p| !p.as_os_str().is_empty()) {
        if parent.exists() {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        }
    } else {
        watcher.watch(Path::new("."), RecursiveMode::NonRecursive)?;
    }
    if themes.exists() {
        watcher.watch(&themes, RecursiveMode::NonRecursive)?;
    }

    let thread = std::thread::Builder::new()
        .name("mt-config-watch".into())
        .spawn(move || {
            while raw_rx.recv().is_ok() {
                // Swallow the rest of the burst until it has been quiet for `debounce`.
                while raw_rx.recv_timeout(debounce).is_ok() {}
                if tx.send_blocking(()).is_err() {
                    break;
                }
            }
        })
        .map_err(notify::Error::io)?;

    Ok((
        ConfigWatcher {
            _watcher: watcher,
            _thread: thread,
        },
        rx,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    trait RecvTimeout {
        fn recv_timeout_compat(&self, d: Duration) -> bool;
    }

    impl RecvTimeout for async_channel::Receiver<()> {
        fn recv_timeout_compat(&self, d: Duration) -> bool {
            let end = std::time::Instant::now() + d;
            while std::time::Instant::now() < end {
                if self.try_recv().is_ok() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        }
    }

    #[test]
    fn burst_of_writes_yields_one_event() {
        let dir = std::env::temp_dir().join(format!("mt_watch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("microterm.toml");
        std::fs::write(&file, "").unwrap();
        let (_w, rx) = watch_with(&file, Duration::from_millis(150)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        for i in 0..5 {
            std::fs::write(&file, format!("scrollback_lines = {}\n", 100 + i)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let first = rx.recv_timeout_compat(Duration::from_secs(5));
        assert!(first, "expected a debounced event");
        std::thread::sleep(Duration::from_millis(400));
        assert!(rx.try_recv().is_err(), "burst must coalesce into one event");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
