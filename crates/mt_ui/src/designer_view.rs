//! Workspace designer overlay: AppWindow glue and rendering (SPEC §17.1.1). The model lives in
//! `designer.rs`; this file opens it, routes keys and draws the modal.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    prelude::*, px, relative, AnyElement, Context, FontWeight, IntoElement, KeyDownEvent, Window,
};
use mt_config::{SplitKind, WorkspacePane, WorkspaceTab};

use crate::{
    app::{typed_text, AppWindow},
    components::{Button, ButtonStyle, Modal, ToastKind},
    designer::{Designer, Field},
    metrics::{PALETTE_TOP_OFFSET, RADIUS_CONTROL, TITLE_BAR_HEIGHT},
    overlay_stack::OverlayKind,
    settings::Settings,
    theme::Theme,
    view::DismissExt,
};

/// Clipboard-style actions that must act on a designer text field instead of the terminal.
#[derive(Clone, Copy)]
pub(crate) enum Clip {
    Copy,
    Paste,
    SelectAll,
}

impl AppWindow {
    /// While the designer is open its text field owns Copy / Paste / Select all. Returns true
    /// when the action was consumed (also when no field is active, so nothing leaks to the pane).
    pub(crate) fn designer_clipboard(&mut self, clip: Clip, cx: &mut Context<Self>) -> bool {
        if self.overlays.top() != Some(crate::overlay_stack::OverlayKind::WorkspaceDesigner) {
            return false;
        }
        if let Some(d) = self.designer.as_mut().filter(|d| d.field != Field::None) {
            match clip {
                Clip::Copy => {
                    if let Some(t) = d.edit.selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                    }
                }
                Clip::Paste => {
                    if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                        d.edit.insert(&t);
                        d.commit_field();
                    }
                }
                Clip::SelectAll => d.edit.select_all(),
            }
            cx.notify();
        }
        true
    }
}

impl AppWindow {
    /// Opens the designer on `select` (a workspace name) or the first workspace.
    pub(crate) fn open_designer(
        &mut self,
        select: Option<String>,
        seed: Option<(String, WorkspaceTab)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = cx.global::<Settings>().path.clone();
        let src = std::fs::read_to_string(&path).unwrap_or_default();
        let d = match seed {
            Some((name, tab)) => Designer::from_layout(src, &name, tab),
            None => Designer::open(src, select.as_deref()),
        };
        self.designer = Some(d);
        self.open_overlay(OverlayKind::WorkspaceDesigner, window, cx);
    }

    /// The live pane tree of the active tab as a declared tab (cwd and ratios; commands are not
    /// readable from a running pane, so they stay empty).
    pub(crate) fn current_layout_tab(&self, cx: &mut Context<Self>) -> Option<WorkspaceTab> {
        use mt_mux::{Axis, PaneTree};
        fn conv(t: &PaneTree) -> WorkspacePane {
            match t {
                PaneTree::Leaf(p) => WorkspacePane {
                    cwd: p.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                PaneTree::Split { axis, ratio, a, b } => WorkspacePane {
                    split: Some(match axis {
                        Axis::Horizontal => SplitKind::Horizontal,
                        Axis::Vertical => SplitKind::Vertical,
                    }),
                    percent: Some(((ratio * 100.0).round() as i32).clamp(1, 99) as u8)
                        .filter(|p| *p != 50),
                    panes: vec![conv(a), conv(b)],
                    ..Default::default()
                },
            }
        }
        let host = crate::mux_host::host(cx);
        let tab = host.mux.window(self.mux_window)?.active_tab()?;
        let root = conv(&tab.tree);
        Some(WorkspaceTab {
            title: tab.title_override.clone(),
            cwd: root.cwd,
            command: vec![],
            split: root.split,
            percent: root.percent,
            panes: root.panes,
        })
    }

    pub(crate) fn save_layout_as_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.current_layout_tab(cx) else {
            return;
        };
        let name = crate::mux_host::host(cx)
            .mux
            .active_workspace()
            .name
            .clone();
        self.open_designer(None, Some((format!("{name} layout"), tab)), window, cx);
    }

    /// Esc inside the designer: leave a field, then ask before discarding edits, then close.
    /// Returns true when Esc was consumed.
    pub(crate) fn designer_escape(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.designer.as_mut() else {
            return false;
        };
        if d.field != Field::None {
            d.field = Field::None;
        } else if d.dirty() && !d.confirm_discard {
            d.confirm_discard = true;
        } else {
            return false;
        }
        cx.notify();
        true
    }

