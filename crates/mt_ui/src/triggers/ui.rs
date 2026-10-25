//! The Triggers side panel in the window (SPEC §10.11.5): per-window state, the toggle and focus
//! state machine, the rule editor's actions (save, delete, duplicate, **Test**, live preview),
//! "Create Trigger from Selection", and the findings jumps. Rendering is in
//! [`super::panel_view`]; the editor's pure state is in [`super::editor`].

use std::time::Duration;

use gpui::{BorrowAppContext, Context, FocusHandle, KeyDownEvent, Window};
use mt_config::edit::EditError;
use mt_proto::selector::Selector;
use mt_triggers::store;
use mt_triggers::{FireCtx, Rule};

use super::editor::{Editor, Focus};
use super::gate::{TriggerGate, DISABLED_TOAST};
use super::preview;
use super::runtime::{self, TriggerRuntime};
use crate::{
    app::AppWindow,
    components::{TextEdit, ToastKind},
    recording::panel::edit_key,
    snippets::{
        panel_state::{Panel, Toggled},
        ui::PanelDrag,
    },
};

pub const MIN_WIDTH: f32 = 280.0;
pub const MAX_WIDTH: f32 = 600.0;

pub fn clamp_width(w: f32) -> f32 {
    w.clamp(MIN_WIDTH, MAX_WIDTH)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    Rules,
    Findings,
}

/// Everything the triggers feature keeps per window.
pub struct TriggersUi {
    pub panel: Panel,
    pub focus: FocusHandle,
    pub drag: Option<PanelDrag>,
    pub tab: PanelTab,
    /// The editor replaces the list while it is open.
    pub editor: Option<Editor>,
    pub preview_epoch: u64,
    /// A preview refresh is scheduled (new output only schedules one when none is).
    pub preview_pending: bool,
    /// Shaped text of the editor's text fields, measured at paint time so a click or drag
    /// becomes a caret position.
    pub field_layout: FieldLayouts,
    /// The editor field a mouse drag started in.
    pub field_drag: Option<super::editor::Focus>,
    /// A clicked finding flashes its whole line (`true`) or only the matched text.
    pub flash_whole_line: bool,
}

/// Editor field to its bounds and shaped line.
pub type FieldLayouts = std::rc::Rc<
    std::cell::RefCell<
        std::collections::HashMap<
            super::editor::Focus,
            (gpui::Bounds<gpui::Pixels>, gpui::ShapedLine),
        >,
    >,
>;

impl TriggersUi {
    pub fn new(cfg: &mt_config::Config, focus: FocusHandle) -> Self {
        let saved = if cfg!(test) {
            crate::state::UiState::default()
        } else {
            crate::state::state_dir()
                .map(|d| crate::state::load_ui(&d))
                .unwrap_or_default()
        };
        let width = saved
            .triggers_width
            .unwrap_or(cfg.triggers.panel_width as f32);
        let open = TriggerGate::of_config(cfg).enabled
            && saved
                .triggers_open
                .unwrap_or(cfg.triggers.panel_open_on_start);
        let mut panel = Panel::new(clamp_width(width), open);
        panel.width = clamp_width(width);
        Self {
            panel,
            focus,
            drag: None,
            tab: PanelTab::Rules,
            editor: None,
            preview_epoch: 0,
            preview_pending: false,
            field_layout: Default::default(),
            field_drag: None,
            flash_whole_line: true,
        }
    }

    /// Closes the panel; an open editor keeps its draft until the panel is closed for good.
    pub fn close_panel(&mut self) {
        self.panel.close();
    }
}

/// `regex::escape` of the first line of `selection`, at most 200 characters.
pub fn regex_of_selection(selection: &str) -> String {
    let line: String = selection
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    regex::escape(line.trim_end())
}

impl AppWindow {
    pub(crate) fn triggers_visible(&self) -> bool {
        self.triggers.panel.open && TriggerGate::of_config(&self.cfg).enabled
    }

    /// False (with the disabled toast and an **Open config** button) while the feature is off.
    pub(crate) fn triggers_guard(&mut self, cx: &mut Context<Self>) -> bool {
        if TriggerGate::of(cx).enabled {
            return true;
        }
        self.toast_open_config(ToastKind::Info, DISABLED_TOAST, cx);
        false
    }

