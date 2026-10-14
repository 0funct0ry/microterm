use gpui::{prelude::*, px, App, SharedString, Window};

use super::theme;

/// A single-line text field *display*: query text, caret and placeholder. Key handling lives in
/// the owning overlay, which forwards characters into its own state.
#[derive(IntoElement)]
pub struct TextInput {
    text: SharedString,
    placeholder: SharedString,
}

impl TextInput {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            placeholder: SharedString::default(),
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
}

impl RenderOnce for TextInput {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let empty = self.text.is_empty();
        gpui::div()
            .flex()
            .flex_1()
            .items_center()
            .h(px(20.))
            .text_size(px(13.))
            .text_color(t.text)
            .when(!empty, |el| el.child(self.text.clone()))
            .child(gpui::div().w(px(1.)).h(px(15.)).bg(t.accent))
            .when(empty, |el| {
                el.child(
                    gpui::div()
                        .ml_1()
                        .text_color(t.text_placeholder)
                        .child(self.placeholder.clone()),
                )
            })
    }
}