    pub(crate) fn designer_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = cx.global::<Settings>().path.clone();
        let Some(d) = self.designer.as_mut() else {
            return;
        };
        if !d.can_save() {
            return;
        }
        if !path.exists() {
            let _ = std::fs::write(&path, "");
            d.opened_hash = mt_config::edit::content_hash("");
        }
        let existing: Vec<String> = mt_config::edit::read_workspaces(&d.src)
            .unwrap_or_default()
            .into_iter()
            .map(|w| w.name)
            .collect();
        let edits_existing = d.workspaces.iter().any(|w| {
            existing.contains(&w.name)
                && !mt_config::edit::read_workspaces(&d.src)
                    .unwrap_or_default()
                    .contains(w)
        });
        let removed: Vec<String> = existing
            .iter()
            .filter(|n| !d.workspaces.iter().any(|w| &w.name == *n))
            .cloned()
            .collect();
        match d.save(&path) {
            Ok(_) => {
                self.close_overlay(OverlayKind::WorkspaceDesigner, true, window, cx);
                crate::settings::reload(cx);
                crate::mux_host::sync_declared(cx);
                // A deleted declaration drops its workspace only if it never started; a running
                // workspace keeps running (SPEC §8.5).
                let host = crate::mux_host::host(cx);
                for name in removed {
                    let unstarted = host
                        .mux
                        .workspace_by_name(&name)
                        .is_some_and(|w| host.mux.counts(w.id).0 == 0);
                    if unstarted {
                        let _ = host.mux.delete_workspace(&name);
                    }
                }
                let msg = if edits_existing {
                    "Workspaces saved. Edits to existing workspaces apply when their windows are next created."
                } else {
                    "Workspaces saved"
                };
                self.toast(ToastKind::Success, msg, cx);
            }
            Err(e) => self.toast(ToastKind::Error, e.to_string(), cx),
        }
    }

    pub(crate) fn designer_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let platform = self.platform;
        let m = ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        let cmd = m.platform || m.control;
        let Some(d) = self.designer.as_mut() else {
            return;
        };
        if d.confirm_discard && key == "enter" {
            d.confirm_discard = false;
            cx.notify();
            return;
        }
        if cmd && key == "s" {
            self.designer_save(window, cx);
            return;
        }
        if d.field != Field::None {
            let ext = m.shift;
            match key {
                "enter" => d.field = Field::None,
                "backspace" if m.alt => d.edit.delete_word_back(),
                "backspace" => d.edit.backspace(),
                "delete" => d.edit.delete_forward(),
                "left" if m.alt => d.edit.word_left_move(ext),
                "right" if m.alt => d.edit.word_right_move(ext),
                "left" => d.edit.left(ext),
                "right" => d.edit.right(ext),
                "home" => d.edit.home(ext),
                "end" => d.edit.end(ext),
                "a" if cmd => d.edit.select_all(),
                "x" if cmd => {
                    if let Some(t) = d.edit.take_selection() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(t));
                    }
                }
                "c" if cmd => {
                    if let Some(t) = d.edit.selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                    }
                }
                "tab" => d.cycle_field(m.shift),
                "v" if cmd => {
                    if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                        d.edit.insert(&t);
                    }
                }
                _ => match typed_text(ev, platform) {
                    Some(t) => d.edit.insert(t),
                    None => return,
                },
            }
            d.commit_field();
        } else if key == "tab" {
            d.cycle_field(m.shift);
        } else {
            let delta = match key {
                "left" | "up" => -5,
                "right" | "down" => 5,
                "enter" => {
                    self.designer_save(window, cx);
                    return;
                }
                _ => return,
            };
            if let Some((_, parent)) = d.sel.clone().split_last() {
                d.nudge_percent(parent, delta);
            }
        }
        cx.notify();
    }

    pub(crate) fn designer_modal(
        &mut self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let d = self.designer.as_ref()?;
        let entity = cx.entity();
        let act = move |f: fn(&mut Designer)| {
            let entity = entity.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                entity.update(cx, |this, cx| {
                    if let Some(d) = this.designer.as_mut() {
                        f(d);
                    }
                    cx.notify();
                });
            }
        };
        let issues = d.issues();
        let layouts = self.designer_layout.clone();
        let field_box = |label: &'static str,
                         field: Field,
                         text: String,
                         id: &'static str,
                         cx: &mut Context<Self>| {
            let shown = if d.field == field {
                FieldText {
                    text: d.edit.text().to_string(),
                    caret: Some(d.edit.caret()),
                    selection: d.edit.selection(),
                }
            } else {
                FieldText {
                    text,
                    caret: None,
                    selection: None,
                }
            };
            text_field(t, label, field, shown, id, layouts.clone(), cx)
        };
        let ws_list = div()
            .flex()
            .flex_col()
            .w(px(170.))
            .gap_1()
            .children(d.workspaces.iter().enumerate().map(|(i, w)| {
                let star = d.startup == w.name;
                div()
                    .id(("ws", i))
                    .flex()
                    .gap_1()
                    .px_2()
                    .h(px(24.))
                    .items_center()
                    .rounded(px(RADIUS_CONTROL))
                    .cursor_pointer()
                    .when(i == d.ws, |el| el.bg(t.element_selected))
                    .child(
                        div()
                            .id(("star", i))
                            .child(if star { "★" } else { "☆" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(d) = this.designer.as_mut() {
                                    d.toggle_startup(i);
                                }
                                cx.notify();
                            })),
                    )
                    .child(w.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.designer.as_mut() {
                            d.select_workspace(i);
                        }
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(Button::new("ws-new", "New").on_click(act(Designer::new_workspace)))
                    .child(
                        Button::new("ws-dup", "Duplicate")
                            .on_click(act(Designer::duplicate_workspace)),
                    )
                    .child(
                        Button::new("ws-del", "Delete").on_click(act(Designer::delete_workspace)),
                    ),
            );

        let tabs = div()
            .flex()
            .gap_1()
            .children(
                d.workspaces
                    .get(d.ws)
                    .into_iter()
                    .flat_map(|w| w.tabs.iter().enumerate())
                    .map(|(i, tab)| {
                        let label = tab
                            .title
                            .clone()
                            .unwrap_or_else(|| format!("Tab {}", i + 1));
                        Button::new(("tab", i), label)
                            .style(if i == d.tab {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(d) = this.designer.as_mut() {
                                    d.select_tab(i);
                                }
                                cx.notify();
                            }))
                    }),
            )
            .child(Button::new("tab-new", "+").on_click(act(Designer::new_tab)))
            .child(Button::new("tab-close", "Close tab").on_click(act(Designer::close_tab)));

        let canvas = match d.root() {
            Some(root) => canvas_node(t, &root, &mut Vec::new(), &d.sel, cx).into_any_element(),
            None => div().into_any_element(),
        };
        let depth = d.depth();
        let has_parent = !d.sel.is_empty();
        let toolbar = div()
            .flex()
            .gap_1()
            .items_center()
            .child(split_button(
                "split-r",
                "Split right",
                d.can_split(),
                SplitKind::Horizontal,
                cx,
            ))
            .child(split_button(
                "split-d",
                "Split down",
                d.can_split(),
                SplitKind::Vertical,
                cx,
            ))
            .child(enabled_if(
                has_parent,
                Button::new("close-pane", "Close pane").on_click(act(Designer::close_pane)),
            ))
            .child(enabled_if(
                has_parent,
                Button::new("swap", "Swap").on_click(act(Designer::swap)),
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.text_muted)
                    .child(format!("depth {depth}/{}", mt_config::MAX_SPLIT_DEPTH)),
            );

        let sel_pane = d.pane();
        let is_leaf = sel_pane.as_ref().is_some_and(|p| p.split.is_none());
        let inspector = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child(d.path_label()),
            )
            .when(is_leaf, |el| {
                el.child(field_box(
                    "cwd",
                    Field::Cwd,
                    sel_pane.clone().and_then(|p| p.cwd).unwrap_or_default(),
                    "f-cwd",
                    cx,
                ))
                .child(field_box(
                    "command",
                    Field::Command,
                    sel_pane
                        .clone()
                        .map(|p| p.command.join(" "))
                        .unwrap_or_default(),
                    "f-cmd",
                    cx,
                ))
            });

        let preview = d.show_preview.then(|| {
            div()
                .id("designer-preview")
                .max_h(px(140.))
                .overflow_y_scroll()
                .p_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(t.background)
                .text_size(px(11.))
                .children(d.preview().into_iter().map(|(tag, line)| {
                    let (mark, color) = match tag {
                        crate::designer::DiffTag::Same => (" ", t.text_muted),
                        crate::designer::DiffTag::Added => ("+", t.accent),
                        crate::designer::DiffTag::Removed => ("-", t.error),
                    };
                    div().text_color(color).child(format!("{mark} {line}"))
                }))
        });

        let errors = div().flex().flex_col().children(issues.iter().map(|i| {
            div()
                .text_size(px(11.))
                .text_color(t.error)
                .child(format!("{}: {}", i.path, i.message))
        }));
        let banner = d.read_only.as_ref().map(|e| {
            div()
                .text_color(t.error)
                .text_size(px(12.))
                .child(format!("Read-only: {e}"))
        });
        let discard = d.confirm_discard.then(|| {
            div()
                .text_color(t.warning)
                .text_size(px(12.))
                .child("Discard unsaved changes? Esc again to discard, Enter to keep editing.")
        });
        let warn = (d.comment_warning && d.dirty()).then(|| {
            div()
                .text_color(t.text_muted)
                .text_size(px(11.))
                .child("Comments inside rewritten [[workspaces]] blocks are not preserved; the previous file is kept as microterm.toml.bak.")
        });
        let can_save = d.can_save();

        Some(
            Modal::new("workspace-designer")
                .width(px(720.))
                .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
                .on_dismiss(cx.listener_for_dismiss())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_4()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Workspace designer"),
                        )
                        .children(banner)
                        .child(
                            div().flex().gap_3().child(ws_list).child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .gap_2()
                                    .child(field_box(
                                        "name",
                                        Field::Name,
                                        d.workspaces
                                            .get(d.ws)
                                            .map(|w| w.name.clone())
                                            .unwrap_or_default(),
                                        "f-name",
                                        cx,
                                    ))
                                    .child(field_box(
                                        "tab title",
                                        Field::TabTitle,
                                        d.tab_ref()
                                            .and_then(|t| t.title.clone())
                                            .unwrap_or_default(),
                                        "f-title",
                                        cx,
                                    ))
                                    .child(field_box(
                                        "tab cwd",
                                        Field::TabCwd,
                                        d.tab_ref().and_then(|t| t.cwd.clone()).unwrap_or_default(),
                                        "f-tcwd",
                                        cx,
                                    ))
                                    .child(tabs)
                                    .child(toolbar)
                                    .child(div().h(px(180.)).child(canvas))
                                    .child(inspector),
                            ),
                        )
                        .children(preview)
                        .child(errors)
                        .children(warn)
                        .children(discard)
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    Button::new("toggle-preview", "TOML preview")
                                        .on_click(act(|d| d.show_preview = !d.show_preview)),
                                )
                                .child(Button::new("cancel", "Cancel").on_click(cx.listener(
                                    |this, _, w, cx| {
                                        if let Some(d) = this.designer.as_mut() {
                                            d.field = Field::None;
                                        }
                                        this.dismiss_top(w, cx)
                                    },
                                )))
                                .child({
                                    let b = Button::new("save", "Save to config")
                                        .style(ButtonStyle::Filled);
                                    if can_save {
                                        b.on_click(
                                            cx.listener(|this, _, w, cx| this.designer_save(w, cx)),
                                        )
                                        .into_any_element()
                                    } else {
                                        enabled_if(false, b)
                                    }
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn div() -> gpui::Div {
    gpui::div()
}

/// Painted text of each designer field: its bounds and shaped line.
pub(crate) type FieldLayouts =
    Rc<RefCell<HashMap<Field, (gpui::Bounds<gpui::Pixels>, gpui::ShapedLine)>>>;

pub(crate) struct FieldText {
    text: String,
    /// `Some` while the field is being edited.
    caret: Option<usize>,
    selection: Option<(usize, usize)>,
}

impl AppWindow {
    /// Byte offset in `field`'s text under window x `x`.
    fn designer_index_at(&self, field: Field, x: gpui::Pixels) -> usize {
        match self.designer_layout.borrow().get(&field) {
            Some((bounds, line)) => line.closest_index_for_x(x - bounds.origin.x),
            None => usize::MAX,
        }
    }

    fn designer_mouse_down(
        &mut self,
        field: Field,
        ev: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let idx = self.designer_index_at(field, ev.position.x);
        let Some(d) = self.designer.as_mut() else {
            return;
        };
        let fresh = d.field != field;
        if fresh {
            d.begin_field(field);
        }
        match ev.click_count {
            2 => d.edit.select_word_at(idx),
            n if n >= 3 => d.edit.select_all(),
            _ => d.edit.set_caret(idx, ev.modifiers.shift && !fresh),
        }
        self.designer_drag = (ev.click_count == 1).then_some(field);
        cx.notify();
    }

    fn designer_mouse_drag(
        &mut self,
        field: Field,
        ev: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if self.designer_drag != Some(field) || ev.pressed_button != Some(gpui::MouseButton::Left) {
            return;
        }
        let idx = self.designer_index_at(field, ev.position.x);
        if let Some(d) = self.designer.as_mut().filter(|d| d.field == field) {
            d.edit.set_caret(idx, true);
            cx.notify();
        }
    }
}

/// A labelled single-line field: click places the caret, drag selects, double-click selects a
/// word, triple-click selects everything.
fn text_field(
    t: &Theme,
    label: &'static str,
    field: Field,
    shown: FieldText,
    id: &'static str,
    layouts: FieldLayouts,
    cx: &mut Context<AppWindow>,
) -> gpui::Stateful<gpui::Div> {
    let editing = shown.caret.is_some();
    let text = shown.text;
    let caret = shown.caret.unwrap_or(text.len());
    let (sa, sb) = shown.selection.unwrap_or((caret, caret));
    let accent = t.accent;
    let caret_bar = move || div().w(px(1.)).mr(px(-1.)).h(px(14.)).bg(accent);
    let sel_bg = t.accent.opacity(0.35);
    let measured = text.clone();
    let measure = gpui::canvas(
        move |bounds, window, _| {
            let style = window.text_style();
            let size = style.font_size.to_pixels(window.rem_size());
            let run = style.to_run(measured.len());
            let line = window
                .text_system()
                .shape_line(measured.into(), size, &[run], None);
            layouts.borrow_mut().insert(field, (bounds, line));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    let body = div()
        .relative()
        .flex()
        .flex_1()
        .h_full()
        .items_center()
        .whitespace_nowrap()
        .overflow_hidden()
        .child(measure)
        .child(text[..sa].to_string())
        .when(editing && caret == sa, |el| el.child(caret_bar()))
        .when(sb > sa, |el| {
            el.child(div().bg(sel_bg).child(text[sa..sb].to_string()))
        })
        .when(editing && caret == sb && sb > sa, |el| {
            el.child(caret_bar())
        })
        .child(text[sb..].to_string());
    gpui::div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .h(px(26.))
        .px_2()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(if editing { t.accent } else { t.border })
        .bg(t.background)
        .cursor_text()
        .child(
            div()
                .w(px(70.))
                .text_size(px(11.))
                .text_color(t.text_muted)
                .child(label),
        )
        .child(body)
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |this, ev, _, cx| this.designer_mouse_down(field, ev, cx)),
        )
        .on_mouse_move(cx.listener(move |this, ev, _, cx| this.designer_mouse_drag(field, ev, cx)))
        .on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, _| this.designer_drag = None),
        )
}

