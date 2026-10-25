//! The snippet editor, Delete, Move to folder and Copy flows on `AppWindow`
//! (SPEC §10.8.6 "Editor", "Gestures").

use std::time::Duration;

use gpui::{Context, KeyDownEvent, Window};
use mt_config::edit::EditError;
use mt_snippets::{expand, store, Mode, Session, Snippet, Template};

use super::{
    context::PaneContext,
    editor::{Editor, EditorField},
    gate::SnippetsGate,
    library,
    panel_state::Gesture,
    ui::Intent,
};
use crate::{
    app::{typed_text, AppWindow, PromptKind},
    components::ToastKind,
    overlay_stack::OverlayKind,
};

impl AppWindow {
    fn snippets_file_text(&self, cx: &gpui::App) -> String {
        std::fs::read_to_string(library::path(cx)).unwrap_or_default()
    }

    /// Opens the editor in the panel on `snippet` (`id` = the entry being edited, if any).
    fn open_editor(
        &mut self,
        snippet: Snippet,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.snippets_file_text(cx);
        let base = store::base_of(&text, id.as_deref());
        self.snippets.editor = Some(Editor::open(snippet, id, base));
        self.snippets.preview = None;
        self.snippets.panel.focus();
        self.recordings.close_panel();
        self.after_panel_change(window, cx);
        self.schedule_preview(cx);
    }

