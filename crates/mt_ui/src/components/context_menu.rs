//! The context-menu popup (SPEC §10.9.1): id-keyed rows, separators, headers, one level of
//! submenus, icons, key chips, a type-to-filter line and `max_rows` windowing. State and
//! keyboard handling live in [`crate::menu::nav::MenuNav`]; this only draws it and reports
//! pointer input.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, App, ElementId, MouseButton, Pixels, Point, ScrollDelta, SharedString,
    Window,
};

use super::{theme, Icon, KeyBinding};
use crate::assets::IconName;
use crate::menu::nav::MenuNav;
use crate::menu::{MenuRow, RowKind};
use crate::metrics::{LIST_ROW_HEIGHT, RADIUS_CONTROL, RADIUS_POPOVER};

pub const MENU_WIDTH: f32 = 240.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const PADDING: f32 = 4.0;
const FILTER_HEIGHT: f32 = 26.0;
/// Swatch grid cell size and the circle drawn in it (SPEC §4.4).
const SWATCH_PITCH: f32 = 22.0;
const SWATCH_SIZE: f32 = 16.0;

type Cb<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// Height of a row in pixels.
pub fn row_height(r: &MenuRow) -> f32 {
    match r.kind {
        RowKind::Separator => SEPARATOR_HEIGHT,
        RowKind::Swatches { .. } => SWATCH_PITCH,
        _ => LIST_ROW_HEIGHT,
    }
}

/// Where a card of `height` placed at `at` must go to stay inside `bounds` (`w`, `h`).
pub fn clamp_card(at: (f32, f32), size: (f32, f32), bounds: (f32, f32)) -> (f32, f32) {
    let x = if at.0 + size.0 > bounds.0 {
        (bounds.0 - size.0).max(0.0)
    } else {
        at.0
    };
    let y = if at.1 + size.1 > bounds.1 {
        (bounds.1 - size.1).max(0.0)
    } else {
        at.1
    };
    (x, y)
}

/// A popup menu at a window position. Clicking anywhere else dismisses it.
#[derive(IntoElement)]
pub struct ContextMenu {
    id: ElementId,
    position: Point<Pixels>,
    nav: MenuNav,
    chips: HashMap<String, String>,
    palette: std::collections::BTreeMap<String, String>,
    on_activate: Option<Cb<String>>,
    on_hover: Option<Cb<Option<usize>>>,
    on_hover_child: Option<Cb<Option<usize>>>,
    on_scroll: Option<Cb<isize>>,
    on_dismiss: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl ContextMenu {
    pub fn new(id: impl Into<ElementId>, position: Point<Pixels>, nav: MenuNav) -> Self {
        Self {
            id: id.into(),
            position,
            nav,
            chips: HashMap::new(),
            palette: Default::default(),
            on_activate: None,
            on_hover: None,
            on_hover_child: None,
            on_scroll: None,
            on_dismiss: None,
        }
    }

    /// Chip text per row id.
    pub fn chips(mut self, chips: HashMap<String, String>) -> Self {
        self.chips = chips;
        self
    }

    /// `[tab_bar.palette]`, for swatch colors.
    pub fn palette(mut self, palette: std::collections::BTreeMap<String, String>) -> Self {
        self.palette = palette;
        self
    }

    pub fn on_activate(mut self, f: impl Fn(String, &mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Rc::new(f));
        self
    }

    pub fn on_hover(mut self, f: impl Fn(Option<usize>, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(f));
        self
    }

    pub fn on_hover_child(
        mut self,
        f: impl Fn(Option<usize>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_hover_child = Some(Rc::new(f));
        self
    }

    pub fn on_scroll(mut self, f: impl Fn(isize, &mut Window, &mut App) + 'static) -> Self {
        self.on_scroll = Some(Rc::new(f));
        self
    }

    pub fn on_dismiss(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(f));
        self
    }
}

struct RowStyle {
    highlighted: bool,
    open: bool,
}

impl ContextMenu {
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        id: ElementId,
        r: &MenuRow,
        style: RowStyle,
        on_click: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
        on_enter: Rc<dyn Fn(&mut Window, &mut App)>,
        cx: &App,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        match &r.kind {
            RowKind::Swatches { items, selected } => {
                return self.swatch_row(id, r, items, selected, style.highlighted, cx)
            }
            RowKind::Separator => {
                return div()
                    .id(id)
                    .h(px(SEPARATOR_HEIGHT))
                    .flex()
                    .items_center()
                    .child(div().h(px(1.)).w_full().bg(t.border_variant))
                    .into_any_element()
            }
            RowKind::Header => {
                return div()
                    .id(id)
                    .h(px(LIST_ROW_HEIGHT))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child(SharedString::from(r.label.to_uppercase()))
                    .into_any_element()
            }
            _ => {}
        }
        let enabled = r.enabled;
        let is_sub = matches!(r.kind, RowKind::Submenu(_));
        let icon = r.icon.as_deref().and_then(IconName::from_name);
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .h(px(LIST_ROW_HEIGHT))
            .px_2()
            .rounded(px(RADIUS_CONTROL))
            .when(style.highlighted || style.open, |el| el.bg(t.element_hover))
            .when(enabled, |el| el.cursor_pointer())
            .when(!enabled, |el| el.text_color(t.text_placeholder))
            .on_hover(move |hovered, window, cx| {
                if *hovered {
                    on_enter(window, cx);
                }
            })
            .when_some(icon, |el, i| el.child(Icon::new(i).size(px(14.))))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(SharedString::from(r.label.clone())),
            )
            .when_some(self.chips.get(&r.id).cloned(), |el, chip| {
                el.child(KeyBinding::from_text(chip))
            })
            .when(is_sub, |el| {
                el.child(Icon::new(IconName::ChevronRight).size(px(12.)))
            })
            .when_some(on_click.filter(|_| enabled && !is_sub), |el, f| {
                el.on_click(move |_, window, cx| f(window, cx))
            })
            .into_any_element()
    }
}

