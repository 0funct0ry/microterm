//! The Mux → Workspace → Window → Tab → PaneTree → Pane model.

use std::path::PathBuf;

use mt_proto::{NotifyLevel, StatusState};

use crate::domain::{
    Domain, DomainId, DomainKind, DomainState, LocalDomain, SshDomain, LOCAL_DOMAIN_ID,
};
use crate::status::{sanitize, NotifyLimiter, Status, StatusSource};

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
    /// The pane's id inside its domain's daemon (`(domain_id, remote_pane_id)`, SPEC §5);
    /// `None` for local panes.
    pub remote_pane_id: Option<u64>,
    pub title: String,
    pub cwd: Option<PathBuf>,
    /// Set by `ctl pane status`; cleared on exit and TTL expiry (SPEC §12.4).
    pub status: Option<Status>,
    /// Attention badge raised by `ctl pane notify`, cleared when the pane is focused.
    pub attention: Option<NotifyLevel>,
}

/// Tree operations live in `tree.rs`; `Axis::Horizontal` puts `a` left of `b` (SPEC §17.1).
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

impl Pane {
    pub fn new(id: Id, domain_id: DomainId, cwd: Option<PathBuf>) -> Self {
        Pane {
            id,
            domain_id,
            remote_pane_id: None,
            title: String::new(),
            cwd,
            status: None,
            attention: None,
        }
    }
}

impl PaneTree {
    /// The first leaf in layout order. With only leaves in v1 this is the single pane.
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
    /// The focused pane of this tab; always a leaf of `tree`.
    pub active_pane: Id,
    /// The pane shown full-size while the tab is zoomed (SPEC §17.1).
    pub zoomed: Option<Id>,
}

