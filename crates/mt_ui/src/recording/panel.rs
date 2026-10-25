//! The Recordings side panel in the window (SPEC §10.10.5): per-window state, the toggle and
//! focus state machine, row actions, import, and the export dialog and its jobs. Rendering is
//! in [`super::panel_view`]; the pure list logic is in [`super::panel_state`].

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{Context, FocusHandle, KeyDownEvent, Window};
use mt_proto::{RecordingStopParams, ReplayParams, Request};
use mt_record::{Sidecar, Store};

use super::{
    export::{ExportDialog, ExportJob},
    gate::RecordingGate,
    ipc,
    panel_state::{self, clamp_width, Entry},
};
use crate::{
    app::{typed_text, AppWindow, ToastButton},
    components::{TextEdit, ToastKind},
    designer_view::Clip,
    mux_host,
    overlay_stack::OverlayKind,
    snippets::{
        panel_state::{Escaped, Panel, Toggled},
        ui::{PanelDrag, RowMenu},
    },
};

/// Everything the recordings feature keeps per window.
pub struct RecordingsUi {
    pub panel: Panel,
    pub focus: FocusHandle,
    pub drag: Option<PanelDrag>,
    /// The saved recordings, refreshed when the panel opens and after each change.
    pub entries: Vec<Entry>,
    pub total_size: u64,
    /// The list must be re-read from the store before the next paint.
    pub stale: bool,
    pub error: Option<String>,
    /// `(id, new title)` while a row is being renamed.
    pub rename: Option<(String, TextEdit)>,
    pub delete_confirm: Option<String>,
    pub row_menu: Option<RowMenu>,
    pub dialog: Option<ExportDialog>,
    pub jobs: Vec<ExportJob>,
    pub job_seq: u64,
    /// The most recent recording saved in this window, for `ExportRecording` without a selection.
    pub last_saved: Option<String>,
    /// Shaped text of the export dialog's text fields (by focus index), measured at paint time
    /// so a click or drag can be turned into a caret position.
    pub field_layout: ExportLayouts,
    /// The export field a mouse drag started in.
    pub field_drag: Option<usize>,
}

/// Focus index to the field's bounds and shaped line.
pub type ExportLayouts = std::rc::Rc<
    std::cell::RefCell<
        std::collections::HashMap<usize, (gpui::Bounds<gpui::Pixels>, gpui::ShapedLine)>,
    >,
>;

impl RecordingsUi {
    pub fn new(cfg: &mt_config::Config, focus: FocusHandle) -> Self {
        let saved = if cfg!(test) {
            crate::state::UiState::default()
        } else {
            crate::state::state_dir()
                .map(|d| crate::state::load_ui(&d))
                .unwrap_or_default()
        };
        let width = saved
            .recordings_width
            .unwrap_or(cfg.recording.panel_width as f32);
        let open = RecordingGate::of_config(cfg).enabled
            && saved
                .recordings_open
                .unwrap_or(cfg.recording.panel_open_on_start);
        let mut panel = Panel::new(clamp_width(width), open);
        panel.width = clamp_width(width);
        Self {
            panel,
            focus,
            drag: None,
            entries: Vec::new(),
            total_size: 0,
            stale: true,
            error: None,
            rename: None,
            delete_confirm: None,
            row_menu: None,
            dialog: None,
            jobs: Vec::new(),
            job_seq: 0,
            last_saved: None,
            field_layout: Default::default(),
            field_drag: None,
        }
    }

    /// Closes the panel and drops the transient state that lives inside it.
    pub fn close_panel(&mut self) {
        self.panel.close();
        self.rename = None;
        self.delete_confirm = None;
        self.row_menu = None;
    }
}

