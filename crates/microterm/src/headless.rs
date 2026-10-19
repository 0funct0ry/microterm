//! `microterm --headless` (hidden): the mux and IPC server with no windows. Panes are real PTYs
//! owned by this process. It exists so the CLI can be tested end to end without a display
//! (SPEC §14); it behaves like the GUI host: a pane closes when its process exits.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mt_mux::ctl::{Rows, Verb};
use mt_mux::{ConfirmClose, Id, Mux, SpawnSpec};
use mt_proto::{Call, EventHub, Request, RpcError, Spawned};
use mt_term::{SpawnOptions, TermEvent, Terminal};
use mt_ui::ipc_ops::{self, Recorders};
use serde_json::{json, Value};

type Reply = Result<Value, RpcError>;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

struct Host {
    mux: Mux,
    panes: HashMap<Id, Arc<Terminal>>,
    recorders: Recorders,
    socket: PathBuf,
    shell: String,
    term: String,
    scrollback: usize,
    env: Vec<(String, String)>,
    confirm_close: ConfirmClose,
}

pub fn run(config: Option<PathBuf>) -> i32 {
    let path = mt_config::load::config_path(config.as_deref());
    let cfg = mt_config::load::load(&path)
        .map(|l| l.config)
        .unwrap_or_default();
    let (tx, rx) = async_channel::unbounded::<Call>();
    let hub = Arc::new(EventHub::default());
    let server = match crate::ipc::start(tx, hub.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("microterm: cannot start the IPC server: {e}");
            return 1;
        }
    };
    // SAFETY: the handler only stores to an atomic.
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as libc::sighandler_t);
    }
    let mut host = Host {
        mux: Mux::new(),
        panes: HashMap::new(),
        recorders: Recorders::default(),
        socket: server.path().to_path_buf(),
        shell: cfg.shell.clone(),
        term: cfg.term.clone(),
        scrollback: cfg.scrollback_lines as usize,
        env: cfg
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        confirm_close: ipc_ops::policy_of(&cfg.confirm_close),
    };
    host.mux
        .subscribe(move |e| hub.publish(ipc_ops::event_of(e)));
    println!("{}", server.path().display());
    use std::io::Write as _;
    let _ = std::io::stdout().flush();

    while !STOP.load(Ordering::Relaxed) {
        match rx.try_recv() {
            Ok(call) => {
                let result = host.handle(call.request);
                let _ = call.reply.send(result);
                continue;
            }
            Err(async_channel::TryRecvError::Closed) => break,
            Err(async_channel::TryRecvError::Empty) => {}
        }
        host.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(server);
    0
}

impl Host {
    /// Applies terminal events: titles, output wakeups, and closing panes whose process exited.
    fn pump(&mut self) {
        let mut exited = Vec::new();
        let ids: Vec<Id> = self.panes.keys().copied().collect();
        for pane in ids {
            let term = self.panes[&pane].clone();
            for ev in term.drain_events() {
                match ev {
                    TermEvent::Title(t) => self.mux.set_pane_title(pane, &t),
                    TermEvent::Wakeup => self.mux.notify_output(pane),
                    TermEvent::ChildExit(_) => exited.push(pane),
                    _ => {}
                }
            }
        }
        for pane in exited {
            let _ = self.close_pane(pane);
        }
    }

    fn close_pane(&mut self, pane: Id) -> Result<(), RpcError> {
        let term = self.panes.remove(&pane);
        self.recorders.finish(pane, term.as_deref());
        self.mux
            .kill_pane(pane)
            .map(|_| ())
            .map_err(|e| RpcError::not_found(e.to_string()))
    }

    fn terminal(&self, pane: Id) -> Result<Arc<Terminal>, RpcError> {
        self.panes
            .get(&pane)
            .cloned()
            .ok_or_else(|| RpcError::not_found(format!("pane {pane} does not exist")))
    }