impl ContextMenu {
    /// One row of the tab-color grid: round swatches, a hollow ring first, a check on the
    /// selected one, a hover ring and a tooltip with the color name.
    fn swatch_row(
        &self,
        id: ElementId,
        r: &MenuRow,
        items: &[crate::tab_color::Swatch],
        selected: &str,
        highlighted: bool,
        cx: &App,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let cells = items.iter().enumerate().map(|(i, sw)| {
            let rgba = sw
                .color
                .map(|c| crate::tab_color::resolve_rgba(&t.tab_ui, &self.palette, c));
            let is_selected = sw.id == selected;
            let focused = highlighted && self.nav.col.min(items.len() - 1) == i;
            let act = self.on_activate.clone();
            let swid = format!("color:{}", sw.id);
            let label = SharedString::from(sw.label.clone());
            let tip = if is_selected {
                SharedString::from(format!("{}, selected", sw.label))
            } else {
                label.clone()
            };
            let enabled = r.enabled;
            div()
                .id((id.clone(), SharedString::from(format!("sw-{}", sw.id))))
                .size(px(SWATCH_PITCH))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .when(enabled, |d| d.cursor_pointer())
                .child(
                    div()
                        .size(px(SWATCH_SIZE))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_2()
                        .border_color(if focused {
                            t.text
                        } else {
                            gpui::transparent_black()
                        })
                        .hover(move |s| s.border_color(t.text_muted))
                        .map(|d| match rgba {
                            Some(c) => d.bg(crate::theme::hsla(c)),
                            None => d.border_1().border_color(t.border),
                        })
                        .when(is_selected, |d| {
                            let white = rgba.map_or(!t.is_dark, crate::tab_color::check_is_white);
                            d.child(Icon::new(IconName::Check).size(px(10.)).color(if white {
                                gpui::white()
                            } else {
                                gpui::black()
                            }))
                        }),
                )
                .tooltip(move |_, cx| cx.new(|_| super::Tooltip::new(tip.clone())).into())
                .when(enabled, |d| {
                    d.on_click(move |_, w, cx| {
                        if let Some(a) = &act {
                            a(swid.clone(), w, cx);
                        }
                    })
                })
        });
        div()
            .id(id.clone())
            .h(px(SWATCH_PITCH))
            .px_2()
            .flex()
            .items_center()
            .children(cells)
            .into_any_element()
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let bounds = window.viewport_size();
        let bounds = (f32::from(bounds.width), f32::from(bounds.height));
        let vis = self.nav.visible();
        let start = self.nav.scroll.min(vis.len());
        let end = (start + self.nav.max_rows).min(vis.len());
        let shown: Vec<_> = vis[start..end].to_vec();
        let filter_h = if self.nav.filter.is_empty() {
            0.0
        } else {
            FILTER_HEIGHT
        };
        let rows_h: f32 = shown
            .iter()
            .filter_map(|&at| self.nav.row(at))
            .map(row_height)
            .sum();
        let height = rows_h + filter_h + PADDING * 2.0 + 2.0;
        let (x, y) = clamp_card(
            (f32::from(self.position.x), f32::from(self.position.y)),
            (MENU_WIDTH, height),
            bounds,
        );

        let mut card_rows = Vec::new();
        let mut sub_at = None;
        let mut offset = PADDING + 1.0 + filter_h;
        for (k, &at) in shown.iter().enumerate() {
            let i = start + k;
            let Some(r) = self.nav.row(at) else { continue };
            let open = self.nav.filter.is_empty()
                && at.1.is_none()
                && self.nav.sub.is_some_and(|(top, _)| top == at.0);
            if open {
                sub_at = Some((at.0, offset));
            }
            let hover = self.on_hover.clone();
            let on_enter: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |w, cx| {
                if let Some(h) = &hover {
                    h(Some(i), w, cx);
                }
            });
            let on_click = self.nav.activate(at).map(|item| {
                let act = self.on_activate.clone();
                Rc::new(move |w: &mut Window, cx: &mut App| {
                    if let Some(a) = &act {
                        a(item.clone(), w, cx);
                    }
                }) as Rc<dyn Fn(&mut Window, &mut App)>
            });
            card_rows.push(self.row(
                super::child_id(&self.id, i),
                r,
                RowStyle {
                    highlighted: self.nav.highlight == Some(i),
                    open,
                },
                on_click,
                on_enter,
                cx,
            ));
            offset += row_height(r);
        }

