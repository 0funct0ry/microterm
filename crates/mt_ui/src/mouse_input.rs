//! Mouse handling for the terminal viewport (SPEC §10.2, §10.3): reporting to applications,
//! selection, the context menu, pointer shape and hyperlink activation.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, Context, CursorStyle, DispatchPhase, Entity, Hitbox, IntoElement,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollWheelEvent, Task, Window,
};
use mt_config::LinkModifier;
use mt_patterns::{is_allowed, scheme_of, LinkCache, LinkSet};
use mt_term::{
    CellFlags, MouseButton as TButton, MouseEvent, MouseEventKind, ScrollCmd, SelectionKind,
};

use crate::actions::ACTIONS;
use crate::app::AppWindow;
use crate::components::{ContextMenu, ContextMenuItem, ToastKind};
use crate::overlay_stack::OverlayKind;
use crate::platform::Platform;
use crate::settings::Settings;
use crate::theme::Theme;

/// A hyperlink under the pointer. Columns are `start..end` on `row`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverLink {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub target: String,
    /// The target differs from the visible text, so the tooltip is worth showing.
    pub show_target: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drag {
    Select,
    Report(TButton),
}

/// State of the viewport context menu.
#[derive(Clone, Debug)]
pub struct ViewportMenu {
    pub position: Point<Pixels>,
    pub link: Option<String>,
    pub has_selection: bool,
}

#[derive(Default)]
pub struct MouseState {
    pub drag: Option<Drag>,
    pub hover: Option<HoverLink>,
    pub pointer: Option<Point<Pixels>>,
    pub pointer_hidden: bool,
    pub mods: Modifiers,
    pub menu: Option<ViewportMenu>,
    pub link_cache: LinkCache,
    /// Last cell (and pixel, for 1016) reported, to drop duplicate motion reports.
    last_report: Option<(usize, usize, usize, usize)>,
    /// Last cell the drag selection was extended to.
    last_cell: (usize, usize),
    /// Lines per tick while dragging past the top (+) or bottom (-) edge.
    autoscroll: i32,
    autoscroll_task: Option<Task<()>>,
}

/// A pointer position resolved against the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub col: usize,
    pub row: usize,
    pub right: bool,
    pub px: usize,
    pub py: usize,
    /// Distance outside the grid in rows: negative above, positive below, 0 inside.
    pub overshoot: i32,
}

/// Maps a window position to a grid cell. Positions outside the grid are clamped and
/// reported through `overshoot`.
pub fn hit_test(
    pos: (f32, f32),
    origin: (f32, f32),
    cell: (f32, f32),
    grid: (usize, usize),
) -> Hit {
    let (cols, rows) = (grid.0.max(1), grid.1.max(1));
    let fx = pos.0 - origin.0;
    let fy = pos.1 - origin.1;
    let max_x = cols as f32 * cell.0 - 1.0;
    let max_y = rows as f32 * cell.1 - 1.0;
    let cx = fx.clamp(0.0, max_x.max(0.0));
    let cy = fy.clamp(0.0, max_y.max(0.0));
    let col = ((cx / cell.0) as usize).min(cols - 1);
    let row = ((cy / cell.1) as usize).min(rows - 1);
    let overshoot = if fy < 0.0 {
        -(((-fy) / cell.1).ceil() as i32)
    } else if fy > max_y + 1.0 {
        ((fy - max_y - 1.0) / cell.1).ceil() as i32
    } else {
        0
    };
    Hit {
        col,
        row,
        right: (cx - col as f32 * cell.0) > cell.0 / 2.0,
        px: cx as usize,
        py: cy as usize,
        overshoot,
    }
}

/// Whether `mods` is the configured link-activation modifier.
pub fn link_modifier_held(mods: &Modifiers, modifier: LinkModifier, platform: Platform) -> bool {
    match modifier {
        LinkModifier::None => true,
        LinkModifier::Ctrl => mods.control,
        LinkModifier::Cmd => match platform {
            Platform::Mac => mods.platform,
            _ => mods.control,
        },
    }
}

