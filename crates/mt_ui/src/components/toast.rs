use gpui::{prelude::*, px, AnyElement, App, ClickEvent, ElementId, SharedString, Window};

use super::{theme, Button, ButtonStyle, Icon, IconButton};
use crate::{assets::IconName, metrics::RADIUS_POPOVER};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(IntoElement)]
pub struct Toast {
    kind: ToastKind,
    text: SharedString,
    action: Option<(ElementId, SharedString, ClickHandler)>,
}

impl Toast {
    pub fn new(kind: ToastKind, text: impl Into<SharedString>) -> Self {
        Self {
            kind,
            text: text.into(),
            action: None,
        }
    }

    /// A button after the text (e.g. "Open config").
    pub fn action(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some((id.into(), label.into(), Box::new(handler)));
        self
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let (icon, color) = match self.kind {
            ToastKind::Info => (IconName::Info, t.accent),
            ToastKind::Success => (IconName::Check, t.success),
            ToastKind::Warning => (IconName::TriangleAlert, t.warning),
            ToastKind::Error => (IconName::CircleX, t.error),
        };
        gpui::div()
            .flex()
            .items_center()
            .gap_2()
            .max_w(px(360.))
            .px_3()
            .py_2()
            .rounded(px(RADIUS_POPOVER))
            .border_1()
            .border_color(t.border)
            .bg(t.elevated_surface)
            .shadow_lg()
            .text_color(t.text)
            .text_size(px(13.))
            .child(Icon::new(icon).color(color))
            .child(gpui::div().child(self.text))
            .when_some(self.action, |el, (id, label, handler)| {
                el.child(
                    Button::new(id, label)
                        .style(ButtonStyle::Subtle)
                        .on_click(handler),
                )
            })
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Full-width message bar (config error banner).
#[derive(IntoElement)]
pub struct Banner {
    id: ElementId,
    kind: ToastKind,
    text: SharedString,
    actions: Vec<(SharedString, ClickHandler)>,
    on_dismiss: Option<ClickHandler>,
}

impl Banner {
    pub fn new(id: impl Into<ElementId>, kind: ToastKind, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            kind,
            text: text.into(),
            actions: Vec::new(),
            on_dismiss: None,
        }
    }

    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.actions.push((label.into(), Box::new(handler)));
        self
    }

    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let color = match self.kind {
            ToastKind::Info => t.accent,
            ToastKind::Success => t.success,
            ToastKind::Warning => t.warning,
            ToastKind::Error => t.error,
        };
        let id = self.id.clone();
        let buttons: Vec<AnyElement> = self
            .actions
            .into_iter()
            .enumerate()
            .map(|(i, (label, handler))| {
                Button::new(super::child_id(&id, i), label)
                    .style(ButtonStyle::Subtle)
                    .on_click(handler)
                    .into_any_element()
            })
            .collect();
        gpui::div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .w_full()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(t.border)
            .bg(color.opacity(if t.is_dark { 0.18 } else { 0.14 }))
            .text_color(t.text)
            .text_size(px(12.))
            .child(Icon::new(IconName::TriangleAlert).color(color))
            .child(gpui::div().flex_1().overflow_hidden().child(self.text))
            .children(buttons)
            .when_some(self.on_dismiss, |el, handler| {
                el.child(
                    IconButton::new("banner-dismiss", IconName::X)
                        .tooltip("Dismiss")
                        .on_click(handler),
                )
            })
    }
}
