//! Split, close, focus, resize, zoom, swap and rotate on a tab's pane tree (SPEC §17.1).

use std::path::PathBuf;

use super::*;
use crate::tree::{clamp_ratio, Dir, Placement, Rect};

/// What [`Mux::close_pane`] removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneClosed {
    /// One pane left a split tab; the tab lives on.
    Pane { window: Id, tab: Id },
    /// It was the tab's last pane, so the tab (and maybe its window) closed.
    Tab {
        window: Id,
        tab: Id,
        window_closed: bool,
    },
}

/// Geometry used for directional focus and swaps. Only the shape of the tree matters.
const UNIT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
};

impl Mux {
    fn tab_at(&self, pane: Id) -> Result<(Id, usize), Error> {
        self.locate_pane(pane).ok_or(Error::NoPane(pane))
    }

    fn tab_mut_of(&mut self, pane: Id) -> Result<(Id, &mut Tab), Error> {
        let (window, index) = self.tab_at(pane)?;
        Ok((window, &mut self.window_mut(window)?.tabs[index]))
    }

    /// Splits `pane` along `axis`; the new pane takes `new_share` (0..1) of the space, goes
    /// second, and gets focus. It inherits the domain and cwd of `pane` unless overridden.
    /// Splitting un-zooms the tab. Returns the new pane's id.
    pub fn split_pane(
        &mut self,
        pane: Id,
        axis: Axis,
        new_share: f32,
        domain: Option<DomainId>,
        cwd: Option<PathBuf>,
    ) -> Result<Id, Error> {
        self.split_pane_placed(pane, axis, Placement::After, new_share, domain, cwd)
    }

    /// Like [`Mux::split_pane`], with the new pane on the `dir` side of `pane`: `Left`/`Up`
    /// put it first (SPEC §17.1, M11.1). It still takes `new_share` and focus.
    pub fn split_pane_toward(
        &mut self,
        pane: Id,
        dir: Dir,
        new_share: f32,
        domain: Option<DomainId>,
        cwd: Option<PathBuf>,
    ) -> Result<Id, Error> {
        self.split_pane_placed(pane, dir.axis(), dir.placement(), new_share, domain, cwd)
    }

    fn split_pane_placed(
        &mut self,
        pane: Id,
        axis: Axis,
        placement: Placement,
        new_share: f32,
        domain: Option<DomainId>,
        cwd: Option<PathBuf>,
    ) -> Result<Id, Error> {
        let (window, index) = self.tab_at(pane)?;
        let (parent_domain, parent_cwd) = {
            let p = self.pane(pane).ok_or(Error::NoPane(pane))?;
            (p.domain_id, p.cwd.clone())
        };
        let domain = domain.unwrap_or(parent_domain);
        self.domain_state(domain).ok_or(Error::NoDomain(domain))?;
        let new_id = self.alloc();
        let new = Pane::new(new_id, domain, cwd.or(parent_cwd));
        let tab = &mut self.window_mut(window)?.tabs[index];
        let tab_id = tab.id;
        let ratio = match placement {
            Placement::Before => clamp_ratio(new_share),
            Placement::After => clamp_ratio(1.0 - new_share),
        };
        tab.tree.split_leaf_at(pane, axis, ratio, new, placement);
        tab.zoomed = None;
        tab.active_pane = new_id;
        self.window_mut(window)?.active = index;
        self.emit(MuxEvent::PaneAdded { pane: new_id });
        self.emit(MuxEvent::LayoutChanged { tab: tab_id });
        self.emit(MuxEvent::PaneFocused { pane: new_id });
        Ok(new_id)
    }

    /// Closes `pane`. The last pane of a tab closes the tab, and the last tab its window.
    pub fn close_pane(&mut self, pane: Id) -> Result<PaneClosed, Error> {
        let (window, index) = self.tab_at(pane)?;
        let tab = &mut self.window_mut(window)?.tabs[index];
        let tab_id = tab.id;
        if tab.tree.leaf_count() == 1 {
            let empty = self.close_tab(window, tab_id)?;
            if empty {
                self.close_window(window)?;
            }
            return Ok(PaneClosed::Tab {
                window,
                tab: tab_id,
                window_closed: empty,
            });
        }
        let order: Vec<Id> = tab.tree.panes().iter().map(|p| p.id).collect();
        let at = order.iter().position(|&p| p == pane).unwrap_or(0);
        tab.tree.remove_leaf(pane);
        let was_active = tab.active_pane == pane;
        if was_active {
            // Focus passes to the pane that preceded it, else the one that followed.
            tab.active_pane = if at > 0 { order[at - 1] } else { order[1] };
        }
        if tab.zoomed == Some(pane) {
            tab.zoomed = None;
        }
        let focus = tab.active_pane;
        self.emit(MuxEvent::PaneRemoved { pane });
        self.emit(MuxEvent::LayoutChanged { tab: tab_id });
        if was_active {
            self.emit(MuxEvent::PaneFocused { pane: focus });
        }
        Ok(PaneClosed::Pane {
            window,
            tab: tab_id,
        })
    }

