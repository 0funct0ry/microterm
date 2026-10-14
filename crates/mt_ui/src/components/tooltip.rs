use gpui::{prelude::*, px, Context, SharedString, Window};

use super::theme;
use crate::metrics::RADIUS_CONTROL;

/// A small label shown on hover via `.tooltip(...)`.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        gpui::div()
            .px_2()
            .py_1()
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(t.border)
            .bg(t.elevated_surface)
            .text_color(t.text)
            .text_size(px(12.))
            .shadow_md()
            .child(self.text.clone())
    }
}
