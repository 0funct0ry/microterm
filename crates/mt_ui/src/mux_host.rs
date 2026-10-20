//! The process-wide mux shared by every GUI window (SPEC §5).
//!
//! `mt_mux::Mux` is the model (workspaces, windows, tabs, panes). This module adds what the
//! model cannot own: the OS windows that show it and the live terminals parked while their
//! workspace is hidden. One workspace is active per process; switching hides the windows of the
//! old workspace (their tabs are parked, shells keep running) and shows the new one's.

use std::collections::HashMap;
use std::path::PathBuf;

use std::sync::Arc;

use gpui::{App, AppContext as _, Global, WindowHandle};
use mt_mux::ctl::{Live, Rows, Verb};
use mt_mux::{Id, Mux, SpawnSpec};
use mt_proto::{Request, RpcError, Spawned};
use mt_term::Terminal;
use serde_json::{json, Value};

use crate::ipc_ops::{self, Recorders};
use crate::ipc_pending::{CallCtx, PaneAccess, PaneRuntime};
use crate::{app::AppWindow, app::TabView, settings::Settings, ssh_host::SshHub};

pub struct MuxHost {
    pub mux: Mux,
    /// Tabs of windows whose workspace is hidden, keyed by mux window id.
    pub(crate) parked: HashMap<Id, Vec<TabView>>,
    /// What to launch for panes that have no terminal yet, keyed by pane id.
    pub(crate) pending: HashMap<Id, SpawnSpec>,
    /// OS windows currently showing a mux window.
    open: HashMap<Id, WindowHandle<AppWindow>>,
    /// `record.start` sessions.
    pub(crate) recorders: Recorders,
    /// Path of the IPC socket, exported to panes as `MICROTERM_UNIX_SOCKET`.
    pub(crate) socket: Option<PathBuf>,
    /// SSH domain connections.
    pub(crate) ssh: SshHub,
    /// Pane exits and the `pane.run` / `pane.wait` calls still waiting.
    pub(crate) runtime: PaneRuntime,
    /// OS notifications for `pane notify`; tests swap in a fake.
    pub(crate) notifier: Arc<dyn crate::notifier::Notifier>,
}

impl Global for MuxHost {}

impl MuxHost {
    fn new() -> Self {
        MuxHost {
            mux: Mux::new(),
            parked: HashMap::new(),
            pending: HashMap::new(),
            open: HashMap::new(),
            recorders: Recorders::default(),
            socket: None,
            ssh: SshHub::default(),
            runtime: PaneRuntime::default(),
            notifier: crate::notifier::system(),
        }
    }

    pub(crate) fn forget_open(&mut self, window: Id) {
        self.open.remove(&window);
    }

    /// Splits `pane` and records what to launch in the new one.
    pub(crate) fn split_pane(
        &mut self,
        pane: Id,
        axis: mt_mux::Axis,
        new_share: f32,
        spec: SpawnSpec,
    ) -> Option<Id> {
        let new = self
            .mux
            .split_pane(pane, axis, new_share, Some(spec.domain), spec.cwd.clone())
            .ok()?;
        self.pending.insert(
            new,
            SpawnSpec {
                split_from: Some(mt_mux::SplitOrigin {
                    parent: pane,
                    axis,
                    permille: (new_share * 1000.0).round().clamp(1.0, 999.0) as u16,
                }),
                ..spec
            },
        );
        Some(new)
    }

    /// Adds a tab (and its launch spec) to a mux window.
    pub(crate) fn spawn_tab(&mut self, window: Id, spec: SpawnSpec) -> Option<(Id, Id)> {
        let (tab, pane) = self
            .mux
            .spawn_tab_in(window, spec.domain, spec.cwd.clone(), spec.title.clone())
            .ok()?;
        self.pending.insert(pane, spec);
        Some((tab, pane))
    }
}

/// Installs the host if missing. Safe to call repeatedly.
pub fn ensure(cx: &mut App) {
    if !cx.has_global::<MuxHost>() {
        cx.set_global(MuxHost::new());
        sync_declared(cx);
    }
}

pub(crate) fn host(cx: &mut App) -> &mut MuxHost {
    ensure(cx);
    cx.global_mut::<MuxHost>()
}

/// Creates a mux workspace for every `[[workspaces]]` entry that does not exist yet.
pub fn sync_declared(cx: &mut App) {
    let names: Vec<String> = cx
        .global::<Settings>()
        .config
        .workspaces
        .iter()
        .map(|w| w.name.clone())
        .collect();
    let host = host_unchecked(cx);
    for name in names {
        if host.mux.workspace_by_name(&name).is_none() {
            let _ = host.mux.create_workspace(&name);
        }
    }
}

fn host_unchecked(cx: &mut App) -> &mut MuxHost {
    cx.global_mut::<MuxHost>()
}

fn expand_tilde(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if path == "~" => std::env::var_os("HOME").map_or_else(|| path.into(), Into::into),
        _ => path.into(),
    }
}

/// Makes sure a workspace has at least one mux window: the declared tabs of its `[[workspaces]]`
/// entry, or one default tab. `first` overrides the spec of the first tab (`--cwd`, `-e`).
fn ensure_workspace_windows(cx: &mut App, workspace: Id, first: Option<SpawnSpec>) {
    let declared = {
        let name = host(cx)
            .mux
            .workspaces()
            .iter()
            .find(|w| w.id == workspace)
            .map(|w| w.name.clone())
            .unwrap_or_default();
        let declared = cx
            .global::<Settings>()
            .config
            .workspaces
            .iter()
            .find(|w| w.name == name)
            .cloned();
        declared
    };
    let host = host_unchecked(cx);
    if host.mux.windows_in(workspace).next().is_some() {
        return;
    }
    let window = host.mux.spawn_window(Some(workspace));
    let tabs: Vec<mt_config::schema::WorkspaceTab> = declared.map(|d| d.tabs).unwrap_or_default();
    let mut specs: Vec<SpawnSpec> = tabs
        .iter()
        .map(|t| SpawnSpec {
            cwd: t.cwd.as_deref().map(expand_tilde),
            command: (!t.command.is_empty()).then(|| t.command.clone()),
            title: t.title.clone(),
            ..Default::default()
        })
        .collect();
    if specs.is_empty() {
        specs.push(SpawnSpec::default());
    }
    // A command-line override replaces the first tab outright, splits included.
    let overridden = first.is_some();
    if let Some(first) = first {
        let title = specs[0].title.take();
        specs[0] = SpawnSpec {
            title: first.title.or(title),
            ..first
        };
    }
    for (i, spec) in specs.into_iter().enumerate() {
        let Some((_, pane)) = host.spawn_tab(window, spec) else {
            continue;
        };
        if let Some(t) = tabs
            .get(i)
            .filter(|t| t.split.is_some() && !(overridden && i == 0))
        {
            let node = mt_config::schema::WorkspacePane {
                cwd: t.cwd.clone(),
                command: t.command.clone(),
                split: t.split,
                percent: t.percent,
                panes: t.panes.clone(),
            };
            let first_leaf = realise_split(host, pane, &node);
            let _ = host.mux.focus_pane(first_leaf);
        }
    }
    // Declared tabs open in order with the first one active.
    let _ = host.mux.activate_tab(window, 0);
}

/// Builds a declared split tree (`[[workspaces]]` tabs, SPEC §17.1) inside `pane`, which becomes
/// the node's first leaf; returns that leaf. The first child keeps `percent` of the space.
fn realise_split(host: &mut MuxHost, pane: Id, node: &mt_config::schema::WorkspacePane) -> Id {
    let leaf_spec = |n: &mt_config::schema::WorkspacePane| SpawnSpec {
        cwd: n.cwd.as_deref().map(expand_tilde),
        command: (!n.command.is_empty()).then(|| n.command.clone()),
        ..Default::default()
    };
    let (Some(kind), [a, b]) = (node.split, node.panes.as_slice()) else {
        host.pending.insert(pane, leaf_spec(node));
        return pane;
    };
    let axis = match kind {
        mt_config::schema::SplitKind::Horizontal => mt_mux::Axis::Horizontal,
        mt_config::schema::SplitKind::Vertical => mt_mux::Axis::Vertical,
    };
    let first_share = f32::from(node.percent.unwrap_or(50).clamp(1, 99)) / 100.0;
    let Some(second) = host.split_pane(pane, axis, 1.0 - first_share, leaf_spec(b)) else {
        return pane;
    };
    let first = realise_split(host, pane, a);
    realise_split(host, second, b);
    first
}

