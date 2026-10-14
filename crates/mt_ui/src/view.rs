//! Rendering for [`AppWindow`]: title bar, tab bar, viewport and every overlay.

use gpui::{
    div, font, prelude::*, px, AnyElement, ClickEvent, Context, Decorations, FontStyle, FontWeight,
    Hsla, KeyContext, MouseButton, SharedString, StrikethroughStyle, StyledText, TextRun,
    UnderlineStyle, Window, WindowControlArea,
};

use crate::{
    actions::*,
    app::{AppWindow, PickerData, QUICK_SELECT_ALPHABET},
    assets::IconName,
    components::*,
    metrics::*,
    mock::{Domain, Line, SpanStyle, Underline},
    overlay_stack::OverlayKind,
    platform::Platform,
    theme::Theme,
};

impl Render for AppWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        let csd = matches!(window.window_decorations(), Decorations::Client { .. })
            && self.platform == Platform::Linux;

        let mut root = div()
            .id("root")
            .key_context(CONTEXT_ROOT)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .font_family(UI_FONT)
            .text_size(px(UI_TEXT_SIZE))
            .when(csd, |el| el.border_1().border_color(t.border))
            .child(self.title_bar(&t, window, cx));

        if let Some(message) = self.config_error.clone() {
            root = root.child(
                Banner::new("config-error", ToastKind::Error, message)
                    .action(
                        "Open config",
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.on_stub("Opening the config in your editor", "M3", cx)
                        }),
                    )
                    .on_dismiss(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.config_error = None;
                        cx.notify();
                    })),
            );
        }

        root = root
            .child(self.tab_bar(cx))
            .child(self.viewport(&t, cx))
            .child(self.toast_stack(cx))
            .when_some(self.hud, |el, (cols, rows)| {
                el.child(resize_hud(&t, cols, rows))
            });

        if let Some((position, tab_id)) = self.tab_menu {
            root = root.child(self.tab_context_menu(position, tab_id, cx));
        }
        if let Some(picker) = &self.picker {
            root = root.child(self.picker_modal(picker, cx));
        }
        if let Some(top) = self.overlays.top() {
            root = root.child(self.focus_node(top, cx));
        }
        root.on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &SpawnTab, _, cx| this.on_action_spawn_tab(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.on_action_close_tab(cx)))
            .on_action(cx.listener(|this, a: &ActivateTab, _, cx| this.activate_tab(a.0, cx)))
            .on_action(cx.listener(|this, a: &ActivateTabRelative, _, cx| {
                this.on_action_tab_relative(a.0, cx)
            }))
            .on_action(cx.listener(|this, a: &SwitchWorkspaceRelative, _, cx| {
                this.on_action_workspace_relative(a.0, cx)
            }))
            .on_action(cx.listener(|_, _: &SpawnWindow, _, cx| {
                let options = crate::app::window_options(cx);
                cx.open_window(options, |window, cx| {
                    cx.new(|cx| AppWindow::new(window, cx))
                })
                .ok();
            }))
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.on_stub("Copy", "M7", cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.on_stub("Paste", "M2", cx)))
            .on_action(cx.listener(|this, _: &ClearScrollback, _, cx| {
                this.on_stub("Clear scrollback", "M2", cx)
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, _, cx| {
                this.on_stub("Font size changes", "M4", cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, _, cx| {
                this.on_stub("Font size changes", "M4", cx)
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, _, cx| {
                this.on_stub("Font size changes", "M4", cx)
            }))
            .on_action(
                cx.listener(|this, _: &ReloadConfig, _, cx| {
                    this.on_stub("Config reload", "M3", cx)
                }),
            )
            .on_action(cx.listener(|this, _: &OpenConfig, _, cx| {
                this.on_stub("Opening the config", "M3", cx)
            }))
            .on_action(cx.listener(|this, _: &ShowCommandPalette, w, cx| {
                this.toggle(OverlayKind::CommandPalette, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowLauncher, w, cx| {
                this.toggle(OverlayKind::Launcher, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowWorkspaceSwitcher, w, cx| {
                this.toggle(OverlayKind::WorkspaceSwitcher, w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectTheme, w, cx| {
                this.toggle(OverlayKind::ThemeSelector, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateQuickSelect, w, cx| {
                this.toggle(OverlayKind::QuickSelect, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateCopyMode, w, cx| {
                this.toggle(OverlayKind::CopyMode, w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &Search, w, cx| this.toggle(OverlayKind::Search, w, cx)),
            )
            .on_action(cx.listener(|this, _: &DebugShowSshOverlay, w, cx| {
                this.open_overlay(OverlayKind::SshReconnect, w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &DebugShowConfigError, _, cx| this.show_config_error(cx)),
            )
            .on_action(cx.listener(|this, _: &DebugShowToast, _, cx| {
                this.toast(
                    ToastKind::Info,
                    "This is a toast. It disappears after 4 seconds.",
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &DismissOverlay, w, cx| this.dismiss_top(w, cx)))
            .on_action(cx.listener(|this, _: &PickerUp, w, cx| this.on_picker_move(true, w, cx)))
            .on_action(cx.listener(|this, _: &PickerDown, w, cx| this.on_picker_move(false, w, cx)))
            .on_action(
                cx.listener(|this, _: &PickerConfirm, w, cx| this.on_picker_confirm(false, w, cx)),
            )
            .on_action(cx.listener(|this, _: &PickerConfirmInNewWindow, w, cx| {
                this.on_picker_confirm(true, w, cx)
            }))
    }
}

impl AppWindow {
    // ------------------------------------------------------------------ title bar

    fn title_bar(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let kind = self.active_kind();
        let reconnecting = self.overlays.contains(OverlayKind::SshReconnect);
        let (dot, domain) = match (reconnecting, kind.map(|k| k.domain())) {
            (true, _) => (StatusKind::Reconnecting, "prod · reconnecting"),
            (false, Some(Domain::Prod)) => (StatusKind::Connected, "prod"),
            _ => (StatusKind::Connected, "local"),
        };
        let title = kind.map(|k| k.window_title()).unwrap_or("microterm");
        let workspace = self.workspaces[self.active_workspace].name.clone();
        let linux = self.platform == Platform::Linux;
        let csd = matches!(window.window_decorations(), Decorations::Client { .. }) && linux;
        let mac_inset = if self.platform == Platform::Mac {
            TRAFFIC_LIGHTS_WIDTH
        } else {
            8.0
        };

        div()
            .id("title-bar")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(px(TITLE_BAR_HEIGHT))
            .pl(px(mac_inset))
            .pr_2()
            .bg(t.surface)
            .border_b_1()
            .border_color(t.border_variant)
            .window_control_area(WindowControlArea::Drag)
            .when(csd, |el| {
                el.on_mouse_down(MouseButton::Left, |e, window, _| {
                    if e.click_count == 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                })
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(UI_TEXT_SIZE_SMALL))
                    .text_color(t.text_muted)
                    .child(title),
            )
            .child(
                div()
                    .id("workspace-chip")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(22.))
                    .px_2()
                    .rounded(px(RADIUS_CONTROL))
                    .border_1()
                    .border_color(t.border)
                    .text_size(px(UI_TEXT_SIZE_SMALL))
                    .cursor_pointer()
                    .hover({
                        let hover = t.element_hover;
                        move |s| s.bg(hover)
                    })
                    .child(workspace)
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(px(12.))
                            .color(t.text_muted),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.toggle(OverlayKind::WorkspaceSwitcher, w, cx)
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(UI_TEXT_SIZE_SMALL))
                    .text_color(t.text_muted)
                    .child(StatusDot::new(dot))
                    .child(domain),
            )
            .when(csd, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .ml_3()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            IconButton::new("win-min", IconName::Minus)
                                .on_click(|_, w, _| w.minimize_window()),
                        )
                        .child(
                            IconButton::new("win-max", IconName::Square)
                                .on_click(|_, w, _| w.zoom_window()),
                        )
                        .child(
                            IconButton::new("win-close", IconName::X)
                                .on_click(|_, w, _| w.remove_window()),
                        ),
                )
            })
    }

    // ------------------------------------------------------------------ tab bar

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = TabBar::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            let id = tab.id;
            bar = bar.tab(
                Tab::new(("tab", id), tab.title.clone())
                    .active(i == self.active_tab)
                    .dirty(tab.kind.dirty())
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.activate_tab(i, cx)),
                    )
                    .on_close(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.close_tab(id, cx)),
                    )
                    .on_middle_click(cx.listener(move |this, _, _, cx| this.close_tab(id, cx)))
                    .on_right_click(cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        this.tab_menu = Some((e.position, id));
                        cx.notify();
                    })),
            );
        }
        bar.trailing(
            IconButton::new("new-tab", IconName::Plus)
                .tooltip("New tab")
                .on_click(cx.listener(|this, _, _, cx| this.on_action_spawn_tab(cx)))
                .on_right_click(
                    cx.listener(|this, _, w, cx| this.open_overlay(OverlayKind::Launcher, w, cx)),
                ),
        )
    }

    fn tab_context_menu(
        &self,
        position: gpui::Point<gpui::Pixels>,
        tab_id: u64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let platform = self.platform;
        let close_chip = ACTIONS
            .iter()
            .find(|a| a.name == "CloseTab")
            .and_then(|a| a.chip(platform))
            .unwrap_or_default();
        ContextMenu::new("tab-menu", position)
            .items(vec![
                ContextMenuItem::new("Close tab").chip(close_chip),
                ContextMenuItem::new("Close other tabs"),
                ContextMenuItem::new("Rename…"),
            ])
            .on_dismiss({
                let entity = cx.entity();
                move |_, app| {
                    entity.update(app, |this, cx| {
                        this.tab_menu = None;
                        cx.notify();
                    });
                }
            })
            .on_select({
                let entity = cx.entity();
                move |i, _, app| {
                    entity.update(app, |this, cx| match i {
                        0 => this.close_tab(tab_id, cx),
                        1 => {
                            this.tabs.retain(|t| t.id == tab_id);
                            this.active_tab = 0;
                            cx.notify();
                        }
                        _ => this.on_stub("Renaming tabs", "M5", cx),
                    });
                }
            })
    }

    // ------------------------------------------------------------------ viewport

    fn viewport(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = self.screen();
        let copy_active = self.overlays.contains(OverlayKind::CopyMode);
        let hits = if self.overlays.contains(OverlayKind::Search) {
            self.search_hits()
        } else {
            Vec::new()
        };
        let last_row = screen.len().saturating_sub(1);
        let focused_viewport = self.overlays.is_empty();

        let lines = screen.iter().enumerate().map(|(row, line)| {
            let line_hits: Vec<(usize, usize, bool)> = hits
                .iter()
                .enumerate()
                .filter(|(_, h)| h.0 == row)
                .map(|(i, h)| (h.1, h.2, i == self.search.current))
                .collect();
            let cursor = if copy_active && self.copy.row == row {
                Some(self.copy.col)
            } else if !copy_active
                && focused_viewport
                && row == last_row
                && self
                    .active_kind()
                    .is_some_and(|k| k.lines().last().is_some())
                && self
                    .tabs
                    .get(self.active_tab)
                    .is_some_and(|tab| tab.kind != crate::mock::MockKind::Cargo)
            {
                Some(line.cols())
            } else {
                None
            };
            div()
                .h(px(LINE_HEIGHT))
                .whitespace_nowrap()
                .child(styled_line(line, t, &line_hits, cursor))
        });

        let mut view = div()
            .id("viewport")
            .relative()
            .flex_1()
            .overflow_hidden()
            .bg(t.background)
            .track_focus(&self.viewport_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, w, cx| {
                    if this.overlays.is_empty() {
                        w.focus(&this.viewport_focus);
                        cx.notify();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|this, _, _, cx| this.on_scroll(cx)))
            .child(
                div()
                    .px(px(VIEWPORT_PADDING_X))
                    .py(px(VIEWPORT_PADDING_Y))
                    .font_family(TERM_FONT)
                    .text_size(px(TERM_TEXT_SIZE))
                    .line_height(px(LINE_HEIGHT))
                    .text_color(t.term.fg)
                    .children(lines),
            );

        if self.tabs.is_empty() {
            view = view.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(t.text_placeholder)
                    .child("No tabs open. Press the + button or use the new-tab shortcut."),
            );
        }

        if self.overlays.contains(OverlayKind::QuickSelect) {
            view = view.child(self.quick_select_chips(t));
        }
        view = view.child(self.mode_strip(t, cx));
        if self.scrollbar_visible {
            view = view.child(
                div()
                    .absolute()
                    .top(px(8.))
                    .right(px(3.))
                    .w(px(4.))
                    .h(px(120.))
                    .rounded_full()
                    .bg(t.text_muted.opacity(0.45)),
            );
        }
        if self.overlays.contains(OverlayKind::SshReconnect) {
            view = view.child(self.ssh_overlay(t, cx));
        }
        view
    }

    fn quick_select_chips(&self, t: &Theme) -> impl IntoElement {
        let typed = self.quick.typed.clone();
        let chips = self
            .quick
            .matches
            .iter()
            .filter(|(label, _)| label.starts_with(&typed))
            .map(|(label, hit)| {
                let x = hit.col as f32 * CELL_WIDTH;
                let y = hit.row as f32 * LINE_HEIGHT;
                let rest = label[typed.len()..].to_string();
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .h(px(LINE_HEIGHT))
                    .w(px(hit.text.chars().count() as f32 * CELL_WIDTH))
                    .rounded(px(2.))
                    .bg(t.accent.opacity(0.28))
                    .border_1()
                    .border_color(t.accent)
                    .child(
                        div()
                            .absolute()
                            .left(px(-1.))
                            .top(px(-1.))
                            .px_1()
                            .h(px(LINE_HEIGHT))
                            .flex()
                            .items_center()
                            .rounded(px(2.))
                            .bg(t.accent)
                            .text_color(if t.is_dark {
                                t.background
                            } else {
                                gpui::white()
                            })
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(11.))
                            .child(rest),
                    )
            });
        div()
            .absolute()
            .top(px(VIEWPORT_PADDING_Y))
            .left(px(VIEWPORT_PADDING_X))
            .font_family(TERM_FONT)
            .children(chips)
    }

    /// Mode pill and find bar, top-right of the viewport.
    fn mode_strip(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let pill = if self.overlays.contains(OverlayKind::CopyMode) {
            Some("COPY")
        } else if self.overlays.contains(OverlayKind::QuickSelect) {
            Some("QUICK SELECT")
        } else if self.overlays.contains(OverlayKind::Search) {
            Some("SEARCH")
        } else {
            None
        };
        div()
            .absolute()
            .top(px(8.))
            .right(px(16.))
            .flex()
            .items_center()
            .gap_2()
            .when_some(pill, |el, text| el.child(Pill::new(text)))
            .when(self.overlays.contains(OverlayKind::Search), |el| {
                el.child(self.search_bar(t, cx))
            })
    }

    fn search_bar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let hits = self.search_hits().len();
        let status = if self.search.query.is_empty() {
            String::new()
        } else if hits == 0 {
            "no results".to_string()
        } else {
            format!("{} of {}", self.search.current + 1, hits)
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(32.))
            .pl_2()
            .pr_1()
            .rounded(px(RADIUS_POPOVER))
            .border_1()
            .border_color(t.border)
            .bg(t.elevated_surface)
            .shadow_md()
            .child(
                div()
                    .w(px(180.))
                    .child(TextInput::new(self.search.query.clone()).placeholder("Find")),
            )
            .child(
                IconButton::new("search-case", IconName::CaseSensitive)
                    .tooltip("Match case")
                    .selected(self.search.case_sensitive)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_search_case(cx))),
            )
            .child(
                IconButton::new("search-regex", IconName::Regex)
                    .tooltip("Use regular expression")
                    .selected(self.search.regex)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_search_regex(cx))),
            )
            .child(
                div()
                    .min_w(px(52.))
                    .text_size(px(12.))
                    .text_color(t.text_muted)
                    .child(status),
            )
            .child(
                IconButton::new("search-prev", IconName::ArrowUp)
                    .tooltip("Previous match")
                    .on_click(cx.listener(|this, _, _, cx| this.step_search(false, cx))),
            )
            .child(
                IconButton::new("search-next", IconName::ArrowDown)
                    .tooltip("Next match")
                    .on_click(cx.listener(|this, _, _, cx| this.step_search(true, cx))),
            )
    }

    fn ssh_overlay(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("ssh-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(if t.is_dark { 0.55 } else { 0.35 }))
            .occlude()
            .child(
                div()
                    .w(px(340.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded(px(RADIUS_POPOVER))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.elevated_surface)
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::WifiOff).size(px(16.)).color(t.warning))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Connection to prod lost")),
                    )
                    .child(div().text_size(px(12.)).text_color(t.text_muted).child("deploy@prod.example.com:22 — your session keeps running on the remote host."))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(StatusDot::new(StatusKind::Reconnecting))
                            .child(format!("Reconnecting… attempt {}", self.ssh_attempt)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .justify_end()
                            .child(Button::new("ssh-close", "Close tab").on_click(cx.listener(|this, _, w, cx| this.close_ssh_tab(w, cx))))
                            .child(Button::new("ssh-reconnect", "Reconnect now").style(ButtonStyle::Filled).on_click(cx.listener(|this, _, _, cx| this.reconnect_now(cx)))),
                    ),
            )
    }

    // ------------------------------------------------------------------ pickers

    fn picker_modal(&self, picker: &PickerData, cx: &mut Context<Self>) -> impl IntoElement {
        let (offset, visible) = picker.state.visible();
        let rows: Vec<PickerRow> = visible
            .iter()
            .map(|m| {
                let entry = picker.state.item(m.index);
                PickerRow {
                    label: picker.state.label(m).to_string(),
                    positions: m.positions.clone(),
                    detail: entry.detail.clone(),
                    chip: entry.chip.clone(),
                    icon: entry.icon,
                    status: entry.status,
                }
            })
            .collect();
        let selected = (!visible.is_empty()).then(|| picker.state.selected() - offset);
        let entity = cx.entity();
        Modal::new("picker-modal")
            .width(px(PALETTE_WIDTH))
            .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
            .on_dismiss(cx.listener_for_dismiss())
            .child(
                Picker::new("picker", picker.state.query().to_string())
                    .placeholder(picker.placeholder)
                    .rows(rows, selected)
                    .on_click(move |i, window, app| {
                        entity.update(app, |this, cx| this.confirm_visible(i, window, cx));
                    }),
            )
    }

    /// Invisible element that owns keyboard focus while any overlay is open.
    fn focus_node(&self, top: OverlayKind, cx: &mut Context<Self>) -> AnyElement {
        let picker = matches!(
            top,
            OverlayKind::CommandPalette
                | OverlayKind::Launcher
                | OverlayKind::WorkspaceSwitcher
                | OverlayKind::ThemeSelector
        );
        let mut ctx = KeyContext::default();
        ctx.add(CONTEXT_OVERLAY);
        if picker {
            ctx.add(CONTEXT_PICKER);
        }
        div()
            .id("overlay-focus")
            .absolute()
            .size_0()
            .track_focus(&self.overlay_focus)
            .key_context(ctx)
            .on_key_down(cx.listener(|this, ev, w, cx| this.on_overlay_key(ev, w, cx)))
            .into_any_element()
    }

    // ------------------------------------------------------------------ toasts

    fn toast_stack(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .flex()
            .flex_col()
            .items_end()
            .gap_2()
            .children(
                self.toasts
                    .iter()
                    .map(|toast| Toast::new(toast.kind, toast.text.clone())),
            )
    }
}

