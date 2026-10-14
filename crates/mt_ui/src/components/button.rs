use gpui::{prelude::*, px, App, ClickEvent, ElementId, SharedString, Window};

use super::{theme, Icon, Tooltip};
use crate::{assets::IconName, metrics::RADIUS_CONTROL};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Accent-coloured primary action.
    Filled,
    /// Transparent until hovered.
    Ghost,
    /// Bordered, quiet.
    Subtle,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    style: ButtonStyle,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            style: ButtonStyle::Subtle,
            on_click: None,
        }
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let (bg, fg, border, hover) = match self.style {
            ButtonStyle::Filled => (
                t.accent,
                if t.is_dark {
                    t.background
                } else {
                    gpui::white()
                },
                t.accent,
                t.accent.opacity(0.85),
            ),
            ButtonStyle::Ghost => (
                gpui::transparent_black(),
                t.text,
                gpui::transparent_black(),
                t.element_hover,
            ),
            ButtonStyle::Subtle => (t.surface, t.text, t.border, t.element_hover),
        };
        gpui::div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(24.))
            .px_2()
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .text_size(px(12.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(self.label)
            .when_some(self.on_click, |el, handler| el.on_click(handler))
    }
}

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
    on_right_click: Option<ClickHandler>,
    selected: bool,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            tooltip: None,
            on_click: None,
            on_right_click: None,
            selected: false,
        }
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    pub fn on_right_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_right_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let hover = t.element_hover;
        let on_right = self.on_right_click;
        gpui::div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(RADIUS_CONTROL))
            .text_color(if self.selected { t.text } else { t.text_muted })
            .when(self.selected, |el| el.bg(t.element_selected))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(Icon::new(self.icon))
            .when_some(self.tooltip, |el, text| {
                el.tooltip(move |_, cx| cx.new(|_| Tooltip::new(text.clone())).into())
            })
            .when_some(self.on_click, |el, handler| {
                el.on_click(move |e, w, c| {
                    if !e.is_right_click() {
                        handler(e, w, c)
                    }
                })
            })
            .when_some(on_right, |el, handler| {
                el.on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_up(gpui::MouseButton::Right, move |e, w, c| {
                        // Synthesize a click for right-button release so callers get a position.
                        let click = ClickEvent::Mouse(gpui::MouseClickEvent {
                            down: gpui::MouseDownEvent {
                                button: gpui::MouseButton::Right,
                                position: e.position,
                                modifiers: e.modifiers,
                                click_count: 1,
                                first_mouse: false,
                            },
                            up: e.clone(),
                        });
                        handler(&click, w, c)
                    })
            })
    }
}
