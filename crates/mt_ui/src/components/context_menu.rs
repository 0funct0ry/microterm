use std::rc::Rc;

use gpui::{prelude::*, px, App, ElementId, MouseButton, Pixels, Point, SharedString, Window};

use super::{theme, KeyBinding};
use crate::metrics::{LIST_ROW_HEIGHT, RADIUS_CONTROL, RADIUS_POPOVER};

pub struct ContextMenuItem {
    pub label: SharedString,
    pub chip: Option<SharedString>,
}

impl ContextMenuItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            chip: None,
        }
    }

    pub fn chip(mut self, chip: impl Into<SharedString>) -> Self {
        self.chip = Some(chip.into());
        self
    }
}

/// A popup menu at a window position. Clicking anywhere else dismisses it.
#[derive(IntoElement)]
pub struct ContextMenu {
    id: ElementId,
    position: Point<Pixels>,
    items: Vec<ContextMenuItem>,
    on_select: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
    on_dismiss: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl ContextMenu {
    pub fn new(id: impl Into<ElementId>, position: Point<Pixels>) -> Self {
        Self {
            id: id.into(),
            position,
            items: Vec::new(),
            on_select: None,
            on_dismiss: None,
        }
    }

    pub fn items(mut self, items: Vec<ContextMenuItem>) -> Self {
        self.items = items;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let hover = t.element_hover;
        let on_select = self.on_select;
        let dismiss = self.on_dismiss;
        let dismiss_left = dismiss.clone();
        let dismiss_right = dismiss.clone();
        let rows = self.items.into_iter().enumerate().map(|(i, item)| {
            let on_select = on_select.clone();
            let dismiss = dismiss.clone();
            gpui::div()
                .id(super::child_id(&self.id, i))
                .flex()
                .items_center()
                .gap_4()
                .h(px(LIST_ROW_HEIGHT))
                .px_2()
                .rounded(px(RADIUS_CONTROL))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(gpui::div().flex_1().child(item.label))
                .when_some(item.chip, |el, chip| el.child(KeyBinding::from_text(chip)))
                .on_click(move |_, window, cx| {
                    if let Some(dismiss) = &dismiss {
                        dismiss(window, cx);
                    }
                    if let Some(select) = &on_select {
                        select(i, window, cx);
                    }
                })
        });
        gpui::div()
            .id(self.id.clone())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(d) = &dismiss_left {
                    d(window, cx);
                }
            })
            .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                if let Some(d) = &dismiss_right {
                    d(window, cx);
                }
            })
            .child(
                gpui::div()
                    .absolute()
                    .left(self.position.x)
                    .top(self.position.y)
                    .min_w(px(180.))
                    .flex()
                    .flex_col()
                    .p_1()
                    .rounded(px(RADIUS_POPOVER))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.elevated_surface)
                    .shadow_lg()
                    .text_size(px(13.))
                    .text_color(t.text)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(rows),
            )
    }
}
