//! Rendering of the Triggers side panel (SPEC §10.11.5): the Rules list, the editor, the Test
//! results and the Findings list. State is in [`super::ui`] and [`super::editor`].

use gpui::{
    canvas, div, prelude::*, px, AnyElement, BorrowAppContext, ClickEvent, Context, FontWeight,
    MouseButton, MouseDownEvent,
};
use mt_triggers::rules::Field;
use mt_triggers::{Action, EventKind, Gate, Rule, When};

use super::editor::{params, Focus, Input, ACTION_GROUPS, EVENTS};
use super::gate::TriggerGate;
use super::runtime::{self, TriggerRuntime};
use super::ui::PanelTab;
use crate::{
    app::AppWindow,
    assets::IconName,
    components::{Button, ButtonStyle, Icon, IconButton},
    metrics::RADIUS_CONTROL,
    snippets::panel_view::{field_box, line_field, toggle},
    theme::Theme,
};

/// `just now`, `42 s ago`, `2 min ago`, `3 h ago`.
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let s = now_ms.saturating_sub(then_ms) / 1000;
    match s {
        0..=4 => "just now".into(),
        5..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

/// `text · /error/ · 3 actions` or `event · command_finished · failure`.
pub fn summary(rule: &Rule) -> String {
    match &rule.when {
        When::Text { regex, .. } => {
            let n = rule.actions.len();
            format!(
                "text · /{regex}/ · {n} action{}",
                if n == 1 { "" } else { "s" }
            )
        }
        When::Event { event, exit } => {
            let exit = match exit {
                mt_triggers::ExitFilter::Any => String::new(),
                mt_triggers::ExitFilter::Success => " · success".into(),
                mt_triggers::ExitFilter::Failure => " · failure".into(),
                mt_triggers::ExitFilter::Code(c) => format!(" · exit {c}"),
            };
            format!("event · {}{exit}", event.name())
        }
    }
}

fn muted(t: &Theme, text: impl Into<String>) -> gpui::Div {
    div()
        .min_w_0()
        .whitespace_normal()
        .text_size(px(11.))
        .text_color(t.text_muted)
        .child(text.into())
}

fn error_line(t: &Theme, text: String) -> gpui::Div {
    div()
        .min_w_0()
        .whitespace_normal()
        .text_size(px(11.))
        .text_color(t.error)
        .child(text)
}

fn label(t: &Theme, text: &'static str) -> gpui::Div {
    div()
        .mt_2()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_muted)
        .child(text)
}

fn pill(t: &Theme, text: String, active: bool) -> gpui::Div {
    div()
        .px_2()
        .h(px(22.))
        .flex()
        .items_center()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(if active { t.accent } else { t.border })
        .bg(if active {
            t.accent.opacity(0.18)
        } else {
            t.background
        })
        .cursor_pointer()
        .child(text)
}

