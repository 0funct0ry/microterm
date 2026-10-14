use gpui::{prelude::*, px, App, SharedString, Window};

use super::theme;
use crate::{keychips::format_keystroke, metrics::RADIUS_CONTROL, platform::Platform};

/// A keybinding chip: `⌘⇧P` on macOS, `Ctrl+Shift+P` on Linux.
#[derive(IntoElement)]
pub struct KeyBinding {
    text: SharedString,
}

impl KeyBinding {
    /// From a GPUI keystroke like `cmd-shift-p`, formatted for the current platform.
    pub fn new(keystroke: &str) -> Self {
        Self::for_platform(keystroke, Platform::current())
    }

    pub fn for_platform(keystroke: &str, platform: Platform) -> Self {
        Self {
            text: format_keystroke(keystroke, platform).into(),
        }
    }

    /// From text that is already formatted.
    pub fn from_text(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for KeyBinding {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        gpui::div()
            .flex_none()
            .px_1p5()
            .h(px(18.))
            .flex()
            .items_center()
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(t.border_variant)
            .text_color(t.text_muted)
            .text_size(px(12.))
            .child(self.text)
    }
}
