//! Pane status, attention badge and notification rate limiting (SPEC §12.4, M10.3).
//!
//! Pure logic over an injected clock (`now_ms`), like [`crate::wait`], so it is unit-testable
//! without timers.

use std::collections::HashMap;

use mt_proto::{iso8601_utc, PaneStatus, StatusState};

use crate::model::Id;

/// A pane's status. Integers only so `Pane` stays `Eq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: StatusState,
    pub text: String,
    pub progress: Option<u8>,
    pub updated_at_ms: u64,
    /// The status clears itself at this time.
    pub ttl_deadline_ms: Option<u64>,
}

impl Status {
    pub fn to_wire(&self) -> PaneStatus {
        PaneStatus {
            state: self.state,
            text: self.text.clone(),
            progress: self.progress,
            updated_at: iso8601_utc(self.updated_at_ms),
        }
    }
}

/// Strips control characters (newlines and tabs become spaces) and truncates to `max_len`
/// characters, ending a cut text with `…`. Status and notification text is attacker-influenced
/// (SPEC §12.6 S5): it must not smuggle escape sequences into app chrome.
pub fn sanitize(text: &str, max_len: usize) -> String {
    let mut out: String = text
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() => None,
            // Bidirectional overrides can reorder the text the user reads.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => None,
            c => Some(c),
        })
        .collect();
    let out_trimmed = out.trim();
    if out_trimmed.len() != out.len() {
        out = out_trimmed.to_string();
    }
    if out.chars().count() > max_len {
        let keep = max_len.saturating_sub(1);
        out = out.chars().take(keep).collect();
        out.truncate(out.trim_end().len());
        if max_len > 0 {
            out.push('…');
        }
    }
    out
}

/// Notifications allowed per pane per window.
pub const NOTIFY_BURST: u32 = 5;
const NOTIFY_WINDOW_MS: u64 = 1000;

/// Per-pane notification rate limit: 5 per second, extras dropped and counted.
#[derive(Debug, Default)]
pub struct NotifyLimiter {
    panes: HashMap<Id, Slot>,
}

#[derive(Debug, Default)]
struct Slot {
    window_start: u64,
    count: u32,
    dropped: u32,
}

impl NotifyLimiter {
    /// `true` when a notification for `pane` may go out now; otherwise it is counted as dropped.
    pub fn allow(&mut self, pane: Id, now_ms: u64) -> bool {
        let s = self.panes.entry(pane).or_insert_with(|| Slot {
            window_start: now_ms,
            ..Slot::default()
        });
        if now_ms.saturating_sub(s.window_start) >= NOTIFY_WINDOW_MS {
            s.window_start = now_ms;
            s.count = 0;
        }
        if s.count < NOTIFY_BURST {
            s.count += 1;
            true
        } else {
            s.dropped += 1;
            false
        }
    }

    /// Notifications dropped for `pane` since it was created.
    pub fn dropped(&self, pane: Id) -> u32 {
        self.panes.get(&pane).map_or(0, |s| s.dropped)
    }

    pub fn forget(&mut self, pane: Id) {
        self.panes.remove(&pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_controls_and_truncates() {
        assert_eq!(sanitize("a\x1b[31mb\x07c", 50), "a[31mbc");
        assert_eq!(sanitize("line1\nline2\tx", 50), "line1 line2 x");
        assert_eq!(sanitize("  hi  ", 50), "hi");
        assert_eq!(sanitize("abcdef", 4), "abc…");
        assert_eq!(sanitize("abcd", 4), "abcd");
        assert_eq!(sanitize("héllo wörld", 5), "héll…");
        assert_eq!(sanitize("a\u{202e}b", 10), "ab");
        assert_eq!(sanitize("abc", 0), "");
        assert_eq!(sanitize("ab cd", 4), "ab…", "no space before the ellipsis");
    }

    #[test]
    fn limiter_allows_five_per_second_per_pane_and_counts_drops() {
        let mut l = NotifyLimiter::default();
        for _ in 0..5 {
            assert!(l.allow(1, 100));
        }
        assert!(!l.allow(1, 500));
        assert!(!l.allow(1, 999));
        assert_eq!(l.dropped(1), 2);
        assert!(l.allow(2, 500), "other panes have their own budget");
        assert!(l.allow(1, 1100), "a new window refills the budget");
        assert_eq!(l.dropped(1), 2, "the counter is cumulative");
        l.forget(1);
        assert_eq!(l.dropped(1), 0);
    }
}
