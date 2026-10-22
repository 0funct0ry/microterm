//! The loaded snippet set as a GPUI global (SPEC §10.8.2). It is read only while
//! `snippets.enabled = true`; a broken file keeps the last valid set and shows a banner that
//! names `snippets.toml`. Reloading identical text is a no-op, which also covers our own
//! write-back (SPEC §8.5).

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Global};
use mt_config::edit::content_hash;
use mt_snippets::store::{self, Diagnostic};
use mt_snippets::SnippetSet;

use super::gate::SnippetsGate;
use crate::{
    app::AppWindow,
    components::ToastKind,
    settings::{Banner, Settings},
};

pub const FILE_NAME: &str = "snippets.toml";

pub struct Library {
    pub set: Arc<SnippetSet>,
    /// Non-fatal findings of the last good load (warnings, skipped entries).
    pub diagnostics: Vec<Diagnostic>,
    /// Shown under the config banner; names `snippets.toml`.
    pub banner: Option<Banner>,
    loaded_hash: Option<u64>,
    active: bool,
}

impl Global for Library {}

impl Library {
    fn inactive() -> Self {
        Self {
            set: Arc::new(SnippetSet::default()),
            diagnostics: Vec::new(),
            banner: None,
            loaded_hash: None,
            active: false,
        }
    }
}

/// `<config_dir>/snippets.toml` next to the active config file.
pub fn path(cx: &App) -> PathBuf {
    mt_config::load::snippets_path(&cx.global::<Settings>().path)
}

/// The current set (empty while disabled).
pub fn set(cx: &App) -> Arc<SnippetSet> {
    cx.try_global::<Library>()
        .map_or_else(Default::default, |l| l.set.clone())
}

pub fn banner(cx: &App) -> Option<Banner> {
    cx.try_global::<Library>().and_then(|l| l.banner.clone())
}

fn render_first(diags: &[Diagnostic]) -> String {
    let mut out = diags
        .first()
        .map_or_else(String::new, |d| d.render(FILE_NAME));
    if diags.len() > 1 {
        out.push_str(&format!(" (+{} more)", diags.len() - 1));
    }
    out
}

/// Brings the global in line with the gate and the file. Returns whether anything changed
/// (callers then refresh their windows). Call from the watcher, after a config reload and
/// after our own writes.
pub fn sync(cx: &mut App) -> bool {
    if !cx.has_global::<Library>() {
        cx.set_global(Library::inactive());
    }
    let gate = SnippetsGate::of(cx);
    let (active, loaded_hash, had_set) = {
        let l = cx.global::<Library>();
        (
            l.active,
            l.loaded_hash,
            !l.set.snippets.is_empty() || l.banner.is_some(),
        )
    };
    if !gate.enabled {
        if active || had_set {
            cx.set_global(Library::inactive());
            return true;
        }
        return false;
    }
    let path = path(cx);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            let banner = Banner {
                kind: ToastKind::Error,
                text: format!(
                    "{FILE_NAME} — cannot read the file: {e}. Using the last valid snippets."
                ),
            };
            return replace(cx, None, banner_only(banner), true);
        }
    };
    let hash = content_hash(&text);
    if active && loaded_hash == Some(hash) {
        return false;
    }
    match store::load_str(&text) {
        Ok(mut loaded) => {
            for message in super::keys::problems(&loaded.set, crate::platform::Platform::current())
            {
                loaded.diagnostics.push(Diagnostic {
                    severity: mt_config::Severity::Warning,
                    path: String::new(),
                    message,
                    line: 0,
                    col: 0,
                });
            }
            let banner = (!loaded.diagnostics.is_empty()).then(|| Banner {
                kind: if loaded
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == mt_config::Severity::Error)
                {
                    ToastKind::Error
                } else {
                    ToastKind::Warning
                },
                text: render_first(&loaded.diagnostics),
            });
            cx.set_global(Library {
                set: Arc::new(loaded.set),
                diagnostics: loaded.diagnostics,
                banner,
                loaded_hash: Some(hash),
                active: true,
            });
            true
        }
        Err(diags) => {
            let banner = Banner {
                kind: ToastKind::Error,
                text: format!("{} Using the last valid snippets.", render_first(&diags)),
            };
            replace(cx, Some(hash), banner_only(banner), true)
        }
    }
}

fn banner_only(b: Banner) -> Option<Banner> {
    Some(b)
}

/// Keeps the previous set, swaps the banner.
fn replace(cx: &mut App, hash: Option<u64>, banner: Option<Banner>, active: bool) -> bool {
    let (set, diagnostics) = {
        let l = cx.global::<Library>();
        (l.set.clone(), l.diagnostics.clone())
    };
    cx.set_global(Library {
        set,
        diagnostics,
        banner,
        loaded_hash: hash,
        active,
    });
    true
}

/// `sync`, then tells every window to refresh (panel rows, banner, keymap, palette).
pub fn sync_and_notify(cx: &mut App) {
    let changed = sync(cx);
    if changed {
        // Per-snippet keys follow the set (and the gate).
        crate::settings::install_keymap(cx);
        notify_windows(cx);
    }
}

pub fn notify_windows(cx: &mut App) {
    for handle in cx.windows() {
        if let Some(h) = handle.downcast::<AppWindow>() {
            let _ = h.update(cx, |this, window, cx| this.snippets_changed(window, cx));
        }
    }
}