/// Opens an OS window for a mux window.
pub(crate) fn open_window(cx: &mut App, mux_window: Id) {
    open_window_at(cx, mux_window, None);
}

/// Like [`open_window`], placed where `bounds` says (a workspace switch reuses the position,
/// size and maximized/fullscreen state of the window it replaces).
fn open_window_at(cx: &mut App, mux_window: Id, bounds: Option<gpui::WindowBounds>) {
    let mut options = crate::app::window_options(cx);
    if bounds.is_some() {
        options.window_bounds = bounds;
    }
    match cx.open_window(options, move |window, cx| {
        cx.new(|cx| AppWindow::attach(mux_window, window, cx))
    }) {
        Ok(handle) => {
            host(cx).open.insert(mux_window, handle);
        }
        Err(e) => eprintln!("microterm: could not open a window: {e}"),
    }
}

/// Makes `name` the active workspace: opens its windows and hides the previous workspace's.
/// With `create`, an unknown name creates the workspace. `first` overrides the first tab of a
/// workspace that has to be started.
pub fn show_workspace(
    cx: &mut App,
    name: &str,
    create: bool,
    first: Option<SpawnSpec>,
) -> Result<(), String> {
    let target = {
        let h = host(cx);
        if h.mux.workspace_by_name(name).is_none() {
            if !create {
                return Err(format!("no workspace named \"{name}\""));
            }
            h.mux.create_workspace(name).map_err(|e| e.to_string())?;
        }
        h.mux.switch_workspace(name).map_err(|e| e.to_string())?;
        h.mux.workspace_by_name(name).expect("just ensured").id
    };
    ensure_workspace_windows(cx, target, first);

    let (to_open, to_hide): (Vec<Id>, Vec<WindowHandle<AppWindow>>) = {
        let h = host_unchecked(cx);
        let wanted: Vec<Id> = h.mux.windows_in(target).map(|w| w.id).collect();
        let to_open = wanted
            .iter()
            .copied()
            .filter(|w| !h.open.contains_key(w))
            .collect();
        let to_hide = h
            .open
            .iter()
            .filter(|(id, _)| !wanted.contains(id))
            .map(|(_, handle)| *handle)
            .collect();
        (to_open, to_hide)
    };
    // The new workspace's windows appear exactly where the one being hidden is.
    let inherited = to_hide
        .first()
        .and_then(|h| h.update(cx, |_, window, _| window.window_bounds()).ok());
    // Open before hiding so the app never has zero windows.
    for w in to_open {
        open_window_at(cx, w, inherited);
    }
    for handle in to_hide {
        handle
            .update(cx, |view, window, _| {
                view.hiding = true;
                window.remove_window();
            })
            .ok();
    }
    Ok(())
}

/// A new window in the active workspace with one tab.
pub fn open_new_window(cx: &mut App, spec: SpawnSpec) {
    let window = {
        let h = host(cx);
        let w = h.mux.spawn_window(None);
        h.spawn_tab(w, spec);
        w
    };
    open_window(cx, window);
}

/// Called when the last OS window closed: shows a workspace that still has parked windows
/// instead of quitting. Returns whether one was shown.
pub fn reopen_parked_workspace(cx: &mut App) -> bool {
    let name = {
        let h = host(cx);
        let ws = h
            .mux
            .windows()
            .iter()
            .find(|w| h.parked.contains_key(&w.id))
            .map(|w| w.workspace_id);
        ws.and_then(|id| h.mux.workspaces().iter().find(|w| w.id == id))
            .map(|w| w.name.clone())
    };
    match name {
        Some(n) => show_workspace(cx, &n, false, None).is_ok(),
        None => false,
    }
}

/// Wires the IPC server in: panes learn the socket path and every mux event is published to
/// `subscribe` connections.
pub fn install_ipc(cx: &mut App, socket: PathBuf, hub: Arc<mt_proto::EventHub>) {
    let h = host(cx);
    h.socket = Some(socket);
    h.mux.subscribe(move |e| hub.publish(ipc_ops::event_of(e)));
}

type Reply = Result<Value, RpcError>;

fn not_found(what: &str, id: Id) -> RpcError {
    RpcError::not_found(format!("{what} {id} does not exist"))
}

fn to_json<T: serde::Serialize>(v: &T) -> Reply {
    serde_json::to_value(v).map_err(|e| RpcError::internal(e.to_string()))
}

/// The live terminal of `pane`, wherever its tab currently lives (a window, or parked).
pub(crate) fn terminal(cx: &mut App, pane: Id) -> Option<Arc<Terminal>> {
    let h = host(cx);
    for tabs in h.parked.values() {
        if let Some(v) = tabs.iter().find_map(|t| t.pane_view(pane)) {
            return v.session.as_ref().map(|s| s.term.clone());
        }
    }
    let handles: Vec<_> = h.open.values().copied().collect();
    handles.into_iter().find_map(|handle| {
        handle
            .read_with(cx, |view, _| {
                view.tabs
                    .iter()
                    .find_map(|t| t.pane_view(pane))
                    .and_then(|v| v.session.as_ref().map(|s| s.term.clone()))
            })
            .ok()
            .flatten()
    })
}

fn pane_terminal(cx: &mut App, pane: Id) -> Result<Arc<Terminal>, RpcError> {
    if host(cx).mux.locate_pane(pane).is_none() {
        return Err(not_found("pane", pane));
    }
    terminal(cx, pane).ok_or_else(|| RpcError::internal(format!("pane {pane} has no terminal")))
}

/// Runs `f` on the first open window (prompts and notices have no better home).
pub(crate) fn with_first_window(
    cx: &mut App,
    f: impl FnOnce(&mut AppWindow, &mut gpui::Window, &mut gpui::Context<AppWindow>),
) {
    let handle = host(cx).open.values().next().copied();
    if let Some(h) = handle {
        let _ = h.update(cx, |view, window, cx| f(view, window, cx));
    }
}

/// Tells every open window that a domain's state changed (overlay, title bar, launcher dots).
pub(crate) fn notify_windows(cx: &mut App) {
    let handles: Vec<_> = host(cx).open.values().copied().collect();
    for h in handles {
        let _ = h.update(cx, |view, window, cx| view.on_domain_event(window, cx));
    }
}

pub(crate) fn toast_all(cx: &mut App, kind: crate::components::ToastKind, text: String) {
    let handles: Vec<_> = host(cx).open.values().copied().collect();
    for h in handles {
        let text = text.clone();
        let _ = h.update(cx, |view, _, cx| view.toast(kind, text, cx));
    }
}

/// Re-syncs the OS window of `window` (if shown) with the mux.
fn reconcile_window(cx: &mut App, window: Id, close: bool) {
    let handle = host(cx).open.get(&window).copied();
    if let Some(handle) = handle {
        let _ = handle.update(cx, |view, win, cx| {
            view.reconcile(win, cx);
            if close {
                win.remove_window();
            }
        });
    }
}

/// The window new tabs go to by default: the focused one of the active workspace.
fn first_window_of_active(cx: &mut App) -> Option<Id> {
    host(cx).mux.focused_window()
}

/// Snapshot of the mux with each pane's live terminal facts, for `ctl` listings and targets.
fn rows(cx: &mut App) -> Rows {
    let panes: Vec<Id> = host(cx)
        .mux
        .windows()
        .iter()
        .flat_map(|w| w.tabs.iter())
        .flat_map(|t| t.tree.panes())
        .map(|p| p.id)
        .collect();
    let live: HashMap<Id, Live> = panes
        .into_iter()
        .map(|p| {
            let term = terminal(cx, p);
            let l = ipc_ops::live_with_exit(term.as_deref(), host(cx).runtime.exit_of(p));
            (p, l)
        })
        .collect();
    host(cx)
        .mux
        .rows(&|id| live.get(&id).cloned().unwrap_or_default())
}

fn confirm_policy(cx: &App) -> mt_mux::ConfirmClose {
    ipc_ops::policy_of(&cx.global::<Settings>().config.confirm_close)
}

fn spawned(cx: &mut App, window: Id, tab_index: usize) -> Reply {
    let h = host(cx);
    let tab = &h
        .mux
        .window(window)
        .ok_or_else(|| not_found("window", window))?
        .tabs[tab_index];
    to_json(&Spawned {
        pane: tab.pane().id,
        tab: tab.id,
        window,
    })
}

