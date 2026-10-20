//! Split panes inside a tab (SPEC §17.1): focus, split/close/zoom/resize actions, dividers and
//! focus-follows-mouse. The tree itself lives in `mt_mux`; this is the window's side of it.

use std::time::Duration;

use gpui::{Context, Window};
use mt_mux::{Axis, Dir, Rect, SpawnSpec};

use crate::app::{AppWindow, CloseTarget};
use crate::overlay_stack::OverlayKind;
use crate::{mux_host, settings::Settings};

/// A divider being dragged: which split it belongs to and the area that split covers
/// (fractions of the pane area).
#[derive(Debug, Clone, PartialEq)]
pub struct DividerDrag {
    pub tab: u64,
    pub path: Vec<bool>,
    pub axis: Axis,
    pub rect: Rect,
}

/// The ratio a split gets when its divider is dragged to `pos` (window pixels), given the pane
/// area `(x, y, w, h)` in pixels and the split's own `rect` as fractions of that area.
pub fn ratio_from_pointer(
    axis: Axis,
    rect: Rect,
    area: (f32, f32, f32, f32),
    pos: (f32, f32),
) -> f32 {
    let (ax, ay, aw, ah) = area;
    let (start, extent) = match axis {
        Axis::Horizontal => (ax + rect.x * aw, rect.w * aw),
        Axis::Vertical => (ay + rect.y * ah, rect.h * ah),
    };
    let p = match axis {
        Axis::Horizontal => pos.0,
        Axis::Vertical => pos.1,
    };
    if extent <= 0.0 {
        return 0.5;
    }
    mt_mux::tree::clamp_ratio((p - start) / extent)
}

/// How a pane is drawn: its fractional rect in the pane area, or `None` when a zoomed tab hides it.
pub fn visible_layout(tree: &mt_mux::PaneTree, zoomed: Option<u64>) -> Vec<(u64, Rect)> {
    let full = Rect::new(0.0, 0.0, 1.0, 1.0);
    match zoomed.filter(|z| tree.contains(*z)) {
        Some(z) => vec![(z, full)],
        None => tree.layout(full),
    }
}

impl AppWindow {
    /// The focused pane of the active tab.
    pub(crate) fn focused_pane(&self) -> Option<u64> {
        self.tabs.get(self.active_tab).map(|t| t.pane)
    }

    /// After focus moved inside the tab: mode state tied to the old pane is gone, the new pane
    /// no longer calls for attention, and keys go to it.
    pub(crate) fn after_pane_focus_change(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.entered_tab();
        self.clear_active_attention(cx);
        self.refocus_viewport = true;
        cx.notify();
    }

    /// Makes `pane` the focused pane of its tab.
    pub(crate) fn focus_pane(&mut self, pane: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused_pane() == Some(pane) {
            return;
        }
        self.leave_tab(cx);
        let _ = mux_host::host(cx).mux.focus_pane(pane);
        self.reconcile(window, cx);
        self.after_pane_focus_change(window, cx);
    }

    /// SplitHorizontal / SplitVertical: the new pane starts like a new tab would (its cwd per
    /// `new_tab_cwd`) in the same domain, and takes focus.
    pub(crate) fn split_focused(
        &mut self,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        self.leave_tab(cx);
        let cwd = self.new_tab_cwd(cx);
        let host = mux_host::host(cx);
        let domain = host
            .mux
            .pane_domain(pane)
            .unwrap_or(mt_mux::LOCAL_DOMAIN_ID);
        let spec = SpawnSpec {
            domain,
            cwd,
            ..Default::default()
        };
        host.split_pane(pane, axis, 0.5, spec);
        self.reconcile(window, cx);
        self.after_pane_focus_change(window, cx);
    }

