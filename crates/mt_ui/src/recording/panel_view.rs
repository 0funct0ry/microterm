//! Rendering of the Recordings side panel, its row menu and the export dialog (SPEC §10.10.5).
//! State lives in [`super::panel`], [`super::panel_state`] and [`super::export`].

use std::time::Duration;

use gpui::{
    canvas, div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, MouseButton,
    MouseDownEvent,
};
use mt_export::OptKind;

use super::{
    export::{ExportDialog, SWATCHES},
    panel_state::{self, Entry, Row},
    ui::{format_elapsed, format_size, reduce_motion},
};
use crate::{
    app::AppWindow,
    assets::IconName,
    components::{Button, ButtonStyle, ContextMenu, Icon, IconButton, Modal},
    menu::{nav::MenuNav, MenuRow},
    metrics::{PALETTE_TOP_OFFSET, RADIUS_CONTROL, TITLE_BAR_HEIGHT},
    snippets::{
        panel_view::{field_box, line_field, menu_row, separator, toggle},
        ui::RowMenu,
    },
    theme::Theme,
    view::DismissExt,
};

fn pill(t: &Theme, text: String, warn: bool) -> gpui::Div {
    div()
        .px_1()
        .rounded(px(RADIUS_CONTROL))
        .bg(if warn {
            t.warning.opacity(0.2)
        } else {
            t.element_hover
        })
        .text_color(if warn { t.warning } else { t.text_muted })
        .text_size(px(10.))
        .whitespace_nowrap()
        .child(text)
}

