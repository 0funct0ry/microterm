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
pub mod terminal_view;
pub mod theme;
mod view;

use gpui::{App, AppContext as _, Application};

use crate::{actions::Quit, app::AppWindow, platform::Platform, theme::Theme};

/// Opens a main window.
pub fn open_main_window(cx: &mut App) {
    let options = app::window_options(cx);
    cx.open_window(options, |window, cx| {
        cx.new(|cx| AppWindow::new(window, cx))
    })
    .expect("failed to open window");
}

/// Launches the GUI and blocks until the last window closes.
pub fn run() {
    gpu_check::ensure_gpu_available();
    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            assets::load_fonts(cx).expect("bundled fonts must load");
            let families = cx.text_system().all_font_names();
            for wanted in [metrics::UI_FONT, metrics::TERM_FONT] {
                if !families.iter().any(|f| f == wanted) {
                    eprintln!("microterm: bundled font {wanted:?} did not register; falling back");
                }
            }
            cx.set_global(match cx.window_appearance() {
                gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight => {
                    Theme::one_light()
                }
                _ => Theme::one_dark(),
            });
            let bindings = actions::key_bindings(Platform::current(), cx);
            cx.bind_keys(bindings);
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
            open_main_window(cx);
            cx.activate(true);
        });
}

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod live_tests;
