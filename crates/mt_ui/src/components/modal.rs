use gpui::{prelude::*, px, AnyElement, App, ElementId, MouseButton, Pixels, Window};

use super::theme;
use crate::metrics::RADIUS_POPOVER;

/// A dim backdrop plus a popover card anchored below the top edge. Clicking the backdrop
/// dismisses; clicks inside the card never reach it.
#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    width: Pixels,
    top: Pixels,
    children: Vec<AnyElement>,
    on_dismiss: Option<Box<dyn Fn(&mut Window, &mut App)>>,
}

impl Modal {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            width: px(560.),
            top: px(98.),
            children: Vec::new(),
            on_dismiss: None,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn top(mut self, top: Pixels) -> Self {
        self.top = top;
        self
    }

    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Modal {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let backdrop = if t.is_dark {
            gpui::black().opacity(0.35)
        } else {
            gpui::black().opacity(0.15)
        };
        gpui::div()
            .id(self.id)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(self.top)
            .bg(backdrop)
            .occlude()
            .when_some(self.on_dismiss, |el, handler| {
                el.on_mouse_down(MouseButton::Left, move |_, window, cx| handler(window, cx))
            })
            .child(
                gpui::div()
                    .id("modal-card")
                    .w(self.width)
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .rounded(px(RADIUS_POPOVER))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.elevated_surface)
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(self.children),
            )
    }
}
