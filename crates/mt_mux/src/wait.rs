//! The conditions of `pane wait` (SPEC §12.4) and the output cap, as plain logic over a
//! [`PaneProbe`] so it is testable with a fake pane and a fake clock.

use mt_proto::{ErrorCode, RpcError, WaitMatch, WaitParams, WaitResult, DEFAULT_WAIT_TIMEOUT_MS};
use regex::Regex;

/// What a wait needs to know about a pane. All times are Unix milliseconds.
pub trait PaneProbe {
    /// The pane is still part of the mux.
    fn exists(&self) -> bool;
    /// `Some(status)` once the process ended (`None` status: killed by a signal). Also answers
    /// for panes that already closed after exiting.
    fn exited(&self) -> Option<Option<i32>>;
    /// The shell has sent OSC 133 marks.
    fn has_marks(&self) -> bool;
    /// Count of `D` marks seen.
    fn finished_total(&self) -> u64;
    /// When PTY bytes last arrived (`0` for never).
    fn last_output_ms(&self) -> u64;
    /// Rendered text: logical lines (soft wraps joined), oldest first.
    fn lines(&self) -> Vec<String>;
}

#[derive(Debug)]
pub enum Outcome {
    Pending,
    Done(WaitResult),
    /// The pane closed and `--exit` was not asked for (exit 3).
    PaneGone,
    Timeout,
}

#[derive(Debug)]
pub struct Waiter {
    regexes: Vec<Regex>,
    idle_ms: Option<u64>,
    exit: bool,
    command_finished: bool,
    new_only: bool,
    timeout_ms: u64,
    started_ms: u64,
    base_finished: u64,
    /// Matches per regex when the wait began (`--new-only`).
    base_counts: Vec<usize>,
    scanned_output_ms: Option<u64>,
}

