//! Workspace designer model (SPEC §17.1.1): a pure editor over `Vec<WorkspaceDef>` with no GPUI,
//! so every control maps one-to-one onto a config edit and is unit-testable.

pub use mt_config::edit::DiffTag;
use mt_config::edit::{self, EditError};
use mt_config::{
    split_command, validate_workspaces, SplitKind, WorkspaceDef, WorkspaceIssue, WorkspacePane,
    WorkspaceTab, MAX_SPLIT_DEPTH,
};

use crate::components::TextEdit;

/// Which text field has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    None,
    Name,
    TabTitle,
    TabCwd,
    Cwd,
    Command,
}

pub struct Designer {
    pub workspaces: Vec<WorkspaceDef>,
    pub startup: String,
    original: (Vec<WorkspaceDef>, String),
    /// Text of the config file when the designer opened (staleness hash + preview base).
    pub src: String,
    pub opened_hash: u64,
    /// Set when the file does not parse: the designer is read-only and shows this.
    pub read_only: Option<String>,
    pub ws: usize,
    pub tab: usize,
    /// Child indexes from the tab root to the selected pane.
    pub sel: Vec<usize>,
    pub field: Field,
    pub edit: TextEdit,
    pub show_preview: bool,
    pub confirm_discard: bool,
    pub comment_warning: bool,
}

/// A tab's split tree seen as a pane tree (a tab is the root node).
fn root_of(t: &WorkspaceTab) -> WorkspacePane {
    WorkspacePane {
        cwd: t.cwd.clone(),
        command: t.command.clone(),
        split: t.split,
        percent: t.percent,
        panes: t.panes.clone(),
    }
}

fn set_root(t: &mut WorkspaceTab, n: WorkspacePane) {
    t.cwd = n.cwd;
    t.command = n.command;
    t.split = n.split;
    t.percent = n.percent;
    t.panes = n.panes;
}

fn node<'a>(n: &'a mut WorkspacePane, path: &[usize]) -> Option<&'a mut WorkspacePane> {
    match path.split_first() {
        None => Some(n),
        Some((i, rest)) => node(n.panes.get_mut(*i)?, rest),
    }
}

/// Snaps a divider percentage to multiples of 5 within 5..=95.
pub fn snap_percent(p: i32) -> u8 {
    (((p + 2).div_euclid(5)) * 5).clamp(5, 95) as u8
}

