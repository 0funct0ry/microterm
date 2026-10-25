//! The Findings list (SPEC §10.11.4): what `findings` actions collected, in a ring. Memory only.

use std::collections::VecDeque;

/// Longest line text kept in an entry.
pub const LINE_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub at_ms: u64,
    pub rule_id: String,
    pub rule_name: String,
    pub pane: u64,
    pub label: String,
    /// The matched line, plain text (never interpreted).
    pub line: String,
    /// Stable id of the line, for the jump.
    pub line_id: u64,
    /// The matched characters of the line, when the rule was a text rule.
    pub span: Option<(usize, usize)>,
    /// `×N` when identical consecutive entries merged.
    pub count: u32,
}

impl Finding {
    /// The line has left the scrollback: the click only focuses the pane.
    pub fn scrolled_out(&self, line_base: u64) -> bool {
        self.line_id < line_base
    }
}

#[derive(Default)]
pub struct Findings {
    items: VecDeque<Finding>,
}

impl Findings {
    /// Adds `f`, merging it into the newest entry when identical (a `×N` count instead of a new
    /// row); the oldest entry goes past `max`.
    pub fn push(&mut self, mut f: Finding, max: usize) {
        f.line = crate::triggers::template_clip(&f.line, LINE_MAX);
        if let Some(last) = self.items.back_mut() {
            if (
                &last.rule_id,
                last.pane,
                &last.label,
                &last.line,
                last.line_id,
            ) == (&f.rule_id, f.pane, &f.label, &f.line, f.line_id)
            {
                last.count = last.count.saturating_add(1);
                last.at_ms = f.at_ms;
                return;
            }
        }
        f.count = 1;
        self.items.push_back(f);
        while self.items.len() > max.max(1) {
            self.items.pop_front();
        }
    }

    pub fn items(&self) -> impl Iterator<Item = &Finding> {
        self.items.iter()
    }

    pub fn get(&self, index: usize) -> Option<&Finding> {
        self.items.get(index)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Entries of a pane that no longer exists stay (the click still names the rule), so
    /// nothing is removed on pane exit.
    pub fn forget_all(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(label: &str, line: &str, id: u64) -> Finding {
        Finding {
            at_ms: 0,
            rule_id: "r".into(),
            rule_name: "Rule".into(),
            pane: 1,
            label: label.into(),
            line: line.into(),
            line_id: id,
            span: None,
            count: 0,
        }
    }

    #[test]
    fn identical_consecutive_entries_merge_into_a_count() {
        let mut s = Findings::default();
        s.push(f("a", "x", 1), 10);
        s.push(f("a", "x", 1), 10);
        s.push(f("a", "x", 1), 10);
        assert_eq!((s.len(), s.get(0).unwrap().count), (1, 3));
        s.push(f("a", "x", 2), 10);
        s.push(f("a", "x", 1), 10);
        assert_eq!(s.len(), 3, "only consecutive entries merge");
    }

    #[test]
    fn ring_drops_the_oldest() {
        let mut s = Findings::default();
        for i in 0..7 {
            s.push(f("a", "x", i), 3);
        }
        let ids: Vec<u64> = s.items().map(|x| x.line_id).collect();
        assert_eq!(ids, [4, 5, 6]);
    }

    #[test]
    fn lines_are_cut_and_scrolled_out_is_relative_to_the_base() {
        let mut s = Findings::default();
        s.push(f("a", &"y".repeat(500), 5), 10);
        assert!(s.get(0).unwrap().line.chars().count() <= LINE_MAX);
        assert!(s.get(0).unwrap().scrolled_out(6));
        assert!(!s.get(0).unwrap().scrolled_out(5));
        s.clear();
        assert!(s.is_empty());
    }
}
