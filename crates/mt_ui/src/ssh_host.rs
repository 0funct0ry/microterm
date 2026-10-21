//! SSH domains in the GUI (SPEC §11): one [`mt_ssh`] link per domain, remote terminals that
//! live in tabs like local ones, connection state mirrored into the mux, and the questions
//! the connection asks the user (host keys, passwords).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use gpui::App;
use mt_mux::{DomainId, DomainState, SpawnSpec, LOCAL_DOMAIN_ID};
use mt_proto::RpcError;
use mt_ssh::config::{parse_destination, DomainSpec, SshConfig};
use mt_ssh::dialer::SshDialer;
use mt_ssh::hostkeys::default_known_hosts;
use mt_ssh::link::{Command, LinkConfig, LinkEvent, LinkHandle, LinkState};
use mt_ssh::prompt::{ChannelPrompter, PromptRequest};
use mt_ssh::runtime::SshRuntime;
use mt_ssh::session::ConnectOptions;
use mt_term::{RemoteIo, SpawnOptions, Terminal};

use crate::mux_host;
use crate::settings::Settings;

/// What it takes to reach a domain.
#[derive(Clone, Debug)]
pub struct DomainDef {
    pub name: String,
    pub spec: DomainSpec,
    pub remote_dir: String,
    pub multiplexing: bool,
    pub timeout: Duration,
}

const DEFAULT_REMOTE_DIR: &str = "~/.local/share/microterm";

fn def_of(d: &mt_config::SshDomain) -> DomainDef {
    DomainDef {
        name: d.name.clone(),
        spec: DomainSpec {
            host: d.host.clone(),
            user: d.user.clone(),
            port: d.port,
            identity_file: d.identity_file.clone(),
        },
        remote_dir: d
            .remote_dir
            .clone()
            .unwrap_or_else(|| DEFAULT_REMOTE_DIR.into()),
        multiplexing: d.multiplexing.as_deref() != Some("none"),
        timeout: Duration::from_secs(u64::from(d.connect_timeout_s.unwrap_or(10).max(1))),
    }
}

/// `ssh:[user@]host[:port]` names an ad-hoc domain (`microterm ssh host`).
pub const ADHOC_PREFIX: &str = "ssh:";

/// Looks `name` up among the configured domains, then as an ad-hoc destination.
pub fn def_for(cx: &App, name: &str) -> Option<DomainDef> {
    if let Some(d) = cx
        .global::<Settings>()
        .config
        .ssh_domains
        .iter()
        .find(|d| d.name == name)
    {
        return Some(def_of(d));
    }
    let dest = name.strip_prefix(ADHOC_PREFIX)?;
    Some(DomainDef {
        name: dest.to_string(),
        spec: parse_destination(dest),
        remote_dir: DEFAULT_REMOTE_DIR.into(),
        multiplexing: true,
        timeout: Duration::from_secs(10),
    })
}

/// Names of the configured SSH domains, in config order.
pub fn configured_names(cx: &App) -> Vec<String> {
    cx.global::<Settings>()
        .config
        .ssh_domains
        .iter()
        .map(|d| d.name.clone())
        .collect()
}

pub(crate) struct Conn {
    /// `user@host:port`, for the overlay.
    pub label: String,
    link: LinkHandle,
    connected: Arc<AtomicBool>,
    terminals: Arc<Mutex<HashMap<u64, Weak<Terminal>>>>,
}

#[derive(Default)]
pub struct SshHub {
    rt: Option<SshRuntime>,
    pub(crate) conns: HashMap<DomainId, Conn>,
    prompts: Option<async_channel::Sender<PromptRequest>>,
}

/// Dropping a remote terminal ends or detaches its remote pane.
struct PaneGuard {
    link: LinkHandle,
    pane: u64,
    connected: Arc<AtomicBool>,
}

impl Drop for PaneGuard {
    fn drop(&mut self) {
        // Closing a tab ends its process; while the link is down the process can only be left
        // running on the host (SPEC §11.4: *Close tab* detaches).
        self.link.send(Command::Close {
            pane: self.pane,
            kill: self.connected.load(Ordering::SeqCst),
        });
    }
}