/// Compiles built-in plus configured hyperlink rules. A rule that fails to compile (config
/// diagnostics normally reject it first) falls back to the built-ins alone.
pub fn build_links(cfg: &mt_config::Config) -> Arc<LinkSet> {
    let rules: Vec<(String, String)> = cfg
        .hyperlink_rules
        .iter()
        .map(|r| (r.regex.clone(), r.format.clone()))
        .collect();
    Arc::new(LinkSet::new(&rules).unwrap_or_else(|_| LinkSet::new(&[]).expect("built-ins")))
}

fn button_of(b: MouseButton) -> Option<TButton> {
    match b {
        MouseButton::Left => Some(TButton::Left),
        MouseButton::Middle => Some(TButton::Middle),
        MouseButton::Right => Some(TButton::Right),
        _ => None,
    }
}

impl AppWindow {
    fn grid_hit(&self, pos: Point<Pixels>) -> Option<Hit> {
        let s = self.live()?;
        Some(hit_test(
            (f32::from(pos.x), f32::from(pos.y)),
            s.grid_origin.get(),
            (self.cell.width, self.cell.height),
            (s.snapshot.cols, s.snapshot.rows),
        ))
    }

    fn report_event(
        &self,
        kind: MouseEventKind,
        button: TButton,
        mods: &Modifiers,
        h: &Hit,
    ) -> MouseEvent {
        let mut ev = MouseEvent::new(kind, button, h.col, h.row);
        ev.shift = mods.shift;
        ev.alt = mods.alt;
        ev.ctrl = mods.control;
        ev.px = h.px;
        ev.py = h.py;
        ev
    }

    /// Mouse reporting is on for the pane and Shift is not held to bypass it.
    fn reporting(&self, mods: &Modifiers) -> bool {
        !mods.shift
            && self
                .live()
                .is_some_and(|s| s.snapshot.modes.mouse_tracking != mt_term::MouseTracking::None)
    }

    fn link_modifier(&self, mods: &Modifiers) -> bool {
        link_modifier_held(mods, self.cfg.hyperlinks.modifier, self.platform)
    }

    pub(crate) fn pointer_style(&self) -> CursorStyle {
        if self.mouse.pointer_hidden {
            return CursorStyle::None;
        }
        if self.mouse.hover.is_some() && self.link_modifier(&self.mouse.mods) {
            return CursorStyle::PointingHand;
        }
        if self.reporting(&self.mouse.mods) {
            return CursorStyle::Arrow;
        }
        CursorStyle::IBeam
    }

    /// The hyperlink at a cell: an explicit OSC 8 link, else an implicit rule match on the
    /// row text (cached by row hash).
    pub(crate) fn link_at(&mut self, row: usize, col: usize) -> Option<HoverLink> {
        let snap = self.live()?.snapshot.clone();
        if row >= snap.rows || col >= snap.cols {
            return None;
        }
        let cells = snap.row(row);
        let col = if col > 0 && cells[col].flags.has(CellFlags::WIDE_SPACER) {
            col - 1
        } else {
            col
        };
        if let Some(l) = &cells[col].link {
            let same = |c: &mt_term::Cell| {
                c.link
                    .as_ref()
                    .is_some_and(|x| x.id == l.id && x.uri == l.uri)
            };
            let (mut s, mut e) = (col, col);
            while s > 0 && same(&cells[s - 1]) {
                s -= 1;
            }
            while e + 1 < cells.len() && same(&cells[e + 1]) {
                e += 1;
            }
            return Some(HoverLink {
                row,
                start: s,
                end: e + 1,
                target: l.uri.clone(),
                show_target: true,
            });
        }
        let mut text = String::new();
        let mut col_of: Vec<usize> = Vec::new();
        for (c, cell) in cells.iter().enumerate() {
            if cell.flags.has(CellFlags::WIDE_SPACER) {
                continue;
            }
            text.push(if cell.ch == '\0' { ' ' } else { cell.ch });
            col_of.push(c);
        }
        let idx = col_of.iter().position(|&c| c == col)?;
        let links = self.links.clone();
        let matches = self
            .mouse
            .link_cache
            .get_or_scan(snap.row_hashes[row], &text, &links);
        let m = matches.iter().find(|m| m.range.contains(&idx))?;
        let start = col_of[m.range.start];
        let last = col_of[m.range.end - 1];
        let end = last
            + if cells[last].flags.has(CellFlags::WIDE) {
                2
            } else {
                1
            };
        let visible: String = text
            .chars()
            .skip(m.range.start)
            .take(m.range.len())
            .collect();
        Some(HoverLink {
            row,
            start,
            end,
            show_target: visible != m.target,
            target: m.target.clone(),
        })
    }