    /// Snapshot of the mux with each pane's live terminal facts.
    fn rows(&self) -> Rows {
        self.mux.rows(&|id| {
            self.panes
                .get(&id)
                .map(|t| ipc_ops::live_of(t))
                .unwrap_or_default()
        })
    }

    fn start_terminal(
        &mut self,
        ids: (Id, Id, Id),
        spec: SpawnSpec,
        size: (u16, u16),
    ) -> Result<(), RpcError> {
        let (pane, tab, window) = ids;
        let mut env = self.env.clone();
        env.extend(ipc_ops::pane_env(pane, tab, window, Some(&self.socket)));
        let opts = SpawnOptions {
            shell: self.shell.clone(),
            command: spec.command,
            cwd: spec.cwd,
            env,
            term: self.term.clone(),
            cols: size.0,
            rows: size.1,
            scrollback: self.scrollback,
            ..Default::default()
        };
        let term = Terminal::spawn(opts, || {})
            .map_err(|e| RpcError::internal(format!("could not start the shell: {e}")))?;
        self.panes.insert(pane, Arc::new(term));
        Ok(())
    }

    fn spawn(&mut self, p: &mt_proto::SpawnParams) -> Reply {
        let spec = ipc_ops::spawn_spec(p)?;
        let size = (p.cols.unwrap_or(120), p.rows.unwrap_or(34));
        let window = match p.window {
            Some(w) => {
                self.mux
                    .window(w)
                    .ok_or_else(|| RpcError::not_found(format!("window {w} does not exist")))?;
                w
            }
            None => {
                if let Some(name) = &p.workspace {
                    if self.mux.workspace_by_name(name).is_none() {
                        self.mux
                            .create_workspace(name)
                            .map_err(|e| RpcError::bad_request(e.to_string()))?;
                    }
                    self.mux
                        .switch_workspace(name)
                        .map_err(|e| RpcError::not_found(e.to_string()))?;
                }
                let ws = self.mux.active_workspace().id;
                if p.new_window {
                    self.mux.spawn_window(Some(ws))
                } else {
                    let existing = self.mux.focused_window();
                    match existing {
                        Some(w) => w,
                        None => self.mux.spawn_window(Some(ws)),
                    }
                }
            }
        };
        let (tab, pane) = self
            .mux
            .spawn_tab(window, spec.cwd.clone(), spec.title.clone())
            .map_err(|e| RpcError::not_found(e.to_string()))?;
        if let Err(e) = self.start_terminal((pane, tab, window), spec, size) {
            let _ = self.mux.kill_pane(pane);
            return Err(e);
        }
        let n = self.mux.window(window).map_or(1, |w| w.tabs.len());
        let _ = self.mux.activate_tab(window, n - 1);
        serde_json::to_value(Spawned { pane, tab, window })
            .map_err(|e| RpcError::internal(e.to_string()))
    }

    fn close(&mut self, request: &Request) -> Reply {
        let set = ipc_ops::plan_close(&self.rows(), request, self.confirm_close)?;
        for pane in &set.panes {
            if self.panes.contains_key(pane) {
                self.close_pane(*pane)?;
            }
        }
        for w in set.windows {
            let _ = self.mux.close_window(w);
        }
        Ok(json!({ "closed": set.panes }))
    }