    /// Focuses `pane`, activating its tab. Focusing another pane leaves zoom.
    pub fn focus_pane(&mut self, pane: Id) -> Result<(), Error> {
        let (window, index) = self.tab_at(pane)?;
        self.window_mut(window)?.active = index;
        let tab = &mut self.window_mut(window)?.tabs[index];
        let tab_id = tab.id;
        let changed = tab.active_pane != pane;
        let unzoomed = tab.zoomed.is_some() && tab.zoomed != Some(pane);
        tab.active_pane = pane;
        if unzoomed {
            tab.zoomed = None;
        }
        if unzoomed {
            self.emit(MuxEvent::LayoutChanged { tab: tab_id });
        }
        if changed {
            self.emit(MuxEvent::PaneFocused { pane });
        }
        Ok(())
    }

    /// Focuses the neighbour of `from` in `dir`. `Ok(None)` when there is none (focus stays).
    pub fn focus_direction(&mut self, from: Id, dir: Dir) -> Result<Option<Id>, Error> {
        let (window, index) = self.tab_at(from)?;
        let next = self.window(window).ok_or(Error::NoWindow(window))?.tabs[index]
            .tree
            .neighbour(from, dir, UNIT);
        if let Some(n) = next {
            self.focus_pane(n)?;
        }
        Ok(next)
    }

    /// Moves `pane`'s border in `dir` by `delta`, a fraction of the whole tab.
    /// Returns whether a border moved.
    pub fn resize_pane(&mut self, pane: Id, dir: Dir, delta: f32) -> Result<bool, Error> {
        let (_, tab) = self.tab_mut_of(pane)?;
        let tab_id = tab.id;
        let moved = tab.tree.resize(pane, dir, delta, UNIT);
        if moved {
            self.emit(MuxEvent::LayoutChanged { tab: tab_id });
        }
        Ok(moved)
    }

