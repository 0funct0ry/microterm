//! Store recordings of running panes (SPEC §10.10.2-§10.10.3). No GPUI.
//!
//! Each recording is one thread draining the pane's timestamped record tap into a
//! [`StreamWriter`]. The thread ends when the tap sender is dropped: stopping, a closed pane
//! (the `Terminal` dropped) and hitting a limit all look the same, so the file is always
//! finalised on the way out.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mt_mux::Id;
use mt_proto::RpcError;
use mt_record::{LimitHit, Limits, Origin, Sidecar, StartMeta, Store, StreamWriter};
use mt_term::{TapEvent, TapKind, Terminal};

pub const PAUSED_MARKER: &str = "input paused (no echo)";

#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    pub input: bool,
    pub redact_no_echo: bool,
    pub title: Option<String>,
    pub origin: Origin,
    pub theme: String,
    pub font: String,
    pub limits: Limits,
}

/// How a recording ended.
#[derive(Debug)]
pub struct Finished {
    pub pane: Id,
    pub result: Result<Sidecar, String>,
    pub limit: Option<LimitHit>,
}

struct Running {
    id: String,
    started: Instant,
    title: String,
    input: bool,
    handle: JoinHandle<(Result<Sidecar, String>, Option<LimitHit>)>,
}

#[derive(Default)]
pub struct StoreRecorders {
    active: HashMap<Id, Running>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInfo {
    pub id: String,
    pub title: String,
}

fn session(
    rx: mpsc::Receiver<TapEvent>,
    mut w: StreamWriter,
    t0: Instant,
    capture_input: bool,
    redact: bool,
) -> (Result<Sidecar, String>, Option<LimitHit>) {
    let mut paused = false;
    let mut limit = None;
    loop {
        let ev = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(ev) => ev,
            // Quiet pane: still push buffered events to disk so a crash loses at most a second.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = w.flush();
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let t = ev.at.saturating_duration_since(t0).as_secs_f64();
        let hit = match ev.kind {
            TapKind::Output(b) => w.output(t, &b),
            TapKind::Resize { cols, rows } => w.resize(t, cols, rows),
            TapKind::Marker(l) => w.marker(t, Some(l.as_str())),
            TapKind::Input { bytes, hidden } => {
                if !capture_input {
                    continue;
                }
                if redact && hidden == Some(true) {
                    // One marker per paused span, where the span starts.
                    if paused {
                        continue;
                    }
                    paused = true;
                    w.marker(t, Some(PAUSED_MARKER))
                } else {
                    paused = false;
                    w.input(t, &bytes)
                }
            }
        };
        match hit {
            Ok(Some(h)) => {
                limit = Some(h);
                break;
            }
            Ok(None) => {}
            Err(e) => return (Err(e.to_string()), None),
        }
    }
    drop(rx); // the Terminal notices the closed channel and drops its sender
    (w.finish().map_err(|e| e.to_string()), limit)
}

impl StoreRecorders {
    /// Joins recordings that ended on their own (limit, pane exit) and returns them.
    pub fn reap(&mut self) -> Vec<Finished> {
        let done: Vec<Id> = self
            .active
            .iter()
            .filter(|(_, r)| r.handle.is_finished())
            .map(|(p, _)| *p)
            .collect();
        done.into_iter()
            .filter_map(|p| {
                let r = self.active.remove(&p)?;
                let (result, limit) = r
                    .handle
                    .join()
                    .unwrap_or_else(|_| (Err("recorder thread panicked".into()), None));
                Some(Finished {
                    pane: p,
                    result,
                    limit,
                })
            })
            .collect()
    }

    pub fn is_recording(&self, pane: Id) -> bool {
        self.active.contains_key(&pane)
    }

    /// `(pane, recording id, elapsed)` of every running recording.
    pub fn running(&self) -> Vec<(Id, String, Duration)> {
        let mut v: Vec<_> = self
            .active
            .iter()
            .map(|(p, r)| (*p, r.id.clone(), r.started.elapsed()))
            .collect();
        v.sort();
        v
    }

    pub fn panes(&self) -> Vec<Id> {
        let mut v: Vec<Id> = self.active.keys().copied().collect();
        v.sort_unstable();
        v
    }

