use std::rc::Rc;

use gpui::{
    prelude::*, px, AnyElement, App, ClickEvent, ElementId, MouseButton, MouseDownEvent,
    SharedString, Window,
};

use super::{theme, IconButton, StatusDot, StatusKind, Tooltip};
use crate::{
    assets::IconName,
    metrics::{RADIUS_CONTROL, TAB_BAR_HEIGHT, TAB_MAX_WIDTH, TAB_MIN_WIDTH},
    tab_chrome::TabChrome,
};

type MouseHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// Drag payload while a tab is being reordered.
#[derive(Clone)]
pub struct DraggedTab {
    pub id: u64,
    pub title: SharedString,
}

/// The ghost shown under the pointer while dragging a tab.
struct TabGhost(SharedString);

impl Render for TabGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        gpui::div()
            .px_3()
            .h(px(TAB_BAR_HEIGHT))
            .flex()
            .items_center()
            .bg(t.background)
            .border_1()
            .border_color(t.border)
            .text_size(px(13.))
            .text_color(t.text)
            .child(self.0.clone())
    }
}

/// One Zed-style tab. The active tab uses `background` with no bottom border; inactive tabs sit
/// on `surface` with muted text. The close button shows on hover or on the active tab.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    tab_id: u64,
    title: SharedString,
    active: bool,
    dirty: bool,
    chrome: TabChrome,
    /// Inline rename: shows this text with a caret instead of the title.
    editing: Option<SharedString>,
    drag: bool,
    on_drop: Option<Rc<dyn Fn(u64, &mut Window, &mut App)>>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    on_close: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    on_middle_click: Option<MouseHandler>,
    on_right_click: Option<MouseHandler>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            tab_id: 0,
            title: title.into(),
            active: false,
            dirty: false,
            chrome: TabChrome::default(),
            editing: None,
            drag: false,
            on_drop: None,
            on_click: None,
            on_close: None,
            on_middle_click: None,
            on_right_click: None,
        }
    }

    /// The id carried in the drag payload (the mux tab id).
    pub fn tab_id(mut self, id: u64) -> Self {
        self.tab_id = id;
        self
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

    /// Status dot, progress line, tooltip and attention badge (`ctl pane status|notify`).
    pub fn chrome(mut self, chrome: TabChrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Shows an inline text editor (rename) in place of the title.
    pub fn editing(mut self, text: Option<SharedString>) -> Self {
        self.editing = text;
        self
    }

    /// Lets the tab be dragged to reorder.
    pub fn draggable(mut self) -> Self {
        self.drag = true;
        self
    }

    /// Called with the dragged tab's id when another tab is dropped on this one.
    pub fn on_drop_tab(mut self, handler: impl Fn(u64, &mut Window, &mut App) + 'static) -> Self {
        self.on_drop = Some(Rc::new(handler));
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
        let drag_payload = DraggedTab {
            id: self.tab_id,
            title: self.title.clone(),
        };
        let drop = self.on_drop;
        let drag_over = t.element_hover;
        let editing = self.editing.clone();
        let chrome = self.chrome;
        let progress_color = chrome.dot.map_or(t.accent, |k| kind_color(&t, k));
        // Tab color (SPEC §4.4): bar = 2 px top line, tint = background mix, dot = 6 px dot.
        let paint = chrome
            .color
            .clone()
            .filter(|c| c.style != mt_config::TabColorStyle::None);
        let (show_bar, show_tint, show_dot) = paint.as_ref().map_or((false, false, false), |c| {
            use mt_config::TabColorStyle::*;
            (
                matches!(c.style, Bar | BarTint),
                matches!(c.style, Tint | BarTint),
                c.style == Dot,
            )
        });
        let color_hsla = paint.as_ref().map(|c| crate::theme::hsla(c.rgba));
        let (bg, hover) = match (&paint, show_tint) {
            (Some(c), true) => (
                crate::theme::hsla(crate::tab_color::tinted_background(
                    &t.tab_ui,
                    c.rgba,
                    self.active,
                    false,
                )),
                crate::theme::hsla(crate::tab_color::tinted_background(
                    &t.tab_ui,
                    c.rgba,
                    self.active,
                    true,
                )),
            ),
            _ => (if self.active { t.background } else { t.surface }, hover),
        };
        let ring = chrome.dot_ring.then_some(bg);
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
            .when(self.active, |el| el.bg(bg).text_color(t.text))
            .when(!self.active, |el| {
                el.bg(bg)
                    .text_color(t.text_muted)
                    .border_b_1()
                    .hover(move |s| s.bg(hover))
            })
            .when(show_bar, |el| {
                el.child(
                    gpui::div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .rounded_t(px(RADIUS_CONTROL))
                        .bg(color_hsla.unwrap_or(t.accent))
                        .opacity(if self.active { 1.0 } else { 0.6 }),
                )
            })
            .when(show_dot, |el| {
                el.child(
                    gpui::div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(color_hsla.unwrap_or(t.accent)),
                )
            })
            // Recording indicator (SPEC §10.10.2); coexists with tab color, status dot and badge.
            .when(chrome.recording.is_some(), |el| {
                el.child(
                    gpui::div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(t.error)
                        .id("tab-rec-dot"),
                )
            })
            // A status dot replaces the dirty dot: both are "something is running here".
            .when_some(chrome.dot, |el, kind| {
                el.child(StatusDot::new(kind).ring(ring))
            })
            .when(self.dirty && chrome.dot.is_none(), |el| {
                el.child(StatusDot::new(StatusKind::Accent))
            })
            .when_some(chrome.tooltip.clone(), |el, text| {
                let text = SharedString::from(text);
                el.tooltip(move |_, cx| cx.new(|_| Tooltip::new(text.clone())).into())
            })
            .child(match editing {
                Some(text) => gpui::div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(t.text)
                    .child(text)
                    .child(gpui::div().w(px(1.)).h(px(14.)).ml(px(1.)).bg(t.accent)),
                None => gpui::div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.title),
            })
            .when_some(chrome.counter.clone(), |el, text| {
                el.child(
                    gpui::div()
                        .flex_none()
                        .px(px(5.))
                        .rounded_full()
                        .bg(t.element_hover)
                        .text_size(px(10.))
                        .text_color(t.text)
                        .child(text),
                )
            })
            .when(chrome.zoomed, |el| {
                el.child(
                    crate::components::Icon::new(crate::assets::IconName::Maximize)
                        .size(px(11.))
                        .color(t.text_muted),
                )
            })
            .when_some(chrome.badge, |el, kind| {
                el.child(StatusDot::new(kind).ring(ring))
            })
            .when(chrome.indeterminate, |el| {
                el.child(
                    gpui::div()
                        .absolute()
                        .left_0()
                        .bottom_0()
                        .h(px(2.))
                        .w_full()
                        .opacity(0.45)
                        .bg(progress_color),
                )
            })
            .when_some(chrome.progress, |el, pct| {
                el.child(
                    gpui::div()
                        .absolute()
                        .left_0()
                        .bottom_0()
                        .h(px(2.))
                        .w(gpui::relative(f32::from(pct.min(100)) / 100.))
                        .bg(progress_color),
                )
            })
            .when(self.drag, |el| {
                el.on_drag(drag_payload.clone(), |dragged, _, _, cx| {
                    let title = dragged.title.clone();
                    cx.new(|_| TabGhost(title))
                })
                .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(drag_over))
            })
            .when_some(drop, |el, handler| {
                el.on_drop(move |dragged: &DraggedTab, w, c| handler(dragged.id, w, c))
            })
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

fn kind_color(t: &crate::theme::Theme, kind: StatusKind) -> gpui::Hsla {
    match kind {
        StatusKind::Connected | StatusKind::Success => t.success,
        StatusKind::Reconnecting | StatusKind::Warning => t.warning,
        StatusKind::Error => t.error,
        StatusKind::Idle => t.text_placeholder,
        StatusKind::Accent => t.accent,
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