    /// Like [`Mux::resize_pane`], in terminal cells. `pane_cells` is the pane's current size
    /// `(cols, rows)`, from which the tab's total size is derived.
    pub fn resize_pane_cells(
        &mut self,
        pane: Id,
        dir: Dir,
        cells: u32,
        pane_cells: (u16, u16),
    ) -> Result<bool, Error> {
        let (_, tab) = self.tab_mut_of(pane)?;
        let rect = tab
            .tree
            .layout(UNIT)
            .into_iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, r)| r)
            .ok_or(Error::NoPane(pane))?;
        let (own, share) = match dir.axis() {
            Axis::Horizontal => (pane_cells.0, rect.w),
            Axis::Vertical => (pane_cells.1, rect.h),
        };
        if own == 0 || share <= 0.0 {
            return Ok(false);
        }
        let total = own as f32 / share;
        self.resize_pane(pane, dir, cells as f32 / total)
    }

    /// Sets one split's ratio by tree path (divider drag).
    pub fn set_split_ratio(&mut self, tab: Id, path: &[bool], ratio: f32) -> Result<(), Error> {
        let (window, index) = self.tab_location(tab).ok_or(Error::NoTab(tab))?;
        let t = &mut self.window_mut(window)?.tabs[index];
        if t.tree.set_ratio_at(path, ratio) {
            self.emit(MuxEvent::LayoutChanged { tab });
        }
        Ok(())
    }

    /// Zooms `pane` to fill its tab, or restores the layout. A single-pane tab never zooms.
    /// Returns whether the tab is zoomed afterwards.
    pub fn toggle_zoom(&mut self, pane: Id) -> Result<bool, Error> {
        let (_, tab) = self.tab_mut_of(pane)?;
        let tab_id = tab.id;
        if tab.tree.leaf_count() < 2 {
            return Ok(false);
        }
        let now = if tab.zoomed.is_some() {
            tab.zoomed = None;
            false
        } else {
            tab.zoomed = Some(pane);
            true
        };
        let changed_focus = tab.active_pane != pane;
        tab.active_pane = pane;
        self.emit(MuxEvent::LayoutChanged { tab: tab_id });
        if changed_focus {
            self.emit(MuxEvent::PaneFocused { pane });
        }
        Ok(now)
    }

    /// Swaps `pane` with its neighbour in `dir`; focus stays on `pane`.
    pub fn swap_pane(&mut self, pane: Id, dir: Dir) -> Result<bool, Error> {
        let (_, tab) = self.tab_mut_of(pane)?;
        let tab_id = tab.id;
        let Some(other) = tab.tree.neighbour(pane, dir, UNIT) else {
            return Ok(false);
        };
        tab.tree.swap_leaves(pane, other);
        self.emit(MuxEvent::LayoutChanged { tab: tab_id });
        Ok(true)
    }

    /// Rotates the positions of the tab's panes by one. Focus stays on the same pane.
    pub fn rotate_panes(&mut self, pane: Id) -> Result<(), Error> {
        let (_, tab) = self.tab_mut_of(pane)?;
        let tab_id = tab.id;
        if tab.tree.leaf_count() > 1 {
            tab.tree.rotate();
            self.emit(MuxEvent::LayoutChanged { tab: tab_id });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn setup() -> (Mux, Id, Id, Id) {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let (tab, pane) = mux.spawn_tab(w, None, None).unwrap();
        (mux, w, tab, pane)
    }

    fn tab_of(mux: &Mux, w: Id, tab: Id) -> Tab {
        mux.window(w)
            .unwrap()
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .unwrap()
            .clone()
    }

    fn order(t: &Tab) -> Vec<Id> {
        t.tree.panes().iter().map(|p| p.id).collect()
    }

    fn log(mux: &mut Mux) -> Arc<Mutex<Vec<MuxEvent>>> {
        let l = Arc::new(Mutex::new(Vec::new()));
        let l2 = l.clone();
        mux.subscribe(move |e| l2.lock().unwrap().push(e.clone()));
        l
    }

    /// Three panes: 1 | (2 over 3). Returns ids in leaf order.
    fn three() -> (Mux, Id, Id, [Id; 3]) {
        let (mut m, w, tab, p1) = setup();
        let p2 = m.split_pane(p1, Axis::Horizontal, 0.5, None, None).unwrap();
        let p3 = m.split_pane(p2, Axis::Vertical, 0.5, None, None).unwrap();
        (m, w, tab, [p1, p2, p3])
    }

    #[test]
    fn split_adds_a_focused_pane_and_emits_in_order() {
        let (mut m, w, tab, p1) = setup();
        let events = log(&mut m);
        let p2 = m.split_pane(p1, Axis::Horizontal, 0.3, None, None).unwrap();
        let t = tab_of(&m, w, tab);
        assert_eq!(order(&t), vec![p1, p2]);
        assert_eq!(t.active_pane, p2);
        assert_eq!(t.pane().id, p2);
        let PaneTree::Split { ratio, .. } = t.tree else {
            panic!()
        };
        assert!((ratio - 0.7).abs() < 1e-5, "the new pane takes its share");
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                MuxEvent::PaneAdded { pane: p2 },
                MuxEvent::LayoutChanged { tab },
                MuxEvent::PaneFocused { pane: p2 },
            ]
        );
    }

    #[test]
    fn split_inherits_cwd_and_ids_stay_monotonic() {
        let (mut m, w, tab, p1) = setup();
        m.set_pane_cwd(p1, "/srv".into());
        let p2 = m.split_pane(p1, Axis::Vertical, 0.5, None, None).unwrap();
        assert!(p2 > p1);
        assert_eq!(
            m.pane(p2).unwrap().cwd.as_deref(),
            Some(std::path::Path::new("/srv"))
        );
        let p3 = m
            .split_pane(p2, Axis::Vertical, 0.5, None, Some("/x".into()))
            .unwrap();
        assert_eq!(
            m.pane(p3).unwrap().cwd.as_deref(),
            Some(std::path::Path::new("/x"))
        );
        assert_eq!(order(&tab_of(&m, w, tab)).len(), 3);
        assert!(matches!(
            m.split_pane(999, Axis::Vertical, 0.5, None, None),
            Err(Error::NoPane(999))
        ));
        assert!(matches!(
            m.split_pane(p1, Axis::Vertical, 0.5, Some(77), None),
            Err(Error::NoDomain(77))
        ));
    }

    #[test]
    fn title_and_cwd_reach_non_first_panes() {
        let (mut m, w, tab, [_, _, p3]) = three();
        m.set_pane_title(p3, "deep");
        assert_eq!(tab_of(&m, w, tab).tree.find(p3).unwrap().title, "deep");
    }

    #[test]
    fn close_pane_promotes_sibling_and_moves_focus() {
        let (mut m, w, tab, [p1, p2, p3]) = three();
        let events = log(&mut m);
        assert_eq!(
            m.close_pane(p3).unwrap(),
            PaneClosed::Pane { window: w, tab }
        );
        let t = tab_of(&m, w, tab);
        assert_eq!(order(&t), vec![p1, p2]);
        assert_eq!(t.active_pane, p2, "focus goes to the previous pane");
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                MuxEvent::PaneRemoved { pane: p3 },
                MuxEvent::LayoutChanged { tab },
                MuxEvent::PaneFocused { pane: p2 },
            ]
        );
    }

    #[test]
    fn closing_first_pane_focuses_the_next() {
        let (mut m, w, tab, [p1, p2, _]) = three();
        m.focus_pane(p1).unwrap();
        m.close_pane(p1).unwrap();
        assert_eq!(tab_of(&m, w, tab).active_pane, p2);
    }

    #[test]
    fn closing_an_unfocused_pane_keeps_focus() {
        let (mut m, w, tab, [p1, _, p3]) = three();
        let events = log(&mut m);
        m.close_pane(p1).unwrap();
        assert_eq!(tab_of(&m, w, tab).active_pane, p3);
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, MuxEvent::PaneFocused { .. })));
    }

    #[test]
    fn closing_the_last_pane_closes_tab_then_window() {
        let (mut m, w, tab, p1) = setup();
        let (tab2, p2) = m.spawn_tab(w, None, None).unwrap();
        assert_eq!(
            m.close_pane(p1).unwrap(),
            PaneClosed::Tab {
                window: w,
                tab,
                window_closed: false
            }
        );
        assert_eq!(
            m.kill_pane(p2).unwrap(),
            (w, tab2, true),
            "kill_pane keeps its (window, tab, window_closed) shape"
        );
        assert!(m.window(w).is_none());
    }

    #[test]
    fn kill_pane_on_a_split_leaves_the_tab() {
        let (mut m, w, tab, [_, p2, _]) = three();
        assert_eq!(m.kill_pane(p2).unwrap(), (w, tab, false));
        assert!(m.window(w).is_some());
    }

    #[test]
    fn focus_pane_activates_its_tab_and_is_quiet_when_unchanged() {
        let (mut m, w, tab, p1) = setup();
        let (_t2, _p2) = m.spawn_tab(w, None, None).unwrap();
        assert_eq!(m.window(w).unwrap().active, 1);
        let events = log(&mut m);
        m.focus_pane(p1).unwrap();
        assert_eq!(m.window(w).unwrap().active, 0);
        assert!(
            events.lock().unwrap().is_empty(),
            "p1 was already the tab's pane"
        );
        let _ = tab;
        assert!(matches!(m.focus_pane(5000), Err(Error::NoPane(5000))));
    }

    #[test]
    fn split_toward_left_and_up_puts_the_new_pane_first_and_focuses_it() {
        let (mut m, w, tab, p1) = setup();
        let left = m.split_pane_toward(p1, Dir::Left, 0.3, None, None).unwrap();
        let t = tab_of(&m, w, tab);
        assert_eq!(order(&t), vec![left, p1]);
        assert_eq!(t.active_pane, left);
        let PaneTree::Split { ratio, axis, .. } = t.tree else {
            panic!()
        };
        assert_eq!(axis, Axis::Horizontal);
        assert!(
            (ratio - 0.3).abs() < 1e-5,
            "the new pane (a) takes its share"
        );
        let up = m.split_pane_toward(p1, Dir::Up, 0.5, None, None).unwrap();
        assert_eq!(order(&tab_of(&m, w, tab)), vec![left, up, p1]);
        assert_eq!(m.focus_direction(up, Dir::Down).unwrap(), Some(p1));
        assert_eq!(m.focus_direction(p1, Dir::Left).unwrap(), Some(left));
        let right = m
            .split_pane_toward(p1, Dir::Right, 0.5, None, None)
            .unwrap();
        assert_eq!(order(&tab_of(&m, w, tab)), vec![left, up, p1, right]);
    }

    #[test]
    fn focus_direction_walks_an_irregular_layout() {
        let (mut m, w, tab, [p1, p2, p3]) = three();
        assert_eq!(m.focus_direction(p3, Dir::Left).unwrap(), Some(p1));
        assert_eq!(tab_of(&m, w, tab).active_pane, p1);
        assert_eq!(m.focus_direction(p1, Dir::Right).unwrap(), Some(p2));
        assert_eq!(m.focus_direction(p2, Dir::Down).unwrap(), Some(p3));
        assert_eq!(m.focus_direction(p3, Dir::Down).unwrap(), None);
        assert_eq!(
            tab_of(&m, w, tab).active_pane,
            p3,
            "focus stays put at an edge"
        );
    }

    #[test]
    fn zoom_toggles_and_leaves_on_focus_change() {
        let (mut m, w, tab, [p1, p2, _]) = three();
        assert!(m.toggle_zoom(p2).unwrap());
        let t = tab_of(&m, w, tab);
        assert_eq!((t.zoomed, t.active_pane), (Some(p2), p2));
        m.focus_pane(p1).unwrap();
        assert_eq!(tab_of(&m, w, tab).zoomed, None);
        assert!(m.toggle_zoom(p1).unwrap());
        assert!(!m.toggle_zoom(p1).unwrap());
        let (mut single, _, _, only) = setup();
        assert!(!single.toggle_zoom(only).unwrap(), "nothing to zoom out of");
    }

    #[test]
    fn closing_the_zoomed_pane_and_splitting_clear_zoom() {
        let (mut m, w, tab, [_, p2, p3]) = three();
        m.toggle_zoom(p3).unwrap();
        m.close_pane(p3).unwrap();
        assert_eq!(tab_of(&m, w, tab).zoomed, None);
        m.toggle_zoom(p2).unwrap();
        m.split_pane(p2, Axis::Vertical, 0.5, None, None).unwrap();
        assert_eq!(tab_of(&m, w, tab).zoomed, None);
    }

    #[test]
    fn swap_and_rotate_reorder_and_keep_focus_on_the_pane() {
        let (mut m, w, tab, [p1, p2, p3]) = three();
        m.focus_pane(p1).unwrap();
        assert!(m.swap_pane(p1, Dir::Right).unwrap());
        let t = tab_of(&m, w, tab);
        assert_eq!(order(&t), vec![p2, p1, p3]);
        assert_eq!(t.active_pane, p1);
        assert!(!m.swap_pane(p2, Dir::Left).unwrap(), "no neighbour");
        m.rotate_panes(p1).unwrap();
        assert_eq!(order(&tab_of(&m, w, tab)), vec![p3, p2, p1]);
    }

    #[test]
    fn resize_emits_only_when_a_border_moves() {
        let (mut m, w, tab, [p1, p2, _]) = three();
        let events = log(&mut m);
        assert!(m.resize_pane(p1, Dir::Right, 0.1).unwrap());
        assert!(!m.resize_pane(p1, Dir::Up, 0.1).unwrap());
        assert_eq!(
            *events.lock().unwrap(),
            vec![MuxEvent::LayoutChanged { tab }]
        );
        let PaneTree::Split { ratio, .. } = tab_of(&m, w, tab).tree else {
            panic!()
        };
        assert!((ratio - 0.6).abs() < 1e-5);
        assert!(m.resize_pane_cells(p1, Dir::Right, 10, (60, 40)).unwrap());
        let PaneTree::Split { ratio, .. } = tab_of(&m, w, tab).tree else {
            panic!()
        };
        assert!((ratio - 0.7).abs() < 1e-4, "10 of 100 columns");
        m.set_split_ratio(tab, &[true], 0.25).unwrap();
        let _ = p2;
        assert!(matches!(
            m.set_split_ratio(404, &[], 0.5),
            Err(Error::NoTab(404))
        ));
    }

    #[test]
    fn close_tab_reports_every_pane_of_a_split_tab() {
        let (mut m, w, tab, [p1, p2, p3]) = three();
        let events = log(&mut m);
        m.close_tab(w, tab).unwrap();
        let removed: Vec<Id> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                MuxEvent::PaneRemoved { pane } => Some(*pane),
                _ => None,
            })
            .collect();
        assert_eq!(removed, vec![p1, p2, p3]);
    }
}
