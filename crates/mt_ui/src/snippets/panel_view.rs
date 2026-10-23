//! Rendering of the snippets side panel, its editor, the fill dialog and the row menu
//! (SPEC §10.8.6). State lives in [`super::ui`], [`super::panel_state`] and [`super::editor`].

use std::time::Instant;

use gpui::{
    canvas, div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, MouseButton,
    MouseDownEvent, SharedString, Window,
};
use mt_snippets::{PromptKind, Snippet, SnippetSet, Template};

use super::{
    editor::EditorField,
    editor_ctl::BODY_LINES,
    gate::SnippetsGate,
    highlight::{self, Style},
    library,
    panel_state::{Gesture, Row},
    ui::RowMenu,
};
use crate::{
    app::AppWindow,
    assets::IconName,
    components::{Button, ButtonStyle, ContextMenu, Icon, IconButton, KeyBinding, Modal, TextEdit},
    menu::{nav::MenuNav, MenuRow, RowKind},
    metrics::{PALETTE_TOP_OFFSET, RADIUS_CONTROL, TITLE_BAR_HEIGHT},
    settings::Settings,
    theme::Theme,
    view::DismissExt,
};

/// Payload while a snippet row is dragged.
#[derive(Clone)]
pub struct DraggedSnippet {
    pub id: String,
    pub folder: String,
    pub title: SharedString,
}

struct Ghost(SharedString);

impl Render for Ghost {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        div()
            .px_2()
            .py_1()
            .bg(t.elevated_surface)
            .border_1()
            .border_color(t.border)
            .rounded(px(RADIUS_CONTROL))
            .text_size(px(12.))
            .text_color(t.text)
            .child(self.0.clone())
    }
}

pub(crate) fn menu_row(id: &str, label: &str, icon: Option<&str>) -> MenuRow {
    MenuRow {
        id: id.into(),
        label: label.into(),
        chip_action: None,
        icon: icon.map(String::from),
        enabled: true,
        kind: RowKind::Item,
    }
}

pub(crate) fn separator(id: &str) -> MenuRow {
    MenuRow {
        id: id.into(),
        label: String::new(),
        chip_action: None,
        icon: None,
        enabled: false,
        kind: RowKind::Separator,
    }
}

/// A single-line field: text, selection and caret.
pub(crate) fn line_field(
    t: &Theme,
    edit: &TextEdit,
    focused: bool,
    placeholder: &str,
) -> gpui::Div {
    let text = edit.text();
    let (a, b) = edit.selection().unwrap_or((edit.caret(), edit.caret()));
    let caret_bar = || div().w(px(1.)).h(px(14.)).bg(t.accent);
    let caret = edit.caret();
    div()
        .flex()
        .items_center()
        .flex_1()
        .overflow_hidden()
        .whitespace_nowrap()
        .when(text.is_empty() && !focused, |el| {
            el.child(
                div()
                    .text_color(t.text_placeholder)
                    .child(placeholder.to_string()),
            )
        })
        .child(text[..a].to_string())
        .when(focused && caret == a, |el| el.child(caret_bar()))
        .when(b > a, |el| {
            el.child(
                div()
                    .bg(t.accent.opacity(0.35))
                    .child(text[a..b].to_string()),
            )
        })
        .when(focused && caret == b && b > a, |el| el.child(caret_bar()))
        .child(text[b..].to_string())
        .when(text.is_empty() && focused, |el| {
            el.child(
                div()
                    .ml_1()
                    .text_color(t.text_placeholder)
                    .child(placeholder.to_string()),
            )
        })
}

fn seg_div(t: &Theme, seg: &highlight::Seg) -> gpui::Div {
    let color = match seg.style {
        Style::Plain => None,
        Style::Tag => Some(t.accent),
        Style::Error => Some(t.error),
    };
    let mut d = div().child(seg.text.clone());
    if let Some(c) = color {
        d = d.text_color(c);
    }
    if seg.selected {
        d = d.bg(t.accent.opacity(0.35));
    }
    d
}

