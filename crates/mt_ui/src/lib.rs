//! GPUI views and the component library (SPEC §4).
#![allow(clippy::type_complexity)] // GPUI callback signatures are inherently long.

pub mod actions;
pub mod app;
pub mod assets;
mod builtin_glyphs;
pub mod components;
mod gpu_check;
pub mod keychips;
pub mod ls_fonts;
pub mod metrics;
pub mod mock;
pub mod mouse_input;
pub mod mru;
pub mod mux_host;
pub mod overlay_stack;
pub mod picker_state;
pub mod platform;
pub mod session;
pub mod settings;
pub mod state;
pub mod terminal_view;
pub mod theme;
pub mod typography;
mod view;

use gpui::{App, Application};

use crate::actions::Quit;

/// Keeps the file watcher alive for the lifetime of the app.
struct WatcherGuard(#[allow(dead_code)] mt_config::watch::ConfigWatcher);
impl gpui::Global for WatcherGuard {}

pub use mux_host::NewWindowRequest;

/// Everything `microterm [FLAGS]` hands to the GUI.
#[derive(Default)]
pub struct RunOptions {
    /// `--config` path.
    pub config: Option<std::path::PathBuf>,
    /// `--workspace`: the workspace to start in (created when undeclared).
    pub workspace: Option<String>,
    /// `--cwd` and `-e CMD`: applied to the first tab.
    pub cwd: Option<std::path::PathBuf>,
    pub command: Option<Vec<String>>,
    /// Window requests from other `microterm` invocations (single-instance IPC).
    pub requests: Option<async_channel::Receiver<NewWindowRequest>>,
}

/// Launches the GUI and blocks until the last window closes.
pub fn run(opts: RunOptions) {
    gpu_check::ensure_gpu_available();
    let path = mt_config::load::config_path(opts.config.as_deref());
    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            assets::load_fonts(cx).expect("bundled fonts must load");
            let families = cx.text_system().all_font_names();
            let settings = settings::Settings::load(path.clone());
            for wanted in [metrics::UI_FONT, settings.config.font.family.as_str()] {
                if !families.iter().any(|f| f == wanted) {
                    eprintln!("microterm: font {wanted:?} is not installed; falling back");
                }
            }
            let startup_workspace = opts
                .workspace
                .clone()
                .unwrap_or_else(|| settings.config.startup_workspace.clone());
            cx.set_global(settings);
            settings::apply_theme(cx);
            settings::install_keymap(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(vec![gpui::Menu {
                name: "microterm".into(),
                items: vec![gpui::MenuItem::action("Quit microterm", Quit)],
            }]);
            cx.on_window_closed(|cx| {
                // Closing the last window of a workspace falls back to one that still has
                // windows (their shells were parked) before quitting.
                if cx.windows().is_empty() && !mux_host::reopen_parked_workspace(cx) {
                    cx.quit();
                }
            })
            .detach();
            start_config_watcher(&path, cx);
            mux_host::ensure(cx);
            let first = (opts.cwd.is_some() || opts.command.is_some()).then(|| mt_mux::SpawnSpec {
                cwd: opts.cwd.clone(),
                command: opts.command.clone(),
                title: None,
            });
            if let Err(e) = mux_host::show_workspace(cx, &startup_workspace, true, first) {
                eprintln!("microterm: {e}");
                mux_host::show_workspace(cx, mt_mux::DEFAULT_WORKSPACE, true, None).ok();
            }
            if let Some(requests) = opts.requests.clone() {
                cx.spawn(async move |cx| {
                    while let Ok(req) = requests.recv().await {
                        if cx.update(|cx| mux_host::handle_request(cx, req)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
            }
            cx.activate(true);
        });
}

/// Hot reload: file events (debounced 150 ms) trigger the reload pipeline on the main thread.
fn start_config_watcher(path: &std::path::Path, cx: &mut App) {
    match mt_config::watch::watch(path) {
        Ok((guard, events)) => {
            cx.set_global(WatcherGuard(guard));
            cx.spawn(async move |cx| {
                while events.recv().await.is_ok() {
                    if cx
                        .update(|cx| {
                            settings::reload(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        Err(e) => eprintln!("microterm: config hot reload is unavailable: {e}"),
    }
}

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod live_tests;
