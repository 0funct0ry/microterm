use gpui::{prelude::*, px, App, SharedString, Window};

use super::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Connected,
    Reconnecting,
    Error,
    Idle,
    Accent,
}

/// The coloured dot used for domain state and dirty-tab indicators.
#[derive(IntoElement)]
pub struct StatusDot {
    kind: StatusKind,
    size: f32,
}

impl StatusDot {
    pub fn new(kind: StatusKind) -> Self {
        Self { kind, size: 6.0 }
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let color = match self.kind {
            StatusKind::Connected => t.success,
            StatusKind::Reconnecting => t.warning,
            StatusKind::Error => t.error,
            StatusKind::Idle => t.text_placeholder,
            StatusKind::Accent => t.accent,
        };
        gpui::div()
            .flex_none()
            .size(px(self.size))
            .rounded_full()
            .bg(color)
    }
}

/// A small rounded label, used for the mode pill ("COPY", "QUICK SELECT", "SEARCH").
#[derive(IntoElement)]
pub struct Pill {
    text: SharedString,
    accent: bool,
}

impl Pill {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            accent: true,
        }
    }

    pub fn muted(mut self) -> Self {
        self.accent = false;
        self
    }
}

impl RenderOnce for Pill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let (bg, fg) = if self.accent {
            (
                t.accent,
                if t.is_dark {
                    t.background
                } else {
                    gpui::white()
                },
            )
        } else {
            (t.element_selected, t.text)
        };
        gpui::div()
            .px_2()
            .h(px(20.))
            .flex()
            .items_center()
            .rounded_full()
            .bg(bg)
            .text_color(fg)
            .text_size(px(11.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(self.text)
    }
}
