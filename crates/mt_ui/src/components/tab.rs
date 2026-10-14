use std::rc::Rc;

use gpui::{
    prelude::*, px, AnyElement, App, ClickEvent, ElementId, MouseButton, MouseDownEvent,
    SharedString, Window,
};

use super::{theme, IconButton, StatusDot, StatusKind};
use crate::{
    assets::IconName,
    metrics::{TAB_BAR_HEIGHT, TAB_MAX_WIDTH, TAB_MIN_WIDTH},
};

type MouseHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// One Zed-style tab. The active tab uses `background` with no bottom border; inactive tabs sit
/// on `surface` with muted text. The close button shows on hover or on the active tab.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    title: SharedString,
    active: bool,
    dirty: bool,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    on_close: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    on_middle_click: Option<MouseHandler>,
    on_right_click: Option<MouseHandler>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            active: false,
            dirty: false,
            on_click: None,
            on_close: None,
            on_middle_click: None,
            on_right_click: None,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Shows the dot for "a foreground process other than the shell is running".
    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }

    pub fn on_middle_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_middle_click = Some(Rc::new(handler));
        self
    }

    pub fn on_right_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_right_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let group: SharedString = format!("tab-{:?}", self.id).into();
        let hover = t.element_hover;
        let close = self.on_close;
        let middle = self.on_middle_click;
        let right = self.on_right_click;
        gpui::div()
            .id(self.id)
            .group(group.clone())
            .relative()
            .flex()
            .flex_1()
            .items_center()
            .gap_2()
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .h(px(TAB_BAR_HEIGHT))
            .px_3()
            .border_r_1()
            .border_color(t.border_variant)
            .text_size(px(13.))
            .cursor_pointer()
            .when(self.active, |el| el.bg(t.background).text_color(t.text))
            .when(!self.active, |el| {
                el.bg(t.surface)
                    .text_color(t.text_muted)
                    .border_b_1()
                    .hover(move |s| s.bg(hover))
            })
            .when(self.dirty, |el| {
                el.child(StatusDot::new(StatusKind::Accent))
            })
            .child(
                gpui::div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.title),
            )
            .when_some(close, |el, close| {
                let button = IconButton::new("tab-close", IconName::X)
                    .on_click(move |e, w, c| close(e, w, c));
                el.child(
                    gpui::div()
                        .flex_none()
                        .when(!self.active, |d| d.invisible())
                        .group_hover(group.clone(), |s| s.visible())
                        .child(button),
                )
            })
            .when_some(self.on_click, |el, handler| {
                el.on_click(move |e, w, c| handler(e, w, c))
            })
            .when_some(middle, |el, handler| {
                el.on_mouse_down(MouseButton::Middle, move |e, w, c| handler(e, w, c))
            })
            .when_some(right, |el, handler| {
                el.on_mouse_down(MouseButton::Right, move |e, w, c| handler(e, w, c))
            })
    }
}

/// The 32 px strip holding the tabs and the trailing `+` button.
#[derive(IntoElement)]
pub struct TabBar {
    tabs: Vec<AnyElement>,
    trailing: Option<AnyElement>,
}

impl TabBar {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            trailing: None,
        }
    }

    pub fn tab(mut self, tab: impl IntoElement) -> Self {
        self.tabs.push(tab.into_any_element());
        self
    }

    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing = Some(el.into_any_element());
        self
    }
}

impl Default for TabBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        gpui::div()
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(px(TAB_BAR_HEIGHT))
            .bg(t.surface)
            .child(
                gpui::div()
                    .flex()
                    .flex_none()
                    .h_full()
                    .overflow_hidden()
                    .max_w_full()
                    .children(self.tabs),
            )
            .when_some(self.trailing, |el, trailing| {
                el.child(gpui::div().flex().items_center().px_1().child(trailing))
            })
            .child(
                gpui::div()
                    .flex_1()
                    .h_full()
                    .border_b_1()
                    .border_color(t.border_variant),
            )
    }
}
