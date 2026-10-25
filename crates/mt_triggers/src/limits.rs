//! Rate limiters (§10.11.3, §10.11.4). The clock is injected: every call takes `now_ms`.

use std::collections::{HashMap, VecDeque};

pub const PER_PANE_PER_SEC: usize = 5;
pub const GLOBAL_PER_SEC: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Run the actions.
    Run,
    /// Inside the rule's own cooldown: dropped silently.
    Cooldown,
    /// Over the per-pane or global limit: dropped and counted as `suppressed`.
    Suppressed,
}

/// Per-action limits: minimum gap per rule, per pane, and a global `n` runs per `window_ms`.
struct ActionLimit {
    per_rule_ms: u64,
    per_pane_ms: u64,
    global: Option<(usize, u64)>,
}

fn action_limit(kind: &str) -> Option<ActionLimit> {
    Some(match kind {
        "bell" => ActionLimit {
            per_rule_ms: 0,
            per_pane_ms: 1000,
            global: None,
        },
        "clipboard" => ActionLimit {
            per_rule_ms: 5000,
            per_pane_ms: 0,
            global: Some((1, 2000)),
        },
        "snapshot" => ActionLimit {
            per_rule_ms: 2000,
            per_pane_ms: 0,
            global: Some((10, 60_000)),
        },
        "screenshot" => ActionLimit {
            per_rule_ms: 10_000,
            per_pane_ms: 0,
            global: None,
        },
        _ => return None,
    })
}

fn prune(q: &mut VecDeque<u64>, now: u64, window: u64) {
    while q.front().is_some_and(|&t| now.saturating_sub(t) >= window) {
        q.pop_front();
    }
}

