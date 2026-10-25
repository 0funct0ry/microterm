//! What a tab shows for `ctl pane status` and `ctl pane notify` (SPEC §4.4, §12.4): a status
//! dot, a 2 px progress line, a tooltip, and an attention badge. Pure data so every state can be
//! asserted without rendering.

use mt_mux::Status;
use mt_proto::{NotifyLevel, StatusState};

use crate::components::StatusKind;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabChrome {
    /// Status dot; `None` for `idle` and for no status.
    pub dot: Option<StatusKind>,
    /// 0–100, drawn along the tab's bottom edge.
    pub progress: Option<u8>,
    /// Status text for the tab tooltip.
    pub tooltip: Option<String>,
    /// Attention badge raised by `pane notify`.
    pub badge: Option<StatusKind>,
    /// A pane of this tab is zoomed (SPEC §17.1).
    pub zoomed: bool,
    /// The tab's resolved color (SPEC §4.4), when it has one.
    pub color: Option<crate::tab_color::TabColorChrome>,
    /// The status dot has the tab's color and gets a 1 px ring in the tab background.
    pub dot_ring: bool,
    /// A pane of this tab is recording: the tooltip line ("Recording pane 12 — 00:42").
    pub recording: Option<String>,
    /// The trigger counter pill after the title (`12`, `99+`), summed over the tab's panes.
    pub counter: Option<String>,
}

pub fn dot_of(state: StatusState) -> Option<StatusKind> {
    match state {
        StatusState::Idle => None,
        StatusState::Working => Some(StatusKind::Accent),
        StatusState::Waiting => Some(StatusKind::Warning),
        StatusState::Done => Some(StatusKind::Success),
        StatusState::Error => Some(StatusKind::Error),
    }
}

pub fn badge_of(level: NotifyLevel) -> StatusKind {
    match level {
        NotifyLevel::Info => StatusKind::Accent,
        NotifyLevel::Attention => StatusKind::Warning,
        NotifyLevel::Error => StatusKind::Error,
    }
}

/// `working 40%: running tests`, `waiting: approve?`, `done`.
pub fn tooltip_of(status: &Status) -> String {
    let mut s = status.state.as_str().to_string();
    if let Some(p) = status.progress {
        s.push_str(&format!(" {p}%"));
    }
    if !status.text.is_empty() {
        s.push_str(": ");
        s.push_str(&status.text);
    }
    s
}

pub fn tab_chrome(status: Option<&Status>, attention: Option<NotifyLevel>) -> TabChrome {
    TabChrome {
        dot: status.and_then(|s| dot_of(s.state)),
        progress: status.and_then(|s| s.progress),
        tooltip: status.map(tooltip_of),
        badge: attention.map(badge_of),
        zoomed: false,
        color: None,
        dot_ring: false,
        recording: None,
        counter: None,
    }
}

impl TabChrome {
    /// Marks the tab as holding a recording pane (SPEC §10.10.2); the tooltip gains a line.
    pub fn with_recording(mut self, pane: u64, elapsed: std::time::Duration) -> Self {
        let line = format!(
            "Recording pane {pane} — {}",
            crate::recording::ui::format_elapsed(elapsed)
        );
        self.tooltip = Some(match self.tooltip.take() {
            Some(t) => format!("{t}\n{line}"),
            None => line.clone(),
        });
        self.recording = Some(line);
        self
    }

    /// Adds the trigger counter pill; the tooltip lists the per-rule counts.
    pub fn with_counter(mut self, total: u32, lines: &[String]) -> Self {
        self.counter = Some(crate::triggers::state::pill(total));
        self.append_tooltip(&lines.join("\n"));
        self
    }

    /// The tab title was set by a trigger (SPEC §10.11.4): a "set by trigger" line.
    pub fn with_trigger_title(mut self) -> Self {
        self.append_tooltip("Title set by a trigger");
        self
    }

    fn append_tooltip(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        self.tooltip = Some(match self.tooltip.take() {
            Some(t) => format!("{t}\n{line}"),
            None => line.to_string(),
        });
    }

