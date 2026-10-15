//! The process-wide mux shared by every GUI window (SPEC §5).
//!
//! `mt_mux::Mux` is the model (workspaces, windows, tabs, panes). This module adds what the
//! model cannot own: the OS windows that show it and the live terminals parked while their
//! workspace is hidden. One workspace is active per process; switching hides the windows of the
//! old workspace (their tabs are parked, shells keep running) and shows the new one's.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{App, AppContext as _, Global, WindowHandle};
use mt_mux::{Id, Mux, SpawnSpec};

use crate::{app::AppWindow, app::TabView, settings::Settings};

pub struct MuxHost {
    pub mux: Mux,
    /// Tabs of windows whose workspace is hidden, keyed by mux window id.
    pub(crate) parked: HashMap<Id, Vec<TabView>>,
    /// What to launch for panes that have no terminal yet, keyed by pane id.
    pub(crate) pending: HashMap<Id, SpawnSpec>,
    /// OS windows currently showing a mux window.
    open: HashMap<Id, WindowHandle<AppWindow>>,
}

impl Global for MuxHost {}

impl MuxHost {
    fn new() -> Self {
        MuxHost {
            mux: Mux::new(),
            parked: HashMap::new(),
            pending: HashMap::new(),
            open: HashMap::new(),
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
    let options = crate::app::window_options(cx);
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
    // Open before hiding so the app never has zero windows.
    for w in to_open {
        open_window(cx, w);
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

/// A request to open a window, from a second `microterm` invocation (SPEC §12.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewWindowRequest {
    pub workspace: Option<String>,
    pub cwd: Option<PathBuf>,
    pub command: Option<Vec<String>>,
}

pub fn handle_request(cx: &mut App, req: NewWindowRequest) {
    let spec = SpawnSpec {
        cwd: req.cwd,
        command: req.command,
        title: None,
    };
    let active = host(cx).mux.active_workspace().name.clone();
    match req.workspace {
        Some(ws) if ws != active => {
            // The workspace switch starts the workspace if needed; the spec lands on its
            // first tab.
            if let Err(e) = show_workspace(cx, &ws, true, Some(spec)) {
                eprintln!("microterm: {e}");
            }
        }
        _ => open_new_window(cx, spec),
    }
    cx.activate(true);
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
}