impl Waiter {
    /// Validates the request and takes the baseline.
    pub fn new(p: &WaitParams, probe: &dyn PaneProbe, now_ms: u64) -> Result<Waiter, RpcError> {
        if p.matches.is_empty() && p.idle_ms.is_none() && !p.exit && !p.command_finished {
            return Err(RpcError::bad_request(
                "pane wait needs a condition: --match, --idle, --exit or --command-finished",
            ));
        }
        let regexes = p
            .matches
            .iter()
            .map(|m| {
                Regex::new(m)
                    .map_err(|e| RpcError::bad_request(format!("bad regex {m:?} for --match: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if p.command_finished && !probe.has_marks() {
            return Err(RpcError::needs_shell_integration(
                "no OSC 133 marks seen in this pane; add `microterm shell-integration` to the \
                 shell's rc file",
            ));
        }
        let base_counts = if p.new_only && !regexes.is_empty() {
            let lines = probe.lines();
            regexes.iter().map(|r| count_matches(r, &lines)).collect()
        } else {
            vec![0; regexes.len()]
        };
        Ok(Waiter {
            regexes,
            idle_ms: p.idle_ms,
            exit: p.exit,
            command_finished: p.command_finished,
            new_only: p.new_only,
            timeout_ms: p.timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS),
            started_ms: now_ms,
            base_finished: probe.finished_total(),
            base_counts,
            scanned_output_ms: None,
        })
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    pub fn deadline_ms(&self) -> u64 {
        self.started_ms.saturating_add(self.timeout_ms)
    }

    fn done(&self, now_ms: u64, reason: &str) -> WaitResult {
        WaitResult {
            reason: reason.into(),
            waited_ms: now_ms.saturating_sub(self.started_ms),
            matched: None,
            exit_code: None,
        }
    }

    pub fn poll(&mut self, probe: &dyn PaneProbe, now_ms: u64) -> Outcome {
        if !probe.exists() {
            return match (self.exit, probe.exited()) {
                (true, status) => {
                    let mut r = self.done(now_ms, "exit");
                    r.exit_code = status.flatten();
                    Outcome::Done(r)
                }
                (false, _) => Outcome::PaneGone,
            };
        }
        if self.exit {
            if let Some(status) = probe.exited() {
                let mut r = self.done(now_ms, "exit");
                r.exit_code = status;
                return Outcome::Done(r);
            }
        }
        if self.command_finished && probe.finished_total() > self.base_finished {
            return Outcome::Done(self.done(now_ms, "command_finished"));
        }
        if !self.regexes.is_empty() {
            let out = probe.last_output_ms();
            if self.scanned_output_ms != Some(out) {
                self.scanned_output_ms = Some(out);
                if let Some(m) = self.scan(&probe.lines()) {
                    let mut r = self.done(now_ms, "match");
                    r.matched = Some(m);
                    return Outcome::Done(r);
                }
            }
        }
        if let Some(idle) = self.idle_ms {
            let quiet_since = probe.last_output_ms().max(self.started_ms);
            if now_ms.saturating_sub(quiet_since) >= idle {
                return Outcome::Done(self.done(now_ms, "idle"));
            }
        }
        if now_ms >= self.deadline_ms() {
            return Outcome::Timeout;
        }
        Outcome::Pending
    }

    fn scan(&mut self, lines: &[String]) -> Option<WaitMatch> {
        for (i, re) in self.regexes.iter().enumerate() {
            let found = matches_in(re, lines);
            if !self.new_only {
                if let Some(m) = found.into_iter().next() {
                    return Some(m);
                }
                continue;
            }
            let base = &mut self.base_counts[i];
            if found.len() > *base {
                return found.into_iter().nth(*base);
            }
            // Old lines scrolled away: do not wait for the count to climb back.
            *base = found.len();
        }
        None
    }
}

fn count_matches(re: &Regex, lines: &[String]) -> usize {
    lines.iter().map(|l| re.find_iter(l).count()).sum()
}

fn matches_in(re: &Regex, lines: &[String]) -> Vec<WaitMatch> {
    let mut out = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        for caps in re.captures_iter(text) {
            out.push(WaitMatch {
                text: caps[0].to_string(),
                groups: caps
                    .iter()
                    .skip(1)
                    .map(|g| g.map(|g| g.as_str().to_string()))
                    .collect(),
                line,
            });
        }
    }
    out
}

/// Keeps the last `max` bytes of `text` (on a character boundary). Returns whether it cut.
pub fn cap_tail(text: String, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), true)
}

/// The error for a wait that ran out of time.
pub fn timeout_error(what: &str, waited_ms: u64) -> RpcError {
    RpcError::new(
        ErrorCode::Timeout,
        format!("timed out after {waited_ms} ms {what}"),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(Default)]
    struct Fake {
        gone: Cell<bool>,
        exited: Cell<Option<Option<i32>>>,
        marks: Cell<bool>,
        finished: Cell<u64>,
        output_ms: Cell<u64>,
        lines: RefCell<Vec<String>>,
    }

    impl PaneProbe for Fake {
        fn exists(&self) -> bool {
            !self.gone.get()
        }
        fn exited(&self) -> Option<Option<i32>> {
            self.exited.get()
        }
        fn has_marks(&self) -> bool {
            self.marks.get()
        }
        fn finished_total(&self) -> u64 {
            self.finished.get()
        }
        fn last_output_ms(&self) -> u64 {
            self.output_ms.get()
        }
        fn lines(&self) -> Vec<String> {
            self.lines.borrow().clone()
        }
    }

    fn params() -> WaitParams {
        WaitParams::default()
    }

    fn say(f: &Fake, t: u64, line: &str) {
        f.lines.borrow_mut().push(line.into());
        f.output_ms.set(t);
    }

    #[test]
    fn needs_a_condition_and_valid_regexes() {
        let f = Fake::default();
        let e = Waiter::new(&params(), &f, 0).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
        let e = Waiter::new(
            &WaitParams {
                matches: vec!["(".into()],
                ..params()
            },
            &f,
            0,
        )
        .unwrap_err();
        assert!(e.message.contains("bad regex"));
    }

    #[test]
    fn match_in_existing_text_is_immediate_with_groups_and_line() {
        let f = Fake::default();
        say(&f, 1, "compiling");
        say(&f, 2, "listening on http://localhost:3000");
        let p = WaitParams {
            matches: vec![r"listening on (\S+):(\d+)".into()],
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 10).unwrap();
        let Outcome::Done(r) = w.poll(&f, 25) else {
            panic!("expected a match")
        };
        assert_eq!(r.reason, "match");
        assert_eq!(r.waited_ms, 15);
        let m = r.matched.unwrap();
        assert_eq!(m.line, 1);
        assert_eq!(
            m.groups,
            vec![Some("http://localhost".into()), Some("3000".into())]
        );
        assert!(m.text.starts_with("listening on"));
    }

    #[test]
    fn new_only_ignores_what_was_already_there() {
        let f = Fake::default();
        say(&f, 1, "ready on 1");
        let p = WaitParams {
            matches: vec!["ready on".into()],
            new_only: true,
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 5).unwrap();
        assert!(matches!(w.poll(&f, 6), Outcome::Pending));
        say(&f, 7, "ready on 2");
        let Outcome::Done(r) = w.poll(&f, 8) else {
            panic!()
        };
        assert_eq!(r.matched.unwrap().line, 1);
    }

    #[test]
    fn new_only_survives_old_matches_scrolling_away() {
        let f = Fake::default();
        say(&f, 1, "ready on 1");
        say(&f, 2, "ready on 1");
        let p = WaitParams {
            matches: vec!["ready".into()],
            new_only: true,
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 5).unwrap();
        *f.lines.borrow_mut() = vec!["other".into()];
        f.output_ms.set(6);
        assert!(matches!(w.poll(&f, 6), Outcome::Pending));
        say(&f, 7, "ready again");
        assert!(matches!(w.poll(&f, 8), Outcome::Done(_)));
    }

    #[test]
    fn scans_only_when_output_arrived() {
        let f = Fake::default();
        let p = WaitParams {
            matches: vec!["x".into()],
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 0).unwrap();
        assert!(matches!(w.poll(&f, 1), Outcome::Pending));
        // Text changes without the output clock moving are not rescanned (cannot happen live).
        f.lines.borrow_mut().push("x".into());
        assert!(matches!(w.poll(&f, 2), Outcome::Pending));
        f.output_ms.set(3);
        assert!(matches!(w.poll(&f, 4), Outcome::Done(_)));
    }

    #[test]
    fn idle_counts_from_the_last_output_or_the_start() {
        let f = Fake::default();
        f.output_ms.set(100);
        let p = WaitParams {
            idle_ms: Some(500),
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 1_000).unwrap();
        // Output long before the wait started: quiet counts from the start.
        assert!(matches!(w.poll(&f, 1_400), Outcome::Pending));
        f.output_ms.set(1_450);
        assert!(matches!(w.poll(&f, 1_900), Outcome::Pending));
        let Outcome::Done(r) = w.poll(&f, 1_950) else {
            panic!()
        };
        assert_eq!(r.reason, "idle");
        assert_eq!(r.waited_ms, 950);
    }

    #[test]
    fn exit_reports_the_status_even_after_the_pane_closed() {
        let f = Fake::default();
        let p = WaitParams {
            exit: true,
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 0).unwrap();
        assert!(matches!(w.poll(&f, 1), Outcome::Pending));
        f.exited.set(Some(Some(3)));
        let Outcome::Done(r) = w.poll(&f, 2) else {
            panic!()
        };
        assert_eq!((r.reason.as_str(), r.exit_code), ("exit", Some(3)));
        // Closed after exiting (the tab closes on exit).
        let f = Fake::default();
        f.gone.set(true);
        f.exited.set(Some(Some(0)));
        let mut w = Waiter::new(&p, &f, 0).unwrap();
        let Outcome::Done(r) = w.poll(&f, 1) else {
            panic!()
        };
        assert_eq!(r.exit_code, Some(0));
    }

    #[test]
    fn a_pane_that_disappears_fails_unless_exit_was_requested() {
        let f = Fake::default();
        let p = WaitParams {
            matches: vec!["x".into()],
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 0).unwrap();
        f.gone.set(true);
        assert!(matches!(w.poll(&f, 1), Outcome::PaneGone));
    }

    #[test]
    fn command_finished_needs_marks_and_waits_for_the_next_d() {
        let f = Fake::default();
        let p = WaitParams {
            command_finished: true,
            ..params()
        };
        let e = Waiter::new(&p, &f, 0).unwrap_err();
        assert_eq!(e.code, ErrorCode::NeedsShellIntegration);
        f.marks.set(true);
        f.finished.set(4);
        let mut w = Waiter::new(&p, &f, 0).unwrap();
        assert!(
            matches!(w.poll(&f, 1), Outcome::Pending),
            "earlier commands do not count"
        );
        f.finished.set(5);
        let Outcome::Done(r) = w.poll(&f, 2) else {
            panic!()
        };
        assert_eq!(r.reason, "command_finished");
    }

    #[test]
    fn first_condition_wins_and_timeout_applies() {
        let f = Fake::default();
        f.marks.set(true);
        let p = WaitParams {
            matches: vec!["never".into()],
            idle_ms: Some(300),
            command_finished: true,
            timeout_ms: Some(1_000),
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 0).unwrap();
        // Idle fires first (300 ms of quiet) although the others are still open.
        assert!(matches!(w.poll(&f, 299), Outcome::Pending));
        assert!(matches!(w.poll(&f, 300), Outcome::Done(r) if r.reason == "idle"));
        let p = WaitParams {
            matches: vec!["never".into()],
            timeout_ms: Some(1_000),
            ..params()
        };
        let mut w = Waiter::new(&p, &f, 50).unwrap();
        assert_eq!(w.deadline_ms(), 1_050);
        assert!(matches!(w.poll(&f, 1_049), Outcome::Pending));
        assert!(matches!(w.poll(&f, 1_050), Outcome::Timeout));
        // The default is 60 s.
        let w = Waiter::new(
            &WaitParams {
                exit: true,
                ..params()
            },
            &f,
            0,
        )
        .unwrap();
        assert_eq!(w.deadline_ms(), 60_000);
    }

    #[test]
    fn cap_tail_keeps_the_end_on_a_char_boundary() {
        assert_eq!(cap_tail("hello".into(), 10), ("hello".into(), false));
        assert_eq!(cap_tail("hello".into(), 3), ("llo".into(), true));
        // "é" is two bytes; cutting inside it moves forward.
        assert_eq!(cap_tail("aéb".into(), 2), ("b".into(), true));
        assert_eq!(cap_tail("aéb".into(), 3), ("éb".into(), true));
        assert_eq!(cap_tail("abc".into(), 0), ("".into(), true));
    }
}
