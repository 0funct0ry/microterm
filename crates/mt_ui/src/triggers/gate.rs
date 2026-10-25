//! The one place that reads `triggers.enabled` and the two second gates (SPEC §10.11.1).
//!
//! The UI, the keymap builder, the palette, IPC dispatch and the line-tap installer all ask this
//! helper, so the feature is either fully inert or fully registered. The second gates are only
//! ever set in `microterm.toml`; nothing here writes them.

use gpui::App;
use mt_config::Config;
use mt_triggers::Gate;

use crate::settings::Settings;

/// Toast shown by every trigger action while the feature is off.
pub const DISABLED_TOAST: &str = "Triggers are disabled — set `triggers.enabled = true`";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerGate {
    pub enabled: bool,
    pub allow_clipboard: bool,
    pub allow_capture: bool,
}

impl TriggerGate {
    pub fn of_config(config: &Config) -> Self {
        Self {
            enabled: config.triggers.enabled,
            allow_clipboard: config.triggers.allow_clipboard,
            allow_capture: config.triggers.allow_capture,
        }
    }

    pub fn of(cx: &App) -> Self {
        Self::of_config(&cx.global::<Settings>().config)
    }

    /// Whether an action behind `gate` may run: the feature is on and its second gate is open.
    pub fn allows(self, gate: Option<Gate>) -> bool {
        self.enabled
            && match gate {
                None => true,
                Some(Gate::Clipboard) => self.allow_clipboard,
                Some(Gate::Capture) => self.allow_capture,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_matrix() {
        let mut c = Config::default();
        let off = TriggerGate::of_config(&c);
        assert!(!off.enabled);
        for g in [None, Some(Gate::Clipboard), Some(Gate::Capture)] {
            assert!(!off.allows(g), "{g:?} must be inert while off");
        }
        c.triggers.enabled = true;
        let on = TriggerGate::of_config(&c);
        assert!(on.allows(None));
        assert!(!on.allows(Some(Gate::Clipboard)) && !on.allows(Some(Gate::Capture)));
        c.triggers.allow_clipboard = true;
        let g = TriggerGate::of_config(&c);
        assert!(g.allows(Some(Gate::Clipboard)) && !g.allows(Some(Gate::Capture)));
        c.triggers.allow_clipboard = false;
        c.triggers.allow_capture = true;
        let g = TriggerGate::of_config(&c);
        assert!(!g.allows(Some(Gate::Clipboard)) && g.allows(Some(Gate::Capture)));
        // The second gates do nothing while the master switch is off.
        c.triggers.enabled = false;
        assert!(!TriggerGate::of_config(&c).allows(Some(Gate::Capture)));
    }

    /// No code outside the gate reads the flags directly (SPEC §10.11.1).
    #[test]
    fn only_the_gate_reads_the_flags() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, hits);
                } else if p.extension().is_some_and(|x| x == "rs")
                    && p.file_name().is_some_and(|n| n != "gate.rs")
                {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for (i, l) in text.lines().enumerate() {
                        let t = l.trim_start();
                        if [
                            "triggers.enabled",
                            "triggers.allow_clipboard",
                            "triggers.allow_capture",
                        ]
                        .iter()
                        .any(|k| l.contains(k))
                            && !t.starts_with("//")
                            && !l.contains('"')
                        {
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
        assert!(hits.is_empty(), "read the gate instead: {hits:?}");
    }
}
