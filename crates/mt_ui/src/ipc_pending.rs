//! `pane.run` and `pane.wait` (SPEC §12.4): requests that answer when a condition holds.
//!
//! Both hosts (the GUI and `--headless`) own a [`PaneRuntime`]. It remembers how panes exited
//! (the tab closes when its process ends, but `wait --exit` and `run` still need the status),
//! which panes `run --keep` must leave open, and the calls still waiting. Hosts call
//! [`PaneRuntime::poll`] a few times a second; a call whose client disconnected is dropped
//! without touching the command it started.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use mt_mux::wait::{cap_tail, timeout_error, Outcome, PaneProbe, Waiter};
use mt_mux::Id;
use mt_proto::{RpcError, RunParams, RunResult, WaitParams};
use mt_term::{now_ms, Terminal};
use serde_json::Value;

type Reply = Result<Value, RpcError>;

/// How a pane's process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitRecord {
    pub status: Option<i32>,
    pub signal: Option<i32>,
    /// The whole rendered output at exit, kept for `run` only.
    pub output: Option<String>,
    pub at_ms: u64,
}

/// What the runtime needs from its host.
pub trait PaneAccess {
    /// The pane is still part of the mux (an exited pane kept by `--keep` still is).
    fn exists(&mut self, pane: Id) -> bool;
    /// Its terminal, once started (a pane in a hidden workspace may not have one yet).
    fn terminal(&mut self, pane: Id) -> Option<Arc<Terminal>>;
}

/// How to answer a request later, and when to stop trying.
pub struct CallCtx {
    pub reply: Sender<Reply>,
    pub cancel: Arc<AtomicBool>,
}

enum Kind {
    Wait {
        pane: Id,
        waiter: Waiter,
    },
    RunSpawn {
        pane: Id,
        tab: Id,
        started_ms: u64,
        deadline_ms: Option<u64>,
        cap: usize,
    },
    RunIn {
        pane: Id,
        finished_before: u64,
        started_ms: u64,
        deadline_ms: Option<u64>,
        cap: usize,
    },
}

struct PendingCall {
    ctx: CallCtx,
    kind: Kind,
}

#[derive(Default)]
pub struct PaneRuntime {
    exits: HashMap<Id, ExitRecord>,
    keep: HashSet<Id>,
    capture: HashSet<Id>,
    calls: Vec<PendingCall>,
}

fn to_value<T: serde::Serialize>(v: &T) -> Reply {
    serde_json::to_value(v).map_err(|e| RpcError::internal(e.to_string()))
}