    /// Adds the tab color: the tooltip gains the color name and source, and a status dot that
    /// would vanish into the color gets a ring.
    pub fn with_color(
        mut self,
        color: crate::tab_color::TabColorChrome,
        ui: &mt_theme::UiTokens,
    ) -> Self {
        let line = crate::tab_color::tooltip(&color.name, color.source, color.rule.as_deref());
        self.tooltip = Some(match self.tooltip.take() {
            Some(status) => format!("{status}\n{line}"),
            None => line,
        });
        self.dot_ring = [self.dot, self.badge]
            .into_iter()
            .flatten()
            .any(|k| crate::tab_color::dot_needs_ring(ui, k, color.rgba));
        self.color = Some(color);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(state: StatusState, text: &str, progress: Option<u8>) -> Status {
        Status {
            source: mt_mux::StatusSource::Ctl,
            state,
            text: text.into(),
            progress,
            updated_at_ms: 0,
            ttl_deadline_ms: None,
        }
    }

    #[test]
    fn every_state_maps_to_its_dot() {
        let cases = [
            (StatusState::Idle, None),
            (StatusState::Working, Some(StatusKind::Accent)),
            (StatusState::Waiting, Some(StatusKind::Warning)),
            (StatusState::Done, Some(StatusKind::Success)),
            (StatusState::Error, Some(StatusKind::Error)),
        ];
        for (state, dot) in cases {
            let c = tab_chrome(Some(&st(state, "x", None)), None);
            assert_eq!(c.dot, dot, "{state:?}");
            assert_eq!(c.badge, None);
        }
    }

    #[test]
    fn recording_adds_a_dot_and_keeps_the_status_tooltip() {
        let c = tab_chrome(Some(&st(StatusState::Working, "x", None)), None)
            .with_recording(12, std::time::Duration::from_secs(42));
        assert_eq!(c.recording.as_deref(), Some("Recording pane 12 — 00:42"));
        assert_eq!(c.dot, Some(StatusKind::Accent));
        assert_eq!(
            c.tooltip.as_deref(),
            Some("working: x\nRecording pane 12 — 00:42")
        );
    }

    #[test]
    fn nothing_set_means_a_plain_tab() {
        assert_eq!(tab_chrome(None, None), TabChrome::default());
    }

    #[test]
    fn progress_and_tooltip_follow_the_status() {
        let c = tab_chrome(
            Some(&st(StatusState::Working, "running tests", Some(40))),
            None,
        );
        assert_eq!(c.progress, Some(40));
        assert_eq!(c.tooltip.as_deref(), Some("working 40%: running tests"));
        let c = tab_chrome(Some(&st(StatusState::Done, "", None)), None);
        assert_eq!((c.progress, c.tooltip.as_deref()), (None, Some("done")));
        // Text arrives already cut to ctl.status_max_len; the tooltip shows it as is.
        let long = mt_mux::sanitize(&"x".repeat(300), 20);
        let c = tab_chrome(Some(&st(StatusState::Waiting, &long, None)), None);
        assert_eq!(c.tooltip.unwrap().chars().count(), "waiting: ".len() + 20);
    }

    #[test]
    fn color_extends_the_tooltip_and_rings_a_matching_dot() {
        use crate::tab_color::TabColorChrome;
        let ui = crate::theme::Theme::one_dark().tab_ui;
        let color = |name: &str, rgba| TabColorChrome {
            name: name.into(),
            rgba,
            source: mt_mux::ColorSource::Rule,
            rule: Some("domain:prod-*".into()),
            style: mt_config::TabColorStyle::BarTint,
        };
        let status = st(StatusState::Error, "boom", None);
        let c = tab_chrome(Some(&status), None).with_color(color("red", ui.tab_color_red), &ui);
        assert_eq!(
            c.tooltip.as_deref(),
            Some("error: boom\nColor: red (rule: domain:prod-*)")
        );
        assert!(c.dot_ring, "red tab, error dot");
        let c = tab_chrome(Some(&status), None).with_color(color("blue", ui.tab_color_blue), &ui);
        assert!(!c.dot_ring);
        let c = tab_chrome(None, None).with_color(color("red", ui.tab_color_red), &ui);
        assert_eq!(
            c.tooltip.as_deref(),
            Some("Color: red (rule: domain:prod-*)")
        );
    }

    #[test]
    fn badge_colour_follows_the_level() {
        let by = |l| tab_chrome(None, Some(l)).badge;
        assert_eq!(by(NotifyLevel::Info), Some(StatusKind::Accent));
        assert_eq!(by(NotifyLevel::Attention), Some(StatusKind::Warning));
        assert_eq!(by(NotifyLevel::Error), Some(StatusKind::Error));
    }
}
