//! Quick select, copy mode and search wired to the active pane (SPEC §10.4–10.6).
//!
//! The logic lives in `quick_select`, `copy_mode` and `search` over a [`TextGrid`]; this module
//! builds grids from the live terminal (or the mock screen for tabs without a shell), applies
//! the outcomes (clipboard, toasts, selection mirror, scrolling) and answers the view's
//! questions about where to draw marks.

use gpui::{ClipboardItem, Context, KeyDownEvent, Window};
use mt_patterns::QuickSet;
use mt_term::SelectionKind;

use crate::app::{typed_text, AppWindow};
use crate::components::ToastKind;
use crate::copy_mode::{self, CopyMode, Keymap, Outcome, SelKind, View};
use crate::overlay_stack::OverlayKind;
use crate::platform::Platform;
use crate::quick_select::{self, QuickHit, Typed};
use crate::search::{self, SearchMatch};
use crate::textgrid::TextGrid;

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub case_sensitive: bool,
    pub regex: bool,
    /// Index into `matches` of the highlighted hit.
    pub current: usize,
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
    /// The query is an invalid regex.
    pub error: Option<String>,
}

#[derive(Default)]
pub struct CopyState {
    pub mode: Option<CopyMode>,
    pub keymap: Keymap,
}

#[derive(Default)]
pub struct QuickState {
    pub hits: Vec<QuickHit>,
    pub typed: String,
}

/// `(viewport row, column, cells, is_current)` of a search hit drawn over the viewport.
pub type ViewMark = (usize, usize, usize, bool);

/// Longest text shown in a toast.
const TOAST_TEXT_MAX: usize = 48;

fn preview(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if one_line.chars().count() > TOAST_TEXT_MAX {
        let head: String = one_line.chars().take(TOAST_TEXT_MAX).collect();
        format!("{head}…")
    } else {
        one_line
    }
}

/// A key as the copy-mode keymap names it, or `None` for keys it never uses.
pub(crate) fn key_name(ev: &KeyDownEvent, platform: Platform) -> Option<String> {
    let k = &ev.keystroke;
    let m = &k.modifiers;
    if m.platform || m.function {
        return None;
    }
    if m.control {
        return Some(copy_mode::notation(true, false, &k.key));
    }
    match typed_text(ev, platform) {
        Some(" ") => Some("space".into()),
        Some(t) if t.chars().count() == 1 => Some(t.to_string()),
        Some(_) => None,
        None => Some(k.key.clone()),
    }
}

fn engine_kind(k: SelKind) -> SelectionKind {
    match k {
        SelKind::Char => SelectionKind::Simple,
        SelKind::Line => SelectionKind::Line,
        SelKind::Block => SelectionKind::Block,
    }
}

pub fn build_quick_set(cfg: &mt_config::Config) -> Result<QuickSet, mt_patterns::QuickError> {
    QuickSet::new(
        &cfg.quick_select.patterns,
        &cfg.quick_select.disable_builtin,
    )
}

impl AppWindow {
    // ------------------------------------------------------------------ grids

    /// `(absolute line of the top visible row, visible rows)`.
    pub(crate) fn view_geometry(&self) -> (usize, usize) {
        match self.live() {
            Some(s) => (
                s.snapshot
                    .history_size
                    .saturating_sub(s.snapshot.display_offset),
                s.snapshot.rows,
            ),
            None => (0, self.screen().len()),
        }
    }

    fn mock_grid(&self) -> TextGrid {
        let texts: Vec<String> = self.screen().iter().map(|l| l.text()).collect();
        let cols = texts
            .iter()
            .map(|t| t.chars().count())
            .max()
            .unwrap_or(1)
            .max(1);
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        TextGrid::from_strs(cols, &refs)
    }

    /// The visible rows.
    pub(crate) fn grid_visible(&self) -> TextGrid {
        match self.live() {
            Some(s) => {
                let (top, rows) = self.view_geometry();
                TextGrid::from_cells(top, s.snapshot.cols, s.term.lines_range(top, top + rows))
            }
            None => self.mock_grid(),
        }
    }

    /// Scrollback plus the screen.
    pub(crate) fn grid_full(&self) -> TextGrid {
        match self.live() {
            Some(s) => TextGrid::from_cells(
                0,
                s.snapshot.cols,
                s.term.lines_range(0, s.term.total_lines()),
            ),
            None => self.mock_grid(),
        }
    }

    /// The terminal cursor as `(absolute line, column)`.
    pub(crate) fn cursor_abs(&self) -> (usize, usize) {
        match self.live() {
            Some(s) => s.term.cursor_abs(),
            None => (self.screen().len().saturating_sub(1), 0),
        }
    }

    // ------------------------------------------------------------------ quick select

