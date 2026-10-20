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
    fn badge_colour_follows_the_level() {
        let by = |l| tab_chrome(None, Some(l)).badge;
        assert_eq!(by(NotifyLevel::Info), Some(StatusKind::Accent));
        assert_eq!(by(NotifyLevel::Attention), Some(StatusKind::Warning));
        assert_eq!(by(NotifyLevel::Error), Some(StatusKind::Error));
    }
}