#[derive(Default)]
pub struct Limiter {
    last_fire: HashMap<(String, u64), u64>,
    pane_runs: HashMap<u64, VecDeque<u64>>,
    global_runs: VecDeque<u64>,
    action_rule: HashMap<(String, &'static str), u64>,
    action_pane: HashMap<(u64, &'static str), u64>,
    action_global: HashMap<&'static str, VecDeque<u64>>,
    suppressed: HashMap<String, u64>,
}

impl Limiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decides whether `rule` may run its actions on `pane` now.
    pub fn admit(
        &mut self,
        rule: &str,
        pane: u64,
        cooldown_ms: u32,
        quiet: bool,
        now: u64,
    ) -> Verdict {
        let key = (rule.to_string(), pane);
        if let Some(&last) = self.last_fire.get(&key) {
            if now.saturating_sub(last) < u64::from(cooldown_ms) {
                return Verdict::Cooldown;
            }
        }
        if quiet {
            // Neither limited by nor counted against the pane and global budgets.
            self.last_fire.insert(key, now);
            return Verdict::Run;
        }
        let pane_q = self.pane_runs.entry(pane).or_default();
        prune(pane_q, now, 1000);
        prune(&mut self.global_runs, now, 1000);
        if pane_q.len() >= PER_PANE_PER_SEC || self.global_runs.len() >= GLOBAL_PER_SEC {
            *self.suppressed.entry(rule.to_string()).or_default() += 1;
            return Verdict::Suppressed;
        }
        pane_q.push_back(now);
        self.global_runs.push_back(now);
        self.last_fire.insert(key, now);
        Verdict::Run
    }

    /// Like [`admit`](Self::admit) but records nothing (the cheap early drop of a hit before the
    /// pane's facts are gathered). A refusal by a rate limit is still counted as suppressed.
    pub fn peek(
        &mut self,
        rule: &str,
        pane: u64,
        cooldown_ms: u32,
        quiet: bool,
        now: u64,
    ) -> Verdict {
        if let Some(&last) = self.last_fire.get(&(rule.to_string(), pane)) {
            if now.saturating_sub(last) < u64::from(cooldown_ms) {
                return Verdict::Cooldown;
            }
        }
        if quiet {
            return Verdict::Run;
        }
        let pane_q = self.pane_runs.entry(pane).or_default();
        prune(pane_q, now, 1000);
        prune(&mut self.global_runs, now, 1000);
        if pane_q.len() >= PER_PANE_PER_SEC || self.global_runs.len() >= GLOBAL_PER_SEC {
            *self.suppressed.entry(rule.to_string()).or_default() += 1;
            return Verdict::Suppressed;
        }
        Verdict::Run
    }

    /// Whether the action `kind` of `rule` may run on `pane` now (§10.11.4 limits). A refusal is
    /// counted as `suppressed` for the rule. Kinds without a limit always pass.
    pub fn admit_action(&mut self, kind: &'static str, rule: &str, pane: u64, now: u64) -> bool {
        let Some(l) = action_limit(kind) else {
            return true;
        };
        let rule_ok = l.per_rule_ms == 0
            || self
                .action_rule
                .get(&(rule.to_string(), kind))
                .is_none_or(|&t| now.saturating_sub(t) >= l.per_rule_ms);
        let pane_ok = l.per_pane_ms == 0
            || self
                .action_pane
                .get(&(pane, kind))
                .is_none_or(|&t| now.saturating_sub(t) >= l.per_pane_ms);
        let global_ok = match l.global {
            None => true,
            Some((n, window)) => {
                let q = self.action_global.entry(kind).or_default();
                prune(q, now, window);
                q.len() < n
            }
        };
        if !(rule_ok && pane_ok && global_ok) {
            *self.suppressed.entry(rule.to_string()).or_default() += 1;
            return false;
        }
        self.action_rule.insert((rule.to_string(), kind), now);
        self.action_pane.insert((pane, kind), now);
        if l.global.is_some() {
            self.action_global.entry(kind).or_default().push_back(now);
        }
        true
    }

    /// Runs dropped for `rule` by the rate limits.
    pub fn suppressed(&self, rule: &str) -> u64 {
        self.suppressed.get(rule).copied().unwrap_or(0)
    }

    /// Forgets a pane (it exited).
    pub fn forget_pane(&mut self, pane: u64) {
        self.pane_runs.remove(&pane);
        self.last_fire.retain(|(_, p), _| *p != pane);
        self.action_pane.retain(|(p, _), _| *p != pane);
    }

    /// Clears all state (rules reloaded or triggers disabled).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_is_per_rule_and_pane() {
        let mut l = Limiter::new();
        assert_eq!(l.admit("a", 1, 1000, false, 0), Verdict::Run);
        assert_eq!(l.admit("a", 1, 1000, false, 999), Verdict::Cooldown);
        assert_eq!(l.admit("a", 2, 1000, false, 999), Verdict::Run);
        assert_eq!(l.admit("b", 1, 1000, false, 999), Verdict::Run);
        assert_eq!(l.admit("a", 1, 1000, false, 1000), Verdict::Run);
        assert_eq!(l.suppressed("a"), 0, "cooldown drops are not counted");
    }

    #[test]
    fn peek_records_nothing() {
        let mut l = Limiter::new();
        for _ in 0..10 {
            assert_eq!(l.peek("a", 1, 1000, false, 0), Verdict::Run);
        }
        assert_eq!(l.admit("a", 1, 1000, false, 0), Verdict::Run);
        assert_eq!(l.peek("a", 1, 1000, false, 10), Verdict::Cooldown);
    }

    #[test]
    fn zero_cooldown_still_hits_the_pane_limit() {
        let mut l = Limiter::new();
        for i in 0..PER_PANE_PER_SEC as u64 {
            assert_eq!(l.admit(&format!("r{i}"), 1, 0, false, i), Verdict::Run);
        }
        assert_eq!(l.admit("x", 1, 0, false, 10), Verdict::Suppressed);
        assert_eq!(l.suppressed("x"), 1);
        // The window slides.
        assert_eq!(l.admit("x", 1, 0, false, 1000), Verdict::Run);
    }

