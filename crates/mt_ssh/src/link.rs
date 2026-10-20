//! The mux link: one task per SSH domain that dials, attaches, pumps frames and reconnects
//! (SPEC §11.4).
//!
//! The UI talks to it through [`LinkHandle`]: `Command`s in, [`LinkEvent`]s out. Everything
//! network-shaped hides behind [`Dialer`], so the whole reconnect story is testable against an
//! in-process daemon without SSH.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mt_proto::mux::{self, Decoder, Msg, SpawnReq};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::backoff::{Backoff, Jitter};
use crate::prompt::BoxFuture;

pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

/// A connected transport to a `microterm-mux proxy`.
pub struct Dialed {
    pub stream: Box<dyn Duplex>,
    /// Interval for application-level pings; three silent intervals mean the link is lost.
    pub keepalive: Duration,
    /// Keeps the underlying SSH connection alive for as long as the link uses it.
    pub keep: Box<dyn Any + Send>,
    /// Shown to the user once connected (e.g. the multiplexing fallback banner).
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialFailure {
    /// Worth retrying with backoff (network down, timeout, refused).
    Transient(String),
    /// Retrying cannot help until the user acts (host key changed, auth failed).
    Fatal(String),
}

pub trait Dialer: Send + Sync {
    fn dial(&self) -> BoxFuture<Result<Dialed, DialFailure>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, next_in: Duration },
    Disconnected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    State(LinkState),
    /// Bytes for a pane. `replay` bytes begin with a terminal reset and rebuild the whole
    /// screen; live output follows them.
    Bytes {
        pane: u64,
        bytes: Vec<u8>,
        replay: bool,
    },
    Title {
        pane: u64,
        title: String,
    },
    /// A message for the user (banner/toast).
    Notice(String),
    /// The remote process ended, or the pane no longer exists on the daemon.
    Exited {
        pane: u64,
        status: Option<i32>,
    },
}

#[derive(Debug, Clone)]
pub enum Command {
    Spawn {
        pane: u64,
        cwd: Option<String>,
        command: Option<Vec<String>>,
        env: Vec<(String, String)>,
        cols: u16,
        rows: u16,
    },
    Input {
        pane: u64,
        bytes: Vec<u8>,
    },
    Resize {
        pane: u64,
        cols: u16,
        rows: u16,
    },
    /// Forget the pane here; with `kill` also end the remote process, otherwise it keeps
    /// running on the host (*Close tab* while disconnected, SPEC §11.4).
    Close {
        pane: u64,
        kill: bool,
    },
    /// `pane` was split off `parent` (local ids): tell the daemon once both exist there, so
    /// the layout can be rebuilt by a later client (SPEC §17.1). `axis` is 0 for side by side,
    /// 1 for stacked.
    Split {
        pane: u64,
        parent: u64,
        axis: u8,
        permille: u16,
    },
    /// Skip the backoff wait (or retry after a fatal failure).
    ReconnectNow,
    Shutdown,
}

pub struct LinkConfig {
    /// Daemon session name: the domain name.
    pub session: String,
    pub scrollback: u32,
}

/// The UI side of a link.
#[derive(Clone)]
pub struct LinkHandle {
    cmds: UnboundedSender<Command>,
}

impl LinkHandle {
    pub fn send(&self, c: Command) {
        let _ = self.cmds.send(c);
    }
}

struct PaneRec {
    remote: Option<u64>,
    last_seq: u64,
    spawn: Option<Command>,
    /// Latest size the UI asked for.
    size: (u16, u16),
    /// Size the `Spawn` request carried; a different `size` is sent once the pane exists.
    spawned_size: (u16, u16),
    /// `(parent, axis, permille, sent)` from [`Command::Split`].
    split: Option<(u64, u8, u16, bool)>,
}

/// `Split` messages that can go out now (both panes exist on the daemon); marks them sent.
fn due_splits(panes: &mut BTreeMap<u64, PaneRec>) -> Vec<Msg> {
    let remotes: BTreeMap<u64, u64> = panes
        .iter()
        .filter_map(|(l, p)| p.remote.map(|r| (*l, r)))
        .collect();
    let mut out = Vec::new();
    for (local, p) in panes.iter_mut() {
        let Some((parent, axis, permille, sent)) =
            p.split.as_mut().map(|s| (s.0, s.1, s.2, &mut s.3))
        else {
            continue;
        };
        if *sent {
            continue;
        }
        if let (Some(&pane), Some(&parent)) = (remotes.get(local), remotes.get(&parent)) {
            *sent = true;
            out.push(Msg::Split {
                pane,
                parent,
                axis,
                permille,
            });
        }
    }
    out
}

