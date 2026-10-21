//! Keyboard navigation of an open menu (SPEC §10.9.1): ↑/↓, → / ← into and out of a submenu,
//! Enter, Esc and type-to-filter, with `max_rows` windowing. GPUI-free.

use super::{MenuRow, RowKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
    Backspace,
    Char(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavOutcome {
    /// Handled; redraw.
    Moved,
    /// Run the item with this id.
    Activate(String),
    Close,
    Ignored,
}

/// A visible row: its index path into `rows` (`(top, Some(child))` for a submenu row shown
/// flat while filtering).
pub type RowRef = (usize, Option<usize>);

#[derive(Debug, Clone, PartialEq)]
pub struct MenuNav {
    pub rows: Vec<MenuRow>,
    pub filter: String,
    /// Highlighted entry of [`MenuNav::visible`].
    pub highlight: Option<usize>,
    /// The open submenu (index into `rows`) and its highlighted child.
    pub sub: Option<(usize, Option<usize>)>,
    /// First visible entry when the list is longer than `max_rows`.
    pub scroll: usize,
    pub max_rows: usize,
}

fn selectable(r: &MenuRow) -> bool {
    r.enabled && matches!(r.kind, RowKind::Item | RowKind::Submenu(_))
}

impl MenuNav {
    pub fn new(rows: Vec<MenuRow>, max_rows: usize) -> Self {
        Self {
            rows,
            filter: String::new(),
            highlight: None,
            sub: None,
            scroll: 0,
            max_rows: max_rows.max(1),
        }
    }

    /// The rows on screen. Unfiltered: every row. Filtered: matching items, submenu children
    /// flattened, with no separators or headers.
    pub fn visible(&self) -> Vec<RowRef> {
        if self.filter.is_empty() {
            return (0..self.rows.len()).map(|i| (i, None)).collect();
        }
        let needle = self.filter.to_lowercase();
        let hit = |r: &MenuRow| r.enabled && r.label.to_lowercase().contains(&needle);
        let mut out = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            match &r.kind {
                RowKind::Item if hit(r) => out.push((i, None)),
                RowKind::Submenu(children) => {
                    for (j, c) in children.iter().enumerate() {
                        if c.kind == RowKind::Item && hit(c) {
                            out.push((i, Some(j)));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub fn row(&self, at: RowRef) -> Option<&MenuRow> {
        let top = self.rows.get(at.0)?;
        match (at.1, &top.kind) {
            (None, _) => Some(top),
            (Some(j), RowKind::Submenu(c)) => c.get(j),
            _ => None,
        }
    }

    fn submenu_rows(&self, i: usize) -> &[MenuRow] {
        match self.rows.get(i).map(|r| &r.kind) {
            Some(RowKind::Submenu(c)) => c,
            _ => &[],
        }
    }

    fn step(
        from: Option<usize>,
        len: usize,
        ok: &dyn Fn(usize) -> bool,
        down: bool,
    ) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let start = match (from, down) {
            (None, true) => len - 1,
            (None, false) => 0,
            (Some(i), _) => i,
        };
        let mut i = start;
        for _ in 0..len {
            i = if down {
                (i + 1) % len
            } else {
                (i + len - 1) % len
            };
            if ok(i) {
                return Some(i);
            }
        }
        None
    }

    fn keep_visible(&mut self) {
        let Some(h) = self.highlight else { return };
        if h < self.scroll {
            self.scroll = h;
        } else if h >= self.scroll + self.max_rows {
            self.scroll = h + 1 - self.max_rows;
        }
    }

    /// Highlights a row under the pointer (`None` clears).
    pub fn hover(&mut self, at: Option<usize>) {
        self.highlight = at;
        if let Some(i) = at {
            let vis = self.visible();
            if let Some((top, None)) = vis.get(i) {
                if matches!(self.rows[*top].kind, RowKind::Submenu(_)) && self.rows[*top].enabled {
                    self.sub = Some((*top, None));
                } else {
                    self.sub = None;
                }
            }
        }
    }

    pub fn hover_child(&mut self, child: Option<usize>) {
        if let Some((top, _)) = self.sub {
            self.sub = Some((top, child));
        }
    }

    /// The id to run when `at` is activated (clicked), if it is runnable.
    pub fn activate(&self, at: RowRef) -> Option<String> {
        let r = self.row(at)?;
        (r.enabled && r.kind == RowKind::Item).then(|| r.id.clone())
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.visible().len().saturating_sub(self.max_rows);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    pub fn key(&mut self, key: NavKey) -> NavOutcome {
        // Inside an open submenu.
        if let Some((top, child)) = self.sub.filter(|_| self.filter.is_empty()) {
            let kids = self.submenu_rows(top).to_vec();
            let ok = |i: usize| selectable(&kids[i]);
            match key {
                NavKey::Down | NavKey::Up if child.is_some() => {
                    let next = Self::step(child, kids.len(), &ok, key == NavKey::Down);
                    self.sub = Some((top, next));
                    return NavOutcome::Moved;
                }
                NavKey::Left | NavKey::Escape if child.is_some() => {
                    self.sub = Some((top, None));
                    if key == NavKey::Escape {
                        self.sub = None;
                    }
                    return NavOutcome::Moved;
                }
                NavKey::Enter | NavKey::Right if child.is_some() => {
                    let c = &kids[child.expect("checked")];
                    return if c.enabled && c.kind == RowKind::Item {
                        NavOutcome::Activate(c.id.clone())
                    } else {
                        NavOutcome::Ignored
                    };
                }
                _ => {}
            }
        }
        let vis = self.visible();
        let flags: Vec<bool> = vis
            .iter()
            .map(|&at| self.row(at).is_some_and(selectable))
            .collect();
        let ok = |i: usize| flags[i];
        match key {
            NavKey::Down | NavKey::Up => {
                self.sub = None;
                self.highlight = Self::step(self.highlight, vis.len(), &ok, key == NavKey::Down);
                self.keep_visible();
                NavOutcome::Moved
            }
            NavKey::Home | NavKey::End => {
                self.sub = None;
                self.highlight = Self::step(None, vis.len(), &ok, key == NavKey::Home);
                self.keep_visible();
                NavOutcome::Moved
            }
            NavKey::Right | NavKey::Enter => {
                let Some(at) = self.highlight.and_then(|h| vis.get(h).copied()) else {
                    return NavOutcome::Ignored;
                };
                let Some(r) = self.row(at) else {
                    return NavOutcome::Ignored;
                };
                match &r.kind {
                    RowKind::Submenu(kids) if r.enabled => {
                        let first = Self::step(None, kids.len(), &|i| selectable(&kids[i]), true);
                        self.sub = Some((at.0, first));
                        NavOutcome::Moved
                    }
                    RowKind::Item if r.enabled && key == NavKey::Enter => {
                        NavOutcome::Activate(r.id.clone())
                    }
                    _ => NavOutcome::Ignored,
                }
            }
            NavKey::Left => {
                if self.sub.take().is_some() {
                    NavOutcome::Moved
                } else {
                    NavOutcome::Ignored
                }
            }
            NavKey::Escape => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.reset();
                    NavOutcome::Moved
                } else {
                    NavOutcome::Close
                }
            }
            NavKey::Backspace => {
                if self.filter.pop().is_some() {
                    self.reset();
                    NavOutcome::Moved
                } else {
                    NavOutcome::Ignored
                }
            }
            NavKey::Char(c) if !c.is_control() => {
                self.filter.push(c);
                self.reset();
                NavOutcome::Moved
            }
            NavKey::Char(_) => NavOutcome::Ignored,
        }
    }

    /// After the filter changed: highlight the first match.
    fn reset(&mut self) {
        self.sub = None;
        self.scroll = 0;
        let vis = self.visible();
        self.highlight = (0..vis.len()).find(|&i| self.row(vis[i]).is_some_and(selectable));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, enabled: bool) -> MenuRow {
        MenuRow {
            id: id.into(),
            label: id.replace('_', " "),
            chip_action: None,
            icon: None,
            enabled,
            kind: RowKind::Item,
        }
    }

    fn sep() -> MenuRow {
        MenuRow {
            kind: RowKind::Separator,
            enabled: false,
            ..item("sep", false)
        }
    }

    fn rows() -> Vec<MenuRow> {
        vec![
            item("copy", false),
            item("paste", true),
            sep(),
            MenuRow {
                kind: RowKind::Submenu(vec![
                    item("swap_left", true),
                    item("swap_right", false),
                    item("swap_up", true),
                ]),
                ..item("swap", true)
            },
            item("zoom", true),
        ]
    }

    #[test]
    fn arrows_skip_disabled_rows_and_separators_and_wrap() {
        let mut n = MenuNav::new(rows(), 20);
        n.key(NavKey::Down);
        assert_eq!(n.highlight, Some(1), "copy is disabled");
        n.key(NavKey::Down);
        assert_eq!(n.highlight, Some(3), "separator skipped");
        n.key(NavKey::Down);
        n.key(NavKey::Down);
        assert_eq!(n.highlight, Some(1), "wraps");
        n.key(NavKey::Up);
        assert_eq!(n.highlight, Some(4));
        assert_eq!(n.key(NavKey::Enter), NavOutcome::Activate("zoom".into()));
    }

    #[test]
    fn right_opens_a_submenu_and_left_closes_it() {
        let mut n = MenuNav::new(rows(), 20);
        n.highlight = Some(3);
        assert_eq!(n.key(NavKey::Right), NavOutcome::Moved);
        assert_eq!(n.sub, Some((3, Some(0))));
        n.key(NavKey::Down);
        assert_eq!(n.sub, Some((3, Some(2))), "disabled child skipped");
        assert_eq!(n.key(NavKey::Enter), NavOutcome::Activate("swap_up".into()));
        n.key(NavKey::Left);
        assert_eq!(n.sub, Some((3, None)));
        n.key(NavKey::Left);
        assert_eq!(n.sub, None);
        assert_eq!(n.key(NavKey::Escape), NavOutcome::Close);
    }

    #[test]
    fn typing_filters_and_flattens_submenus() {
        let mut n = MenuNav::new(rows(), 20);
        n.key(NavKey::Char('s'));
        n.key(NavKey::Char('w'));
        let vis = n.visible();
        assert_eq!(
            vis,
            vec![(3, Some(0)), (3, Some(2))],
            "disabled swap_right is gone"
        );
        assert_eq!(n.highlight, Some(0));
        assert_eq!(
            n.key(NavKey::Enter),
            NavOutcome::Activate("swap_left".into())
        );
        n.key(NavKey::Backspace);
        assert_eq!(n.filter, "s");
        assert_eq!(
            n.key(NavKey::Escape),
            NavOutcome::Moved,
            "Esc clears the filter first"
        );
        assert!(n.filter.is_empty());
        assert_eq!(n.key(NavKey::Escape), NavOutcome::Close);
    }

    #[test]
    fn long_menus_scroll_to_keep_the_highlight_visible() {
        let many: Vec<_> = (0..30).map(|i| item(&format!("i{i}"), true)).collect();
        let mut n = MenuNav::new(many, 10);
        n.key(NavKey::End);
        assert_eq!(n.highlight, Some(29));
        assert_eq!(n.scroll, 20);
        n.key(NavKey::Home);
        assert_eq!(n.scroll, 0);
        n.scroll_by(50);
        assert_eq!(n.scroll, 20);
    }

    #[test]
    fn hover_opens_submenus_and_clicks_activate_items_only() {
        let mut n = MenuNav::new(rows(), 20);
        n.hover(Some(3));
        assert_eq!(n.sub, Some((3, None)));
        n.hover(Some(4));
        assert_eq!(n.sub, None);
        assert_eq!(n.activate((3, Some(0))), Some("swap_left".into()));
        assert_eq!(n.activate((3, None)), None);
        assert_eq!(n.activate((0, None)), None, "disabled");
    }
}