fn state_of(s: &LinkState) -> DomainState {
    match s {
        LinkState::Connecting => DomainState::Connecting,
        LinkState::Connected => DomainState::Connected,
        LinkState::Reconnecting { attempt, next_in } => DomainState::Reconnecting {
            attempt: *attempt,
            next_in_secs: next_in.as_secs_f64().ceil() as u32,
        },
        LinkState::Disconnected { reason } => DomainState::Disconnected {
            reason: reason.clone(),
        },
    }
}

/// The mux domain id for a domain name from the CLI, config or launcher; starts the
/// connection the first time. `"local"` is the local domain.
pub fn resolve_domain(cx: &mut App, name: &str) -> Result<DomainId, RpcError> {
    if name == "local" {
        return Ok(LOCAL_DOMAIN_ID);
    }
    let def = def_for(cx, name)
        .ok_or_else(|| RpcError::not_found(format!("no domain named \"{name}\"")))?;
    let id = mux_host::host(cx).mux.register_ssh_domain(&def.name);
    if !mux_host::host(cx).ssh.conns.contains_key(&id) && !cfg!(test) {
        start_conn(cx, id, def).map_err(RpcError::internal)?;
    }
    Ok(id)
}

fn start_conn(cx: &mut App, id: DomainId, def: DomainDef) -> Result<(), String> {
    let first_prompt_rx = {
        let hub = &mut mux_host::host(cx).ssh;
        if hub.rt.is_none() {
            hub.rt = Some(SshRuntime::new().map_err(|e| e.to_string())?);
        }
        if hub.prompts.is_none() {
            let (tx, rx) = async_channel::unbounded();
            hub.prompts = Some(tx);
            Some(rx)
        } else {
            None
        }
    };
    if let Some(rx) = first_prompt_rx {
        cx.spawn(async move |cx| {
            while let Ok(req) = rx.recv().await {
                if cx.update(|cx| deliver_prompt(cx, req)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    let target = SshConfig::load_default().target(&def.spec);
    let label = format!("{}@{}:{}", target.user, target.host, target.port);
    let hub = &mux_host::host(cx).ssh;
    let tx = hub.prompts.clone().expect("prompt channel exists");
    let dialer = Arc::new(SshDialer {
        target,
        remote_dir: def.remote_dir.clone(),
        multiplexing: def.multiplexing,
        opts: ConnectOptions {
            known_hosts: default_known_hosts(),
            timeout: def.timeout,
            prompter: Arc::new(Arc::new(ChannelPrompter {
                domain: def.name.clone(),
                tx,
            })),
        },
    });
    let (link, events) = hub.rt.as_ref().unwrap().start_link(
        LinkConfig {
            session: def.name.clone(),
            scrollback: 10_000,
        },
        dialer,
    );
    let conn = Conn {
        label,
        link,
        connected: Arc::new(AtomicBool::new(false)),
        terminals: Default::default(),
    };
    mux_host::host(cx).ssh.conns.insert(id, conn);
    cx.spawn(async move |cx| {
        while let Ok(ev) = events.recv().await {
            if cx.update(|cx| handle_event(cx, id, ev)).is_err() {
                break;
            }
        }
    })
    .detach();
    Ok(())
}

fn handle_event(cx: &mut App, domain: DomainId, ev: LinkEvent) {
    match ev {
        LinkEvent::State(s) => {
            let host = mux_host::host(cx);
            if let Some(c) = host.ssh.conns.get(&domain) {
                c.connected
                    .store(matches!(s, LinkState::Connected), Ordering::SeqCst);
            }
            let _ = host.mux.set_domain_state(domain, state_of(&s));
            mux_host::notify_windows(cx);
        }
        LinkEvent::Bytes { pane, bytes, .. } => {
            if let Some(t) = terminal_of(cx, domain, pane) {
                t.feed(&bytes);
            }
        }
        LinkEvent::Title { .. } => {} // titles arrive in the byte stream (OSC 2) and in replays
        LinkEvent::Exited { pane, status } => {
            if let Some(t) = terminal_of(cx, domain, pane) {
                let msg = match status {
                    Some(s) => format!("\r\n[process exited with status {s}]\r\n"),
                    None => "\r\n[process exited]\r\n".to_string(),
                };
                t.feed(msg.as_bytes());
            }
        }
        LinkEvent::Notice(text) => {
            mux_host::toast_all(cx, crate::components::ToastKind::Warning, text)
        }
    }
}

fn terminal_of(cx: &mut App, domain: DomainId, pane: u64) -> Option<Arc<Terminal>> {
    mux_host::host(cx)
        .ssh
        .conns
        .get(&domain)?
        .terminals
        .lock()
        .ok()?
        .get(&pane)?
        .upgrade()
}

fn deliver_prompt(cx: &mut App, req: PromptRequest) {
    // Dropping `req` (no window to show it in) cancels the connection attempt.
    mux_host::with_first_window(cx, move |view, window, cx| {
        view.enqueue_ssh_prompt(req, window, cx)
    });
}

/// A terminal for a pane in `domain` and its `Spawn` request to the daemon.
pub(crate) fn remote_terminal(
    cx: &mut App,
    domain: DomainId,
    pane: u64,
    spec: &SpawnSpec,
    opts: SpawnOptions,
    wake: async_channel::Sender<()>,
) -> Result<Arc<Terminal>, String> {
    let conn = mux_host::host(cx)
        .ssh
        .conns
        .get(&domain)
        .ok_or("this domain has no connection")?;
    let link = conn.link.clone();
    let guard = PaneGuard {
        link: link.clone(),
        pane,
        connected: conn.connected.clone(),
    };
    let (cols, rows) = (opts.cols, opts.rows);
    let (in_link, resize_link) = (link.clone(), link.clone());
    let term = Arc::new(Terminal::remote(
        SpawnOptions {
            // Only the grid and presentation settings matter here; the process runs remotely.
            command: None,
            ..opts
        },
        RemoteIo {
            input: Box::new(move |bytes| {
                let _keep = &guard;
                in_link.send(Command::Input { pane, bytes });
            }),
            resize: Box::new(move |cols, rows| {
                resize_link.send(Command::Resize { pane, cols, rows })
            }),
        },
        move || {
            let _ = wake.try_send(());
        },
    ));
    conn.terminals
        .lock()
        .map_err(|e| e.to_string())?
        .insert(pane, Arc::downgrade(&term));
    link.send(Command::Spawn {
        pane,
        cwd: spec.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
        command: spec.command.clone(),
        // Never MICROTERM_* here: the local socket must not be reachable from a remote host (S7).
        env: Vec::new(),
        cols,
        rows,
    });
    if let Some(origin) = spec.split_from {
        link.send(Command::Split {
            pane,
            parent: origin.parent,
            axis: match origin.axis {
                mt_mux::Axis::Horizontal => 0,
                mt_mux::Axis::Vertical => 1,
            },
            permille: origin.permille,
        });
    }
    Ok(term)
}

/// Tells the daemon about the manual color of a remote tab (the daemon is authoritative: it
/// keeps the color for reconnects and other clients). `root` is the tab's root pane.
pub(crate) fn send_tab_color(cx: &mut App, domain: DomainId, root: u64, color: Option<String>) {
    if let Some(c) = mux_host::host(cx).ssh.conns.get(&domain) {
        c.link.send(Command::SetTabColor { pane: root, color });
    }
}

pub(crate) fn reconnect_now(cx: &mut App, domain: DomainId) {
    if let Some(c) = mux_host::host(cx).ssh.conns.get(&domain) {
        c.link.send(Command::ReconnectNow);
    }
}

/// `user@host:port` of a connected-or-trying domain.
pub fn label(cx: &mut App, domain: DomainId) -> Option<String> {
    mux_host::host(cx)
        .ssh
        .conns
        .get(&domain)
        .map(|c| c.label.clone())
}