impl Tab {
    /// The focused pane.
    pub fn pane(&self) -> &Pane {
        self.tree
            .find(self.active_pane)
            .unwrap_or_else(|| self.tree.first_pane())
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

/// Where a notification came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifySource {
    /// `microterm ctl pane notify`.
    Ctl,
    /// A program wrote OSC 9.
    Osc9,
}

impl NotifySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ctl => "ctl",
            Self::Osc9 => "osc9",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    PaneAdded {
        pane: Id,
    },
    PaneRemoved {
        pane: Id,
    },
    /// The tab's focused pane changed.
    PaneFocused {
        pane: Id,
    },
    /// Splits, ratios, pane order or zoom of a tab changed.
    LayoutChanged {
        tab: Id,
    },
    /// Output arrived; carries no data (wake only).
    PaneOutput {
        pane: Id,
    },
    TitleChanged {
        pane: Id,
    },
    CwdChanged {
        pane: Id,
    },
    /// The pane's status was set, changed, cleared or expired.
    StatusChanged {
        pane: Id,
    },
    /// `ctl pane notify` got through the rate limit.
    Notification {
        pane: Id,
        level: NotifyLevel,
        title: Option<String>,
        message: String,
        source: NotifySource,
    },
    /// The pane's process ended (`None` status: killed by a signal).
    PaneExited {
        pane: Id,
        status: Option<i32>,
    },
    /// OSC 133 `D`: a command finished.
    CommandFinished {
        pane: Id,
        exit_code: Option<i32>,
    },
    /// A context-menu item with a side effect ran (SPEC §10.9.5, audit).
    MenuInvoked {
        pane: Id,
        menu: String,
        item: String,
        kind: String,
        summary: String,
    },
    /// A pane's shell-registered menu items changed (SPEC §10.9.7).
    MenuItemsChanged {
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
    WorkspaceAdded {
        workspace: Id,
    },
    WorkspaceRenamed {
        workspace: Id,
    },
    WorkspaceSwitched {
        from: Id,
        to: Id,
    },
    WorkspaceRemoved {
        workspace: Id,
    },
    DomainStateChanged {
        domain: DomainId,
        state: DomainState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("no such window {0}")]
    NoWindow(Id),
    #[error("no such tab {0}")]
    NoTab(Id),
    #[error("no such pane {0}")]
    NoPane(Id),
    #[error("no such workspace '{0}'")]
    NoWorkspace(String),
    #[error("workspace '{0}' already exists")]
    DuplicateWorkspace(String),
    #[error("no such domain {0}")]
    NoDomain(DomainId),
    #[error("workspace name cannot be empty")]
    EmptyName,
    #[error("the '{0}' workspace cannot be renamed")]
    ProtectedWorkspace(String),
    #[error("the '{0}' workspace cannot be deleted")]
    UndeletableWorkspace(String),
    #[error("'{0}' is the active workspace; switch to another one before deleting it")]
    ActiveWorkspace(String),
}

type Listener = Box<dyn FnMut(&MuxEvent) + Send>;

struct DomainRec {
    domain: Box<dyn Domain>,
    state: DomainState,
}

pub struct Mux {
    next_id: Id,
    domains: Vec<DomainRec>,
    workspaces: Vec<Workspace>,
    windows: Vec<Window>,
    active_workspace: Id,
    /// The window `ctl` treats as focused; `None` means the first window of the active
    /// workspace. Always belongs to the active workspace when read through
    /// [`Mux::focused_window`].
    focused_window: Option<Id>,
    listeners: Vec<Listener>,
    limiter: NotifyLimiter,
}

impl Default for Mux {
    fn default() -> Self {
        Self::new()
    }
}

impl Mux {
    pub fn new() -> Self {
        let mut mux = Mux {
            // Id 0 is the local domain; every other id (domains included) comes from `alloc`.
            next_id: 1,
            domains: vec![DomainRec {
                domain: Box::new(LocalDomain),
                state: DomainState::Connected,
            }],
            workspaces: Vec::new(),
            windows: Vec::new(),
            active_workspace: 0,
            focused_window: None,
            listeners: Vec::new(),
            limiter: NotifyLimiter::default(),
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
        self.emit(MuxEvent::WorkspaceAdded { workspace: id });
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
        self.emit(MuxEvent::WorkspaceRenamed { workspace: id });
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

    /// Deletes a workspace and closes its windows. The `default` and the active workspace are
    /// refused. Returns the ids of the panes that went away (the host stops their terminals
    /// first; see [`Mux::workspace_panes`]).
    pub fn delete_workspace(&mut self, name: &str) -> Result<Vec<Id>, Error> {
        let ws = self
            .workspace_by_name(name)
            .ok_or_else(|| Error::NoWorkspace(name.into()))?
            .clone();
        if ws.name == DEFAULT_WORKSPACE {
            return Err(Error::UndeletableWorkspace(ws.name));
        }
        if ws.id == self.active_workspace {
            return Err(Error::ActiveWorkspace(ws.name));
        }
        let panes = self.workspace_panes(ws.id);
        let windows: Vec<Id> = self.windows_in(ws.id).map(|w| w.id).collect();
        for w in windows {
            self.close_window(w)?;
        }
        self.workspaces.retain(|w| w.id != ws.id);
        self.emit(MuxEvent::WorkspaceRemoved { workspace: ws.id });
        Ok(panes)
    }

    /// Every pane in the windows of `workspace`.
    pub fn workspace_panes(&self, workspace: Id) -> Vec<Id> {
        self.windows_in(workspace)
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.tree.panes())
            .map(|p| p.id)
            .collect()
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

    /// The focused window: the one `focus_window` chose if it is in the active workspace,
    /// otherwise the first window of the active workspace.
    pub fn focused_window(&self) -> Option<Id> {
        self.focused_window
            .and_then(|id| self.window(id))
            .filter(|w| w.workspace_id == self.active_workspace)
            .map(|w| w.id)
            .or_else(|| self.windows_in(self.active_workspace).next().map(|w| w.id))
    }

    /// Marks `id` as the focused window. Its workspace becomes the active one.
    pub fn focus_window(&mut self, id: Id) -> Result<(), Error> {
        let ws = self.window(id).ok_or(Error::NoWindow(id))?.workspace_id;
        if ws != self.active_workspace {
            let name = self
                .workspaces
                .iter()
                .find(|w| w.id == ws)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            self.switch_workspace(&name)?;
        }
        self.focused_window = Some(id);
        Ok(())
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
        if self.focused_window == Some(id) {
            self.focused_window = None;
        }
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

    // ---- domains ----

    pub fn domains(&self) -> impl Iterator<Item = (&dyn Domain, &DomainState)> {
        self.domains.iter().map(|d| (d.domain.as_ref(), &d.state))
    }

    pub fn domain_by_name(&self, name: &str) -> Option<DomainId> {
        self.domains
            .iter()
            .find(|d| d.domain.name() == name)
            .map(|d| d.domain.id())
    }

    pub fn domain_state(&self, id: DomainId) -> Option<&DomainState> {
        self.domains
            .iter()
            .find(|d| d.domain.id() == id)
            .map(|d| &d.state)
    }

    pub fn domain_kind(&self, id: DomainId) -> Option<DomainKind> {
        self.domains
            .iter()
            .find(|d| d.domain.id() == id)
            .map(|d| d.domain.kind())
    }

    /// Registers an SSH domain (or returns the existing one with that name). New domains start
    /// `Connecting`.
    pub fn register_ssh_domain(&mut self, name: &str) -> DomainId {
        if let Some(id) = self.domain_by_name(name) {
            return id;
        }
        let id = self.alloc();
        self.domains.push(DomainRec {
            domain: Box::new(SshDomain::new(id, name)),
            state: DomainState::Connecting,
        });
        self.emit(MuxEvent::DomainStateChanged {
            domain: id,
            state: DomainState::Connecting,
        });
        id
    }

    pub fn set_domain_state(&mut self, id: DomainId, state: DomainState) -> Result<(), Error> {
        let rec = self
            .domains
            .iter_mut()
            .find(|d| d.domain.id() == id)
            .ok_or(Error::NoDomain(id))?;
        if rec.state != state {
            rec.state = state.clone();
            self.emit(MuxEvent::DomainStateChanged { domain: id, state });
        }
        Ok(())
    }

    /// The domain `pane` lives in.
    pub fn pane_domain(&self, pane: Id) -> Option<DomainId> {
        self.windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.tree.panes())
            .find(|p| p.id == pane)
            .map(|p| p.domain_id)
    }

    /// Records the daemon-side id of a remote pane.
    pub fn set_remote_pane_id(&mut self, pane: Id, remote: u64) {
        if let Some(p) = self.pane_mut(pane) {
            p.remote_pane_id = Some(remote);
        }
    }

    /// Panes living in `domain`.
    pub fn panes_in_domain(&self, domain: DomainId) -> Vec<Id> {
        self.windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.tree.panes())
            .filter(|p| p.domain_id == domain)
            .map(|p| p.id)
            .collect()
    }

    /// Open a local tab after the active one and activate it. Returns `(tab_id, pane_id)`.
    pub fn spawn_tab(
        &mut self,
        window: Id,
        cwd: Option<PathBuf>,
        title: Option<String>,
    ) -> Result<(Id, Id), Error> {
        self.spawn_tab_in(window, LOCAL_DOMAIN_ID, cwd, title)
    }

    /// Like [`Mux::spawn_tab`], in `domain`.
    pub fn spawn_tab_in(
        &mut self,
        window: Id,
        domain: DomainId,
        cwd: Option<PathBuf>,
        title: Option<String>,
    ) -> Result<(Id, Id), Error> {
        self.window(window).ok_or(Error::NoWindow(window))?;
        self.domain_state(domain).ok_or(Error::NoDomain(domain))?;
        let tab_id = self.alloc();
        let pane_id = self.alloc();
        let pane = Pane::new(pane_id, domain, cwd);
        let tab = Tab {
            id: tab_id,
            title_override: title,
            tree: PaneTree::Leaf(pane),
            active_pane: pane_id,
            zoomed: None,
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
        let pane = t.pane().id;
        self.emit(MuxEvent::TitleChanged { pane });
        Ok(())
    }

    // ---- panes ----

    fn pane_mut(&mut self, pane: Id) -> Option<&mut Pane> {
        self.windows
            .iter_mut()
            .flat_map(|w| w.tabs.iter_mut())
            .find_map(|t| t.tree.find_mut(pane))
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
            if p.cwd.as_ref() != Some(&cwd) {
                p.cwd = Some(cwd);
                self.emit(MuxEvent::CwdChanged { pane });
            }
        }
    }

    pub fn notify_output(&mut self, pane: Id) {
        self.emit(MuxEvent::PaneOutput { pane });
    }

    pub fn notify_exited(&mut self, pane: Id, status: Option<i32>) {
        // A dead process cannot be "working": clear before announcing the exit.
        self.clear_pane_status(pane);
        self.emit(MuxEvent::PaneExited { pane, status });
    }

    // ---- status, attention and notifications (M10.3) ----

    pub fn pane(&self, pane: Id) -> Option<&Pane> {
        self.windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.tree.panes())
            .find(|p| p.id == pane)
    }

