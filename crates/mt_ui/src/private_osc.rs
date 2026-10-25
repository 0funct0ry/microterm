//! The private OSC master switch on the UI side (SPEC §6.4.2).
//!
//! `PrivateOscGate` is the only type that reads `[private_osc]` and the session override; the
//! terminals get the result as a `mt_term::private_osc::PrivateGate`. The override is
//! session-only and works in both directions; nothing a program can send touches it, and
//! there is deliberately no way to turn the switch on from `ctl`, IPC or MCP.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::App;
use mt_config::Config;
use mt_term::private_osc::{Feature, Features, PrivateGate};

use crate::settings::Settings;

/// 0 = follow the config, 1 = forced off, 2 = forced on.
static OVERRIDE: AtomicU8 = AtomicU8::new(0);

/// The session override of the config's master switch.
pub fn session_override() -> Option<bool> {
    match OVERRIDE.load(Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn set_override(v: Option<bool>) {
    OVERRIDE.store(
        match v {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        },
        Ordering::Relaxed,
    );
}

/// What the UI needs to know about the two layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivateOscGate {
    /// The effective master switch (config, unless the session override holds it).
    pub enabled: bool,
    /// The session override is engaged (the status bar shows a lock icon while it holds the
    /// switch off).
    pub overridden: bool,
}

impl PrivateOscGate {
    pub fn of_config(config: &Config) -> Self {
        let over = session_override();
        Self {
            enabled: over.unwrap_or(config.private_osc.enabled),
            overridden: over.is_some(),
        }
    }

    pub fn of(cx: &App) -> Self {
        Self::of_config(&cx.global::<Settings>().config)
    }

    /// The lock icon: the session override holds the switch off.
    pub fn locked(self) -> bool {
        self.overridden && !self.enabled
    }
}

/// The gate the terminals evaluate, from the config and the session override.
pub fn gate_of_config(config: &Config) -> PrivateGate {
    let c = &config.private_osc;
    let mut features = Features::default();
    for (f, on) in [
        (Feature::Menu, c.features.menu),
        (Feature::Caps, c.features.caps),
        (Feature::Status, c.features.status),
        (Feature::Progress, c.features.progress),
        (Feature::Tab, c.features.tab),
        (Feature::Badge, c.features.badge),
        (Feature::Mark, c.features.mark),
        (Feature::Key, c.features.key),
        (Feature::Palette, c.features.palette),
        (Feature::Fold, c.features.fold),
        (Feature::Link, c.features.link),
    ] {
        features.set(f, on);
    }
    PrivateGate {
        enabled: PrivateOscGate::of_config(config).enabled,
        features,
        rate: c.rate_per_second.clamp(2, 100),
        replies: true,
    }
}

/// `TogglePrivateOsc`: flips the effective switch for this session. Returns the new value.
pub fn toggle(config: &Config) -> bool {
    let now = !PrivateOscGate::of_config(config).enabled;
    OVERRIDE.store(if now { 2 } else { 1 }, Ordering::Relaxed);
    now
}

/// `ctl private-osc disable`: forces the switch off for this session. There is no counterpart.
pub fn disable() {
    OVERRIDE.store(1, Ordering::Relaxed);
}

/// `private_osc.status`: the layers, the per-feature switches and the per-pane counters. Carries
/// no content of any segment, item or reply.
pub fn status_json(
    config: &Config,
    panes: &std::collections::HashMap<u64, std::sync::Arc<mt_term::Terminal>>,
) -> serde_json::Value {
    let g = PrivateOscGate::of_config(config);
    let gate = gate_of_config(config);
    let mut features = serde_json::Map::new();
    for f in Feature::ALL {
        features.insert(
            f.name().into(),
            serde_json::json!(config_feature(config, f)),
        );
    }
    let mut ids: Vec<_> = panes.keys().copied().collect();
    ids.sort_unstable();
    let rows: Vec<_> = ids
        .into_iter()
        .map(|id| {
            let t = &panes[&id];
            let c = t.private_counters();
            serde_json::json!({
                "pane": id,
                "applied": c.applied,
                "malformed": c.malformed,
                "dropped": c.dropped,
                "swallowed_off": c.swallowed_off,
                "swallowed_disabled": c.swallowed_disabled,
                "segments": t.status_segments().len(),
            })
        })
        .collect();
    serde_json::json!({
        "enabled": g.enabled,
        "config_enabled": config.private_osc.enabled,
        "session_override": g.overridden,
        "rate_per_second": gate.rate,
        "features": features,
        "panes": rows,
    })
}

fn config_feature(config: &Config, f: Feature) -> bool {
    let c = &config.private_osc.features;
    match f {
        Feature::Menu => c.menu,
        Feature::Caps => c.caps,
        Feature::Status => c.status,
        Feature::Progress => c.progress,
        Feature::Tab => c.tab,
        Feature::Badge => c.badge,
        Feature::Mark => c.mark,
        Feature::Key => c.key,
        Feature::Palette => c.palette,
        Feature::Fold => c.fold,
        Feature::Link => c.link,
    }
}

/// Pushes the current gate into every pane of every window; what it no longer allows is
/// cleared and not restored.
pub fn push_gate(cx: &mut App) {
    for handle in cx.windows() {
        if let Some(h) = handle.downcast::<crate::app::AppWindow>() {
            let _ = h.update(cx, |this, _, cx| {
                this.push_private_gate(cx);
                cx.notify();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The override is process-global: tests that touch it run one at a time.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn override_wins_in_both_directions() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut c = Config::default();
        set_override(None);
        assert!(!PrivateOscGate::of_config(&c).enabled);
        assert!(!gate_of_config(&c).enabled);
        // On by toggling over an off config.
        assert!(toggle(&c));
        assert!(gate_of_config(&c).enabled && !PrivateOscGate::of_config(&c).locked());
        // Off by toggling again: the lock shows.
        assert!(!toggle(&c));
        assert!(PrivateOscGate::of_config(&c).locked());
        // The override also holds against an enabled config, and survives a reload.
        c.private_osc.enabled = true;
        assert!(!gate_of_config(&c).enabled);
        set_override(None);
        assert!(gate_of_config(&c).enabled);
        disable();
        assert!(!gate_of_config(&c).enabled);
        set_override(None);
    }

    #[test]
    fn features_and_rate_follow_the_config() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_override(None);
        let mut c = Config::default();
        c.private_osc.enabled = true;
        c.private_osc.features.status = false;
        c.private_osc.rate_per_second = 7;
        let g = gate_of_config(&c);
        assert!(g.allows(Feature::Caps) && !g.allows(Feature::Status));
        assert_eq!(g.rate, 7);
    }

    /// No code outside this module reads `private_osc.enabled` (SPEC §6.4.2).
    #[test]
    fn only_the_gate_reads_the_switch() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, hits);
                } else if p.extension().is_some_and(|x| x == "rs")
                    && p.file_name().is_some_and(|n| n != "private_osc.rs")
                {
                    for (i, l) in std::fs::read_to_string(&p).unwrap().lines().enumerate() {
                        if !l.trim_start().starts_with("//") && l.contains("private_osc.enabled") {
                            hits.push(format!("{}:{}", p.display(), i + 1));
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut hits,
        );
        assert!(hits.is_empty(), "{hits:?}");
    }
}