        let card = |id: ElementId, x: f32, y: f32| {
            div()
                .id(id)
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(MENU_WIDTH))
                .flex()
                .flex_col()
                .p(px(PADDING))
                .rounded(px(RADIUS_POPOVER))
                .border_1()
                .border_color(t.border)
                .bg(t.elevated_surface)
                .shadow_lg()
                .text_size(px(13.))
                .text_color(t.text)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        };

        let scroll = self.on_scroll.clone();
        let mut main = card(self.id.clone(), x, y).on_scroll_wheel(move |ev, w, cx| {
            let dy = match ev.delta {
                ScrollDelta::Lines(l) => l.y,
                ScrollDelta::Pixels(p) => f32::from(p.y) / LIST_ROW_HEIGHT,
            };
            if let Some(s) = &scroll {
                if dy.abs() >= 0.5 {
                    s(if dy > 0.0 { -1 } else { 1 }, w, cx);
                }
            }
            cx.stop_propagation();
        });
        if !self.nav.filter.is_empty() {
            main = main.child(
                div()
                    .h(px(FILTER_HEIGHT))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(t.text_muted)
                    .child(Icon::new(IconName::Search).size(px(12.)))
                    .child(SharedString::from(self.nav.filter.clone())),
            );
        }
        let main = main.children(card_rows);

        // The open submenu, beside its row (or on the left when there is no room).
        let sub = sub_at.and_then(|(top, row_y)| {
            let RowKind::Submenu(kids) = &self.nav.rows.get(top)?.kind else {
                return None;
            };
            let h: f32 = kids.iter().map(row_height).sum::<f32>() + PADDING * 2.0 + 2.0;
            let want_x = x + MENU_WIDTH - 4.0;
            let sx = if want_x + MENU_WIDTH > bounds.0 {
                (x - MENU_WIDTH + 4.0).max(0.0)
            } else {
                want_x
            };
            let (_, sy) = clamp_card((sx, y + row_y - PADDING - 1.0), (MENU_WIDTH, h), bounds);
            let child_hl = self.nav.sub.and_then(|(_, c)| c);
            let sub_id: ElementId = (self.id.clone(), SharedString::from("sub")).into();
            let rows = kids.iter().enumerate().map(|(j, r)| {
                let hover = self.on_hover_child.clone();
                let on_enter: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |w, cx| {
                    if let Some(h) = &hover {
                        h(Some(j), w, cx);
                    }
                });
                let on_click = self.nav.activate((top, Some(j))).map(|item| {
                    let act = self.on_activate.clone();
                    Rc::new(move |w: &mut Window, cx: &mut App| {
                        if let Some(a) = &act {
                            a(item.clone(), w, cx);
                        }
                    }) as Rc<dyn Fn(&mut Window, &mut App)>
                });
                self.row(
                    super::child_id(&sub_id, j),
                    r,
                    RowStyle {
                        highlighted: child_hl == Some(j),
                        open: false,
                    },
                    on_click,
                    on_enter,
                    cx,
                )
            });
            Some(card(sub_id.clone(), sx, sy).children(rows.collect::<Vec<_>>()))
        });

        let dismiss_left = self.on_dismiss.clone();
        let dismiss_right = self.on_dismiss.clone();
        div()
            .id((self.id.clone(), SharedString::from("backdrop")))
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
            .child(main)
            .children(sub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_stay_inside_the_window() {
        assert_eq!(
            clamp_card((10.0, 10.0), (240.0, 100.0), (800.0, 600.0)),
            (10.0, 10.0)
        );
        assert_eq!(
            clamp_card((700.0, 550.0), (240.0, 100.0), (800.0, 600.0)),
            (560.0, 500.0)
        );
        assert_eq!(
            clamp_card((5.0, 5.0), (900.0, 700.0), (800.0, 600.0)),
            (0.0, 0.0)
        );
    }
}