    /// ClosePane: asks first when `confirm_close` says so; a tab's last pane closes the tab.
    pub(crate) fn request_close_focused_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let single = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|t| t.panes.len() <= 1);
        if single {
            let id = self.tabs[self.active_tab].id;
            self.request_close_tab(id, window, cx);
            return;
        }
        let busy = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.pane_view(pane))
            .and_then(|v| v.session.as_ref())
            .is_some_and(|s| s.term.foreground_process().is_some());
        if mt_mux::should_confirm_close(self.confirm_policy(), busy) {
            self.close_confirm = Some(CloseTarget::Pane(pane));
            self.open_overlay(OverlayKind::CloseConfirm, window, cx);
        } else {
            self.close_pane(pane, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn focus_direction(
        &mut self,
        dir: Dir,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        self.leave_tab(cx);
        let moved = mux_host::host(cx)
            .mux
            .focus_direction(pane, dir)
            .ok()
            .flatten()
            .is_some();
        if moved {
            self.reconcile(window, cx);
            self.after_pane_focus_change(window, cx);
        }
    }

    pub(crate) fn resize_focused(
        &mut self,
        dir: Dir,
        cells: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let size = self
            .live()
            .map(|s| s.size.get())
            .filter(|s| *s != (0, 0))
            .unwrap_or((120, 34));
        let moved = mux_host::host(cx)
            .mux
            .resize_pane_cells(pane, dir, cells, size)
            .unwrap_or(false);
        if moved {
            self.reconcile(window, cx);
        }
    }

    pub(crate) fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let _ = mux_host::host(cx).mux.toggle_zoom(pane);
        self.reconcile(window, cx);
    }

    pub(crate) fn swap_focused(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let _ = mux_host::host(cx).mux.swap_pane(pane, dir);
        self.reconcile(window, cx);
    }

    pub(crate) fn rotate_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let _ = mux_host::host(cx).mux.rotate_panes(pane);
        self.reconcile(window, cx);
    }

    /// The wheel over `pane`: the focused pane gets the full treatment (mouse reports, alternate
    /// scroll); another one just scrolls its history.
    pub(crate) fn on_scroll_wheel_pane(
        &mut self,
        pane: u64,
        ev: &gpui::ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        if self.focused_pane() == Some(pane) {
            self.on_scroll_wheel(ev, cx);
            return;
        }
        let cell_height = self.cell.height;
        let multiplier = self.cfg.mouse.scroll_multiplier;
        let Some(session) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|t| t.pane_view_mut(pane))
            .and_then(|v| v.session.as_mut())
        else {
            return;
        };
        let lines = match ev.delta {
            gpui::ScrollDelta::Lines(p) => p.y * multiplier,
            gpui::ScrollDelta::Pixels(p) => f32::from(p.y) / cell_height,
        };
        session.scroll_remainder += lines;
        let n = session.scroll_remainder.trunc();
        session.scroll_remainder -= n;
        if n != 0.0 && !session.snapshot.modes.alt_screen {
            session
                .term
                .scroll_display(mt_term::ScrollCmd::Lines(n as i32));
            session.refresh();
            cx.notify();
        }
    }

    /// The pointer is over `pane`. With `mouse.focus_follows_mouse` this focuses it after
    /// `panes.focus_delay_ms`, unless something else owns the pointer.
    pub(crate) fn hover_pane(&mut self, pane: u64, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = cx
            .try_global::<Settings>()
            .is_some_and(|s| s.config.mouse.focus_follows_mouse);
        let busy = !self.overlays.is_empty()
            || self.mouse.menu.is_some()
            || self.tab_menu.is_some()
            || self.mouse.drag.is_some()
            || self.divider_drag.is_some();
        if !enabled || busy || self.focused_pane() == Some(pane) {
            // Moving back onto the focused pane cancels a pending switch.
            self.focus_epoch += 1;
            return;
        }
        let delay = self.cfg.panes.focus_delay_ms;
        if delay == 0 {
            self.focus_pane(pane, window, cx);
            return;
        }
        self.focus_epoch += 1;
        let epoch = self.focus_epoch;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(u64::from(delay)))
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.focus_epoch == epoch {
                    this.focus_pane(pane, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Starts dragging the divider of the split at `path` in the active tab.
    pub(crate) fn begin_divider_drag(&mut self, path: Vec<bool>, axis: Axis, rect: Rect) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            self.divider_drag = Some(DividerDrag {
                tab: tab.id,
                path,
                axis,
                rect,
            });
        }
    }

    pub(crate) fn drag_divider_to(
        &mut self,
        pos: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.divider_drag.clone() else {
            return;
        };
        let ratio = ratio_from_pointer(drag.axis, drag.rect, self.pane_area.get(), pos);
        let _ = mux_host::host(cx)
            .mux
            .set_split_ratio(drag.tab, &drag.path, ratio);
        self.reconcile(window, cx);
    }

    pub(crate) fn end_divider_drag(&mut self, cx: &mut Context<Self>) {
        if self.divider_drag.take().is_some() {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_mux::{Pane, PaneTree, LOCAL_DOMAIN_ID};

    const AREA: (f32, f32, f32, f32) = (100.0, 50.0, 800.0, 400.0);

    #[test]
    fn dragging_maps_pointer_to_ratio_of_the_split() {
        // A root split: pointer at 40% of the width.
        let r = Rect::new(0.0, 0.0, 1.0, 1.0);
        assert!((ratio_from_pointer(Axis::Horizontal, r, AREA, (420.0, 0.0)) - 0.4).abs() < 1e-5);
        assert!((ratio_from_pointer(Axis::Vertical, r, AREA, (0.0, 250.0)) - 0.5).abs() < 1e-5);
        // A nested split covering the right half: its own ratio, not the window's.
        let right = Rect::new(0.5, 0.0, 0.5, 1.0);
        assert!(
            (ratio_from_pointer(Axis::Horizontal, right, AREA, (700.0, 0.0)) - 0.5).abs() < 1e-5
        );
    }

    #[test]
    fn dragging_past_the_edges_clamps() {
        let r = Rect::new(0.0, 0.0, 1.0, 1.0);
        assert_eq!(
            ratio_from_pointer(Axis::Horizontal, r, AREA, (0.0, 0.0)),
            mt_mux::tree::MIN_RATIO
        );
        assert_eq!(
            ratio_from_pointer(Axis::Horizontal, r, AREA, (5000.0, 0.0)),
            mt_mux::tree::MAX_RATIO
        );
        assert_eq!(
            ratio_from_pointer(Axis::Horizontal, r, (0.0, 0.0, 0.0, 0.0), (1.0, 1.0)),
            0.5
        );
    }

    fn tree() -> PaneTree {
        let p = |id| Pane::new(id, LOCAL_DOMAIN_ID, None);
        let mut t = PaneTree::Leaf(p(1));
        t.split_leaf(1, Axis::Horizontal, 0.5, p(2));
        t
    }

    #[test]
    fn zoom_draws_only_the_zoomed_pane_full_size() {
        let t = tree();
        assert_eq!(visible_layout(&t, None).len(), 2);
        assert_eq!(
            visible_layout(&t, Some(2)),
            vec![(2, Rect::new(0.0, 0.0, 1.0, 1.0))]
        );
        assert_eq!(
            visible_layout(&t, Some(99)).len(),
            2,
            "a stale zoom id is ignored"
        );
    }
}
