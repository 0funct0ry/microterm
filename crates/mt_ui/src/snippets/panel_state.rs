//! Pure state behind the Snippets side panel (SPEC §10.8.6): the toggle/focus state machine,
//! folders, ranked search, selection, the double-click debounce and the gesture table.
//! No GPUI.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use mt_snippets::{Snippet, SnippetSet};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

use crate::components::TextEdit;

/// A second click on the same row inside this window is ignored (SPEC §10.8.6).
pub const CLICK_DEBOUNCE: Duration = Duration::from_millis(400);

pub const MIN_WIDTH: f32 = 220.0;
pub const MAX_WIDTH: f32 = 600.0;

pub fn clamp_width(w: f32) -> f32 {
    w.clamp(MIN_WIDTH, MAX_WIDTH)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Folder {
        name: String,
        collapsed: bool,
        count: usize,
    },
    Snippet {
        id: String,
    },
}

/// What `ToggleSnippetsPanel` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggled {
    /// Was closed: now open with the search field focused.
    Opened,
    /// Was open but unfocused: now focused.
    Focused,
    /// Was focused: now closed and the terminal has focus.
    Closed,
}

/// What `Esc` did inside the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escaped {
    ClearedQuery,
    /// Focus returns to the terminal; the panel stays open.
    FocusTerminal,
}

/// How an insert was requested: the snippet's own `run` unless a modifier overrides it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gesture {
    /// `Some(true)` = insert and run, `Some(false)` = never run, `None` = the snippet decides.
    pub run: Option<bool>,
    /// Insert into every pane of the active tab.
    pub broadcast: bool,
}

impl Gesture {
    /// `⌥` runs, `⇧` never runs (`⌥` wins when both are held), `⌘`/`Ctrl` broadcasts.
    pub fn from_modifiers(alt: bool, shift: bool, secondary: bool) -> Self {
        Self {
            run: if alt {
                Some(true)
            } else if shift {
                Some(false)
            } else {
                None
            },
            broadcast: secondary,
        }
    }
}

#[derive(Debug)]
pub struct Panel {
    pub open: bool,
    pub focused: bool,
    pub width: f32,
    pub query: TextEdit,
    pub selected: Option<String>,
    pub collapsed: HashSet<String>,
    last_click: Option<(String, Instant)>,
}

impl Panel {
    pub fn new(width: f32, open: bool) -> Self {
        Self {
            open,
            focused: false,
            width: clamp_width(width),
            query: TextEdit::default(),
            selected: None,
            collapsed: HashSet::new(),
            last_click: None,
        }
    }

    /// closed → open and focus search; open but unfocused → focus; focused → close.
    pub fn toggle(&mut self) -> Toggled {
        if !self.open {
            self.open = true;
            self.focused = true;
            Toggled::Opened
        } else if !self.focused {
            self.focused = true;
            Toggled::Focused
        } else {
            self.open = false;
            self.focused = false;
            Toggled::Closed
        }
    }

    /// `FocusSnippetsPanel`: opens if needed, never closes.
    pub fn focus(&mut self) {
        self.open = true;
        self.focused = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focused = false;
    }

    /// `Esc` clears a non-empty query, else hands focus to the terminal (the panel stays).
    pub fn escape(&mut self) -> Escaped {
        if self.query.text().is_empty() {
            self.focused = false;
            Escaped::FocusTerminal
        } else {
            self.query = TextEdit::default();
            Escaped::ClearedQuery
        }
    }

    /// Registers a click on `id`; false when it is a second click inside [`CLICK_DEBOUNCE`].
    pub fn click_allowed(&mut self, id: &str, now: Instant) -> bool {
        if let Some((last_id, at)) = &self.last_click {
            if last_id == id && now.saturating_duration_since(*at) < CLICK_DEBOUNCE {
                return false;
            }
        }
        self.last_click = Some((id.to_string(), now));
        true
    }

    pub fn toggle_folder(&mut self, name: &str) {
        if !self.collapsed.remove(name) {
            self.collapsed.insert(name.to_string());
        }
    }

    pub fn collapse_all(&mut self, set: &SnippetSet) {
        self.collapsed = set
            .folders()
            .into_iter()
            .filter(|f| !f.is_empty())
            .map(String::from)
            .collect();
    }

    pub fn rows(&self, set: &SnippetSet) -> Vec<Row> {
        rows(set, self.query.text(), &self.collapsed)
    }

    /// Ids of the visible snippet rows, in order.
    pub fn visible_ids(&self, set: &SnippetSet) -> Vec<String> {
        self.rows(set)
            .into_iter()
            .filter_map(|r| match r {
                Row::Snippet { id } => Some(id),
                Row::Folder { .. } => None,
            })
            .collect()
    }

