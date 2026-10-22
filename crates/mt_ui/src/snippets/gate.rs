//! The one place that reads `snippets.enabled` and `snippets.templates` (SPEC §10.8.1).
//!
//! The UI, the keymap builder, the palette, IPC dispatch and MCP all ask this helper, so the
//! gate matrix (off/off, on/off, on/on) is decided once.

use gpui::App;
use mt_config::Config;

use crate::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnippetsGate {
    pub enabled: bool,
    pub templates: bool,
}

impl SnippetsGate {
    pub fn of_config(config: &Config) -> Self {
        Self {
            enabled: config.snippets.enabled,
            templates: config.snippets.enabled && config.snippets.templates,
        }
    }

    pub fn of(cx: &App) -> Self {
        Self::of_config(&cx.global::<Settings>().config)
    }

    /// Template expansion is on: `enabled && templates` (`templates` alone has no effect).
    pub fn templates_on(self) -> bool {
        self.enabled && self.templates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool, templates: bool) -> Config {
        let mut c = Config::default();
        c.snippets.enabled = enabled;
        c.snippets.templates = templates;
        c
    }

    #[test]
    fn gate_matrix() {
        let off = SnippetsGate::of_config(&cfg(false, false));
        assert!(!off.enabled && !off.templates_on());
        // `templates` without `enabled` has no effect.
        let t_only = SnippetsGate::of_config(&cfg(false, true));
        assert!(!t_only.enabled && !t_only.templates_on());
        let literal = SnippetsGate::of_config(&cfg(true, false));
        assert!(literal.enabled && !literal.templates_on());
        let full = SnippetsGate::of_config(&cfg(true, true));
        assert!(full.enabled && full.templates_on());
    }

    /// No code outside the gate reads the two flags directly (SPEC §10.8.1).
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
                        if l.contains("snippets.enabled") || l.contains("snippets.templates") {
                            let t = l.trim_start();
                            if !t.starts_with("//") && !t.starts_with('"') && !l.contains("\"") {
                                hits.push(format!("{}:{}", p.display(), i + 1));
                            }
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
