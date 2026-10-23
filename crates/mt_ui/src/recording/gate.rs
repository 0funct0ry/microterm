//! The one place that reads `recording.enabled` (SPEC §10.10.1).
//!
//! The UI, the keymap builder, the palette, IPC dispatch and MCP all ask this helper, so the
//! feature is either fully inert or fully registered.

use gpui::App;
use mt_config::Config;

use crate::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingGate {
    pub enabled: bool,
}

impl RecordingGate {
    pub fn of_config(config: &Config) -> Self {
        Self {
            enabled: config.recording.enabled,
        }
    }

    pub fn of(cx: &App) -> Self {
        Self::of_config(&cx.global::<Settings>().config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_config_flag() {
        let mut c = Config::default();
        assert!(!RecordingGate::of_config(&c).enabled);
        c.recording.enabled = true;
        assert!(RecordingGate::of_config(&c).enabled);
    }

    /// No code outside the gate reads the flag directly (SPEC §10.10.1).
    #[test]
    fn only_the_gate_reads_the_flag() {
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
                        if l.contains("recording.enabled")
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