fn split_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    kind: SplitKind,
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let b = Button::new(id, label);
    if enabled {
        b.on_click(cx.listener(move |this, _, _, cx| {
            if let Some(d) = this.designer.as_mut() {
                d.split(kind);
            }
            cx.notify();
        }))
        .into_any_element()
    } else {
        div().opacity(0.4).child(b).into_any_element()
    }
}

/// Draws the pane tree as nested rects; the selected pane is outlined.
fn canvas_node(
    t: &Theme,
    n: &WorkspacePane,
    path: &mut Vec<usize>,
    sel: &[usize],
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let id: gpui::ElementId = gpui::SharedString::from(format!(
        "cv-{}",
        path.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("-")
    ))
    .into();
    match (n.split, n.panes.as_slice()) {
        (Some(kind), [a, b]) => {
            let share = f32::from(n.percent.unwrap_or(50)) / 100.0;
            let mut row = div().id(id).size_full().flex().gap(px(2.));
            row = match kind {
                SplitKind::Horizontal => row.flex_row(),
                SplitKind::Vertical => row.flex_col(),
            };
            let mut child = |idx: usize, node: &WorkspacePane, frac: f32, path: &mut Vec<usize>| {
                path.push(idx);
                let el = canvas_node(t, node, path, sel, cx);
                path.pop();
                let wrap = div().child(el);
                match kind {
                    SplitKind::Horizontal => wrap.w(relative(frac)).h_full(),
                    SplitKind::Vertical => wrap.h(relative(frac)).w_full(),
                }
            };
            row.child(child(0, a, share, path))
                .child(child(1, b, 1.0 - share, path))
                .into_any_element()
        }
        _ => {
            let here = path.clone();
            let selected = sel == path.as_slice();
            div()
                .id(id)
                .size_full()
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(if selected { t.accent } else { t.border })
                .bg(if selected {
                    t.element_selected
                } else {
                    t.surface
                })
                .p_1()
                .text_size(px(11.))
                .text_color(t.text_muted)
                .overflow_hidden()
                .cursor_pointer()
                .child(n.command.join(" "))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this.designer.as_mut() {
                        d.select_pane(here.clone());
                    }
                    cx.notify();
                }))
                .into_any_element()
        }
    }
}

/// Draws `el` dimmed and inert when `enabled` is false.
fn enabled_if(enabled: bool, el: impl IntoElement) -> AnyElement {
    if enabled {
        el.into_any_element()
    } else {
        div().opacity(0.4).occlude().child(el).into_any_element()
    }
}