enum End {
    Shutdown,
    Lost(#[allow(dead_code)] String),
}

/// Starts the link task on `rt`. Events arrive on the returned channel.
pub fn start(
    rt: &tokio::runtime::Handle,
    cfg: LinkConfig,
    dialer: Arc<dyn Dialer>,
) -> (LinkHandle, async_channel::Receiver<LinkEvent>) {
    let (ctx, crx) = unbounded_channel();
    let (etx, erx) = async_channel::unbounded();
    rt.spawn(run(cfg, dialer, crx, etx));
    (LinkHandle { cmds: ctx }, erx)
}

fn emit(tx: &async_channel::Sender<LinkEvent>, e: LinkEvent) {
    let _ = tx.try_send(e);
}

/// Applies a command while there is no connection. `true` = stop waiting (reconnect / shutdown).
fn apply_offline(c: Command, panes: &mut BTreeMap<u64, PaneRec>) -> Option<bool> {
    match c {
        Command::Spawn {
            pane, cols, rows, ..
        } => {
            panes.insert(
                pane,
                PaneRec {
                    remote: None,
                    last_seq: 0,
                    spawn: Some(c),
                    size: (cols, rows),
                    spawned_size: (cols, rows),
                    split: None,
                },
            );
            None
        }
        Command::Split {
            pane,
            parent,
            axis,
            permille,
        } => {
            if let Some(p) = panes.get_mut(&pane) {
                p.split = Some((parent, axis, permille, false));
            }
            None
        }
        Command::Resize { pane, cols, rows } => {
            if let Some(p) = panes.get_mut(&pane) {
                p.size = (cols, rows);
            }
            None
        }
        Command::Close { pane, .. } => {
            panes.remove(&pane);
            None
        }
        Command::Input { .. } => None, // blocked while disconnected
        Command::ReconnectNow => Some(false),
        Command::Shutdown => Some(true),
    }
}

async fn run(
    cfg: LinkConfig,
    dialer: Arc<dyn Dialer>,
    mut cmds: UnboundedReceiver<Command>,
    events: async_channel::Sender<LinkEvent>,
) {
    let mut panes: BTreeMap<u64, PaneRec> = BTreeMap::new();
    let mut backoff = Backoff::new(Jitter::from_clock());
    let mut ever_connected = false;
    loop {
        emit(
            &events,
            LinkEvent::State(if ever_connected {
                LinkState::Reconnecting {
                    attempt: backoff.attempt().max(1),
                    next_in: Duration::ZERO,
                }
            } else {
                LinkState::Connecting
            }),
        );
        let dialed = tokio::select! {
            d = dialer.dial() => d,
            // Commands that arrive while dialing are applied after it finishes, except Shutdown.
            () = wait_shutdown(&mut cmds, &mut panes) => return,
        };
        match dialed {
            Ok(d) => {
                match session(
                    d,
                    &cfg,
                    &mut cmds,
                    &events,
                    &mut panes,
                    &mut backoff,
                    &mut ever_connected,
                )
                .await
                {
                    End::Shutdown => return,
                    End::Lost(_) => {}
                }
            }
            Err(DialFailure::Fatal(reason)) => {
                emit(
                    &events,
                    LinkEvent::State(LinkState::Disconnected { reason }),
                );
                loop {
                    match cmds.recv().await {
                        None => return,
                        Some(c) => match apply_offline(c, &mut panes) {
                            Some(true) => return,
                            Some(false) => break,
                            None => {}
                        },
                    }
                }
                continue;
            }
            Err(DialFailure::Transient(_)) => {}
        }

        // Backoff, with a visible countdown.
        let delay = backoff.next_delay();
        let attempt = backoff.attempt();
        let deadline = Instant::now() + delay;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            emit(
                &events,
                LinkEvent::State(LinkState::Reconnecting {
                    attempt,
                    next_in: left,
                }),
            );
            if left.is_zero() {
                break;
            }
            let tick = left.min(Duration::from_secs(1));
            tokio::select! {
                () = tokio::time::sleep(tick) => {}
                c = cmds.recv() => match c {
                    None => return,
                    Some(c) => match apply_offline(c, &mut panes) {
                        Some(true) => return,
                        Some(false) => break,
                        None => {}
                    },
                }
            }
        }
    }
}

/// Resolves when `Shutdown` arrives (or the handle is dropped); other commands are applied.
async fn wait_shutdown(cmds: &mut UnboundedReceiver<Command>, panes: &mut BTreeMap<u64, PaneRec>) {
    loop {
        match cmds.recv().await {
            None => return,
            Some(c) => {
                if apply_offline(c, panes) == Some(true) {
                    return;
                }
            }
        }
    }
}

