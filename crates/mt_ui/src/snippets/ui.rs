//! Per-window snippets state and the `AppWindow` methods behind the panel, the insert flow and
//! the fill dialog (SPEC §10.8.5, §10.8.6).

use std::collections::HashMap;
use std::sync::Arc;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Context, FocusHandle, KeyDownEvent, Pixels, Point, Window};
use mt_snippets::{expand, Expanded, Mode, Session, Snippet, Template};

use super::{
    context::PaneContext,
    editor::Editor,
    fill::{Fill, FillAction},
    gate::SnippetsGate,
    insert::{self, Delivery},
    library,
    panel_state::{clamp_width, Escaped, Gesture, Panel, Toggled},
};
use crate::{
    app::{typed_text, AppWindow},
    components::ToastKind,
    mux_host,
    overlay_stack::OverlayKind,
    theme::Theme,
};

/// The right-click menu of a snippet row.
pub struct RowMenu {
    pub id: String,
    pub position: Point<Pixels>,
    pub nav: crate::menu::nav::MenuNav,
}

/// What an insert does with the expanded text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Insert,
    /// "Copy expanded": the clipboard instead of a pane.
    Copy,
}

/// A panel-edge drag in progress.
#[derive(Clone, Copy, Debug)]
pub struct PanelDrag {
    pub start_x: f32,
    pub start_width: f32,
}

/// Everything the snippets feature keeps per window.
pub struct SnippetsUi {
    pub panel: Panel,
    pub focus: FocusHandle,
    pub drag: Option<PanelDrag>,
    pub fill: Option<Fill>,
    /// `(snippet id, label)` → last value typed in this window; never secrets.
    pub remembered: HashMap<String, String>,
    /// Text waiting on the multi-line confirmation: one delivery per target pane.
    pub pending: Option<Vec<(u64, Delivery)>>,
    /// The editor replaces the list while it is open; the draft survives closing the panel.
    pub editor: Option<Editor>,
    pub row_menu: Option<RowMenu>,
    /// Snippet awaiting "Delete…" confirmation.
    pub delete_confirm: Option<String>,
    /// Snippet whose "Move to folder…" prompt is open.
    pub move_target: Option<String>,
    /// Painted origin of the editor body `(x, y)`, for mouse hit-testing.
    pub body_origin: Rc<Cell<(f32, f32)>>,
    pub preview_epoch: u64,
    /// The editor preview text (or its error), refreshed 150 ms after the last edit.
    pub preview: Option<Result<String, String>>,
    /// `⌥Enter` in the palette: the next picker confirmation runs the snippet.
    pub alt_confirm: bool,
}

impl SnippetsUi {
    pub fn new(cfg: &mt_config::Config, focus: FocusHandle) -> Self {
        // `ui.json` wins over the config once it exists (SPEC §8.3).
        let saved = if cfg!(test) {
            crate::state::UiState::default()
        } else {
            crate::state::state_dir()
                .map(|d| crate::state::load_ui(&d))
                .unwrap_or_default()
        };
        Self {
            panel: Panel::new(
                saved.panel_width.unwrap_or(cfg.snippets.panel_width as f32),
                SnippetsGate::of_config(cfg).enabled
                    && saved.panel_open.unwrap_or(cfg.snippets.panel_open_on_start),
            ),
            focus,
            drag: None,
            fill: None,
            remembered: HashMap::new(),
            pending: None,
            editor: None,
            row_menu: None,
            delete_confirm: None,
            move_target: None,
            body_origin: Rc::new(Cell::new((0.0, 0.0))),
            preview_epoch: 0,
            preview: None,
            alt_confirm: false,
        }
    }
}

/// What an insert needs once its values are known.
struct Job {
    intent: Intent,
    title: String,
    body: String,
    run: bool,
    targets: Vec<u64>,
    template: Option<Arc<Template>>,
    session: Option<Session>,
}

impl AppWindow {
    /// False (with the disabled toast) while the feature is off (SPEC §10.8.1).
    pub(crate) fn snippets_guard(&mut self, cx: &mut Context<Self>) -> bool {
        if SnippetsGate::of(cx).enabled {
            return true;
        }
        self.toast_open_config(
            ToastKind::Info,
            "Snippets are disabled — set `snippets.enabled = true`",
            cx,
        );
        false
    }

    /// The panel is open and the feature is on (this window's copy of the config).
    pub(crate) fn panel_visible(&self) -> bool {
        self.snippets.panel.open && SnippetsGate::of_config(&self.cfg).enabled
    }

    /// Where keyboard focus belongs: an overlay, else the panel, else the terminal.
    pub(crate) fn focus_target(&self) -> crate::overlay_stack::FocusTarget {
        use crate::overlay_stack::FocusTarget;
        match self.overlays.top() {
            Some(k) => FocusTarget::Overlay(k),
            None if self.panel_visible() && self.snippets.panel.focused => FocusTarget::Panel,
            None => FocusTarget::Viewport,
        }
    }

