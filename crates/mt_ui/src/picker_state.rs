//! Pure state machine behind the `Picker` component: fuzzy filtering with nucleo, selection,
//! scrolling and the keyboard outcomes. No GPUI types, so it is unit-testable in isolation.

use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

use crate::metrics::PALETTE_MAX_ROWS;

/// One filtered row: index into the item list plus the matched character positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub index: usize,
    pub score: u32,
    pub positions: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerInput {
    Char(char),
    Backspace,
    Up,
    Down,
    Confirm,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerOutcome {
    /// Nothing observable changed.
    None,
    /// Query or selection changed; the view should re-render (and live-preview if it wants).
    Changed,
    /// Enter on a row: index into the *original* item list.
    Confirmed(usize),
    Cancelled,
}

pub struct PickerState<T> {
    items: Vec<(String, T)>,
    query: String,
    matches: Vec<Match>,
    selected: usize,
    scroll: usize,
    max_rows: usize,
    matcher: Matcher,
    /// Provider group of each item (parallel to `items`); 0 when unused.
    groups: Vec<u8>,
    /// MRU rank of each item (0 = most recent), parallel to `items`.
    mru: Vec<Option<usize>>,
    /// `(prefix char, group)`: a query starting with the char filters to that group.
    prefixes: Vec<(char, u8)>,
    /// Group shown when the query has no prefix; `None` shows every group.
    default_group: Option<u8>,
}

impl<T> PickerState<T> {
    pub fn new(items: Vec<(String, T)>) -> Self {
        let len = items.len();
        let mut state = Self {
            items,
            groups: vec![0; len],
            mru: vec![None; len],
            prefixes: Vec::new(),
            default_group: None,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            scroll: 0,
            max_rows: PALETTE_MAX_ROWS,
            matcher: Matcher::new(Config::DEFAULT),
        };
        state.refilter();
        state
    }

    /// Tags each item with a provider group and an MRU rank (both parallel to the items).
    pub fn with_meta(mut self, groups: Vec<u8>, mru: Vec<Option<usize>>) -> Self {
        assert_eq!(groups.len(), self.items.len());
        assert_eq!(mru.len(), self.items.len());
        self.groups = groups;
        self.mru = mru;
        self.refilter();
        self
    }

    /// Enables provider prefixes such as `>` or `@`; see `prefixes` and `default_group`.
    pub fn with_prefixes(mut self, prefixes: &[(char, u8)], default_group: Option<u8>) -> Self {
        self.prefixes = prefixes.to_vec();
        self.default_group = default_group;
        self.refilter();
        self
    }

    /// The group the current query is restricted to, and the query text without its prefix.
    pub fn scope(&self) -> (Option<u8>, &str) {
        if let Some(first) = self.query.chars().next() {
            if let Some((_, g)) = self.prefixes.iter().find(|(c, _)| *c == first) {
                return (Some(*g), &self.query[first.len_utf8()..]);
            }
        }
        (self.default_group, &self.query)
    }

    pub fn with_max_rows(mut self, rows: usize) -> Self {
        self.max_rows = rows.max(1);
        self.scroll_into_view();
        self
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub fn label(&self, m: &Match) -> &str {
        &self.items[m.index].0
    }

    pub fn item(&self, index: usize) -> &T {
        &self.items[index].1
    }

    /// Index into `matches()` of the highlighted row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The highlighted match, if any row survives the filter.
    pub fn selected_match(&self) -> Option<&Match> {
        self.matches.get(self.selected)
    }

    pub fn selected_item(&self) -> Option<&T> {
        self.selected_match().map(|m| &self.items[m.index].1)
    }

    /// Slice of `matches()` that fits in the viewport, with the offset of its first row.
    pub fn visible(&self) -> (usize, &[Match]) {
        let end = (self.scroll + self.max_rows).min(self.matches.len());
        (self.scroll, &self.matches[self.scroll..end])
    }

    /// Highlights row `index` of `matches()` (clamped) and scrolls it into view.
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.matches.len().saturating_sub(1));
        self.scroll_into_view();
    }

    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.refilter();
    }

    pub fn handle(&mut self, input: PickerInput) -> PickerOutcome {
        match input {
            PickerInput::Char(c) => {
                self.query.push(c);
                self.refilter();
                PickerOutcome::Changed
            }
            PickerInput::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter();
                    PickerOutcome::Changed
                } else {
                    PickerOutcome::None
                }
            }
            PickerInput::Up => self.move_selection(-1),
            PickerInput::Down => self.move_selection(1),
            PickerInput::Confirm => match self.selected_match() {
                Some(m) => PickerOutcome::Confirmed(m.index),
                None => PickerOutcome::None,
            },
            PickerInput::Cancel => PickerOutcome::Cancelled,
        }
    }

    fn move_selection(&mut self, delta: isize) -> PickerOutcome {
        let len = self.matches.len();
        if len == 0 {
            return PickerOutcome::None;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.scroll_into_view();
        PickerOutcome::Changed
    }

    fn scroll_into_view(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + self.max_rows {
            self.scroll = self.selected + 1 - self.max_rows;
        }
    }

    fn refilter(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        let (group, text) = {
            let (g, t) = self.scope();
            (g, t.trim_start().to_string())
        };
        let in_scope = |groups: &[u8], i: usize| group.is_none_or(|g| groups[i] == g);
        // Recency orders equal scores; never-run items come after run ones.
        let mru_key = |mru: &[Option<usize>], i: usize| mru[i].unwrap_or(usize::MAX);
        if text.is_empty() {
            let mut idx: Vec<usize> = (0..self.items.len())
                .filter(|&i| in_scope(&self.groups, i))
                .collect();
            idx.sort_by_key(|&i| mru_key(&self.mru, i));
            self.matches = idx
                .into_iter()
                .map(|index| Match {
                    index,
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
            return;
        }
        let pattern = Pattern::parse(&text, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut matches = Vec::new();
        for (index, (label, _)) in self.items.iter().enumerate() {
            if !in_scope(&self.groups, index) {
                continue;
            }
            let mut positions = Vec::new();
            let haystack = Utf32Str::new(label, &mut buf);
            if let Some(score) = pattern.indices(haystack, &mut self.matcher, &mut positions) {
                positions.sort_unstable();
                positions.dedup();
                matches.push(Match {
                    index,
                    score,
                    positions,
                });
            }
        }
        // Highest score first, then most recently used; the sort is stable so remaining ties
        // keep the original order.
        let mru = &self.mru;
        matches.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| mru_key(mru, a.index).cmp(&mru_key(mru, b.index)))
        });
        self.matches = matches;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> PickerState<u32> {
        PickerState::new(
            [
                "New tab",
                "Close tab",
                "Show command palette",
                "Select theme",
                "Search",
            ]
            .iter()
            .enumerate()
            .map(|(i, s)| (s.to_string(), i as u32))
            .collect(),
        )
    }

    fn type_str(p: &mut PickerState<u32>, s: &str) {
        for c in s.chars() {
            p.handle(PickerInput::Char(c));
        }
    }

    #[test]
    fn empty_query_lists_everything_in_order() {
        let p = picker();
        let idx: Vec<_> = p.matches().iter().map(|m| m.index).collect();
        assert_eq!(idx, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn typing_filters_and_ranks() {
        let mut p = picker();
        type_str(&mut p, "tab");
        let labels: Vec<_> = p.matches().iter().map(|m| p.label(m).to_string()).collect();
        assert_eq!(labels.len(), 2);
        assert!(labels.contains(&"New tab".to_string()));
        assert!(labels.contains(&"Close tab".to_string()));
    }

    #[test]
    fn fuzzy_subsequence_match_reports_positions() {
        let mut p = picker();
        type_str(&mut p, "sth");
        let m = p.selected_match().expect("Select theme matches 'sth'");
        assert_eq!(p.label(m), "Select theme");
        assert_eq!(m.positions.len(), 3);
        assert!(m.positions.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn no_match_gives_empty_list_and_no_confirm() {
        let mut p = picker();
        type_str(&mut p, "zzzz");
        assert!(p.matches().is_empty());
        assert_eq!(p.handle(PickerInput::Confirm), PickerOutcome::None);
        assert_eq!(p.handle(PickerInput::Down), PickerOutcome::None);
    }

    #[test]
    fn backspace_widens_the_filter() {
        let mut p = picker();
        type_str(&mut p, "tabz");
        assert!(p.matches().is_empty());
        assert_eq!(p.handle(PickerInput::Backspace), PickerOutcome::Changed);
        assert_eq!(p.matches().len(), 2);
        let mut empty = picker();
        assert_eq!(empty.handle(PickerInput::Backspace), PickerOutcome::None);
    }

    #[test]
    fn arrows_move_and_wrap() {
        let mut p = picker();
        assert_eq!(p.selected(), 0);
        p.handle(PickerInput::Down);
        assert_eq!(p.selected(), 1);
        p.handle(PickerInput::Up);
        p.handle(PickerInput::Up);
        assert_eq!(p.selected(), 4, "wraps to the last row");
        p.handle(PickerInput::Down);
        assert_eq!(p.selected(), 0, "wraps to the first row");
    }

    #[test]
    fn query_change_resets_selection() {
        let mut p = picker();
        p.handle(PickerInput::Down);
        p.handle(PickerInput::Down);
        p.handle(PickerInput::Char('t'));
        assert_eq!(p.selected(), 0);
    }

    #[test]
    fn enter_confirms_original_index() {
        let mut p = picker();
        type_str(&mut p, "theme");
        assert_eq!(p.handle(PickerInput::Confirm), PickerOutcome::Confirmed(3));
        assert_eq!(p.selected_item(), Some(&3));
    }

    #[test]
    fn select_clamps_and_scrolls() {
        let items = (0..25).map(|i| (format!("item {i}"), i)).collect();
        let mut p = PickerState::new(items).with_max_rows(10);
        p.select(20);
        assert_eq!(p.selected(), 20);
        assert_eq!(p.visible().0, 11);
        p.select(99);
        assert_eq!(p.selected(), 24);
    }

    #[test]
    fn escape_cancels() {
        let mut p = picker();
        assert_eq!(p.handle(PickerInput::Cancel), PickerOutcome::Cancelled);
    }

    #[test]
    fn viewport_follows_selection() {
        let items = (0..25).map(|i| (format!("item {i}"), i)).collect();
        let mut p = PickerState::new(items).with_max_rows(10);
        assert_eq!(p.visible().1.len(), 10);
        for _ in 0..12 {
            p.handle(PickerInput::Down);
        }
        let (offset, rows) = p.visible();
        assert_eq!(p.selected(), 12);
        assert_eq!(offset, 3);
        assert!(offset <= p.selected() && p.selected() < offset + rows.len());
        for _ in 0..4 {
            p.handle(PickerInput::Up);
        }
        assert_eq!(p.visible().0, 3, "no scroll while selection stays in view");
        for _ in 0..7 {
            p.handle(PickerInput::Up);
        }
        assert_eq!(p.visible().0, 1);
    }

    #[test]
    fn smart_case_is_case_insensitive_for_lowercase_queries() {
        let mut p = picker();
        type_str(&mut p, "new");
        assert_eq!(p.matches().len(), 1);
        let mut q = picker();
        type_str(&mut q, "NEW");
        assert_eq!(q.matches().len(), 0, "an uppercase query is case-sensitive");
    }

    fn grouped() -> PickerState<u32> {
        PickerState::new(vec![
            ("New tab".into(), 0),
            ("Switch to workspace: api".into(), 1),
            ("Go to tab: cargo".into(), 2),
            ("Theme: Tokyo Night".into(), 3),
        ])
        .with_meta(vec![0, 1, 2, 3], vec![None; 4])
        .with_prefixes(&[('>', 0), ('@', 1), ('#', 2), ('~', 3)], Some(0))
    }

    #[test]
    fn default_scope_shows_only_the_default_group() {
        let p = grouped();
        assert_eq!(p.matches().len(), 1);
        assert_eq!(p.matches()[0].index, 0);
    }

    #[test]
    fn prefix_selects_provider_and_strips_from_the_match() {
        let mut p = grouped();
        p.set_query("@api");
        assert_eq!(p.matches().iter().map(|m| m.index).collect::<Vec<_>>(), [1]);
        p.set_query("~tokyo");
        assert_eq!(p.matches().iter().map(|m| m.index).collect::<Vec<_>>(), [3]);
        p.set_query("#");
        assert_eq!(p.matches().iter().map(|m| m.index).collect::<Vec<_>>(), [2]);
        p.set_query(">new");
        assert_eq!(p.matches().iter().map(|m| m.index).collect::<Vec<_>>(), [0]);
        assert_eq!(p.scope(), (Some(0), "new"));
    }

    #[test]
    fn no_default_group_merges_everything() {
        let p = PickerState::new(vec![("a".to_string(), 0u32), ("b".into(), 1)])
            .with_meta(vec![0, 1], vec![None, None])
            .with_prefixes(&[('@', 1)], None);
        assert_eq!(p.matches().len(), 2);
    }

    #[test]
    fn mru_orders_empty_query_and_breaks_score_ties() {
        let p = PickerState::new(vec![
            ("alpha".to_string(), 0u32),
            ("beta".into(), 1),
            ("gamma".into(), 2),
        ])
        .with_meta(vec![0; 3], vec![None, Some(1), Some(0)]);
        let order: Vec<_> = p.matches().iter().map(|m| m.index).collect();
        assert_eq!(order, [2, 1, 0]);

        let mut tie = PickerState::new(vec![("ab".to_string(), 0u32), ("ab".into(), 1)])
            .with_meta(vec![0; 2], vec![None, Some(0)]);
        tie.set_query("ab");
        assert_eq!(tie.matches()[0].index, 1);
    }
}