async fn send(w: &mut (impl AsyncWrite + Unpin), msg: &Msg) -> Result<(), String> {
    let bytes = mux::encode(msg).map_err(|e| e.to_string())?;
    w.write_all(&bytes).await.map_err(|e| e.to_string())?;
    w.flush().await.map_err(|e| e.to_string())
}

async fn session(
    dialed: Dialed,
    cfg: &LinkConfig,
    cmds: &mut UnboundedReceiver<Command>,
    events: &async_channel::Sender<LinkEvent>,
    panes: &mut BTreeMap<u64, PaneRec>,
    backoff: &mut Backoff,
    ever_connected: &mut bool,
) -> End {
    let Dialed {
        stream,
        keepalive,
        keep: _keep,
        notice,
    } = dialed;
    let (mut r, mut w) = tokio::io::split(stream);
    let mut dec = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];

    // Handshake.
    if let Err(e) = send(&mut w, &mux::hello()).await {
        return End::Lost(e);
    }
    let hello = loop {
        if let Ok(Some(m)) = dec.next_msg() {
            break m;
        }
        match tokio::time::timeout(Duration::from_secs(15), r.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => dec.push(&buf[..n]),
            _ => return End::Lost("no reply to Hello".into()),
        }
    };
    if let Err(reason) = mux::check_hello(&hello) {
        emit(
            events,
            LinkEvent::State(LinkState::Disconnected {
                reason: reason.clone(),
            }),
        );
        // A version mismatch is not transient; wait for the user (or shutdown).
        loop {
            match cmds.recv().await {
                None => return End::Shutdown,
                Some(c) => match apply_offline(c, panes) {
                    Some(true) => return End::Shutdown,
                    Some(false) => return End::Lost(reason),
                    None => {}
                },
            }
        }
    }

    // Attach to what we know, spawn what is pending, restore sizes.
    let known: Vec<(u64, u64)> = panes
        .values()
        .filter_map(|p| p.remote.map(|id| (id, p.last_seq)))
        .collect();
    let mut out = vec![Msg::Attach {
        session: cfg.session.clone(),
        panes: known,
    }];
    for (local, p) in panes.iter_mut() {
        match (&p.remote, &p.spawn) {
            (Some(id), _) => out.push(Msg::Resize {
                pane: *id,
                cols: p.size.0,
                rows: p.size.1,
            }),
            (
                None,
                Some(Command::Spawn {
                    cwd, command, env, ..
                }),
            ) => {
                // Start at the latest size, not the one the pane was created with.
                out.push(Msg::Spawn(SpawnReq {
                    req_id: *local,
                    session: cfg.session.clone(),
                    cwd: cwd.clone(),
                    command: command.clone(),
                    env: env.clone(),
                    cols: p.size.0,
                    rows: p.size.1,
                    scrollback: cfg.scrollback,
                }));
                p.spawned_size = p.size;
            }
            _ => {}
        }
    }
    for m in &out {
        if let Err(e) = send(&mut w, m).await {
            return End::Lost(e);
        }
    }
    *ever_connected = true;
    emit(events, LinkEvent::State(LinkState::Connected));
    if let Some(n) = notice {
        emit(events, LinkEvent::Notice(n));
    }

    let ping_every = keepalive.max(Duration::from_millis(100));
    let mut ping = tokio::time::interval(ping_every);
    ping.reset();
    let mut last_rx = Instant::now();
    let mut ping_n = 0u64;
    let mut attached = false;

    loop {
        tokio::select! {
            c = cmds.recv() => {
                let Some(c) = c else { return End::Shutdown };
                let msg = match c {
                    Command::Shutdown => {
                        let _ = send(&mut w, &Msg::Detach).await;
                        return End::Shutdown;
                    }
                    Command::ReconnectNow => continue,
                    Command::Spawn { pane, cols, rows, ref cwd, ref command, ref env } => {
                        let req = SpawnReq {
                            req_id: pane,
                            session: cfg.session.clone(),
                            cwd: cwd.clone(),
                            command: command.clone(),
                            env: env.clone(),
                            cols, rows,
                            scrollback: cfg.scrollback,
                        };
                        panes.insert(pane, PaneRec { remote: None, last_seq: 0, size: (cols, rows), spawned_size: (cols, rows), spawn: Some(c), split: None });
                        Some(Msg::Spawn(req))
                    }
                    Command::Split { pane, parent, axis, permille } => {
                        if let Some(p) = panes.get_mut(&pane) {
                            p.split = Some((parent, axis, permille, false));
                        }
                        for m in due_splits(panes) {
                            if let Err(e) = send(&mut w, &m).await { return End::Lost(e); }
                        }
                        None
                    }
                    Command::Input { pane, bytes } => {
                        panes.get(&pane).and_then(|p| p.remote).map(|id| Msg::Input { pane: id, bytes })
                    }
                    Command::Resize { pane, cols, rows } => {
                        panes.get_mut(&pane).and_then(|p| {
                            p.size = (cols, rows);
                            p.remote
                        }).map(|id| Msg::Resize { pane: id, cols, rows })
                    }
                    Command::Close { pane, kill } => {
                        let rec = panes.remove(&pane);
                        rec.and_then(|p| p.remote).filter(|_| kill).map(|id| Msg::Kill { pane: id })
                    }
                };
                if let Some(m) = msg {
                    if let Err(e) = send(&mut w, &m).await { return End::Lost(e); }
                }
            }
            n = r.read(&mut buf) => {
                match n {
                    Ok(0) => return End::Lost("connection closed".into()),
                    Err(e) => return End::Lost(e.to_string()),
                    Ok(n) => {
                        last_rx = Instant::now();
                        dec.push(&buf[..n]);
                        let mut replies = Vec::new();
                        loop {
                            match dec.next_msg() {
                                Ok(Some(m)) => {
                                    if let Some(reason) = handle_msg(m, panes, events, backoff, &mut attached, &mut replies) {
                                        return End::Lost(reason);
                                    }
                                }
                                Ok(None) => break,
                                Err(e) => return End::Lost(e.to_string()),
                            }
                        }
                        replies.extend(due_splits(panes));
                        for m in &replies {
                            if let Err(e) = send(&mut w, m).await { return End::Lost(e); }
                        }
                    }
                }
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > ping_every * 3 {
                    return End::Lost("keepalive timeout".into());
                }
                ping_n += 1;
                if let Err(e) = send(&mut w, &Msg::Ping(ping_n)).await { return End::Lost(e); }
            }
        }
    }
}