fn spawn(cx: &mut App, p: &mt_proto::SpawnParams) -> Reply {
    let mut spec = ipc_ops::spawn_spec(p)?;
    if let Some(d) = p.domain.as_deref() {
        spec.domain = crate::ssh_host::resolve_domain(cx, d)?;
    }
    // An explicit window decides the workspace; otherwise `workspace`, otherwise the active one.
    let ws_name = {
        let h = host(cx);
        match p.window {
            Some(w) => {
                let win = h.mux.window(w).ok_or_else(|| not_found("window", w))?;
                h.mux
                    .workspaces()
                    .iter()
                    .find(|ws| ws.id == win.workspace_id)
                    .map(|ws| ws.name.clone())
                    .unwrap_or_default()
            }
            None => p
                .workspace
                .clone()
                .unwrap_or_else(|| h.mux.active_workspace().name.clone()),
        }
    };
    let (is_active, fresh) = {
        let h = host(cx);
        let ws = h.mux.workspace_by_name(&ws_name).map(|w| w.id);
        (
            ws_name == h.mux.active_workspace().name,
            ws.is_none_or(|id| h.mux.windows_in(id).next().is_none()),
        )
    };
    let uses_first_tab = fresh && p.window.is_none();
    if !is_active || fresh {
        // Starts the workspace when it has no windows: the spec becomes its first tab.
        show_workspace(cx, &ws_name, true, uses_first_tab.then(|| spec.clone()))
            .map_err(RpcError::internal)?;
        if uses_first_tab {
            let w = first_window_of_active(cx).ok_or_else(|| RpcError::internal("no window"))?;
            return spawned(cx, w, 0);
        }
    }
    if p.new_window {
        let w = {
            let h = host(cx);
            let w = h.mux.spawn_window(None);
            h.spawn_tab(w, spec);
            w
        };
        open_window(cx, w);
        return spawned(cx, w, 0);
    }
    let window = p
        .window
        .or_else(|| first_window_of_active(cx))
        .ok_or_else(|| RpcError::internal("no window"))?;
    let index = {
        let h = host(cx);
        h.spawn_tab(window, spec)
            .ok_or_else(|| not_found("window", window))?;
        let n = h.mux.window(window).map_or(1, |w| w.tabs.len());
        let _ = h.mux.activate_tab(window, n - 1);
        n - 1
    };
    reconcile_window(cx, window, false);
    spawned(cx, window, index)
}

fn kill_pane(cx: &mut App, pane: Id) -> Reply {
    if host(cx).mux.locate_pane(pane).is_none() {
        return Err(not_found("pane", pane));
    }
    let term = terminal(cx, pane);
    let h = host(cx);
    h.recorders.finish(pane, term.as_deref());
    h.runtime.forget(pane);
    h.pending.remove(&pane);
    let (window, tab, closed) = h
        .mux
        .kill_pane(pane)
        .map_err(|e| RpcError::not_found(e.to_string()))?;
    if let Some(tabs) = h.parked.get_mut(&window) {
        // A split tab survives losing one pane; `reconcile` drops the pane's view when the
        // window is shown again.
        if h.mux.tab_location(tab).is_none() {
            tabs.retain(|t| t.id != tab);
        } else if let Some(t) = tabs.iter_mut().find(|t| t.id == tab) {
            t.panes.retain(|p| p.pane != pane);
        }
        if closed {
            h.parked.remove(&window);
        }
    }
    reconcile_window(cx, window, closed);
    Ok(json!({}))
}

fn show(cx: &mut App, name: &str) -> Result<(), RpcError> {
    show_workspace(cx, name, false, None).map_err(RpcError::not_found)
}

/// Makes `index` the active tab of `window`, showing the window's workspace first.
fn activate_tab(cx: &mut App, window: Id, index: usize) -> Reply {
    show_window_workspace(cx, window)?;
    host(cx)
        .mux
        .activate_tab(window, index)
        .map_err(RpcError::from)?;
    reconcile_window(cx, window, false);
    Ok(json!({}))
}

/// Shows the workspace `window` belongs to when it is not the active one.
fn show_window_workspace(cx: &mut App, window: Id) -> Result<(), RpcError> {
    let name = {
        let h = host(cx);
        let win = h
            .mux
            .window(window)
            .ok_or_else(|| not_found("window", window))?;
        (win.workspace_id != h.mux.active_workspace().id).then(|| {
            h.mux
                .workspaces()
                .iter()
                .find(|w| w.id == win.workspace_id)
                .map(|w| w.name.clone())
                .unwrap_or_default()
        })
    };
    match name {
        Some(n) => show(cx, &n),
        None => Ok(()),
    }
}

fn rename_tab(cx: &mut App, p: &mt_proto::TabRenameParams) -> Reply {
    let rows = rows(cx);
    let (window, tab) = ipc_ops::resolve_rename(&host(cx).mux, &rows, &p.target)?;
    let title = (!p.title.is_empty()).then(|| p.title.clone());
    host(cx)
        .mux
        .rename_tab(window, tab, title)
        .map_err(RpcError::from)?;
    reconcile_window(cx, window, false);
    Ok(json!({}))
}

fn focus_window(cx: &mut App, t: &mt_proto::Target) -> Reply {
    let id = ipc_ops::resolve_window(&rows(cx), t)?;
    show_window_workspace(cx, id)?;
    host(cx).mux.focus_window(id).map_err(RpcError::from)?;
    let handle = host(cx).open.get(&id).copied();
    if let Some(handle) = handle {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    }
    cx.activate(true);
    Ok(json!({}))
}

/// `pane|tab|window.close`: honours `confirm_close` without ever showing the dialog.
fn close(cx: &mut App, request: &Request) -> Reply {
    let rows = rows(cx);
    let set = ipc_ops::plan_close(&rows, request, confirm_policy(cx))?;
    for pane in &set.panes {
        if host(cx).mux.locate_pane(*pane).is_some() {
            kill_pane(cx, *pane)?;
        }
    }
    for w in set.windows {
        if host(cx).mux.window(w).is_some() {
            let h = host(cx);
            let _ = h.mux.close_window(w);
            h.parked.remove(&w);
            reconcile_window(cx, w, true);
        }
    }
    Ok(json!({ "closed": set.panes }))
}

/// `pane.split`: the new pane inherits the target's domain; its shell starts on `reconcile`.
fn split(cx: &mut App, p: &mt_proto::PaneSplitParams) -> Reply {
    let rows = rows(cx);
    let h = host(cx);
    let done = ipc_ops::split_model(&mut h.mux, &rows, p)?;
    let domain = h
        .mux
        .pane_domain(done.pane)
        .unwrap_or(mt_mux::LOCAL_DOMAIN_ID);
    let cwd = h.mux.pane(done.pane).and_then(|pane| pane.cwd.clone());
    h.pending.insert(
        done.pane,
        SpawnSpec {
            domain,
            cwd,
            command: p.command.clone().filter(|c| !c.is_empty()),
            split_from: Some(mt_mux::SplitOrigin {
                parent: p.target.pane.unwrap_or_default(),
                axis: match p.axis {
                    mt_proto::SplitAxis::Horizontal => mt_mux::Axis::Horizontal,
                    mt_proto::SplitAxis::Vertical => mt_mux::Axis::Vertical,
                },
                permille: u16::from(p.percent.unwrap_or(50)) * 10,
            }),
            ..Default::default()
        },
    );
    show_window_workspace(cx, done.window)?;
    reconcile_window(cx, done.window, false);
    to_json(&done)
}

fn delete_workspace(cx: &mut App, name: &str, force: bool) -> Reply {
    let rows = rows(cx);
    let panes = ipc_ops::plan_workspace_delete(&rows, name, force, confirm_policy(cx))?;
    for pane in panes {
        if host(cx).mux.locate_pane(pane).is_some() {
            kill_pane(cx, pane)?;
        }
    }
    host(cx)
        .mux
        .delete_workspace(name)
        .map_err(RpcError::from)?;
    Ok(json!({}))
}