    /// Sets the pane's status. `text` is sanitised and cut to `max_len` characters, `progress` is
    /// clamped to 100. Emits `StatusChanged` only when state, text or progress changed (a TTL
    /// refresh alone is silent). Returns `false` for an unknown pane.
    #[allow(clippy::too_many_arguments)]
    pub fn set_pane_status(
        &mut self,
        pane: Id,
        state: StatusState,
        text: &str,
        progress: Option<u8>,
        ttl_ms: Option<u64>,
        max_len: usize,
        now_ms: u64,
    ) -> bool {
        let new = Status {
            source: StatusSource::Ctl,
            state,
            text: sanitize(text, max_len),
            progress: progress.map(|p| p.min(100)),
            updated_at_ms: now_ms,
            ttl_deadline_ms: ttl_ms.map(|t| now_ms.saturating_add(t)),
        };
        let Some(p) = self.pane_mut(pane) else {
            return false;
        };
        let changed = match &p.status {
            Some(old) => {
                (old.state, &old.text, old.progress) != (new.state, &new.text, new.progress)
            }
            None => true,
        };
        p.status = Some(new);
        if changed {
            self.emit(MuxEvent::StatusChanged { pane });
        }
        true
    }

    /// Applies an OSC 9;4 progress report as the pane's status. A live `ctl pane status` wins:
    /// it is left alone and `false` is returned. `value` `None` means no progress bar.
    pub fn set_osc_progress(
        &mut self,
        pane: Id,
        state: StatusState,
        value: Option<u8>,
        now_ms: u64,
    ) -> bool {
        let Some(p) = self.pane_mut(pane) else {
            return false;
        };
        if p.status
            .as_ref()
            .is_some_and(|s| s.source == StatusSource::Ctl)
        {
            return false;
        }
        let new = Status {
            source: StatusSource::Osc,
            state,
            text: String::new(),
            progress: value.map(|v| v.min(100)),
            updated_at_ms: now_ms,
            ttl_deadline_ms: None,
        };
        let changed = p
            .status
            .as_ref()
            .is_none_or(|old| (old.state, old.progress) != (new.state, new.progress));
        p.status = Some(new);
        if changed {
            self.emit(MuxEvent::StatusChanged { pane });
        }
        true
    }