impl AppWindow {
    /// The panel, or `None` while it is closed or the feature is off.
    pub(crate) fn triggers_panel(
        &mut self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.triggers_visible() {
            return None;
        }
        let width = self.triggers.panel.width;
        let body = self.triggers_body(t, cx);
        let entity = cx.entity();
        let (e_move, e_up) = (entity.clone(), entity);
        let tracker = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let e = e_move.clone();
                window.on_mouse_event(move |ev: &gpui::MouseMoveEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        e.update(cx, |this, cx| {
                            if this.triggers.drag.is_some() {
                                this.drag_triggers_to(f32::from(ev.position.x), cx);
                            }
                        });
                    }
                });
                let e = e_up.clone();
                window.on_mouse_event(move |_: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        e.update(cx, |this, cx| this.end_triggers_drag(window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();
        Some(
            div()
                .id("triggers-panel")
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
                .track_focus(&self.triggers.focus)
                .on_key_down(cx.listener(|this, ev, w, cx| this.triggers_key(ev, w, cx)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, w, cx| {
                        if !this.triggers.panel.focused {
                            this.triggers.panel.focused = true;
                            this.snippets.panel.focused = false;
                            this.recordings.panel.focused = false;
                            this.sync_focus(w);
                            cx.notify();
                        }
                    }),
                )
                .child(tracker)
                .child(
                    div()
                        .id("triggers-resize")
                        .absolute()
                        .left(px(-3.))
                        .top_0()
                        .bottom_0()
                        .w(px(6.))
                        .cursor_col_resize()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                this.begin_triggers_drag(f32::from(ev.position.x));
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(body)
                .into_any_element(),
        )
    }

    fn triggers_body(&mut self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let in_editor = self.triggers.editor.is_some();
        let tab = self.triggers.tab;
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(32.))
            .px_2()
            .border_b_1()
            .border_color(t.border_variant)
            .child(Icon::new(IconName::Regex).color(t.text_muted))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Triggers"),
            )
            .when(!in_editor, |el| {
                el.child(
                    pill(t, "Rules".into(), tab == PanelTab::Rules)
                        .id("triggers-tab-rules")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.triggers.tab = PanelTab::Rules;
                            cx.notify();
                        })),
                )
                .child(
                    pill(t, "Findings".into(), tab == PanelTab::Findings)
                        .id("triggers-tab-findings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.triggers.tab = PanelTab::Findings;
                            cx.notify();
                        })),
                )
                .child(
                    IconButton::new("triggers-new", IconName::Plus)
                        .tooltip("New trigger")
                        .on_click(cx.listener(|this, _, w, cx| this.new_trigger(w, cx))),
                )
            })
            .child(
                IconButton::new("triggers-hide", IconName::X)
                    .tooltip("Hide panel")
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.triggers.close_panel();
                        this.after_panel_change(w, cx);
                    })),
            );
        let banner = runtime::banner(cx).map(|b| {
            div()
                .flex_none()
                .px_2()
                .py_1()
                .bg(t.error.opacity(0.15))
                .text_color(t.error)
                .text_size(px(11.))
                .child(b.text)
        });
        let main = if in_editor {
            self.trigger_editor_view(t, cx)
        } else if tab == PanelTab::Findings {
            self.findings_view(t, cx)
        } else {
            self.rules_view(t, cx)
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .children(banner)
            .child(main)
            .into_any_element()
    }

    // ------------------------------------------------------------------ rules list

    fn rules_view(&mut self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let now = mt_term::now_ms();
        struct Row {
            id: String,
            name: String,
            on: bool,
            summary: String,
            fired: u64,
            last: Option<String>,
            error: Option<String>,
            suppressed: u64,
            /// The last file a capture action of this rule wrote.
            capture: Option<std::path::PathBuf>,
        }
        let (rows, paused, dropped, secrets) = match cx.try_global::<TriggerRuntime>() {
            Some(rt) => {
                let gate = TriggerGate::of(cx);
                let rows: Vec<Row> = rt
                    .engine
                    .rules()
                    .map(|r| {
                        let s = rt.engine.stats(&r.id);
                        Row {
                            id: r.id.clone(),
                            name: r.name.clone(),
                            on: rt.engine.is_enabled(&r.id),
                            summary: summary(r),
                            fired: s.fired,
                            last: s.last_fired_ms.map(|m| ago(now, m)),
                            error: s.last_error,
                            suppressed: rt.engine.suppressed(&r.id),
                            capture: rt.captures_last.get(&r.id).cloned(),
                        }
                    })
                    .collect();
                let secrets = gate.allow_capture
                    && rt.engine.rules().any(|r| {
                        rt.engine.is_enabled(&r.id)
                            && r.actions.iter().any(|a| {
                                matches!(a, Action::Snapshot { .. } | Action::Screenshot { .. })
                            })
                    });
                (
                    rows,
                    rt.engine.paused(),
                    runtime::dropped_lines(cx),
                    secrets,
                )
            }
            None => (Vec::new(), false, 0, false),
        };
        let mut list = div()
            .id("triggers-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if rows.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .p_4()
                    .child(div().text_color(t.text_muted).child("No triggers yet."))
                    .child(
                        Button::new("triggers-empty-new", "New trigger")
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(|this, _, w, cx| this.new_trigger(w, cx))),
                    ),
            );
        }
        for (ix, r) in rows.into_iter().enumerate() {
            let (id_row, id_switch) = (r.id.clone(), r.id.clone());
            let on = r.on;
            list = list.child(
                div()
                    .id(("trigger-row", ix))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.element_hover))
                    .on_click(cx.listener(move |this, _, w, cx| this.edit_trigger(&id_row, w, cx)))
                    .child(
                        toggle(t, on)
                            .id(("trigger-switch", ix))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_trigger_enabled(&id_switch, !on, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_color(if on { t.text } else { t.text_muted })
                                            .child(r.name),
                                    )
                                    .when_some(r.error.clone(), |el, why| {
                                        el.child(
                                            div()
                                                .id(("trigger-error", ix))
                                                .size(px(7.))
                                                .rounded_full()
                                                .bg(t.error)
                                                .tooltip(move |_, cx| {
                                                    cx.new(|_| {
                                                        crate::components::Tooltip::new(why.clone())
                                                    })
                                                    .into()
                                                }),
                                        )
                                    }),
                            )
                            .child(muted(t, r.summary).overflow_hidden().whitespace_nowrap())
                            .child(muted(
                                t,
                                match (&r.last, r.suppressed) {
                                    (Some(l), 0) => format!("fired {} · last fired {l}", r.fired),
                                    (Some(l), s) => format!(
                                        "fired {} · last fired {l} · suppressed {s}",
                                        r.fired
                                    ),
                                    (None, _) => "not fired yet".to_string(),
                                },
                            ))
                            .when_some(r.capture.clone(), |el, path| {
                                let name = path
                                    .file_name()
                                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                                let dir = path
                                    .parent()
                                    .map(std::path::Path::to_path_buf)
                                    .unwrap_or(path);
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            muted(t, format!("last capture: {name}"))
                                                .overflow_hidden(),
                                        )
                                        .child(
                                            IconButton::new(
                                                ("capture-reveal", ix),
                                                IconName::FolderOpen,
                                            )
                                            .tooltip("Reveal the captures folder")
                                            .on_click(
                                                cx.listener(move |_, _, _, cx| {
                                                    cx.stop_propagation();
                                                    cx.reveal_path(&dir);
                                                }),
                                            ),
                                        ),
                                )
                            }),
                    ),
            );
        }
        let footer = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(t.border_variant)
            .when(secrets, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.))
                        .text_color(t.warning)
                        .child(Icon::new(IconName::TriangleAlert).color(t.warning))
                        .child("Captures may contain secrets"),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new(
                            "triggers-pause",
                            if paused {
                                "Paused — resume"
                            } else {
                                "Pause all"
                            },
                        )
                        .style(if paused {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Subtle
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_triggers_paused(cx))),
                    )
                    .child(div().flex_1().child(muted(
                        t,
                        if dropped > 0 {
                            format!("{dropped} lines dropped")
                        } else {
                            String::new()
                        },
                    )))
                    .child(
                        IconButton::new("triggers-file", IconName::FileText)
                            .tooltip("Open triggers.toml")
                            .on_click(cx.listener(|this, _, w, cx| this.open_triggers_file(w, cx))),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(list)
            .child(footer)
            .into_any_element()
    }

    // ------------------------------------------------------------------ findings

    fn findings_view(&mut self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let now = mt_term::now_ms();
        let items: Vec<(usize, super::findings::Finding, bool)> =
            match cx.try_global::<TriggerRuntime>() {
                Some(rt) => {
                    let mut v = Vec::new();
                    for (i, f) in rt.findings.items().enumerate() {
                        let out = crate::mux_host::host_ref(cx)
                            .mux
                            .locate_pane(f.pane)
                            .is_none();
                        v.push((i, f.clone(), out));
                    }
                    v
                }
                None => Vec::new(),
            };
        // Whether each line left the scrollback is decided by the pane's terminal.
        let mut list = div()
            .id("findings-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if items.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_color(t.text_muted)
                    .child("No findings yet. Add a “findings” action to a rule."),
            );
        }
        for (i, f, pane_gone) in items.into_iter().rev() {
            let base = cx
                .try_global::<TriggerRuntime>()
                .and_then(|rt| rt.line_bases.get(&f.pane).copied());
            let scrolled_out = pane_gone || base.is_some_and(|b| f.scrolled_out(b));
            let text = f.line.clone();
            let copy = text.clone();
            list = list.child(
                div()
                    .id(("finding", i))
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.click_finding(i, cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(f.rule_name.clone()),
                                    )
                                    .when(f.count > 1, |el| {
                                        el.child(muted(t, format!("×{}", f.count)))
                                    })
                                    .child(muted(
                                        t,
                                        format!("pane {} · {}", f.pane, ago(now, f.at_ms)),
                                    ))
                                    .when(scrolled_out, |el| el.child(muted(t, "scrolled out"))),
                            )
                            .child(div().text_size(px(11.)).child(text)),
                    )
                    .child(
                        IconButton::new(("finding-copy", i), IconName::Copy)
                            .tooltip("Copy text")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    copy.clone(),
                                ));
                            })),
                    ),
            );
        }
        let whole = self.triggers.flash_whole_line;
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .p_2()
            .child(
                toggle(t, whole)
                    .id("findings-whole-line")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.triggers.flash_whole_line = !this.triggers.flash_whole_line;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .child("Mark whole line"),
            )
            .border_t_1()
            .border_color(t.border_variant)
            .child(
                Button::new("findings-clear", "Clear").on_click(cx.listener(|_, _, _, cx| {
                    cx.update_global::<TriggerRuntime, _>(|rt, _| rt.findings.clear());
                })),
            );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(list)
            .child(footer)
            .into_any_element()
    }

    // ------------------------------------------------------------------ editor

    fn trigger_editor_view(&mut self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let gate = TriggerGate::of_config(&self.cfg);
        let panel_focused = self.triggers.panel.focused;
        let panes_expr = self
            .triggers
            .editor
            .as_ref()
            .map(|e| e.panes.text().to_string())
            .unwrap_or_default();
        let panes_info = self.selector_pane_count(&panes_expr, cx);
        let Some(ed) = self.triggers.editor.as_ref() else {
            return div().into_any_element();
        };
        let focus = ed.focus;
        let focused = |f: Focus| panel_focused && focus == f;
        let diags = ed.diagnostics();
        let issue = |f: Field| diags.iter().find(|(x, _)| *x == f).map(|(_, m)| m.clone());
        let blocker = ed.save_blocker();
        let placeholders = ed.placeholders();
        let is_new = ed.id.is_none();
        let (confirm_discard, confirm_delete, stale, error) = (
            ed.confirm_discard,
            ed.confirm_delete,
            ed.stale,
            ed.error.clone(),
        );

        let layouts = self.triggers.field_layout.clone();
        let field = |id: &'static str,
                     f: Focus,
                     edit: &crate::components::TextEdit,
                     ph: &'static str,
                     cx: &mut Context<Self>| {
            let is_focused = focused(f);
            let text = edit.text().to_string();
            let (a, b) = edit.selection().unwrap_or((edit.caret(), edit.caret()));
            let caret = edit.caret();
            let (accent, placeholder_c) = (t.accent, t.text_placeholder);
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
                    store.borrow_mut().insert(f, (bounds, line));
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
                    el.child(div().text_color(placeholder_c).child(ph.to_string()))
                })
                .child(text[..a].to_string())
                .when(is_focused && caret == a, |el| el.child(caret_bar()))
                .when(b > a, |el| {
                    el.child(div().bg(accent.opacity(0.35)).child(text[a..b].to_string()))
                })
                .when(is_focused && caret == b && b > a, |el| {
                    el.child(caret_bar())
                })
                .child(text[b..].to_string());
            field_box(t, is_focused, body)
                .id(id)
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev, _, cx| this.editor_field_down(f, ev, cx)),
                )
                .on_mouse_move(
                    cx.listener(move |this, ev, _, cx| this.editor_field_drag(f, ev, cx)),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.triggers.field_drag = None),
                )
        };

        let mut col = div()
            .id("trigger-editor")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    IconButton::new("editor-back", IconName::ChevronRight)
                        .tooltip("Back")
                        .on_click(cx.listener(|this, _, _, cx| this.rule_editor_escape(cx))),
                )
                .child(div().font_weight(FontWeight::SEMIBOLD).child(if is_new {
                    "New trigger"
                } else {
                    "Edit trigger"
                })),
        );
        // Name.
        col = col
            .child(label(t, "Name"))
            .child(field(
                "editor-name",
                Focus::Name,
                &ed.name,
                "Build failed",
                cx,
            ))
            .children(issue(Field::Name).map(|m| error_line(t, m)));
        // Applies to.
        col = col
            .child(label(t, "Applies to"))
            .child(field(
                "editor-panes",
                Focus::Panes,
                &ed.panes,
                "every pane (e.g. domain:local)",
                cx,
            ))
            .child(match (&panes_info, issue(Field::Panes)) {
                (_, Some(m)) => error_line(t, m),
                (Ok(n), None) => muted(
                    t,
                    format!("matches {n} pane{}", if *n == 1 { "" } else { "s" }),
                ),
                (Err(m), None) => error_line(t, m.clone()),
            });
        // When.
        let is_text = ed.is_text;
        col =
            col.child(label(t, "When")).child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        pill(t, "Text".into(), is_text)
                            .id("when-text")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(ed) = this.triggers.editor.as_mut() {
                                    ed.set_text_kind(true);
                                }
                                this.schedule_trigger_preview(cx);
                                cx.notify();
                            })),
                    )
                    .child(pill(t, "Event".into(), !is_text).id("when-event").on_click(
                        cx.listener(|this, _, _, cx| {
                            if let Some(ed) = this.triggers.editor.as_mut() {
                                ed.set_text_kind(false);
                            }
                            this.schedule_trigger_preview(cx);
                            cx.notify();
                        }),
                    )),
            );
        if is_text {
            let ci = ed.case_insensitive;
            let output_only = matches!(ed.scope, mt_triggers::MatchIn::Output);
            col = col
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            field("editor-regex", Focus::Regex, &ed.regex, "error: (.+)", cx)
                                .flex_1(),
                        )
                        .child(
                            IconButton::new("editor-ci", IconName::CaseSensitive)
                                .tooltip(if ci {
                                    "Case-insensitive (on)"
                                } else {
                                    "Case-insensitive (off)"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(ed) = this.triggers.editor.as_mut() {
                                        ed.case_insensitive = !ed.case_insensitive;
                                    }
                                    this.schedule_trigger_preview(cx);
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            toggle(t, output_only)
                                .id("editor-scope")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(ed) = this.triggers.editor.as_mut() {
                                        ed.scope = match ed.scope {
                                            mt_triggers::MatchIn::Output => {
                                                mt_triggers::MatchIn::Any
                                            }
                                            mt_triggers::MatchIn::Any => {
                                                mt_triggers::MatchIn::Output
                                            }
                                        };
                                        ed.test = None;
                                    }
                                    this.schedule_trigger_preview(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .child("Output only"),
                        ),
                )
                .children(issue(Field::Regex).map(|m| error_line(t, m)))
                .children(ed.preview.clone().map(|p| muted(t, p)));
        } else {
            let ev = ed.event;
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        pill(t, ev.name().to_string(), true)
                            .id("editor-event")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(ed) = this.triggers.editor.as_mut() {
                                    let i = EVENTS
                                        .iter()
                                        .position(|e| *e == ed.event)
                                        .map_or(0, |i| (i + 1) % EVENTS.len());
                                    ed.set_event(EVENTS[i]);
                                }
                                cx.notify();
                            })),
                    )
                    .when(ev.has_exit(), |el| {
                        el.child(muted(t, "exit")).child(
                            field(
                                "editor-exit",
                                Focus::Exit,
                                &ed.exit,
                                "any | success | failure | N",
                                cx,
                            )
                            .flex_1(),
                        )
                    }),
            );
            col = col.children(issue(Field::Exit).map(|m| error_line(t, m)));
        }
        // Actions.
        col = col.child(label(t, "Actions"));
        let n_actions = ed.actions.len();
        for (ai, a) in ed.actions.iter().enumerate() {
            let kind = a.kind;
            let gate_hint = ed_action_gate(a)
                .filter(|g| !gate.allows(Some(*g)))
                .map(|g| g.hint());
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(t.border_variant)
                .when(gate_hint.is_some(), |el| el.opacity(0.55))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(kind))
                        .child(
                            IconButton::new(("action-up", ai), IconName::ArrowUp)
                                .tooltip("Run earlier")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(ed) = this.triggers.editor.as_mut() {
                                        ed.move_action(ai, true);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            IconButton::new(("action-down", ai), IconName::ArrowDown)
                                .tooltip("Run later")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(ed) = this.triggers.editor.as_mut() {
                                        ed.move_action(ai, false);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            IconButton::new(("action-remove", ai), IconName::Trash2)
                                .tooltip("Remove action")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(ed) = this.triggers.editor.as_mut() {
                                        ed.remove_action(ai);
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            for (pi, p) in params(kind).iter().enumerate() {
                let value = a
                    .fields
                    .get(pi)
                    .map_or(String::new(), |f| f.text().to_string());
                let err = issue(Field::Action(ai, p.key));
                let row = match p.input {
                    Input::Text { .. } => {
                        let f = Focus::Param(ai, pi);
                        let edit = a.fields[pi].clone();
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(110.))
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(t.text_muted)
                                    .child(p.label),
                            )
                            .child(
                                field_box(t, focused(f), line_field(t, &edit, focused(f), ""))
                                    .id(("param", ai * 16 + pi))
                                    .flex_1()
                                    .cursor_text()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(ed) = this.triggers.editor.as_mut() {
                                            ed.focus = f;
                                        }
                                        cx.notify();
                                    })),
                            )
                    }
                    Input::Choice(_) => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(110.))
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(t.text_muted)
                                .child(p.label),
                        )
                        .child(pill(t, value, true).id(("param", ai * 16 + pi)).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(ed) = this.triggers.editor.as_mut() {
                                    ed.cycle_param(ai, pi);
                                }
                                cx.notify();
                            }),
                        )),
                    Input::Bool => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(110.))
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(t.text_muted)
                                .child(p.label),
                        )
                        .child(
                            toggle(t, value == "true")
                                .id(("param", ai * 16 + pi))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(ed) = this.triggers.editor.as_mut() {
                                        ed.actions[ai].cycle(pi);
                                    }
                                    cx.notify();
                                })),
                        ),
                };
                card = card.child(row).children(err.map(|m| error_line(t, m)));
            }
            if let Some(h) = gate_hint {
                card = card.child(muted(t, h));
            }
            if kind == "clipboard" && !is_text {
                card = card.child(muted(
                    t,
                    "An event has no matched text, so nothing is copied. Use a text rule.",
                ));
            }
            if kind == "screenshot" {
                card = card.child(muted(
                    t,
                    "A screenshot shows the whole visible screen and cannot be redacted.",
                ));
            }
            col = col.child(card);
        }
        if let Some(m) = issue(Field::Actions) {
            col = col.child(error_line(t, m));
        }
        if ed.focus_is_template() && !placeholders.is_empty() {
            col = col.child(muted(
                t,
                format!(
                    "Placeholders: {}",
                    placeholders
                        .iter()
                        .map(|p| format!("{{{p}}}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            ));
        }
        // Add action.
        if n_actions < mt_triggers::MAX_ACTIONS {
            let open = ed.add_menu;
            col = col.child(
                Button::new("editor-add-action", "Add action").on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(ed) = this.triggers.editor.as_mut() {
                            ed.add_menu = !ed.add_menu;
                        }
                        cx.notify();
                    },
                )),
            );
            if open {
                for (group, kinds) in ACTION_GROUPS {
                    col = col.child(muted(t, group));
                    let mut row = div().flex().flex_wrap().gap_1();
                    for kind in kinds {
                        let kind = *kind;
                        let key = ACTION_GROUPS
                            .iter()
                            .flat_map(|(_, k)| k.iter())
                            .position(|k| *k == kind)
                            .unwrap_or(0);
                        row = row.child(
                            pill(t, kind.to_string(), false)
                                .id(("add-action", key))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(ed) = this.triggers.editor.as_mut() {
                                        ed.add_action(kind);
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    col = col.child(row);
                }
            }
        } else {
            col = col.child(muted(t, "At most 4 actions per rule."));
        }
        // Cooldown.
        col = col
            .child(label(t, "Cooldown (ms)"))
            .child(field(
                "editor-cooldown",
                Focus::Cooldown,
                &ed.cooldown,
                "1000",
                cx,
            ))
            .children(issue(Field::Cooldown).map(|m| error_line(t, m)));
        // Test results.
        if let Some(results) = &ed.test {
            col = col.child(label(t, "Test (nothing was run)"));
            if results.is_empty() {
                col = col.child(muted(
                    t,
                    "No firings: nothing in the last 200 lines matches.",
                ));
            }
            for r in results.iter().take(20) {
                col = col.child(
                    div()
                        .flex()
                        .flex_col()
                        .p_1()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(t.background)
                        .child(
                            div()
                                .text_size(px(11.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(r.input.clone()),
                        )
                        .children(
                            r.intents
                                .iter()
                                .map(|i| muted(t, format!("→ {}", i.describe()))),
                        ),
                );
            }
        }
        if let Some(e) = error.clone() {
            col = col.child(error_line(t, e));
        }
        if stale {
            col = col.child(
                Button::new("editor-reload", "Reload and re-apply")
                    .on_click(cx.listener(|this, _, _, cx| this.reload_rule_editor(cx))),
            );
        }
        // Buttons.
        let buttons = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(t.border_variant)
            .child(if let Some(b) = blocker.clone() {
                div()
                    .id("editor-save-disabled")
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS_CONTROL))
                    .bg(t.border)
                    .text_color(t.text_muted)
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::components::Tooltip::new(b.clone()))
                            .into()
                    })
                    .child("Save")
                    .into_any_element()
            } else {
                Button::new("editor-save", "Save")
                    .style(ButtonStyle::Filled)
                    .on_click(cx.listener(|this, _, _, cx| this.save_rule_editor(cx)))
                    .into_any_element()
            })
            .child(
                Button::new("editor-cancel", "Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.rule_editor_escape(cx))),
            )
            .child(
                Button::new("editor-test", "Test")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test_rule_editor(cx))),
            )
            .when(!is_new, |el| {
                el.child(
                    Button::new("editor-dup", "Duplicate")
                        .on_click(cx.listener(|this, _, _, cx| this.duplicate_rule_editor(cx))),
                )
                .child(
                    Button::new(
                        "editor-delete",
                        if confirm_delete {
                            "Really delete?"
                        } else {
                            "Delete"
                        },
                    )
                    .style(if confirm_delete {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.delete_rule_editor(cx))),
                )
            })
            .when(confirm_discard, |el| {
                el.child(error_line(
                    t,
                    "Unsaved changes — Esc again to discard".into(),
                ))
            });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(col)
            .child(buttons)
            .into_any_element()
    }
}

