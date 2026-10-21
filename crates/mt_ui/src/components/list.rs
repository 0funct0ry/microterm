use std::rc::Rc;

use gpui::{
    prelude::*, px, AnyElement, App, ElementId, FontWeight, HighlightStyle, SharedString,
    StyledText, Window,
};

use super::{theme, Icon, KeyBinding, StatusDot, StatusKind, TextInput};
use crate::{
    assets::IconName,
    metrics::{LIST_ROW_HEIGHT, RADIUS_CONTROL},
};

/// A 28 px list row: optional leading element, label, muted detail, trailing element.
#[derive(IntoElement)]
pub struct ListItem {
    id: ElementId,
    selected: bool,
    start: Option<AnyElement>,
    label: SharedString,
    highlights: Vec<u32>,
    detail: Option<SharedString>,
    end: Option<AnyElement>,
    on_click: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            start: None,
            label: label.into(),
            highlights: Vec::new(),
            detail: None,
            end: None,
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn start(mut self, el: impl IntoElement) -> Self {
        self.start = Some(el.into_any_element());
        self
    }

    pub fn end(mut self, el: impl IntoElement) -> Self {
        self.end = Some(el.into_any_element());
        self
    }

    /// Character positions (as reported by nucleo) to emphasise in the label.
    pub fn highlights(mut self, positions: Vec<u32>) -> Self {
        self.highlights = positions;
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

/// Converts char positions into merged byte ranges of `text`.
pub(crate) fn byte_ranges(text: &str, positions: &[u32]) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let offsets: Vec<(usize, char)> = text.char_indices().collect();
    for &p in positions {
        let Some(&(start, ch)) = offsets.get(p as usize) else {
            continue;
        };
        let end = start + ch.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

impl RenderOnce for ListItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let hover = t.element_hover;
        let highlight = HighlightStyle {
            color: Some(t.accent),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let label = StyledText::new(self.label.clone()).with_highlights(
            byte_ranges(&self.label, &self.highlights)
                .into_iter()
                .map(|r| (r, highlight)),
        );
        gpui::div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .w_full()
            .h(px(LIST_ROW_HEIGHT))
            .px_2()
            .rounded(px(RADIUS_CONTROL))
            .text_size(px(13.))
            .text_color(t.text)
            .cursor_pointer()
            .when(self.selected, |el| el.bg(t.element_selected))
            .when(!self.selected, |el| el.hover(move |s| s.bg(hover)))
            .when_some(self.start, |el, start| el.child(start))
            .child(gpui::div().flex_none().child(label))
            .when_some(self.detail, |el, detail| {
                el.child(
                    gpui::div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .text_color(t.text_muted)
                        .child(detail),
                )
            })
            .child(gpui::div().flex_1())
            .when_some(self.end, |el, end| el.child(end))
            .when_some(self.on_click, |el, handler| {
                el.on_click(move |_, window, cx| handler(window, cx))
            })
    }
}

/// What the owner of a picker hands over for each visible row.
pub struct PickerRow {
    pub label: String,
    pub positions: Vec<u32>,
    pub detail: Option<String>,
    /// Pre-formatted keybinding chip text, e.g. `⌘⇧P`.
    pub chip: Option<String>,
    pub icon: Option<IconName>,
    pub status: Option<StatusKind>,
    /// A color dot before the label (a tab color); a transparent one draws as a hollow ring.
    pub swatch: Option<gpui::Hsla>,
}

impl PickerRow {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            positions: Vec::new(),
            detail: None,
            chip: None,
            icon: None,
            status: None,
            swatch: None,
        }
    }
}

/// Query field plus a fuzzy-filtered list. Stateless: feed it rows from `PickerState`.
#[derive(IntoElement)]
pub struct Picker {
    id: ElementId,
    query: SharedString,
    placeholder: SharedString,
    rows: Vec<PickerRow>,
    /// Index of the highlighted row within `rows`.
    selected: Option<usize>,
    empty_text: SharedString,
    on_click: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
}

impl Picker {
    pub fn new(id: impl Into<ElementId>, query: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            query: query.into(),
            placeholder: "Search…".into(),
            rows: Vec::new(),
            selected: None,
            empty_text: "No matches".into(),
            on_click: None,
        }
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn rows(mut self, rows: Vec<PickerRow>, selected: Option<usize>) -> Self {
        self.rows = rows;
        self.selected = selected;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Picker {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let on_click = self.on_click;
        let rows = self.rows.into_iter().enumerate().map(|(i, row)| {
            let mut item = ListItem::new(super::child_id(&self.id, i), row.label)
                .selected(self.selected == Some(i))
                .highlights(row.positions);
            if let Some(icon) = row.icon {
                item = item.start(Icon::new(icon).color(t.text_muted));
            }
            if let Some(status) = row.status {
                item = item.start(StatusDot::new(status));
            }
            if let Some(color) = row.swatch {
                item = item.start(
                    gpui::div()
                        .flex_none()
                        .size(px(10.))
                        .rounded_full()
                        .border_1()
                        .border_color(t.border)
                        .bg(color),
                );
            }
            if let Some(detail) = row.detail {
                item = item.detail(detail);
            }
            if let Some(chip) = row.chip {
                item = item.end(KeyBinding::from_text(chip));
            }
            if let Some(handler) = on_click.clone() {
                item = item.on_click(move |w, c| handler(i, w, c));
            }
            item
        });
        let empty = self.selected.is_none();
        gpui::div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                gpui::div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h(px(36.))
                    .border_b_1()
                    .border_color(t.border_variant)
                    .child(Icon::new(IconName::Search).color(t.text_placeholder))
                    .child(TextInput::new(self.query).placeholder(self.placeholder)),
            )
            .child(
                gpui::div()
                    .flex()
                    .flex_col()
                    .p_1()
                    .children(rows)
                    .when(empty, |el| {
                        el.child(
                            gpui::div()
                                .h(px(LIST_ROW_HEIGHT))
                                .px_2()
                                .flex()
                                .items_center()
                                .text_size(px(13.))
                                .text_color(t.text_placeholder)
                                .child(self.empty_text),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::byte_ranges;

    #[test]
    fn merges_adjacent_positions() {
        assert_eq!(byte_ranges("Select theme", &[0, 1, 2, 7]), vec![0..3, 7..8]);
    }

    #[test]
    fn handles_multibyte_chars() {
        assert_eq!(byte_ranges("é–a", &[0, 2]), vec![0..2, 5..6]);
    }

    #[test]
    fn ignores_out_of_range_positions() {
        assert!(byte_ranges("ab", &[5]).is_empty());
    }
}