    /// Moves the selection by `delta` among the visible snippets (wrapping is off: it stops at
    /// the ends). With nothing selected, `+1` selects the first and `-1` the last.
    pub fn move_selection(&mut self, set: &SnippetSet, delta: i32) {
        let ids = self.visible_ids(set);
        if ids.is_empty() {
            self.selected = None;
            return;
        }
        let cur = self
            .selected
            .as_ref()
            .and_then(|s| ids.iter().position(|i| i == s));
        let next = match cur {
            None if delta >= 0 => 0,
            None => ids.len() - 1,
            Some(i) => (i as i64 + i64::from(delta)).clamp(0, ids.len() as i64 - 1) as usize,
        };
        self.selected = Some(ids[next].clone());
    }

    /// Keeps the selection valid after the set or the query changed.
    pub fn reconcile(&mut self, set: &SnippetSet) {
        let ids = self.visible_ids(set);
        if !self.selected.as_ref().is_some_and(|s| ids.contains(s)) {
            self.selected = ids.first().cloned();
        }
    }
}

/// Weighted fuzzy score of `s` for the query pattern, or `None` when nothing matches.
fn score(pattern: &Pattern, matcher: &mut Matcher, s: &Snippet) -> Option<u32> {
    let mut buf = Vec::new();
    let mut best: Option<u32> = None;
    let tags = s.tags.join(" ");
    for (text, weight) in [
        (s.title.as_str(), 4),
        (tags.as_str(), 3),
        (s.folder.as_str(), 2),
        (s.description.as_str(), 2),
        (s.body.as_str(), 1),
    ] {
        if text.is_empty() {
            continue;
        }
        if let Some(sc) = pattern.score(Utf32Str::new(text, &mut buf), matcher) {
            best = best.max(Some(sc * weight));
        }
    }
    best
}

/// Snippet indices matching `query`, best first (ties keep file order).
pub fn rank(set: &SnippetSet, query: &str) -> Vec<usize> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut scored: Vec<(usize, u32)> = set
        .snippets
        .iter()
        .enumerate()
        .filter_map(|(i, s)| score(&pattern, &mut matcher, s).map(|sc| (i, sc)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| i).collect()
}