/// The rendered text of a terminal, one line per logical line, newline-terminated.
fn rendered(term: &Terminal) -> String {
    let lines = term.logical_lines();
    if lines.is_empty() {
        return String::new();
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

struct TermProbe {
    exists: bool,
    term: Option<Arc<Terminal>>,
    exit: Option<Option<i32>>,
}

impl PaneProbe for TermProbe {
    fn exists(&self) -> bool {
        self.exists
    }
    fn exited(&self) -> Option<Option<i32>> {
        self.exit
    }
    fn has_marks(&self) -> bool {
        self.term.as_ref().is_some_and(|t| t.has_marks())
    }
    fn finished_total(&self) -> u64 {
        self.term.as_ref().map_or(0, |t| t.commands_finished())
    }
    fn last_output_ms(&self) -> u64 {
        self.term.as_ref().map_or(0, |t| t.last_output_ms())
    }
    fn lines(&self) -> Vec<String> {
        self.term
            .as_ref()
            .map_or_else(Vec::new, |t| t.logical_lines())
    }
}

impl PaneRuntime {
    /// How the process of `pane` ended, if it did (also after its tab closed).
    pub fn exit_of(&self, pane: Id) -> Option<&ExitRecord> {
        self.exits.get(&pane)
    }

    pub fn has_pending(&self) -> bool {
        !self.calls.is_empty()
    }

    /// Prepares a spawned pane for `run`: `keep` leaves its tab open after exit, `capture`
    /// stores its whole output when it exits.
    pub fn mark_run(&mut self, pane: Id, keep: bool, capture: bool) {
        if keep {
            self.keep.insert(pane);
        }
        if capture {
            self.capture.insert(pane);
        }
    }

    /// Records that the process of `pane` ended. Returns whether the host must keep the tab
    /// open (`run --keep`); the notice is already printed into it.
    pub fn on_exit(&mut self, pane: Id, term: Option<&Terminal>, status: Option<i32>) -> bool {
        let signal = term.and_then(|t| t.exit_signal());
        let output = self
            .capture
            .remove(&pane)
            .then(|| term.map(rendered).unwrap_or_default());
        self.exits.insert(
            pane,
            ExitRecord {
                status,
                signal,
                output,
                at_ms: now_ms(),
            },
        );
        let keep = self.keep.contains(&pane);
        if keep {
            if let Some(t) = term {
                let notice = match (status, signal) {
                    (Some(c), _) => format!("\r\n[process exited with status {c}]\r\n"),
                    (None, Some(s)) => format!("\r\n[process killed by signal {s}]\r\n"),
                    (None, None) => "\r\n[process exited]\r\n".to_string(),
                };
                t.inject(notice.as_bytes());
            }
        }
        keep
    }

    /// Forgets a pane that was closed for good.
    pub fn forget(&mut self, pane: Id) {
        self.keep.remove(&pane);
        self.capture.remove(&pane);
    }

    fn probe(&self, access: &mut dyn PaneAccess, pane: Id) -> TermProbe {
        TermProbe {
            exists: access.exists(pane),
            term: access.terminal(pane),
            exit: self.exits.get(&pane).map(|e| e.status),
        }
    }

    /// `pane.wait` for the resolved `pane`. `Some` is the answer; `None` means the call is now
    /// pending and will be answered by [`PaneRuntime::poll`].
    pub fn begin_wait(
        &mut self,
        access: &mut dyn PaneAccess,
        pane: Id,
        p: &WaitParams,
        ctx: CallCtx,
    ) -> Option<Reply> {
        let probe = self.probe(access, pane);
        let now = now_ms();
        let waiter = match Waiter::new(p, &probe, now) {
            Ok(w) => w,
            Err(e) => return Some(Err(e)),
        };
        let mut call = PendingCall {
            ctx,
            kind: Kind::Wait { pane, waiter },
        };
        match self.step(access, &mut call, now) {
            Some(r) => Some(r),
            None => {
                self.calls.push(call);
                None
            }
        }
    }

    /// `pane.run` in spawn mode, after the tab for `pane` was created.
    pub fn begin_run_spawn(
        &mut self,
        pane: Id,
        tab: Id,
        p: &RunParams,
        cap: usize,
        ctx: CallCtx,
    ) -> Option<Reply> {
        if p.detach {
            return Some(Ok(serde_json::json!({ "pane": pane, "tab": tab })));
        }
        let now = now_ms();
        self.calls.push(PendingCall {
            ctx,
            kind: Kind::RunSpawn {
                pane,
                tab,
                started_ms: now,
                deadline_ms: p.timeout_ms.map(|t| now.saturating_add(t)),
                cap,
            },
        });
        None
    }

    /// `pane.run --in`: types the line into the shell of `pane` and waits for its `D` mark.
    pub fn begin_run_in(
        &mut self,
        access: &mut dyn PaneAccess,
        pane: Id,
        p: &RunParams,
        cap: usize,
        ctx: CallCtx,
    ) -> Option<Reply> {
        let Some(line) = p.line.as_deref() else {
            return Some(Err(RpcError::bad_request("run --in needs a command line")));
        };
        let Some(term) = access.terminal(pane) else {
            return Some(Err(RpcError::not_found(format!(
                "pane {pane} has no running terminal"
            ))));
        };
        if !term.has_marks() {
            return Some(Err(RpcError::needs_shell_integration(format!(
                "pane {pane} has not sent OSC 133 marks; add `eval \"$(microterm \
                 shell-integration <shell>)\"` to the shell's rc file"
            ))));
        }
        if term.busy() {
            return Some(Err(RpcError::busy(format!(
                "pane {pane} is running a command"
            ))));
        }
        let now = now_ms();
        let finished_before = term.commands_finished();
        term.write(format!("{line}\r").into_bytes());
        self.calls.push(PendingCall {
            ctx,
            kind: Kind::RunIn {
                pane,
                finished_before,
                started_ms: now,
                deadline_ms: p.timeout_ms.map(|t| now.saturating_add(t)),
                cap,
            },
        });
        None
    }

    /// Answers every pending call whose condition holds or whose time ran out.
    pub fn poll(&mut self, access: &mut dyn PaneAccess) {
        if self.calls.is_empty() {
            return;
        }
        let now = now_ms();
        let calls = std::mem::take(&mut self.calls);
        for mut call in calls {
            if call.ctx.cancel.load(Ordering::Relaxed) {
                continue; // the client left; the command it started keeps running
            }
            match self.step(access, &mut call, now) {
                Some(r) => {
                    let _ = call.ctx.reply.send(r);
                }
                None => self.calls.push(call),
            }
        }
    }

    fn step(
        &mut self,
        access: &mut dyn PaneAccess,
        call: &mut PendingCall,
        now: u64,
    ) -> Option<Reply> {
        match &mut call.kind {
            Kind::Wait { pane, waiter } => {
                let pane = *pane;
                let probe = TermProbe {
                    exists: access.exists(pane),
                    term: access.terminal(pane),
                    exit: self.exits.get(&pane).map(|e| e.status),
                };
                match waiter.poll(&probe, now) {
                    Outcome::Pending => None,
                    Outcome::Done(r) => Some(to_value(&r)),
                    Outcome::PaneGone => Some(Err(RpcError::not_found(format!(
                        "pane {pane} closed while waiting"
                    )))),
                    Outcome::Timeout => Some(Err(timeout_error(
                        &format!("waiting for pane {pane}"),
                        waiter.timeout_ms(),
                    ))),
                }
            }
            Kind::RunSpawn {
                pane,
                tab,
                started_ms,
                deadline_ms,
                cap,
            } => {
                let (pane, tab, started, cap) = (*pane, *tab, *started_ms, *cap);
                if let Some(rec) = self.exits.get_mut(&pane) {
                    let (output, truncated) = cap_tail(rec.output.take().unwrap_or_default(), cap);
                    return Some(to_value(&RunResult {
                        pane,
                        tab: Some(tab),
                        exit_code: rec.status,
                        signal: rec.signal,
                        duration_ms: Some(rec.at_ms.saturating_sub(started)),
                        output: Some(output),
                        truncated,
                        timed_out: false,
                    }));
                }
                if deadline_ms.is_some_and(|d| now >= d) {
                    let partial = access
                        .terminal(pane)
                        .map(|t| rendered(&t))
                        .unwrap_or_default();
                    let (output, truncated) = cap_tail(partial, cap);
                    return Some(to_value(&RunResult {
                        pane,
                        tab: Some(tab),
                        exit_code: None,
                        signal: None,
                        duration_ms: Some(now.saturating_sub(started)),
                        output: Some(output),
                        truncated,
                        timed_out: true,
                    }));
                }
                if !access.exists(pane) {
                    return Some(Err(RpcError::not_found(format!(
                        "pane {pane} was closed before its process exited"
                    ))));
                }
                None
            }
            Kind::RunIn {
                pane,
                finished_before,
                started_ms,
                deadline_ms,
                cap,
            } => {
                let (pane, before, started, cap) = (*pane, *finished_before, *started_ms, *cap);
                let Some(term) = access.terminal(pane) else {
                    return Some(Err(RpcError::not_found(format!(
                        "pane {pane} closed while its command was running"
                    ))));
                };
                if term.commands_finished() > before {
                    let Some(info) = term.last_command(1) else {
                        return Some(Err(RpcError::internal(
                            "the command finished but its output is no longer available",
                        )));
                    };
                    let (output, cut) = cap_tail(info.output, cap);
                    return Some(to_value(&RunResult {
                        pane,
                        tab: None,
                        exit_code: info.exit_code,
                        signal: None,
                        duration_ms: Some(now.saturating_sub(started)),
                        output: Some(output),
                        truncated: info.truncated || cut,
                        timed_out: false,
                    }));
                }
                if deadline_ms.is_some_and(|d| now >= d) {
                    let (output, truncated) =
                        cap_tail(term.running_output().unwrap_or_default(), cap);
                    return Some(to_value(&RunResult {
                        pane,
                        tab: None,
                        exit_code: None,
                        signal: None,
                        duration_ms: Some(now.saturating_sub(started)),
                        output: Some(output),
                        truncated,
                        timed_out: true,
                    }));
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    use mt_proto::{ErrorCode, WaitResult};

    use super::*;

    struct Fake {
        panes: HashMap<Id, Arc<Terminal>>,
        gone: HashSet<Id>,
    }

    impl PaneAccess for Fake {
        fn exists(&mut self, pane: Id) -> bool {
            !self.gone.contains(&pane)
        }
        fn terminal(&mut self, pane: Id) -> Option<Arc<Terminal>> {
            self.panes.get(&pane).cloned()
        }
    }

    /// A shell stand-in with integration: prompt marks, then for each typed line C, the output
    /// of `eval`, and `D;status`.
    const FIXTURE: &str = r#"
        printf '\033]133;A\007$ \033]133;B\007'
        while IFS= read -r line; do
          printf '\033]133;C\007'
          eval "$line"
          st=$?
          printf '\033]133;D;%d\007' "$st"
          printf '\033]133;A\007$ \033]133;B\007'
        done
    "#;

    fn term(command: &[&str]) -> Arc<Terminal> {
        let t = Terminal::spawn(
            mt_term::SpawnOptions {
                command: Some(command.iter().map(|s| s.to_string()).collect()),
                cols: 80,
                rows: 24,
                ..Default::default()
            },
            || {},
        )
        .unwrap();
        Arc::new(t)
    }

    fn fixture() -> Arc<Terminal> {
        term(&["/bin/sh", "-c", FIXTURE])
    }

    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn ctx() -> (CallCtx, Receiver<Reply>) {
        let (tx, rx) = channel();
        (
            CallCtx {
                reply: tx,
                cancel: Arc::default(),
            },
            rx,
        )
    }

    /// Polls until the call answers.
    fn answer(rt: &mut PaneRuntime, fake: &mut Fake, rx: &Receiver<Reply>) -> Reply {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            rt.poll(fake);
            if let Ok(r) = rx.try_recv() {
                return r;
            }
            assert!(Instant::now() < end, "no answer");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn setup(t: Arc<Terminal>) -> (PaneRuntime, Fake) {
        (
            PaneRuntime::default(),
            Fake {
                panes: HashMap::from([(1, t)]),
                gone: HashSet::new(),
            },
        )
    }

    fn run_in(line: &str, timeout_ms: Option<u64>) -> RunParams {
        RunParams {
            in_pane: Some(mt_proto::Target::pane(1)),
            line: Some(line.into()),
            timeout_ms,
            ..Default::default()
        }
    }

    fn result(v: Value) -> RunResult {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn run_in_returns_exactly_the_command_output_and_status() {
        let t = fixture();
        until("first prompt", || t.has_marks());
        let (mut rt, mut fake) = setup(t);
        let (c, rx) = ctx();
        assert!(rt
            .begin_run_in(
                &mut fake,
                1,
                &run_in("echo hello; echo world; exit_code=3; (exit 3)", None),
                1 << 20,
                c
            )
            .is_none());
        let r = result(answer(&mut rt, &mut fake, &rx).unwrap());
        assert_eq!(r.output.as_deref(), Some("hello\nworld\n"));
        assert_eq!(r.exit_code, Some(3));
        assert!(!r.truncated && !r.timed_out);
        // A second command gets only its own output.
        let (c, rx) = ctx();
        rt.begin_run_in(&mut fake, 1, &run_in("echo again", None), 1 << 20, c);
        let r = result(answer(&mut rt, &mut fake, &rx).unwrap());
        assert_eq!(r.output.as_deref(), Some("again\n"));
        assert_eq!(r.exit_code, Some(0));
    }

    #[test]
    fn run_in_truncates_keeping_the_tail() {
        let t = fixture();
        until("first prompt", || t.has_marks());
        let (mut rt, mut fake) = setup(t);
        let (c, rx) = ctx();
        rt.begin_run_in(&mut fake, 1, &run_in("seq 1 200", None), 20, c);
        let r = result(answer(&mut rt, &mut fake, &rx).unwrap());
        assert!(r.truncated);
        let out = r.output.unwrap();
        assert!(out.len() <= 20 && out.ends_with("199\n200\n"), "{out:?}");
    }

    #[test]
    fn run_in_needs_marks_and_an_idle_shell() {
        let plain = term(&["/bin/sh", "-c", "sleep 30"]);
        let (mut rt, mut fake) = setup(plain);
        let (c, _rx) = ctx();
        let e = rt
            .begin_run_in(&mut fake, 1, &run_in("ls", None), 1024, c)
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::NeedsShellIntegration);

        let t = fixture();
        until("first prompt", || t.has_marks());
        let (mut rt, mut fake) = setup(t);
        let (c, _rx1) = ctx();
        assert!(rt
            .begin_run_in(&mut fake, 1, &run_in("sleep 30", None), 1024, c)
            .is_none());
        until("sleep to start", || fake.panes[&1].busy());
        let (c, _rx2) = ctx();
        let e = rt
            .begin_run_in(&mut fake, 1, &run_in("echo no", None), 1024, c)
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Busy);
    }

    #[test]
    fn run_in_timeout_returns_partial_output_and_leaves_the_command_running() {
        let t = fixture();
        until("first prompt", || t.has_marks());
        let (mut rt, mut fake) = setup(t.clone());
        let (c, rx) = ctx();
        rt.begin_run_in(
            &mut fake,
            1,
            &run_in("echo started; sleep 30", Some(300)),
            1024,
            c,
        );
        let r = result(answer(&mut rt, &mut fake, &rx).unwrap());
        assert!(r.timed_out);
        assert!(r.output.unwrap().contains("started"));
        assert!(t.busy(), "the process is not killed");
    }

    #[test]
    fn a_cancelled_call_is_dropped_but_the_command_runs_on() {
        let t = fixture();
        until("first prompt", || t.has_marks());
        let (mut rt, mut fake) = setup(t.clone());
        let (c, rx) = ctx();
        let cancel = c.cancel.clone();
        rt.begin_run_in(
            &mut fake,
            1,
            &run_in("echo x; sleep 0.3; echo y", None),
            1024,
            c,
        );
        cancel.store(true, Ordering::Relaxed);
        rt.poll(&mut fake);
        assert!(!rt.has_pending());
        assert!(rx.try_recv().is_err());
        until("the command to finish", || t.commands_finished() == 1);
        assert_eq!(t.last_command(1).unwrap().output, "x\ny\n");
    }

    #[test]
    fn wait_match_idle_and_command_finished() {
        let t = fixture();
        until("first prompt", || t.has_marks());
        let (mut rt, mut fake) = setup(t.clone());

        // A command that prints later; wait for the new output only.
        let (c, rx) = ctx();
        let w = WaitParams {
            target: mt_proto::Target::pane(1),
            matches: vec![r"ready on (\d+)".into()],
            new_only: true,
            ..Default::default()
        };
        assert!(rt.begin_wait(&mut fake, 1, &w, c).is_none());
        t.write(b"sleep 0.2; echo ready on 3000\r".to_vec());
        let r: WaitResult =
            serde_json::from_value(answer(&mut rt, &mut fake, &rx).unwrap()).unwrap();
        assert_eq!(r.reason, "match");
        assert_eq!(r.matched.unwrap().groups, vec![Some("3000".into())]);

        until("the command to finish", || !t.busy());
        let (c, rx) = ctx();
        let w = WaitParams {
            command_finished: true,
            ..Default::default()
        };
        assert!(rt.begin_wait(&mut fake, 1, &w, c).is_none());
        t.write(b"sleep 0.2\r".to_vec());
        let r: WaitResult =
            serde_json::from_value(answer(&mut rt, &mut fake, &rx).unwrap()).unwrap();
        assert_eq!(r.reason, "command_finished");

        let (c, rx) = ctx();
        let w = WaitParams {
            idle_ms: Some(200),
            ..Default::default()
        };
        assert!(rt.begin_wait(&mut fake, 1, &w, c).is_none());
        let r: WaitResult =
            serde_json::from_value(answer(&mut rt, &mut fake, &rx).unwrap()).unwrap();
        assert_eq!(r.reason, "idle");
        assert!(r.waited_ms >= 200);
    }

    #[test]
    fn wait_times_out_with_exit_6_and_fails_when_the_pane_disappears() {
        let t = fixture();
        until("first prompt", || t.has_marks());
        let (mut rt, mut fake) = setup(t);
        let (c, rx) = ctx();
        let w = WaitParams {
            matches: vec!["never appears".into()],
            timeout_ms: Some(150),
            ..Default::default()
        };
        assert!(rt.begin_wait(&mut fake, 1, &w, c).is_none());
        let e = answer(&mut rt, &mut fake, &rx).unwrap_err();
        assert_eq!(e.code, ErrorCode::Timeout);

        let (c, rx) = ctx();
        let w = WaitParams {
            matches: vec!["never appears".into()],
            ..Default::default()
        };
        assert!(rt.begin_wait(&mut fake, 1, &w, c).is_none());
        fake.gone.insert(1);
        let e = answer(&mut rt, &mut fake, &rx).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }

    #[test]
    fn spawn_run_reports_status_output_and_keeps_the_pane_on_request() {
        let t = term(&["/bin/sh", "-c", "echo out; echo err 1>&2; exit 4"]);
        let mut rt = PaneRuntime::default();
        rt.mark_run(1, true, true);
        let mut fake = Fake {
            panes: HashMap::from([(1, t.clone())]),
            gone: HashSet::new(),
        };
        let (c, rx) = ctx();
        let p = RunParams::default();
        assert!(rt.begin_run_spawn(1, 9, &p, 1 << 20, c).is_none());
        // The host would learn of the exit through the terminal's event.
        let mut status = None;
        until("child exit", || {
            for e in t.drain_events() {
                if let mt_term::TermEvent::ChildExit(c) = e {
                    status = Some(c);
                }
            }
            status.is_some()
        });
        let keep = rt.on_exit(1, Some(&t), status.unwrap());
        assert!(keep, "--keep leaves the tab open");
        let r = result(answer(&mut rt, &mut fake, &rx).unwrap());
        assert_eq!((r.pane, r.tab, r.exit_code), (1, Some(9), Some(4)));
        let out = r.output.unwrap();
        assert!(out.contains("out\n") && out.contains("err\n"), "{out:?}");
        // The notice was printed into the kept pane.
        assert!(t
            .logical_lines()
            .iter()
            .any(|l| l == "[process exited with status 4]"));
        assert_eq!(rt.exit_of(1).unwrap().status, Some(4));
    }

    #[test]
    fn spawn_run_detach_replies_at_once_and_timeout_keeps_running() {
        let mut rt = PaneRuntime::default();
        let (c, _rx) = ctx();
        let r = rt
            .begin_run_spawn(
                5,
                6,
                &RunParams {
                    detach: true,
                    ..Default::default()
                },
                10,
                c,
            )
            .unwrap()
            .unwrap();
        let r = result(r);
        assert_eq!((r.pane, r.tab, r.exit_code), (5, Some(6), None));

        let t = term(&["/bin/sh", "-c", "echo started; sleep 30"]);
        let mut fake = Fake {
            panes: HashMap::from([(1, t)]),
            gone: HashSet::new(),
        };
        let (c, rx) = ctx();
        rt.mark_run(1, false, true);
        rt.begin_run_spawn(
            1,
            2,
            &RunParams {
                timeout_ms: Some(300),
                ..Default::default()
            },
            1024,
            c,
        );
        let r = result(answer(&mut rt, &mut fake, &rx).unwrap());
        assert!(r.timed_out);
        assert!(r.output.unwrap().contains("started"));
    }

    #[test]
    fn spawn_run_fails_when_the_pane_is_closed_by_someone_else() {
        let mut rt = PaneRuntime::default();
        let t = term(&["/bin/sh", "-c", "sleep 30"]);
        let mut fake = Fake {
            panes: HashMap::from([(1, t)]),
            gone: HashSet::from([1]),
        };
        let (c, rx) = ctx();
        rt.begin_run_spawn(1, 2, &RunParams::default(), 1024, c);
        let e = answer(&mut rt, &mut fake, &rx).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }
}