/// Returns `Some(reason)` when the connection must be dropped.
fn handle_msg(
    m: Msg,
    panes: &mut BTreeMap<u64, PaneRec>,
    events: &async_channel::Sender<LinkEvent>,
    backoff: &mut Backoff,
    attached: &mut bool,
    replies: &mut Vec<Msg>,
) -> Option<String> {
    let local_of = |panes: &BTreeMap<u64, PaneRec>, remote: u64| {
        panes
            .iter()
            .find(|(_, p)| p.remote == Some(remote))
            .map(|(l, _)| *l)
    };
    match m {
        Msg::Attached { panes: listed } => {
            *attached = true;
            backoff.reset();
            // Panes we track that the daemon no longer has were killed in the meantime.
            let gone: Vec<u64> = panes
                .iter()
                .filter(|(_, p)| {
                    p.remote
                        .is_some_and(|id| !listed.iter().any(|i| i.id == id))
                })
                .map(|(l, _)| *l)
                .collect();
            for l in gone {
                panes.remove(&l);
                emit(
                    events,
                    LinkEvent::Exited {
                        pane: l,
                        status: None,
                    },
                );
            }
        }
        Msg::Spawned { req_id, pane } => {
            if let Some(p) = panes.get_mut(&req_id) {
                p.remote = Some(pane);
                p.spawn = None;
                // The window was resized while the daemon was still starting the process.
                if p.size != p.spawned_size {
                    replies.push(Msg::Resize {
                        pane,
                        cols: p.size.0,
                        rows: p.size.1,
                    });
                    p.spawned_size = p.size;
                }
            }
        }
        Msg::Output { pane, seq, bytes } => {
            if let Some(l) = local_of(panes, pane) {
                panes.get_mut(&l).unwrap().last_seq = seq;
                emit(
                    events,
                    LinkEvent::Bytes {
                        pane: l,
                        bytes,
                        replay: false,
                    },
                );
            }
        }
        Msg::Replay { pane, seq, bytes } => {
            if let Some(l) = local_of(panes, pane) {
                panes.get_mut(&l).unwrap().last_seq = seq;
                emit(
                    events,
                    LinkEvent::Bytes {
                        pane: l,
                        bytes,
                        replay: true,
                    },
                );
            }
        }
        Msg::TitleChanged { pane, title } => {
            if let Some(l) = local_of(panes, pane) {
                emit(events, LinkEvent::Title { pane: l, title });
            }
        }
        Msg::PaneExited { pane, status } => {
            if let Some(l) = local_of(panes, pane) {
                panes.remove(&l);
                emit(events, LinkEvent::Exited { pane: l, status });
            }
        }
        Msg::Error { message } => {
            // Per-request errors (unknown pane …) are not fatal to the link.
            let _ = message;
        }
        Msg::Pong(_) | Msg::Listing { .. } => {}
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests;