/// The panel's list: with an empty query, folders (collapsible, in order of first appearance)
/// and their snippets; with a query, one flat ranked list.
pub fn rows(set: &SnippetSet, query: &str, collapsed: &HashSet<String>) -> Vec<Row> {
    if !query.trim().is_empty() {
        return rank(set, query)
            .into_iter()
            .map(|i| Row::Snippet {
                id: set.snippets[i].id.clone(),
            })
            .collect();
    }
    let mut out = Vec::new();
    for folder in set.folders() {
        let members: Vec<&Snippet> = set.in_folder(folder).collect();
        let is_collapsed = !folder.is_empty() && collapsed.contains(folder);
        if !folder.is_empty() {
            out.push(Row::Folder {
                name: folder.to_string(),
                collapsed: is_collapsed,
                count: members.len(),
            });
        }
        if !is_collapsed {
            out.extend(
                members
                    .into_iter()
                    .map(|s| Row::Snippet { id: s.id.clone() }),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(id: &str, title: &str, folder: &str) -> Snippet {
        Snippet {
            id: id.into(),
            title: title.into(),
            folder: folder.into(),
            body: format!("echo {id}"),
            ..Default::default()
        }
    }

    fn set() -> SnippetSet {
        SnippetSet {
            snippets: vec![
                snip("a", "Tail pod logs", "k8s"),
                snip("b", "Plain thing", ""),
                snip("c", "Restart deployment", "k8s"),
                snip("d", "Rebase", "git"),
            ],
        }
    }

    fn ids(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Folder { name, .. } => format!("[{name}]"),
                Row::Snippet { id } => id.clone(),
            })
            .collect()
    }

    #[test]
    fn toggle_state_machine() {
        let mut p = Panel::new(300.0, false);
        assert_eq!(p.toggle(), Toggled::Opened);
        assert!(p.open && p.focused);
        // Focus moved to the terminal while the panel stays open.
        p.focused = false;
        assert_eq!(p.toggle(), Toggled::Focused);
        assert!(p.open && p.focused);
        assert_eq!(p.toggle(), Toggled::Closed);
        assert!(!p.open && !p.focused);
        p.focus();
        assert!(p.open && p.focused);
    }

    #[test]
    fn escape_clears_the_query_then_returns_focus() {
        let mut p = Panel::new(300.0, true);
        p.focused = true;
        p.query.insert("po");
        assert_eq!(p.escape(), Escaped::ClearedQuery);
        assert!(p.focused && p.query.text().is_empty());
        assert_eq!(p.escape(), Escaped::FocusTerminal);
        assert!(p.open && !p.focused, "the panel stays open");
    }

    #[test]
    fn width_is_clamped() {
        assert_eq!(Panel::new(100.0, false).width, 220.0);
        assert_eq!(Panel::new(900.0, false).width, 600.0);
        assert_eq!(Panel::new(333.0, false).width, 333.0);
    }

    #[test]
    fn double_click_inside_400ms_is_ignored() {
        let mut p = Panel::new(300.0, true);
        let t0 = Instant::now();
        assert!(p.click_allowed("a", t0));
        assert!(!p.click_allowed("a", t0 + Duration::from_millis(399)));
        // A different row is a new click.
        assert!(p.click_allowed("b", t0 + Duration::from_millis(100)));
        assert!(p.click_allowed("a", t0 + Duration::from_millis(200)));
        assert!(p.click_allowed("a", t0 + Duration::from_millis(200 + 400)));
        assert!(!p.click_allowed("a", t0 + Duration::from_millis(200 + 400 + 10)));
    }

    #[test]
    fn gestures() {
        let g = Gesture::from_modifiers;
        assert_eq!(
            g(false, false, false),
            Gesture {
                run: None,
                broadcast: false
            }
        );
        assert_eq!(
            g(true, false, false),
            Gesture {
                run: Some(true),
                broadcast: false
            }
        );
        assert_eq!(
            g(false, true, false),
            Gesture {
                run: Some(false),
                broadcast: false
            }
        );
        assert_eq!(
            g(false, false, true),
            Gesture {
                run: None,
                broadcast: true
            }
        );
        assert_eq!(
            g(true, true, true),
            Gesture {
                run: Some(true),
                broadcast: true
            }
        );
    }

    #[test]
    fn folders_in_first_appearance_order_and_collapse() {
        let s = set();
        let mut p = Panel::new(300.0, true);
        assert_eq!(ids(&p.rows(&s)), ["[k8s]", "a", "c", "b", "[git]", "d"]);
        p.toggle_folder("k8s");
        assert_eq!(ids(&p.rows(&s)), ["[k8s]", "b", "[git]", "d"]);
        p.toggle_folder("k8s");
        p.collapse_all(&s);
        assert_eq!(ids(&p.rows(&s)), ["[k8s]", "b", "[git]"]);
        let Row::Folder {
            count, collapsed, ..
        } = &p.rows(&s)[0]
        else {
            panic!()
        };
        assert_eq!((*count, *collapsed), (2, true));
    }

    #[test]
    fn a_query_flattens_and_ranks() {
        let s = set();
        assert_eq!(ids(&rows(&s, "tail", &HashSet::new())), ["a"]);
        assert_eq!(
            ids(&rows(&s, "zzzz", &HashSet::new())),
            Vec::<String>::new()
        );
        // Collapsed folders do not hide search results.
        let collapsed: HashSet<String> = ["k8s".to_string()].into();
        assert_eq!(ids(&rows(&s, "re", &collapsed)).len(), 2);
        // A title match outranks a body match.
        let mut s2 = set();
        s2.snippets[1].body = "deployment deployment".into();
        let r = ids(&rows(&s2, "deployment", &HashSet::new()));
        assert_eq!(r, ["c", "b"]);
    }

    #[test]
    fn query_matches_tags_folder_and_description() {
        let mut s = set();
        s.snippets[3].tags = vec!["squash".into()];
        s.snippets[1].description = "unrelated".into();
        assert_eq!(ids(&rows(&s, "squash", &HashSet::new())), ["d"]);
        assert_eq!(ids(&rows(&s, "k8s", &HashSet::new())), ["a", "c"]);
        assert_eq!(ids(&rows(&s, "unrelated", &HashSet::new())), ["b"]);
    }

    #[test]
    fn selection_moves_within_visible_rows() {
        let s = set();
        let mut p = Panel::new(300.0, true);
        p.move_selection(&s, 1);
        assert_eq!(p.selected.as_deref(), Some("a"));
        p.move_selection(&s, 1);
        p.move_selection(&s, 1);
        assert_eq!(p.selected.as_deref(), Some("b"));
        p.move_selection(&s, -1);
        assert_eq!(p.selected.as_deref(), Some("c"));
        for _ in 0..10 {
            p.move_selection(&s, 1);
        }
        assert_eq!(p.selected.as_deref(), Some("d"), "stops at the end");
        p.selected = None;
        p.move_selection(&s, -1);
        assert_eq!(p.selected.as_deref(), Some("d"));
    }

    #[test]
    fn reconcile_repairs_a_stale_selection() {
        let mut s = set();
        let mut p = Panel::new(300.0, true);
        p.selected = Some("gone".into());
        p.reconcile(&s);
        assert_eq!(p.selected.as_deref(), Some("a"));
        p.query.insert("rebase");
        p.reconcile(&s);
        assert_eq!(p.selected.as_deref(), Some("d"));
        s.snippets.clear();
        p.reconcile(&s);
        assert_eq!(p.selected, None);
    }
}