    pub(crate) fn clear_hover(&mut self) -> bool {
        self.mouse.hover.take().is_some()
    }

    // ------------------------------------------------------------------ pointer events

    pub(crate) fn mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.overlays.is_empty() || self.mouse.menu.is_some() || self.tab_menu.is_some() {
            return;
        }
        let Some(hit) = self.grid_hit(ev.position) else {
            return;
        };
        self.mouse.pointer = Some(ev.position);
        self.mouse.pointer_hidden = false;
        self.mouse.mods = ev.modifiers;
        let Some(button) = button_of(ev.button) else {
            return;
        };
        if self.reporting(&ev.modifiers) {
            if let Some(s) = self.live() {
                s.term.send_mouse(&self.report_event(
                    MouseEventKind::Press,
                    button,
                    &ev.modifiers,
                    &hit,
                ));
            }
            self.mouse.drag = Some(Drag::Report(button));
            self.mouse.last_report = Some((hit.col, hit.row, hit.px, hit.py));
            cx.notify();
            return;
        }
        match ev.button {
            MouseButton::Left => self.left_down(ev, &hit, cx),
            MouseButton::Middle => self.paste_primary(window, cx),
            MouseButton::Right => match self.cfg.mouse.right_click {
                mt_config::RightClick::ContextMenu => {
                    let link = self.link_at(hit.row, hit.col).map(|l| l.target);
                    let has_selection = self.live().is_some_and(|s| s.term.has_selection());
                    self.mouse.menu = Some(ViewportMenu {
                        position: ev.position,
                        link,
                        has_selection,
                    });
                    cx.notify();
                }
                mt_config::RightClick::Paste => self.paste(window, cx),
                mt_config::RightClick::None => {}
            },
            _ => {}
        }
    }

    fn left_down(&mut self, ev: &MouseDownEvent, hit: &Hit, cx: &mut Context<Self>) {
        if self.link_modifier(&ev.modifiers) {
            if let Some(link) = self.link_at(hit.row, hit.col) {
                self.open_link(&link.target, cx);
                return;
            }
        }
        let word_chars = self.cfg.selection.word_chars.clone();
        let Some(session) = self.live() else { return };
        let term = session.term.clone();
        if ev.modifiers.shift && term.has_selection() && ev.click_count <= 1 {
            term.selection_update(hit.row, hit.col, hit.right);
        } else {
            let kind = match ev.click_count {
                0 | 1 if ev.modifiers.alt => SelectionKind::Block,
                0 | 1 => SelectionKind::Simple,
                2 => SelectionKind::Word,
                _ => SelectionKind::Line,
            };
            term.selection_start(kind, hit.row, hit.col, hit.right, &word_chars);
        }
        self.mouse.drag = Some(Drag::Select);
        self.mouse.last_cell = (hit.row, hit.col);
        self.refresh_session();
        cx.notify();
    }

    /// Installs the window-level pointer listeners for this frame. They run in the bubble
    /// phase; presses need the viewport to be the topmost hitbox, while moves and releases
    /// are delivered everywhere so a drag keeps working outside the grid.
    pub(crate) fn register_mouse(entity: &Entity<Self>, hitbox: Hitbox, window: &mut Window) {
        let (e, hb) = (entity.clone(), hitbox.clone());
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                e.update(cx, |this, cx| this.mouse_down(ev, window, cx));
            }
        });
        let (e, hb) = (entity.clone(), hitbox);
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = hb.is_hovered(window);
                e.update(cx, |this, cx| this.mouse_move(ev, hovered, cx));
            }
        });
        let e = entity.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                e.update(cx, |this, cx| this.mouse_up(ev, cx));
            }
        });
    }

    pub(crate) fn mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if !hovered && self.mouse.drag.is_none() {
            if self.clear_hover() {
                cx.notify();
            }
            return;
        }
        let was_hidden = std::mem::replace(&mut self.mouse.pointer_hidden, false);
        self.mouse.mods = ev.modifiers;
        let moved = self.mouse.pointer != Some(ev.position);
        self.mouse.pointer = Some(ev.position);
        let Some(hit) = self.grid_hit(ev.position) else {
            return;
        };
        match self.mouse.drag {
            Some(Drag::Report(button)) => {
                self.report_motion(button, &ev.modifiers, &hit);
            }
            Some(Drag::Select) => {
                self.extend_selection(&hit, cx);
            }
            None => {
                if self.reporting(&ev.modifiers) {
                    self.report_motion(TButton::Unpressed, &ev.modifiers, &hit);
                    if self.clear_hover() || was_hidden {
                        cx.notify();
                    }
                    return;
                }
                if !self.overlays.is_empty() || self.mouse.menu.is_some() || self.tab_menu.is_some()
                {
                    return;
                }
                let inside = hit.overshoot == 0;
                let link = if inside {
                    self.link_at(hit.row, hit.col)
                } else {
                    None
                };
                if link != self.mouse.hover || was_hidden || moved && link.is_some() {
                    self.mouse.hover = link;
                    cx.notify();
                }
            }
        }
    }

    fn report_motion(&mut self, button: TButton, mods: &Modifiers, hit: &Hit) {
        let pixels = self.live().is_some_and(|s| s.snapshot.modes.sgr_pixels);
        let key = (
            hit.col,
            hit.row,
            if pixels { hit.px } else { 0 },
            if pixels { hit.py } else { 0 },
        );
        if self.mouse.last_report == Some(key) {
            return;
        }
        self.mouse.last_report = Some(key);
        if let Some(s) = self.live() {
            s.term
                .send_mouse(&self.report_event(MouseEventKind::Motion, button, mods, hit));
        }
    }

    fn extend_selection(&mut self, hit: &Hit, cx: &mut Context<Self>) {
        self.mouse.last_cell = (hit.row, hit.col);
        self.mouse.autoscroll = (-hit.overshoot).clamp(-3, 3);
        if let Some(s) = self.live() {
            s.term.selection_update(hit.row, hit.col, hit.right);
        }
        self.refresh_session();
        if self.mouse.autoscroll != 0 && self.mouse.autoscroll_task.is_none() {
            self.mouse.autoscroll_task = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let keep = this
                    .update(cx, |this, cx| this.autoscroll_tick(cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }));
        } else if self.mouse.autoscroll == 0 {
            self.mouse.autoscroll_task = None;
        }
        cx.notify();
    }

    fn autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.mouse.drag != Some(Drag::Select) || self.mouse.autoscroll == 0 {
            self.mouse.autoscroll_task = None;
            return false;
        }
        let lines = self.mouse.autoscroll;
        let (row, col) = self.mouse.last_cell;
        if let Some(s) = self.live() {
            s.term.scroll_display(ScrollCmd::Lines(lines));
            s.term.selection_update(row, col, lines < 0);
        }
        self.refresh_session();
        self.on_scroll(cx);
        cx.notify();
        true
    }

    pub(crate) fn mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.mouse.drag.take() else {
            return;
        };
        self.mouse.autoscroll = 0;
        self.mouse.autoscroll_task = None;
        self.mouse.last_report = None;
        match drag {
            Drag::Report(button) => {
                if let Some(hit) = self.grid_hit(ev.position) {
                    if let Some(s) = self.live() {
                        s.term.send_mouse(&self.report_event(
                            MouseEventKind::Release,
                            button,
                            &ev.modifiers,
                            &hit,
                        ));
                    }
                }
            }
            Drag::Select => self.finish_selection(cx),
        }
        cx.notify();
    }

    fn finish_selection(&mut self, cx: &mut Context<Self>) {
        let trim = self.cfg.selection.trim_trailing_whitespace;
        let Some(text) = self.live().and_then(|s| s.term.selection_text(trim)) else {
            return;
        };
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        cx.write_to_primary(gpui::ClipboardItem::new_string(text.clone()));
        if self.cfg.selection.copy_on_select {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn mouse_wheel_report(&mut self, ev: &ScrollWheelEvent, lines: i32) -> bool {
        if lines == 0 || !self.reporting(&ev.modifiers) {
            return false;
        }
        let Some(hit) = self.grid_hit(ev.position) else {
            return false;
        };
        let button = if lines > 0 {
            TButton::WheelUp
        } else {
            TButton::WheelDown
        };
        if let Some(s) = self.live() {
            let e = self.report_event(MouseEventKind::Press, button, &ev.modifiers, &hit);
            for _ in 0..lines.unsigned_abs() {
                s.term.send_mouse(&e);
            }
        }
        true
    }

    /// The viewport context menu (SPEC §10.2). Entry order matches `viewport_menu_select`.
    pub(crate) fn viewport_menu(
        &self,
        menu: ViewportMenu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let chip = |name: &str| {
            ACTIONS
                .iter()
                .find(|a| a.name == name)
                .and_then(|a| cx.global::<Settings>().chip(a))
                .unwrap_or_default()
        };
        let has_link = menu.link.is_some();
        ContextMenu::new("viewport-menu", menu.position)
            .items(vec![
                ContextMenuItem::new("Copy")
                    .chip(chip("Copy"))
                    .enabled(menu.has_selection),
                ContextMenuItem::new("Paste").chip(chip("Paste")),
                ContextMenuItem::new("Select all").chip(chip("SelectAll")),
                ContextMenuItem::new("Open link").enabled(has_link),
                ContextMenuItem::new("Copy link").enabled(has_link),
                ContextMenuItem::new("Search for selection").enabled(menu.has_selection),
                ContextMenuItem::new("Clear scrollback").chip(chip("ClearScrollback")),
            ])
            .on_dismiss({
                let entity = cx.entity();
                move |_, app| {
                    entity.update(app, |this, cx| {
                        this.mouse.menu = None;
                        cx.notify();
                    });
                }
            })
            .on_select({
                let entity = cx.entity();
                move |i, window, app| {
                    entity.update(app, |this, cx| {
                        this.mouse.menu = None;
                        this.viewport_menu_select(i, &menu, window, cx);
                    });
                }
            })
    }

    /// Tooltip showing where a hovered link really goes (SPEC §6.3).
    pub(crate) fn link_tooltip(&self, t: &Theme) -> Option<impl IntoElement> {
        let link = self.mouse.hover.as_ref().filter(|l| l.show_target)?;
        let at = self.mouse.pointer?;
        Some(
            div()
                .absolute()
                .left(at.x + px(12.))
                .top(at.y + px(20.))
                .max_w(px(520.))
                .px_2()
                .py_1()
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .bg(t.elevated_surface)
                .text_color(t.text)
                .text_size(px(12.))
                .overflow_hidden()
                .child(link.target.clone()),
        )
    }

    fn refresh_session(&mut self) {
        if let Some(s) = self.live_mut() {
            s.refresh();
        }
    }

    // ------------------------------------------------------------------ clipboard / links

    /// Copies the selection to the clipboard. Returns whether there was one.
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let trim = self.cfg.selection.trim_trailing_whitespace;
        match self.live().and_then(|s| s.term.selection_text(trim)) {
            Some(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    pub(crate) fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.live() {
            s.term.select_all();
        }
        self.refresh_session();
        cx.notify();
    }

    /// Linux: pastes the primary selection. Elsewhere there is no such buffer.
    pub(crate) fn paste_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(text) = cx.read_from_primary().and_then(|i| i.text()) {
            self.paste_text(text, window, cx);
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        {
            let _ = window;
            self.toast(
                ToastKind::Info,
                "The primary selection exists on Linux only",
                cx,
            );
        }
    }

    /// Opens `target` with the system opener (never a shell) if its scheme is allowed.
    pub(crate) fn open_link(&mut self, target: &str, cx: &mut Context<Self>) {
        if target.chars().any(|c| c.is_control()) {
            self.toast(
                ToastKind::Warning,
                "Refused to open a link with control characters",
                cx,
            );
            return;
        }
        if !is_allowed(target, &self.cfg.hyperlinks.schemes) {
            let scheme = scheme_of(target).unwrap_or("(none)").to_string();
            self.toast(
                ToastKind::Warning,
                format!("Blocked link scheme \"{scheme}\" (allow it in hyperlinks.schemes)"),
                cx,
            );
            return;
        }
        cx.open_url(target);
    }

    pub(crate) fn copy_link(&mut self, target: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(target.to_string()));
        self.toast(ToastKind::Success, "Copied link", cx);
    }

    pub(crate) fn search_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let trim = self.cfg.selection.trim_trailing_whitespace;
        let Some(text) = self.live().and_then(|s| s.term.selection_text(trim)) else {
            return;
        };
        self.search.query = text.lines().next().unwrap_or("").to_string();
        self.open_overlay(OverlayKind::Search, window, cx);
    }

    /// Runs a context-menu entry. Indices follow `view::viewport_menu`.
    pub(crate) fn viewport_menu_select(
        &mut self,
        index: usize,
        menu: &ViewportMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match index {
            0 => {
                self.copy_selection(cx);
            }
            1 => self.paste(window, cx),
            2 => self.select_all(cx),
            3 => {
                if let Some(l) = &menu.link {
                    self.open_link(&l.clone(), cx);
                }
            }
            4 => {
                if let Some(l) = &menu.link {
                    self.copy_link(&l.clone(), cx);
                }
            }
            5 => self.search_selection(window, cx),
            _ => self.clear_scrollback(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const O: (f32, f32) = (10.0, 20.0);
    const C: (f32, f32) = (8.0, 16.0);

    #[test]
    fn hit_inside() {
        let h = hit_test(
            (10.0 + 8.0 * 3.0 + 1.0, 20.0 + 16.0 * 2.0 + 1.0),
            O,
            C,
            (80, 24),
        );
        assert_eq!((h.col, h.row, h.right, h.overshoot), (3, 2, false, 0));
        let h = hit_test((10.0 + 8.0 * 3.0 + 6.0, 20.0), O, C, (80, 24));
        assert!(h.right);
    }

    #[test]
    fn hit_clamps_and_reports_overshoot() {
        let h = hit_test((0.0, 20.0 - 40.0), O, C, (80, 24));
        assert_eq!((h.col, h.row), (0, 0));
        assert!(h.overshoot < 0);
        let h = hit_test((10_000.0, 20.0 + 16.0 * 24.0 + 40.0), O, C, (80, 24));
        assert_eq!((h.col, h.row), (79, 23));
        assert!(h.overshoot > 0);
    }

    #[test]
    fn pixel_coordinates_are_grid_relative() {
        let h = hit_test((10.0 + 17.0, 20.0 + 35.0), O, C, (80, 24));
        assert_eq!((h.px, h.py), (17, 35));
    }

    #[test]
    fn link_modifier_per_platform() {
        let mut m = Modifiers::default();
        assert!(!link_modifier_held(&m, LinkModifier::Cmd, Platform::Mac));
        m.platform = true;
        assert!(link_modifier_held(&m, LinkModifier::Cmd, Platform::Mac));
        assert!(!link_modifier_held(&m, LinkModifier::Cmd, Platform::Linux));
        m = Modifiers::default();
        m.control = true;
        assert!(link_modifier_held(&m, LinkModifier::Cmd, Platform::Linux));
        assert!(!link_modifier_held(&m, LinkModifier::Cmd, Platform::Mac));
        assert!(link_modifier_held(
            &Modifiers::default(),
            LinkModifier::None,
            Platform::Mac
        ));
    }
}