impl AppWindow {
    /// The panel, or `None` while it is closed or the feature is off.
    pub(crate) fn recordings_panel(
        &mut self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.recordings_visible() {
            return None;
        }
        if self.recordings.stale {
            self.refresh_recordings(cx);
        }
        let width = self.recordings.panel.width;
        let body = self.recordings_body(t, cx);
        let entity = cx.entity();
        let (e_move, e_up) = (entity.clone(), entity);
        let tracker = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let e = e_move.clone();
                window.on_mouse_event(move |ev: &gpui::MouseMoveEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        e.update(cx, |this, cx| {
                            if this.recordings.drag.is_some() {
                                this.drag_recordings_to(f32::from(ev.position.x), cx);
                            }
                        });
                    }
                });
                let e = e_up.clone();
                window.on_mouse_event(move |_: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        e.update(cx, |this, cx| this.end_recordings_drag(window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();
        Some(
            div()
                .id("recordings-panel")
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
                .track_focus(&self.recordings.focus)
                .on_key_down(cx.listener(|this, ev, w, cx| this.recordings_key(ev, w, cx)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, w, cx| {
                        if !this.recordings.panel.focused {
                            this.recordings.panel.focused = true;
                            this.snippets.panel.focused = false;
                            this.sync_focus(w);
                            cx.notify();
                        }
                    }),
                )
                .child(tracker)
                .child(
                    div()
                        .id("recordings-resize")
                        .absolute()
                        .left(px(-3.))
                        .top_0()
                        .bottom_0()
                        .w(px(6.))
                        .cursor_col_resize()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                this.begin_recordings_drag(f32::from(ev.position.x));
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(body)
                .into_any_element(),
        )
    }

    fn recordings_body(&mut self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.recording_entries(cx);
        let now = mt_record::store::unix_now();
        let panel = &self.recordings.panel;
        let rows = panel_state::rows(&entries, panel.query.text(), now);
        let focused = panel.focused;
        let selected = panel.selected.clone();
        let searching = !panel.query.text().trim().is_empty();
        let any_running = entries.iter().any(|e| e.running);
        let total = self.recordings.total_size;
        let pane_recording = self.focused_is_recording(cx);

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(32.))
            .px_2()
            .border_b_1()
            .border_color(t.border_variant)
            .child(Icon::new(IconName::Play).color(t.text_muted))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Recordings"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_muted)
                            .child(format_size(total)),
                    ),
            )
            .child(
                IconButton::new(
                    "recordings-record",
                    if pane_recording {
                        IconName::Square
                    } else {
                        IconName::Plus
                    },
                )
                .tooltip(if pane_recording {
                    "Stop recording the focused pane"
                } else {
                    "Record the focused pane"
                })
                .on_click(cx.listener(|this, _, w, cx| this.toggle_recording(w, cx))),
            )
            .child(
                IconButton::new("recordings-more", IconName::ChevronDown)
                    .tooltip("More")
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                        let rows = vec![
                            menu_row("reveal-store", "Reveal store folder", Some("folder-open")),
                            menu_row("import", "Import .cast…", Some("file-text")),
                            menu_row("refresh", "Refresh", Some("refresh-cw")),
                        ];
                        this.recordings.row_menu = Some(RowMenu {
                            id: String::new(),
                            position: ev.position(),
                            nav: MenuNav::new(rows, 8),
                        });
                        cx.notify();
                    })),
            )
            .child(
                IconButton::new("recordings-hide", IconName::X)
                    .tooltip("Hide panel")
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.recordings.close_panel();
                        this.after_panel_change(w, cx);
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
                .child(line_field(
                    t,
                    &self.recordings.panel.query,
                    focused && self.recordings.rename.is_none(),
                    "Search recordings",
                )),
        );

        let mut list = div()
            .id("recordings-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(e) = &self.recordings.error {
            list = list.child(div().p_3().text_color(t.error).child(e.clone()));
        } else if entries.is_empty() {
            let key = if cfg!(target_os = "macos") {
                "⌘⌥R"
            } else {
                "Ctrl+Alt+R"
            };
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .p_4()
                    .child(
                        div()
                            .text_color(t.text_muted)
                            .child(format!("No recordings yet — press {key} in a pane.")),
                    )
                    .child(
                        Button::new("recordings-empty-start", "Start recording")
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(|this, _, w, cx| this.start_recording(w, cx))),
                    ),
            );
        } else if rows.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_color(t.text_muted)
                    .child("No matching recordings"),
            );
        }
        for (ix, row) in rows.iter().enumerate() {
            match row {
                Row::Header(g) => {
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(24.))
                            .px_2()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_muted)
                            .child(g.label()),
                    );
                }
                Row::Item(id) => {
                    let Some(e) = entries.iter().find(|e| &e.sc.id == id) else {
                        continue;
                    };
                    list = list.child(self.recording_row(
                        t,
                        ix,
                        e,
                        selected.as_deref() == Some(id.as_str()),
                        now,
                        searching,
                        cx,
                    ));
                }
            }
        }

        let footer = self.recordings_footer(t, cx);
        let _ = any_running;
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(search)
            .child(list)
            .children(self.export_jobs_view(t, cx))
            .children(footer)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn recording_row(
        &mut self,
        t: &Theme,
        ix: usize,
        e: &Entry,
        selected: bool,
        now: u64,
        _searching: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = e.sc.id.clone();
        let renaming = self
            .recordings
            .rename
            .as_ref()
            .filter(|(rid, _)| *rid == id)
            .map(|(_, edit)| edit.clone());
        let dim = reduce_motion() || self.rec_pulse;
        let meta = if e.running {
            format!(
                "{} · REC{}",
                format_elapsed(Duration::from_millis(e.sc.duration_ms)),
                if e.sc.has_input { " +INPUT" } else { "" }
            )
        } else {
            panel_state::meta_line(&e.sc)
        };
        let badges = panel_state::badges(&e.sc);
        let (id_click, id_menu) = (id.clone(), id.clone());
        let pane = e.pane;
        let running = e.running;
        div()
            .id(("recording-row", ix))
            .relative()
            .flex()
            .flex_col()
            .gap(px(1.))
            .px_2()
            .py_1()
            .cursor_pointer()
            .when(selected, |el| el.bg(t.element_selected))
            .when(!selected, |el| el.hover(|s| s.bg(t.element_hover)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(running, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .size(px(8.))
                                .rounded_full()
                                .bg(t.error)
                                .opacity(if dim { 1.0 } else { 0.35 }),
                        )
                    })
                    .child(match renaming {
                        Some(edit) => field_box(t, true, line_field(t, &edit, true, ""))
                            .flex_1()
                            .into_any_element(),
                        None => div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(e.sc.title.clone())
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(t.text_muted)
                            .child(if running {
                                String::new()
                            } else {
                                panel_state::relative_time(e.sc.created_at, now)
                            }),
                    )
                    .when_some(pane.filter(|_| running), |el, p| {
                        el.child(
                            Button::new(("recording-stop", ix), "Stop").on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.stop_recording_of(p, cx);
                                },
                            )),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_muted)
                            .child(meta),
                    )
                    .children(badges.into_iter().map(|b| {
                        let warn = b == "input not redacted";
                        pill(t, b, warn)
                    })),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.recordings.panel.selected = Some(id_click.clone());
                this.recordings.rename = None;
                let double = ev.click_count() >= 2;
                if double && !running && this.recording_click(&id_click) {
                    this.replay_recording(&id_click, 1.0, cx);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.open_recording_menu(&id_menu, ev.position, cx);
                }),
            )
            .into_any_element()
    }

    /// The delete confirmation. Replay and Export live in the row menu, so the panel has no
    /// footer otherwise (running exports show their own progress lines above this).
    fn recordings_footer(&mut self, t: &Theme, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let id = self.recordings.delete_confirm.clone()?;
        let title = self
            .recordings
            .entries
            .iter()
            .find(|e| e.sc.id == id)
            .map_or(id.clone(), |e| e.sc.title.clone());
        Some(
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
                    Button::new("rec-del-yes", "Delete")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_recording_delete(cx))),
                )
                .child(Button::new("rec-del-no", "Cancel").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.recordings.delete_confirm = None;
                        cx.notify();
                    },
                ))),
        )
    }

    /// The running exports: title and percentage, a progress bar and a Cancel button at the
    /// right end below it.
    fn export_jobs_view(&mut self, t: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.recordings
            .jobs
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let frac = j.progress.fraction().clamp(0.0, 1.0);
                let cancelling = j.progress.is_cancelling();
                let progress = j.progress.clone();
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .pt_3()
                    .pb_3()
                    .border_t_1()
                    .border_color(t.border_variant)
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(format!("Exporting {} as {}", j.title, j.format)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(t.text_muted)
                                    .child(if cancelling {
                                        "Cancelling…".to_string()
                                    } else {
                                        format!("{}%", (frac * 100.0).round() as u32)
                                    }),
                            ),
                    )
                    .child(progress_bar(t, frac))
                    .child(div().flex().justify_end().child(
                        Button::new(("export-cancel", i), "Cancel").on_click(cx.listener(
                            move |_, _, _, cx| {
                                progress.cancel();
                                cx.notify();
                            },
                        )),
                    ))
                    .into_any_element()
            })
            .collect()
    }

    // ------------------------------------------------------------------ row menu

    pub(crate) fn open_recording_menu(
        &mut self,
        id: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.recordings.panel.selected = Some(id.to_string());
        let running = self
            .recording_entries(cx)
            .iter()
            .any(|e| e.sc.id == id && e.running);
        let rows: Vec<MenuRow> = if running {
            vec![menu_row("stop", "Stop", Some("square"))]
        } else {
            vec![
                menu_row("replay", "Replay", Some("play")),
                menu_row("replay-2x", "Replay at 2×", Some("play")),
                separator("s1"),
                menu_row("export", "Export…", Some("file-text")),
                menu_row("rename", "Rename", Some("pencil")),
                separator("s2"),
                menu_row("copy-path", "Copy path", Some("clipboard")),
                menu_row(
                    "reveal",
                    if cfg!(target_os = "macos") {
                        "Reveal in Finder"
                    } else {
                        "Reveal in file manager"
                    },
                    Some("folder-open"),
                ),
                separator("s3"),
                menu_row("delete", "Delete…", Some("trash-2")),
            ]
        };
        let n = rows.len();
        self.recordings.row_menu = Some(RowMenu {
            id: id.to_string(),
            position,
            nav: MenuNav::new(rows, n),
        });
        cx.notify();
    }

    pub(crate) fn activate_recording_menu(
        &mut self,
        row: String,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.recordings.row_menu.take() else {
            return;
        };
        cx.notify();
        let id = menu.id;
        match row.as_str() {
            "reveal-store" => self.reveal_store_folder(cx),
            "import" => self.import_recording(cx),
            "refresh" => self.refresh_recordings(cx),
            "replay" => self.replay_recording(&id, 1.0, cx),
            "replay-2x" => self.replay_recording(&id, 2.0, cx),
            "export" => self.export_recording(Some(id), window, cx),
            "rename" => self.begin_recording_rename(cx),
            "copy-path" => self.copy_recording_path(&id, cx),
            "reveal" => self.reveal_recording(&id, cx),
            "delete" => self.ask_recording_delete(cx),
            "stop" => {
                let pane = self
                    .recording_entries(cx)
                    .iter()
                    .find(|e| e.sc.id == id)
                    .and_then(|e| e.pane);
                if let Some(p) = pane {
                    self.stop_recording_of(p, cx);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn render_recording_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.recordings.row_menu.as_ref()?;
        let entity = cx.entity();
        let (e1, e2, e3) = (entity.clone(), entity.clone(), entity);
        Some(
            ContextMenu::new("recording-row-menu", m.position, m.nav.clone())
                .on_dismiss(move |_, app| {
                    e1.update(app, |this, cx| {
                        this.recordings.row_menu = None;
                        cx.notify();
                    });
                })
                .on_activate(move |id, window, app| {
                    e2.update(app, |this, cx| this.activate_recording_menu(id, window, cx));
                })
                .on_hover(move |i, _, app| {
                    e3.update(app, |this, cx| {
                        if let Some(m) = this.recordings.row_menu.as_mut() {
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

    // ------------------------------------------------------------------ export dialog

    pub(crate) fn export_modal(&mut self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.recordings.dialog.as_ref()?;
        let specs = d.specs();
        let reg = mt_export::registry();

        let formats = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child("Format"),
            )
            .child(
                div()
                    .flex()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(px(RADIUS_CONTROL))
                    .border_1()
                    .border_color(if d.focus == 0 { t.accent } else { t.border })
                    .bg(t.background)
                    .children(reg.iter().enumerate().map(|(i, e)| {
                        let why = d.unavailable(i);
                        let on = i == d.format;
                        div()
                            .id(("export-format", i))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .px_2()
                            .h(px(26.))
                            .rounded(px(RADIUS_CONTROL))
                            .when(on, |el| el.bg(t.element_selected))
                            .when(why.is_none() && !on, |el| {
                                el.cursor_pointer().hover(|s| s.bg(t.element_hover))
                            })
                            .when(why.is_some(), |el| el.text_color(t.text_placeholder))
                            .child(div().whitespace_nowrap().child(e.label()))
                            .when_some(why.clone(), |el, w| {
                                el.child(
                                    div()
                                        .flex_1()
                                        .text_size(px(11.))
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .child(w),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(d) = this.recordings.dialog.as_mut() {
                                    d.focus = 0;
                                    d.theme_open = false;
                                    d.select_format(i);
                                }
                                cx.notify();
                            }))
                    })),
            );

        // Options in two panes: a column of rows, each row one full-width or two half-width cells.
        let pane_w = px((DIALOG_WIDTH - 32. - PANE_GAP) / 2.);
        let layouts = self.recordings.field_layout.clone();
        let mut panes = Vec::new();
        for pane in d.panes() {
            let mut rows: Vec<AnyElement> = Vec::new();
            for row in &pane.rows {
                let wide = row.len() == 1;
                let cells: Vec<AnyElement> = row
                    .iter()
                    .map(|&i| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(export_option(t, d, i, &specs[i], wide, &layouts, cx))
                            .into_any_element()
                    })
                    .collect();
                rows.push(div().flex().gap_3().children(cells).into_any_element());
            }
            panes.push(
                div()
                    .w(pane_w)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when_some(pane.title, |el, title| {
                        el.child(
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_placeholder)
                                .child(title.to_uppercase()),
                        )
                    })
                    .children(rows),
            );
        }

        let path_focus = d.path_focus();
        let path_focused = d.focus == path_focus;
        let open_focused = d.focus == d.open_focus();
        let error = d.error.clone();
        let title = d.sidecar.title.clone();
        let open_on = d.open_when_done;
        let can_export = d.unavailable(d.format).is_none();
        let hint = d
            .focused_spec()
            .map(|s| format!("{}: {}", s.label, s.help))
            .unwrap_or_else(|| "Enter exports · Tab next field · Esc cancels".into());
        let sheet = d.theme_open.then(|| theme_sheet(t, d, pane_w, cx));
        let output = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child("Output file"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        export_text_field(
                            t,
                            &d.path,
                            path_focus,
                            path_focused,
                            "Choose where to save (home or browse buttons)",
                            &layouts,
                            cx,
                        )
                        .flex_1(),
                    )
                    .child(
                        IconButton::new("export-home", IconName::House)
                            .tooltip("Home folder: use <home>/<recording name>")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(d) = this.recordings.dialog.as_mut() {
                                    d.fill_home();
                                    d.focus = path_focus;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        IconButton::new("export-browse", IconName::FolderOpen)
                            .tooltip("Browse…")
                            .on_click(cx.listener(|this, _, _, cx| this.browse_export_path(cx))),
                    ),
            );

        Some(
            Modal::new("export-dialog")
                .width(px(DIALOG_WIDTH))
                .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
                .on_dismiss(cx.listener_for_dismiss())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("Export \"{title}\"")),
                        )
                        .child(formats)
                        .child(
                            div()
                                .id("export-fields")
                                .max_h(px(FIELDS_MAX_HEIGHT))
                                .overflow_y_scroll()
                                .flex()
                                .gap(px(PANE_GAP))
                                .children(panes),
                        )
                        .child(output)
                        .when_some(error, |el, e| {
                            el.child(div().text_color(t.error).text_size(px(11.)).child(e))
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .id("export-open")
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(d) = this.recordings.dialog.as_mut() {
                                                d.open_when_done = !d.open_when_done;
                                            }
                                            cx.notify();
                                        }))
                                        .child(toggle(t, open_on))
                                        .child(
                                            div()
                                                .text_color(if open_focused {
                                                    t.accent
                                                } else {
                                                    t.text
                                                })
                                                .child("Open when done"),
                                        ),
                                )
                                .child(div().flex_1())
                                .child(Button::new("export-cancel-btn", "Cancel").on_click(
                                    cx.listener(|this, _, w, cx| this.close_export_dialog(w, cx)),
                                ))
                                .when(can_export, |el| {
                                    el.child(
                                        Button::new("export-go", "Export")
                                            .style(ButtonStyle::Filled)
                                            .on_click(cx.listener(|this, _, w, cx| {
                                                this.submit_export(w, cx)
                                            })),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(t.text_muted)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(hint),
                        )
                        .children(sheet),
                )
                .into_any_element(),
        )
    }
}

/// A single-line text field of the export dialog: click places the caret, drag selects, double
/// click selects a word and triple click everything, like the designer's fields. The text is
/// measured at paint time so the pointer position maps to a byte offset.
fn export_text_field(
    t: &Theme,
    edit: &crate::components::TextEdit,
    ix: usize,
    focused: bool,
    placeholder: &'static str,
    layouts: &super::panel::ExportLayouts,
    cx: &mut Context<AppWindow>,
) -> gpui::Stateful<gpui::Div> {
    let text = edit.text().to_string();
    let (a, b) = edit.selection().unwrap_or((edit.caret(), edit.caret()));
    let caret = edit.caret();
    let accent = t.accent;
    let caret_bar = move || div().w(px(1.)).mr(px(-1.)).h(px(14.)).bg(accent);
    let measured = text.clone();
    let store = layouts.clone();
    let measure = canvas(
        move |bounds, window, _| {
            let style = window.text_style();
            let size = style.font_size.to_pixels(window.rem_size());
            let run = style.to_run(measured.len());
            let line = window
                .text_system()
                .shape_line(measured.into(), size, &[run], None);
            store.borrow_mut().insert(ix, (bounds, line));
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
        .when(text.is_empty(), |el| {
            el.child(
                div()
                    .text_color(t.text_placeholder)
                    .child(placeholder.to_string()),
            )
        })
        .child(text[..a].to_string())
        .when(focused && caret == a, |el| el.child(caret_bar()))
        .when(b > a, |el| {
            el.child(div().bg(accent.opacity(0.35)).child(text[a..b].to_string()))
        })
        .when(focused && caret == b && b > a, |el| el.child(caret_bar()))
        .child(text[b..].to_string());
    div()
        .id(("export-text", ix))
        .flex()
        .items_center()
        .h(px(26.))
        .px_2()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(if focused { t.accent } else { t.border })
        .bg(t.background)
        .cursor_text()
        .child(body)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev, _, cx| this.export_field_down(ix, ev, cx)),
        )
        .on_mouse_move(cx.listener(move |this, ev, _, cx| this.export_field_drag(ix, ev, cx)))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.recordings.field_drag = None),
        )
}

fn rgb_u32(c: mt_theme::Rgba) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

/// "macos" shows as "macOS", the rest capitalised.
fn enum_label(v: &str) -> String {
    match v {
        "macos" => "macOS".into(),
        _ => {
            let mut c = v.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

/// A small preview of a theme: its background with "Aa" and three palette dots.
fn theme_chip(t: &Theme, theme: Option<&mt_theme::Theme>) -> gpui::Div {
    let chip = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(3.))
        .px(px(5.))
        .w(px(58.))
        .h(px(18.))
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(t.border);
    let Some(th) = theme else {
        return chip.bg(t.surface);
    };
    chip.bg(gpui::rgb(rgb_u32(th.background)))
        .child(
            div()
                .text_size(px(10.))
                .text_color(gpui::rgb(rgb_u32(th.foreground)))
                .child("Aa"),
        )
        .children([1usize, 2, 4].map(|k| {
            div()
                .size(px(5.))
                .rounded(px(2.5))
                .bg(gpui::rgb(rgb_u32(th.ansi[k])))
        }))
}

/// One option of the dialog: label (with its help text when `wide`) over its control.
fn export_option(
    t: &Theme,
    d: &ExportDialog,
    i: usize,
    spec: &mt_export::OptionSpec,
    wide: bool,
    layouts: &super::panel::ExportLayouts,
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let n = i + 1;
    let focused = d.focus == n;
    let edit = &d.fields[d.format][i];
    let key = spec.key;
    let input = match (&spec.kind, key) {
        (_, "theme") => {
            let shown = if edit.text().is_empty() {
                d.sidecar.theme.as_str()
            } else {
                edit.text()
            };
            let found = d.themes.iter().find(|x| x.name.eq_ignore_ascii_case(shown));
            let name = if edit.text().is_empty() {
                if shown.is_empty() {
                    "Recording's theme".to_string()
                } else {
                    format!("Recording's ({shown})")
                }
            } else {
                shown.to_string()
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(26.))
                .px_2()
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(if focused { t.accent } else { t.border })
                .bg(t.background)
                .cursor_pointer()
                .child(theme_chip(t, found))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(name),
                )
                .child(Icon::new(IconName::ChevronDown).color(t.text_muted))
                .into_any_element()
        }
        (_, "margin_fill") => {
            let cur = edit.text().trim().to_string();
            let parsed = mt_theme::Rgba::parse(&cur).ok();
            let invalid = !cur.is_empty() && parsed.is_none();
            let pick = |value: &'static str| {
                cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this.recordings.dialog.as_mut() {
                        d.focus = n;
                        d.set_value("margin_fill", value);
                    }
                    cx.stop_propagation();
                    cx.notify();
                })
            };
            let auto = div()
                .id(("export-swatch-auto", i))
                .flex()
                .items_center()
                .px_2()
                .h(px(18.))
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(if cur.is_empty() { t.accent } else { t.border })
                .text_size(px(10.))
                .cursor_pointer()
                .child("Auto")
                .on_click(pick(""));
            let swatches = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(5.))
                .child(auto)
                .children(SWATCHES.iter().enumerate().map(|(k, hex)| {
                    let c = mt_theme::Rgba::parse(hex).unwrap_or(mt_theme::Rgba::rgb(0, 0, 0));
                    div()
                        .id(("export-swatch", i * 32 + k))
                        .size(px(18.))
                        .rounded(px(RADIUS_CONTROL))
                        .bg(gpui::rgb(rgb_u32(c)))
                        .border_1()
                        .border_color(if cur.eq_ignore_ascii_case(hex) {
                            t.accent
                        } else {
                            t.border
                        })
                        .cursor_pointer()
                        .on_click(pick(hex))
                }));
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(swatches)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .size(px(26.))
                                .flex_none()
                                .rounded(px(RADIUS_CONTROL))
                                .border_1()
                                .border_color(if invalid { t.error } else { t.border })
                                .bg(match parsed {
                                    Some(c) => gpui::rgb(rgb_u32(c)).into(),
                                    None => t.surface,
                                }),
                        )
                        .child(
                            export_text_field(
                                t,
                                edit,
                                n,
                                focused,
                                "#rrggbb, empty = automatic",
                                layouts,
                                cx,
                            )
                            .flex_1(),
                        ),
                )
                .into_any_element()
        }
        (OptKind::Bool, _) => div()
            .flex()
            .items_center()
            .h(px(26.))
            .child(toggle(t, edit.text() == "true"))
            .into_any_element(),
        (OptKind::Enum(values), _) => div()
            .flex()
            .gap_1()
            .children(values.iter().enumerate().map(|(k, v)| {
                let on = edit.text() == *v;
                let v: &'static str = v;
                div()
                    .id(("export-enum", i * 16 + k))
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS_CONTROL))
                    .border_1()
                    .border_color(if on { t.accent } else { t.border })
                    .when(on, |el| el.bg(t.element_selected))
                    .cursor_pointer()
                    .child(enum_label(v))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.recordings.dialog.as_mut() {
                            d.focus = n;
                            d.fields[d.format][i] = crate::components::TextEdit::new(v);
                            d.error = None;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
            }))
            .into_any_element(),
        _ => export_text_field(t, edit, n, focused, "", layouts, cx).into_any_element(),
    };
    let is_bool = matches!(spec.kind, OptKind::Bool);
    let is_theme = key == "theme";
    div()
        .id(("export-opt", i))
        .flex()
        .flex_col()
        .gap_1()
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(d) = this.recordings.dialog.as_mut() {
                d.focus = n;
                if is_bool {
                    d.cycle_value(1);
                } else if is_theme {
                    d.toggle_theme_sheet();
                }
            }
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(if focused { t.text } else { t.text_muted })
                        .whitespace_nowrap()
                        .child(spec.label),
                )
                .when(wide, |el| {
                    el.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(t.text_placeholder)
                            .child(spec.help),
                    )
                }),
        )
        .child(input)
        .into_any_element()
}