    fn handle(&mut self, request: Request) -> Reply {
        let ok = || Ok(json!({}));
        let to = |v: &dyn erased::Ser| Ok(v.value());
        match request {
            Request::MuxList
            | Request::WorkspaceList
            | Request::WindowList(_)
            | Request::TabList(_)
            | Request::PaneList(_)
            | Request::PaneGet(_) => {
                ipc_ops::read_only(&self.rows(), &request).expect("a listing request")
            }
            Request::TabNew(p) => self.spawn(&mt_proto::SpawnParams {
                new_window: false,
                ..p
            }),
            Request::WindowNew(p) => self.spawn(&mt_proto::SpawnParams {
                new_window: true,
                ..p
            }),
            Request::WindowFocus(t) => {
                let id = ipc_ops::resolve_window(&self.rows(), &t)?;
                self.mux.focus_window(id)?;
                ok()
            }
            Request::PaneSendText(p) => {
                let panes = ipc_ops::resolve_panes(
                    &self.rows(),
                    &p.target,
                    Verb::Acting { all_allowed: true },
                )?;
                for pane in panes {
                    let term = self.terminal(pane)?;
                    ipc_ops::send_text(&term, &p);
                }
                ok()
            }
            Request::PaneGetText(p) => {
                let panes = ipc_ops::resolve_panes(&self.rows(), &p.target, Verb::ReadOnly)?;
                let term = self.terminal(panes[0])?;
                to(&ipc_ops::get_text(&term, &p)?)
            }
            Request::PaneClose(_) | Request::TabClose(_) | Request::WindowClose(_) => {
                self.close(&request)
            }
            Request::TabFocus(p) => {
                let (w, i) = ipc_ops::resolve_focus(&self.mux, &self.rows(), &p)?;
                self.mux.focus_window(w)?;
                self.mux.activate_tab(w, i)?;
                ok()
            }
            Request::TabRename(p) => {
                let (w, tab) = ipc_ops::resolve_rename(&self.mux, &self.rows(), &p.target)?;
                let title = (!p.title.is_empty()).then(|| p.title.clone());
                self.mux.rename_tab(w, tab, title)?;
                ok()
            }
            Request::WorkspaceSwitch { name } => {
                self.mux.switch_workspace(&name)?;
                ok()
            }
            Request::WorkspaceCreate { name } => {
                self.mux
                    .create_workspace(&name)
                    .map_err(|e| RpcError::bad_request(e.to_string()))?;
                ok()
            }
            Request::WorkspaceRename { old, new } => {
                let id = self
                    .mux
                    .workspace_by_name(&old)
                    .ok_or_else(|| RpcError::not_found(format!("no workspace named \"{old}\"")))?
                    .id;
                self.mux
                    .rename_workspace(id, &new)
                    .map_err(|e| RpcError::bad_request(e.to_string()))?;
                ok()
            }
            Request::WorkspaceDelete { name, force } => {
                let panes =
                    ipc_ops::plan_workspace_delete(&self.rows(), &name, force, self.confirm_close)?;
                for pane in panes {
                    if self.panes.contains_key(&pane) {
                        self.close_pane(pane)?;
                    }
                }
                self.mux.delete_workspace(&name)?;
                ok()
            }
            Request::RecordStart(p) => {
                let term = self.terminal(p.pane)?;
                self.recorders.start(p.pane, &term, &p.path, p.input)?;
                ok()
            }
            Request::RecordStop { pane } => {
                let term = self.panes.get(&pane).cloned();
                self.recorders.stop(pane, term.as_deref())?;
                ok()
            }
            Request::ReplayStart(p) => {
                let file = std::fs::File::open(&p.path)
                    .map_err(|e| RpcError::not_found(format!("cannot open {}: {e}", p.path)))?;
                mt_record::Reader::new(std::io::BufReader::new(file))
                    .map_err(|e| RpcError::bad_request(format!("{}: {e}", p.path)))?;
                let exe = std::env::current_exe().map_err(|e| RpcError::internal(e.to_string()))?;
                self.spawn(&mt_proto::SpawnParams {
                    new_window: p.new_window,
                    title: Some(format!("replay: {}", p.path)),
                    command: Some(vec![
                        exe.to_string_lossy().into_owned(),
                        "__replay".into(),
                        p.path,
                        "--speed".into(),
                        p.speed.to_string(),
                    ]),
                    ..Default::default()
                })
            }
            // There is no config file watcher to poke without a GUI.
            Request::ConfigReload | Request::Subscribe => ok(),
        }
    }
}

mod erased {
    pub trait Ser {
        fn value(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn value(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
        }
    }
}
