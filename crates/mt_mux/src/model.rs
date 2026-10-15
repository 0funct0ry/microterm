//! The Mux → Workspace → Window → Tab → PaneTree → Pane model.

use std::path::PathBuf;

use crate::domain::{DomainId, LOCAL_DOMAIN_ID};

pub type Id = u64;

pub const DEFAULT_WORKSPACE: &str = "default";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: Id,
    pub domain_id: DomainId,
    pub title: String,
    pub cwd: Option<PathBuf>,
}

/// `Split` exists in the type from day one (SPEC D4) but is unreachable until M12.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneTree {
    Leaf(Pane),
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<PaneTree>,
        b: Box<PaneTree>,
    },
}

impl PaneTree {
    /// The focused pane. With only leaves in v1 this is the single pane.
    pub fn first_pane(&self) -> &Pane {
        match self {
            PaneTree::Leaf(p) => p,
            PaneTree::Split { a, .. } => a.first_pane(),
        }
    }

    pub fn first_pane_mut(&mut self) -> &mut Pane {
        match self {
            PaneTree::Leaf(p) => p,
            PaneTree::Split { a, .. } => a.first_pane_mut(),
        }
    }

    pub fn panes(&self) -> Vec<&Pane> {
        match self {
            PaneTree::Leaf(p) => vec![p],
            PaneTree::Split { a, b, .. } => {
                let mut v = a.panes();
                v.extend(b.panes());
                v
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub id: Id,
    pub title_override: Option<String>,
    pub tree: PaneTree,
}

impl Tab {
    pub fn pane(&self) -> &Pane {
        self.tree.first_pane()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub id: Id,
    pub workspace_id: Id,
    pub title: String,
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Window {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: Id,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    PaneAdded {
        pane: Id,
    },
    PaneRemoved {
        pane: Id,
    },
    /// Output arrived; carries no data (wake only).
    PaneOutput {
        pane: Id,
    },
    TitleChanged {
        pane: Id,
    },
    TabAdded {
        window: Id,
        tab: Id,
    },
    TabRemoved {
        window: Id,
        tab: Id,
    },
    TabMoved {
        window: Id,
        tab: Id,
        from: usize,
        to: usize,
    },
    WindowAdded {
        window: Id,
    },
    WindowRemoved {
        window: Id,
    },
    WorkspaceSwitched {
        from: Id,
        to: Id,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("no such window {0}")]
    NoWindow(Id),
    #[error("no such tab {0}")]
    NoTab(Id),
    #[error("no such workspace '{0}'")]
    NoWorkspace(String),
    #[error("workspace '{0}' already exists")]
    DuplicateWorkspace(String),
    #[error("workspace name cannot be empty")]
    EmptyName,
    #[error("the '{0}' workspace cannot be renamed")]
    ProtectedWorkspace(String),
}

type Listener = Box<dyn FnMut(&MuxEvent) + Send>;

pub struct Mux {
    next_id: Id,
    workspaces: Vec<Workspace>,
    windows: Vec<Window>,
    active_workspace: Id,
    listeners: Vec<Listener>,
}

impl Default for Mux {
    fn default() -> Self {
        Self::new()
    }
}

impl Mux {
    pub fn new() -> Self {
        let mut mux = Mux {
            next_id: 1,
            workspaces: Vec::new(),
            windows: Vec::new(),
            active_workspace: 0,
            listeners: Vec::new(),
        };
        let id = mux.alloc();
        mux.workspaces.push(Workspace {
            id,
            name: DEFAULT_WORKSPACE.into(),
        });
        mux.active_workspace = id;
        mux
    }

    fn alloc(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn emit(&mut self, ev: MuxEvent) {
        for l in &mut self.listeners {
            l(&ev);
        }
    }

    pub fn subscribe(&mut self, listener: impl FnMut(&MuxEvent) + Send + 'static) {
        self.listeners.push(Box::new(listener));
    }

    // ---- workspaces ----

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn active_workspace(&self) -> &Workspace {
        self.workspaces
            .iter()
            .find(|w| w.id == self.active_workspace)
            .expect("active workspace exists")
    }

    pub fn workspace_by_name(&self, name: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.name == name)
    }

    pub fn create_workspace(&mut self, name: &str) -> Result<Id, Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::EmptyName);
        }
        if self.workspace_by_name(name).is_some() {
            return Err(Error::DuplicateWorkspace(name.into()));
        }
        let id = self.alloc();
        self.workspaces.push(Workspace {
            id,
            name: name.into(),
        });
        Ok(id)
    }

    pub fn rename_workspace(&mut self, id: Id, name: &str) -> Result<(), Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::EmptyName);
        }
        if self.workspaces.iter().any(|w| w.name == name && w.id != id) {
            return Err(Error::DuplicateWorkspace(name.into()));
        }
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or_else(|| Error::NoWorkspace(id.to_string()))?;
        if ws.name == DEFAULT_WORKSPACE {
            return Err(Error::ProtectedWorkspace(ws.name.clone()));
        }
        ws.name = name.into();
        Ok(())
    }

    /// Switch the single active workspace by name. Returns the previous workspace id.
    pub fn switch_workspace(&mut self, name: &str) -> Result<Id, Error> {
        let to = self
            .workspace_by_name(name)
            .ok_or_else(|| Error::NoWorkspace(name.into()))?
            .id;
        let from = self.active_workspace;
        if from != to {
            self.active_workspace = to;
            self.emit(MuxEvent::WorkspaceSwitched { from, to });
        }
        Ok(from)
    }

    pub fn switch_workspace_relative(&mut self, delta: isize) -> Id {
        let n = self.workspaces.len() as isize;
        let cur = self
            .workspaces
            .iter()
            .position(|w| w.id == self.active_workspace)
            .unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(n) as usize;
        let name = self.workspaces[next].name.clone();
        self.switch_workspace(&name).expect("exists");
        self.active_workspace
    }

    // ---- windows ----

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn window(&self, id: Id) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn window_mut(&mut self, id: Id) -> Result<&mut Window, Error> {
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(Error::NoWindow(id))
    }

    pub fn windows_in(&self, workspace: Id) -> impl Iterator<Item = &Window> {
        self.windows
            .iter()
            .filter(move |w| w.workspace_id == workspace)
    }

    /// (windows, tabs) in a workspace, for the switcher.
    pub fn counts(&self, workspace: Id) -> (usize, usize) {
        let ws: Vec<_> = self.windows_in(workspace).collect();
        (ws.len(), ws.iter().map(|w| w.tabs.len()).sum())
    }

    /// New window in `workspace` (the active one when `None`). Starts with no tabs.
    pub fn spawn_window(&mut self, workspace: Option<Id>) -> Id {
        let workspace_id = workspace.unwrap_or(self.active_workspace);
        let id = self.alloc();
        self.windows.push(Window {
            id,
            workspace_id,
            title: String::new(),
            tabs: Vec::new(),
            active: 0,
        });
        self.emit(MuxEvent::WindowAdded { window: id });
        id
    }

    pub fn close_window(&mut self, id: Id) -> Result<(), Error> {
        let pos = self
            .windows
            .iter()
            .position(|w| w.id == id)
            .ok_or(Error::NoWindow(id))?;
        let win = self.windows.remove(pos);
        for tab in &win.tabs {
            for p in tab.tree.panes() {
                self.emit(MuxEvent::PaneRemoved { pane: p.id });
            }
            self.emit(MuxEvent::TabRemoved {
                window: id,
                tab: tab.id,
            });
        }
        self.emit(MuxEvent::WindowRemoved { window: id });
        Ok(())
    }

    // ---- tabs ----

    /// Open a tab after the active one and activate it. Returns `(tab_id, pane_id)`.
    pub fn spawn_tab(
        &mut self,
        window: Id,
        cwd: Option<PathBuf>,
        title: Option<String>,
    ) -> Result<(Id, Id), Error> {
        self.window(window).ok_or(Error::NoWindow(window))?;
        let tab_id = self.alloc();
        let pane_id = self.alloc();
        let pane = Pane {
            id: pane_id,
            domain_id: LOCAL_DOMAIN_ID,
            title: String::new(),
            cwd,
        };
        let tab = Tab {
            id: tab_id,
            title_override: title,
            tree: PaneTree::Leaf(pane),
        };
        let win = self.window_mut(window)?;
        let at = if win.tabs.is_empty() {
            0
        } else {
            win.active + 1
        };
        win.tabs.insert(at, tab);
        win.active = at;
        self.emit(MuxEvent::PaneAdded { pane: pane_id });
        self.emit(MuxEvent::TabAdded {
            window,
            tab: tab_id,
        });
        Ok((tab_id, pane_id))
    }

    /// Close a tab. Returns `true` when the window has no tabs left (the caller decides
    /// whether to close the window).
    pub fn close_tab(&mut self, window: Id, tab: Id) -> Result<bool, Error> {
        let win = self.window_mut(window)?;
        let pos = win
            .tabs
            .iter()
            .position(|t| t.id == tab)
            .ok_or(Error::NoTab(tab))?;
        let removed = win.tabs.remove(pos);
        if win.active > pos || win.active >= win.tabs.len() {
            win.active = win.active.saturating_sub(1);
        }
        let empty = win.tabs.is_empty();
        for p in removed.tree.panes() {
            self.emit(MuxEvent::PaneRemoved { pane: p.id });
        }
        self.emit(MuxEvent::TabRemoved { window, tab });
        Ok(empty)
    }

    pub fn activate_tab(&mut self, window: Id, index: usize) -> Result<(), Error> {
        let win = self.window_mut(window)?;
        if index < win.tabs.len() {
            win.active = index;
        }
        Ok(())
    }

    pub fn activate_tab_relative(&mut self, window: Id, delta: isize) -> Result<(), Error> {
        let win = self.window_mut(window)?;
        if !win.tabs.is_empty() {
            let n = win.tabs.len() as isize;
            win.active = (win.active as isize + delta).rem_euclid(n) as usize;
        }
        Ok(())
    }

    /// Move a tab to `to`, keeping it active if it was. Used by drag-reorder.
    pub fn move_tab(&mut self, window: Id, from: usize, to: usize) -> Result<(), Error> {
        let win = self.window_mut(window)?;
        let n = win.tabs.len();
        if from >= n || to >= n || from == to {
            return Ok(());
        }
        let active_id = win.tabs[win.active].id;
        let tab = win.tabs.remove(from);
        let tab_id = tab.id;
        win.tabs.insert(to, tab);
        win.active = win.tabs.iter().position(|t| t.id == active_id).unwrap_or(0);
        self.emit(MuxEvent::TabMoved {
            window,
            tab: tab_id,
            from,
            to,
        });
        Ok(())
    }

    /// Move the active tab by `delta`, wrapping around.
    pub fn move_tab_relative(&mut self, window: Id, delta: isize) -> Result<(), Error> {
        let win = self.window_mut(window)?;
        let n = win.tabs.len() as isize;
        if n < 2 {
            return Ok(());
        }
        let from = win.active;
        let to = (from as isize + delta).rem_euclid(n) as usize;
        self.move_tab(window, from, to)
    }

    pub fn rename_tab(&mut self, window: Id, tab: Id, title: Option<String>) -> Result<(), Error> {
        let win = self.window_mut(window)?;
        let t = win
            .tabs
            .iter_mut()
            .find(|t| t.id == tab)
            .ok_or(Error::NoTab(tab))?;
        t.title_override = title.filter(|s| !s.trim().is_empty());
        let pane = t.tree.first_pane().id;
        self.emit(MuxEvent::TitleChanged { pane });
        Ok(())
    }

    // ---- panes ----

    fn pane_mut(&mut self, pane: Id) -> Option<&mut Pane> {
        self.windows
            .iter_mut()
            .flat_map(|w| w.tabs.iter_mut())
            .find(|t| t.tree.first_pane().id == pane)
            .map(|t| t.tree.first_pane_mut())
    }

    pub fn set_pane_title(&mut self, pane: Id, title: &str) {
        if let Some(p) = self.pane_mut(pane) {
            if p.title != title {
                p.title = title.into();
                self.emit(MuxEvent::TitleChanged { pane });
            }
        }
    }

    pub fn set_pane_cwd(&mut self, pane: Id, cwd: PathBuf) {
        if let Some(p) = self.pane_mut(pane) {
            p.cwd = Some(cwd);
        }
    }

    pub fn notify_output(&mut self, pane: Id) {
        self.emit(MuxEvent::PaneOutput { pane });
    }

    /// `(window id, tab index)` holding `pane`.
    pub fn locate_pane(&self, pane: Id) -> Option<(Id, usize)> {
        self.windows.iter().find_map(|w| {
            w.tabs
                .iter()
                .position(|t| t.tree.panes().iter().any(|p| p.id == pane))
                .map(|i| (w.id, i))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn recorded(mux: &mut Mux) -> Arc<Mutex<Vec<MuxEvent>>> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        mux.subscribe(move |e| l.lock().unwrap().push(e.clone()));
        log
    }

    fn mux_with_tabs(n: usize) -> (Mux, Id, Vec<Id>) {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let tabs = (0..n)
            .map(|_| mux.spawn_tab(w, None, None).unwrap().0)
            .collect();
        (mux, w, tabs)
    }

    #[test]
    fn default_workspace_exists_and_is_active() {
        let mux = Mux::new();
        assert_eq!(mux.active_workspace().name, DEFAULT_WORKSPACE);
        assert_eq!(mux.workspaces().len(), 1);
    }

    #[test]
    fn ids_are_monotonic_and_never_reused() {
        let (mut mux, w, tabs) = mux_with_tabs(3);
        let mut seen: Vec<Id> = vec![w];
        for t in &tabs {
            seen.push(*t);
        }
        mux.close_tab(w, tabs[1]).unwrap();
        let (t, p) = mux.spawn_tab(w, None, None).unwrap();
        assert!(t > *tabs.iter().max().unwrap());
        assert!(p > t);
        assert!(!seen.contains(&t));
        mux.close_window(w).unwrap();
        let w2 = mux.spawn_window(None);
        assert!(w2 > p);
    }

    #[test]
    fn spawn_tab_inserts_after_active_and_activates() {
        let (mut mux, w, tabs) = mux_with_tabs(2);
        mux.activate_tab(w, 0).unwrap();
        let (t, _) = mux.spawn_tab(w, None, None).unwrap();
        let win = mux.window(w).unwrap();
        assert_eq!(win.tabs[1].id, t);
        assert_eq!(win.active, 1);
        assert_eq!(win.tabs[0].id, tabs[0]);
        assert_eq!(win.tabs[2].id, tabs[1]);
    }

    #[test]
    fn close_tab_adjusts_active_and_reports_empty() {
        let (mut mux, w, tabs) = mux_with_tabs(2);
        assert_eq!(mux.window(w).unwrap().active, 1);
        assert!(!mux.close_tab(w, tabs[1]).unwrap());
        assert_eq!(mux.window(w).unwrap().active, 0);
        assert!(mux.close_tab(w, tabs[0]).unwrap());
        assert_eq!(mux.close_tab(w, tabs[0]), Err(Error::NoTab(tabs[0])));
    }

    #[test]
    fn closing_tab_before_active_keeps_active_tab() {
        let (mut mux, w, tabs) = mux_with_tabs(3);
        // active is the last tab
        mux.close_tab(w, tabs[0]).unwrap();
        let win = mux.window(w).unwrap();
        assert_eq!(win.active_tab().unwrap().id, tabs[2]);
    }

    #[test]
    fn relative_activation_wraps() {
        let (mut mux, w, _) = mux_with_tabs(3);
        mux.activate_tab_relative(w, 1).unwrap();
        assert_eq!(mux.window(w).unwrap().active, 0);
        mux.activate_tab_relative(w, -1).unwrap();
        assert_eq!(mux.window(w).unwrap().active, 2);
    }

    #[test]
    fn move_tab_keeps_active_tab_active() {
        let (mut mux, w, tabs) = mux_with_tabs(3);
        mux.activate_tab(w, 1).unwrap();
        mux.move_tab(w, 0, 2).unwrap();
        let win = mux.window(w).unwrap();
        assert_eq!(win.active_tab().unwrap().id, tabs[1]);
        assert_eq!(
            win.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![tabs[1], tabs[2], tabs[0]]
        );
    }

    #[test]
    fn move_tab_relative_wraps() {
        let (mut mux, w, tabs) = mux_with_tabs(3);
        mux.activate_tab(w, 2).unwrap();
        mux.move_tab_relative(w, 1).unwrap();
        let win = mux.window(w).unwrap();
        assert_eq!(win.tabs[0].id, tabs[2]);
        assert_eq!(win.active, 0);
    }

    #[test]
    fn workspace_create_switch_rename() {
        let mut mux = Mux::new();
        let api = mux.create_workspace("api").unwrap();
        mux.create_workspace("infra").unwrap();
        assert_eq!(
            mux.create_workspace("api"),
            Err(Error::DuplicateWorkspace("api".into()))
        );
        assert_eq!(mux.create_workspace("  "), Err(Error::EmptyName));
        mux.switch_workspace("api").unwrap();
        assert_eq!(mux.active_workspace().id, api);
        assert!(matches!(
            mux.switch_workspace("nope"),
            Err(Error::NoWorkspace(_))
        ));
        mux.rename_workspace(api, "backend").unwrap();
        assert_eq!(mux.active_workspace().name, "backend");
        let def = mux.workspace_by_name(DEFAULT_WORKSPACE).unwrap().id;
        assert!(matches!(
            mux.rename_workspace(def, "x"),
            Err(Error::ProtectedWorkspace(_))
        ));
    }

    #[test]
    fn workspace_relative_wraps() {
        let mut mux = Mux::new();
        mux.create_workspace("a").unwrap();
        mux.switch_workspace_relative(-1);
        assert_eq!(mux.active_workspace().name, "a");
        mux.switch_workspace_relative(1);
        assert_eq!(mux.active_workspace().name, DEFAULT_WORKSPACE);
    }

    #[test]
    fn counts_per_workspace() {
        let mut mux = Mux::new();
        let api = mux.create_workspace("api").unwrap();
        let w1 = mux.spawn_window(Some(api));
        let w2 = mux.spawn_window(Some(api));
        mux.spawn_tab(w1, None, None).unwrap();
        mux.spawn_tab(w1, None, None).unwrap();
        mux.spawn_tab(w2, None, None).unwrap();
        assert_eq!(mux.counts(api), (2, 3));
        assert_eq!(mux.counts(mux.active_workspace().id), (0, 0));
    }

    #[test]
    fn events_are_emitted_in_order() {
        let mut mux = Mux::new();
        let log = recorded(&mut mux);
        let w = mux.spawn_window(None);
        let (t, p) = mux.spawn_tab(w, None, None).unwrap();
        mux.create_workspace("x").unwrap();
        mux.switch_workspace("x").unwrap();
        mux.close_tab(w, t).unwrap();
        mux.close_window(w).unwrap();
        let from = 1;
        let to = mux.active_workspace().id;
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                MuxEvent::WindowAdded { window: w },
                MuxEvent::PaneAdded { pane: p },
                MuxEvent::TabAdded { window: w, tab: t },
                MuxEvent::WorkspaceSwitched { from, to },
                MuxEvent::PaneRemoved { pane: p },
                MuxEvent::TabRemoved { window: w, tab: t },
                MuxEvent::WindowRemoved { window: w },
            ]
        );
    }

    #[test]
    fn title_and_cwd_updates() {
        let (mut mux, w, tabs) = mux_with_tabs(1);
        let log = recorded(&mut mux);
        let pane = mux.window(w).unwrap().tabs[0].pane().id;
        mux.set_pane_title(pane, "vim");
        mux.set_pane_title(pane, "vim"); // unchanged: no second event
        mux.set_pane_cwd(pane, "/tmp".into());
        mux.rename_tab(w, tabs[0], Some("api".into())).unwrap();
        let tab = &mux.window(w).unwrap().tabs[0];
        assert_eq!(tab.pane().title, "vim");
        assert_eq!(
            tab.pane().cwd.as_deref(),
            Some(std::path::Path::new("/tmp"))
        );
        assert_eq!(tab.title_override.as_deref(), Some("api"));
        assert_eq!(log.lock().unwrap().len(), 2);
        mux.rename_tab(w, tabs[0], Some("  ".into())).unwrap();
        assert_eq!(mux.window(w).unwrap().tabs[0].title_override, None);
        assert_eq!(mux.locate_pane(pane), Some((w, 0)));
    }
}
