//! The live view of the running instance that completion and notices need: a cached `mux.list`
//! snapshot invalidated by a `subscribe` connection, the connection state for the prompt, and
//! async `[event]` notices (SPEC §12.7).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mt_proto::{Event, MuxSnapshot, Request, SubscribeParams};

use crate::ctl_cmd::connect;

pub struct Shared {
    socket: Option<PathBuf>,
    snap: Mutex<Option<MuxSnapshot>>,
    dirty: AtomicBool,
    connected: AtomicBool,
    notices: AtomicBool,
    last_try: Mutex<Option<Instant>>,
    /// `[event] …` lines waiting for the next prompt.
    queue: Mutex<Vec<String>>,
    /// Snippet ids for completion, refreshed at most every two seconds.
    snippets: Mutex<(Option<Instant>, Vec<String>)>,
}

/// Structural events only: `pane.output` would invalidate the snapshot on every byte.
const WATCHED: &[&str] = &[
    "workspace.*",
    "window.*",
    "tab.*",
    "pane.added",
    "pane.removed",
    "pane.focused",
    "pane.title_changed",
    "pane.cwd_changed",
    "pane.exited",
    "pane.command_finished",
    "pane.status_changed",
    "pane.notification",
    "domain.state_changed",
];

/// The `[event] …` line for an event worth interrupting the prompt for.
pub fn notice(ev: &Event) -> Option<String> {
    Some(match ev {
        Event::PaneExited { pane, status } => match status {
            Some(s) => format!("[event] pane {pane} exited (status {s})"),
            None => format!("[event] pane {pane} exited"),
        },
        Event::CommandFinished { pane, exit_code } => match exit_code {
            Some(c) => format!("[event] pane {pane} command finished (exit {c})"),
            None => format!("[event] pane {pane} command finished"),
        },
        Event::Notification { pane, message, .. } => format!("[event] pane {pane}: {message}"),
        Event::DomainStateChanged { domain, state, .. } => {
            format!("[event] domain {domain} {state}")
        }
        _ => return None,
    })
}

impl Shared {
    pub fn new(socket: Option<PathBuf>) -> Arc<Shared> {
        Arc::new(Shared {
            socket,
            snap: Mutex::new(None),
            dirty: AtomicBool::new(true),
            connected: AtomicBool::new(true),
            notices: AtomicBool::new(true),
            last_try: Mutex::new(None),
            queue: Mutex::new(Vec::new()),
            snippets: Mutex::new((None, Vec::new())),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn set_connected(&self, up: bool) {
        self.connected.store(up, Ordering::Relaxed);
    }

    pub fn set_notices(&self, on: bool) {
        self.notices.store(on, Ordering::Relaxed);
    }

    pub fn notices(&self) -> bool {
        self.notices.load(Ordering::Relaxed)
    }

    /// The notices that arrived since the last prompt. They are shown above the next prompt:
    /// rustyline 17 loses burst input (pastes) when an external printer is attached, so the
    /// line editor is never interrupted from another thread.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.queue.lock().expect("queue lock"))
    }

    /// The snippet ids (empty while snippets are disabled or no instance answers).
    pub fn snippet_ids(&self) -> Vec<String> {
        let mut cache = self.snippets.lock().expect("snippets lock");
        if cache
            .0
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return cache.1.clone();
        }
        let ids = connect(self.socket.as_deref())
            .ok()
            .and_then(|mut c| {
                c.call(&Request::SnippetList(mt_proto::SnippetListParams::default()))
                    .ok()
            })
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .filter_map(|s| s["id"].as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        *cache = (Some(Instant::now()), ids.clone());
        ids
    }

    pub fn invalidate(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// The current snapshot, fetched again when events say it changed. `None` when no instance
    /// answers (completion then degrades to the static layer). While disconnected a fetch is
    /// tried at most once a second so typing stays instant.
    pub fn snapshot(&self) -> Option<MuxSnapshot> {
        let mut snap = self.snap.lock().expect("snapshot lock");
        let stale = self.dirty.load(Ordering::Relaxed) || snap.is_none();
        if stale {
            let mut last = self.last_try.lock().expect("last_try lock");
            if !self.is_connected() && last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                return snap.clone();
            }
            *last = Some(Instant::now());
            self.dirty.store(false, Ordering::Relaxed);
            match self.fetch() {
                Some(s) => {
                    self.set_connected(true);
                    *snap = Some(s);
                }
                None => {
                    self.set_connected(false);
                    *snap = None;
                }
            }
        }
        snap.clone()
    }

    fn fetch(&self) -> Option<MuxSnapshot> {
        let mut c = connect(self.socket.as_deref()).ok()?;
        let v = c.call(&Request::MuxList).ok()?;
        serde_json::from_value(v).ok()
    }

    /// Watches the instance on a background thread: invalidates the snapshot on every change,
    /// tracks the connection and prints notices. Reconnects every two seconds.
    pub fn spawn_listener(self: &Arc<Self>) {
        let me = Arc::clone(self);
        std::thread::spawn(move || loop {
            let params = SubscribeParams {
                types: WATCHED.iter().map(|s| s.to_string()).collect(),
                expr: None,
                snapshot: false,
            };
            match connect(me.socket.as_deref()).and_then(|c| c.subscribe_with(params)) {
                Ok(mut events) => {
                    me.set_connected(true);
                    me.invalidate();
                    loop {
                        match events.next_event() {
                            Ok(Some(ev)) => {
                                me.invalidate();
                                if me.notices() {
                                    if let Some(line) = notice(&ev) {
                                        let mut q = me.queue.lock().expect("queue lock");
                                        // A burst of events must not grow without bound.
                                        if q.len() < 100 {
                                            q.push(line);
                                        }
                                    }
                                }
                            }
                            Ok(None) => {}
                            Err(_) => {
                                me.set_connected(false);
                                break;
                            }
                        }
                    }
                }
                Err(_) => me.set_connected(false),
            }
            std::thread::sleep(Duration::from_secs(2));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_cover_exits_finished_commands_and_notifications() {
        assert_eq!(
            notice(&Event::PaneExited {
                pane: 12,
                status: Some(0)
            })
            .as_deref(),
            Some("[event] pane 12 exited (status 0)")
        );
        assert_eq!(
            notice(&Event::CommandFinished {
                pane: 3,
                exit_code: Some(1)
            })
            .as_deref(),
            Some("[event] pane 3 command finished (exit 1)")
        );
        assert_eq!(
            notice(&Event::PaneAdded { pane: 1 }),
            None,
            "structural events only invalidate the snapshot"
        );
    }

    #[test]
    fn notices_queue_until_the_next_prompt() {
        let shared = Shared::new(None);
        assert!(shared.take_notices().is_empty());
        shared
            .queue
            .lock()
            .unwrap()
            .push("[event] pane 1 exited".into());
        assert_eq!(shared.take_notices(), ["[event] pane 1 exited"]);
        assert!(shared.take_notices().is_empty());
    }

    #[test]
    fn no_instance_means_no_snapshot_and_a_disconnected_prompt() {
        let dir = std::env::temp_dir().join(format!("mt-live-{}", std::process::id()));
        let shared = Shared::new(Some(dir.join("absent.sock")));
        assert!(shared.snapshot().is_none());
        assert!(!shared.is_connected());
    }
}