    /// `ToggleSnippetsPanel`: closed → open and focus search; unfocused → focus; focused → close.
    pub(crate) fn toggle_snippets_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.snippets_guard(cx) {
            return;
        }
        if self.snippets.panel.toggle() == Toggled::Closed {
            self.snippets.panel.close();
        } else {
            self.snippets.panel.reconcile(&library::set(cx));
        }
        self.after_panel_change(window, cx);
    }

    /// `FocusSnippetsPanel`.
    pub(crate) fn focus_snippets_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.snippets_guard(cx) {
            return;
        }
        self.snippets.panel.focus();
        self.snippets.panel.reconcile(&library::set(cx));
        self.after_panel_change(window, cx);
    }

    /// Remembers the panel's open state and width for new windows and the next run.
    pub(crate) fn persist_panel_state(&self) {
        if cfg!(test) {
            return;
        }
        if let Some(dir) = crate::state::state_dir() {
            let _ = crate::state::save_ui(
                &dir,
                &crate::state::UiState {
                    panel_open: Some(self.snippets.panel.open),
                    panel_width: Some(self.snippets.panel.width),
                },
            );
        }
    }

    /// Re-layouts the panes (the viewport width changed) and moves focus.
    pub(crate) fn after_panel_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.persist_panel_state();
        self.relayout(window);
        self.sync_focus(window);
        cx.notify();
    }

    /// The library, the gate or the config changed: refresh everything that depends on them.
    pub(crate) fn snippets_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let gate = SnippetsGate::of(cx);
        if !gate.enabled {
            // Turning the feature off closes the panel and any fill dialog (SPEC §10.8.1).
            if self.snippets.editor.take().is_some() {
                self.toast(
                    ToastKind::Info,
                    "Unsaved snippet draft discarded (snippets are disabled)",
                    cx,
                );
            }
            self.snippets.row_menu = None;
            self.snippets.delete_confirm = None;
            let had_overlay = self.snippets.fill.take().is_some();
            if had_overlay {
                self.overlays.close(OverlayKind::SnippetFill);
            }
            if self.snippets.panel.open {
                self.snippets.panel.close();
                self.relayout(window);
            }
            self.snippets.panel.focused = false;
        } else {
            let set = library::set(cx);
            self.snippets.panel.reconcile(&set);
        }
        self.sync_focus(window);
        cx.notify();
    }

    // ------------------------------------------------------------------ keys in the panel

    pub(crate) fn panel_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snippets.editor.is_some() {
            return self.editor_key(ev, window, cx);
        }
        if self.snippets.delete_confirm.is_some() {
            return self.delete_key(ev, window, cx);
        }
        let m = ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        let secondary = m.platform || m.control;
        let set = library::set(cx);
        if secondary && key == "e" {
            if let Some(id) = self.snippets.panel.selected.clone() {
                self.edit_snippet(&id, window, cx);
            }
            return;
        }
        let panel = &mut self.snippets.panel;
        match key {
            "escape" => {
                if panel.escape() == Escaped::FocusTerminal {
                    self.sync_focus(window);
                }
            }
            "up" => panel.move_selection(&set, -1),
            "down" => panel.move_selection(&set, 1),
            "enter" => {
                if let Some(id) = panel.selected.clone() {
                    let g = Gesture::from_modifiers(m.alt, m.shift, secondary);
                    self.insert_snippet(&id, g, window, cx);
                }
            }
            "backspace" if m.alt => {
                panel.query.delete_word_back();
                panel.reconcile(&set);
            }
            "backspace" => {
                panel.query.backspace();
                panel.reconcile(&set);
            }
            "delete" => {
                panel.query.delete_forward();
                panel.reconcile(&set);
            }
            "left" => panel.query.left(m.shift),
            "right" => panel.query.right(m.shift),
            "home" => panel.query.home(m.shift),
            "end" => panel.query.end(m.shift),
            "a" if secondary => panel.query.select_all(),
            "v" if secondary => {
                if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    panel.query.insert(&t);
                    panel.reconcile(&set);
                }
            }
            "c" if secondary => {
                if let Some(t) = panel.query.selected_text() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                }
            }
            _ => match typed_text(ev, self.platform) {
                Some(t) => {
                    panel.query.insert(t);
                    panel.reconcile(&set);
                }
                None => return,
            },
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ inserting

    /// The pane an insert goes to, or every pane of the active tab for a broadcast.
    pub(crate) fn insert_targets(&self, broadcast: bool) -> Vec<u64> {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return Vec::new();
        };
        if broadcast {
            tab.panes.iter().map(|p| p.pane).collect()
        } else {
            vec![tab.pane]
        }
    }

    pub(crate) fn pane_context(
        &self,
        pane: u64,
        needs_clipboard: bool,
        cx: &mut Context<Self>,
    ) -> PaneContext {
        // Never `mux_host::rows(cx)` here: it reads every open window, and this one is already
        // being updated (GPUI panics on the re-entrant read). Use this window's own terminals.
        let lives: HashMap<u64, mt_mux::ctl::Live> = self
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter())
            .filter_map(|v| {
                v.session
                    .as_ref()
                    .map(|s| (v.pane, crate::ipc_ops::live_of(&s.term)))
            })
            .collect();
        let rows = mux_host::host(cx)
            .mux
            .rows(&|id| lives.get(&id).cloned().unwrap_or_default());
        let term = self
            .tabs
            .iter()
            .find_map(|t| t.pane_view(pane))
            .and_then(|v| v.session.as_ref())
            .map(|s| s.term.clone());
        PaneContext {
            cfg: self.cfg.clone(),
            rows,
            pane: Some(pane),
            term,
            clipboard: needs_clipboard.then(|| {
                cx.read_from_clipboard()
                    .and_then(|i| i.text())
                    .unwrap_or_default()
            }),
            font_size: Some(self.font_size()),
            theme: Some(cx.global::<Theme>().name.to_string()),
        }
    }

    /// The one insert path for the panel, palette, keys and IPC (SPEC §10.8.5).
    pub(crate) fn insert_snippet(
        &mut self,
        id: &str,
        gesture: Gesture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.snippets_guard(cx) {
            return;
        }
        let set = library::set(cx);
        let Some(snippet) = set.get(id).cloned() else {
            self.toast(ToastKind::Warning, format!("No snippet \"{id}\""), cx);
            return;
        };
        self.insert_snippet_value(&snippet, gesture, Intent::Insert, window, cx);
    }

    pub(crate) fn insert_snippet_value(
        &mut self,
        snippet: &Snippet,
        gesture: Gesture,
        intent: Intent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.insert_targets(gesture.broadcast);
        if targets.is_empty() {
            self.toast(ToastKind::Warning, "No pane to insert into", cx);
            return;
        }
        let run = gesture.run.unwrap_or(snippet.run);
        let gate = SnippetsGate::of(cx);
        let mut job = Job {
            intent,
            title: snippet.title.clone(),
            body: snippet.body.clone(),
            run,
            targets,
            template: None,
            session: None,
        };
        if gate.templates_on() {
            let tpl = Template::cached(&snippet.body);
            if let Some(d) = tpl.diagnostics().first() {
                let (line, col) = d.line_col(&snippet.body);
                self.toast(
                    ToastKind::Error,
                    format!(
                        "Snippet \"{}\" has a template error at {line}:{col}: {}",
                        snippet.title, d.message
                    ),
                    cx,
                );
                return;
            }
            if !tpl.analysis.prompts.is_empty() {
                let ctx = self.pane_context(job.targets[0], tpl.src.contains("clipboard"), cx);
                let mut fill = Fill::new(
                    &snippet.id,
                    &snippet.title,
                    tpl,
                    &self.snippets.remembered,
                    &ctx,
                    run,
                    gesture.broadcast,
                    job.targets,
                );
                fill.copy = intent == Intent::Copy;
                self.snippets.fill = Some(fill);
                self.open_overlay(OverlayKind::SnippetFill, window, cx);
                return;
            }
            job.template = Some(tpl);
        }
        self.execute_insert(job, window, cx);
    }

    /// Evaluates for every target, then sends; any failure sends nothing (SPEC §10.8.3).
    fn execute_insert(&mut self, mut job: Job, window: &mut Window, cx: &mut Context<Self>) {
        let mut deliveries: Vec<(u64, Delivery)> = Vec::new();
        // The first pane's context and session: later panes fork the session, and the counters
        // are committed through it once everything expanded.
        let mut first: Option<(PaneContext, Session)> = None;
        let needs_clip = job
            .template
            .as_ref()
            .is_some_and(|t| t.src.contains("clipboard"));
        for (i, pane) in job.targets.clone().into_iter().enumerate() {
            let expanded: Expanded = match &job.template {
                None => Expanded::literal(&job.body),
                Some(tpl) => {
                    let mut ctx = self.pane_context(pane, needs_clip, cx);
                    let mut session = match (&first, job.session.take()) {
                        (_, Some(s)) => s,
                        (Some((_, s0)), None) => s0.fork(),
                        (None, None) => Session::new(&ctx),
                    };
                    match expand(tpl, &mut ctx, &mut session, Mode::Insert) {
                        Ok(e) => {
                            if i == 0 {
                                first = Some((ctx, session));
                            }
                            e
                        }
                        Err(e) => {
                            self.toast(
                                ToastKind::Error,
                                format!("Snippet \"{}\": {e}", job.title),
                                cx,
                            );
                            return;
                        }
                    }
                }
            };
            deliveries.push((pane, insert::plan(&expanded, job.run)));
        }
        // Counters advance only now that every pane expanded (SPEC §10.8.4).
        if let Some((mut ctx, session)) = first {
            session.commit(&mut ctx);
        }
        if job.intent == Intent::Copy {
            let text = deliveries
                .first()
                .map(|(_, d)| d.text.clone())
                .unwrap_or_default();
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            self.toast(ToastKind::Success, format!("Copied \"{}\"", job.title), cx);
            return;
        }
        let multiline = self.cfg.snippets.confirm_multiline
            && deliveries
                .iter()
                .any(|(_, d)| insert::is_multiline(&d.text));
        if multiline {
            let preview = deliveries[0].1.text.clone();
            self.snippets.pending = Some(deliveries);
            self.paste_confirm = Some(preview);
            self.open_overlay(OverlayKind::PasteConfirm, window, cx);
            return;
        }
        self.deliver_all(deliveries, window, cx);
    }

    pub(crate) fn deliver_all(
        &mut self,
        deliveries: Vec<(u64, Delivery)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (pane, d) in &deliveries {
            if let Some(term) = self
                .tabs
                .iter()
                .find_map(|t| t.pane_view(*pane))
                .and_then(|v| v.session.as_ref())
                .map(|s| s.term.clone())
            {
                insert::deliver(&*term, d);
            }
        }
        if self.cfg.snippets.focus_terminal_after_insert {
            self.snippets.panel.focused = false;
        }
        self.sync_focus(window);
        cx.notify();
    }

    // ------------------------------------------------------------------ fill dialog

    pub(crate) fn fill_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        let secondary = m.platform || m.control;
        let platform = self.platform;
        let Some(fill) = self.snippets.fill.as_mut() else {
            return;
        };
        let action = if secondary && key == "v" {
            if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                fill.type_text(&t);
            }
            FillAction::Nothing
        } else if secondary && key == "a" {
            if let Some(f) = fill.focused() {
                f.edit.select_all();
            }
            FillAction::Nothing
        } else if let Some(t) = typed_text(ev, platform).filter(|_| !matches!(key, "enter" | "tab"))
        {
            fill.type_text(t);
            FillAction::Nothing
        } else {
            fill.key(key, m.alt, m.shift)
        };
        match action {
            FillAction::Nothing => cx.notify(),
            FillAction::Cancel => self.cancel_fill(window, cx),
            FillAction::Insert { run } => self.confirm_fill(run, window, cx),
        }
    }

    pub(crate) fn cancel_fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.snippets.fill = None;
        self.close_overlay(OverlayKind::SnippetFill, true, window, cx);
    }

    fn confirm_fill(&mut self, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut fill) = self.snippets.fill.take() else {
            return;
        };
        fill.apply_values();
        for (k, v) in fill.to_remember() {
            self.snippets.remembered.insert(k, v);
        }
        let job = Job {
            intent: if fill.copy {
                Intent::Copy
            } else {
                Intent::Insert
            },
            title: fill.title.clone(),
            body: fill.template.src.clone(),
            run,
            targets: fill.targets.clone(),
            template: Some(fill.template.clone()),
            session: Some(fill.session),
        };
        self.close_overlay(OverlayKind::SnippetFill, true, window, cx);
        self.execute_insert(job, window, cx);
    }

    /// The preview text for the dialog, evaluated against the first target.
    pub(crate) fn fill_preview(&mut self, cx: &mut Context<Self>) -> Result<Expanded, String> {
        let (pane, clip) = match self.snippets.fill.as_ref() {
            Some(f) => (
                f.targets.first().copied().unwrap_or(0),
                f.template.src.contains("clipboard"),
            ),
            None => return Err(String::new()),
        };
        let mut ctx = self.pane_context(pane, clip, cx);
        let fill = self.snippets.fill.as_mut().expect("checked above");
        fill.preview(&mut ctx).map_err(|e| e.to_string())
    }

    // ------------------------------------------------------------------ panel width drag

    pub(crate) fn begin_panel_drag(&mut self, x: f32) {
        self.snippets.drag = Some(PanelDrag {
            start_x: x,
            start_width: self.snippets.panel.width,
        });
    }

    pub(crate) fn drag_panel_to(&mut self, x: f32, cx: &mut Context<Self>) {
        if let Some(d) = self.snippets.drag {
            // Dragging left widens the right-hand panel.
            self.snippets.panel.width = clamp_width(d.start_width + (d.start_x - x));
            cx.notify();
        }
    }

    pub(crate) fn end_panel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.snippets.drag.take().is_some() {
            self.persist_panel_state();
            self.relayout(window);
            cx.notify();
        }
    }
}