    pub(crate) fn prepare_quick_select(&mut self, cx: &mut Context<Self>) {
        let set = match build_quick_set(&self.cfg) {
            Ok(s) => s,
            Err(e) => {
                self.toast(
                    ToastKind::Warning,
                    format!("{e}; using built-in patterns"),
                    cx,
                );
                QuickSet::new(&[], &self.cfg.quick_select.disable_builtin).expect("built-ins")
            }
        };
        let hits = quick_select::scan(
            &self.grid_visible(),
            &set,
            self.cursor_abs(),
            &self.cfg.quick_select.alphabet,
        );
        self.quick = QuickState {
            hits,
            typed: String::new(),
        };
    }

    pub(crate) fn quick_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ch) = typed_text(ev, self.platform).and_then(|t| t.chars().next()) else {
            return;
        };
        let mut typed = std::mem::take(&mut self.quick.typed);
        let res = quick_select::type_char(&self.quick.hits, &mut typed, ch);
        self.quick.typed = typed;
        match res {
            Typed::Partial => cx.notify(),
            Typed::Miss => {}
            Typed::Hit { index, paste } => {
                let text = self.quick.hits[index].text.clone();
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.close_overlay(OverlayKind::QuickSelect, true, window, cx);
                if paste {
                    self.paste_text(text.clone(), window, cx);
                }
                self.toast(ToastKind::Success, format!("Copied {}", preview(&text)), cx);
            }
        }
    }

    // ------------------------------------------------------------------ search

    /// Re-runs the search over scrollback and screen. `jump` selects the first match at or
    /// below the viewport top and scrolls to it; otherwise the current match is kept.
    pub(crate) fn search_refresh(&mut self, jump: bool) {
        let s = &self.search;
        let res = search::find_all(&self.grid_full(), &s.query, s.case_sensitive, s.regex);
        let (top, _) = self.view_geometry();
        let s = &mut self.search;
        s.error = res.error;
        s.truncated = res.truncated;
        s.matches = res.matches;
        if jump {
            s.current = search::first_from(&s.matches, top);
            self.search_reveal();
        } else {
            s.current = s.current.min(s.matches.len().saturating_sub(1));
        }
    }

    /// Scrolls the current match into view when it is off screen.
    fn search_reveal(&mut self) {
        let Some(m) = self.search.matches.get(self.search.current) else {
            return;
        };
        let line = m.start().0;
        // A hit inside a collapsed `7721` region expands it.
        if self.live().is_some_and(|s| s.term.expand_fold_at_abs(line)) {
            if let Some(s) = self.live_mut() {
                s.refresh();
            }
        }
        let (top, rows) = self.view_geometry();
        if line < top || line >= top + rows {
            if let Some(s) = self.live() {
                s.term.scroll_to_abs(line, true);
            }
            if let Some(s) = self.live_mut() {
                s.refresh();
            }
        }
    }

    pub(crate) fn search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "enter" {
            self.step_search(!k.modifiers.shift, cx);
            return;
        } else if k.key == "backspace" {
            self.search.query.pop();
        } else if k.modifiers.alt && k.key == "c" {
            self.search.case_sensitive = !self.search.case_sensitive;
        } else if k.modifiers.alt && k.key == "r" {
            self.search.regex = !self.search.regex;
        } else if let Some(text) = typed_text(ev, self.platform) {
            self.search.query.push_str(text);
        } else {
            return;
        }
        self.search_refresh(true);
        cx.notify();
    }

    pub(crate) fn step_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        // Output may have arrived since the last keystroke.
        self.search_refresh(false);
        let n = self.search.matches.len();
        if n > 0 {
            let cur = self.search.current;
            self.search.current = if forward {
                (cur + 1) % n
            } else {
                (cur + n - 1) % n
            };
            self.search_reveal();
            cx.notify();
        }
    }

    pub(crate) fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        self.search.case_sensitive = !self.search.case_sensitive;
        self.search_refresh(true);
        cx.notify();
    }

    pub(crate) fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        self.search.regex = !self.search.regex;
        self.search_refresh(true);
        cx.notify();
    }

    /// Keeps the match list current while output arrives with the find bar open.
    pub(crate) fn search_follow_output(&mut self) {
        if self.overlays.contains(OverlayKind::Search) && !self.search.query.is_empty() {
            self.search_refresh(false);
        }
    }

    /// Search hits inside the viewport, in viewport rows.
    pub(crate) fn search_marks(&self) -> Vec<ViewMark> {
        let (top, rows) = self.view_geometry();
        let mut out = Vec::new();
        for (i, m) in self.search.matches.iter().enumerate() {
            let (first, last) = (m.segments[0].0, m.segments[m.segments.len() - 1].0);
            if last < top || first >= top + rows {
                continue;
            }
            for &(abs, col, len) in &m.segments {
                if abs >= top && abs < top + rows {
                    out.push((abs - top, col, len, i == self.search.current));
                }
            }
        }
        out
    }

    /// The find bar's `n of m` text.
    pub(crate) fn search_status(&self) -> String {
        let s = &self.search;
        if s.query.is_empty() {
            String::new()
        } else if s.error.is_some() {
            "invalid regex".into()
        } else if s.matches.is_empty() {
            "no results".into()
        } else {
            let plus = if s.truncated { "+" } else { "" };
            format!("{} of {}{plus}", s.current + 1, s.matches.len())
        }
    }

    // ------------------------------------------------------------------ copy mode

    pub(crate) fn prepare_copy_mode(&mut self, cx: &mut Context<Self>) {
        let overrides: Vec<(String, Option<String>)> = mt_config::keys::bindings(&self.cfg)
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.scope == mt_config::keys::Scope::CopyMode)
            .map(|b| {
                let k = &b.keystroke;
                (
                    copy_mode::notation(k.ctrl, k.shift, &k.key),
                    b.action.map(|a| a.name),
                )
            })
            .collect();
        let (keymap, warnings) = Keymap::with_overrides(&overrides);
        for w in warnings {
            self.toast(ToastKind::Warning, w, cx);
        }
        let trim = self.cfg.selection.trim_trailing_whitespace;
        self.copy = CopyState {
            mode: Some(CopyMode::new(self.cursor_abs(), trim)),
            keymap,
        };
    }

    pub(crate) fn teardown_copy_mode(&mut self) {
        self.copy.mode = None;
        if let Some(s) = self.live() {
            s.term.selection_clear();
        }
        if let Some(s) = self.live_mut() {
            s.refresh();
        }
    }

    /// Esc in copy mode: clears the selection first, then exits.
    pub(crate) fn copy_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_input("escape", window, cx);
    }

    pub(crate) fn copy_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(name) = key_name(ev, self.platform) {
            self.copy_input(&name, window, cx);
        }
    }

    fn copy_input(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut mode) = self.copy.mode.take() else {
            return;
        };
        let grid = self.grid_full();
        let marks = self
            .live()
            .map(|s| s.term.prompt_marks())
            .unwrap_or_default();
        let (top, rows) = self.view_geometry();
        let out = mode.key(name, &self.copy.keymap, &grid, &marks, View { top, rows });
        self.sync_copy_view(&mode);
        self.copy.mode = Some(mode);
        match out {
            Outcome::None => {}
            Outcome::Exit => self.close_overlay(OverlayKind::CopyMode, true, window, cx),
            Outcome::Yank { text, exit } => {
                let chars = text.chars().count();
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.toast(
                    ToastKind::Success,
                    if text.contains('\n') {
                        format!("Copied {chars} characters")
                    } else {
                        format!("Copied {}", preview(&text))
                    },
                    cx,
                );
                if exit {
                    self.close_overlay(OverlayKind::CopyMode, true, window, cx);
                }
            }
        }
        cx.notify();
    }

    /// Mirrors the copy-mode selection into the terminal (so it is highlighted like a mouse
    /// selection) and scrolls the cursor into view.
    fn sync_copy_view(&mut self, mode: &CopyMode) {
        let Some(s) = self.live() else { return };
        match mode.sel {
            Some((kind, anchor)) => s.term.select_abs(engine_kind(kind), anchor, mode.cursor),
            None => s.term.selection_clear(),
        }
        let (top, rows) = self.view_geometry();
        if mode.cursor.0 < top || mode.cursor.0 >= top + rows {
            s.term.scroll_to_abs(mode.cursor.0, false);
        }
        if let Some(s) = self.live_mut() {
            s.refresh();
        }
    }

    /// The copy-mode cursor in viewport `(row, col)`, when it is on screen.
    pub(crate) fn copy_cursor_view(&self) -> Option<(usize, usize)> {
        let mode = self.copy.mode.as_ref()?;
        let (top, rows) = self.view_geometry();
        let (abs, col) = mode.cursor;
        (abs >= top && abs < top + rows).then(|| (abs - top, col))
    }

    /// The text shown in the copy-mode pill and the `/` prompt line.
    pub(crate) fn copy_pill(&self) -> Option<(String, Option<String>)> {
        let mode = self.copy.mode.as_ref()?;
        let mut label = mode.mode_label().to_string();
        let pending = mode.pending_text();
        if !pending.is_empty() {
            label.push(' ');
            label.push_str(&pending);
        }
        let prompt = mode
            .prompt
            .as_ref()
            .map(|p| format!("{}{}", if p.forward { '/' } else { '?' }, p.text));
        Some((label, prompt))
    }
}
