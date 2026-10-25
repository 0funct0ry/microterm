//! Triggers (M12.6, SPEC §10.11): opt-in local automation rules.

mod apply;
pub mod deco;
pub mod editor;
pub mod engine;
pub mod findings;
pub mod gate;
mod gated;
pub mod ipc;
pub mod paint;
#[cfg(test)]
mod panel_tests;
mod panel_view;
pub mod preview;
pub mod runtime;
pub mod state;
#[cfg(test)]
mod tests;
pub mod ui;

pub use gate::TriggerGate;

/// `text` cut to `max` characters (with a trailing `…`), whitespace collapsed.
pub(crate) fn template_clip(text: &str, max: usize) -> String {
    mt_triggers::template::sanitize_with(text, max)
}

/// OSC 133 `C` in `pane` (counters with `reset_on = "command_start"` restart).
pub(crate) fn on_command_start(cx: &mut gpui::App, pane: u64) {
    apply::on_command_start(cx, pane);
}

/// `pane` got focus (counters with `reset_on = "focus"` restart, `clear_on_focus` colors clear).
pub(crate) fn on_focus(cx: &mut gpui::App, pane: u64) {
    apply::on_focus(cx, pane);
}
