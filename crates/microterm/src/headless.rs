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
use mt_ui::ipc_pending::{CallCtx, PaneAccess, PaneRuntime};
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
    capture_max_bytes: usize,
    status_max_len: usize,
    /// `[notifications].badge`: raise the tab badge on `pane notify`.
    badge: bool,
    /// `[terminal].osc_notifications` and the rest of the VT settings for new panes.
    osc_notifications: bool,
    vt: mt_term::VtConfig,
    runtime: PaneRuntime,
}

/// The headless mux as pending `pane.run` / `pane.wait` calls see it.
struct Access<'a> {
    mux: &'a Mux,
    panes: &'a HashMap<Id, Arc<Terminal>>,
}

impl PaneAccess for Access<'_> {
    fn exists(&mut self, pane: Id) -> bool {
        self.mux.locate_pane(pane).is_some()
    }
    fn terminal(&mut self, pane: Id) -> Option<Arc<Terminal>> {
        self.panes.get(&pane).cloned()
    }
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
        capture_max_bytes: cfg.ctl.capture_max_bytes.min(usize::MAX as u64) as usize,
        status_max_len: cfg.ctl.status_max_len as usize,
        badge: cfg.notifications.badge,
        osc_notifications: cfg.terminal.osc_notifications,
        vt: mt_ui::app::vt_config(&cfg),
        runtime: PaneRuntime::default(),
    };
    host.mux
        .subscribe(move |e| hub.publish(ipc_ops::event_of(e)));
    println!("{}", server.path().display());
    use std::io::Write as _;
    let _ = std::io::stdout().flush();

    while !STOP.load(Ordering::Relaxed) {
        match rx.try_recv() {
            Ok(call) => {
                host.dispatch(call);
                continue;
            }
            Err(async_channel::TryRecvError::Closed) => break,
            Err(async_channel::TryRecvError::Empty) => {}
        }
        host.pump();
        host.poll_pending();
        host.mux.expire_statuses(mt_term::now_ms());
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
                    TermEvent::CommandFinished(code) => {
                        self.mux.notify_command_finished(pane, code)
                    }
                    TermEvent::ChildExit(status) => exited.push((pane, status)),
                    TermEvent::Progress { state, value } => {
                        ipc_ops::osc_progress(&mut self.mux, pane, state, value, mt_term::now_ms());
                    }
                    TermEvent::Notify(text) => {
                        if self.osc_notifications {
                            let d = ipc_ops::osc_notify(
                                &mut self.mux,
                                pane,
                                &text,
                                self.status_max_len,
                                mt_term::now_ms(),
                            );
                            // No window: badge only, like `pane notify`.
                            if let (Some(d), true) = (d, self.badge) {
                                self.mux.raise_attention(d.pane, d.level);
                            }
                        }
                    }
                    TermEvent::Bell
                    | TermEvent::ResetTitle
                    | TermEvent::ClipboardStore(..)
                    | TermEvent::ClipboardLoad(..)
                    | TermEvent::CursorBlinkingChange => {}
                }
            }
        }
        for (pane, status) in exited {
            self.mux.notify_exited(pane, status);
            let term = self.panes.get(&pane).cloned();
            // `run --keep` leaves the exited pane open.
            if !self.runtime.on_exit(pane, term.as_deref(), status) {
                let _ = self.close_pane(pane);
            }
        }
    }

    fn poll_pending(&mut self) {
        if self.runtime.has_pending() {
            let mut access = Access {
                mux: &self.mux,
                panes: &self.panes,
            };
            self.runtime.poll(&mut access);
        }
    }

    /// Answers one call now, or (for `pane.run` / `pane.wait`) when it is ready.
    fn dispatch(&mut self, call: Call) {
        let Call {
            request,
            reply,
            cancel,
        } = call;
        let ctx = CallCtx {
            reply: reply.clone(),
            cancel,
        };
        let answer = match request {
            Request::PaneRun(p) => self.run(p, ctx),
            Request::PaneWait(p) => {
                match ipc_ops::resolve_panes(&self.rows(), &p.target, Verb::ReadOnly) {
                    Err(e) => Some(Err(e)),
                    Ok(v) => {
                        let mut access = Access {
                            mux: &self.mux,
                            panes: &self.panes,
                        };
                        self.runtime.begin_wait(&mut access, v[0], &p, ctx)
                    }
                }
            }
            other => Some(self.handle(other)),
        };
        if let Some(a) = answer {
            let _ = reply.send(a);
        }
    }

    fn run(&mut self, p: mt_proto::RunParams, ctx: CallCtx) -> Option<Reply> {
        let cap = self.capture_max_bytes;
        if let Some(target) = p.in_pane.clone() {
            let pane = match ipc_ops::resolve_panes(
                &self.rows(),
                &target,
                Verb::Acting { all_allowed: false },
            ) {
                Ok(v) => v[0],
                Err(e) => return Some(Err(e)),
            };
            let mut access = Access {
                mux: &self.mux,
                panes: &self.panes,
            };
            return self.runtime.begin_run_in(&mut access, pane, &p, cap, ctx);
        }
        if p.spawn.command.as_ref().is_none_or(|c| c.is_empty()) {
            return Some(Err(RpcError::bad_request("run needs a command after `--`")));
        }
        let spawned = match self.spawn(&p.spawn) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        let Ok(s) = serde_json::from_value::<Spawned>(spawned) else {
            return Some(Err(RpcError::internal("bad spawn result")));
        };
        self.runtime.mark_run(s.pane, p.keep, !p.detach);
        self.runtime.begin_run_spawn(s.pane, s.tab, &p, cap, ctx)
    }

    fn close_pane(&mut self, pane: Id) -> Result<(), RpcError> {
        let term = self.panes.remove(&pane);
        self.runtime.forget(pane);
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
            ipc_ops::live_with_exit(self.panes.get(&id).map(|t| &**t), self.runtime.exit_of(id))
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
            vt: self.vt,
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

    fn split(&mut self, p: &mt_proto::PaneSplitParams) -> Reply {
        let rows = self.rows();
        let done = ipc_ops::split_model(&mut self.mux, &rows, p)?;
        let spec = SpawnSpec {
            cwd: self.mux.pane(done.pane).and_then(|pane| pane.cwd.clone()),
            command: p.command.clone().filter(|c| !c.is_empty()),
            ..Default::default()
        };
        if let Err(e) = self.start_terminal((done.pane, done.tab, done.window), spec, (120, 34)) {
            let _ = self.mux.close_pane(done.pane);
            return Err(e);
        }
        serde_json::to_value(done).map_err(|e| RpcError::internal(e.to_string()))
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
                ipc_ops::get_text(&term, &p, self.capture_max_bytes)
            }
            Request::PaneClose(_) | Request::TabClose(_) | Request::WindowClose(_) => {
                self.close(&request)
            }
            Request::PaneSplit(p) => self.split(&p),
            Request::PaneFocus(_)
            | Request::PaneZoom(_)
            | Request::PaneResize(_)
            | Request::PaneSwap(_)
            | Request::PaneRotate(_) => {
                let rows = self.rows();
                ipc_ops::layout_op(&mut self.mux, &rows, &request)
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
            Request::ConfigReload | Request::Subscribe(_) => ok(),
            Request::PaneStatusSet(p) => {
                let rows = self.rows();
                ipc_ops::status_set(
                    &mut self.mux,
                    &rows,
                    &p,
                    self.status_max_len,
                    mt_term::now_ms(),
                )
            }
            Request::PaneStatusClear(t) => {
                let rows = self.rows();
                ipc_ops::status_clear(&mut self.mux, &rows, &t)
            }
            Request::PaneStatusGet(t) => ipc_ops::status_get(&self.mux, &self.rows(), &t),
            Request::PaneNotify(p) => {
                let rows = self.rows();
                let (result, delivered) = ipc_ops::notify(
                    &mut self.mux,
                    &rows,
                    &p,
                    self.status_max_len,
                    mt_term::now_ms(),
                )?;
                // No window, so nothing is ever "in view": badge only; no toast or OS popup.
                if self.badge {
                    for d in delivered {
                        self.mux.raise_attention(d.pane, d.level);
                    }
                }
                Ok(result)
            }
            Request::PaneRun(_) | Request::PaneWait(_) => Err(RpcError::internal(
                "pane.run and pane.wait answer later; they go through dispatch()",
            )),
        }
    }
}
