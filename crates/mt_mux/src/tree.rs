//! Pane-tree operations (SPEC §17.1): split, close, directional focus, resize, swap, rotate.
//!
//! `Axis::Horizontal` lays the two children side by side (`a` left, `b` right);
//! `Axis::Vertical` stacks them (`a` top, `b` bottom). `ratio` is the share of `a`.
//! All geometry here is unitless so the model, the UI (pixels) and tests share it.

use crate::model::{Axis, Id, Pane, PaneTree};

pub const MIN_RATIO: f32 = 0.05;
pub const MAX_RATIO: f32 = 0.95;

/// Where a split puts the new pane relative to the one it splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The new pane is `a` (left or top).
    Before,
    /// The new pane is `b` (right or bottom).
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl From<mt_proto::Direction> for Dir {
    fn from(d: mt_proto::Direction) -> Self {
        match d {
            mt_proto::Direction::Left => Dir::Left,
            mt_proto::Direction::Right => Dir::Right,
            mt_proto::Direction::Up => Dir::Up,
            mt_proto::Direction::Down => Dir::Down,
        }
    }
}

impl Dir {
    pub fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Horizontal,
            Dir::Up | Dir::Down => Axis::Vertical,
        }
    }

    /// Splitting toward `Left`/`Up` puts the new pane before the old one.
    pub fn placement(self) -> Placement {
        match self {
            Dir::Left | Dir::Up => Placement::Before,
            Dir::Right | Dir::Down => Placement::After,
        }
    }

    pub fn parse(s: &str) -> Option<Dir> {
        match s {
            "left" => Some(Dir::Left),
            "right" => Some(Dir::Right),
            "up" => Some(Dir::Up),
            "down" => Some(Dir::Down),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Dir::Left => "left",
            Dir::Right => "right",
            Dir::Up => "up",
            Dir::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// One split node of a laid-out tree, for divider drawing and hit-testing.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitRect {
    /// Path from the root: `false` = `a`, `true` = `b`.
    pub path: Vec<bool>,
    pub axis: Axis,
    pub ratio: f32,
    /// The area the whole split occupies.
    pub rect: Rect,
}

pub fn clamp_ratio(r: f32) -> f32 {
    if r.is_nan() {
        0.5
    } else {
        r.clamp(MIN_RATIO, MAX_RATIO)
    }
}

fn halves(axis: Axis, ratio: f32, r: Rect) -> (Rect, Rect) {
    match axis {
        Axis::Horizontal => {
            let wa = r.w * ratio;
            (
                Rect::new(r.x, r.y, wa, r.h),
                Rect::new(r.x + wa, r.y, r.w - wa, r.h),
            )
        }
        Axis::Vertical => {
            let ha = r.h * ratio;
            (
                Rect::new(r.x, r.y, r.w, ha),
                Rect::new(r.x, r.y + ha, r.w, r.h - ha),
            )
        }
    }
}

impl PaneTree {
    pub fn find(&self, id: Id) -> Option<&Pane> {
        match self {
            PaneTree::Leaf(p) => (p.id == id).then_some(p),
            PaneTree::Split { a, b, .. } => a.find(id).or_else(|| b.find(id)),
        }
    }

    pub fn find_mut(&mut self, id: Id) -> Option<&mut Pane> {
        match self {
            PaneTree::Leaf(p) => (p.id == id).then_some(p),
            PaneTree::Split { a, b, .. } => {
                if a.find(id).is_some() {
                    a.find_mut(id)
                } else {
                    b.find_mut(id)
                }
            }
        }
    }

    pub fn contains(&self, id: Id) -> bool {
        self.find(id).is_some()
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            PaneTree::Leaf(_) => 1,
            PaneTree::Split { a, b, .. } => a.leaf_count() + b.leaf_count(),
        }
    }

    /// Replaces leaf `target` with a split holding it (`a`) and `new` (`b`).
    pub fn split_leaf(&mut self, target: Id, axis: Axis, ratio: f32, new: Pane) -> bool {
        self.split_leaf_at(target, axis, ratio, new, Placement::After)
    }

    /// Replaces leaf `target` with a split holding it and `new`, with `new` on the side given
    /// by `placement`. `ratio` stays the share of `a`, whichever pane that is.
    pub fn split_leaf_at(
        &mut self,
        target: Id,
        axis: Axis,
        ratio: f32,
        new: Pane,
        placement: Placement,
    ) -> bool {
        match self {
            PaneTree::Leaf(p) if p.id == target => {
                let old = Box::new(PaneTree::Leaf(p.clone()));
                let new = Box::new(PaneTree::Leaf(new));
                let (a, b) = match placement {
                    Placement::Before => (new, old),
                    Placement::After => (old, new),
                };
                *self = PaneTree::Split {
                    axis,
                    ratio: clamp_ratio(ratio),
                    a,
                    b,
                };
                true
            }
            PaneTree::Leaf(_) => false,
            PaneTree::Split { a, b, .. } => {
                if a.contains(target) {
                    a.split_leaf_at(target, axis, ratio, new, placement)
                } else {
                    b.split_leaf_at(target, axis, ratio, new, placement)
                }
            }
        }
    }

    /// Removes leaf `id`, promoting its sibling. False when `id` is missing or the only leaf.
    pub fn remove_leaf(&mut self, id: Id) -> bool {
        let PaneTree::Split { a, b, .. } = self else {
            return false;
        };
        let is_leaf = |t: &PaneTree| matches!(t, PaneTree::Leaf(p) if p.id == id);
        if is_leaf(a) {
            let sibling = (**b).clone();
            *self = sibling;
            true
        } else if is_leaf(b) {
            let sibling = (**a).clone();
            *self = sibling;
            true
        } else if a.contains(id) {
            a.remove_leaf(id)
        } else {
            b.remove_leaf(id)
        }
    }

    /// Leaf rectangles inside `rect`, in leaf order.
    pub fn layout(&self, rect: Rect) -> Vec<(Id, Rect)> {
        let mut out = Vec::new();
        self.layout_into(rect, &mut out);
        out
    }

    fn layout_into(&self, rect: Rect, out: &mut Vec<(Id, Rect)>) {
        match self {
            PaneTree::Leaf(p) => out.push((p.id, rect)),
            PaneTree::Split { axis, ratio, a, b } => {
                let (ra, rb) = halves(*axis, *ratio, rect);
                a.layout_into(ra, out);
                b.layout_into(rb, out);
            }
        }
    }

    /// Every split node with the area it covers, outermost first.
    pub fn splits(&self, rect: Rect) -> Vec<SplitRect> {
        let mut out = Vec::new();
        self.splits_into(rect, &mut Vec::new(), &mut out);
        out
    }

    fn splits_into(&self, rect: Rect, path: &mut Vec<bool>, out: &mut Vec<SplitRect>) {
        if let PaneTree::Split { axis, ratio, a, b } = self {
            out.push(SplitRect {
                path: path.clone(),
                axis: *axis,
                ratio: *ratio,
                rect,
            });
            let (ra, rb) = halves(*axis, *ratio, rect);
            path.push(false);
            a.splits_into(ra, path, out);
            path.pop();
            path.push(true);
            b.splits_into(rb, path, out);
            path.pop();
        }
    }

    /// Sets the ratio of the split at `path` (divider drag). False if there is none.
    pub fn set_ratio_at(&mut self, path: &[bool], ratio: f32) -> bool {
        match (self, path.split_first()) {
            (PaneTree::Split { ratio: r, .. }, None) => {
                *r = clamp_ratio(ratio);
                true
            }
            (PaneTree::Split { a, b, .. }, Some((&side, rest))) => {
                if side {
                    b.set_ratio_at(rest, ratio)
                } else {
                    a.set_ratio_at(rest, ratio)
                }
            }
            _ => false,
        }
    }

    /// The neighbour of `from` in direction `dir`, judged on the geometry of `rect`.
    ///
    /// Candidates must lie fully past the pane's edge and overlap it on the other axis;
    /// the nearest wins, then the one with the largest overlap, then the first in leaf order.
    pub fn neighbour(&self, from: Id, dir: Dir, rect: Rect) -> Option<Id> {
        const EPS: f32 = 1e-4;
        let leaves = self.layout(rect);
        let (_, me) = *leaves.iter().find(|(id, _)| *id == from)?;
        let mut best: Option<(f32, f32, Id)> = None;
        for (id, r) in leaves.iter().copied().filter(|(id, _)| *id != from) {
            let (gap, overlap) = match dir {
                Dir::Right => (
                    r.x - me.right(),
                    overlap(me.y, me.bottom(), r.y, r.bottom()),
                ),
                Dir::Left => (
                    me.x - r.right(),
                    overlap(me.y, me.bottom(), r.y, r.bottom()),
                ),
                Dir::Down => (r.y - me.bottom(), overlap(me.x, me.right(), r.x, r.right())),
                Dir::Up => (me.y - r.bottom(), overlap(me.x, me.right(), r.x, r.right())),
            };
            if gap < -EPS || overlap <= EPS {
                continue;
            }
            let better = match best {
                None => true,
                Some((bg, bo, _)) => {
                    gap < bg - EPS || ((gap - bg).abs() <= EPS && overlap > bo + EPS)
                }
            };
            if better {
                best = Some((gap, overlap, id));
            }
        }
        // Wrapping is deliberately absent: no neighbour means focus stays put.
        best.map(|(_, _, id)| id)
    }

    /// Swaps the positions of two leaves.
    pub fn swap_leaves(&mut self, x: Id, y: Id) -> bool {
        if x == y {
            return false;
        }
        let mut panes: Vec<Pane> = self.panes().into_iter().cloned().collect();
        let (Some(ix), Some(iy)) = (
            panes.iter().position(|p| p.id == x),
            panes.iter().position(|p| p.id == y),
        ) else {
            return false;
        };
        panes.swap(ix, iy);
        self.assign_leaves(&mut panes.into_iter());
        true
    }

    /// Rotates leaf positions by one in leaf order (the last pane moves to the front).
    pub fn rotate(&mut self) {
        let mut panes: Vec<Pane> = self.panes().into_iter().cloned().collect();
        if panes.len() < 2 {
            return;
        }
        panes.rotate_right(1);
        let mut it = panes.into_iter();
        self.assign_leaves(&mut it);
    }

    fn assign_leaves(&mut self, it: &mut impl Iterator<Item = Pane>) {
        match self {
            PaneTree::Leaf(p) => {
                if let Some(n) = it.next() {
                    *p = n;
                }
            }
            PaneTree::Split { a, b, .. } => {
                a.assign_leaves(it);
                b.assign_leaves(it);
            }
        }
    }

    /// Moves the border of `id` in `dir` by `delta`, a fraction of the whole `rect`.
    ///
    /// The nearest ancestor split on `dir`'s axis moves: the pane grows if `dir` points away
    /// from its sibling and shrinks otherwise. False when there is no such split.
    pub fn resize(&mut self, id: Id, dir: Dir, delta: f32, rect: Rect) -> bool {
        let PaneTree::Split { axis, ratio, a, b } = self else {
            return false;
        };
        let (ra, rb) = halves(*axis, *ratio, rect);
        // Deepest matching ancestor wins, so recurse first.
        let child = if a.contains(id) { (a, ra) } else { (b, rb) };
        if child.0.resize(id, dir, delta, child.1) {
            return true;
        }
        if *axis != dir.axis() {
            return false;
        }
        let extent = match axis {
            Axis::Horizontal => rect.w,
            Axis::Vertical => rect.h,
        };
        if extent <= 0.0 {
            return false;
        }
        // Moving the shared border toward b enlarges a.
        let sign = if matches!(dir, Dir::Right | Dir::Down) {
            1.0
        } else {
            -1.0
        };
        *ratio = clamp_ratio(*ratio + sign * delta / extent);
        true
    }
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    a1.min(b1) - a0.max(b0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::LOCAL_DOMAIN_ID;

    fn pane(id: Id) -> Pane {
        Pane {
            id,
            domain_id: LOCAL_DOMAIN_ID,
            remote_pane_id: None,
            title: String::new(),
            cwd: None,
            status: None,
            attention: None,
        }
    }

    fn leaf(id: Id) -> PaneTree {
        PaneTree::Leaf(pane(id))
    }

    fn ids(t: &PaneTree) -> Vec<Id> {
        t.panes().iter().map(|p| p.id).collect()
    }

    const R: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };

    const U: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    /// 1 | (2 over 3): left column is pane 1, right column is 2 on top of 3.
    fn irregular() -> PaneTree {
        let mut t = leaf(1);
        assert!(t.split_leaf(1, Axis::Horizontal, 0.5, pane(2)));
        assert!(t.split_leaf(2, Axis::Vertical, 0.5, pane(3)));
        t
    }

    #[test]
    fn split_leaf_puts_new_pane_second() {
        let mut t = leaf(1);
        assert!(t.split_leaf(1, Axis::Horizontal, 0.3, pane(2)));
        assert_eq!(ids(&t), vec![1, 2]);
        let l = t.layout(R);
        assert!((l[0].1.w - 30.0).abs() < 1e-3 && (l[1].1.x - 30.0).abs() < 1e-3);
    }

    #[test]
    fn split_before_puts_new_pane_first() {
        let mut t = leaf(1);
        assert!(t.split_leaf_at(1, Axis::Horizontal, 0.3, pane(2), Placement::Before));
        assert_eq!(ids(&t), vec![2, 1]);
        let l = t.layout(R);
        assert_eq!(l[0].0, 2);
        assert!((l[0].1.w - 30.0).abs() < 1e-3, "ratio is the share of `a`");
        let mut t = leaf(1);
        assert!(t.split_leaf_at(1, Axis::Vertical, 0.5, pane(2), Placement::Before));
        assert_eq!(t.neighbour(1, Dir::Up, U), Some(2));
        assert_eq!(t.neighbour(2, Dir::Down, U), Some(1));
    }

    #[test]
    fn dir_maps_to_axis_and_placement() {
        assert_eq!(Dir::Left.placement(), Placement::Before);
        assert_eq!(Dir::Up.placement(), Placement::Before);
        assert_eq!(Dir::Right.placement(), Placement::After);
        assert_eq!(Dir::Down.placement(), Placement::After);
    }

    #[test]
    fn split_unknown_target_fails() {
        let mut t = leaf(1);
        assert!(!t.split_leaf(9, Axis::Vertical, 0.5, pane(2)));
        assert_eq!(t.leaf_count(), 1);
    }

    #[test]
    fn ratio_is_clamped() {
        let mut t = leaf(1);
        t.split_leaf(1, Axis::Vertical, 5.0, pane(2));
        let PaneTree::Split { ratio, .. } = t else {
            panic!()
        };
        assert_eq!(ratio, MAX_RATIO);
        assert_eq!(clamp_ratio(f32::NAN), 0.5);
        assert_eq!(clamp_ratio(-1.0), MIN_RATIO);
    }

    #[test]
    fn remove_promotes_sibling() {
        let mut t = irregular();
        assert!(t.remove_leaf(2));
        assert_eq!(ids(&t), vec![1, 3]);
        assert!(t.remove_leaf(1));
        assert_eq!(t, leaf(3));
        assert!(!t.remove_leaf(3), "the only leaf cannot be removed");
        assert!(!t.remove_leaf(42));
    }

    #[test]
    fn layout_covers_the_rect_exactly() {
        let t = irregular();
        let l = t.layout(R);
        let area: f32 = l.iter().map(|(_, r)| r.w * r.h).sum();
        assert!((area - 10_000.0).abs() < 1e-3);
        assert_eq!(l[1].1, Rect::new(50.0, 0.0, 50.0, 50.0));
        assert_eq!(l[2].1, Rect::new(50.0, 50.0, 50.0, 50.0));
    }

    #[test]
    fn neighbours_on_an_irregular_layout() {
        let t = irregular();
        assert_eq!(
            t.neighbour(1, Dir::Right, R),
            Some(2),
            "ties go to the first leaf"
        );
        assert_eq!(t.neighbour(2, Dir::Left, R), Some(1));
        assert_eq!(t.neighbour(3, Dir::Left, R), Some(1));
        assert_eq!(t.neighbour(2, Dir::Down, R), Some(3));
        assert_eq!(t.neighbour(3, Dir::Up, R), Some(2));
        assert_eq!(t.neighbour(1, Dir::Left, R), None);
        assert_eq!(t.neighbour(1, Dir::Up, R), None);
        assert_eq!(t.neighbour(3, Dir::Down, R), None);
        assert_eq!(t.neighbour(3, Dir::Right, R), None);
        assert_eq!(t.neighbour(99, Dir::Right, R), None);
    }

    #[test]
    fn neighbour_prefers_larger_overlap() {
        // Left column 1; right column 2 (top 20%) over 3 (bottom 80%).
        let mut t = leaf(1);
        t.split_leaf(1, Axis::Horizontal, 0.5, pane(2));
        t.split_leaf(2, Axis::Vertical, 0.2, pane(3));
        assert_eq!(t.neighbour(1, Dir::Right, R), Some(3));
    }

    #[test]
    fn neighbour_picks_nearest_not_widest() {
        // 1 | 2 | 3 in a row.
        let mut t = leaf(1);
        t.split_leaf(1, Axis::Horizontal, 0.34, pane(2));
        t.split_leaf(2, Axis::Horizontal, 0.5, pane(3));
        assert_eq!(t.neighbour(1, Dir::Right, R), Some(2));
        assert_eq!(t.neighbour(3, Dir::Left, R), Some(2));
    }

    #[test]
    fn swap_exchanges_positions() {
        let mut t = irregular();
        assert!(t.swap_leaves(1, 3));
        assert_eq!(ids(&t), vec![3, 2, 1]);
        assert!(!t.swap_leaves(1, 1));
        assert!(!t.swap_leaves(1, 99));
    }

    #[test]
    fn rotate_moves_every_pane_one_slot() {
        let mut t = irregular();
        t.rotate();
        assert_eq!(ids(&t), vec![3, 1, 2]);
        t.rotate();
        t.rotate();
        assert_eq!(ids(&t), vec![1, 2, 3]);
        let mut single = leaf(1);
        single.rotate();
        assert_eq!(single, leaf(1));
    }

    #[test]
    fn resize_moves_nearest_matching_border() {
        let mut t = irregular();
        // Pane 1 grows right: outer horizontal split ratio rises.
        assert!(t.resize(1, Dir::Right, 0.1, U));
        let PaneTree::Split { ratio, .. } = &t else {
            panic!()
        };
        assert!((ratio - 0.6).abs() < 1e-5);
        // Pane 2 grows down: the inner vertical split.
        assert!(t.resize(2, Dir::Down, 0.2, U));
        let l = t.layout(R);
        assert!((l[1].1.h - 70.0).abs() < 1e-3);
        // Pane 3 pushing its left border left grows b of the outer split (ratio falls).
        assert!(t.resize(3, Dir::Left, 0.1, U));
        let PaneTree::Split { ratio, .. } = &t else {
            panic!()
        };
        assert!((ratio - 0.5).abs() < 1e-5);
    }

    #[test]
    fn resize_without_a_split_on_that_axis_fails() {
        let mut t = irregular();
        assert!(
            !t.resize(1, Dir::Up, 0.1, U),
            "pane 1 spans the full height"
        );
        let mut single = leaf(1);
        assert!(!single.resize(1, Dir::Right, 0.1, U));
    }

    #[test]
    fn resize_delta_is_a_fraction_of_the_whole_area() {
        // 1 | (2 over 3): the inner split spans half the width, but vertically the whole height.
        let mut t = irregular();
        assert!(t.resize(2, Dir::Down, 0.1, U));
        assert!((t.layout(U)[1].1.h - 0.6).abs() < 1e-5);
        // Nested one level deeper along the same axis: 1 | (2 | 3), growing 2 rightwards moves
        // a split that is only half as wide, so the ratio moves twice as far.
        let mut t = leaf(1);
        t.split_leaf(1, Axis::Horizontal, 0.5, pane(2));
        t.split_leaf(2, Axis::Horizontal, 0.5, pane(3));
        assert!(t.resize(2, Dir::Right, 0.1, U));
        let l = t.layout(U);
        assert!(
            (l[1].1.w - 0.35).abs() < 1e-5,
            "pane 2 grew by 0.1 of the window"
        );
    }

    #[test]
    fn resize_clamps() {
        let mut t = irregular();
        t.resize(1, Dir::Right, 10.0, U);
        let PaneTree::Split { ratio, .. } = &t else {
            panic!()
        };
        assert_eq!(*ratio, MAX_RATIO);
    }

    #[test]
    fn splits_and_set_ratio_at() {
        let mut t = irregular();
        let s = t.splits(R);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].path, Vec::<bool>::new());
        assert_eq!(s[1].path, vec![true]);
        assert_eq!(s[1].rect, Rect::new(50.0, 0.0, 50.0, 100.0));
        assert!(t.set_ratio_at(&[true], 0.25));
        assert_eq!(t.layout(R)[1].1.h, 25.0);
        assert!(!t.set_ratio_at(&[false], 0.5), "path ends at a leaf");
    }

    #[test]
    fn find_mut_reaches_non_first_panes() {
        let mut t = irregular();
        t.find_mut(3).unwrap().title = "x".into();
        assert_eq!(t.find(3).unwrap().title, "x");
        assert!(t.find_mut(9).is_none());
    }
}