/// Applies one editing key to a single-line field. Returns whether the key was an edit.
pub(crate) fn edit_key(
    edit: &mut TextEdit,
    ev: &KeyDownEvent,
    platform: crate::platform::Platform,
    cx: &mut gpui::App,
) -> bool {
    let m = ev.keystroke.modifiers;
    let secondary = m.platform || m.control;
    match ev.keystroke.key.as_str() {
        "backspace" if m.alt => edit.delete_word_back(),
        "backspace" => edit.backspace(),
        "delete" => edit.delete_forward(),
        "left" => edit.left(m.shift),
        "right" => edit.right(m.shift),
        "home" => edit.home(m.shift),
        "end" => edit.end(m.shift),
        "a" if secondary => edit.select_all(),
        "v" if secondary => match cx.read_from_clipboard().and_then(|i| i.text()) {
            Some(t) => edit.insert(&t),
            None => return false,
        },
        "c" if secondary => {
            if let Some(t) = edit.selected_text() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
            }
        }
        "x" if secondary => match edit.take_selection() {
            Some(t) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(t)),
            None => return false,
        },
        _ => match typed_text(ev, platform) {
            Some(t) => edit.insert(t),
            None => return false,
        },
    }
    true
}

fn store() -> Option<Store> {
    Store::from_env()
}

impl AppWindow {
    pub(crate) fn recordings_visible(&self) -> bool {
        self.recordings.panel.open && RecordingGate::of_config(&self.cfg).enabled
    }

    // ------------------------------------------------------------------ list data

    /// Re-reads the store (SPEC §10.10.5: the list is the store plus the running recordings).
    pub(crate) fn refresh_recordings(&mut self, cx: &mut Context<Self>) {
        self.recordings.stale = false;
        let running: Vec<String> = mux_host::host_ref(cx)
            .store_recorders
            .running()
            .into_iter()
            .map(|r| r.1)
            .collect();
        match store() {
            Some(st) => match st.list_skipping(&running) {
                Ok(list) => {
                    self.recordings.entries = list
                        .into_iter()
                        .map(|sc| Entry {
                            sc,
                            running: false,
                            pane: None,
                        })
                        .collect();
                    self.recordings.total_size = st.total_size();
                    self.recordings.error = None;
                }
                Err(e) => self.recordings.error = Some(e.to_string()),
            },
            None => {
                self.recordings.error =
                    Some("Cannot locate the recordings store: HOME is not set".into())
            }
        }
        self.reconcile_recordings(cx);
    }

    /// Saved entries plus the recordings still being written.
    pub(crate) fn recording_entries(&self, cx: &gpui::App) -> Vec<Entry> {
        let mut v = self.recordings.entries.clone();
        let now = mt_record::store::unix_now();
        let rec = &mux_host::host_ref(cx).store_recorders;
        for (pane, id, elapsed) in rec.running() {
            v.retain(|e| e.sc.id != id);
            v.push(Entry {
                sc: Sidecar {
                    id,
                    title: rec.title_of(pane).unwrap_or("Recording").to_string(),
                    created_at: now.saturating_sub(elapsed.as_secs()),
                    duration_ms: elapsed.as_millis() as u64,
                    has_input: rec.captures_input(pane),
                    ..Sidecar::default()
                },
                running: true,
                pane: Some(pane),
            });
        }
        v
    }

    pub(crate) fn recording_rows(&self, cx: &gpui::App) -> Vec<panel_state::Row> {
        panel_state::rows(
            &self.recording_entries(cx),
            self.recordings.panel.query.text(),
            mt_record::store::unix_now(),
        )
    }

    fn visible_recording_ids(&self, cx: &gpui::App) -> Vec<String> {
        panel_state::visible_ids(&self.recording_rows(cx))
    }

    fn reconcile_recordings(&mut self, cx: &gpui::App) {
        let ids = self.visible_recording_ids(cx);
        panel_state::reconcile(&mut self.recordings.panel.selected, &ids);
    }

    /// A recording was saved, renamed, deleted or imported.
    pub(crate) fn recordings_changed(&mut self, select: Option<String>, cx: &mut Context<Self>) {
        if let Some(id) = &select {
            self.recordings.last_saved = Some(id.clone());
        }
        if self.recordings.panel.open {
            self.refresh_recordings(cx);
            if select.is_some() {
                self.recordings.panel.selected = select;
            }
        } else {
            self.recordings.stale = true;
        }
        cx.notify();
    }