    pub fn start(
        &mut self,
        store: &Store,
        pane: Id,
        term: &Arc<Terminal>,
        mut opts: StartOpts,
    ) -> Result<RunInfo, RpcError> {
        // A recording that already ended by itself must not block a new one.
        let _ = self.reap();
        if self.active.contains_key(&pane) {
            return Err(RpcError::bad_request(format!(
                "pane {pane} is already being recorded"
            )));
        }
        let (cols, rows) = term.size();
        opts.origin.kind = if opts.origin.kind.is_empty() {
            "local".into()
        } else {
            opts.origin.kind
        };
        let remote = term.is_remote();
        let input_redacted = if !opts.input {
            "none"
        } else if remote {
            "n/a"
        } else if opts.redact_no_echo {
            "no_echo"
        } else {
            "none"
        };
        let mut env = std::collections::BTreeMap::new();
        for k in ["TERM", "SHELL"] {
            if let Ok(v) = std::env::var(k) {
                env.insert(k.to_string(), v);
            }
        }
        let meta = StartMeta {
            cols: cols as u16,
            rows: rows as u16,
            title: opts.title.clone(),
            origin: opts.origin,
            theme: opts.theme,
            font: opts.font,
            capture_input: opts.input,
            input_redacted: input_redacted.into(),
            env,
            app_version: env!("CARGO_PKG_VERSION").into(),
        };
        let w = StreamWriter::create(store, meta, opts.limits)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let info = RunInfo {
            id: w.id().to_string(),
            title: w.title().to_string(),
        };
        let (tx, rx) = mpsc::channel();
        let t0 = Instant::now();
        // The pane already shows a prompt and maybe more: seed the recording with the visible
        // screen as its first event, so replays and exports do not start blank.
        let seed = term.replay_bytes(0);
        if !seed.is_empty() {
            let _ = tx.send(TapEvent {
                at: t0,
                kind: TapKind::Output(seed),
            });
        }
        let (capture, redact) = (opts.input, opts.redact_no_echo);
        let handle = std::thread::Builder::new()
            .name("microterm-recording".into())
            .spawn(move || session(rx, w, t0, capture, redact))
            .map_err(|e| RpcError::internal(e.to_string()))?;
        term.set_record_tap(Some(tx));
        self.active.insert(
            pane,
            Running {
                id: info.id.clone(),
                started: t0,
                title: info.title.clone(),
                input: opts.input,
                handle,
            },
        );
        Ok(info)
    }

    pub fn marker(
        &mut self,
        pane: Id,
        term: &Terminal,
        label: Option<&str>,
    ) -> Result<(), RpcError> {
        if !self.active.contains_key(&pane) {
            return Err(RpcError::not_found(format!(
                "pane {pane} is not being recorded"
            )));
        }
        term.record_marker(label);
        Ok(())
    }

    /// Stops and saves the pane's recording. `term` is `None` when the pane is already gone
    /// (its exit finished the recording).
    pub fn stop(&mut self, pane: Id, term: Option<&Terminal>) -> Result<Sidecar, RpcError> {
        let Some(r) = self.active.remove(&pane) else {
            return Err(RpcError::not_found(format!(
                "pane {pane} is not being recorded"
            )));
        };
        if let Some(t) = term {
            t.set_record_tap(None);
        }
        let (result, _) = r
            .handle
            .join()
            .unwrap_or_else(|_| (Err("recorder thread panicked".into()), None));
        result.map_err(RpcError::internal)
    }

    /// Stops the recording of a pane that is closing; panes that are not recorded are ignored.
    pub fn finish(&mut self, pane: Id, term: Option<&Terminal>) {
        let _ = self.stop(pane, term);
    }

    /// Stops and saves every running recording (gate off, app quit).
    pub fn stop_all(&mut self, term_of: impl Fn(Id) -> Option<Arc<Terminal>>) -> Vec<Finished> {
        let mut out = self.reap();
        for pane in self.panes() {
            let term = term_of(pane);
            let r = self.stop(pane, term.as_deref());
            out.push(Finished {
                pane,
                result: r.map_err(|e| e.message),
                limit: None,
            });
        }
        out
    }

    /// Whether the pane's running recording captures input (`REC +INPUT`).
    pub fn captures_input(&self, pane: Id) -> bool {
        self.active.get(&pane).is_some_and(|r| r.input)
    }

