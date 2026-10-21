//! The daemon: sessions of panes, any number of clients, replay on attach (SPEC §11.4, D6).
//!
//! Lock order: pane state (inside `HeadlessPane`) → pane `meta`. The registry lock is never
//! held while taking either, and the pane reader thread takes the registry only after
//! releasing `meta`.

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use mt_proto::mux::{self, ExitInfo, Msg, PaneId, PaneInfo, SpawnReq, SplitInfo};
use mt_term::{HeadlessOptions, HeadlessPane, PaneEvent};

/// Messages queued per client before it is dropped as too slow (it re-attaches and replays).
const CLIENT_QUEUE: usize = 4096;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Sub {
    client: u64,
    tx: SyncSender<Msg>,
}

#[derive(Default)]
struct Meta {
    title: String,
    exited: Option<ExitInfo>,
    subs: Vec<Sub>,
    split: Option<SplitInfo>,
    /// The tab's manual color; only ever set on a root pane (no `split`).
    color: Option<String>,
}

struct PaneEntry {
    id: PaneId,
    session: String,
    pane: OnceLock<HeadlessPane>,
    meta: Mutex<Meta>,
}

impl PaneEntry {
    fn pane(&self) -> &HeadlessPane {
        self.pane
            .get()
            .expect("pane is set before it is registered")
    }

    /// Sends `msg` to every subscriber; slow or gone ones are dropped.
    fn broadcast(&self, msg: Msg) -> bool {
        let mut m = lock(&self.meta);
        m.subs.retain(|s| s.tx.try_send(msg.clone()).is_ok());
        !m.subs.is_empty()
    }

    fn info(&self) -> PaneInfo {
        let (cols, rows) = self.pane().size();
        let m = lock(&self.meta);
        PaneInfo {
            id: self.id,
            title: m.title.clone(),
            cols: cols as u16,
            rows: rows as u16,
            exited: m.exited,
            session: self.session.clone(),
            split: m.split,
            color: m.color.clone(),
        }
    }
}

#[derive(Default)]
struct Registry {
    next_pane: PaneId,
    panes: BTreeMap<PaneId, Arc<PaneEntry>>,
}