    pub(crate) fn selected_entry(&self, cx: &gpui::App) -> Option<Entry> {
        let id = self.recordings.panel.selected.as_ref()?;
        self.recording_entries(cx)
            .into_iter()
            .find(|e| &e.sc.id == id)
    }

    // ------------------------------------------------------------------ toggle and focus

    /// `ToggleRecordingsPanel`: closed → open and focus search; unfocused → focus; focused →
    /// close.
    pub(crate) fn toggle_recordings_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording_guard(cx) {
            return;
        }
        if self.recordings.panel.toggle() == Toggled::Closed {
            self.recordings.close_panel();
        } else {
            self.snippets.panel.close();
            self.triggers.close_panel();
            self.refresh_recordings(cx);
        }
        self.after_panel_change(window, cx);
    }

    /// `FocusRecordingsPanel`: opens if needed, never closes.
    pub(crate) fn focus_recordings_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording_guard(cx) {
            return;
        }
        self.recordings.panel.focus();
        self.snippets.panel.close();
        self.triggers.close_panel();
        self.refresh_recordings(cx);
        self.after_panel_change(window, cx);
    }

    /// The recording gate changed: the panel and its dialogs go away with the feature.
    pub(crate) fn recordings_gate_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if RecordingGate::of_config(&self.cfg).enabled {
            return;
        }
        if self.recordings.dialog.take().is_some() {
            self.overlays.close(OverlayKind::ExportDialog);
        }
        for j in &self.recordings.jobs {
            j.progress.cancel();
        }
        let was_open = self.recordings.panel.open;
        self.recordings.close_panel();
        self.recordings.entries.clear();
        self.recordings.stale = true;
        if was_open {
            self.relayout(window);
        }
        self.sync_focus(window);
        cx.notify();
    }

    // ------------------------------------------------------------------ keys

    pub(crate) fn recordings_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = ev.keystroke.key.as_str();
        if self.recordings.rename.is_some() {
            return self.recording_rename_key(ev, cx);
        }
        if self.recordings.delete_confirm.is_some() {
            match key {
                "enter" => self.confirm_recording_delete(cx),
                "escape" => {
                    self.recordings.delete_confirm = None;
                    cx.notify();
                }
                _ => {}
            }
            return;
        }
        let m = ev.keystroke.modifiers;
        let secondary = m.platform || m.control;
        let ids = self.visible_recording_ids(cx);
        match key {
            "escape" => {
                if self.recordings.panel.escape() == Escaped::FocusTerminal {
                    self.sync_focus(window);
                }
            }
            "up" => panel_state::move_selection(&mut self.recordings.panel.selected, &ids, -1),
            "down" => panel_state::move_selection(&mut self.recordings.panel.selected, &ids, 1),
            "enter" => self.replay_selected(if m.alt { 2.0 } else { 1.0 }, cx),
            "f2" => self.begin_recording_rename(cx),
            "e" if secondary => self.export_recording(None, window, cx),
            "delete" => self.ask_recording_delete(cx),
            // Backspace edits the search; on an empty search it deletes the row.
            "backspace" if self.recordings.panel.query.text().is_empty() && !m.alt => {
                self.ask_recording_delete(cx)
            }
            _ => {
                let platform = self.platform;
                if edit_key(&mut self.recordings.panel.query, ev, platform, cx) {
                    let ids = self.visible_recording_ids(cx);
                    panel_state::reconcile(&mut self.recordings.panel.selected, &ids);
                } else {
                    return;
                }
            }
        }
        cx.notify();
    }

    /// Copy / Paste / Select all in the export dialog's focused text field. Consumed (returns
    /// true) whenever the dialog is on top, also with no text field focused, so nothing leaks
    /// to the terminal pane behind it.
    pub(crate) fn export_clipboard(&mut self, clip: Clip, cx: &mut Context<Self>) -> bool {
        if self.overlays.top() != Some(crate::overlay_stack::OverlayKind::ExportDialog) {
            return false;
        }
        let paste = cx.read_from_clipboard().and_then(|i| i.text());
        if let Some(d) = self.recordings.dialog.as_mut() {
            if let Some(edit) = d.field_mut() {
                match clip {
                    Clip::Copy => {
                        if let Some(t) = edit.selected_text() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                        }
                    }
                    Clip::Paste => {
                        if let Some(t) = paste {
                            edit.insert(&t);
                            d.touched();
                        }
                    }
                    Clip::SelectAll => edit.select_all(),
                }
            }
        }
        cx.notify();
        true
    }

    /// Copy / Paste / Select all for whichever text field owns the keyboard: the designer, the
    /// export dialog, or a side panel.
    pub(crate) fn text_clipboard(&mut self, clip: Clip, cx: &mut Context<Self>) -> bool {
        self.designer_clipboard(clip, cx)
            || self.export_clipboard(clip, cx)
            || self.snippets_clipboard(clip, cx)
    }

    /// Mouse down in an export text field: place the caret, select a word (double click) or
    /// everything (triple click).
    pub(crate) fn export_field_down(
        &mut self,
        ix: usize,
        ev: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let idx = self.export_index_at(ix, ev.position.x);
        let Some(d) = self.recordings.dialog.as_mut() else {
            return;
        };
        let was = d.focus == ix;
        d.focus = ix;
        d.theme_open = false;
        if let Some(edit) = d.edit_at(ix) {
            match ev.click_count {
                2 => edit.select_word_at(idx),
                n if n >= 3 => edit.select_all(),
                _ => edit.set_caret(idx, ev.modifiers.shift && was),
            }
        }
        self.recordings.field_drag = (ev.click_count == 1).then_some(ix);
        cx.stop_propagation();
        cx.notify();
    }

    /// Dragging with the button held extends the selection.
    pub(crate) fn export_field_drag(
        &mut self,
        ix: usize,
        ev: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if self.recordings.field_drag != Some(ix)
            || ev.pressed_button != Some(gpui::MouseButton::Left)
        {
            return;
        }
        let idx = self.export_index_at(ix, ev.position.x);
        if let Some(edit) = self.recordings.dialog.as_mut().and_then(|d| d.edit_at(ix)) {
            edit.set_caret(idx, true);
            cx.notify();
        }
    }

    /// Byte offset in field `ix`'s text under window x `x`.
    fn export_index_at(&self, ix: usize, x: gpui::Pixels) -> usize {
        match self.recordings.field_layout.borrow().get(&ix) {
            Some((bounds, line)) => line.closest_index_for_x(x - bounds.origin.x),
            None => usize::MAX,
        }
    }

    /// Copy / Paste / Select all while the panel owns focus.
    pub(crate) fn recordings_clipboard(&mut self, clip: Clip, cx: &mut Context<Self>) -> bool {
        let paste = cx.read_from_clipboard().and_then(|i| i.text());
        let edit = match self.recordings.rename.as_mut() {
            Some((_, e)) => e,
            None => &mut self.recordings.panel.query,
        };
        match clip {
            Clip::Copy => {
                if let Some(t) = edit.selected_text() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                }
            }
            Clip::Paste => {
                if let Some(t) = paste {
                    edit.insert(&t);
                }
            }
            Clip::SelectAll => edit.select_all(),
        }
        let ids = self.visible_recording_ids(cx);
        panel_state::reconcile(&mut self.recordings.panel.selected, &ids);
        cx.notify();
        true
    }

    // ------------------------------------------------------------------ row actions

    /// Replays a saved recording in a new tab (`ctl replay`'s path).
    pub(crate) fn replay_recording(&mut self, id: &str, speed: f64, cx: &mut Context<Self>) {
        let Some(st) = store() else { return };
        let path = st.cast_path(id);
        if !path.is_file() {
            self.toast(ToastKind::Error, "That recording is no longer saved", cx);
            return;
        }
        let params = ReplayParams {
            path: path.to_string_lossy().into_owned(),
            speed,
            new_window: false,
        };
        cx.spawn(async move |this, cx| {
            let r = cx.update(|app| mux_host::replay(app, &params));
            if let Ok(Err(e)) = r {
                let _ = this.update(cx, |this, cx| {
                    this.toast(
                        ToastKind::Error,
                        format!("Replay failed: {}", e.message),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    pub(crate) fn replay_selected(&mut self, speed: f64, cx: &mut Context<Self>) {
        if let Some(e) = self.selected_entry(cx).filter(|e| !e.running) {
            self.replay_recording(&e.sc.id, speed, cx);
        }
    }

    pub(crate) fn begin_recording_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(e) = self.selected_entry(cx).filter(|e| !e.running) {
            let mut edit = TextEdit::new(e.sc.title.clone());
            edit.select_all();
            self.recordings.rename = Some((e.sc.id, edit));
            self.recordings.row_menu = None;
            cx.notify();
        }
    }

    fn recording_rename_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "escape" => self.recordings.rename = None,
            "enter" => self.commit_recording_rename(cx),
            _ => {
                let platform = self.platform;
                if let Some((_, edit)) = self.recordings.rename.as_mut() {
                    if !edit_key(edit, ev, platform, cx) {
                        return;
                    }
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn commit_recording_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, edit)) = self.recordings.rename.take() else {
            return;
        };
        let title = edit.text().trim().to_string();
        if title.is_empty() {
            return;
        }
        let Some(st) = store() else { return };
        match st.rename(&id, &title) {
            Ok(_) => self.recordings_changed(Some(id), cx),
            Err(e) => self.toast(ToastKind::Error, format!("Rename failed: {e}"), cx),
        }
    }

    /// Asks before deleting, unless `recording.confirm_delete` is off.
    pub(crate) fn ask_recording_delete(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry(cx) else {
            return;
        };
        self.recordings.row_menu = None;
        if e.running {
            self.toast(ToastKind::Info, "Stop the recording before deleting it", cx);
            return;
        }
        self.recordings.delete_confirm = Some(e.sc.id);
        if !self.cfg.recording.confirm_delete {
            return self.confirm_recording_delete(cx);
        }
        cx.notify();
    }

    pub(crate) fn confirm_recording_delete(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.recordings.delete_confirm.take() else {
            return;
        };
        let Some(st) = store() else { return };
        match st.remove(&id) {
            Ok(()) => {
                if self.recordings.panel.selected.as_deref() == Some(id.as_str()) {
                    // Land on the neighbour that is about to take the row's place.
                    let ids = self.visible_recording_ids(cx);
                    let at = ids.iter().position(|i| *i == id).unwrap_or(0);
                    self.recordings.panel.selected = ids
                        .get(at + 1)
                        .or_else(|| at.checked_sub(1).and_then(|p| ids.get(p)))
                        .cloned();
                }
                let keep = self.recordings.panel.selected.clone();
                self.recordings_changed(None, cx);
                self.recordings.panel.selected = keep;
                self.reconcile_recordings(cx);
            }
            Err(e) => self.toast(ToastKind::Error, format!("Delete failed: {e}"), cx),
        }
        cx.notify();
    }

    pub(crate) fn copy_recording_path(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(st) = store() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                st.cast_path(id).to_string_lossy().into_owned(),
            ));
            self.toast(ToastKind::Success, "Path copied", cx);
        }
    }

    pub(crate) fn reveal_recording(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(st) = store() {
            cx.reveal_path(&st.cast_path(id));
        }
    }

    pub(crate) fn reveal_store_folder(&mut self, cx: &mut Context<Self>) {
        match store() {
            Some(st) if st.dir().is_dir() => cx.reveal_path(st.dir()),
            _ => self.toast(ToastKind::Info, "There are no recordings yet", cx),
        }
    }

    /// Stops the running recording of `pane` (the running row's only action).
    pub(crate) fn stop_recording_of(&mut self, pane: u64, cx: &mut Context<Self>) {
        let req = Request::RecordingStop(RecordingStopParams {
            pane: Some(pane),
            id: None,
            all: false,
        });
        match ipc::handle(&mut self.window_host(cx), req) {
            Ok(v) => {
                let saved: Option<Sidecar> = v
                    .get("stopped")
                    .and_then(|a| a.get(0))
                    .and_then(|s| serde_json::from_value(s.clone()).ok());
                if let Some(s) = saved {
                    self.toast_saved(&s, cx);
                }
            }
            Err(e) => self.toast(
                ToastKind::Error,
                format!("Recording failed: {}", e.message),
                cx,
            ),
        }
        cx.notify();
    }

    /// Opens the file picker and copies the chosen `.cast` into the store.
    pub(crate) fn import_recording(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            if let Some(p) = paths.first() {
                let p = p.clone();
                let _ = this.update(cx, |this, cx| this.import_cast(&p, cx));
            }
        })
        .detach();
    }

    pub(crate) fn import_cast(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(st) = store() else { return };
        match st.import(path) {
            Ok(sc) => {
                self.toast(ToastKind::Success, format!("Imported {}", sc.title), cx);
                self.recordings_changed(Some(sc.id), cx);
            }
            Err(e) => self.toast(ToastKind::Error, format!("Import failed: {e}"), cx),
        }
    }

    // ------------------------------------------------------------------ width drag

    pub(crate) fn begin_recordings_drag(&mut self, x: f32) {
        self.recordings.drag = Some(PanelDrag {
            start_x: x,
            start_width: self.recordings.panel.width,
        });
    }

    pub(crate) fn drag_recordings_to(&mut self, x: f32, cx: &mut Context<Self>) {
        if let Some(d) = self.recordings.drag {
            self.recordings.panel.width = clamp_width(d.start_width + (d.start_x - x));
            cx.notify();
        }
    }

    pub(crate) fn end_recordings_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recordings.drag.take().is_some() {
            self.persist_panel_state();
            self.relayout(window);
            cx.notify();
        }
    }

    // ------------------------------------------------------------------ toast buttons

    pub(crate) fn toast_button(
        &mut self,
        button: ToastButton,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match button {
            ToastButton::OpenPanel => self.focus_recordings_panel(window, cx),
            ToastButton::Export(id) => self.export_recording(Some(id), window, cx),
            ToastButton::Reveal(p) => cx.reveal_path(&p),
            ToastButton::Open(p) => cx.open_with_system(&p),
        }
    }

    // ------------------------------------------------------------------ export

    /// `ExportRecording`: the dialog for `id`, else the selected recording, else the last saved.
    pub(crate) fn export_recording(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.recording_guard(cx) {
            return;
        }
        let id = id
            .or_else(|| self.recordings.panel.selected.clone())
            .or_else(|| self.recordings.last_saved.clone());
        let Some(id) = id else {
            self.toast(ToastKind::Info, "Select a recording to export", cx);
            return;
        };
        if mux_host::host_ref(cx)
            .store_recorders
            .running()
            .iter()
            .any(|r| r.1 == id)
        {
            self.toast(
                ToastKind::Info,
                "Stop the recording before exporting it",
                cx,
            );
            return;
        }
        let Some(st) = store() else { return };
        match st.load(&id) {
            Ok(sc) => {
                let themes = cx
                    .global::<crate::settings::Settings>()
                    .registry
                    .all()
                    .to_vec();
                self.recordings.dialog =
                    Some(ExportDialog::new(sc, st.cast_path(&id), &self.cfg).with_themes(themes));
                self.open_overlay(OverlayKind::ExportDialog, window, cx);
            }
            Err(e) => self.toast(ToastKind::Error, format!("Cannot export: {e}"), cx),
        }
    }

    pub(crate) fn export_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = ev.keystroke.key.as_str();
        let shift = ev.keystroke.modifiers.shift;
        let platform = self.platform;
        let Some(d) = self.recordings.dialog.as_mut() else {
            return;
        };
        match key {
            // The theme list is a sheet over the dialog: Esc and Enter close it first.
            "escape" if d.theme_open => d.theme_open = false,
            "enter" if d.theme_open => d.theme_open = false,
            "escape" => return self.close_export_dialog(window, cx),
            "tab" => {
                d.theme_open = false;
                d.move_focus(if shift { -1 } else { 1 })
            }
            "enter" => return self.submit_export(window, cx),
            "space" | " " if d.is_theme_focus() => d.toggle_theme_sheet(),
            "up" if d.focus == 0 => d.cycle_format(-1),
            "down" if d.focus == 0 => d.cycle_format(1),
            "left" | "right" | "space" | " " if d.focus == 0 || d.is_toggle() => {
                d.cycle_value(if key == "left" { -1 } else { 1 })
            }
            _ => {
                if let Some(edit) = d.field_mut() {
                    if edit_key(edit, ev, platform, cx) {
                        d.touched();
                    }
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn close_export_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(OverlayKind::ExportDialog, true, window, cx);
    }

    /// Validates the dialog and starts the job; a bad option or an existing file stays in it.
    pub(crate) fn submit_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = self.cfg.clone();
        let Some(d) = self.recordings.dialog.as_mut() else {
            return;
        };
        let (opts, path) = match d.build(&cfg) {
            Ok(v) => v,
            Err(e) => {
                d.error = Some(e);
                return cx.notify();
            }
        };
        if path.exists() && !d.overwrite_armed {
            d.overwrite_armed = true;
            d.error = Some("That file exists. Press Enter again to replace it".into());
            return cx.notify();
        }
        let (exporter, sidecar, cast, open) = (
            d.exporter(),
            d.sidecar.clone(),
            d.cast_path.clone(),
            d.open_when_done,
        );
        self.recordings.job_seq += 1;
        let n = self.recordings.job_seq;
        let themes = cx.global::<crate::settings::Settings>().registry.clone();
        let job = ExportJob::spawn(
            n,
            exporter,
            sidecar,
            cast,
            opts,
            path,
            true,
            open,
            Some(themes),
        );
        self.recordings.jobs.push(job);
        self.close_export_dialog(window, cx);
        self.watch_exports(cx);
    }

    /// Browse…: the native save panel (the text field stays the fallback).
    pub(crate) fn browse_export_path(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.recordings.dialog.as_ref() else {
            return;
        };
        let (dir, name) = d.browse_start();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(p))) = rx.await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(d) = this.recordings.dialog.as_mut() {
                        d.set_path(p.to_string_lossy().into_owned());
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Polls the running exports: repaints the progress bars and reports each result.
    fn watch_exports(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let Ok(more) = this.update(cx, |this, cx| this.poll_exports(cx)) else {
                break;
            };
            if !more {
                break;
            }
        })
        .detach();
    }

    /// Returns whether any export is still running.
    pub(crate) fn poll_exports(&mut self, cx: &mut Context<Self>) -> bool {
        let mut finished = Vec::new();
        self.recordings.jobs.retain(|j| match j.take_result() {
            Some(r) => {
                finished.push((
                    j.title.clone(),
                    j.format,
                    j.path.clone(),
                    j.open_when_done,
                    r,
                ));
                false
            }
            None => true,
        });
        for (title, format, path, open, r) in finished {
            match r {
                Ok(report) => {
                    if open {
                        cx.open_with_system(&path);
                    }
                    let text = format!(
                        "Exported {title} as {format} · {}",
                        super::ui::format_size(report.bytes)
                    );
                    self.toast(ToastKind::Success, text, cx);
                    if let Some(t) = self.toasts.last_mut() {
                        t.buttons =
                            vec![ToastButton::Reveal(path.clone()), ToastButton::Open(path)];
                    }
                }
                Err(mt_export::ExportError::Cancelled) => {
                    self.toast(ToastKind::Info, format!("Export of {title} cancelled"), cx)
                }
                Err(e) => self.toast(ToastKind::Error, format!("Export failed: {e}"), cx),
            }
        }
        cx.notify();
        !self.recordings.jobs.is_empty()
    }

    /// Instant of the last click on a row, for the double-click debounce.
    pub(crate) fn recording_click(&mut self, id: &str) -> bool {
        self.recordings.panel.click_allowed(id, Instant::now())
    }
}