impl Designer {
    pub fn open(src: String, select: Option<&str>) -> Self {
        let (workspaces, read_only) = match edit::read_workspaces(&src) {
            Ok(w) => (w, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let startup = toml::from_str::<toml::Table>(&src)
            .ok()
            .and_then(|t| t.get("startup_workspace")?.as_str().map(str::to_string))
            .unwrap_or_else(|| "default".into());
        let ws = select
            .and_then(|n| workspaces.iter().position(|w| w.name == n))
            .unwrap_or(0);
        let comment_warning = src.contains('#');
        let mut d = Self {
            original: (workspaces.clone(), startup.clone()),
            workspaces,
            startup,
            opened_hash: edit::content_hash(&src),
            src,
            read_only,
            ws,
            tab: 0,
            sel: Vec::new(),
            field: Field::None,
            edit: TextEdit::default(),
            show_preview: false,
            confirm_discard: false,
            comment_warning,
        };
        d.ensure_valid_selection();
        d
    }

    /// Seeds a one-workspace design from a live tab ("Save current layout as workspace…").
    pub fn from_layout(src: String, name: &str, tab: WorkspaceTab) -> Self {
        let mut d = Self::open(src, None);
        d.workspaces.push(WorkspaceDef {
            name: name.into(),
            tabs: vec![tab],
        });
        d.ws = d.workspaces.len() - 1;
        d.tab = 0;
        d.sel.clear();
        d
    }

    pub fn dirty(&self) -> bool {
        self.original != (self.workspaces.clone(), self.startup.clone())
    }

    pub fn issues(&self) -> Vec<WorkspaceIssue> {
        validate_workspaces(&self.workspaces, &self.startup)
    }

    pub fn can_save(&self) -> bool {
        self.read_only.is_none() && self.issues().is_empty()
    }

    fn ensure_valid_selection(&mut self) {
        self.ws = self.ws.min(self.workspaces.len().saturating_sub(1));
        let tabs = self.workspaces.get(self.ws).map_or(0, |w| w.tabs.len());
        self.tab = self.tab.min(tabs.saturating_sub(1));
        if self.pane().is_none() {
            self.sel.clear();
        }
    }

    // ---- workspaces

    pub fn new_workspace(&mut self) {
        self.field = Field::None;
        let mut n = 1;
        let name = loop {
            let c = format!("workspace-{n}");
            if self.workspaces.iter().all(|w| w.name != c) {
                break c;
            }
            n += 1;
        };
        self.workspaces.push(WorkspaceDef {
            name,
            tabs: vec![WorkspaceTab {
                title: None,
                cwd: None,
                command: vec![],
                split: None,
                percent: None,
                panes: vec![],
            }],
        });
        self.select_workspace(self.workspaces.len() - 1);
    }

    pub fn duplicate_workspace(&mut self) {
        self.field = Field::None;
        let Some(w) = self.workspaces.get(self.ws).cloned() else {
            return;
        };
        let mut n = 2;
        let name = loop {
            let c = format!("{} {n}", w.name);
            if self.workspaces.iter().all(|x| x.name != c) {
                break c;
            }
            n += 1;
        };
        self.workspaces
            .insert(self.ws + 1, WorkspaceDef { name, ..w });
        self.select_workspace(self.ws + 1);
    }

    pub fn delete_workspace(&mut self) {
        self.field = Field::None;
        if self.ws < self.workspaces.len() {
            let gone = self.workspaces.remove(self.ws);
            if self.startup == gone.name {
                self.startup = "default".into();
            }
            self.ensure_valid_selection();
        }
    }

    pub fn select_workspace(&mut self, i: usize) {
        self.ws = i;
        self.tab = 0;
        self.sel.clear();
        self.field = Field::None;
        self.ensure_valid_selection();
    }

    pub fn toggle_startup(&mut self, i: usize) {
        self.field = Field::None;
        if let Some(w) = self.workspaces.get(i) {
            self.startup = if self.startup == w.name {
                "default".into()
            } else {
                w.name.clone()
            };
        }
    }

    pub fn rename_workspace(&mut self, name: String) {
        if let Some(w) = self.workspaces.get_mut(self.ws) {
            if self.startup == w.name {
                self.startup = name.clone();
            }
            w.name = name;
        }
    }

    // ---- tabs

    pub fn new_tab(&mut self) {
        self.field = Field::None;
        if let Some(w) = self.workspaces.get_mut(self.ws) {
            w.tabs.push(WorkspaceTab {
                title: None,
                cwd: None,
                command: vec![],
                split: None,
                percent: None,
                panes: vec![],
            });
            self.tab = w.tabs.len() - 1;
            self.sel.clear();
        }
    }

    pub fn close_tab(&mut self) {
        self.field = Field::None;
        if let Some(w) = self.workspaces.get_mut(self.ws) {
            if self.tab < w.tabs.len() {
                w.tabs.remove(self.tab);
            }
        }
        self.ensure_valid_selection();
    }

    pub fn select_tab(&mut self, i: usize) {
        self.field = Field::None;
        self.tab = i;
        self.sel.clear();
        self.ensure_valid_selection();
    }

    pub fn tab_ref(&self) -> Option<&WorkspaceTab> {
        self.workspaces.get(self.ws)?.tabs.get(self.tab)
    }

    fn with_root<R>(&mut self, f: impl FnOnce(&mut WorkspacePane) -> R) -> Option<R> {
        let t = self.workspaces.get_mut(self.ws)?.tabs.get_mut(self.tab)?;
        let mut root = root_of(t);
        let r = f(&mut root);
        set_root(t, root);
        Some(r)
    }

    // ---- panes

    pub fn root(&self) -> Option<WorkspacePane> {
        self.tab_ref().map(root_of)
    }

    pub fn pane(&self) -> Option<WorkspacePane> {
        let mut r = self.root()?;
        node(&mut r, &self.sel).map(|n| n.clone())
    }

    pub fn select_pane(&mut self, path: Vec<usize>) {
        self.sel = path;
        self.field = Field::None;
        self.ensure_valid_selection();
    }

    /// TOML path of the selection, e.g. `workspaces[0].tabs[0].panes[1]`.
    pub fn path_label(&self) -> String {
        let mut s = format!("workspaces[{}].tabs[{}]", self.ws, self.tab);
        for i in &self.sel {
            s.push_str(&format!(".panes[{i}]"));
        }
        s
    }

    /// Nesting depth the selected leaf's split would have (a split tab is depth 1).
    pub fn depth(&self) -> usize {
        self.root().map_or(0, |r| depth_of(&r))
    }

    pub fn can_split(&self) -> bool {
        self.pane()
            .is_some_and(|p| p.split.is_none() && self.sel.len() < MAX_SPLIT_DEPTH)
    }

    /// Split right (`Horizontal`) or down (`Vertical`): the old pane becomes the first child.
    pub fn split(&mut self, kind: SplitKind) {
        self.field = Field::None;
        if !self.can_split() {
            return;
        }
        let sel = self.sel.clone();
        self.with_root(|r| {
            if let Some(n) = node(r, &sel) {
                let first = WorkspacePane {
                    cwd: n.cwd.take(),
                    command: std::mem::take(&mut n.command),
                    ..Default::default()
                };
                n.split = Some(kind);
                n.percent = None;
                n.panes = vec![first, WorkspacePane::default()];
            }
        });
        self.sel.push(1);
    }

    /// Closing a pane lets its sibling replace the parent split.
    pub fn close_pane(&mut self) {
        self.field = Field::None;
        let Some((&last, parent)) = self.sel.split_last() else {
            return;
        };
        let parent = parent.to_vec();
        self.with_root(|r| {
            if let Some(p) = node(r, &parent) {
                if p.panes.len() == 2 {
                    let sibling = p.panes.remove(1 - last);
                    let inherited = p.cwd.take();
                    *p = sibling;
                    if p.split.is_none() && p.cwd.is_none() {
                        p.cwd = inherited;
                    }
                }
            }
        });
        self.sel = parent;
    }

    pub fn swap(&mut self) {
        self.field = Field::None;
        let Some((_, parent)) = self.sel.split_last() else {
            return;
        };
        let parent = parent.to_vec();
        self.with_root(|r| {
            if let Some(p) = node(r, &parent) {
                p.panes.swap(0, 1);
            }
        });
        let n = self.sel.len() - 1;
        self.sel[n] = 1 - self.sel[n];
    }

    /// Sets the first child's share of the split at `path` (snapped to 5).
    pub fn set_percent(&mut self, path: &[usize], percent: i32) {
        let snapped = snap_percent(percent);
        self.with_root(|r| {
            if let Some(n) = node(r, path) {
                if n.split.is_some() {
                    n.percent = (snapped != 50).then_some(snapped);
                }
            }
        });
    }

    pub fn nudge_percent(&mut self, path: &[usize], delta: i32) {
        let cur = self
            .root()
            .and_then(|mut r| node(&mut r, path).map(|n| n.percent.unwrap_or(50)))
            .unwrap_or(50);
        self.set_percent(path, i32::from(cur) + delta);
    }

    // ---- fields

    /// Fields reachable with Tab, in visual order.
    fn tab_order(&self) -> Vec<Field> {
        let mut v = vec![Field::Name, Field::TabTitle, Field::TabCwd];
        if self.pane().is_some_and(|p| p.split.is_none()) {
            v.extend([Field::Cwd, Field::Command]);
        }
        v
    }

    /// Moves editing to the next (or previous) text field, wrapping around.
    pub fn cycle_field(&mut self, back: bool) {
        let order = self.tab_order();
        let n = order.len();
        let next = match order.iter().position(|f| *f == self.field) {
            Some(i) => (i + if back { n - 1 } else { 1 }) % n,
            None if back => n - 1,
            None => 0,
        };
        self.begin_field(order[next]);
    }

    /// Starts editing `field` with its current text.
    pub fn begin_field(&mut self, field: Field) {
        let text = match field {
            Field::None => String::new(),
            Field::Name => self
                .workspaces
                .get(self.ws)
                .map_or_else(String::new, |w| w.name.clone()),
            Field::TabTitle => self
                .tab_ref()
                .and_then(|t| t.title.clone())
                .unwrap_or_default(),
            Field::TabCwd => self
                .tab_ref()
                .and_then(|t| t.cwd.clone())
                .unwrap_or_default(),
            Field::Cwd => self.pane().and_then(|p| p.cwd).unwrap_or_default(),
            Field::Command => self.pane().map(|p| p.command.join(" ")).unwrap_or_default(),
        };
        self.field = field;
        self.edit = TextEdit::new(text);
    }

    /// Writes the edit buffer back into the model (called after every edit).
    pub fn commit_field(&mut self) {
        let text = self.edit.text().to_string();
        let opt = (!text.trim().is_empty()).then(|| text.clone());
        let sel = self.sel.clone();
        match self.field {
            Field::None => {}
            Field::Name => self.rename_workspace(text),
            Field::TabTitle => {
                if let Some(t) = self
                    .workspaces
                    .get_mut(self.ws)
                    .and_then(|w| w.tabs.get_mut(self.tab))
                {
                    t.title = opt;
                }
            }
            Field::TabCwd => {
                if let Some(t) = self
                    .workspaces
                    .get_mut(self.ws)
                    .and_then(|w| w.tabs.get_mut(self.tab))
                {
                    t.cwd = opt;
                }
            }
            Field::Cwd => {
                self.with_root(|r| {
                    if let Some(n) = node(r, &sel) {
                        n.cwd = opt;
                    }
                });
            }
            Field::Command => {
                // An unparsable string (open quote) keeps the previous command until fixed.
                if let Ok(argv) = split_command(&text) {
                    self.with_root(|r| {
                        if let Some(n) = node(r, &sel) {
                            n.command = argv;
                        }
                    });
                }
            }
        }
    }

    // ---- preview and save

    pub fn preview(&self) -> Vec<(DiffTag, String)> {
        // Only the workspace section: render both sides into an empty document.
        let (old_ws, old_start) = &self.original;
        match (
            edit::patch("", old_ws, old_start),
            edit::patch("", &self.workspaces, &self.startup),
        ) {
            (Ok(old), Ok(new)) => edit::preview(&old, &new),
            _ => Vec::new(),
        }
    }

    pub fn save(&self, path: &std::path::Path) -> Result<String, EditError> {
        if let Some(e) = &self.read_only {
            return Err(EditError::Unparsable(e.clone()));
        }
        edit::save(path, self.opened_hash, &self.workspaces, &self.startup)
    }
}

fn depth_of(n: &WorkspacePane) -> usize {
    if n.split.is_none() {
        0
    } else {
        1 + n.panes.iter().map(depth_of).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Designer {
        let mut d = Designer::open(String::new(), None);
        d.new_workspace();
        d
    }

    #[test]
    fn split_close_swap_and_percent_produce_expected_defs() {
        let mut d = fresh();
        d.split(SplitKind::Horizontal);
        assert_eq!(d.sel, vec![1]);
        d.split(SplitKind::Vertical);
        assert_eq!(d.path_label(), "workspaces[0].tabs[0].panes[1].panes[1]");
        d.set_percent(&[], 58);
        d.set_percent(&[1], 12);
        let t = d.tab_ref().unwrap().clone();
        assert_eq!(t.split, Some(SplitKind::Horizontal));
        assert_eq!(t.percent, Some(60));
        assert_eq!(t.panes[1].percent, Some(10));
        d.swap();
        assert_eq!(d.sel, vec![1, 0]);
        d.close_pane();
        let t = d.tab_ref().unwrap();
        assert_eq!(t.panes[1].split, None);
        assert_eq!(d.sel, vec![1]);
        d.close_pane();
        assert_eq!(d.tab_ref().unwrap().split, None);
        assert!(d.tab_ref().unwrap().panes.is_empty());
    }

    #[test]
    fn structural_edits_end_field_editing() {
        let mut d = fresh();
        d.begin_field(Field::TabTitle);
        d.edit.insert("prod");
        d.commit_field();
        d.new_tab();
        assert_eq!(d.field, Field::None);
        d.select_tab(0);
        d.begin_field(Field::TabTitle);
        assert_eq!(d.edit.text(), "prod");
        d.select_tab(1);
        assert_eq!(d.field, Field::None);
        assert_eq!(d.tab_ref().unwrap().title, None);
    }

    #[test]
    fn tab_cycles_through_fields_and_wraps() {
        let mut d = fresh();
        d.cycle_field(false);
        assert_eq!(d.field, Field::Name);
        for f in [
            Field::TabTitle,
            Field::TabCwd,
            Field::Cwd,
            Field::Command,
            Field::Name,
        ] {
            d.cycle_field(false);
            assert_eq!(d.field, f);
        }
        d.cycle_field(true);
        assert_eq!(d.field, Field::Command);
    }

    #[test]
    fn depth_cap_disables_split() {
        let mut d = fresh();
        for _ in 0..MAX_SPLIT_DEPTH {
            assert!(d.can_split());
            d.split(SplitKind::Horizontal);
        }
        assert!(!d.can_split());
        assert_eq!(d.depth(), MAX_SPLIT_DEPTH);
        assert!(d.issues().is_empty());
    }

    #[test]
    fn save_is_disabled_on_errors_and_enabled_when_fixed() {
        let mut d = fresh();
        d.new_workspace();
        d.rename_workspace("workspace-1".into());
        assert!(!d.can_save());
        d.rename_workspace("api".into());
        assert!(d.can_save());
        assert!(d.dirty());
    }

    #[test]
    fn fields_commit_and_inherit_command_parsing() {
        let mut d = fresh();
        d.begin_field(Field::Command);
        d.edit.insert("cargo watch -x 'test all'");
        d.commit_field();
        assert_eq!(
            d.pane().unwrap().command,
            vec!["cargo", "watch", "-x", "test all"]
        );
    }

    #[test]
    fn save_writes_nested_form_and_leaves_rest_untouched() {
        let dir = std::env::temp_dir().join(format!("mt_designer_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("microterm.toml");
        let src = "# top\nfont_size = 13\n";
        std::fs::write(&p, src).unwrap();
        let mut d = Designer::open(src.into(), None);
        d.new_workspace();
        d.rename_workspace("shop".into());
        d.toggle_startup(0);
        d.split(SplitKind::Horizontal);
        let out = d.save(&p).unwrap();
        assert!(out.starts_with(src));
        assert!(out.contains("[[workspaces.tabs.panes]]"));
        assert!(out.contains("startup_workspace = \"shop\""));
        // A hand edit after opening makes the next save refuse.
        std::fs::write(&p, "other = 1\n").unwrap();
        assert!(matches!(d.save(&p), Err(EditError::Stale)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