fn replay(cx: &mut App, p: &mt_proto::ReplayParams) -> Reply {
    let file = std::fs::File::open(&p.path)
        .map_err(|e| RpcError::not_found(format!("cannot open {}: {e}", p.path)))?;
    mt_record::Reader::new(std::io::BufReader::new(file))
        .map_err(|e| RpcError::bad_request(format!("{}: {e}", p.path)))?;
    let exe = std::env::current_exe().map_err(|e| RpcError::internal(e.to_string()))?;
    let name = std::path::Path::new(&p.path)
        .file_name()
        .map_or_else(|| p.path.clone(), |n| n.to_string_lossy().into_owned());
    spawn(
        cx,
        &mt_proto::SpawnParams {
            new_window: p.new_window,
            title: Some(format!("replay: {name}")),
            command: Some(vec![
                exe.to_string_lossy().into_owned(),
                "__replay".into(),
                p.path.clone(),
                "--speed".into(),
                p.speed.to_string(),
            ]),
            ..Default::default()
        },
    )
}

/// `[ctl].status_max_len`: status and notification text is cut to this many characters.
fn status_max_len(cx: &App) -> usize {
    cx.global::<Settings>().config.ctl.status_max_len as usize
}

/// Repaints every open window (tab status dots, badges, progress lines).
pub(crate) fn refresh_windows(cx: &mut App) {
    let handles: Vec<_> = host(cx).open.values().copied().collect();
    for h in handles {
        let _ = h.update(cx, |_, _, cx| cx.notify());
    }
}

/// Whether the user is looking at `pane` right now: its tab is the active tab of a window that
/// is shown and has OS focus.
fn pane_in_view(cx: &mut App, pane: Id) -> bool {
    let Some((window, index)) = host(cx).mux.locate_pane(pane) else {
        return false;
    };
    // A pane beside the focused one is on screen but not the one being typed into.
    let active = host(cx)
        .mux
        .window(window)
        .is_some_and(|w| w.active == index && w.tabs[index].pane().id == pane);
    let handle = host(cx).open.get(&window).copied();
    active
        && handle.is_some_and(|h| {
            h.update(cx, |_, win, _| win.is_window_active())
                .unwrap_or(false)
        })
}

/// Whether any microterm window has OS focus.
fn app_focused(cx: &mut App) -> bool {
    let handles: Vec<_> = host(cx).open.values().copied().collect();
    handles.into_iter().any(|h| {
        h.update(cx, |_, win, _| win.is_window_active())
            .unwrap_or(false)
    })
}

/// Surfaces a notification that got through the rate limit according to `[notifications]`:
/// attention badge, toast and OS notification (SPEC §12.4). `--desktop` forces the last one.
fn deliver(cx: &mut App, d: ipc_ops::Delivered) {
    use mt_config::{DesktopMode, ToastMode};
    use mt_proto::NotifyLevel;

    use crate::components::ToastKind;

    let cfg = cx.global::<Settings>().config.notifications.clone();
    let in_view = pane_in_view(cx, d.pane);
    if cfg.badge && !in_view {
        host(cx).mux.raise_attention(d.pane, d.level);
        refresh_windows(cx);
    }
    let text = match &d.title {
        Some(t) if !d.message.is_empty() => format!("{t}: {}", d.message),
        Some(t) => t.clone(),
        None => d.message.clone(),
    };
    let toast = match cfg.toast {
        ToastMode::Always => true,
        ToastMode::Background => !in_view,
        ToastMode::Never => false,
    };
    if toast && !text.is_empty() {
        let kind = match d.level {
            NotifyLevel::Info => ToastKind::Info,
            NotifyLevel::Attention => ToastKind::Warning,
            NotifyLevel::Error => ToastKind::Error,
        };
        toast_all(cx, kind, text.clone());
    }
    let desktop = d.desktop
        || match cfg.desktop {
            DesktopMode::Always => true,
            DesktopMode::Unfocused => !app_focused(cx),
            DesktopMode::Never => false,
        };
    if desktop {
        let summary = d.title.clone().unwrap_or_else(|| "microterm".into());
        let body = if d.title.is_some() {
            d.message.clone()
        } else {
            text
        };
        host(cx).notifier.clone().notify(&summary, &body);
    }
}

/// The status dot, progress line, tooltip and badge of `pane`'s tab.
pub(crate) fn tab_chrome(cx: &App, pane: Id) -> crate::tab_chrome::TabChrome {
    let Some(h) = cx.try_global::<MuxHost>() else {
        return Default::default();
    };
    let mut chrome = h.mux.pane(pane).map_or_else(Default::default, |p| {
        crate::tab_chrome::tab_chrome(p.status.as_ref(), p.attention)
    });
    chrome.zoomed = h
        .mux
        .locate_pane(pane)
        .and_then(|(w, i)| h.mux.window(w).map(|w| w.tabs[i].zoomed.is_some()))
        .unwrap_or(false);
    chrome
}

/// `[ctl].capture_max_bytes`.
fn capture_cap(cx: &App) -> usize {
    cx.global::<Settings>()
        .config
        .ctl
        .capture_max_bytes
        .min(usize::MAX as u64) as usize
}

/// The mux as the pending `pane.run` / `pane.wait` calls see it.
struct GuiAccess<'a>(&'a mut App);

impl PaneAccess for GuiAccess<'_> {
    fn exists(&mut self, pane: Id) -> bool {
        host(self.0).mux.locate_pane(pane).is_some()
    }
    fn terminal(&mut self, pane: Id) -> Option<Arc<Terminal>> {
        terminal(self.0, pane)
    }
}

/// Runs `f` on the runtime with a [`GuiAccess`]; the runtime is moved out of the host meanwhile
/// so that access can borrow the app.
fn with_runtime<R>(cx: &mut App, f: impl FnOnce(&mut PaneRuntime, &mut GuiAccess) -> R) -> R {
    let mut rt = std::mem::take(&mut host(cx).runtime);
    let r = f(&mut rt, &mut GuiAccess(cx));
    let h = host(cx);
    // Exits recorded while the runtime was out (none today) would be lost; keep the newest.
    let newer = std::mem::replace(&mut h.runtime, rt);
    drop(newer);
    r
}

/// Answers the pending `pane.run` / `pane.wait` calls whose condition now holds. Called a few
/// times a second.
pub fn poll_pending(cx: &mut App) {
    // Statuses set with `--ttl` clear themselves (a crashed agent leaves no stale marker).
    if !host(cx).mux.expire_statuses(mt_term::now_ms()).is_empty() {
        refresh_windows(cx);
    }
    if !host(cx).runtime.has_pending() {
        return;
    }
    with_runtime(cx, |rt, access| rt.poll(access));
}

/// What to do after a pane's process ended: records the exit and reports whether the tab must
/// stay (`pane run --keep`).
pub(crate) fn on_pane_exit(cx: &mut App, pane: Id, term: &Terminal, status: Option<i32>) -> bool {
    let h = host(cx);
    h.mux.notify_exited(pane, status);
    h.runtime.on_exit(pane, Some(term), status)
}

/// OSC 9;4 from a program in `pane`.
pub(crate) fn on_osc_progress(
    cx: &mut App,
    pane: Id,
    state: mt_term::ProgressState,
    value: Option<u8>,
) {
    let now = mt_term::now_ms();
    if ipc_ops::osc_progress(&mut host(cx).mux, pane, state, value, now) {
        refresh_windows(cx);
    }
}

/// OSC 9 from a program in `pane`, through the `[notifications]` pipeline.
pub(crate) fn on_osc_notify(cx: &mut App, pane: Id, text: &str) {
    if !cx.global::<Settings>().config.terminal.osc_notifications {
        return;
    }
    let max = status_max_len(cx);
    let now = mt_term::now_ms();
    if let Some(d) = ipc_ops::osc_notify(&mut host(cx).mux, pane, text, max, now) {
        deliver(cx, d);
    }
}

pub(crate) fn on_command_finished(cx: &mut App, pane: Id, exit_code: Option<i32>) {
    host(cx).mux.notify_command_finished(pane, exit_code);
}