    pub fn title_of(&self, pane: Id) -> Option<&str> {
        self.active.get(&pane).map(|r| r.title.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_term::SpawnOptions;
    use std::fs;
    use std::time::Duration;

    fn store(name: &str) -> Store {
        let d = std::env::temp_dir().join(format!("mteng-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        Store::new(d)
    }

    fn spawn(script: &str) -> Arc<Terminal> {
        let t = Terminal::spawn(
            SpawnOptions {
                shell: "/bin/sh".into(),
                cols: 80,
                rows: 5,
                ..Default::default()
            },
            || {},
        )
        .unwrap();
        t.write(format!("{script}\n").into_bytes());
        Arc::new(t)
    }

    fn wait_text(t: &Terminal, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            t.drain_events();
            let s = t.snapshot();
            if (0..s.rows).any(|r| s.row_text(r).contains(needle)) || Instant::now() > deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn cast(s: &Store, id: &str) -> String {
        fs::read_to_string(s.cast_path(id)).unwrap()
    }

    #[test]
    fn records_output_input_and_markers_then_saves() {
        let s = store("basic");
        let t = spawn("echo ready-$((1+1))");
        wait_text(&t, "ready-2");
        let mut r = StoreRecorders::default();
        let opts = StartOpts {
            input: true,
            redact_no_echo: true,
            ..Default::default()
        };
        let info = r.start(&s, 7, &t, opts.clone()).unwrap();
        assert!(r.start(&s, 7, &t, opts).is_err());
        t.write(b"echo typed-$((3+4))\n".to_vec());
        wait_text(&t, "typed-7");
        r.marker(7, &t, Some("here")).unwrap();
        let sc = r.stop(7, Some(&t)).unwrap();
        assert_eq!(sc.id, info.id);
        assert!(sc.has_input);
        assert_eq!(sc.input_redacted, "no_echo");
        assert!(sc.markers.iter().any(|m| m.label == "here"));
        let text = cast(&s, &sc.id);
        assert!(text.contains("typed-7") && text.contains("\"i\""));
        // What was on screen before recording started is the first event.
        assert!(text.contains("ready-2"), "{text}");
        assert!(r.stop(7, Some(&t)).is_err());
        let _ = fs::remove_dir_all(s.dir());
    }

    #[test]
    fn password_prompt_input_becomes_one_marker_and_no_bytes() {
        let s = store("redact");
        let t = spawn("stty -echo; echo prompt-$((1+1)); sleep 5");
        wait_text(&t, "prompt-2");
        let mut r = StoreRecorders::default();
        r.start(
            &s,
            1,
            &t,
            StartOpts {
                input: true,
                redact_no_echo: true,
                ..Default::default()
            },
        )
        .unwrap();
        t.write(b"hunter2\n".to_vec());
        t.write(b"more\n".to_vec());
        t.write(vec![3]);
        let sc = r.stop(1, Some(&t)).unwrap();
        let text = cast(&s, &sc.id);
        assert!(
            !text.contains("hunter2") && !text.contains("more\\n"),
            "{text}"
        );
        assert_eq!(text.matches(PAUSED_MARKER).count(), 1, "{text}");
        let _ = fs::remove_dir_all(s.dir());
    }

    #[test]
    fn output_only_recording_captures_no_input_and_pane_exit_saves_it() {
        let s = store("exit");
        let t = spawn("sleep 1; echo bye-$((2+3))");
        let mut r = StoreRecorders::default();
        r.start(&s, 3, &t, StartOpts::default()).unwrap();
        t.write(b"typed-but-not-recorded\n".to_vec());
        wait_text(&t, "bye-5");
        // The pane going away drops the Terminal, which closes the tap.
        drop(t);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut fin = Vec::new();
        while fin.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            fin = r.reap();
        }
        let sc = fin.pop().expect("finished").result.unwrap();
        let text = cast(&s, &sc.id);
        assert!(text.contains("bye-5") && !text.contains("\"i\""));
        assert!(!sc.has_input);
        let _ = fs::remove_dir_all(s.dir());
    }

    #[test]
    fn limit_ends_the_recording_by_itself() {
        let s = store("limit");
        let t = spawn("echo start");
        wait_text(&t, "start");
        let mut r = StoreRecorders::default();
        r.start(
            &s,
            4,
            &t,
            StartOpts {
                limits: Limits {
                    max_duration_secs: 0.0,
                    max_bytes: 300,
                },
                ..Default::default()
            },
        )
        .unwrap();
        t.write(
            b"i=0; while [ $i -lt 200 ]; do echo line-$i-xxxxxxxxxxxx; i=$((i+1)); done\n".to_vec(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut fin = Vec::new();
        while fin.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            fin = r.reap();
        }
        let f = fin.pop().expect("limit finishes the recording");
        assert_eq!(f.limit, Some(LimitHit::Size));
        assert!(f.result.is_ok());
        let _ = fs::remove_dir_all(s.dir());
    }
}