    #[test]
    fn global_limit_spans_panes() {
        let mut l = Limiter::new();
        for i in 0..GLOBAL_PER_SEC as u64 {
            assert_eq!(l.admit("r", 100 + i, 0, false, 0), Verdict::Run);
        }
        assert_eq!(l.admit("r", 999, 0, false, 500), Verdict::Suppressed);
        assert_eq!(l.admit("r", 999, 0, false, 1000), Verdict::Run);
    }

    #[test]
    fn a_rule_matching_every_line_is_bounded() {
        let mut l = Limiter::new();
        let mut ran = 0;
        for ms in 0..10_000u64 {
            if l.admit("all", 1, 0, false, ms) == Verdict::Run {
                ran += 1;
            }
        }
        assert!(ran <= 5 * 10 + 5, "ran {ran}");
        assert!(l.suppressed("all") > 9000);
    }

    #[test]
    fn action_limits() {
        let mut l = Limiter::new();
        assert!(l.admit_action("bell", "r", 1, 0));
        assert!(!l.admit_action("bell", "r", 1, 999));
        assert!(l.admit_action("bell", "r", 2, 999));
        assert!(l.admit_action("bell", "r", 1, 1000));
        assert!(l.admit_action("notify", "r", 1, 0) && l.admit_action("notify", "r", 1, 0));
        // clipboard: 1 per 5 s per rule, 1 per 2 s overall.
        assert!(l.admit_action("clipboard", "a", 1, 0));
        assert!(!l.admit_action("clipboard", "b", 1, 1999));
        assert!(l.admit_action("clipboard", "b", 1, 2000));
        assert!(!l.admit_action("clipboard", "a", 1, 4999));
        assert!(l.admit_action("clipboard", "a", 1, 5000));
        // screenshot: 1 per 10 s per rule.
        assert!(l.admit_action("screenshot", "s", 1, 0));
        assert!(!l.admit_action("screenshot", "s", 1, 9999));
        assert!(l.admit_action("screenshot", "s", 1, 10_000));
        assert_eq!(l.suppressed("a"), 1);
        assert_eq!(l.suppressed("b"), 1);
    }

    #[test]
    fn snapshot_global_ten_per_minute() {
        let mut l = Limiter::new();
        let mut ok = 0;
        for i in 0..30u64 {
            if l.admit_action("snapshot", &format!("r{i}"), 1, i * 100) {
                ok += 1;
            }
        }
        assert_eq!(ok, 10);
        assert!(l.admit_action("snapshot", "late", 1, 60_100));
    }

    #[test]
    fn forget_and_reset() {
        let mut l = Limiter::new();
        l.admit("a", 1, 10_000, false, 0);
        l.forget_pane(1);
        assert_eq!(l.admit("a", 1, 10_000, false, 1), Verdict::Run);
        l.reset();
        assert_eq!(l.suppressed("a"), 0);
    }

    #[test]
    fn quiet_rules_skip_the_pane_and_global_budgets_but_not_the_cooldown() {
        let mut l = Limiter::new();
        for i in 0..100 {
            assert_eq!(l.admit("count", 1, 0, true, i), Verdict::Run);
        }
        assert_eq!(l.suppressed("count"), 0);
        // They do not use up the budget of loud rules either.
        for i in 0..PER_PANE_PER_SEC as u64 {
            assert_eq!(l.admit("loud", 1, 0, false, 200 + i), Verdict::Run);
        }
        assert_eq!(l.admit("slow", 1, 500, true, 300), Verdict::Run);
        assert_eq!(l.admit("slow", 1, 500, true, 400), Verdict::Cooldown);
    }

    #[test]
    fn only_decorating_and_counting_rules_are_quiet() {
        use crate::model::{Action, ResetOn, Rule};
        let mut r = Rule::new_text("r", "x");
        assert!(r.is_quiet(), "highlight only");
        r.actions.push(Action::Counter {
            reset_on: ResetOn::Focus,
        });
        assert!(r.is_quiet());
        r.actions.push(Action::Bell {});
        assert!(!r.is_quiet());
        r.actions.clear();
        assert!(!r.is_quiet(), "no actions");
    }
}
