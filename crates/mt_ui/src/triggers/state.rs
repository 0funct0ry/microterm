//! What triggers have set on windows and panes: counters, tab colors, titles, statuses and
//! trigger-started recordings, with their expiry. Pure bookkeeping (no GPUI), so every timing
//! rule is tested with an injected clock (SPEC §10.11.4).

use std::collections::HashMap;

use mt_triggers::ResetOn;

/// `12`, or `99+` above 99.
pub fn pill(n: u32) -> String {
    if n > 99 {
        "99+".to_string()
    } else {
        n.to_string()
    }
}

pub const COUNTER_MAX: u32 = 9999;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Count {
    n: u32,
    reset_on: ResetOn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorFx {
    pub pane: u64,
    pub expires_ms: Option<u64>,
    pub clear_on_focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleFx {
    pub pane: u64,
    pub expires_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecFx {
    pub rule: String,
    pub stop_at_ms: Option<u64>,
}

#[derive(Default)]
pub struct Effects {
    counters: HashMap<(String, u64), Count>,
    /// Tab color set by a trigger, by tab.
    pub colors: HashMap<u64, ColorFx>,
    pub titles: HashMap<u64, TitleFx>,
    /// Panes whose status a trigger set, with the instant it clears itself.
    pub statuses: HashMap<u64, Option<u64>>,
    /// Recordings a trigger started, by pane.
    pub recs: HashMap<u64, RecFx>,
}

impl Effects {
    /// Counts one firing of `rule` on `pane` (capped at 9999) and returns the new count.
    pub fn bump(&mut self, rule: &str, pane: u64, reset_on: ResetOn) -> u32 {
        let c = self
            .counters
            .entry((rule.to_string(), pane))
            .or_insert(Count { n: 0, reset_on });
        c.reset_on = reset_on;
        c.n = (c.n + 1).min(COUNTER_MAX);
        c.n
    }

    /// The pane was focused: counters that reset on focus restart.
    pub fn reset_focus(&mut self, pane: u64) {
        self.reset(pane, ResetOn::Focus);
    }

    /// OSC 133 `C`: a new command started in the pane.
    pub fn reset_command_start(&mut self, pane: u64) {
        self.reset(pane, ResetOn::CommandStart);
    }

    fn reset(&mut self, pane: u64, on: ResetOn) {
        self.counters
            .retain(|(_, p), c| !(*p == pane && c.reset_on == on));
    }

    /// Sum over `panes` and the per-rule breakdown (rule id, count) for the tooltip.
    pub fn counts_for(&self, panes: &[u64]) -> (u32, Vec<(String, u32)>) {
        let mut per: Vec<(String, u32)> = Vec::new();
        for ((rule, p), c) in &self.counters {
            if panes.contains(p) {
                match per.iter_mut().find(|(r, _)| r == rule) {
                    Some(e) => e.1 += c.n,
                    None => per.push((rule.clone(), c.n)),
                }
            }
        }
        per.sort();
        (per.iter().map(|(_, n)| n).sum(), per)
    }

    /// Tabs whose trigger color has expired by `now`; they are removed.
    pub fn expired_colors(&mut self, now: u64) -> Vec<u64> {
        take_expired(&mut self.colors, now, |c| c.expires_ms)
    }

    pub fn expired_titles(&mut self, now: u64) -> Vec<u64> {
        take_expired(&mut self.titles, now, |t| t.expires_ms)
    }

    /// Panes whose trigger-set status is due to clear.
    pub fn expired_statuses(&mut self, now: u64) -> Vec<u64> {
        let due: Vec<u64> = self
            .statuses
            .iter()
            .filter(|(_, at)| at.is_some_and(|at| at <= now))
            .map(|(p, _)| *p)
            .collect();
        for p in &due {
            self.statuses.remove(p);
        }
        due
    }

    /// Panes whose `stop_after_s` has elapsed; their entries stay until the stop happens.
    pub fn recs_due(&self, now: u64) -> Vec<u64> {
        self.recs
            .iter()
            .filter(|(_, r)| r.stop_at_ms.is_some_and(|at| at <= now))
            .map(|(p, _)| *p)
            .collect()
    }

    /// Tabs whose color clears when `pane` gets focus.
    pub fn colors_cleared_by_focus(&mut self, pane: u64) -> Vec<u64> {
        let tabs: Vec<u64> = self
            .colors
            .iter()
            .filter(|(_, c)| c.pane == pane && c.clear_on_focus)
            .map(|(t, _)| *t)
            .collect();
        for t in &tabs {
            self.colors.remove(t);
        }
        tabs
    }

    /// `pane` exited: everything a trigger set for it ends. Returns the tabs whose color and
    /// title must be cleared.
    pub fn pane_exited(&mut self, pane: u64) -> (Vec<u64>, Vec<u64>) {
        self.counters.retain(|(_, p), _| *p != pane);
        self.statuses.remove(&pane);
        let colors: Vec<u64> = self
            .colors
            .iter()
            .filter(|(_, c)| c.pane == pane)
            .map(|(t, _)| *t)
            .collect();
        let titles: Vec<u64> = self
            .titles
            .iter()
            .filter(|(_, t)| t.pane == pane)
            .map(|(t, _)| *t)
            .collect();
        for t in &colors {
            self.colors.remove(t);
        }
        for t in &titles {
            self.titles.remove(t);
        }
        (colors, titles)
    }

    pub fn clear(&mut self) {
        *self = Effects::default();
    }
}

fn take_expired<V>(
    map: &mut HashMap<u64, V>,
    now: u64,
    at: impl Fn(&V) -> Option<u64>,
) -> Vec<u64> {
    let due: Vec<u64> = map
        .iter()
        .filter(|(_, v)| at(v).is_some_and(|at| at <= now))
        .map(|(k, _)| *k)
        .collect();
    for k in &due {
        map.remove(k);
    }
    due
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pill_text() {
        assert_eq!(pill(1), "1");
        assert_eq!(pill(99), "99");
        assert_eq!(pill(100), "99+");
        assert_eq!(pill(9999), "99+");
    }

    #[test]
    fn counters_sum_over_a_tabs_panes_and_stop_at_9999() {
        let mut e = Effects::default();
        e.bump("a", 1, ResetOn::Focus);
        e.bump("a", 1, ResetOn::Focus);
        e.bump("a", 2, ResetOn::Focus);
        e.bump("b", 2, ResetOn::Never);
        e.bump("a", 9, ResetOn::Focus);
        let (total, per) = e.counts_for(&[1, 2]);
        assert_eq!(total, 4);
        assert_eq!(per, vec![("a".to_string(), 3), ("b".to_string(), 1)]);
        assert_eq!(e.counts_for(&[7]).0, 0);
        for _ in 0..10_000 {
            e.bump("big", 5, ResetOn::Never);
        }
        assert_eq!(e.counts_for(&[5]).0, COUNTER_MAX);
    }

    #[test]
    fn reset_on_focus_command_start_and_never() {
        let mut e = Effects::default();
        e.bump("f", 1, ResetOn::Focus);
        e.bump("c", 1, ResetOn::CommandStart);
        e.bump("n", 1, ResetOn::Never);
        e.reset_focus(1);
        assert_eq!(
            e.counts_for(&[1])
                .1
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["c", "n"]
        );
        e.reset_command_start(1);
        assert_eq!(e.counts_for(&[1]).1, vec![("n".to_string(), 1)]);
        e.reset_focus(1);
        e.reset_command_start(1);
        assert_eq!(e.counts_for(&[1]).0, 1, "never resets");
        // Another pane's focus does not touch this pane.
        e.bump("f", 2, ResetOn::Focus);
        e.reset_focus(1);
        assert_eq!(e.counts_for(&[2]).0, 1);
    }

    #[test]
    fn colors_expire_and_clear_on_focus() {
        let mut e = Effects::default();
        e.colors.insert(
            10,
            ColorFx {
                pane: 1,
                expires_ms: Some(1000),
                clear_on_focus: false,
            },
        );
        e.colors.insert(
            11,
            ColorFx {
                pane: 2,
                expires_ms: None,
                clear_on_focus: true,
            },
        );
        e.colors.insert(
            12,
            ColorFx {
                pane: 3,
                expires_ms: None,
                clear_on_focus: false,
            },
        );
        assert!(e.expired_colors(999).is_empty());
        assert_eq!(e.expired_colors(1000), vec![10]);
        assert!(e.colors_cleared_by_focus(3).is_empty());
        assert_eq!(e.colors_cleared_by_focus(2), vec![11]);
        assert!(
            e.colors.contains_key(&12),
            "no ttl, no clear_on_focus: stays until the pane exits"
        );
    }

    #[test]
    fn titles_statuses_and_recordings_have_deadlines() {
        let mut e = Effects::default();
        e.titles.insert(
            1,
            TitleFx {
                pane: 1,
                expires_ms: Some(50),
            },
        );
        e.statuses.insert(1, Some(60));
        e.statuses.insert(2, None);
        e.recs.insert(
            1,
            RecFx {
                rule: "r".into(),
                stop_at_ms: Some(70),
            },
        );
        e.recs.insert(
            2,
            RecFx {
                rule: "r".into(),
                stop_at_ms: None,
            },
        );
        assert_eq!(e.expired_titles(50), vec![1]);
        assert_eq!(e.expired_statuses(59), Vec::<u64>::new());
        assert_eq!(e.expired_statuses(60), vec![1]);
        assert!(e.statuses.contains_key(&2));
        assert!(e.recs_due(69).is_empty());
        assert_eq!(e.recs_due(70), vec![1]);
    }

    #[test]
    fn pane_exit_ends_everything_for_that_pane() {
        let mut e = Effects::default();
        e.bump("a", 1, ResetOn::Never);
        e.colors.insert(
            10,
            ColorFx {
                pane: 1,
                expires_ms: None,
                clear_on_focus: false,
            },
        );
        e.titles.insert(
            10,
            TitleFx {
                pane: 1,
                expires_ms: None,
            },
        );
        e.statuses.insert(1, None);
        let (c, t) = e.pane_exited(1);
        assert_eq!((c, t), (vec![10], vec![10]));
        assert!(e.statuses.is_empty() && e.counts_for(&[1]).0 == 0);
    }
}