    /// `ToggleTriggersPanel`: closed → open and focused; unfocused → focus; focused → close.
    pub(crate) fn toggle_triggers_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.triggers_guard(cx) {
            return;
        }
        if self.triggers.panel.toggle() == Toggled::Closed {
            self.triggers.close_panel();
        } else {
            self.snippets.panel.close();
            self.recordings.close_panel();
        }
        self.after_panel_change(window, cx);
    }

    /// `FocusTriggersPanel`: opens if needed, never closes.
    pub(crate) fn focus_triggers_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.triggers_guard(cx) {
            return;
        }
        self.triggers.panel.focus();
        self.snippets.panel.close();
        self.recordings.close_panel();
        self.after_panel_change(window, cx);
    }

    /// `NewTrigger`: the editor with a blank rule.
    pub(crate) fn new_trigger(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.triggers_guard(cx) {
            return;
        }
        self.open_rule_editor(Editor::blank(), window, cx);
    }

    /// `CreateTriggerFromSelection`: the editor with the escaped selection as the regex and a
    /// default `highlight` action (SPEC §10.11.5).
    pub(crate) fn create_trigger_from_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.triggers_guard(cx) {
            return;
        }
        let trim = self.cfg.selection.trim_trailing_whitespace;
        let text = self
            .live()
            .and_then(|s| s.term.selection_text(trim))
            .filter(|t| !t.trim().is_empty());
        let Some(text) = text else {
            self.toast(ToastKind::Info, "Select some text first", cx);
            return;
        };
        let mut ed = Editor::blank();
        ed.regex = TextEdit::new(regex_of_selection(&text));
        ed.name = TextEdit::new(
            text.lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(40)
                .collect::<String>()
                .trim()
                .to_string(),
        );
        self.open_rule_editor(ed, window, cx);
    }

    /// `OpenTriggersFile`: `triggers.toml` in `$VISUAL`/`$EDITOR`, else the system opener.
    pub(crate) fn open_triggers_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.triggers_guard(cx) {
            return;
        }
        let path = runtime::path(cx);
        if !path.exists() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&path, "version = 1\n") {
                self.toast(
                    ToastKind::Error,
                    format!("Cannot create {}: {e}", path.display()),
                    cx,
                );
                return;
            }
        }
        let Some(argv) = mt_config::editor::editor_argv_from_env(&path) else {
            cx.open_with_system(&path);
            return;
        };
        let title = std::path::Path::new(&argv[0])
            .file_name()
            .map_or_else(|| "editor".into(), |n| n.to_string_lossy().into_owned());
        let spec = mt_mux::SpawnSpec {
            cwd: self.new_tab_cwd(cx),
            command: Some(argv),
            title: Some(title),
            ..Default::default()
        };
        self.spawn_tab(Some(spec), window, cx);
    }

    /// Takes keyboard focus away from every side panel (the terminal is clicked). Returns whether
    /// any panel had it.
    pub(crate) fn release_panel_focus(&mut self) -> bool {
        let had = self.snippets.panel.focused
            || self.recordings.panel.focused
            || self.triggers.panel.focused;
        self.snippets.panel.focused = false;
        self.recordings.panel.focused = false;
        self.triggers.panel.focused = false;
        had
    }

    /// `NextMarker` / `PreviousMarker` on the focused pane (trigger and program markers). Both
    /// are palette entries; with triggers off only `7722` marks exist.
    pub(crate) fn step_marker(&mut self, forward: bool, cx: &mut Context<Self>) {
        // The jump reads other windows' terminals, so it runs once this update has finished.
        cx.defer(move |cx| {
            runtime::step_marker(cx, forward);
        });
    }

    /// `ClearMarkers` on the focused pane.
    pub(crate) fn clear_trigger_markers(&mut self, cx: &mut Context<Self>) {
        cx.defer(runtime::clear_markers);
    }

    /// `ToggleTriggersPaused`: the session-wide pause (the panel footer's button).
    pub(crate) fn toggle_triggers_paused(&mut self, cx: &mut Context<Self>) {
        if !self.triggers_guard(cx) {
            return;
        }
        let paused = cx.update_global::<TriggerRuntime, _>(|rt, _| {
            let p = !rt.engine.paused();
            rt.engine.set_paused(p);
            p
        });
        self.toast(
            ToastKind::Info,
            if paused {
                "Triggers paused"
            } else {
                "Triggers resumed"
            },
            cx,
        );
        cx.notify();
    }

    /// The gate changed: the panel, its editor draft and the preview go away with the feature.
    pub(crate) fn triggers_gate_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if TriggerGate::of_config(&self.cfg).enabled {
            return;
        }
        if self.triggers.editor.take().is_some() {
            self.toast(
                ToastKind::Info,
                "Unsaved trigger draft discarded (triggers are disabled)",
                cx,
            );
        }
        let was_open = self.triggers.panel.open;
        self.triggers.close_panel();
        if was_open {
            self.relayout(window);
        }
        self.sync_focus(window);
        cx.notify();
    }

    // ------------------------------------------------------------------ the editor

    pub(crate) fn open_rule_editor(
        &mut self,
        editor: Editor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.triggers.editor = Some(editor);
        self.triggers.tab = PanelTab::Rules;
        self.triggers.panel.focus();
        self.snippets.panel.close();
        self.recordings.close_panel();
        self.refresh_trigger_preview(cx);
        self.after_panel_change(window, cx);
    }

    /// Opens the rule `id` from the list.
    pub(crate) fn edit_trigger(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = cx
            .try_global::<TriggerRuntime>()
            .and_then(|r| r.engine.rule(id).cloned())
        else {
            return;
        };
        let text = std::fs::read_to_string(runtime::path(cx)).unwrap_or_default();
        let base = store::base_of(&text, Some(id));
        self.open_rule_editor(Editor::open(rule, Some(id.to_string()), base), window, cx);
    }

    /// Byte offset in editor field `f`'s text under window x `x`.
    fn editor_index_at(&self, f: super::editor::Focus, x: gpui::Pixels) -> usize {
        match self.triggers.field_layout.borrow().get(&f) {
            Some((bounds, line)) => line.closest_index_for_x(x - bounds.origin.x),
            None => usize::MAX,
        }
    }

    /// Mouse down in an editor text field: caret, word (double click) or all (triple click).
    pub(crate) fn editor_field_down(
        &mut self,
        f: super::editor::Focus,
        ev: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let idx = self.editor_index_at(f, ev.position.x);
        let Some(ed) = self.triggers.editor.as_mut() else {
            return;
        };
        let was = ed.focus == f;
        ed.focus = f;
        if let Some(edit) = ed.line_edit() {
            match ev.click_count {
                2 => edit.select_word_at(idx),
                n if n >= 3 => edit.select_all(),
                _ => edit.set_caret(idx, ev.modifiers.shift && was),
            }
        }
        self.triggers.field_drag = (ev.click_count == 1).then_some(f);
        cx.stop_propagation();
        cx.notify();
    }

    /// Dragging with the button held extends the selection.
    pub(crate) fn editor_field_drag(
        &mut self,
        f: super::editor::Focus,
        ev: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if self.triggers.field_drag != Some(f) || ev.pressed_button != Some(gpui::MouseButton::Left)
        {
            return;
        }
        let idx = self.editor_index_at(f, ev.position.x);
        if let Some(edit) = self.triggers.editor.as_mut().and_then(|e| e.line_edit()) {
            edit.set_caret(idx, true);
            cx.notify();
        }
    }

    pub(crate) fn close_rule_editor(&mut self, cx: &mut Context<Self>) {
        self.triggers.editor = None;
        self.triggers.preview_pending = false;
        cx.update_global::<TriggerRuntime, _>(|rt, _| rt.deco.clear_preview());
        cx.defer(crate::mux_host::refresh_windows);
        cx.notify();
    }

    /// Esc / back: asks before discarding unsaved changes.
    pub(crate) fn rule_editor_escape(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.triggers.editor.as_mut() else {
            return;
        };
        if ed.add_menu {
            ed.add_menu = false;
        } else if ed.confirm_delete {
            ed.confirm_delete = false;
        } else if ed.dirty() && !ed.confirm_discard {
            ed.confirm_discard = true;
        } else {
            return self.close_rule_editor(cx);
        }
        cx.notify();
    }

    fn persist_rule(&mut self, rule: &Rule, cx: &mut Context<Self>) -> Result<String, EditError> {
        let path = runtime::path(cx);
        let base = self
            .triggers
            .editor
            .as_ref()
            .map_or_else(|| store::base_of("", None), |e| e.base.clone());
        store::save_rule(&path, &base, rule)
    }

    /// Writes the editor's rule back to `triggers.toml` surgically (SPEC §10.11.2).
    pub(crate) fn save_rule_editor(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.triggers.editor.as_ref() else {
            return;
        };
        if let Some(why) = ed.save_blocker() {
            if let Some(ed) = self.triggers.editor.as_mut() {
                ed.error = Some(why);
            }
            cx.notify();
            return;
        }
        let Ok(mut rule) = ed.to_rule() else {
            return;
        };
        if rule.id.is_empty() {
            let used: std::collections::HashSet<String> = cx
                .try_global::<TriggerRuntime>()
                .map(|r| r.engine.rules().map(|r| r.id.clone()).collect())
                .unwrap_or_default();
            rule.id = mt_triggers::rules::new_id(&used);
        }
        match self.persist_rule(&rule, cx) {
            Ok(_) => {
                self.toast(ToastKind::Success, format!("Saved \"{}\"", rule.name), cx);
                self.close_rule_editor(cx);
                cx.defer(|cx| {
                    runtime::sync(cx);
                });
            }
            Err(EditError::Stale) => {
                if let Some(ed) = self.triggers.editor.as_mut() {
                    ed.stale = true;
                    ed.error =
                        Some("triggers.toml changed on disk since you opened this rule".into());
                }
            }
            Err(e) => {
                if let Some(ed) = self.triggers.editor.as_mut() {
                    ed.error = Some(e.to_string());
                }
            }
        }
        cx.notify();
    }

    /// Stale save: re-reads the file and keeps the draft on top of it ("Reload and re-apply").
    pub(crate) fn reload_rule_editor(&mut self, cx: &mut Context<Self>) {
        let text = std::fs::read_to_string(runtime::path(cx)).unwrap_or_default();
        if let Some(ed) = self.triggers.editor.as_mut() {
            ed.base = store::base_of(&text, ed.id.as_deref());
            ed.stale = false;
            ed.error = None;
        }
        cx.notify();
    }

    /// **Delete** (asks first unless `triggers.confirm_delete = false`).
    pub(crate) fn delete_rule_editor(&mut self, cx: &mut Context<Self>) {
        let confirm = self.cfg.triggers.confirm_delete;
        let Some(ed) = self.triggers.editor.as_mut() else {
            return;
        };
        let Some(id) = ed.id.clone() else {
            return self.close_rule_editor(cx);
        };
        if confirm && !ed.confirm_delete {
            ed.confirm_delete = true;
            cx.notify();
            return;
        }
        let base = ed.base.clone();
        match store::remove_rule(&runtime::path(cx), &base, &id) {
            Ok(_) => {
                self.toast(ToastKind::Success, "Trigger deleted", cx);
                self.close_rule_editor(cx);
                cx.defer(|cx| {
                    runtime::sync(cx);
                });
            }
            Err(e) => {
                if let Some(ed) = self.triggers.editor.as_mut() {
                    ed.confirm_delete = false;
                    ed.error = Some(e.to_string());
                }
            }
        }
        cx.notify();
    }

    /// **Duplicate**: a new rule with the same fields (saved as a separate rule).
    pub(crate) fn duplicate_rule_editor(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.triggers.editor.as_ref() else {
            return;
        };
        let Ok(mut rule) = ed.to_rule() else {
            return;
        };
        rule.id.clear();
        rule.name = format!("{} copy", rule.name);
        let text = std::fs::read_to_string(runtime::path(cx)).unwrap_or_default();
        let mut copy = Editor::open(rule, None, store::base_of(&text, None));
        copy.focus = Focus::Name;
        self.triggers.editor = Some(copy);
        cx.notify();
    }

    /// Flips a rule's switch in the list; written back to the file.
    pub(crate) fn set_trigger_enabled(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        let Some(mut rule) = cx
            .try_global::<TriggerRuntime>()
            .and_then(|r| r.engine.rule(id).cloned())
        else {
            return;
        };
        rule.enabled = on;
        let path = runtime::path(cx);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let base = store::base_of(&text, Some(id));
        match store::save_rule(&path, &base, &rule) {
            Ok(_) => {
                cx.defer(|cx| {
                    runtime::sync(cx);
                });
            }
            Err(e) => self.toast(ToastKind::Error, format!("Cannot save: {e}"), cx),
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ test and preview

    /// The last 200 logical lines of the focused pane with their stable ids.
    fn recent_lines(&self, scope: mt_triggers::MatchIn) -> Vec<(u64, String)> {
        self.live()
            .map_or_else(Vec::new, |s| runtime::recent_lines(&s.term, scope))
    }

    fn base_ctx(&self, cx: &mut Context<Self>) -> FireCtx {
        let Some(pane) = self.focused_pane() else {
            return FireCtx::default();
        };
        let row = crate::mux_host::host(cx)
            .mux
            .rows(&|_| mt_mux::ctl::Live::default())
            .panes
            .into_iter()
            .find(|p| p.id == pane);
        row.map_or_else(FireCtx::default, |r| FireCtx {
            pane: r.id.to_string(),
            title: r.title,
            cwd: r.cwd.unwrap_or_default(),
            domain: r.domain,
            ..FireCtx::default()
        })
    }

    /// **Test**: a dry run over the last 200 lines of the focused pane (or sample events). It
    /// runs nothing: no decoration is kept, nothing is sent, no file is written.
    pub(crate) fn test_rule_editor(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(rule)) = self.triggers.editor.as_ref().map(Editor::to_rule) else {
            return;
        };
        let base = self.base_ctx(cx);
        let result = if rule.is_text() {
            let lines: Vec<String> = self
                .recent_lines(rule.scope)
                .into_iter()
                .map(|l| l.1)
                .collect();
            mt_triggers::dry_run_text(&rule, &lines, &base)
        } else {
            mt_triggers::dry_run_event(&rule, &base)
        };
        if let Some(ed) = self.triggers.editor.as_mut() {
            ed.test = Some(result);
        }
        cx.notify();
    }

    /// Recomputes the preview 150 ms after the last edit of the regex.
    pub(crate) fn schedule_trigger_preview(&mut self, cx: &mut Context<Self>) {
        self.triggers.preview_epoch += 1;
        self.triggers.preview_pending = true;
        let epoch = self.triggers.preview_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            this.update(cx, |this, cx| {
                if this.triggers.preview_epoch == epoch {
                    this.triggers.preview_pending = false;
                    this.refresh_trigger_preview(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// New output in the focused pane: the preview follows it (at most one refresh per 150 ms).
    pub(crate) fn follow_trigger_preview(&mut self, cx: &mut Context<Self>) {
        if self.triggers.editor.is_some() && !self.triggers.preview_pending {
            self.schedule_trigger_preview(cx);
        }
    }

    pub(crate) fn refresh_trigger_preview(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.triggers.editor.as_ref() else {
            return;
        };
        let pane = self.focused_pane();
        let (summary, hits) = if ed.is_text && !ed.regex.text().is_empty() {
            let lines = self.recent_lines(ed.scope);
            match preview::scan(&lines, ed.regex.text(), ed.case_insensitive) {
                Some(p) => (Some(p.summary(lines.len())), p.hits),
                None => (None, Vec::new()),
            }
        } else {
            (None, Vec::new())
        };
        if let Some(ed) = self.triggers.editor.as_mut() {
            ed.preview = summary;
        }
        if let Some(pane) = pane {
            cx.update_global::<TriggerRuntime, _>(|rt, _| rt.deco.set_preview(pane, hits));
            cx.defer(crate::mux_host::refresh_windows);
        }
        cx.notify();
    }

    /// How many panes a selector matches, for the editor's live "matches N panes".
    pub(crate) fn selector_pane_count(
        &self,
        expr: &str,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        if expr.trim().is_empty() || expr.trim() == "*" {
            return Ok(crate::mux_host::host(cx)
                .mux
                .rows(&|_| mt_mux::ctl::Live::default())
                .panes
                .len());
        }
        let sel = Selector::parse(expr).map_err(|e| e.message)?;
        sel.check(mt_proto::selector::Resource::Pane)
            .map_err(|e| e.message)?;
        let rows = crate::mux_host::host(cx)
            .mux
            .rows(&|_| mt_mux::ctl::Live::default());
        Ok(rows
            .panes
            .iter()
            .filter(|p| mt_mux::ctl::matches(&sel, *p))
            .count())
    }

    // ------------------------------------------------------------------ findings

    /// A click on a finding row.
    pub(crate) fn click_finding(&mut self, index: usize, cx: &mut Context<Self>) {
        let whole = self.triggers.flash_whole_line;
        cx.defer(move |cx| {
            runtime::jump_to_finding(cx, index, whole);
        });
    }

    // ------------------------------------------------------------------ keys

    pub(crate) fn triggers_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = ev.keystroke.key.as_str();
        let m = ev.keystroke.modifiers;
        if self.triggers.editor.is_none() {
            if key == "escape" {
                self.triggers.panel.focused = false;
                self.sync_focus(window);
                cx.notify();
            }
            return;
        }
        let secondary = m.platform || m.control;
        match key {
            "escape" => return self.rule_editor_escape(cx),
            "s" if secondary => return self.save_rule_editor(cx),
            "enter" => {
                let confirm_delete = self
                    .triggers
                    .editor
                    .as_ref()
                    .is_some_and(|e| e.confirm_delete);
                let confirm_discard = self
                    .triggers
                    .editor
                    .as_ref()
                    .is_some_and(|e| e.confirm_discard);
                if confirm_delete {
                    return self.delete_rule_editor(cx);
                }
                if confirm_discard {
                    return self.close_rule_editor(cx);
                }
                if secondary {
                    return self.save_rule_editor(cx);
                }
                return;
            }
            "tab" => {
                if let Some(ed) = self.triggers.editor.as_mut() {
                    ed.focus_next(m.shift);
                }
                cx.notify();
                return;
            }
            _ => {}
        }
        let platform = self.platform;
        let Some(ed) = self.triggers.editor.as_mut() else {
            return;
        };
        let focus = ed.focus;
        let Some(edit) = ed.line_edit() else {
            return;
        };
        if edit_key(edit, ev, platform, cx) {
            ed.confirm_discard = false;
            ed.test = None;
            ed.error = None;
            if focus == Focus::Regex {
                self.schedule_trigger_preview(cx);
            }
            cx.notify();
        }
    }

    // ------------------------------------------------------------------ panel width drag

    pub(crate) fn begin_triggers_drag(&mut self, x: f32) {
        self.triggers.drag = Some(PanelDrag {
            start_x: x,
            start_width: self.triggers.panel.width,
        });
    }

    pub(crate) fn drag_triggers_to(&mut self, x: f32, cx: &mut Context<Self>) {
        if let Some(d) = self.triggers.drag {
            self.triggers.panel.width = clamp_width(d.start_width + (d.start_x - x));
            cx.notify();
        }
    }

    pub(crate) fn end_triggers_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.triggers.drag.take().is_some() {
            self.persist_panel_state();
            self.relayout(window);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_become_escaped_first_line_regexes() {
        assert_eq!(
            regex_of_selection("error: build failed"),
            r"error: build failed"
        );
        assert_eq!(regex_of_selection("a.b*c(d)"), r"a\.b\*c\(d\)");
        assert_eq!(regex_of_selection("first\nsecond"), "first");
        assert_eq!(regex_of_selection(&"x".repeat(500)).len(), 200);
        assert_eq!(regex_of_selection("tail   "), "tail");
        let re = regex::Regex::new(&regex_of_selection("[E0308] a+b?")).unwrap();
        assert!(re.is_match("see [E0308] a+b? here"));
    }

    #[test]
    fn width_is_clamped_to_280_600() {
        assert_eq!(clamp_width(100.0), 280.0);
        assert_eq!(clamp_width(1000.0), 600.0);
        assert_eq!(clamp_width(360.0), 360.0);
    }
}
