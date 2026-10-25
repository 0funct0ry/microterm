//! The trigger engine's logic without any window: compiled rules, the matcher snapshot shared
//! with the worker thread, selector checks against current pane facts, cooldown and rate limits,
//! per-rule statistics and the gate filter (SPEC §10.11.3). The GPUI glue in `runtime` applies
//! the resulting intents.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use mt_mux::ctl::{PaneRow, Row};
use mt_proto::selector::{Key, Selector};
use mt_term::LineQueue;
use mt_triggers::{
    plan, EventKind, FireCtx, Hit, Intent, Limiter, MatchIn, Matcher, Rule, Verdict, When,
};

use super::gate::TriggerGate;

/// What the worker thread scans with: the matcher over the enabled text rules and the id of each
/// rule it was built from (a `Hit` carries an index into that list).
pub struct Snapshot {
    pub matcher: Matcher,
    pub ids: Vec<String>,
}

impl Snapshot {
    fn empty() -> Arc<Snapshot> {
        Arc::new(Snapshot {
            matcher: Matcher::new(&[]),
            ids: Vec::new(),
        })
    }
}

pub type SharedSnapshot = Arc<RwLock<Arc<Snapshot>>>;

/// A text-rule match reported by the worker to the main thread (`TriggerFired` of §10.11.3).
#[derive(Debug, Clone)]
pub struct WorkerHit {
    pub rule: String,
    pub pane: u64,
    pub abs_line: u64,
    pub partial: bool,
    /// The line is the shell prompt or the command being typed (see `CompletedLine::input`).
    pub input: bool,
    pub line: String,
    pub hit: Hit,
}

/// Counters the panel and `ctl trigger list` show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleStats {
    pub fired: u64,
    pub last_fired_ms: Option<u64>,
    /// Why an action failed or was skipped on the latest firing (red dot in the panel).
    pub last_error: Option<String>,
    pub skipped_gate: u64,
}

/// The facts of an event firing.
#[derive(Debug, Clone)]
pub struct EventInput {
    pub kind: EventKind,
    pub pane: u64,
    pub exit: Option<i64>,
}

/// A rule that fired: the intents to apply, already gated and rate-limited.
#[derive(Debug, Clone)]
pub struct Firing {
    pub rule_id: String,
    pub rule_name: String,
    pub pane: u64,
    pub abs_line: Option<u64>,
    pub line: Option<String>,
    pub intents: Vec<Intent>,
}

struct Compiled {
    rule: Rule,
    selector: Option<Selector>,
}