impl AppWindow {
    /// The panel, or `None` while it is closed or the feature is off.
    pub(crate) fn snippets_panel(
        &mut self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.panel_visible() {
            return None;
        }
        let set = library::set(cx);
        let width = self.snippets.panel.width;
        let body = if self.snippets.editor.is_some() {
            self.editor_view(t, cx)
        } else {
            self.list_view(t, &set, cx)
        };
        let entity = cx.entity();
        let (e_move, e_up) = (entity.clone(), entity);
        // A drag keeps tracking the pointer when it leaves the panel (it moves over the panes).
        let tracker = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let e = e_move.clone();
                window.on_mouse_event(move |ev: &gpui::MouseMoveEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        e.update(cx, |this, cx| {
                            if this.snippets.drag.is_some() {
                                this.drag_panel_to(f32::from(ev.position.x), cx);
                            }
                        });
                    }
                });
                let e = e_up.clone();
                window.on_mouse_event(move |_: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        e.update(cx, |this, cx| this.end_panel_drag(window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();
        Some(
            div()
                .id("snippets-panel")
                .relative()
                .flex()
                .flex_col()
                .flex_none()
                .w(px(width))
                .min_h_0()
                .border_l_1()
                .border_color(t.border)
                .bg(t.surface)
                .text_color(t.text)
                .text_size(px(12.))
                .track_focus(&self.snippets.focus)
                .on_key_down(cx.listener(|this, ev, w, cx| this.panel_key(ev, w, cx)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, w, cx| {
                        if !this.snippets.panel.focused {
                            this.snippets.panel.focused = true;
                            this.sync_focus(w);
                            cx.notify();
                        }
                    }),
                )
                .child(tracker)
                .child(
                    div()
                        .id("snippets-resize")
                        .absolute()
                        .left(px(-3.))
                        .top_0()
                        .bottom_0()
                        .w(px(6.))
                        .cursor_col_resize()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                this.begin_panel_drag(f32::from(ev.position.x));
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(body)
                .into_any_element(),
        )
    }

    // ------------------------------------------------------------------ list

    fn list_view(&mut self, t: &Theme, set: &SnippetSet, cx: &mut Context<Self>) -> AnyElement {
        let panel = &self.snippets.panel;
        let templates_on = SnippetsGate::of(cx).templates_on();
        let rows = panel.rows(set);
        let focused = panel.focused;
        let selected = panel.selected.clone();
        let preview_lines = self.cfg.snippets.preview_lines as usize;
        let banner = library::banner(cx);
        let searching = !panel.query.text().trim().is_empty();

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(32.))
            .px_2()
            .border_b_1()
            .border_color(t.border_variant)
            .child(Icon::new(IconName::FileText).color(t.text_muted))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Snippets"),
            )
            .child(
                IconButton::new("snippets-new", IconName::Plus)
                    .tooltip("New snippet")
                    .on_click(cx.listener(|this, _, w, cx| this.new_snippet(w, cx))),
            )
            .child(
                IconButton::new("snippets-more", IconName::ChevronDown)
                    .tooltip("More")
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                        let pos = ev.position();
                        let rows = vec![
                            menu_row("open-file", "Open snippets.toml", Some("file-text")),
                            menu_row("collapse-all", "Collapse all folders", Some("folder")),
                        ];
                        this.snippets.row_menu = Some(RowMenu {
                            id: String::new(),
                            position: pos,
                            nav: MenuNav::new(rows, 8),
                        });
                        cx.notify();
                    })),
            );

        let search = div().flex_none().p_2().child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .h(px(26.))
                .px_2()
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(if focused { t.accent } else { t.border })
                .bg(t.background)
                .child(Icon::new(IconName::Search).color(t.text_muted))
                .child(line_field(t, &panel.query, focused, "Search snippets")),
        );

        let mut list = div()
            .id("snippets-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if set.snippets.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .p_4()
                    .child(div().text_color(t.text_muted).child("No snippets yet"))
                    .child(
                        Button::new("snippets-empty-new", "New snippet")
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(|this, _, w, cx| this.new_snippet(w, cx))),
                    ),
            );
        } else if rows.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_color(t.text_muted)
                    .child("No matching snippets"),
            );
        }
        for (ix, row) in rows.iter().enumerate() {
            match row {
                Row::Folder {
                    name,
                    collapsed,
                    count,
                } => {
                    let n = name.clone();
                    list = list.child(
                        div()
                            .id(("snippet-folder", ix))
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(24.))
                            .px_2()
                            .text_color(t.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.element_hover))
                            .child(Icon::new(if *collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            }))
                            .child(div().flex_1().child(name.clone()))
                            .child(div().text_size(px(11.)).child(count.to_string()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.snippets.panel.toggle_folder(&n);
                                cx.notify();
                            })),
                    );
                }
                Row::Snippet { id } => {
                    let Some(s) = set.get(id) else { continue };
                    list = list.child(self.snippet_row(
                        t,
                        ix,
                        s,
                        selected.as_deref() == Some(id),
                        preview_lines,
                        templates_on,
                        searching,
                        cx,
                    ));
                }
            }
        }

        let sel_snippet = selected.as_deref().and_then(|id| set.get(id));
        let footer = if let Some(id) = self.snippets.delete_confirm.clone() {
            let title = set.get(&id).map_or(id.clone(), |s| s.title.clone());
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(t.border_variant)
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .child(format!("Delete \"{title}\"?")),
                )
                .child(
                    Button::new("snip-del-yes", "Delete")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, w, cx| this.confirm_delete(w, cx))),
                )
                .child(Button::new("snip-del-no", "Cancel").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.snippets.delete_confirm = None;
                        cx.notify();
                    },
                )))
        } else {
            let chip = sel_snippet
                .filter(|s| !s.keys.is_empty())
                .map(|s| s.keys.clone());
            let (sel_id, sel_id2) = (selected.clone(), selected.clone());
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(t.border_variant)
                .child(
                    div()
                        .flex_1()
                        .when_some(chip, |el, k| el.child(KeyBinding::from_text(k))),
                )
                .child(Button::new("snip-insert", "Insert").on_click(cx.listener(
                    move |this, _, w, cx| {
                        if let Some(id) = sel_id.clone() {
                            this.insert_snippet(
                                &id,
                                Gesture {
                                    run: Some(false),
                                    broadcast: false,
                                },
                                w,
                                cx,
                            );
                        }
                    },
                )))
                .child(
                    Button::new("snip-run", "Run")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            if let Some(id) = sel_id2.clone() {
                                this.insert_snippet(
                                    &id,
                                    Gesture {
                                        run: Some(true),
                                        broadcast: false,
                                    },
                                    w,
                                    cx,
                                );
                            }
                        })),
                )
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(search)
            .when_some(banner, |el, b| {
                let color = match b.kind {
                    crate::components::ToastKind::Error => t.error,
                    crate::components::ToastKind::Warning => t.warning,
                    _ => t.accent,
                };
                el.child(
                    div()
                        .flex_none()
                        .px_2()
                        .pb_1()
                        .text_size(px(11.))
                        .text_color(color)
                        .child(b.text),
                )
            })
            .child(list)
            .child(footer)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn snippet_row(
        &self,
        t: &Theme,
        ix: usize,
        s: &Snippet,
        selected: bool,
        preview_lines: usize,
        templates_on: bool,
        searching: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = s.id.clone();
        let tpl = templates_on.then(|| Template::cached(&s.body));
        let diag = tpl
            .as_ref()
            .and_then(|t| t.diagnostics().first().map(|d| d.message.clone()));
        let tags = if templates_on {
            highlight::tag_ranges(&s.body)
        } else {
            Vec::new()
        };
        let mut previews = Vec::new();
        let mut start = 0;
        for line in s.body.split('\n').take(preview_lines) {
            let r = start..start + line.len();
            start = r.end + 1;
            let segs = highlight::segments(&s.body, r, None, None, &tags, &[]);
            previews.push(
                div()
                    .flex()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(px(11.))
                    .font_family(self.cfg.font.family.clone())
                    .text_color(t.text_muted)
                    .children(segs.iter().map(|g| seg_div(t, g))),
            );
        }
        let payload = DraggedSnippet {
            id: id.clone(),
            folder: s.folder.clone(),
            title: s.title.clone().into(),
        };
        let (id_click, id_menu, id_drop) = (id.clone(), id.clone(), id.clone());
        let (id_edit, id_del) = (id.clone(), id.clone());
        let drag_over = t.element_hover;
        div()
            .id(("snippet-row", ix))
            .relative()
            .flex()
            .flex_col()
            .gap(px(1.))
            .px_2()
            .py_1()
            .when(!searching && !s.folder.is_empty(), |el| el.pl_5())
            .cursor_pointer()
            .when(selected, |el| el.bg(t.element_selected))
            .when(!selected, |el| el.hover(|s| s.bg(t.element_hover)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(s.title.clone()),
                    )
                    .when(s.run, |el| {
                        el.child(div().text_color(t.text_muted).child("↵"))
                    })
                    .when_some(diag, |el, d| {
                        el.child(
                            div()
                                .px_1()
                                .rounded(px(RADIUS_CONTROL))
                                .bg(t.error.opacity(0.2))
                                .text_color(t.error)
                                .text_size(px(10.))
                                .max_w(px(120.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(d),
                        )
                    })
                    .child(
                        IconButton::new(("snippet-edit", ix), IconName::Pencil)
                            .tooltip("Edit")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.edit_snippet(&id_edit, w, cx);
                            })),
                    )
                    .child(
                        IconButton::new(("snippet-delete", ix), IconName::Trash2)
                            .tooltip("Delete")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.ask_delete(&id_del, cx);
                            })),
                    ),
            )
            .children(previews)
            .on_click(cx.listener(move |this, ev: &ClickEvent, w, cx| {
                this.snippets.panel.selected = Some(id_click.clone());
                if !this.snippets.panel.click_allowed(&id_click, Instant::now()) {
                    cx.notify();
                    return;
                }
                let m = ev.modifiers();
                let g = Gesture::from_modifiers(m.alt, m.shift, m.platform || m.control);
                this.insert_snippet(&id_click, g, w, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.open_row_menu(&id_menu, ev.position, cx);
                }),
            )
            .on_drag(payload, |d, _, _, cx| cx.new(|_| Ghost(d.title.clone())))
            .drag_over::<DraggedSnippet>(move |s, _, _, _| s.bg(drag_over))
            .on_drop(cx.listener(move |this, d: &DraggedSnippet, w, cx| {
                if d.id != id_drop {
                    let same = library::set(cx)
                        .get(&id_drop)
                        .is_some_and(|t| t.folder == d.folder);
                    if same {
                        this.reorder_snippet(&d.id, Some(&id_drop), w, cx);
                    }
                }
            }))
            .into_any_element()
    }

    pub(crate) fn open_row_menu(
        &mut self,
        id: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.snippets.panel.selected = Some(id.to_string());
        let rows = vec![
            menu_row("insert", "Insert", Some("corner-down-left")),
            menu_row("insert-run", "Insert & Run", Some("play")),
            separator("s1"),
            menu_row("edit", "Edit", Some("pencil")),
            menu_row("duplicate", "Duplicate", Some("copy")),
            separator("s2"),
            menu_row("copy-expanded", "Copy expanded", Some("clipboard")),
            menu_row("copy-raw", "Copy raw", Some("clipboard")),
            separator("s3"),
            menu_row("move", "Move to folder…", Some("folder")),
            menu_row("delete", "Delete…", Some("trash-2")),
        ];
        self.snippets.row_menu = Some(RowMenu {
            id: id.to_string(),
            position,
            nav: MenuNav::new(rows, 12),
        });
        cx.notify();
    }

    pub(crate) fn activate_row_menu(
        &mut self,
        row: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.snippets.row_menu.take() else {
            return;
        };
        cx.notify();
        let id = menu.id;
        match row.as_str() {
            "open-file" => self.open_snippets_file(window, cx),
            "collapse-all" => {
                let set = library::set(cx);
                self.snippets.panel.collapse_all(&set);
            }
            "insert" => self.insert_snippet(&id, Gesture::default(), window, cx),
            "insert-run" => self.insert_snippet(
                &id,
                Gesture {
                    run: Some(true),
                    broadcast: false,
                },
                window,
                cx,
            ),
            "edit" => self.edit_snippet(&id, window, cx),
            "duplicate" => self.duplicate_snippet(&id, window, cx),
            "copy-expanded" => self.copy_expanded(&id, window, cx),
            "copy-raw" => self.copy_raw(&id, cx),
            "move" => self.ask_move(&id, window, cx),
            "delete" => self.ask_delete(&id, cx),
            _ => {}
        }
    }

    pub(crate) fn render_row_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.snippets.row_menu.as_ref()?;
        let entity = cx.entity();
        let (e1, e2, e3) = (entity.clone(), entity.clone(), entity);
        Some(
            ContextMenu::new("snippet-row-menu", m.position, m.nav.clone())
                .on_dismiss(move |_, app| {
                    e1.update(app, |this, cx| {
                        this.snippets.row_menu = None;
                        cx.notify();
                    });
                })
                .on_activate(move |id, window, app| {
                    e2.update(app, |this, cx| this.activate_row_menu(id, window, cx));
                })
                .on_hover(move |i, _, app| {
                    e3.update(app, |this, cx| {
                        if let Some(m) = this.snippets.row_menu.as_mut() {
                            if m.nav.highlight != i {
                                m.nav.hover(i);
                                cx.notify();
                            }
                        }
                    });
                })
                .into_any_element(),
        )
    }

    // ------------------------------------------------------------------ editor

    fn editor_view(&mut self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let templates_on = SnippetsGate::of(cx).templates_on();
        let cell = self.cell;
        let font = self.cfg.font.family.clone();
        let font_size = self.font_size();
        let origin = self.snippets.body_origin.clone();
        let preview = self.snippets.preview.clone();
        let focused_panel = self.snippets.panel.focused;
        let Some(ed) = self.snippets.editor.as_ref() else {
            return div().into_any_element();
        };
        let field = ed.field;
        let blocker = ed.save_blocker(templates_on);
        let diags = ed.diagnostics(templates_on);
        let cursor_warning = ed.cursor_warning(templates_on);
        let key_error = ed.key_error();
        let (confirm_discard, stale, error) = (ed.confirm_discard, ed.stale, ed.error.clone());
        let is_new = ed.id.is_none();

        let row = |label: &'static str,
                   f: EditorField,
                   edit: &TextEdit,
                   ph: &'static str,
                   cx: &mut Context<Self>| {
            div()
                .id(label)
                .flex()
                .items_center()
                .gap_2()
                .h(px(26.))
                .px_2()
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(if field == f && focused_panel {
                    t.accent
                } else {
                    t.border
                })
                .bg(t.background)
                .cursor_text()
                .child(
                    div()
                        .w(px(64.))
                        .text_size(px(11.))
                        .text_color(t.text_muted)
                        .child(label),
                )
                .child(line_field(t, edit, field == f && focused_panel, ph))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(ed) = this.snippets.editor.as_mut() {
                            ed.field = f;
                        }
                        cx.notify();
                    }),
                )
        };
        let ed = self.snippets.editor.as_ref().expect("checked above");

        // Body: monospace lines with tag highlighting, selection and the caret.
        let body_text = ed.body.text();
        let tags = if templates_on {
            highlight::tag_ranges(body_text)
        } else {
            Vec::new()
        };
        let errs = if templates_on {
            highlight::error_ranges(body_text)
        } else {
            Vec::new()
        };
        let sel = ed.body.selection();
        let caret = (field == EditorField::Body && focused_panel).then(|| ed.body.caret());
        let mut body_lines = Vec::new();
        for line in ed.body.scroll..(ed.body.scroll + BODY_LINES).min(ed.body.line_count()) {
            let r = ed.body.line_range(line);
            let segs = highlight::segments(body_text, r, sel, caret, &tags, &errs);
            let mut d = div().flex().h(px(cell.height)).whitespace_nowrap();
            for g in &segs {
                if g.caret_before {
                    d = d.child(div().w(px(1.)).h(px(cell.height)).bg(t.accent));
                }
                if !g.text.is_empty() {
                    d = d.child(seg_div(t, g));
                }
            }
            body_lines.push(d);
        }
        let scroll = ed.body.scroll;
        let body = div()
            .id("snippet-body")
            .relative()
            .flex_none()
            .h(px(cell.height * BODY_LINES as f32 + 8.))
            .p(px(4.))
            .overflow_hidden()
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(if field == EditorField::Body && focused_panel {
                t.accent
            } else {
                t.border
            })
            .bg(t.background)
            .font_family(font)
            .text_size(px(font_size))
            .cursor_text()
            .child(
                canvas(
                    {
                        let origin = origin.clone();
                        move |bounds, _, _| {
                            origin.set((
                                f32::from(bounds.origin.x) + 4.0,
                                f32::from(bounds.origin.y) + 4.0,
                            ));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(body_lines)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let (ox, oy) = this.snippets.body_origin.get();
                    let (cw, ch) = (this.cell.width.max(1.0), this.cell.height.max(1.0));
                    let line = scroll + ((f32::from(ev.position.y) - oy).max(0.0) / ch) as usize;
                    let col = ((f32::from(ev.position.x) - ox).max(0.0) / cw).round() as usize;
                    if let Some(ed) = this.snippets.editor.as_mut() {
                        ed.field = EditorField::Body;
                        let r = ed.body.line_range(line.min(ed.body.line_count() - 1));
                        let byte = ed.body.text()[r.clone()]
                            .char_indices()
                            .nth(col)
                            .map_or(r.len(), |(i, _)| i);
                        match ev.click_count {
                            2 => ed.body.select_word_at(r.start + byte),
                            n if n >= 3 => ed.body.select_all(),
                            _ => ed.body.set_caret_line_col(line, byte, ev.modifiers.shift),
                        }
                    }
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _, cx| {
                let dy = match ev.delta {
                    gpui::ScrollDelta::Lines(l) => l.y,
                    gpui::ScrollDelta::Pixels(p) => f32::from(p.y) / 16.0,
                };
                if let Some(ed) = this.snippets.editor.as_mut() {
                    ed.body.scroll_by((-dy).round() as isize, BODY_LINES);
                }
                cx.notify();
            }));

        let run_row = div()
            .id("snippet-run")
            .flex()
            .items_center()
            .gap_2()
            .h(px(26.))
            .px_2()
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(if field == EditorField::Run && focused_panel {
                t.accent
            } else {
                t.border
            })
            .bg(t.background)
            .cursor_pointer()
            .child(div().flex_1().child("Run after insert (press Enter)"))
            .child(toggle(t, ed.run))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(ed) = this.snippets.editor.as_mut() {
                    ed.field = EditorField::Run;
                    ed.run = !ed.run;
                }
                cx.notify();
            }));

        let preview_box = div()
            .flex_none()
            .p_2()
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(t.border_variant)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child("Preview"),
            )
            .child(if templates_on {
                match preview {
                    Some(Ok(text)) => div()
                        .font_family(self.cfg.font.family.clone())
                        .text_size(px(11.))
                        .child(text),
                    Some(Err(e)) => div().text_color(t.error).child(e),
                    None => div().text_color(t.text_muted).child("…"),
                }
            } else {
                div()
                    .text_color(t.text_muted)
                    .child("Template expansion is off (`snippets.templates`)")
            });

        let mut col = div()
            .id("snippets-editor")
            .flex()
            .flex_col()
            .gap_2()
            .size_full()
            .min_h_0()
            .p_2()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(
                        IconButton::new("snippets-back", IconName::ChevronRight)
                            .tooltip("Back to the list")
                            .on_click(cx.listener(|this, _, w, cx| this.editor_back(w, cx))),
                    )
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(if is_new {
                        "New snippet"
                    } else {
                        "Edit snippet"
                    })),
            )
            .child(row("Title", EditorField::Title, &ed.title, "required", cx))
            .child(row(
                "Folder",
                EditorField::Folder,
                &ed.folder,
                "optional",
                cx,
            ))
            .child(row(
                "Tags",
                EditorField::Tags,
                &ed.tags,
                "comma-separated",
                cx,
            ))
            .child(row(
                "About",
                EditorField::Description,
                &ed.description,
                "optional",
                cx,
            ))
            .child(body);
        for (l, c, m) in diags.iter().take(3) {
            col = col.child(
                div()
                    .text_size(px(11.))
                    .text_color(t.error)
                    .child(format!("{l}:{c}  {m}")),
            );
        }
        if let Some(w) = cursor_warning {
            col = col.child(div().text_size(px(11.)).text_color(t.warning).child(w));
        }
        col = col
            .child(run_row)
            .child(row("Key", EditorField::Key, &ed.key, "e.g. cmd+alt+1", cx));
        if let Some(e) = key_error {
            col = col.child(div().text_size(px(11.)).text_color(t.error).child(e));
        }
        col = col.child(preview_box);
        let status = if confirm_discard {
            Some((
                t.warning,
                "Discard unsaved changes? Enter discards, Esc keeps editing.".to_string(),
            ))
        } else {
            error.or(blocker).map(|b| (t.text_muted, b))
        };
        if let Some((color, text)) = status {
            col = col.child(div().text_size(px(11.)).text_color(color).child(text));
        }
        col.child(
            div()
                .flex()
                .flex_none()
                .gap_2()
                .when(stale, |el| {
                    el.child(
                        Button::new("snip-reload", "Reload and re-apply")
                            .on_click(cx.listener(|this, _, _, cx| this.reload_editor_base(cx))),
                    )
                })
                .child(
                    Button::new("snip-save", "Save")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, w, cx| this.save_editor(w, cx))),
                )
                .child(
                    Button::new("snip-cancel", "Cancel")
                        .on_click(cx.listener(|this, _, w, cx| this.editor_back(w, cx))),
                ),
        )
        .into_any_element()
    }

    /// The back arrow / Cancel: asks first when there are unsaved changes.
    pub(crate) fn editor_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.snippets.editor.as_mut() {
            Some(ed) if ed.dirty() => {
                ed.confirm_discard = true;
                cx.notify();
            }
            Some(_) => self.close_editor(window, cx),
            None => {}
        }
    }

    // ------------------------------------------------------------------ fill dialog

    pub(crate) fn fill_modal(&mut self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.snippets.fill.as_ref()?;
        let preview = self.fill_preview(cx);
        let font = self.cfg.font.family.clone();
        let fill = self.snippets.fill.as_ref()?;
        let mut fields = Vec::new();
        for (i, f) in fill.fields.iter().enumerate() {
            let focused = i == fill.focus;
            let label = div()
                .text_size(px(11.))
                .text_color(t.text_muted)
                .child(format!("{} ({})", f.prompt.label, f.prompt.kind.name()));
            let input: AnyElement = match f.prompt.kind {
                PromptKind::Choice => {
                    let (shown, hl) = fill.options(i);
                    div()
                        .flex()
                        .flex_col()
                        .rounded(px(RADIUS_CONTROL))
                        .border_1()
                        .border_color(if focused { t.accent } else { t.border })
                        .bg(t.background)
                        .child(
                            div()
                                .h(px(22.))
                                .px_2()
                                .flex()
                                .items_center()
                                .child(line_field(t, &f.edit, focused, "type to filter")),
                        )
                        .children(shown.iter().enumerate().map(|(k, o)| {
                            div()
                                .px_2()
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .when(k == hl, |el| el.bg(t.element_selected))
                                .child((*o).to_string())
                        }))
                        .into_any_element()
                }
                PromptKind::Secret => {
                    let masked = TextEdit::new("•".repeat(f.edit.text().chars().count()));
                    field_box(t, focused, line_field(t, &masked, focused, "")).into_any_element()
                }
                PromptKind::Input => {
                    field_box(t, focused, line_field(t, &f.edit, focused, "")).into_any_element()
                }
            };
            fields.push(div().flex().flex_col().gap_1().child(label).child(input));
        }
        let preview_el = match preview {
            Ok(e) => div().child(e.text),
            Err(e) if e.is_empty() => div(),
            Err(e) => div().text_color(t.error).child(e),
        };
        let title = fill.title.clone();
        let copy = fill.copy;
        Some(
            Modal::new("snippet-fill")
                .width(px(480.))
                .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
                .on_dismiss(cx.listener_for_dismiss())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                        .children(fields)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_size(px(11.)).text_color(t.text_muted).child("Preview"))
                                .child(
                                    div()
                                        .p_2()
                                        .rounded(px(RADIUS_CONTROL))
                                        .border_1()
                                        .border_color(t.border_variant)
                                        .bg(t.background)
                                        .font_family(font)
                                        .text_size(px(12.))
                                        .child(preview_el),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(t.text_muted)
                                .child(if copy {
                                    "Enter copies · Tab next field · Esc cancels"
                                } else {
                                    "Enter inserts · ⌥Enter inserts and runs · Tab next field · Esc cancels"
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

pub(crate) fn field_box(t: &Theme, focused: bool, inner: gpui::Div) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .h(px(26.))
        .px_2()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(if focused { t.accent } else { t.border })
        .bg(t.background)
        .child(inner)
}

/// A small on/off switch.
pub(crate) fn toggle(t: &Theme, on: bool) -> gpui::Div {
    div()
        .w(px(28.))
        .h(px(16.))
        .rounded(px(8.))
        .bg(if on { t.accent } else { t.border })
        .p(px(2.))
        .flex()
        .when(on, |el| el.justify_end())
        .child(div().size(px(12.)).rounded(px(6.)).bg(t.background))
}

#[allow(dead_code)]
fn _settings(_: &Settings) {}