fn ed_action_gate(a: &super::editor::ActionEdit) -> Option<Gate> {
    a.to_action().ok().and_then(|x| x.gate())
}

#[allow(dead_code)]
fn _event_kind(_: EventKind) {}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_triggers::{ExitFilter, Rule};

    #[test]
    fn relative_times() {
        assert_eq!(ago(10_000, 10_000), "just now");
        assert_eq!(ago(50_000, 10_000), "40 s ago");
        assert_eq!(ago(10_000 + 120_000, 10_000), "2 min ago");
        assert_eq!(ago(10_000 + 3 * 3_600_000, 10_000), "3 h ago");
        assert_eq!(ago(5, 10), "just now", "a clock step back is harmless");
    }

    #[test]
    fn row_summaries() {
        let mut r = Rule::new_text("x", "error");
        assert_eq!(summary(&r), "text · /error/ · 1 action");
        r.actions.push(mt_triggers::Action::Bell {});
        assert_eq!(summary(&r), "text · /error/ · 2 actions");
        r.when = When::Event {
            event: EventKind::CommandFinished,
            exit: ExitFilter::Failure,
        };
        assert_eq!(summary(&r), "event · command_finished · failure");
        r.when = When::Event {
            event: EventKind::Bell,
            exit: ExitFilter::Any,
        };
        assert_eq!(summary(&r), "event · bell");
        r.when = When::Event {
            event: EventKind::PaneExited,
            exit: ExitFilter::Code(2),
        };
        assert_eq!(summary(&r), "event · pane_exited · exit 2");
    }
}
