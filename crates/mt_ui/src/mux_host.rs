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
use mt_mux::{Id, Mux, SpawnSpec};
use mt_proto::{Request, RpcError, Spawned};
use mt_term::Terminal;
use serde_json::{json, Value};

use crate::ipc_ops::{self, Recorders};
use crate::{app::AppWindow, app::TabView, settings::Settings};

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
        }
    }

    pub(crate) fn forget_open(&mut self, window: Id) {
        self.open.remove(&window);
    }

    /// Adds a tab (and its launch spec) to a mux window.
    pub(crate) fn spawn_tab(&mut self, window: Id, spec: SpawnSpec) -> Option<(Id, Id)> {
        let (tab, pane) = self
            .mux
            .spawn_tab(window, spec.cwd.clone(), spec.title.clone())
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
    let mut specs: Vec<SpawnSpec> = declared
        .map(|d| {
            d.tabs
                .into_iter()
                .map(|t| SpawnSpec {
                    cwd: t.cwd.as_deref().map(expand_tilde),
                    command: (!t.command.is_empty()).then_some(t.command),
                    title: t.title,
                })
                .collect()
        })
        .unwrap_or_default();
    if specs.is_empty() {
        specs.push(SpawnSpec::default());
    }
    if let Some(first) = first {
        let title = specs[0].title.take();
        specs[0] = SpawnSpec {
            title: first.title.or(title),
            ..first
        };
    }
    for spec in specs {
        host.spawn_tab(window, spec);
    }
    // Declared tabs open in order with the first one active.
    let _ = host.mux.activate_tab(window, 0);
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
        if let Some(t) = tabs.iter().find(|t| t.pane == pane) {
            return t.session.as_ref().map(|s| s.term.clone());
        }
    }
    let handles: Vec<_> = h.open.values().copied().collect();
    handles.into_iter().find_map(|handle| {
        handle
            .read_with(cx, |view, _| {
                view.tabs
                    .iter()
                    .find(|t| t.pane == pane)
                    .and_then(|t| t.session.as_ref().map(|s| s.term.clone()))
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

fn first_window_of_active(cx: &mut App) -> Option<Id> {
    let h = host(cx);
    let ws = h.mux.active_workspace().id;
    h.mux.windows_in(ws).next().map(|w| w.id)
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
    let spec = ipc_ops::spawn_spec(p)?;
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
    let uses_first_tab = fresh && !p.new_window && p.window.is_none();
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
    h.pending.remove(&pane);
    let (window, tab, closed) = h
        .mux
        .kill_pane(pane)
        .map_err(|e| RpcError::not_found(e.to_string()))?;
    if let Some(tabs) = h.parked.get_mut(&window) {
        tabs.retain(|t| t.id != tab);
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

fn activate_tab(cx: &mut App, p: &mt_proto::ActivateTabParams) -> Reply {
    let default = first_window_of_active(cx);
    let (window, index) = {
        let h = host(cx);
        ipc_ops::resolve_activation(&h.mux, p, default)?
    };
    let ws = {
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
    if let Some(name) = ws {
        show(cx, &name)?;
    }
    host(cx)
        .mux
        .activate_tab(window, index)
        .map_err(|e| RpcError::not_found(e.to_string()))?;
    reconcile_window(cx, window, false);
    Ok(json!({}))
}

fn set_tab_title(cx: &mut App, p: &mt_proto::SetTabTitleParams) -> Reply {
    let default = first_window_of_active(cx);
    let (window, tab) = {
        let h = host(cx);
        match p.tab_id {
            Some(tab) => {
                let (w, _) = h
                    .mux
                    .tab_location(tab)
                    .ok_or_else(|| not_found("tab", tab))?;
                (w, tab)
            }
            None => {
                let w = default.ok_or_else(|| RpcError::not_found("no window"))?;
                let tab = h
                    .mux
                    .window(w)
                    .and_then(|w| w.active_tab())
                    .ok_or_else(|| RpcError::not_found("no active tab"))?
                    .id;
                (w, tab)
            }
        }
    };
    let title = (!p.title.is_empty()).then(|| p.title.clone());
    host(cx)
        .mux
        .rename_tab(window, tab, title)
        .map_err(|e| RpcError::not_found(e.to_string()))?;
    reconcile_window(cx, window, false);
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

/// Executes one IPC request on the GPUI main thread (SPEC §12.2).
pub fn handle_call(cx: &mut App, request: Request) -> Reply {
    match request {
        Request::MuxList | Request::WorkspaceList => to_json(&ipc_ops::snapshot(&host(cx).mux)),
        Request::PaneSpawn(p) => spawn(cx, &p),
        Request::WindowNew(p) => {
            // The single-instance request of a second `microterm` (SPEC §12.1).
            let spec = ipc_ops::spawn_spec(&p)?;
            let active = host(cx).mux.active_workspace().name.clone();
            match p.workspace {
                Some(ws) if ws != active => {
                    show_workspace(cx, &ws, true, Some(spec)).map_err(RpcError::internal)?;
                }
                _ => open_new_window(cx, spec),
            }
            cx.activate(true);
            Ok(json!({}))
        }
        Request::PaneSendText(p) => {
            let term = pane_terminal(cx, p.pane)?;
            ipc_ops::send_text(&term, &p);
            Ok(json!({}))
        }
        Request::PaneGetText(p) => {
            let term = pane_terminal(cx, p.pane)?;
            to_json(&ipc_ops::get_text(&term, &p)?)
        }
        Request::PaneKill { pane } => kill_pane(cx, pane),
        Request::TabActivate(p) => activate_tab(cx, &p),
        Request::TabSetTitle(p) => set_tab_title(cx, &p),
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
        Request::Subscribe => Ok(json!({})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use gpui::TestAppContext;

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

    fn call(cx: &mut TestAppContext, req: Request) -> Reply {
        cx.update(|cx| handle_call(cx, req))
    }

    fn spawned(v: Value) -> Spawned {
        serde_json::from_value(v).unwrap()
    }

    #[gpui::test]
    fn ipc_spawn_list_title_activate_and_kill(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| show_workspace(cx, "default", true, None).unwrap());
        let a = spawned(
            call(
                cx,
                Request::PaneSpawn(mt_proto::SpawnParams {
                    title: Some("one".into()),
                    ..Default::default()
                }),
            )
            .unwrap(),
        );
        let b = spawned(call(cx, Request::PaneSpawn(Default::default())).unwrap());
        assert_eq!(a.window, b.window);
        let snap: mt_proto::MuxSnapshot =
            serde_json::from_value(call(cx, Request::MuxList).unwrap()).unwrap();
        let tabs = &snap.workspaces[0].windows[0].tabs;
        assert_eq!(tabs.len(), 3, "the default tab plus two spawned ones");
        assert!(tabs[2].active);

        call(
            cx,
            Request::TabSetTitle(mt_proto::SetTabTitleParams {
                tab_id: Some(a.tab),
                title: "renamed".into(),
            }),
        )
        .unwrap();
        call(
            cx,
            Request::TabActivate(mt_proto::ActivateTabParams {
                tab_id: Some(a.tab),
                ..Default::default()
            }),
        )
        .unwrap();
        let snap: mt_proto::MuxSnapshot =
            serde_json::from_value(call(cx, Request::MuxList).unwrap()).unwrap();
        let tabs = &snap.workspaces[0].windows[0].tabs;
        assert_eq!(tabs[1].title, "renamed");
        assert!(tabs[1].active);

        call(cx, Request::PaneKill { pane: a.pane }).unwrap();
        assert_eq!(
            call(cx, Request::PaneKill { pane: a.pane })
                .unwrap_err()
                .code,
            mt_proto::ErrorCode::NotFound
        );
        assert_eq!(
            call(
                cx,
                Request::PaneGetText(mt_proto::GetTextParams {
                    pane: 9999,
                    start_line: None,
                    end_line: None,
                    escapes: false,
                })
            )
            .unwrap_err()
            .code,
            mt_proto::ErrorCode::NotFound
        );
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
                Request::PaneSpawn(mt_proto::SpawnParams {
                    workspace: Some("svc".into()),
                    ..Default::default()
                }),
            )
            .unwrap(),
        );
        cx.run_until_parked();
        let snap: mt_proto::MuxSnapshot =
            serde_json::from_value(call(cx, Request::WorkspaceList).unwrap()).unwrap();
        let svc = snap.workspaces.iter().find(|w| w.name == "svc").unwrap();
        assert!(svc.active);
        assert_eq!(svc.windows[0].tabs.len(), 1);
        assert_eq!(svc.windows[0].tabs[0].panes[0].id, first.pane);
        let w2 = spawned(
            call(
                cx,
                Request::PaneSpawn(mt_proto::SpawnParams {
                    new_window: true,
                    ..Default::default()
                }),
            )
            .unwrap(),
        );
        assert_ne!(w2.window, first.window);
        assert!(call(
            cx,
            Request::WorkspaceSwitch {
                name: "ghost".into()
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
        // A foreign domain does not exist yet.
        assert_eq!(
            call(
                cx,
                Request::PaneSpawn(mt_proto::SpawnParams {
                    domain: Some("prod".into()),
                    ..Default::default()
                })
            )
            .unwrap_err()
            .code,
            mt_proto::ErrorCode::NotFound
        );
    }
}