/// The theme list: a sheet over the left pane with a preview chip per theme.
fn theme_sheet(
    t: &Theme,
    d: &ExportDialog,
    width: gpui::Pixels,
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let ix = d.spec_ix("theme");
    let current = d.value("theme").to_string();
    let mut entries: Vec<(String, String, Option<&mt_theme::Theme>)> = vec![(
        String::new(),
        if d.sidecar.theme.is_empty() {
            "Recording's theme".to_string()
        } else {
            format!("Recording's theme ({})", d.sidecar.theme)
        },
        d.themes
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(&d.sidecar.theme)),
    )];
    entries.extend(
        d.themes
            .iter()
            .map(|x| (x.name.clone(), x.name.clone(), Some(x))),
    );
    let rows = entries
        .into_iter()
        .enumerate()
        .map(|(k, (value, label, th))| {
            let on = value == current;
            div()
                .id(("export-theme", k))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(28.))
                .flex_none()
                .when(on, |el| el.bg(t.element_selected))
                .when(!on, |el| el.hover(|s| s.bg(t.element_hover)))
                .cursor_pointer()
                .child(theme_chip(t, th))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(label),
                )
                .when(on, |el| el.child(Icon::new(IconName::Check)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this.recordings.dialog.as_mut() {
                        d.set_value("theme", &value);
                        d.theme_open = false;
                        if let Some(i) = ix {
                            d.focus = i + 1;
                        }
                    }
                    cx.notify();
                }))
        });
    div()
        .id("export-theme-sheet")
        .absolute()
        .top(px(SHEET_TOP))
        .left(px(16.))
        .w(width)
        .h(px(SHEET_HEIGHT))
        .flex()
        .flex_col()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(t.border)
        .bg(t.elevated_surface)
        .shadow_lg()
        .occlude()
        .child(
            div()
                .flex()
                .items_center()
                .px_2()
                .h(px(30.))
                .flex_none()
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Theme"),
                )
                .child(
                    IconButton::new("export-theme-close", IconName::X)
                        .tooltip("Close")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(d) = this.recordings.dialog.as_mut() {
                                d.theme_open = false;
                            }
                            cx.notify();
                        })),
                ),
        )
        .child(
            div()
                .id("export-theme-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&d.theme_scroll)
                .flex()
                .flex_col()
                .children(rows),
        )
        .into_any_element()
}

/// The export dialog is wide so option fields fit two per row.
const DIALOG_WIDTH: f32 = 800.;
/// Gap between the two option panes.
const PANE_GAP: f32 = 24.;
/// The theme sheet covers the left pane from just under the format row.
const SHEET_TOP: f32 = 108.;
const SHEET_HEIGHT: f32 = 330.;
/// Option fields scroll past this height so the dialog fits short windows.
const FIELDS_MAX_HEIGHT: f32 = 460.;

fn progress_bar(t: &Theme, frac: f32) -> gpui::Div {
    div().h(px(4.)).w_full().rounded(px(2.)).bg(t.border).child(
        div()
            .h_full()
            .w(gpui::relative(frac.max(0.02)))
            .rounded(px(2.))
            .bg(t.accent),
    )
}