pub struct Engine {
    compiled: Vec<Compiled>,
    stats: HashMap<String, RuleStats>,
    limiter: Limiter,
    paused: bool,
    /// `trigger enable|disable` of this session; never written to `triggers.toml` (§10.11.6).
    session: HashMap<String, bool>,
    snapshot: SharedSnapshot,
    /// Matches the worker could not hand to the main thread because it was behind.
    pub overflow: Arc<AtomicU64>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine {
            compiled: Vec::new(),
            stats: HashMap::new(),
            limiter: Limiter::new(),
            paused: false,
            session: HashMap::new(),
            snapshot: Arc::new(RwLock::new(Snapshot::empty())),
            overflow: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// Selector keys that never change for a pane, so a tap can be decided once at install time.
fn is_static(key: Key) -> bool {
    matches!(
        key,
        Key::Id | Key::Domain | Key::Workspace | Key::Window | Key::Tab
    )
}

fn selector_of(panes: &str) -> Option<Selector> {
    (panes != "*")
        .then(|| Selector::parse(panes).ok())
        .flatten()
}

fn full_match(sel: &Option<Selector>, row: &PaneRow) -> bool {
    sel.as_ref().is_none_or(|s| mt_mux::ctl::matches(s, row))
}

/// Only the clauses about facts that cannot change: "could this rule ever match that pane".
fn static_match(sel: &Option<Selector>, row: &PaneRow) -> bool {
    sel.as_ref().is_none_or(|s| {
        s.clauses
            .iter()
            .filter(|c| is_static(c.key))
            .all(|c| row.field(c.key).is_some_and(|v| c.pattern.is_match(&v)) != c.negate)
    })
}

impl Engine {
    /// Replaces the rule set. Statistics of rules that remain are kept.
    pub fn load(&mut self, rules: Vec<Rule>) {
        let ids: std::collections::HashSet<_> = rules.iter().map(|r| r.id.clone()).collect();
        self.stats.retain(|id, _| ids.contains(id));
        self.session.retain(|id, _| ids.contains(id));
        self.compiled = rules
            .into_iter()
            .map(|rule| Compiled {
                selector: selector_of(&rule.panes),
                rule,
            })
            .collect();
        self.rebuild();
    }

    /// Forgets everything (the feature was turned off).
    pub fn clear(&mut self) {
        self.compiled.clear();
        self.stats.clear();
        self.session.clear();
        self.limiter.reset();
        self.paused = false;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let enabled: Vec<Rule> = self
            .compiled
            .iter()
            .filter(|c| self.is_enabled(&c.rule.id) && c.rule.is_text())
            .map(|c| c.rule.clone())
            .collect();
        let snap = Arc::new(Snapshot {
            ids: enabled.iter().map(|r| r.id.clone()).collect(),
            matcher: Matcher::new(&enabled),
        });
        *self.snapshot.write().unwrap_or_else(|e| e.into_inner()) = snap;
    }

    pub fn snapshot(&self) -> SharedSnapshot {
        self.snapshot.clone()
    }

    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.compiled.iter().map(|c| &c.rule)
    }

    pub fn rule(&self, id: &str) -> Option<&Rule> {
        self.rules().find(|r| r.id == id)
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.session
            .get(id)
            .copied()
            .or_else(|| {
                self.compiled
                    .iter()
                    .find(|c| c.rule.id == id)
                    .map(|c| c.rule.enabled)
            })
            .unwrap_or(false)
    }

    /// `trigger enable|disable` for this session only. `false` when the rule does not exist.
    pub fn set_session_enabled(&mut self, id: &str, on: bool) -> bool {
        if self.rule(id).is_none() {
            return false;
        }
        self.session.insert(id.to_string(), on);
        self.rebuild();
        true
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, on: bool) {
        self.paused = on;
    }

    pub fn stats(&self, id: &str) -> RuleStats {
        self.stats.get(id).cloned().unwrap_or_default()
    }

    pub fn suppressed(&self, id: &str) -> u64 {
        self.limiter.suppressed(id)
    }

    /// Whether some enabled text rule could match `row` (the tap is installed only then).
    pub fn wants_pane(&self, row: &PaneRow) -> bool {
        self.compiled.iter().any(|c| {
            c.rule.is_text() && self.is_enabled(&c.rule.id) && static_match(&c.selector, row)
        })
    }

    fn ctx(rule: &Rule, row: &PaneRow) -> FireCtx {
        FireCtx {
            rule_name: rule.name.clone(),
            pane: row.id.to_string(),
            title: row.title.clone(),
            cwd: row.cwd.clone().unwrap_or_default(),
            domain: row.domain.clone(),
            ..FireCtx::default()
        }
    }

    /// Cooldown, limits, gates and per-action limits for `rule` firing on `row`; the intents
    /// that survive.
    fn admit(
        &mut self,
        idx: usize,
        row: &PaneRow,
        ctx: &FireCtx,
        gate: TriggerGate,
        now: u64,
    ) -> Option<Vec<Intent>> {
        let (id, cooldown, rule) = {
            let c = &self.compiled[idx];
            (c.rule.id.clone(), c.rule.cooldown_ms, c.rule.clone())
        };
        match self
            .limiter
            .admit(&id, row.id, cooldown, rule.is_quiet(), now)
        {
            Verdict::Run => {}
            Verdict::Cooldown | Verdict::Suppressed => return None,
        }
        let mut errors: Vec<String> = Vec::new();
        let mut skipped_gate = 0;
        let mut out = Vec::new();
        for intent in plan(&rule, ctx) {
            if !gate.allows(intent.gate()) {
                skipped_gate += 1;
                errors.push(format!(
                    "{} skipped: {}",
                    intent.kind(),
                    intent.gate().map_or_else(String::new, |g| g.hint())
                ));
                continue;
            }
            let kind: &'static str = match intent.kind() {
                "bell" => "bell",
                "clipboard" => "clipboard",
                "snapshot" => "snapshot",
                "screenshot" => "screenshot",
                _ => "",
            };
            if !kind.is_empty() && !self.limiter.admit_action(kind, &id, row.id, now) {
                errors.push(format!("{kind} skipped: rate limit"));
                continue;
            }
            out.push(intent);
        }
        let st = self.stats.entry(id).or_default();
        st.fired += 1;
        st.last_fired_ms = Some(now);
        st.skipped_gate += skipped_gate;
        st.last_error = errors.into_iter().next();
        Some(out)
    }