/// `pane.run`: a new tab (spawn mode) or a line typed into an existing shell (`--in`).
fn run(cx: &mut App, p: mt_proto::RunParams, ctx: CallCtx) -> Option<Reply> {
    let cap = capture_cap(cx);
    if let Some(target) = p.in_pane.clone() {
        let pane =
            match ipc_ops::resolve_panes(&rows(cx), &target, Verb::Acting { all_allowed: false }) {
                Ok(v) => v[0],
                Err(e) => return Some(Err(e)),
            };
        return with_runtime(cx, |rt, access| rt.begin_run_in(access, pane, &p, cap, ctx));
    }
    if p.spawn.command.as_ref().is_none_or(|c| c.is_empty()) {
        return Some(Err(RpcError::bad_request("run needs a command after `--`")));
    }
    let spawned = match spawn(cx, &p.spawn) {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    let Ok(s) = serde_json::from_value::<Spawned>(spawned) else {
        return Some(Err(RpcError::internal("bad spawn result")));
    };
    host(cx).runtime.mark_run(s.pane, p.keep, !p.detach);
    with_runtime(cx, |rt, _| rt.begin_run_spawn(s.pane, s.tab, &p, cap, ctx))
}

/// `pane.wait`.
fn wait(cx: &mut App, p: mt_proto::WaitParams, ctx: CallCtx) -> Option<Reply> {
    let pane = match ipc_ops::resolve_panes(&rows(cx), &p.target, Verb::ReadOnly) {
        Ok(v) => v[0],
        Err(e) => return Some(Err(e)),
    };
    with_runtime(cx, |rt, access| rt.begin_wait(access, pane, &p, ctx))
}

/// Executes one IPC call on the GPUI main thread and sends its answer, now or (for `pane.run`
/// and `pane.wait`) when it is ready.
pub fn dispatch(cx: &mut App, call: mt_proto::Call) {
    let mt_proto::Call {
        request,
        reply,
        cancel,
    } = call;
    let ctx = CallCtx {
        reply: reply.clone(),
        cancel,
    };
    let answer = match request {
        Request::PaneRun(p) => run(cx, p, ctx),
        Request::PaneWait(p) => wait(cx, p, ctx),
        other => Some(handle_call(cx, other)),
    };
    if let Some(a) = answer {
        let _ = reply.send(a);
    }
}

/// Executes one non-deferred IPC request on the GPUI main thread (SPEC §12.2).
pub fn handle_call(cx: &mut App, request: Request) -> Reply {
    match request {
        Request::MuxList
        | Request::WorkspaceList
        | Request::WindowList(_)
        | Request::TabList(_)
        | Request::PaneList(_)
        | Request::PaneGet(_) => {
            let rows = rows(cx);
            ipc_ops::read_only(&rows, &request).expect("a listing request")
        }
        Request::TabNew(p) => spawn(
            cx,
            &mt_proto::SpawnParams {
                new_window: false,
                ..p
            },
        ),
        Request::WindowNew(p) => {
            // Also the single-instance request of a second `microterm` (SPEC §12.1).
            let r = spawn(
                cx,
                &mt_proto::SpawnParams {
                    new_window: true,
                    ..p
                },
            );
            cx.activate(true);
            r
        }
        Request::WindowFocus(t) => focus_window(cx, &t),
        Request::PaneSendText(p) => {
            let panes =
                ipc_ops::resolve_panes(&rows(cx), &p.target, Verb::Acting { all_allowed: true })?;
            for pane in panes {
                let term = pane_terminal(cx, pane)?;
                ipc_ops::send_text(&term, &p);
            }
            Ok(json!({}))
        }
        Request::PaneGetText(p) => {
            let panes = ipc_ops::resolve_panes(&rows(cx), &p.target, Verb::ReadOnly)?;
            let term = pane_terminal(cx, panes[0])?;
            ipc_ops::get_text(&term, &p, capture_cap(cx))
        }
        Request::PaneStatusSet(p) => {
            let rows = rows(cx);
            let max = status_max_len(cx);
            let r = ipc_ops::status_set(&mut host(cx).mux, &rows, &p, max, mt_term::now_ms())?;
            refresh_windows(cx);
            Ok(r)
        }
        Request::PaneStatusClear(t) => {
            let rows = rows(cx);
            let r = ipc_ops::status_clear(&mut host(cx).mux, &rows, &t)?;
            refresh_windows(cx);
            Ok(r)
        }
        Request::PaneStatusGet(t) => {
            let rows = rows(cx);
            ipc_ops::status_get(&host(cx).mux, &rows, &t)
        }
        Request::PaneNotify(p) => {
            let rows = rows(cx);
            let max = status_max_len(cx);
            let (result, delivered) =
                ipc_ops::notify(&mut host(cx).mux, &rows, &p, max, mt_term::now_ms())?;
            for d in delivered {
                deliver(cx, d);
            }
            Ok(result)
        }
        Request::PaneSplit(p) => split(cx, &p),
        Request::PaneFocus(_)
        | Request::PaneZoom(_)
        | Request::PaneResize(_)
        | Request::PaneSwap(_)
        | Request::PaneRotate(_) => {
            let rows = rows(cx);
            let r = ipc_ops::layout_op(&mut host(cx).mux, &rows, &request)?;
            let pane = r.get("pane").and_then(Value::as_u64);
            if let Some((window, _)) = pane.and_then(|p| host(cx).mux.locate_pane(p)) {
                show_window_workspace(cx, window)?;
                reconcile_window(cx, window, false);
            }
            Ok(r)
        }
        Request::PaneRun(_) | Request::PaneWait(_) => Err(RpcError::internal(
            "pane.run and pane.wait answer later; they go through dispatch()",
        )),
        Request::PaneClose(_) | Request::TabClose(_) | Request::WindowClose(_) => {
            close(cx, &request)
        }
        Request::TabFocus(p) => {
            let rows = rows(cx);
            let (window, index) = ipc_ops::resolve_focus(&host(cx).mux, &rows, &p)?;
            activate_tab(cx, window, index)
        }
        Request::TabRename(p) => rename_tab(cx, &p),
        Request::WorkspaceSwitch { name } => {
            show(cx, &name)?;
            Ok(json!({}))
        }
        Request::WorkspaceCreate { name } => {
            host(cx)
                .mux
                .create_workspace(&name)
                .map_err(|e| RpcError::bad_request(e.to_string()))?;
            Ok(json!({}))
        }
        Request::WorkspaceRename { old, new } => {
            let h = host(cx);
            let id = h
                .mux
                .workspace_by_name(&old)
                .ok_or_else(|| RpcError::not_found(format!("no workspace named \"{old}\"")))?
                .id;
            h.mux
                .rename_workspace(id, &new)
                .map_err(|e| RpcError::bad_request(e.to_string()))?;
            Ok(json!({}))
        }
        Request::WorkspaceDelete { name, force } => delete_workspace(cx, &name, force),
        Request::RecordStart(p) => {
            let term = pane_terminal(cx, p.pane)?;
            host(cx).recorders.start(p.pane, &term, &p.path, p.input)?;
            Ok(json!({}))
        }
        Request::RecordStop { pane } => {
            let term = terminal(cx, pane);
            host(cx).recorders.stop(pane, term.as_deref())?;
            Ok(json!({}))
        }
        Request::ReplayStart(p) => replay(cx, &p),
        Request::ConfigReload => {
            crate::settings::reload(cx);
            Ok(json!({}))
        }
        // Streaming is the server's business; it never forwards `subscribe`.
        Request::Subscribe(_) => Ok(json!({})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use gpui::TestAppContext;
    use mt_proto::Target;

    fn init(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Settings::defaults());
            cx.set_global(Theme::one_dark());
            ensure(cx);
        });
    }

    #[gpui::test]
    fn switching_workspaces_hides_and_shows_windows(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            show_workspace(cx, "default", true, None).unwrap();
            assert_eq!(cx.windows().len(), 1);
            show_workspace(cx, "api", true, None).unwrap();
            assert_eq!(cx.windows().len(), 1, "default's window was hidden");
            open_new_window(cx, SpawnSpec::default());
            assert_eq!(cx.windows().len(), 2);
            show_workspace(cx, "infra", true, None).unwrap();
            assert_eq!(cx.windows().len(), 1);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let h = cx.global::<MuxHost>();
            assert_eq!(h.mux.active_workspace().name, "infra");
            let api = h.mux.workspace_by_name("api").unwrap().id;
            assert_eq!(h.mux.counts(api), (2, 2));
            assert_eq!(h.parked.len(), 3, "hidden windows keep their tabs");
            show_workspace(cx, "api", false, None).unwrap();
            assert_eq!(cx.windows().len(), 2);
            assert!(show_workspace(cx, "ghost", false, None).is_err());
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                cx.global::<MuxHost>().parked.len(),
                2,
                "default's and infra's windows are parked"
            );
        });
    }

    #[gpui::test]
    fn declared_workspaces_start_with_their_tabs(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let src = "[[workspaces]]\nname = \"api\"\ntabs = [{ title = \"server\" }, { title = \"logs\", cwd = \"/tmp\" }]\n";
            let mut s = Settings::defaults();
            s.config = std::sync::Arc::new(mt_config::parse_str(src).unwrap().config);
            cx.set_global(s);
            cx.set_global(Theme::one_dark());
            ensure(cx);
            assert!(host(cx).mux.workspace_by_name("api").is_some());
            show_workspace(cx, "api", false, None).unwrap();
            let h = cx.global::<MuxHost>();
            let ws = h.mux.workspace_by_name("api").unwrap().id;
            let win = h.mux.windows_in(ws).next().unwrap();
            let titles: Vec<_> = win
                .tabs
                .iter()
                .map(|t| t.title_override.clone().unwrap())
                .collect();
            assert_eq!(titles, ["server", "logs"]);
            assert_eq!(win.tabs[1].pane().cwd.as_deref(), Some(std::path::Path::new("/tmp")));
            assert_eq!(win.active, 0);
        });
    }

    #[gpui::test]
    fn cli_overrides_apply_to_the_first_tab(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            let spec = SpawnSpec {
                cwd: Some("/var".into()),
                command: Some(vec!["htop".into()]),
                title: None,
                ..Default::default()
            };
            show_workspace(cx, "ops", true, Some(spec)).unwrap();
            let h = cx.global::<MuxHost>();
            let pane = h.mux.windows().last().unwrap().tabs[0].pane().id;
            assert_eq!(
                h.pending[&pane].command.as_deref(),
                Some(&["htop".to_string()][..])
            );
        });
    }

    #[gpui::test]
    fn declared_nested_splits_build_the_tree(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let src = r#"
[[workspaces]]
name = "api"
tabs = [
  { title = "dev", split = "horizontal", percent = 60, panes = [
      { cwd = "/srv", command = ["nvim"] },
      { split = "vertical", panes = [ { command = ["cargo", "watch"] }, { command = ["htop"] } ] },
  ] },
  { title = "plain" },
]
"#;
            let mut s = Settings::defaults();
            s.config = std::sync::Arc::new(mt_config::parse_str(src).unwrap().config);
            cx.set_global(s);
            cx.set_global(Theme::one_dark());
            ensure(cx);
            show_workspace(cx, "api", false, None).unwrap();
            let h = cx.global::<MuxHost>();
            let ws = h.mux.workspace_by_name("api").unwrap().id;
            let win = h.mux.windows_in(ws).next().unwrap();
            assert_eq!(win.tabs.len(), 2);
            let tab = &win.tabs[0];
            let ids: Vec<_> = tab.tree.panes().iter().map(|p| p.id).collect();
            assert_eq!(ids.len(), 3);
            let cmds: Vec<_> = ids
                .iter()
                .map(|id| h.pending.get(id).and_then(|s| s.command.clone()))
                .collect();
            // Leaves are in layout order; the spawned terminals are not started in tests, so
            // `pending` still holds each pane's launch spec.
            assert_eq!(
                cmds,
                [
                    Some(vec!["nvim".to_string()]),
                    Some(vec!["cargo".to_string(), "watch".to_string()]),
                    Some(vec!["htop".to_string()]),
                ]
            );
            assert_eq!(
                h.pending[&ids[0]].cwd.as_deref(),
                Some(std::path::Path::new("/srv"))
            );
            assert_eq!(tab.pane().id, ids[0], "focus starts on the first pane");
            let rows = h.mux.rows(&|_| Default::default());
            let mt_proto::LayoutNode::Split { axis, percent, .. } = rows.tabs[0].layout.clone()
            else {
                panic!("declared a split")
            };
            assert_eq!((axis, percent), (mt_proto::SplitAxis::Horizontal, 60));
            assert_eq!(win.tabs[1].tree.leaf_count(), 1);
        });
    }

    #[gpui::test]
    fn ipc_split_focus_zoom_resize_and_close(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| show_workspace(cx, "default", true, None).unwrap());
        let first = list::<mt_proto::PaneInfo>(cx, Request::PaneList(Default::default()))[0].id;
        let split = |cx: &mut TestAppContext, target: u64, axis| -> mt_proto::PaneSplitResult {
            serde_json::from_value(
                call(
                    cx,
                    Request::PaneSplit(mt_proto::PaneSplitParams {
                        target: Target::pane(target),
                        axis,
                        percent: None,
                        cwd: None,
                        command: Some(vec!["htop".into()]),
                    }),
                )
                .unwrap(),
            )
            .unwrap()
        };
        let b = split(cx, first, mt_proto::SplitAxis::Horizontal);
        let c = split(cx, b.pane, mt_proto::SplitAxis::Vertical);
        assert_eq!((b.tab, c.tab), (b.tab, b.tab), "splits stay in the tab");

        let snap: mt_proto::MuxSnapshot =
            serde_json::from_value(call(cx, Request::MuxList).unwrap()).unwrap();
        let tab = &snap.workspaces[0].windows[0].tabs[0];
        assert_eq!(tab.panes.len(), 3);
        assert_eq!(tab.panes.iter().filter(|p| p.focused).count(), 1);
        assert!(tab.panes.iter().find(|p| p.id == c.pane).unwrap().focused);
        assert!(matches!(
            tab.layout,
            Some(mt_proto::LayoutNode::Split {
                axis: mt_proto::SplitAxis::Horizontal,
                ..
            })
        ));

        // Directional focus: pane c is bottom-right, so left is the first pane.
        call(
            cx,
            Request::PaneFocus(mt_proto::PaneFocusParams {
                target: Target::pane(c.pane),
                direction: Some(mt_proto::Direction::Left),
            }),
        )
        .unwrap();
        let panes = list::<mt_proto::PaneInfo>(cx, Request::PaneList(Default::default()));
        assert!(panes.iter().find(|p| p.id == first).unwrap().focused);
        let err = call(
            cx,
            Request::PaneFocus(mt_proto::PaneFocusParams {
                target: Target::pane(first),
                direction: Some(mt_proto::Direction::Up),
            }),
        )
        .unwrap_err();
        assert_eq!(err.code, mt_proto::ErrorCode::NotFound);

        let z = call(cx, Request::PaneZoom(Target::pane(b.pane))).unwrap();
        assert_eq!(z["zoomed"], true);
        let panes = list::<mt_proto::PaneInfo>(cx, Request::PaneList(Default::default()));
        assert!(panes.iter().find(|p| p.id == b.pane).unwrap().zoomed);
        assert!(
            cx.update(|cx| tab_chrome(cx, first).zoomed),
            "the tab shows the zoom indicator while any of its panes is zoomed"
        );
        call(cx, Request::PaneZoom(Target::pane(b.pane))).unwrap();
        assert!(!cx.update(|cx| tab_chrome(cx, first).zoomed));

        call(
            cx,
            Request::PaneResize(mt_proto::PaneResizeParams {
                target: Target::pane(first),
                direction: mt_proto::Direction::Right,
                cells: 12,
            }),
        )
        .unwrap();
        let snap: mt_proto::MuxSnapshot =
            serde_json::from_value(call(cx, Request::MuxList).unwrap()).unwrap();
        let Some(mt_proto::LayoutNode::Split { percent, .. }) =
            snap.workspaces[0].windows[0].tabs[0].layout.clone()
        else {
            panic!("split layout")
        };
        assert!(
            percent > 50,
            "growing the left pane to the right: {percent}"
        );

        // Closing one pane leaves the tab; closing the rest closes it.
        call(
            cx,
            Request::PaneClose(Target {
                force: true,
                ..Target::pane(c.pane)
            }),
        )
        .unwrap();
        let tabs = list::<mt_proto::TabInfo>(cx, Request::TabList(Default::default()));
        assert_eq!(tabs[0].panes.len(), 2);
        call(
            cx,
            Request::PaneSwap(mt_proto::PaneDirParams {
                target: Target::pane(first),
                direction: mt_proto::Direction::Right,
            }),
        )
        .unwrap();
        call(cx, Request::PaneRotate(Target::pane(first))).unwrap();
        assert!(call(
            cx,
            Request::PaneResize(mt_proto::PaneResizeParams {
                target: Target::pane(first),
                direction: mt_proto::Direction::Up,
                cells: 1,
            })
        )
        .is_err());
    }

    fn call(cx: &mut TestAppContext, req: Request) -> Reply {
        cx.update(|cx| handle_call(cx, req))
    }

    fn spawned(v: Value) -> Spawned {
        serde_json::from_value(v).unwrap()
    }

    fn list<T: serde::de::DeserializeOwned>(cx: &mut TestAppContext, req: Request) -> Vec<T> {
        serde_json::from_value(call(cx, req).unwrap()).unwrap()
    }

    fn tab_titles(cx: &mut TestAppContext) -> Vec<(u64, String, bool)> {
        list::<mt_proto::TabInfo>(cx, Request::TabList(Default::default()))
            .into_iter()
            .map(|t| (t.id, t.title, t.active))
            .collect()
    }

    #[gpui::test]
    fn ipc_new_list_rename_focus_and_close(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| show_workspace(cx, "default", true, None).unwrap());
        let a = spawned(
            call(
                cx,
                Request::TabNew(mt_proto::SpawnParams {
                    title: Some("one".into()),
                    ..Default::default()
                }),
            )
            .unwrap(),
        );
        let b = spawned(call(cx, Request::TabNew(Default::default())).unwrap());
        assert_eq!(a.window, b.window);
        let snap: mt_proto::MuxSnapshot =
            serde_json::from_value(call(cx, Request::MuxList).unwrap()).unwrap();
        let tabs = &snap.workspaces[0].windows[0].tabs;
        assert_eq!(tabs.len(), 3, "the default tab plus two spawned ones");
        assert!(tabs[2].active);

        call(
            cx,
            Request::TabRename(mt_proto::TabRenameParams {
                target: Target::tab(a.tab),
                title: "renamed".into(),
            }),
        )
        .unwrap();
        call(
            cx,
            Request::TabFocus(mt_proto::TabFocusParams {
                target: Target::tab(a.tab),
                ..Default::default()
            }),
        )
        .unwrap();
        let tabs = tab_titles(cx);
        assert_eq!(tabs[1], (a.tab, "renamed".into(), true));

        // Selectors: one match closes it; a title nobody has is "not found".
        let by_title = |t: &str| Target::matching(format!("title:{t}"));
        let e = call(cx, Request::TabClose(by_title("nope"))).unwrap_err();
        assert_eq!(e.code, mt_proto::ErrorCode::NotFound);
        // Closing never falls back to the focused tab.
        let e = call(cx, Request::TabClose(Target::default())).unwrap_err();
        assert_eq!(e.code, mt_proto::ErrorCode::BadRequest);
        call(cx, Request::TabClose(by_title("renamed"))).unwrap();
        assert_eq!(tab_titles(cx).len(), 2);

        // Pane close by id, then again: not found.
        call(cx, Request::PaneClose(Target::pane(b.pane))).unwrap();
        assert_eq!(
            call(cx, Request::PaneClose(Target::pane(b.pane)))
                .unwrap_err()
                .code,
            mt_proto::ErrorCode::NotFound
        );
        assert_eq!(
            call(
                cx,
                Request::PaneGetText(mt_proto::GetTextParams {
                    target: Target::pane(9999),
                    start_line: None,
                    end_line: None,
                    escapes: false,
                    last_command: None,
                })
            )
            .unwrap_err()
            .code,
            mt_proto::ErrorCode::NotFound
        );
    }

    #[gpui::test]
    fn ipc_ambiguous_matches_need_all(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| show_workspace(cx, "default", true, None).unwrap());
        for _ in 0..2 {
            call(
                cx,
                Request::TabNew(mt_proto::SpawnParams {
                    title: Some("build".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        }
        let sel = Target::matching("title:build");
        let e = call(cx, Request::TabClose(sel.clone())).unwrap_err();
        assert_eq!(e.code, mt_proto::ErrorCode::Ambiguous);
        assert_eq!(e.candidates.len(), 2);
        call(cx, Request::TabClose(Target { all: true, ..sel })).unwrap();
        assert_eq!(tab_titles(cx).len(), 1);
    }

    #[gpui::test]
    fn ipc_workspaces_and_new_windows(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| show_workspace(cx, "default", true, None).unwrap());
        call(cx, Request::WorkspaceCreate { name: "api".into() }).unwrap();
        assert!(call(cx, Request::WorkspaceCreate { name: "api".into() }).is_err());
        call(
            cx,
            Request::WorkspaceRename {
                old: "api".into(),
                new: "svc".into(),
            },
        )
        .unwrap();
        // The first spawn into a window-less workspace becomes its first tab.
        let first = spawned(
            call(
                cx,
                Request::TabNew(mt_proto::SpawnParams {
                    workspace: Some("svc".into()),
                    ..Default::default()
                }),
            )
            .unwrap(),
        );
        cx.run_until_parked();
        let workspaces: Vec<mt_proto::WorkspaceInfo> = list(cx, Request::WorkspaceList);
        let svc = workspaces.iter().find(|w| w.name == "svc").unwrap();
        assert!(svc.active);
        assert_eq!(svc.windows, [first.window]);
        let panes: Vec<mt_proto::PaneInfo> = list(cx, Request::PaneList(Default::default()));
        let p = panes.iter().find(|p| p.id == first.pane).unwrap();
        assert_eq!(
            (p.tab, p.window, p.workspace.as_str()),
            (first.tab, first.window, "svc")
        );
        let w2 = spawned(call(cx, Request::WindowNew(Default::default())).unwrap());
        assert_ne!(w2.window, first.window);
        assert!(call(
            cx,
            Request::WorkspaceSwitch {
                name: "ghost".into()
            }
        )
        .is_err());

        // The active workspace and `default` cannot be deleted; an inactive one can.
        assert!(call(
            cx,
            Request::WorkspaceDelete {
                name: "svc".into(),
                force: true
            }
        )
        .is_err());
        assert!(call(
            cx,
            Request::WorkspaceDelete {
                name: "default".into(),
                force: true
            }
        )
        .is_err());
        call(
            cx,
            Request::WorkspaceSwitch {
                name: "default".into(),
            },
        )
        .unwrap();
        call(
            cx,
            Request::WorkspaceDelete {
                name: "svc".into(),
                force: false,
            },
        )
        .unwrap();
        let names: Vec<String> = list::<mt_proto::WorkspaceInfo>(cx, Request::WorkspaceList)
            .into_iter()
            .map(|w| w.name)
            .collect();
        assert_eq!(names, ["default"]);

        // A foreign domain does not exist yet.
        assert_eq!(
            call(
                cx,
                Request::TabNew(mt_proto::SpawnParams {
                    domain: Some("prod".into()),
                    ..Default::default()
                })
            )
            .unwrap_err()
            .code,
            mt_proto::ErrorCode::NotFound
        );
    }

    fn edit_config(cx: &mut App, f: impl FnOnce(&mut mt_config::Config)) {
        f(Arc::make_mut(&mut cx.global_mut::<Settings>().config));
    }

    fn fake_notifier(cx: &mut TestAppContext) -> Arc<crate::notifier::FakeNotifier> {
        let fake = Arc::new(crate::notifier::FakeNotifier::default());
        let f = fake.clone();
        cx.update(|cx| host(cx).notifier = f);
        fake
    }

    /// Two tabs in the default workspace: `(background pane, active pane)`.
    fn two_tabs(cx: &mut TestAppContext) -> (Spawned, Spawned) {
        cx.update(|cx| show_workspace(cx, "default", true, None).unwrap());
        let a = spawned(call(cx, Request::TabNew(Default::default())).unwrap());
        let b = spawned(call(cx, Request::TabNew(Default::default())).unwrap());
        (a, b)
    }

    fn chrome_of(cx: &mut TestAppContext, pane: u64) -> crate::tab_chrome::TabChrome {
        cx.update(|cx| tab_chrome(cx, pane))
    }

    #[gpui::test]
    fn status_set_and_clear_drive_the_tab_chrome(cx: &mut TestAppContext) {
        init(cx);
        let (a, _b) = two_tabs(cx);
        call(
            cx,
            Request::PaneStatusSet(mt_proto::StatusSetParams {
                target: Target::pane(a.pane),
                state: Some(mt_proto::StatusState::Working),
                text: Some("running tests".into()),
                progress: Some(40),
                ttl_ms: None,
            }),
        )
        .unwrap();
        let c = chrome_of(cx, a.pane);
        assert_eq!(c.dot, Some(crate::components::StatusKind::Accent));
        assert_eq!(c.progress, Some(40));
        assert_eq!(c.tooltip.as_deref(), Some("working 40%: running tests"));
        let got: mt_proto::StatusResult =
            serde_json::from_value(call(cx, Request::PaneStatusGet(Target::pane(a.pane))).unwrap())
                .unwrap();
        assert_eq!(got.status.unwrap().text, "running tests");
        call(cx, Request::PaneStatusClear(Target::pane(a.pane))).unwrap();
        assert_eq!(chrome_of(cx, a.pane), Default::default());
        // Acting verbs never use the focused pane.
        let e = call(
            cx,
            Request::PaneStatusSet(mt_proto::StatusSetParams::default()),
        )
        .unwrap_err();
        assert_eq!(e.code, mt_proto::ErrorCode::BadRequest);
    }

    #[gpui::test]
    fn ttl_expiry_clears_a_status_from_the_poll_loop(cx: &mut TestAppContext) {
        init(cx);
        let (a, _b) = two_tabs(cx);
        call(
            cx,
            Request::PaneStatusSet(mt_proto::StatusSetParams {
                target: Target::pane(a.pane),
                state: Some(mt_proto::StatusState::Waiting),
                ttl_ms: Some(1),
                ..Default::default()
            }),
        )
        .unwrap();
        assert!(chrome_of(cx, a.pane).dot.is_some());
        std::thread::sleep(std::time::Duration::from_millis(5));
        cx.update(poll_pending);
        assert_eq!(chrome_of(cx, a.pane), Default::default());
    }

    fn notify(cx: &mut TestAppContext, pane: u64, desktop: bool) -> mt_proto::ActResult {
        serde_json::from_value(
            call(
                cx,
                Request::PaneNotify(mt_proto::NotifyParams {
                    target: Target::pane(pane),
                    level: mt_proto::NotifyLevel::Attention,
                    desktop,
                    message: "done".into(),
                    ..Default::default()
                }),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[gpui::test]
    fn notify_badges_a_background_tab_and_honours_the_desktop_setting(cx: &mut TestAppContext) {
        use mt_config::DesktopMode;
        init(cx);
        let fake = fake_notifier(cx);
        let (a, _b) = two_tabs(cx);
        cx.update(|cx| edit_config(cx, |c| c.notifications.desktop = DesktopMode::Never));
        notify(cx, a.pane, false);
        assert_eq!(
            chrome_of(cx, a.pane).badge,
            Some(crate::components::StatusKind::Warning)
        );
        assert!(fake.sent.lock().unwrap().is_empty(), "desktop = never");
        notify(cx, a.pane, true);
        assert_eq!(fake.sent.lock().unwrap().len(), 1, "--desktop forces it");
        cx.update(|cx| edit_config(cx, |c| c.notifications.desktop = DesktopMode::Always));
        notify(cx, a.pane, false);
        let sent = fake.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1], ("microterm".to_string(), "done".to_string()));
    }

    #[gpui::test]
    fn notify_badge_can_be_switched_off_and_never_downgrades(cx: &mut TestAppContext) {
        init(cx);
        let _ = fake_notifier(cx);
        let (a, _b) = two_tabs(cx);
        cx.update(|cx| edit_config(cx, |c| c.notifications.badge = false));
        notify(cx, a.pane, false);
        assert_eq!(chrome_of(cx, a.pane).badge, None);
        cx.update(|cx| edit_config(cx, |c| c.notifications.badge = true));
        call(
            cx,
            Request::PaneNotify(mt_proto::NotifyParams {
                target: Target::pane(a.pane),
                level: mt_proto::NotifyLevel::Error,
                message: "bad".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        notify(cx, a.pane, false); // attention after error keeps the error colour
        assert_eq!(
            chrome_of(cx, a.pane).badge,
            Some(crate::components::StatusKind::Error)
        );
    }

    #[gpui::test]
    fn notify_is_rate_limited_to_five_per_second_per_pane(cx: &mut TestAppContext) {
        init(cx);
        let _ = fake_notifier(cx);
        let (a, _b) = two_tabs(cx);
        let dropped: u32 = (0..8).map(|_| notify(cx, a.pane, false).dropped).sum();
        assert_eq!(dropped, 3);
    }

    #[gpui::test]
    fn notification_text_is_sanitised_and_cut_to_the_configured_length(cx: &mut TestAppContext) {
        init(cx);
        let fake = fake_notifier(cx);
        let (a, _b) = two_tabs(cx);
        cx.update(|cx| edit_config(cx, |c| c.ctl.status_max_len = 8));
        call(
            cx,
            Request::PaneNotify(mt_proto::NotifyParams {
                target: Target::pane(a.pane),
                desktop: true,
                message: "a\u{1b}]0;x\u{7}bcdefghijkl".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        assert_eq!(fake.sent.lock().unwrap()[0].1, "a]0;xbc…");
    }

    #[gpui::test]
    fn osc_progress_shows_on_the_tab_and_a_live_ctl_status_wins(cx: &mut TestAppContext) {
        use mt_term::ProgressState as P;
        init(cx);
        let (a, _b) = two_tabs(cx);
        cx.update(|cx| on_osc_progress(cx, a.pane, P::Normal, Some(40)));
        let c = chrome_of(cx, a.pane);
        assert_eq!(c.progress, Some(40));
        assert_eq!(c.dot, Some(crate::components::StatusKind::Accent));
        cx.update(|cx| on_osc_progress(cx, a.pane, P::Error, Some(10)));
        assert_eq!(
            chrome_of(cx, a.pane).dot,
            Some(crate::components::StatusKind::Error)
        );
        cx.update(|cx| on_osc_progress(cx, a.pane, P::Paused, Some(55)));
        assert_eq!(
            chrome_of(cx, a.pane).dot,
            Some(crate::components::StatusKind::Warning)
        );
        cx.update(|cx| on_osc_progress(cx, a.pane, P::Indeterminate, None));
        assert_eq!(chrome_of(cx, a.pane).progress, None);
        cx.update(|cx| on_osc_progress(cx, a.pane, P::Remove, None));
        assert_eq!(chrome_of(cx, a.pane), Default::default());
        // A status set through ctl wins while it is live, and OSC cannot clear it.
        call(
            cx,
            Request::PaneStatusSet(mt_proto::StatusSetParams {
                target: Target::pane(a.pane),
                state: Some(mt_proto::StatusState::Waiting),
                text: Some("approve?".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        cx.update(|cx| on_osc_progress(cx, a.pane, P::Normal, Some(90)));
        let c = chrome_of(cx, a.pane);
        assert_eq!(c.progress, None);
        assert_eq!(c.tooltip.as_deref(), Some("waiting: approve?"));
        cx.update(|cx| on_osc_progress(cx, a.pane, P::Remove, None));
        assert!(chrome_of(cx, a.pane).tooltip.is_some());
    }

    #[gpui::test]
    fn osc_9_goes_through_the_notification_pipeline(cx: &mut TestAppContext) {
        init(cx);
        let fake = fake_notifier(cx);
        let (a, _b) = two_tabs(cx);
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let l = log.clone();
        cx.update(|cx| {
            host(cx).mux.subscribe(move |e| {
                if let mt_mux::MuxEvent::Notification { source, level, .. } = e {
                    l.lock().unwrap().push((source.as_str(), *level));
                }
            })
        });
        cx.update(|cx| on_osc_notify(cx, a.pane, "build\u{1b}]0;x\u{7} done"));
        assert_eq!(
            log.lock().unwrap().as_slice(),
            [("osc9", mt_proto::NotifyLevel::Info)]
        );
        assert_eq!(fake.sent.lock().unwrap()[0].1, "build]0;x done");
        // Badge on the background tab.
        assert!(chrome_of(cx, a.pane).badge.is_some());
        // Rate limit: five per second per pane, the rest dropped.
        for _ in 0..8 {
            cx.update(|cx| on_osc_notify(cx, a.pane, "spam"));
        }
        assert_eq!(log.lock().unwrap().len(), 5);
        // Off switch.
        cx.update(|cx| edit_config(cx, |c| c.terminal.osc_notifications = false));
        let b2 = two_tabs(cx).1;
        cx.update(|cx| on_osc_notify(cx, b2.pane, "ignored"));
        assert_eq!(log.lock().unwrap().len(), 5);
    }
}
