//! GPUI views and the component library (SPEC §4).
#![allow(clippy::type_complexity)] // GPUI callback signatures are inherently long.

pub mod actions;
pub mod app;
pub mod assets;
mod builtin_glyphs;
pub mod components;
pub mod copy_mode;
pub mod designer;
mod designer_view;
mod gpu_check;
pub mod ipc_ops;
pub mod ipc_pending;
pub mod keychips;
pub mod ls_fonts;
pub mod menu;
pub mod menu_bar;
mod menu_view;
pub mod metrics;
pub mod mock;
pub mod mode_overlays;
pub mod mouse_input;
pub mod mru;
pub mod mux_host;
pub mod notifier;
pub mod overlay_stack;
pub mod panes;
pub mod picker_state;
pub mod platform;
pub mod pointer_shape;
pub mod quick_select;
pub mod recording;
pub mod right_panel;
pub mod search;
pub mod serial_host;
pub mod session;
pub mod settings;
pub mod snippets;
pub mod ssh_host;
pub mod state;
pub mod tab_chrome;
pub mod tab_color;
pub mod terminal_view;
pub mod textgrid;
pub mod theme;
pub mod typography;
mod view;

use gpui::{App, Application};

use crate::actions::Quit;

/// Keeps the file watcher alive for the lifetime of the app.
struct WatcherGuard(#[allow(dead_code)] mt_config::watch::ConfigWatcher);
impl gpui::Global for WatcherGuard {}

/// The IPC server's hooks into the GUI (SPEC §12): requests to execute on the main thread,
/// the event hub `subscribe` connections read from, and the socket path panes are told about.
pub struct IpcHandle {
    pub calls: async_channel::Receiver<mt_proto::Call>,
    pub hub: std::sync::Arc<mt_proto::EventHub>,
    pub socket: std::path::PathBuf,
}

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
    /// Domain of the first tab (`microterm ssh`): a configured name or `ssh:[user@]host[:port]`.
    pub domain: Option<String>,
    /// IPC requests from other `microterm` invocations and `microterm ctl`.
    pub ipc: Option<IpcHandle>,
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
            snippets::library::sync(cx);
            settings::apply_theme(cx);
            settings::install_keymap(cx);
            cx.on_action(|_: &Quit, cx| recording::ipc::quit(cx));
            cx.set_menus(vec![gpui::Menu {
                name: "microterm".into(),
                items: vec![gpui::MenuItem::action("Quit microterm", Quit)],
            }]);
            menu_bar::install_app_actions(cx);
            cx.on_window_closed(|cx| {
                // Closing the last window of a workspace falls back to one that still has
                // windows (their shells were parked) before quitting.
                if cx.windows().is_empty() && !mux_host::reopen_parked_workspace(cx) {
                    recording::ipc::quit(cx);
                }
            })
            .detach();
            start_config_watcher(&path, cx);
            mux_host::ensure(cx);
            menu_bar::install(cx);
            if let Some(ipc) = &opts.ipc {
                mux_host::install_ipc(cx, ipc.socket.clone(), ipc.hub.clone());
            }
            let domain = opts.domain.as_deref().and_then(|d| {
                ssh_host::resolve_domain(cx, d)
                    .map_err(|e| eprintln!("microterm: {}", e.message))
                    .ok()
            });
            let first =
                (opts.cwd.is_some() || opts.command.is_some() || domain.is_some()).then(|| {
                    mt_mux::SpawnSpec {
                        domain: domain.unwrap_or(mt_mux::LOCAL_DOMAIN_ID),
                        cwd: opts.cwd.clone(),
                        command: opts.command.clone(),
                        title: None,
                        ..Default::default()
                    }
                });
            if let Err(e) = mux_host::show_workspace(cx, &startup_workspace, true, first) {
                eprintln!("microterm: {e}");
                mux_host::show_workspace(cx, mt_mux::DEFAULT_WORKSPACE, true, None).ok();
            }
            if let Some(calls) = opts.ipc.as_ref().map(|i| i.calls.clone()) {
                cx.spawn(async move |cx| {
                    while let Ok(call) = calls.recv().await {
                        let ok = cx.update(|cx| mux_host::dispatch(cx, call));
                        if ok.is_err() {
                            break;
                        }
                    }
                })
                .detach();
                // `pane.run` / `pane.wait` answer when their condition holds.
                cx.spawn(async move |cx| loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(50))
                        .await;
                    if cx.update(mux_host::poll_pending).is_err() {
                        break;
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
                            // The watcher also reports `snippets.toml`; unchanged text is a no-op.
                            snippets::library::sync_and_notify(cx);
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