    /// Ends an OSC 9;4 progress report. A status set through `ctl` is never cleared by it.
    pub fn clear_osc_progress(&mut self, pane: Id) -> bool {
        let osc = self
            .pane(pane)
            .and_then(|p| p.status.as_ref())
            .is_some_and(|s| s.source == StatusSource::Osc);
        osc && self.clear_pane_status(pane)
    }

    /// Clears the status. Returns whether there was one.
    pub fn clear_pane_status(&mut self, pane: Id) -> bool {
        let had = self
            .pane_mut(pane)
            .is_some_and(|p| p.status.take().is_some());
        if had {
            self.emit(MuxEvent::StatusChanged { pane });
        }
        had
    }

    /// Clears every status whose TTL has passed; returns those panes.
    pub fn expire_statuses(&mut self, now_ms: u64) -> Vec<Id> {
        let due: Vec<Id> = self
            .windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.tree.panes())
            .filter(|p| {
                p.status
                    .as_ref()
                    .and_then(|s| s.ttl_deadline_ms)
                    .is_some_and(|d| d <= now_ms)
            })
            .map(|p| p.id)
            .collect();
        for &pane in &due {
            self.clear_pane_status(pane);
        }
        due
    }

    /// Earliest TTL deadline, so a host can sleep until then.
    pub fn next_status_deadline(&self) -> Option<u64> {
        self.windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.tree.panes())
            .filter_map(|p| p.status.as_ref().and_then(|s| s.ttl_deadline_ms))
            .min()
    }

    /// Raises the attention badge on `pane`; a higher level is never downgraded.
    pub fn raise_attention(&mut self, pane: Id, level: NotifyLevel) {
        if let Some(p) = self.pane_mut(pane) {
            let rank = |l: NotifyLevel| match l {
                NotifyLevel::Info => 0,
                NotifyLevel::Attention => 1,
                NotifyLevel::Error => 2,
            };
            if p.attention.is_none_or(|cur| rank(level) > rank(cur)) {
                p.attention = Some(level);
            }
        }
    }

    /// Clears the badge; returns whether one was showing.
    pub fn clear_attention(&mut self, pane: Id) -> bool {
        self.pane_mut(pane)
            .is_some_and(|p| p.attention.take().is_some())
    }

    /// Runs `pane notify` through the per-pane rate limit. On success emits `Notification` and
    /// returns `true`; otherwise the notification is dropped and counted.
    pub fn notify_pane(
        &mut self,
        pane: Id,
        level: NotifyLevel,
        title: Option<String>,
        message: String,
        source: NotifySource,
        now_ms: u64,
    ) -> bool {
        if !self.limiter.allow(pane, now_ms) {
            return false;
        }
        self.emit(MuxEvent::Notification {
            pane,
            level,
            title,
            message,
            source,
        });
        true
    }

    /// Notifications for `pane` dropped by the rate limit so far.
    pub fn notifications_dropped(&self, pane: Id) -> u32 {
        self.limiter.dropped(pane)
    }

    pub fn notify_command_finished(&mut self, pane: Id, exit_code: Option<i32>) {
        self.emit(MuxEvent::CommandFinished { pane, exit_code });
    }

    /// A pane's shell-registered menu items changed (`OSC 7717`).
    pub fn notify_menu_items_changed(&mut self, pane: Id) {
        if self.pane(pane).is_some() {
            self.emit(MuxEvent::MenuItemsChanged { pane });
        }
    }

    /// Records a context-menu invocation for `ctl events` (SPEC §10.9.5).
    pub fn notify_menu_invoked(
        &mut self,
        pane: Id,
        menu: &str,
        item: &str,
        kind: &str,
        summary: &str,
    ) {
        self.emit(MuxEvent::MenuInvoked {
            pane,
            menu: menu.into(),
            item: item.into(),
            kind: kind.into(),
            summary: summary.into(),
        });
    }

    /// `(window id, tab index)` of the tab with id `tab`.
    pub fn tab_location(&self, tab: Id) -> Option<(Id, usize)> {
        self.windows
            .iter()
            .find_map(|w| w.tabs.iter().position(|t| t.id == tab).map(|i| (w.id, i)))
    }

    /// Removes `pane`; see [`Mux::close_pane`]. Returns `(window, tab, window_closed)`.
    pub fn kill_pane(&mut self, pane: Id) -> Result<(Id, Id, bool), Error> {
        Ok(match self.close_pane(pane)? {
            PaneClosed::Pane { window, tab } => (window, tab, false),
            PaneClosed::Tab {
                window,
                tab,
                window_closed,
            } => (window, tab, window_closed),
        })
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

mod panes;
pub use panes::PaneClosed;

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
    fn delete_workspace_closes_its_windows_and_protects_default_and_active() {
        let mut mux = Mux::new();
        let log = recorded(&mut mux);
        let api = mux.create_workspace("api").unwrap();
        mux.switch_workspace("api").unwrap();
        let w = mux.spawn_window(None);
        let (_, pane) = mux.spawn_tab(w, None, None).unwrap();
        mux.switch_workspace("default").unwrap();
        assert_eq!(mux.workspace_panes(api), [pane]);
        assert_eq!(
            mux.delete_workspace("default"),
            Err(Error::UndeletableWorkspace("default".into()))
        );
        mux.switch_workspace("api").unwrap();
        assert_eq!(
            mux.delete_workspace("api"),
            Err(Error::ActiveWorkspace("api".into()))
        );
        mux.switch_workspace("default").unwrap();
        assert_eq!(
            mux.delete_workspace("ghost"),
            Err(Error::NoWorkspace("ghost".into()))
        );
        log.lock().unwrap().clear();
        assert_eq!(mux.delete_workspace("api").unwrap(), [pane]);
        assert!(mux.window(w).is_none() && mux.workspace_by_name("api").is_none());
        let events = log.lock().unwrap().clone();
        assert_eq!(
            events.last(),
            Some(&MuxEvent::WorkspaceRemoved { workspace: api })
        );
        assert!(events.contains(&MuxEvent::WindowRemoved { window: w }));
    }

    #[test]
    fn focus_follows_focus_window_and_falls_back_to_the_first_window() {
        let mut mux = Mux::new();
        let w1 = mux.spawn_window(None);
        let w2 = mux.spawn_window(None);
        assert_eq!(
            mux.focused_window(),
            Some(w1),
            "defaults to the first window"
        );
        mux.focus_window(w2).unwrap();
        assert_eq!(mux.focused_window(), Some(w2));
        mux.close_window(w2).unwrap();
        assert_eq!(mux.focused_window(), Some(w1));
        // Focusing a window of another workspace switches to it.
        mux.create_workspace("api").unwrap();
        mux.switch_workspace("api").unwrap();
        let w3 = mux.spawn_window(None);
        mux.switch_workspace("default").unwrap();
        mux.focus_window(w3).unwrap();
        assert_eq!(mux.active_workspace().name, "api");
        assert_eq!(mux.focused_window(), Some(w3));
        assert_eq!(mux.focus_window(999), Err(Error::NoWindow(999)));
    }

    #[test]
    fn ssh_domains_register_once_with_monotonic_ids_and_state_events() {
        let mut mux = Mux::new();
        let log = recorded(&mut mux);
        assert_eq!(
            mux.domain_state(LOCAL_DOMAIN_ID),
            Some(&DomainState::Connected)
        );
        let prod = mux.register_ssh_domain("prod");
        assert_ne!(prod, LOCAL_DOMAIN_ID);
        assert_eq!(
            mux.register_ssh_domain("prod"),
            prod,
            "same name, same domain"
        );
        let infra = mux.register_ssh_domain("infra");
        assert!(infra > prod, "ids are monotonic and never reused");
        assert_eq!(mux.domain_by_name("infra"), Some(infra));
        assert_eq!(
            mux.domain_kind(prod),
            Some(DomainKind::Ssh {
                name: "prod".into()
            })
        );
        assert_eq!(mux.domain_state(prod), Some(&DomainState::Connecting));

        mux.set_domain_state(prod, DomainState::Connected).unwrap();
        mux.set_domain_state(prod, DomainState::Connected).unwrap(); // no change, no event
        mux.set_domain_state(
            prod,
            DomainState::Reconnecting {
                attempt: 3,
                next_in_secs: 4,
            },
        )
        .unwrap();
        assert!(mux.set_domain_state(999, DomainState::Connected).is_err());
        let states: Vec<_> = log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                MuxEvent::DomainStateChanged { domain, state } if *domain == prod => {
                    Some(state.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            states,
            vec![
                DomainState::Connecting,
                DomainState::Connected,
                DomainState::Reconnecting {
                    attempt: 3,
                    next_in_secs: 4
                }
            ]
        );
    }

    #[test]
    fn remote_tabs_record_their_domain_and_remote_pane_id() {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let prod = mux.register_ssh_domain("prod");
        let (_, local) = mux.spawn_tab(w, None, None).unwrap();
        let (_, remote) = mux.spawn_tab_in(w, prod, None, None).unwrap();
        mux.set_remote_pane_id(remote, 42);
        let p = mux
            .window(w)
            .unwrap()
            .tabs
            .iter()
            .flat_map(|t| t.tree.panes())
            .find(|p| p.id == remote)
            .unwrap()
            .clone();
        assert_eq!((p.domain_id, p.remote_pane_id), (prod, Some(42)));
        assert_eq!(mux.panes_in_domain(prod), vec![remote]);
        assert_eq!(mux.panes_in_domain(LOCAL_DOMAIN_ID), vec![local]);
        assert_eq!(
            mux.spawn_tab_in(w, 777, None, None),
            Err(Error::NoDomain(777))
        );
    }

    #[test]
    fn kill_pane_closes_tab_then_window() {
        let (mut mux, w, tabs) = mux_with_tabs(2);
        let log = recorded(&mut mux);
        let pane = mux.window(w).unwrap().tabs[0].pane().id;
        assert_eq!(mux.kill_pane(pane).unwrap(), (w, tabs[0], false));
        assert_eq!(mux.tab_location(tabs[1]), Some((w, 0)));
        assert_eq!(mux.tab_location(tabs[0]), None);
        let last = mux.window(w).unwrap().tabs[0].pane().id;
        assert_eq!(mux.kill_pane(last).unwrap(), (w, tabs[1], true));
        assert!(mux.window(w).is_none());
        assert_eq!(mux.kill_pane(last), Err(Error::NoPane(last)));
        assert!(log
            .lock()
            .unwrap()
            .contains(&MuxEvent::PaneRemoved { pane }));
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
        let x = mux.create_workspace("x").unwrap();
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
                MuxEvent::WorkspaceAdded { workspace: x },
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
        assert_eq!(
            log.lock().unwrap().len(),
            3,
            "title, cwd, and nothing for the repeat"
        );
        mux.rename_tab(w, tabs[0], Some("  ".into())).unwrap();
        assert_eq!(mux.window(w).unwrap().tabs[0].title_override, None);
        assert_eq!(mux.locate_pane(pane), Some((w, 0)));
    }

    fn first_pane(mux: &Mux) -> Id {
        mux.windows()[0].tabs[0].pane().id
    }

    #[test]
    fn status_state_machine_with_ttl_on_a_fake_clock() {
        let (mut mux, _, _) = mux_with_tabs(1);
        let log = recorded(&mut mux);
        let p = first_pane(&mux);
        assert!(mux.set_pane_status(
            p,
            StatusState::Working,
            "build",
            Some(40),
            Some(30_000),
            120,
            1_000
        ));
        let st = mux.pane(p).unwrap().status.clone().unwrap();
        assert_eq!(
            (st.state, st.progress, st.ttl_deadline_ms),
            (StatusState::Working, Some(40), Some(31_000))
        );
        // Same visible content, only the TTL moves: no new event.
        assert!(mux.set_pane_status(
            p,
            StatusState::Working,
            "build",
            Some(40),
            Some(30_000),
            120,
            5_000
        ));
        assert_eq!(mux.next_status_deadline(), Some(35_000));
        assert!(mux.expire_statuses(34_999).is_empty());
        assert_eq!(mux.expire_statuses(35_000), vec![p]);
        assert!(mux.pane(p).unwrap().status.is_none());
        let n = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, MuxEvent::StatusChanged { .. }))
            .count();
        assert_eq!(n, 2, "set + expiry; the TTL refresh was silent");
    }

    #[test]
    fn status_text_is_sanitised_and_progress_clamped() {
        let (mut mux, _, _) = mux_with_tabs(1);
        let p = first_pane(&mux);
        mux.set_pane_status(
            p,
            StatusState::Waiting,
            "a\x1b[2Jb\nc and more text",
            Some(250),
            None,
            10,
            0,
        );
        let st = mux.pane(p).unwrap().status.clone().unwrap();
        assert_eq!(st.text, "a[2Jb c a…");
        assert_eq!(st.progress, Some(100));
        assert!(!mux.set_pane_status(9999, StatusState::Done, "", None, None, 10, 0));
    }

    #[test]
    fn exit_clears_status_before_announcing_it() {
        let (mut mux, _, _) = mux_with_tabs(1);
        let p = first_pane(&mux);
        mux.set_pane_status(p, StatusState::Working, "x", None, None, 120, 0);
        let log = recorded(&mut mux);
        mux.notify_exited(p, Some(0));
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                MuxEvent::StatusChanged { pane: p },
                MuxEvent::PaneExited {
                    pane: p,
                    status: Some(0)
                }
            ]
        );
        assert!(!mux.clear_pane_status(p), "already cleared");
    }

    #[test]
    fn attention_keeps_the_highest_level_until_cleared() {
        let (mut mux, _, _) = mux_with_tabs(1);
        let p = first_pane(&mux);
        mux.raise_attention(p, NotifyLevel::Attention);
        mux.raise_attention(p, NotifyLevel::Info);
        assert_eq!(mux.pane(p).unwrap().attention, Some(NotifyLevel::Attention));
        mux.raise_attention(p, NotifyLevel::Error);
        assert_eq!(mux.pane(p).unwrap().attention, Some(NotifyLevel::Error));
        assert!(mux.clear_attention(p));
        assert!(!mux.clear_attention(p));
    }

    #[test]
    fn notifications_are_rate_limited_per_pane_and_emit_events() {
        let (mut mux, _, _) = mux_with_tabs(2);
        let log = recorded(&mut mux);
        let p = first_pane(&mux);
        let sent = (0..8)
            .filter(|_| {
                mux.notify_pane(
                    p,
                    NotifyLevel::Info,
                    None,
                    "hi".into(),
                    NotifySource::Ctl,
                    100,
                )
            })
            .count();
        assert_eq!(sent, 5);
        assert_eq!(mux.notifications_dropped(p), 3);
        let n = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, MuxEvent::Notification { .. }))
            .count();
        assert_eq!(n, 5);
    }

    #[test]
    fn cwd_and_workspace_changes_emit_events() {
        let (mut mux, _, _) = mux_with_tabs(1);
        let p = first_pane(&mux);
        let log = recorded(&mut mux);
        mux.set_pane_cwd(p, "/tmp".into());
        mux.set_pane_cwd(p, "/tmp".into());
        let w = mux.create_workspace("api").unwrap();
        mux.rename_workspace(w, "api2").unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                MuxEvent::CwdChanged { pane: p },
                MuxEvent::WorkspaceAdded { workspace: w },
                MuxEvent::WorkspaceRenamed { workspace: w },
            ]
        );
    }
}