    pub(crate) fn new_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.snippets_guard(cx) {
            return;
        }
        if self.snippets.editor.is_some() {
            self.snippets.panel.focus();
            self.recordings.close_panel();
            self.after_panel_change(window, cx);
            return;
        }
        self.open_editor(Snippet::default(), None, window, cx);
    }

    pub(crate) fn edit_snippet(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.snippets_guard(cx) {
            return;
        }
        let Some(s) = library::set(cx).get(id).cloned() else {
            return;
        };
        self.open_editor(s, Some(id.to_string()), window, cx);
    }

    pub(crate) fn duplicate_snippet(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut s) = library::set(cx).get(id).cloned() else {
            return;
        };
        s.title = format!("{} copy", s.title);
        s.keys.clear();
        s.id.clear();
        self.open_editor(s, None, window, cx);
    }

    pub(crate) fn save_selection_as_snippet(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.snippets_guard(cx) {
            return;
        }
        let trim = self.cfg.selection.trim_trailing_whitespace;
        let text = self
            .live()
            .and_then(|s| s.term.selection_text(trim))
            .filter(|t| !t.is_empty());
        match text {
            Some(t) => {
                let title: String = t.lines().next().unwrap_or("").chars().take(40).collect();
                self.open_editor(
                    Snippet {
                        title: title.trim().to_string(),
                        body: t,
                        ..Default::default()
                    },
                    None,
                    window,
                    cx,
                );
            }
            None => self.toast(ToastKind::Info, "Select some text first", cx),
        }
    }

    pub(crate) fn save_last_command_as_snippet(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.snippets_guard(cx) {
            return;
        }
        let cmd = self.live().and_then(|s| {
            s.term
                .has_marks()
                .then(|| s.term.last_command(1).map(|c| c.command))
                .flatten()
        });
        match cmd.filter(|c| !c.trim().is_empty()) {
            Some(c) => self.open_editor(
                Snippet {
                    title: c.chars().take(40).collect::<String>().trim().to_string(),
                    body: c,
                    ..Default::default()
                },
                None,
                window,
                cx,
            ),
            None => self.toast(
                ToastKind::Info,
                "No last command: it needs shell integration (OSC 133)",
                cx,
            ),
        }
    }

    /// `OpenSnippetsFile`: snippets.toml in `$VISUAL`/`$EDITOR`, else the system opener.
    pub(crate) fn open_snippets_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.snippets_guard(cx) {
            return;
        }
        let path = library::path(cx);
        if !path.exists() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&path, "") {
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

    // ------------------------------------------------------------------ saving

    pub(crate) fn save_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let templates_on = SnippetsGate::of(cx).templates_on();
        let set = library::set(cx);
        let path = library::path(cx);
        let Some(ed) = self.snippets.editor.as_mut() else {
            return;
        };
        if let Some(b) = ed.save_blocker(templates_on) {
            ed.error = Some(b);
            cx.notify();
            return;
        }
        let snippet = ed.to_snippet(&set);
        match store::save_snippet(&path, &ed.base, &snippet) {
            Ok(_) => {
                let title = snippet.title.clone();
                self.snippets.panel.selected = Some(snippet.id);
                self.snippets.editor = None;
                self.snippets.preview = None;
                self.toast(ToastKind::Success, format!("Saved \"{title}\""), cx);
                // Reloading updates every window, this one included: after this update.
                window.defer(cx, |_, cx| library::sync_and_notify(cx));
            }
            Err(EditError::Stale) => {
                ed.stale = true;
                ed.error =
                    Some("snippets.toml changed on disk since you opened this snippet".into());
            }
            Err(e) => ed.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// "Reload and re-apply": rebases the editor on the file as it is now, then Save applies the
    /// edits to it.
    pub(crate) fn reload_editor_base(&mut self, cx: &mut Context<Self>) {
        let text = self.snippets_file_text(cx);
        if let Some(ed) = self.snippets.editor.as_mut() {
            ed.base = store::base_of(&text, ed.id.as_deref());
            ed.stale = false;
            ed.error = None;
        }
        cx.notify();
    }

    pub(crate) fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.snippets.editor = None;
        self.snippets.preview = None;
        self.sync_focus(window);
        cx.notify();
    }

    fn editor_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.snippets.editor.as_mut() {
            Some(ed) if ed.dirty() && !ed.confirm_discard => {
                ed.confirm_discard = true;
                cx.notify();
            }
            Some(_) => self.close_editor(window, cx),
            None => {}
        }
    }

    // ------------------------------------------------------------------ preview

    /// Re-runs the editor preview 150 ms after the last edit (SPEC §10.8.6).
    pub(crate) fn schedule_preview(&mut self, cx: &mut Context<Self>) {
        self.snippets.preview_epoch += 1;
        let epoch = self.snippets.preview_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            this.update(cx, |this, cx| {
                if this.snippets.preview_epoch == epoch {
                    this.refresh_preview(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        let templates_on = SnippetsGate::of(cx).templates_on();
        let Some(body) = self
            .snippets
            .editor
            .as_ref()
            .map(|e| e.body.text().to_string())
        else {
            return;
        };
        self.snippets.preview = templates_on.then(|| {
            let tpl = Template::cached(&body);
            if let Some(d) = tpl.diagnostics().first() {
                return Err(d.message.clone());
            }
            let pane = self.tabs.get(self.active_tab).map(|t| t.pane).unwrap_or(0);
            let mut ctx: PaneContext = self.pane_context(pane, body.contains("clipboard"), cx);
            let mut session = Session::new(&ctx);
            expand(&tpl, &mut ctx, &mut session, Mode::Placeholder)
                .map(|e| e.text)
                .map_err(|e| e.to_string())
        });
        cx.notify();
    }

    // ------------------------------------------------------------------ keys

    pub(crate) fn editor_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        let secondary = m.platform || m.control;
        let platform = self.platform;
        let Some(ed) = self.snippets.editor.as_mut() else {
            return;
        };
        if ed.confirm_discard {
            match key {
                "enter" => self.close_editor(window, cx),
                "escape" => {
                    ed.confirm_discard = false;
                    cx.notify();
                }
                _ => {}
            }
            return;
        }
        if secondary && key == "s" {
            self.save_editor(window, cx);
            return;
        }
        if key == "escape" {
            self.editor_escape(window, cx);
            return;
        }
        if key == "tab" {
            ed.focus_next(m.shift);
            cx.notify();
            return;
        }
        let mut body_changed = false;
        let ext = m.shift;
        match ed.field {
            EditorField::Run => {
                if matches!(key, "space" | "enter") {
                    ed.run = !ed.run;
                }
            }
            EditorField::Body => {
                let b = &mut ed.body;
                match key {
                    "enter" => {
                        b.newline();
                        body_changed = true;
                    }
                    "backspace" if m.alt => {
                        b.delete_word_back();
                        body_changed = true;
                    }
                    "backspace" => {
                        b.backspace();
                        body_changed = true;
                    }
                    "delete" => {
                        b.delete_forward();
                        body_changed = true;
                    }
                    "left" if m.alt => b.word_left_move(ext),
                    "right" if m.alt => b.word_right_move(ext),
                    "left" => b.left(ext),
                    "right" => b.right(ext),
                    "up" if secondary => b.doc_home(ext),
                    "down" if secondary => b.doc_end(ext),
                    "up" => b.up(ext),
                    "down" => b.down(ext),
                    "home" => b.home(ext),
                    "end" => b.end(ext),
                    "a" if secondary => b.select_all(),
                    "x" if secondary => {
                        if let Some(t) = b.take_selection() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(t));
                            body_changed = true;
                        }
                    }
                    "c" if secondary => {
                        if let Some(t) = b.selected_text() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                        }
                    }
                    "v" if secondary => {
                        if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                            b.insert(&t);
                            body_changed = true;
                        }
                    }
                    _ => match typed_text(ev, platform) {
                        Some(t) => {
                            b.insert(t);
                            body_changed = true;
                        }
                        None => return,
                    },
                }
                b.scroll_to_caret(BODY_LINES);
            }
            _ => {
                let Some(e) = ed.line_edit() else {
                    return;
                };
                match key {
                    "enter" => ed.focus_next(false),
                    "backspace" if m.alt => e.delete_word_back(),
                    "backspace" => e.backspace(),
                    "delete" => e.delete_forward(),
                    "left" if m.alt => e.word_left_move(ext),
                    "right" if m.alt => e.word_right_move(ext),
                    "left" => e.left(ext),
                    "right" => e.right(ext),
                    "home" => e.home(ext),
                    "end" => e.end(ext),
                    "a" if secondary => e.select_all(),
                    "x" if secondary => {
                        if let Some(t) = e.take_selection() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(t));
                        }
                    }
                    "c" if secondary => {
                        if let Some(t) = e.selected_text() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                        }
                    }
                    "v" if secondary => {
                        if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                            e.insert(&t);
                        }
                    }
                    _ => match typed_text(ev, platform) {
                        Some(t) => e.insert(t),
                        None => return,
                    },
                }
            }
        }
        ed.error = None;
        if body_changed {
            self.schedule_preview(cx);
        }
        cx.notify();
    }

    /// Copy / Paste / Select all while the panel owns focus. Returns whether it was consumed.
    pub(crate) fn snippets_clipboard(
        &mut self,
        clip: crate::designer_view::Clip,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::designer_view::Clip;
        if self.focus_target() != crate::overlay_stack::FocusTarget::Panel {
            return false;
        }
        if !self.panel_visible() {
            return self.recordings_clipboard(clip, cx);
        }
        let paste = || cx.read_from_clipboard().and_then(|i| i.text());
        if let Some(ed) = self.snippets.editor.as_mut() {
            let mut changed = false;
            match (ed.field, clip) {
                (EditorField::Body, Clip::Copy) => {
                    if let Some(t) = ed.body.selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                    }
                }
                (EditorField::Body, Clip::Paste) => {
                    if let Some(t) = paste() {
                        ed.body.insert(&t);
                        changed = true;
                    }
                }
                (EditorField::Body, Clip::SelectAll) => ed.body.select_all(),
                (_, Clip::Copy) => {
                    if let Some(t) = ed
                        .line_edit()
                        .and_then(|e| e.selected_text().map(str::to_string))
                    {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(t));
                    }
                }
                (_, Clip::Paste) => {
                    if let (Some(t), Some(e)) = (paste(), ed.line_edit()) {
                        e.insert(&t);
                    }
                }
                (_, Clip::SelectAll) => {
                    if let Some(e) = ed.line_edit() {
                        e.select_all();
                    }
                }
            }
            if changed {
                self.schedule_preview(cx);
            }
        } else {
            let set = library::set(cx);
            let panel = &mut self.snippets.panel;
            match clip {
                Clip::Copy => {
                    if let Some(t) = panel.query.selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.to_string()));
                    }
                }
                Clip::Paste => {
                    if let Some(t) = paste() {
                        panel.query.insert(&t);
                        panel.reconcile(&set);
                    }
                }
                Clip::SelectAll => panel.query.select_all(),
            }
        }
        cx.notify();
        true
    }

    // ------------------------------------------------------------------ delete, move, copy

    pub(crate) fn ask_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.snippets.delete_confirm = Some(id.to_string());
        self.snippets.row_menu = None;
        cx.notify();
    }

    pub(crate) fn delete_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev.keystroke.key.as_str() {
            "enter" => self.confirm_delete(window, cx),
            "escape" => {
                self.snippets.delete_confirm = None;
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.snippets.delete_confirm.take() else {
            return;
        };
        let path = library::path(cx);
        let text = self.snippets_file_text(cx);
        let base = store::base_of(&text, Some(&id));
        match store::remove_snippet(&path, &base, &id) {
            Ok(_) => {
                self.toast(ToastKind::Success, "Snippet deleted", cx);
                window.defer(cx, |_, cx| library::sync_and_notify(cx));
            }
            Err(e) => self.toast(ToastKind::Error, e.to_string(), cx),
        }
        cx.notify();
    }

    /// "Move to folder…": the name prompt overlay.
    pub(crate) fn ask_move(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = library::set(cx)
            .get(id)
            .map(|s| s.folder.clone())
            .unwrap_or_default();
        self.snippets.move_target = Some(id.to_string());
        self.snippets.row_menu = None;
        self.begin_prompt(PromptKind::MoveSnippet, current, window, cx);
    }

    /// Called from the name prompt with the typed folder (may be empty = ungrouped).
    pub(crate) fn confirm_move(
        &mut self,
        folder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.snippets.move_target.take() else {
            return;
        };
        let set = library::set(cx);
        let Some(mut s) = set.get(&id).cloned() else {
            return;
        };
        s.folder = folder.trim().to_string();
        let path = library::path(cx);
        let text = self.snippets_file_text(cx);
        let base = store::base_of(&text, Some(&id));
        self.close_overlay(OverlayKind::NamePrompt, true, window, cx);
        match store::save_snippet(&path, &base, &s) {
            Ok(_) => window.defer(cx, |_, cx| library::sync_and_notify(cx)),
            Err(e) => self.toast(ToastKind::Error, e.to_string(), cx),
        }
    }

    pub(crate) fn copy_raw(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(s) = library::set(cx).get(id) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(s.body.clone()));
            self.toast(ToastKind::Success, format!("Copied \"{}\"", s.title), cx);
        }
    }

    pub(crate) fn copy_expanded(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = library::set(cx).get(id).cloned() else {
            return;
        };
        self.insert_snippet_value(&s, Gesture::default(), Intent::Copy, window, cx);
    }

    /// Writes a drag-reorder: `id` goes before `before` (`None` = the end).
    pub(crate) fn reorder_snippet(
        &mut self,
        id: &str,
        before: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = library::path(cx);
        let text = self.snippets_file_text(cx);
        let base = store::base_of(&text, Some(id));
        let (id, before) = (id.to_string(), before.map(str::to_string));
        match store::save_with(&path, &base, &id, |src| {
            store::reorder(src, &id, before.as_deref())
        }) {
            Ok(_) => window.defer(cx, |_, cx| library::sync_and_notify(cx)),
            Err(e) => self.toast(ToastKind::Error, e.to_string(), cx),
        }
    }
}

/// Lines of the editor body shown at once.
pub const BODY_LINES: usize = 8;
