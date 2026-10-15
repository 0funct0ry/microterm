//! GPUI views and the component library (SPEC §4).
#![allow(clippy::type_complexity)] // GPUI callback signatures are inherently long.

pub mod actions;
pub mod app;
pub mod assets;
mod builtin_glyphs;
pub mod components;
mod gpu_check;
pub mod keychips;
pub mod metrics;
pub mod mock;
pub mod overlay_stack;
pub mod picker_state;
pub mod platform;
pub mod session;
pub mod settings;
pub mod terminal_view;
pub mod theme;
mod view;

use gpui::{App, AppContext as _, Application};

use crate::{actions::Quit, app::AppWindow};

/// Opens a main window.
pub fn open_main_window(cx: &mut App) {
    let options = app::window_options(cx);
    cx.open_window(options, |window, cx| {
        cx.new(|cx| AppWindow::new(window, cx))
    })
    .expect("failed to open window");
}

/// Keeps the file watcher alive for the lifetime of the app.
struct WatcherGuard(#[allow(dead_code)] mt_config::watch::ConfigWatcher);
impl gpui::Global for WatcherGuard {}

/// Launches the GUI and blocks until the last window closes. `config` is the `--config` path.
pub fn run(config: Option<std::path::PathBuf>) {
    gpu_check::ensure_gpu_available();
    let path = mt_config::load::config_path(config.as_deref());
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
            cx.set_global(settings);
            settings::apply_theme(cx);
            settings::install_keymap(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(vec![gpui::Menu {
                name: "microterm".into(),
                items: vec![gpui::MenuItem::action("Quit microterm", Quit)],
            }]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            start_config_watcher(&path, cx);
            open_main_window(cx);
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
