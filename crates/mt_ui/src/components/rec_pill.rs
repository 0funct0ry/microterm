use gpui::{prelude::*, px, App, ClickEvent, ElementId, SharedString, Window};

use super::theme;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The `REC` mode pill (SPEC §10.10.2): a red dot, the label and a click-to-stop handler.
#[derive(IntoElement)]
pub struct RecPill {
    id: ElementId,
    text: SharedString,
    /// Dot opacity; the owner flips it once per second, or keeps it at 1.0 under reduced motion.
    dim: bool,
    on_click: Option<ClickHandler>,
}

impl RecPill {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            dim: false,
            on_click: None,
        }
    }

    pub fn dim(mut self, dim: bool) -> Self {
        self.dim = dim;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for RecPill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        gpui::div()
            .id(self.id)
            .px_2()
            .h(px(20.))
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_full()
            .bg(t.element_selected)
            .text_color(t.text)
            .text_size(px(11.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .cursor_pointer()
            .child(
                gpui::div()
                    .flex_none()
                    .size(px(8.))
                    .rounded_full()
                    .bg(t.error)
                    .opacity(if self.dim { 0.35 } else { 1.0 }),
            )
            .child(self.text)
            .when_some(self.on_click, |el, f| el.on_click(f))
    }
}