#[derive(Clone)]
pub struct Server {
    registry: Arc<Mutex<Registry>>,
    next_client: Arc<AtomicU64>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Self {
        Server {
            registry: Arc::new(Mutex::new(Registry {
                next_pane: 1,
                panes: BTreeMap::new(),
            })),
            next_client: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Accepts connections until the listener fails. One thread per connection.
    pub fn run(&self, listener: UnixListener) {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let server = self.clone();
            let _ = std::thread::Builder::new()
                .name("mux-client".into())
                .spawn(move || {
                    if let Ok(uid) = crate::paths::peer_uid(&stream) {
                        if uid == crate::paths::uid() {
                            server.handle(stream);
                        }
                    }
                });
        }
    }

    fn find(&self, pane: PaneId) -> Option<Arc<PaneEntry>> {
        lock(&self.registry).panes.get(&pane).cloned()
    }

    fn session_panes(&self, session: &str) -> Vec<Arc<PaneEntry>> {
        lock(&self.registry)
            .panes
            .values()
            .filter(|p| p.session == session)
            .cloned()
            .collect()
    }

    fn handle(&self, stream: UnixStream) {
        let client = self.next_client.fetch_add(1, Ordering::Relaxed);
        let Ok(write_half) = stream.try_clone() else {
            return;
        };
        let (tx, rx) = sync_channel::<Msg>(CLIENT_QUEUE);
        let closer = stream.try_clone();
        std::thread::spawn(move || {
            let mut w = std::io::BufWriter::new(write_half);
            'outer: while let Ok(first) = rx.recv() {
                let mut next = Some(first);
                // Batch whatever is already queued into one flush.
                while let Some(msg) = next {
                    if mux::write_frame(&mut w, &msg).is_err() {
                        break 'outer;
                    }
                    next = rx.try_recv().ok();
                }
                if w.flush().is_err() {
                    break;
                }
            }
            if let Ok(s) = closer {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        });
        let mut r = BufReader::new(stream);
        match mux::read_frame(&mut r) {
            Ok(Some(hello)) => match mux::check_hello(&hello) {
                Ok(_) => {
                    let _ = tx.send(mux::hello());
                }
                Err(message) => {
                    let _ = tx.send(Msg::Error { message });
                    return;
                }
            },
            _ => return,
        }
        while let Ok(Some(msg)) = mux::read_frame(&mut r) {
            if !self.dispatch(client, &tx, msg) {
                break;
            }
        }
        self.unsubscribe(client);
    }

    fn unsubscribe(&self, client: u64) {
        let panes: Vec<_> = lock(&self.registry).panes.values().cloned().collect();
        for p in panes {
            lock(&p.meta).subs.retain(|s| s.client != client);
        }
    }

    /// Returns `false` to close the connection.
    fn dispatch(&self, client: u64, tx: &SyncSender<Msg>, msg: Msg) -> bool {
        let err = |message: String| {
            let _ = tx.send(Msg::Error { message });
        };
        match msg {
            Msg::Ping(n) => {
                let _ = tx.send(Msg::Pong(n));
            }
            Msg::Detach => return false,
            Msg::Attach { session, panes } => self.attach(client, tx, &session, &panes),
            Msg::Spawn(req) => self.spawn(client, tx, req),
            Msg::Input { pane, bytes } => match self.find(pane) {
                Some(p) => p.pane().write(bytes),
                None => err(format!("pane {pane} does not exist")),
            },
            Msg::Resize { pane, cols, rows } => match self.find(pane) {
                Some(p) => p.pane().resize(cols.max(1), rows.max(1)),
                None => err(format!("pane {pane} does not exist")),
            },
            Msg::Split {
                pane,
                parent,
                axis,
                permille,
            } => match (self.find(pane), self.find(parent)) {
                (Some(p), Some(q)) if p.session == q.session && pane != parent && axis <= 1 => {
                    lock(&p.meta).split = Some(SplitInfo {
                        parent,
                        axis,
                        permille: permille.clamp(1, 999),
                    });
                }
                _ => err(format!("cannot record split of pane {pane} from {parent}")),
            },
            Msg::SetTabColor { pane, color } => {
                let valid = color
                    .as_deref()
                    .map_or(Ok(()), |c| mt_proto::color::TabColor::parse(c).map(|_| ()));
                match (self.find(pane), valid) {
                    (Some(p), Ok(())) if lock(&p.meta).split.is_none() => {
                        lock(&p.meta).color = color;
                    }
                    (Some(_), Ok(())) => err(format!(
                        "pane {pane} is not the root pane of its tab; colors live on the root"
                    )),
                    (None, _) => err(format!("pane {pane} does not exist")),
                    (_, Err(e)) => err(e.to_string()),
                }
            }
            Msg::Kill { pane } => match self.find(pane) {
                Some(p) => p.pane().kill(),
                None => err(format!("pane {pane} does not exist")),
            },
            Msg::KillSession { session } => {
                for p in self.session_panes(&session) {
                    p.pane().kill();
                }
            }
            Msg::List => {
                let panes = lock(&self.registry)
                    .panes
                    .values()
                    .cloned()
                    .collect::<Vec<_>>();
                let _ = tx.send(Msg::Listing {
                    panes: panes.iter().map(|p| p.info()).collect(),
                });
            }
            other => err(format!("unexpected message (kind {})", other.kind())),
        }
        true
    }

    fn attach(&self, client: u64, tx: &SyncSender<Msg>, session: &str, known: &[(PaneId, u64)]) {
        let panes = self.session_panes(session);
        let _ = tx.send(Msg::Attached {
            panes: panes.iter().map(|p| p.info()).collect(),
        });
        for e in panes {
            let known_seq = known.iter().find(|(id, _)| *id == e.id).map(|(_, s)| *s);
            let exited = lock(&e.meta).exited;
            e.pane().replay_with(|seq, bytes| {
                if known_seq != Some(seq) || exited.is_some() {
                    let _ = tx.try_send(Msg::Replay {
                        pane: e.id,
                        seq,
                        bytes,
                    });
                }
                match exited {
                    Some(x) => {
                        let _ = tx.try_send(Msg::PaneExited {
                            pane: e.id,
                            status: x.status,
                        });
                    }
                    None => lock(&e.meta).subs.push(Sub {
                        client,
                        tx: tx.clone(),
                    }),
                }
            });
            if exited.is_some() {
                // Delivered: the exited pane is no longer needed.
                lock(&self.registry).panes.remove(&e.id);
            }
        }
    }

    fn spawn(&self, client: u64, tx: &SyncSender<Msg>, req: SpawnReq) {
        let id = {
            let mut r = lock(&self.registry);
            let id = r.next_pane;
            r.next_pane += 1;
            id
        };
        let entry = Arc::new(PaneEntry {
            id,
            session: req.session.clone(),
            pane: OnceLock::new(),
            meta: Mutex::new(Meta::default()),
        });
        // Held until the pane is registered and `Spawned` is queued, so the client sees
        // `Spawned` before any output and an instantly exiting pane is registered first.
        let mut meta = lock(&entry.meta);
        meta.subs.push(Sub {
            client,
            tx: tx.clone(),
        });
        let sink_entry = entry.clone();
        let registry = self.registry.clone();
        let sink = move |ev: PaneEvent| match ev {
            PaneEvent::Output { seq, bytes } => {
                sink_entry.broadcast(Msg::Output {
                    pane: id,
                    seq,
                    bytes,
                });
            }
            PaneEvent::Title(t) => {
                lock(&sink_entry.meta).title = t.clone();
                sink_entry.broadcast(Msg::TitleChanged { pane: id, title: t });
            }
            PaneEvent::Exited(status) => {
                lock(&sink_entry.meta).exited = Some(ExitInfo { status });
                let delivered = sink_entry.broadcast(Msg::PaneExited { pane: id, status });
                if delivered {
                    lock(&registry).panes.remove(&id);
                }
            }
        };
        let opts = HeadlessOptions {
            command: req.command.filter(|c| !c.is_empty()),
            cwd: req.cwd.map(expand_home),
            env: req.env,
            cols: req.cols.max(1),
            rows: req.rows.max(1),
            scrollback: req.scrollback as usize,
        };
        match HeadlessPane::spawn(opts, sink) {
            Ok(p) => {
                let _ = entry.pane.set(p);
                lock(&self.registry).panes.insert(id, entry.clone());
                let _ = tx.send(Msg::Spawned {
                    req_id: req.req_id,
                    pane: id,
                });
            }
            Err(e) => {
                // Show the failure in the tab instead of leaving it blank.
                let text = format!("\r\n[could not start the process: {e}]\r\n");
                let _ = tx.send(Msg::Spawned {
                    req_id: req.req_id,
                    pane: id,
                });
                let _ = tx.send(Msg::Output {
                    pane: id,
                    seq: text.len() as u64,
                    bytes: text.into_bytes(),
                });
                let _ = tx.send(Msg::PaneExited {
                    pane: id,
                    status: None,
                });
            }
        }
        drop(meta);
    }

    /// Number of panes (running, or exited and not yet delivered).
    pub fn pane_count(&self) -> usize {
        lock(&self.registry).panes.len()
    }
}

fn expand_home(p: String) -> PathBuf {
    if let Some(rest) = p
        .strip_prefix("~/")
        .or(if p == "~" { Some("") } else { None })
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}
