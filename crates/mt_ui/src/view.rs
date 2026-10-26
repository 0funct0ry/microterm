//! Rendering for [`AppWindow`]: title bar, tab bar, viewport and every overlay.

use crate::designer_view::Clip;
use gpui::{
    canvas, div, font, prelude::*, px, relative, AnyElement, ClickEvent, Context, Decorations,
    FontStyle, FontWeight, Hsla, KeyContext, MouseButton, SharedString, StrikethroughStyle,
    StyledText, TextRun, UnderlineStyle, Window, WindowControlArea,
};
use mt_mux::DomainState;
use mt_term::ScrollCmd;

use crate::{
    actions::*,
    app::{AppWindow, PickerData},
    assets::IconName,
    components::*,
    metrics::*,
    mock::{Line, SpanStyle, Underline},
    overlay_stack::OverlayKind,
    platform::Platform,
    terminal_view::{paint_terminal, TerminalPaint},
    theme::Theme,
};

impl Render for AppWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        let csd = matches!(window.window_decorations(), Decorations::Client { .. })
            && self.platform == Platform::Linux;
        self.sync_window_title(window);
        if std::mem::take(&mut self.refocus_viewport) && self.overlays.is_empty() {
            window.focus(&self.viewport_focus);
        }

        let mut root = div()
            .id("root")
            .key_context(CONTEXT_ROOT)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background.opacity(self.cfg.window.opacity))
            .text_color(t.text)
            .font_family(UI_FONT)
            .text_size(px(UI_TEXT_SIZE))
            .when(csd, |el| el.border_1().border_color(t.border))
            .child(self.title_bar(&t, window, cx));

        if let Some(message) = self.config_error.clone() {
            root = root.child(
                Banner::new("config-error", self.config_banner_kind, message)
                    .action(
                        "Open config",
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.open_config(window, cx)
                        }),
                    )
                    .on_dismiss(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.config_error = None;
                        cx.notify();
                    })),
            );
        }

        let tab_bar_hidden = self.cfg.tab_bar.hide_if_single && self.tabs.len() <= 1;
        let tab_bar_bottom = self.cfg.tab_bar.position == mt_config::TabBarPosition::Bottom;
        if !tab_bar_bottom && !tab_bar_hidden {
            root = root.child(self.tab_bar(cx));
        }
        let viewport = self.viewport(&t, window, cx);
        let side_panel = match self.right_panel() {
            Some(crate::right_panel::RightPanel::Snippets) => self.snippets_panel(&t, cx),
            Some(crate::right_panel::RightPanel::Recordings) => self.recordings_panel(&t, cx),
            Some(crate::right_panel::RightPanel::Triggers) => self.triggers_panel(&t, cx),
            None => None,
        };
        root = root.child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(viewport)
                .children(side_panel),
        );
        if tab_bar_bottom && !tab_bar_hidden {
            root = root.child(self.tab_bar(cx));
        }
        if let Some(strip) = self.progress_strip(&t) {
            root = root.child(strip);
        }
        if let Some(bar) = self.status_bar(&t, cx) {
            root = root.child(bar);
        }
        root = root
            .child(self.toast_stack(cx))
            .when_some(self.hud, |el, (cols, rows)| {
                el.child(resize_hud(&t, cols, rows))
            });

        if let Some(menu) = self.render_open_menu(cx) {
            root = root.child(menu);
        }
        if let Some(menu) = self.render_row_menu(cx) {
            root = root.child(menu);
        }
        if let Some(m) = self.fill_modal(&t, cx) {
            root = root.child(m);
        }
        if let Some(m) = self.export_modal(&t, cx) {
            root = root.child(m);
        }
        if let Some(menu) = self.render_recording_menu(cx) {
            root = root.child(menu);
        }
        if let Some(tip) = self.link_tooltip(&t) {
            root = root.child(tip);
        }
        if let Some(picker) = &self.picker {
            root = root.child(self.picker_modal(picker, cx));
        }
        if let Some(text) = &self.paste_confirm {
            root = root.child(self.paste_modal(&t, text, cx));
        }
        if self.close_confirm.is_some() {
            root = root.child(self.close_modal(&t, cx));
        }
        if self.about {
            root = root.child(self.about_modal(&t, cx));
        }
        if let Some(m) = self.menu_confirm_modal(&t, cx) {
            root = root.child(m);
        }
        if let Some(prompt) = &self.prompt {
            root = root.child(self.prompt_modal(&t, prompt, cx));
        }
        if self.designer.is_some() {
            if let Some(m) = self.designer_modal(&t, cx) {
                root = root.child(m);
            }
        }
        if let Some(req) = self.ssh_prompts.front() {
            root = root.child(self.ssh_prompt_modal(&t, req, cx));
        }
        if let Some(top) = self.overlays.top() {
            root = root.child(self.focus_node(top, cx));
        }
        root.on_action(cx.listener(|_, _: &Quit, _, cx| crate::recording::ipc::quit(cx)))
            .on_action(cx.listener(|this, _: &SpawnTab, w, cx| this.on_action_spawn_tab(w, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, w, cx| this.on_action_close_tab(w, cx)))
            .on_action(cx.listener(|this, _: &SplitHorizontal, w, cx| {
                this.split_focused(mt_mux::Dir::Right, w, cx)
            }))
            .on_action(cx.listener(|this, _: &SplitVertical, w, cx| {
                this.split_focused(mt_mux::Dir::Down, w, cx)
            }))
            .on_action(cx.listener(|this, a: &Split, w, cx| this.split_focused(a.0, w, cx)))
            .on_action(
                cx.listener(|this, a: &ShowContextMenu, _, cx| this.show_context_menu(&a.0, cx)),
            )
            .on_action(
                cx.listener(|this, a: &InvokeMenuItem, w, cx| {
                    this.invoke_menu_address(&a.0, w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ClosePane, w, cx| this.request_close_focused_pane(w, cx)),
            )
            .on_action(cx.listener(|this, _: &TogglePaneZoom, w, cx| this.toggle_zoom(w, cx)))
            .on_action(cx.listener(|this, _: &RotatePanes, w, cx| this.rotate_panes(w, cx)))
            .on_action(cx.listener(|this, a: &ActivatePaneDirection, w, cx| {
                this.focus_direction(a.0, w, cx)
            }))
            .on_action(cx.listener(|this, a: &SwapPane, w, cx| this.swap_focused(a.0, w, cx)))
            .on_action(
                cx.listener(|this, a: &ResizePane, w, cx| this.resize_focused(a.0, a.1, w, cx)),
            )
            .on_action(
                cx.listener(|this, a: &MoveTabRelative, w, cx| this.move_tab_relative(a.0, w, cx)),
            )
            .on_action(cx.listener(|this, _: &RenameTab, _, cx| this.begin_rename(cx)))
            .on_action(cx.listener(|this, _: &CloseWindow, w, cx| this.request_close_window(w, cx)))
            .on_action(cx.listener(|this, a: &SwitchWorkspace, _, cx| {
                this.switch_workspace_named(a.0.clone(), false, cx)
            }))
            .on_action(cx.listener(|this, _: &CreateWorkspace, w, cx| {
                this.begin_prompt(
                    crate::app::PromptKind::CreateWorkspace,
                    String::new(),
                    w,
                    cx,
                )
            }))
            .on_action(
                cx.listener(|this, _: &RenameWorkspace, w, cx| this.begin_rename_workspace(w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DesignWorkspace, w, cx| {
                    this.open_designer(None, None, w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &SaveLayoutAsWorkspace, w, cx| {
                this.save_layout_as_workspace(w, cx)
            }))
            .on_action(cx.listener(|this, a: &SpawnTabInDomain, w, cx| {
                if a.0 == "local" {
                    this.on_action_spawn_tab(w, cx)
                } else {
                    this.spawn_tab_in_domain(&a.0, w, cx)
                }
            }))
            .on_action(cx.listener(|this, a: &ActivateTab, _, cx| this.activate_tab(a.0, cx)))
            .on_action(cx.listener(|this, a: &ActivateTabRelative, _, cx| {
                this.on_action_tab_relative(a.0, cx)
            }))
            .on_action(cx.listener(|this, a: &SwitchWorkspaceRelative, _, cx| {
                this.on_action_workspace_relative(a.0, cx)
            }))
            .on_action(cx.listener(|this, _: &SpawnWindow, _, cx| this.on_action_spawn_window(cx)))
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if this.text_clipboard(Clip::Copy, cx) {
                    return;
                }
                if !this.copy_selection(cx) {
                    this.toast(ToastKind::Info, "Nothing selected", cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if !this.text_clipboard(Clip::SelectAll, cx) {
                    this.select_all(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SearchSelection, w, cx| this.search_selection(w, cx)))
            .on_action(cx.listener(|this, _: &Paste, w, cx| {
                if !this.text_clipboard(Clip::Paste, cx) {
                    this.paste(w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PastePrimary, w, cx| {
                if !this.text_clipboard(Clip::Paste, cx) {
                    this.paste_primary(w, cx)
                }
            }))
            .on_action(cx.listener(|this, a: &SendString, _, cx| this.send_string(&a.0, cx)))
            .on_action(cx.listener(|this, a: &SpawnCommand, w, cx| {
                let spec =
                    crate::app::spawn_spec_for(&a.command, a.cwd.as_deref(), a.title.clone());
                this.spawn_tab(Some(spec), w, cx)
            }))
            .on_action(cx.listener(|this, a: &ConnectSsh, w, cx| {
                if a.0.is_empty() {
                    this.begin_prompt(crate::app::PromptKind::ConnectHost, String::new(), w, cx)
                } else {
                    this.spawn_tab_in_domain(&a.0, w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ConnectSerial, w, cx| {
                this.open_overlay(OverlayKind::SerialPicker, w, cx)
            }))
            .on_action(cx.listener(|this, _: &SerialSendBreak, _, cx| {
                this.serial_op(crate::app::SerialOp::Break, cx)
            }))
            .on_action(cx.listener(|this, _: &SerialToggleDtr, _, cx| {
                this.serial_op(crate::app::SerialOp::Dtr, cx)
            }))
            .on_action(cx.listener(|this, _: &SerialToggleRts, _, cx| {
                this.serial_op(crate::app::SerialOp::Rts, cx)
            }))
            // Needs OSC 133 prompt marks (M8); a no-op until a pane has them.
            .on_action(cx.listener(|_, _: &ScrollToPrompt, _, _| {}))
            .on_action(cx.listener(|this, _: &ClearScrollback, _, cx| this.clear_scrollback(cx)))
            .on_action(cx.listener(|this, _: &ResetTerminal, _, cx| this.reset_terminal(cx)))
            .on_action(cx.listener(|this, _: &ShowAbout, w, cx| this.show_about(w, cx)))
            .on_action(cx.listener(|this, _: &OpenLinkAtMouse, _, cx| this.open_link_at_mouse(cx)))
            .on_action(cx.listener(|this, _: &DetachWorkspace, _, cx| this.detach_workspace(cx)))
            .on_action(cx.listener(|this, a: &AttachWorkspace, _, cx| {
                this.attach_workspace(a.0.clone(), cx)
            }))
            .on_action(cx.listener(|_, _: &Minimize, w, _| w.minimize_window()))
            .on_action(cx.listener(|_, _: &ZoomWindow, w, _| w.zoom_window()))
            .on_action(cx.listener(|this, _: &OpenDocs, _, cx| {
                this.open_help(crate::menu_bar::help_urls().0, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenReleaseNotes, _, cx| {
                this.open_help(crate::menu_bar::help_urls().1, cx)
            }))
            .on_action(cx.listener(|this, _: &ReportIssue, _, cx| {
                this.open_help(crate::menu_bar::help_urls().2, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowConfigFolder, _, cx| this.show_config_folder(cx)))
            .on_action(cx.listener(|this, a: &ScrollByPage, _, cx| {
                let cmd = if a.0 < 0 {
                    ScrollCmd::PageUp
                } else {
                    ScrollCmd::PageDown
                };
                for _ in 0..a.0.unsigned_abs().max(1) {
                    this.scroll_terminal(cmd, cx);
                }
            }))
            .on_action(cx.listener(|this, a: &ScrollByLine, _, cx| {
                // Negative scrolls towards older output, like the keyboard direction.
                this.scroll_terminal(ScrollCmd::Lines(-(a.0 as i32)), cx)
            }))
            .on_action(
                cx.listener(|this, _: &ScrollToTop, _, cx| {
                    this.scroll_terminal(ScrollCmd::Top, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ScrollToBottom, _, cx| {
                this.scroll_terminal(ScrollCmd::Bottom, cx)
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, window, cx| {
                this.change_font_size(Some(crate::typography::FONT_SIZE_STEP), window, cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, window, cx| {
                this.change_font_size(Some(-crate::typography::FONT_SIZE_STEP), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, window, cx| {
                this.change_font_size(None, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ReloadConfig, window, cx| this.reload_config(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenConfig, window, cx| this.open_config(window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleSnippetsPanel, w, cx| {
                    this.toggle_snippets_panel(w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &FocusSnippetsPanel, w, cx| this.focus_snippets_panel(w, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleRecording, w, cx| this.toggle_recording(w, cx)))
            .on_action(cx.listener(|this, _: &StartRecording, w, cx| this.start_recording(w, cx)))
            .on_action(cx.listener(|this, _: &StopRecording, w, cx| this.stop_recording(w, cx)))
            .on_action(cx.listener(|this, _: &RecordingMarker, w, cx| this.recording_marker(w, cx)))
            .on_action(cx.listener(|this, _: &ToggleRecordingsPanel, w, cx| {
                this.toggle_recordings_panel(w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusRecordingsPanel, w, cx| {
                this.focus_recordings_panel(w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ExportRecording, w, cx| this.export_recording(None, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleTriggersPanel, w, cx| {
                    this.toggle_triggers_panel(w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &FocusTriggersPanel, w, cx| this.focus_triggers_panel(w, cx)),
            )
            .on_action(cx.listener(|this, _: &NewTrigger, w, cx| this.new_trigger(w, cx)))
            .on_action(cx.listener(|this, _: &CreateTriggerFromSelection, w, cx| {
                this.create_trigger_from_selection(w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenTriggersFile, w, cx| this.open_triggers_file(w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleTriggersPaused, _, cx| {
                    this.toggle_triggers_paused(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &TogglePrivateOsc, _, cx| this.toggle_private_osc(cx)))
            .on_action(cx.listener(|this, _: &ToggleFoldAtCursor, _, cx| {
                this.fold_action(crate::status_bar::FoldOp::ToggleAtCursor, cx)
            }))
            .on_action(cx.listener(|this, _: &FoldAll, _, cx| {
                this.fold_action(crate::status_bar::FoldOp::All, cx)
            }))
            .on_action(cx.listener(|this, _: &UnfoldAll, _, cx| {
                this.fold_action(crate::status_bar::FoldOp::UnfoldAll, cx)
            }))
            .on_action(cx.listener(|this, _: &NextMarker, _, cx| this.step_marker(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousMarker, _, cx| this.step_marker(false, cx)))
            .on_action(cx.listener(|this, _: &ClearMarkers, _, cx| this.clear_trigger_markers(cx)))
            .on_action(cx.listener(|this, _: &NewSnippet, w, cx| this.new_snippet(w, cx)))
            .on_action(cx.listener(|this, _: &SaveSelectionAsSnippet, w, cx| {
                this.save_selection_as_snippet(w, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveLastCommandAsSnippet, w, cx| {
                this.save_last_command_as_snippet(w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenSnippetsFile, w, cx| this.open_snippets_file(w, cx)),
            )
            .on_action(cx.listener(|this, a: &InsertSnippet, w, cx| {
                let g = crate::snippets::panel_state::Gesture {
                    run: a.run,
                    broadcast: false,
                };
                this.insert_snippet(&a.id, g, w, cx)
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
            .on_action(cx.listener(|this, a: &SetTabColor, _, cx| {
                let palette = this.cfg.tab_bar.palette.clone();
                match mt_mux::TabColor::parse_with_palette(&a.0, &palette) {
                    Ok(c) => {
                        if let Some(id) = this.tabs.get(this.active_tab).map(|t| t.id) {
                            this.set_tab_color(id, Some(c), cx);
                        }
                    }
                    Err(e) => this.toast(ToastKind::Warning, e.to_string(), cx),
                }
            }))
            .on_action(cx.listener(|this, _: &ClearTabColor, _, cx| {
                if let Some(id) = this.tabs.get(this.active_tab).map(|t| t.id) {
                    this.set_tab_color(id, None, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SetTabColorInteractive, w, cx| {
                this.toggle(OverlayKind::TabColorPicker, w, cx)
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
            .on_action(cx.listener(|this, _: &PickerConfirmRun, w, cx| {
                this.snippets.alt_confirm = true;
                this.on_picker_confirm(false, w, cx)
            }))
    }
}

/// The badge is hidden in panes narrower than this (SPEC §6.4.8).
const BADGE_MIN_COLS: usize = 24;
/// Opacity of the pill behind the badge text.
const BADGE_PILL_ALPHA: f32 = 0.9;

/// Font size of the badge: a little larger than the terminal font, and its pill never exceeds
/// 22 % of the pane height.
pub(crate) fn badge_font_size(cell_font: f32, pane_height: f32) -> f32 {
    (cell_font * 1.3).min(pane_height * 0.22 / 2.0).max(1.0)
}

impl AppWindow {
    // ------------------------------------------------------------------ title bar

    fn title_bar(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (dot, domain) = match self.ssh_card(cx) {
            None => (StatusKind::Connected, "local".to_string()),
            Some(c) => match c.state {
                DomainState::Connected => (StatusKind::Connected, c.name),
                DomainState::Connecting => {
                    (StatusKind::Reconnecting, format!("{} · connecting", c.name))
                }
                DomainState::Reconnecting { .. } => (
                    StatusKind::Reconnecting,
                    format!("{} · reconnecting", c.name),
                ),
                DomainState::Disconnected { .. } => {
                    (StatusKind::Error, format!("{} · disconnected", c.name))
                }
            },
        };
        let title = self.active_title();
        let workspace = self.active_workspace_name(cx);
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
            // macOS: the transparent title bar is ours, so honour the system double-click
            // preference (zoom or minimize) like native apps.
            .when(!csd && self.platform == Platform::Mac, |el| {
                el.on_mouse_down(MouseButton::Left, |e, window, _| {
                    if e.click_count == 2 {
                        window.titlebar_double_click();
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
                    .child(title.to_string()),
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

    /// The tab's chrome plus the recording dot when any of its panes records (gate on).
    #[cfg(test)]
    pub(crate) fn tab_chrome_for_test(
        &self,
        tab: &crate::app::TabView,
        cx: &gpui::App,
    ) -> crate::tab_chrome::TabChrome {
        self.tab_chrome_for(tab, cx)
    }

    fn tab_chrome_for(
        &self,
        tab: &crate::app::TabView,
        cx: &gpui::App,
    ) -> crate::tab_chrome::TabChrome {
        let mut chrome = crate::mux_host::tab_chrome(cx, tab.pane);
        if crate::private_osc::PrivateOscGate::of_config(&self.cfg).enabled {
            let bars = tab.progress_bars();
            chrome = chrome.with_bars(&bars);
        }
        if !crate::recording::gate::RecordingGate::of_config(&self.cfg).enabled {
            return chrome;
        }
        let running = crate::mux_host::host_ref(cx).store_recorders.running();
        match running
            .into_iter()
            .find(|(p, _, _)| tab.panes.iter().any(|v| v.pane == *p))
        {
            Some((pane, _, elapsed)) => chrome.with_recording(pane, elapsed),
            None => chrome,
        }
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = TabBar::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            let id = tab.id;
            let editing = self
                .renaming
                .as_ref()
                .filter(|(t, _)| *t == id)
                .map(|(_, text)| SharedString::from(text.clone()));
            bar = bar.tab(
                Tab::new(("tab", id), tab.title.clone())
                    .tab_id(id)
                    .active(i == self.active_tab)
                    .dirty(tab.dirty)
                    .chrome(self.tab_chrome_for(tab, cx))
                    .editing(editing)
                    .draggable()
                    .on_drop_tab({
                        let entity = cx.entity();
                        move |dragged, window, app| {
                            entity.update(app, |this, cx| {
                                this.move_tab_onto(dragged, id, window, cx)
                            });
                        }
                    })
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.activate_tab(i, cx)),
                    )
                    .on_close(cx.listener(move |this, _: &ClickEvent, w, cx| {
                        this.request_close_tab(id, w, cx)
                    }))
                    .on_middle_click(
                        cx.listener(move |this, _, w, cx| this.request_close_tab(id, w, cx)),
                    )
                    .on_right_click(cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        this.open_tab_menu(id, e.position, cx);
                    })),
            );
        }
        bar.trailing(
            IconButton::new("new-tab", IconName::Plus)
                .tooltip("New tab")
                .on_click(cx.listener(|this, _, w, cx| this.on_action_spawn_tab(w, cx)))
                .on_right_click(
                    cx.listener(|this, _, w, cx| this.open_overlay(OverlayKind::Launcher, w, cx)),
                ),
        )
    }

    // ------------------------------------------------------------------ viewport

    fn viewport(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tabs.get(self.active_tab);
        let live = tab.is_some_and(|tb| tb.session().is_some());
        let layout = tab
            .map(|tb| crate::panes::visible_layout(&tb.tree, tb.zoomed))
            .unwrap_or_default();
        let screen = if live { Vec::new() } else { self.screen() };
        let copy_active = self.overlays.contains(OverlayKind::CopyMode);
        let copy_cursor = if copy_active {
            self.copy_cursor_view()
        } else {
            None
        };
        let hits = if self.overlays.contains(OverlayKind::Search) {
            self.search_marks()
        } else {
            Vec::new()
        };
        let last_row = screen.len().saturating_sub(1);
        let focused_viewport = self.overlays.is_empty();
        let cell = self.cell;

        let lines = screen.iter().enumerate().map(|(row, line)| {
            let line_hits: Vec<(usize, usize, bool)> = hits
                .iter()
                .filter(|h| h.0 == row)
                .map(|h| (h.1, h.2, h.3))
                .collect();
            let cursor = if let Some((r, c)) = copy_cursor.filter(|(r, _)| *r == row) {
                let _ = r;
                Some(c)
            } else if !copy_active && focused_viewport && row == last_row {
                Some(line.cols())
            } else {
                None
            };
            div()
                .h(px(cell.height))
                .whitespace_nowrap()
                .child(styled_line(line, t, &line_hits, cursor))
        });

        let mut view = div()
            .id("viewport")
            .relative()
            .flex_1()
            .overflow_hidden()
            .bg(t.background.opacity(self.cfg.window.opacity))
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
            .on_key_down(cx.listener(|this, ev, w, cx| this.on_viewport_key(ev, w, cx)))
            .on_key_up(cx.listener(|this, ev, _, cx| this.on_viewport_key_up(ev, cx)));

        if live {
            let tab = tab.expect("live implies a tab");
            let multi = layout.len() > 1;
            let entity = cx.entity();
            let area = self.pane_area.clone();
            let mut stage = div().absolute().inset_0().child(
                canvas(|_, _, _| (), {
                    let entity = entity.clone();
                    move |bounds, _, window, _| {
                        area.set((
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.y),
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                        ));
                        // A divider drag keeps tracking the pointer outside the grid.
                        let e = entity.clone();
                        window.on_mouse_event(
                            move |ev: &gpui::MouseMoveEvent, phase, window, cx| {
                                if phase == gpui::DispatchPhase::Bubble {
                                    e.update(cx, |this, cx| {
                                        if this.divider_drag.is_some() {
                                            this.drag_divider_to(
                                                (
                                                    f32::from(ev.position.x),
                                                    f32::from(ev.position.y),
                                                ),
                                                window,
                                                cx,
                                            );
                                        }
                                    });
                                }
                            },
                        );
                        let e = entity.clone();
                        window.on_mouse_event(move |_: &gpui::MouseUpEvent, phase, _, cx| {
                            if phase == gpui::DispatchPhase::Bubble {
                                e.update(cx, |this, cx| this.end_divider_drag(cx));
                            }
                        });
                    }
                })
                .absolute()
                .size_full(),
            );
            for (pane, rect) in &layout {
                stage = stage.child(self.pane_element(
                    *pane,
                    *rect,
                    multi,
                    t,
                    window,
                    &hits,
                    copy_cursor,
                    cx,
                ));
            }
            if multi && tab.zoomed.is_none() {
                for split in tab.tree.splits(mt_mux::Rect::new(0.0, 0.0, 1.0, 1.0)) {
                    stage = stage.child(self.divider(split, t, cx));
                }
            }
            view = view.child(stage);
            if self.bell_flash {
                view = view.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(t.text.opacity(0.10))
                        .into_any_element(),
                );
            }
        } else {
            view = view.child(
                div()
                    .px(px(self.cfg.window.padding.x))
                    .py(px(self.cfg.window.padding.y))
                    .font_family(self.font.family.clone())
                    .text_size(px(cell.font_size))
                    .line_height(px(cell.height))
                    .text_color(t.term.fg)
                    .children(lines),
            );
        }

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

        view = view.child(self.mode_strip(t, cx));
        if let Some(card) = self
            .ssh_card(cx)
            .filter(|c| c.state != DomainState::Connected)
        {
            view = view.child(self.ssh_overlay(t, &card, cx));
        }
        view
    }

    /// One pane of the active tab, placed by its fractional `rect` in the pane area. Overlays
    /// that belong to "the pane" (search marks, quick-select chips, the scrollbar) are children
    /// of the focused one, so they anchor to its origin.
    #[allow(clippy::too_many_arguments)]
    fn pane_element(
        &self,
        pane: u64,
        rect: mt_mux::Rect,
        multi: bool,
        t: &Theme,
        window: &Window,
        hits: &[crate::mode_overlays::ViewMark],
        copy_cursor: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tab = &self.tabs[self.active_tab];
        let is_focus = tab.pane == pane;
        let mut el = div()
            .id(("pane", pane))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.w))
            .h(relative(rect.h))
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, w, cx| {
                    if this.overlays.is_empty() {
                        if this.focused_pane() != Some(pane) {
                            this.focus_pane(pane, w, cx);
                        }
                        // A click in the terminal gives it the keyboard back from a side panel
                        // (otherwise Ctrl+V and typing keep going to the panel).
                        if this.release_panel_focus() {
                            this.sync_focus(w);
                            cx.notify();
                        }
                    }
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, ev, _, cx| this.on_scroll_wheel_pane(pane, ev, cx)),
            );
        let Some(session) = tab.pane_view(pane).and_then(|v| v.session.as_ref()) else {
            return el;
        };
        let cell = self.cell;
        let pointer = self.pointer_style();
        let deco = crate::triggers::paint::for_pane(cx, pane, &session.snapshot, t);
        let ticks = deco.ticks.clone();
        let paint = TerminalPaint {
            deco,
            snapshot: session.snapshot.clone(),
            theme: *t,
            metrics: cell,
            focused: is_focus
                && self.viewport_focus.is_focused(window)
                && window.is_window_active(),
            cursor_on: self.cursor_phase,
            cache: session.cache.clone(),
            // The size is part of the key: cached lines were shaped at one size.
            font_generation: self.font_generation ^ (u64::from(cell.font_size.to_bits()) << 32),
            font: self.font.clone(),
            hover_link: is_focus
                .then(|| self.mouse.hover.as_ref().map(|l| (l.row, l.start, l.end)))
                .flatten(),
        };
        let (term, size, origin) = (
            session.term.clone(),
            session.size.clone(),
            session.grid_origin.clone(),
        );
        let entity = cx.entity();
        let armed = self.hud_armed.clone();
        el = el.child(
            div()
                .size_full()
                .px(px(self.cfg.window.padding.x))
                .py(px(self.cfg.window.padding.y))
                .child(
                    canvas(
                        move |bounds, window, _| {
                            window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal)
                        },
                        move |bounds, hitbox, window, cx| {
                            origin.set((f32::from(bounds.origin.x), f32::from(bounds.origin.y)));
                            if is_focus {
                                window.set_cursor_style(pointer, &hitbox);
                                Self::register_mouse(&entity, hitbox.clone(), window);
                            }
                            if !is_focus {
                                // Right-click on another pane opens its menu without moving
                                // focus (SPEC §10.9.1); the focused pane does it in `mouse_down`.
                                let (e, hb) = (entity.clone(), hitbox.clone());
                                window.on_mouse_event(
                                    move |ev: &gpui::MouseDownEvent, phase, window, cx| {
                                        if phase == gpui::DispatchPhase::Bubble
                                            && ev.button == MouseButton::Right
                                            && hb.is_hovered(window)
                                        {
                                            e.update(cx, |this, cx| {
                                                let reporting = this
                                                    .tabs
                                                    .get(this.active_tab)
                                                    .and_then(|t| t.pane_view(pane))
                                                    .and_then(|v| v.session.as_ref())
                                                    .is_some_and(|s| {
                                                        s.snapshot.modes.mouse_reporting
                                                    });
                                                if !reporting || ev.modifiers.shift {
                                                    this.pane_right_click(
                                                        pane,
                                                        ev.position,
                                                        &ev.modifiers,
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            });
                                        }
                                    },
                                );
                            }
                            if multi {
                                let (e, hb) = (entity.clone(), hitbox);
                                window.on_mouse_event(
                                    move |_: &gpui::MouseMoveEvent, phase, window, cx| {
                                        if phase == gpui::DispatchPhase::Bubble
                                            && hb.is_hovered(window)
                                        {
                                            e.update(cx, |this, cx| {
                                                this.hover_pane(pane, window, cx)
                                            });
                                        }
                                    },
                                );
                            }
                            let (cols, rows) = paint.metrics.grid_for(
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            );
                            if size.get() != (cols, rows) {
                                size.set((cols, rows));
                                term.resize(
                                    cols,
                                    rows,
                                    paint.metrics.width as u16,
                                    paint.metrics.height as u16,
                                );
                                // The first layout is not a user resize; only the focused pane
                                // reports its size.
                                if armed.replace(true) && is_focus {
                                    let entity = entity.clone();
                                    window.defer(cx, move |_, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.show_hud(cols as usize, rows as usize, cx)
                                        });
                                    });
                                }
                            }
                            paint_terminal(bounds, &paint, window, cx);
                        },
                    )
                    .size_full(),
                ),
        );
        if let Some(badge) = session.term.badge() {
            el = el.child(Self::badge_overlay(
                &badge,
                t,
                &cell,
                session.snapshot.cols,
                session.snapshot.rows,
            ));
        }
        if is_focus {
            el = el.child(self.live_marks(t, hits, copy_cursor));
            if self.overlays.contains(OverlayKind::QuickSelect) {
                el = el.child(self.quick_select_chips(t));
            }
            if !ticks.is_empty() {
                el = el.child(Self::marker_ticks(&ticks));
            }
            let show_scrollbar = match self.cfg.scrollbar.mode {
                mt_config::ScrollbarMode::Always => true,
                mt_config::ScrollbarMode::Never => false,
                mt_config::ScrollbarMode::Auto => self.scrollbar_visible,
            };
            if show_scrollbar {
                el = el.child(self.scrollbar(t));
            }
            if multi {
                el = el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_1()
                        .border_color(t.accent.opacity(0.45)),
                );
            }
        } else if multi {
            el = el.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(t.background.opacity(self.cfg.panes.inactive_dim)),
            );
        }
        el
    }

    /// The `7718` badge (SPEC §6.4.8): a pill in the bottom-right, a plain element
    /// without hitbox or handlers so mouse events fall through to the terminal, and never part of
    /// the grid, so selection, copy, `get-text` and exports cannot see it. Hidden below 24 columns.
    fn badge_overlay(
        badge: &mt_term::private_osc::Badge,
        t: &Theme,
        cell: &crate::terminal_view::CellMetrics,
        cols: usize,
        rows: usize,
    ) -> gpui::Div {
        use mt_term::private_osc::BadgeToken;
        let hidden = cols < BADGE_MIN_COLS;
        let color = match badge.token {
            BadgeToken::Muted => t.text_muted,
            BadgeToken::Accent => t.accent,
            BadgeToken::Success => t.success,
            BadgeToken::Warning => t.warning,
            BadgeToken::Error => t.error,
        };
        let pane_h = rows as f32 * cell.height;
        let size = badge_font_size(cell.font_size, pane_h);
        let el = div().absolute().inset_0().flex().items_end().justify_end();
        if hidden {
            return el;
        }
        // A pill with a dot in the token color, like the recording indicator.
        el.child(
            div().pr(px(cell.width * 2.0)).pb(px(cell.height)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(size * 0.55))
                    .px(px(size * 0.9))
                    .py(px(size * 0.4))
                    .rounded_full()
                    .bg(t.elevated_surface.opacity(BADGE_PILL_ALPHA))
                    .text_size(px(size))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(t.text)
                    .child(div().size(px(size * 0.75)).rounded_full().bg(color))
                    .child(badge.text.clone()),
            ),
        )
    }

    /// The draggable line between the two halves of `split`: 1 px visible, 6 px to grab.
    fn divider(
        &self,
        split: mt_mux::SplitRect,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use mt_mux::Axis;
        let r = split.rect;
        let id: SharedString = format!(
            "divider-{}",
            split
                .path
                .iter()
                .map(|b| if *b { '1' } else { '0' })
                .collect::<String>()
        )
        .into();
        let hit = px(DIVIDER_HIT_AREA);
        let line = px(DIVIDER_WIDTH);
        let (axis, path, rect) = (split.axis, split.path, split.rect);
        let base = div().id(id).absolute();
        let el = match split.axis {
            Axis::Horizontal => base
                .left(relative(r.x + r.w * split.ratio))
                .top(relative(r.y))
                .h(relative(r.h))
                .w(hit)
                .ml(-(hit / 2.0))
                .cursor_col_resize()
                .child(
                    div()
                        .absolute()
                        .left((hit - line) / 2.0)
                        .w(line)
                        .h_full()
                        .bg(t.border),
                ),
            Axis::Vertical => base
                .top(relative(r.y + r.h * split.ratio))
                .left(relative(r.x))
                .w(relative(r.w))
                .h(hit)
                .mt(-(hit / 2.0))
                .cursor_row_resize()
                .child(
                    div()
                        .absolute()
                        .top((hit - line) / 2.0)
                        .h(line)
                        .w_full()
                        .bg(t.border),
                ),
        };
        el.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.begin_divider_drag(path.clone(), axis, rect);
                cx.stop_propagation();
                cx.notify();
            }),
        )
    }

    /// One 2 px tick per trigger marker along the scrollbar track (SPEC §10.11.4).
    fn marker_ticks(ticks: &[(f32, Hsla)]) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(3.))
            .w(px(6.))
            .children(
                ticks
                    .iter()
                    .map(|(f, c)| div().absolute().top(relative(*f)).h(px(2.)).w_full().bg(*c)),
            )
    }

    /// Overlay scrollbar: thumb size and position follow the scrollback position.
    fn scrollbar(&self, t: &Theme) -> impl IntoElement {
        let (thumb, top) = match self.live() {
            Some(s) if s.snapshot.history_size > 0 => {
                let snap = &s.snapshot;
                let total = (snap.rows + snap.history_size) as f32;
                let thumb = (snap.rows as f32 / total).max(0.04);
                let top =
                    ((snap.history_size - snap.display_offset) as f32 / total).min(1.0 - thumb);
                (thumb, top)
            }
            _ => (0.3, 0.0),
        };
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(3.))
            .w(px(6.))
            .child(
                div()
                    .absolute()
                    .top(relative(top))
                    .h(relative(thumb))
                    .w_full()
                    .rounded_full()
                    .bg(t.text_muted.opacity(0.45)),
            )
    }

    /// Search hits and the copy-mode cursor, drawn over a live terminal using cell metrics.
    fn live_marks(
        &self,
        t: &Theme,
        hits: &[crate::mode_overlays::ViewMark],
        copy_cursor: Option<(usize, usize)>,
    ) -> impl IntoElement {
        let c = self.cell;
        let (pad_x, pad_y) = (self.cfg.window.padding.x, self.cfg.window.padding.y);
        let mark = move |row: usize, col: usize, len: usize, color: Hsla| {
            div()
                .absolute()
                .left(px(pad_x + col as f32 * c.width))
                .top(px(pad_y + row as f32 * c.height))
                .w(px(len as f32 * c.width))
                .h(px(c.height))
                .bg(color)
        };
        div()
            .absolute()
            .inset_0()
            .children(hits.iter().map(|&(row, col, len, current)| {
                let color = if current {
                    t.accent.opacity(0.6)
                } else {
                    t.warning.opacity(0.35)
                };
                mark(row, col, len, color)
            }))
            .when_some(copy_cursor, |el, (row, col)| {
                el.child(mark(row, col, 1, t.term.fg.opacity(0.7)))
            })
    }

    /// About microterm: the logo, version, licence and where the third-party notices are.
    fn about_modal(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (docs, _, _) = crate::menu_bar::help_urls();
        let notices = crate::app_icon::notices_url();
        Modal::new("about-modal")
            .width(px(420.))
            .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
            .on_dismiss(cx.listener_for_dismiss())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .p_4()
                    .child(
                        gpui::img(crate::app_icon::logo_image())
                            .size(px(72.))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(16.))
                            .child("microterm"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child("Licence: MIT. Built with Rust and GPUI."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("about-docs", "Docs")
                                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(docs))),
                            )
                            .child(
                                Button::new("about-notices", "Third-party notices").on_click(
                                    cx.listener(move |_, _, _, cx| cx.open_url(&notices)),
                                ),
                            )
                            .child(
                                Button::new("about-close", "Close")
                                    .style(ButtonStyle::Filled)
                                    .on_click(
                                        cx.listener(|this, _, w, cx| this.dismiss_top(w, cx)),
                                    ),
                            ),
                    ),
            )
    }

    fn close_modal(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.close_confirm_text();
        Modal::new("close-modal")
            .width(px(420.))
            .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
            .on_dismiss(cx.listener_for_dismiss())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Close?"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child(text),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("close-cancel", "Cancel").on_click(
                                    cx.listener(|this, _, w, cx| this.dismiss_top(w, cx)),
                                ),
                            )
                            .child(
                                Button::new("close-confirm", "Close")
                                    .style(ButtonStyle::Filled)
                                    .on_click(
                                        cx.listener(|this, _, w, cx| this.confirm_close(w, cx)),
                                    ),
                            ),
                    ),
            )
    }

    fn prompt_modal(
        &self,
        t: &Theme,
        prompt: &crate::app::NamePrompt,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = match prompt.kind {
            crate::app::PromptKind::ConnectHost => "Connect to host (user@host:port)",
            crate::app::PromptKind::ConnectSerial => "Connect to serial port (PORT [BAUD])",
            crate::app::PromptKind::CreateWorkspace => "Create workspace",
            crate::app::PromptKind::RenameWorkspace(_) => "Rename workspace",
            crate::app::PromptKind::MoveSnippet => "Move snippet to folder (empty = ungrouped)",
        };
        Modal::new("name-prompt")
            .width(px(380.))
            .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
            .on_dismiss(cx.listener_for_dismiss())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(30.))
                            .px_2()
                            .rounded(px(RADIUS_CONTROL))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.background)
                            .child(prompt.text.clone())
                            .child(div().w(px(1.)).h(px(16.)).ml(px(1.)).bg(t.accent)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child("Enter to confirm, Esc to cancel."),
                    ),
            )
    }

    fn paste_modal(&self, t: &Theme, text: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = text.lines().count();
        let preview: String = text.lines().take(4).collect::<Vec<_>>().join("\n");
        let more = lines.saturating_sub(4);
        Modal::new("paste-modal")
            .width(px(440.))
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
                            .child(format!("Paste {lines} lines into the terminal?")),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child("This program has not enabled bracketed paste, so each newline runs as Enter."),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded(px(RADIUS_CONTROL))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.background)
                            .font_family(TERM_FONT)
                            .text_size(px(12.))
                            .overflow_hidden()
                            .child(preview)
                            .when(more > 0, |el| {
                                el.child(
                                    div()
                                        .text_color(t.text_placeholder)
                                        .child(format!("… {more} more lines")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .justify_end()
                            .child(Button::new("paste-cancel", "Cancel").on_click(
                                cx.listener(|this, _, w, cx| this.dismiss_top(w, cx)),
                            ))
                            .child(
                                Button::new("paste-confirm", "Paste")
                                    .style(ButtonStyle::Filled)
                                    .on_click(cx.listener(|this, _, w, cx| this.confirm_paste(w, cx))),
                            ),
                    ),
            )
    }

    fn quick_select_chips(&self, t: &Theme) -> impl IntoElement {
        let typed = self.quick.typed.clone();
        let (top, rows) = self.view_geometry();
        let cell = self.cell;
        let chips = self
            .quick
            .hits
            .iter()
            .filter(|h| h.label.starts_with(&typed))
            .flat_map(|hit| {
                let rest = hit.label[typed.len()..].to_string();
                hit.segments
                    .iter()
                    .enumerate()
                    .filter(|(_, (abs, _, _))| *abs >= top && *abs < top + rows)
                    .map(|(i, &(abs, col, len))| {
                        // The label chip sits on the first segment only.
                        let label = (i == 0).then(|| rest.clone());
                        (abs - top, col, len, label)
                    })
                    .collect::<Vec<_>>()
            })
            .map(|(row, col, len, label)| {
                div()
                    .absolute()
                    .left(px(col as f32 * cell.width))
                    .top(px(row as f32 * cell.height))
                    .h(px(cell.height))
                    .w(px(len as f32 * cell.width))
                    .rounded(px(2.))
                    .bg(t.accent.opacity(0.28))
                    .border_1()
                    .border_color(t.accent)
                    .when_some(label, |el, label| {
                        el.child(
                            div()
                                .absolute()
                                .left(px(-1.))
                                .top(px(-1.))
                                .px_1()
                                .h(px(cell.height))
                                .flex()
                                .items_center()
                                .rounded(px(2.))
                                .bg(t.accent)
                                .text_color(if t.is_dark {
                                    t.background
                                } else {
                                    gpui::white()
                                })
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(11.))
                                .child(label),
                        )
                    })
            });
        div()
            .absolute()
            .top(px(self.cfg.window.padding.y))
            .left(px(self.cfg.window.padding.x))
            .font_family(self.font.family.clone())
            .children(chips)
    }

    /// Mode pill and find bar, top-right of the viewport.
    fn mode_strip(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let copy = if self.overlays.contains(OverlayKind::CopyMode) {
            self.copy_pill()
        } else {
            None
        };
        let pill: Option<String> = if let Some((label, _)) = &copy {
            Some(label.clone())
        } else if self.overlays.contains(OverlayKind::QuickSelect) {
            Some("QUICK SELECT".into())
        } else if self.overlays.contains(OverlayKind::Search) {
            Some("SEARCH".into())
        } else {
            None
        };
        let prompt = copy.and_then(|(_, p)| p);
        let rec = self.recording_pill(cx).map(|info| {
            let text = format!(
                "REC{} {}",
                if info.input { " +INPUT" } else { "" },
                crate::recording::ui::format_elapsed(info.elapsed)
            );
            RecPill::new("rec-pill", text)
                .dim(self.rec_pulse && !crate::recording::ui::reduce_motion())
                .on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.stop_recording(w, cx)))
        });
        div()
            .absolute()
            .top(px(8.))
            .right(px(16.))
            .flex()
            .items_center()
            .gap_2()
            .when_some(rec, |el, pill| el.child(pill))
            .when_some(prompt, |el, text| {
                el.child(
                    div()
                        .h(px(24.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_POPOVER))
                        .border_1()
                        .border_color(t.border)
                        .bg(t.elevated_surface)
                        .text_size(px(12.))
                        .child(format!("{text}▏")),
                )
            })
            .when_some(pill, |el, text| el.child(Pill::new(text)))
            .when(self.overlays.contains(OverlayKind::Search), |el| {
                el.child(self.search_bar(t, cx))
            })
    }

    fn search_bar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.search_status();
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
            .when(!status.is_empty(), |el| {
                el.child(
                    div()
                        .px_1()
                        .text_size(px(12.))
                        .text_color(t.text_muted)
                        .whitespace_nowrap()
                        .child(status),
                )
            })
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

    fn ssh_overlay(
        &self,
        t: &Theme,
        card: &crate::app::SshCard,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (title, detail, status, dot) = match &card.state {
            DomainState::Reconnecting {
                attempt,
                next_in_secs,
            } if card.serial => (
                format!("Serial port {} disconnected", card.name),
                format!("{} — waiting for the device to come back.", card.label),
                if *next_in_secs == 0 {
                    format!("Looking for the device… (attempt {attempt})")
                } else {
                    format!("Retrying in {next_in_secs} s (attempt {attempt}).")
                },
                StatusKind::Reconnecting,
            ),
            DomainState::Disconnected { reason } if card.serial => (
                format!("Could not open {}", card.name),
                reason.clone(),
                "Not connected".to_string(),
                StatusKind::Error,
            ),
            DomainState::Connecting if card.serial => (
                format!("Opening {}", card.name),
                card.label.clone(),
                "Opening…".to_string(),
                StatusKind::Reconnecting,
            ),
            DomainState::Reconnecting {
                attempt,
                next_in_secs,
            } => (
                format!("Connection to {} lost", card.name),
                format!(
                    "{} — your session keeps running on the remote host.",
                    card.label
                ),
                if *next_in_secs == 0 {
                    format!("Reconnecting… (attempt {attempt})")
                } else {
                    format!("Reconnecting in {next_in_secs} s (attempt {attempt}).")
                },
                StatusKind::Reconnecting,
            ),
            DomainState::Disconnected { reason } => (
                format!("Could not connect to {}", card.name),
                reason.clone(),
                "Disconnected".to_string(),
                StatusKind::Error,
            ),
            _ => (
                format!("Connecting to {}", card.name),
                card.label.clone(),
                "Connecting…".to_string(),
                StatusKind::Reconnecting,
            ),
        };
        let retry = if matches!(card.state, DomainState::Disconnected { .. }) {
            "Retry"
        } else if card.serial {
            "Retry now"
        } else {
            "Reconnect now"
        };
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
                    .w(px(360.))
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
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(title)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child(detail),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(StatusDot::new(dot))
                            .child(status),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("ssh-close", "Close tab").on_click(
                                    cx.listener(|this, _, w, cx| this.close_ssh_tab(w, cx)),
                                ),
                            )
                            .child(
                                Button::new("ssh-reconnect", retry)
                                    .style(ButtonStyle::Filled)
                                    .on_click(cx.listener(|this, _, _, cx| this.reconnect_now(cx))),
                            ),
                    ),
            )
    }

    /// Host-key confirmation and password / passphrase prompts (SPEC §4.6).
    fn ssh_prompt_modal(
        &self,
        t: &Theme,
        req: &mt_ssh::prompt::PromptRequest,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use mt_ssh::prompt::{PromptKind, SecretPrompt};
        let mut body = div().flex().flex_col().gap_3().p_4();
        match &req.kind {
            PromptKind::HostKey(h) => {
                let host = if h.port == 22 {
                    h.host.clone()
                } else {
                    format!("[{}]:{}", h.host, h.port)
                };
                body = body
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Trust host {host}?")))
                    .child(div().text_size(px(12.)).text_color(t.text_muted).child(format!(
                        "The authenticity of this host (for \"{}\") cannot be established. It presents a {} key with fingerprint:",
                        req.domain, h.key_type
                    )))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(t.background)
                            .border_1()
                            .border_color(t.border)
                            .font_family("monospace")
                            .text_size(px(12.))
                            .child(h.fingerprint.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .justify_end()
                            .child(Button::new("hk-cancel", "Cancel").on_click(cx.listener(|this, _, w, cx| this.confirm_host_key(false, w, cx))))
                            .child(Button::new("hk-trust", "Trust and connect").style(ButtonStyle::Filled).on_click(cx.listener(|this, _, w, cx| this.confirm_host_key(true, w, cx)))),
                    );
            }
            PromptKind::Secret(p) => {
                let (title, echo) = match p {
                    SecretPrompt::Password { user, host } => {
                        (format!("Password for {user}@{host}"), false)
                    }
                    SecretPrompt::Passphrase { path } => (format!("Passphrase for {path}"), false),
                    SecretPrompt::Interactive { text, echo, .. } => (text.clone(), *echo),
                };
                // Typed text is shown as dots unless the server marked the prompt as echoing.
                let shown = if echo {
                    self.ssh_input.clone()
                } else {
                    "•".repeat(self.ssh_input.chars().count())
                };
                body = body
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(30.))
                            .px_2()
                            .rounded(px(RADIUS_CONTROL))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.background)
                            .child(shown)
                            .child(div().w(px(1.)).h(px(16.)).ml(px(1.)).bg(t.accent)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child("Enter to continue, Esc to cancel. It is not stored."),
                    );
            }
        }
        Modal::new("ssh-prompt")
            .width(px(440.))
            .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
            .on_dismiss(cx.listener_for_dismiss())
            .child(body)
    }

    // ------------------------------------------------------------------ pickers

    fn picker_modal(&self, picker: &PickerData, cx: &mut Context<Self>) -> impl IntoElement {
        let (offset, visible) = picker.state.visible();
        let theme = *cx.global::<crate::theme::Theme>();
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
                    swatch: entry.swatch.map(|c| match c {
                        Some(c) => crate::theme::hsla(crate::tab_color::resolve_rgba(
                            &theme.tab_ui,
                            &self.cfg.tab_bar.palette,
                            c,
                        )),
                        None => gpui::transparent_black(),
                    }),
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
                | OverlayKind::SerialPicker
                | OverlayKind::TabColorPicker
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

    fn toast_stack(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .flex()
            .flex_col()
            .items_end()
            .gap_2()
            .children(self.toasts.iter().map(|toast| {
                let mut t = Toast::new(toast.kind, toast.text.clone());
                for (i, b) in toast.buttons.iter().cloned().enumerate() {
                    let label = b.label();
                    t = t.action(
                        ("toast-button", toast.id as usize * 8 + i),
                        label,
                        cx.listener(move |this, _: &ClickEvent, w, cx| {
                            this.toast_button(b.clone(), w, cx)
                        }),
                    );
                }
                if toast.open_config {
                    t.action(
                        ("toast-open-config", toast.id as usize),
                        "Open config",
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.open_config(window, cx)
                        }),
                    )
                } else {
                    t
                }
            }))
    }
}

pub(crate) trait DismissExt {
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

#[cfg(test)]
mod badge_tests {
    use super::badge_font_size;

    #[test]
    fn the_watermark_never_exceeds_a_fifth_of_the_pane() {
        assert!((badge_font_size(14.0, 10_000.0) - 18.2).abs() < 0.01);
        let s = badge_font_size(14.0, 100.0);
        assert!(s * 2.0 <= 100.0 * 0.22 + 0.01);
        assert!(badge_font_size(14.0, 0.0) >= 1.0);
    }
}