    /// Cheap early drop for a worker hit, before the pane's facts are gathered: the rule must
    /// exist, be enabled and not be inside its cooldown or over a rate limit.
    pub fn precheck(&mut self, hit: &WorkerHit, now: u64) -> bool {
        if self.paused {
            return false;
        }
        let Some(c) = self.compiled.iter().find(|c| c.rule.id == hit.rule) else {
            return false;
        };
        if hit.input && c.rule.scope == MatchIn::Output {
            return false;
        }
        let (id, cooldown, quiet) = (c.rule.id.clone(), c.rule.cooldown_ms, c.rule.is_quiet());
        self.is_enabled(&id)
            && self.limiter.peek(&id, hit.pane, cooldown, quiet, now) == Verdict::Run
    }

    /// A text match from the worker: re-checks the rule, the selector against `row`'s current
    /// facts, and applies the limits.
    pub fn accept_text(
        &mut self,
        hit: &WorkerHit,
        row: &PaneRow,
        gate: TriggerGate,
        now: u64,
    ) -> Option<Firing> {
        if self.paused || !gate.enabled {
            return None;
        }
        let idx = self.compiled.iter().position(|c| c.rule.id == hit.rule)?;
        let c = &self.compiled[idx];
        if !c.rule.is_text()
            || (hit.input && c.rule.scope == MatchIn::Output)
            || !self.is_enabled(&hit.rule)
            || !full_match(&c.selector, row)
        {
            return None;
        }
        let mut ctx = Self::ctx(&c.rule, row);
        ctx.line = Some(hit.line.clone());
        ctx.hit = Some(hit.hit.clone());
        let intents = self.admit(idx, row, &ctx, gate, now)?;
        let c = &self.compiled[idx];
        Some(Firing {
            rule_id: c.rule.id.clone(),
            rule_name: c.rule.name.clone(),
            pane: row.id,
            abs_line: Some(hit.abs_line),
            line: Some(hit.line.clone()),
            intents,
        })
    }

    /// A pane event: every enabled event rule of that kind whose exit filter and selector
    /// accept it.
    pub fn accept_event(
        &mut self,
        ev: &EventInput,
        row: &PaneRow,
        gate: TriggerGate,
        now: u64,
    ) -> Vec<Firing> {
        if self.paused || !gate.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for idx in 0..self.compiled.len() {
            let c = &self.compiled[idx];
            let When::Event { event, exit } = &c.rule.when else {
                continue;
            };
            if *event != ev.kind
                || !exit.matches(ev.exit)
                || !self.is_enabled(&c.rule.id)
                || !full_match(&c.selector, row)
            {
                continue;
            }
            let mut ctx = Self::ctx(&c.rule, row);
            ctx.event = Some((ev.kind.name().to_string(), ev.exit));
            if let Some(intents) = self.admit(idx, row, &ctx, gate, now) {
                let c = &self.compiled[idx];
                out.push(Firing {
                    rule_id: c.rule.id.clone(),
                    rule_name: c.rule.name.clone(),
                    pane: row.id,
                    abs_line: None,
                    line: None,
                    intents,
                });
            }
        }
        out
    }

    /// Records why an action of `id` could not run (the red dot of the panel row).
    pub fn note_error(&mut self, id: &str, message: impl Into<String>) {
        self.stats.entry(id.to_string()).or_default().last_error = Some(message.into());
    }

    pub fn forget_pane(&mut self, pane: u64) {
        self.limiter.forget_pane(pane);
    }
}

/// The `mt-triggers` worker: drains the line queue, runs the matcher snapshot (one `RegexSet`
/// pass per line) and reports hits. It is never on the PTY or the main thread (§10.11.3). A full
/// hand-off channel drops the hit and counts it in `overflow`.
pub fn spawn_worker(
    queue: Arc<LineQueue>,
    snapshot: SharedSnapshot,
    tx: async_channel::Sender<WorkerHit>,
    overflow: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("mt-triggers".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let batch = queue.recv_batch(4096, Duration::from_millis(200));
                if batch.is_empty() {
                    continue;
                }
                // Coalesce: bounds this thread's wake-ups, and what waking it costs the PTY
                // threads, to ~1000/s however fast lines arrive (SPEC §10.11.10).
                std::thread::sleep(Duration::from_millis(1));
                let snap = snapshot.read().unwrap_or_else(|e| e.into_inner()).clone();
                if snap.matcher.is_empty() {
                    continue;
                }
                for line in batch {
                    for hit in snap.matcher.scan(&line.text) {
                        let Some(rule) = snap.ids.get(hit.rule) else {
                            continue;
                        };
                        let msg = WorkerHit {
                            rule: rule.clone(),
                            pane: line.pane,
                            abs_line: line.abs_line,
                            partial: line.partial,
                            input: line.input,
                            line: line.text.clone(),
                            hit,
                        };
                        if tx.try_send(msg).is_err() {
                            overflow.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
}

#[cfg(test)]
mod tests;