trait DismissExt {
    fn listener_for_dismiss(&mut self) -> Box<dyn Fn(&mut Window, &mut gpui::App)>;
}

impl DismissExt for Context<'_, AppWindow> {
    fn listener_for_dismiss(&mut self) -> Box<dyn Fn(&mut Window, &mut gpui::App)> {
        let entity = self.entity();
        Box::new(move |window, app| {
            entity.update(app, |this, cx| this.dismiss_top(window, cx));
        })
    }
}

fn resize_hud(t: &Theme, cols: usize, rows: usize) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .px_3()
                .py_1p5()
                .rounded_full()
                .border_1()
                .border_color(t.border)
                .bg(t.elevated_surface)
                .shadow_lg()
                .text_color(t.text)
                .font_family(TERM_FONT)
                .text_size(px(14.))
                .child(format!("{cols}×{rows}")),
        )
}

/// One styled terminal row. `cursor` draws a block at that column; `hits` are
/// `(col, len, is_current)` search highlights.
fn styled_line(
    line: &Line,
    t: &Theme,
    hits: &[(usize, usize, bool)],
    cursor: Option<usize>,
) -> StyledText {
    #[derive(Clone, Copy, PartialEq)]
    struct Cell {
        style: SpanStyle,
        bg: Option<Hsla>,
        fg: Option<Hsla>,
    }
    let mut cells: Vec<(char, Cell)> = line
        .spans
        .iter()
        .flat_map(|s| {
            s.text.chars().map(move |c| {
                (
                    c,
                    Cell {
                        style: s.style,
                        bg: None,
                        fg: None,
                    },
                )
            })
        })
        .collect();
    if let Some(col) = cursor {
        while cells.len() <= col {
            cells.push((
                ' ',
                Cell {
                    style: SpanStyle::PLAIN,
                    bg: None,
                    fg: None,
                },
            ));
        }
    }
    for &(col, len, current) in hits {
        for (_, cell) in cells.iter_mut().skip(col).take(len) {
            cell.bg = Some(if current {
                t.accent.opacity(0.6)
            } else {
                t.warning.opacity(0.35)
            });
        }
    }
    if let Some(col) = cursor {
        let cell = &mut cells[col].1;
        cell.bg = Some(t.term.fg);
        cell.fg = Some(t.background);
    }

    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut prev: Option<Cell> = None;
    for (ch, cell) in &cells {
        text.push(*ch);
        if prev == Some(*cell) {
            runs.last_mut().expect("run exists").len += ch.len_utf8();
            continue;
        }
        prev = Some(*cell);
        let color = cell.fg.unwrap_or_else(|| cell.style.color.resolve(&t.term));
        let mut f = font(TERM_FONT);
        if cell.style.bold {
            f.weight = FontWeight::BOLD;
        }
        if cell.style.italic {
            f.style = FontStyle::Italic;
        }
        runs.push(TextRun {
            len: ch.len_utf8(),
            font: f,
            color,
            background_color: cell.bg,
            underline: match cell.style.underline {
                Underline::None => None,
                Underline::Single => Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                Underline::Curly => Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: true,
                }),
            },
            strikethrough: cell.style.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        });
    }
    if text.is_empty() {
        // An empty row still needs one run so the element has a baseline.
        return StyledText::new(SharedString::from(" "));
    }
    StyledText::new(text).with_runs(runs)
}

#[allow(dead_code)]
const _: &str = QUICK_SELECT_ALPHABET;
