//! Pure state behind the Recordings side panel (SPEC §10.10.5): the entries, their grouping,
//! the ranked search, selection and the row text. No GPUI.

use std::time::Duration;

use mt_record::Sidecar;
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

use super::ui::{format_elapsed, format_size};

pub const MIN_WIDTH: f32 = 280.0;
pub const MAX_WIDTH: f32 = 600.0;

pub fn clamp_width(w: f32) -> f32 {
    w.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// One panel row: a saved recording, or one that is still being written.
#[derive(Clone, Debug)]
pub struct Entry {
    pub sc: Sidecar,
    pub running: bool,
    /// The pane being recorded, for the running row's Stop button.
    pub pane: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Now,
    Today,
    Yesterday,
    Earlier,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Self::Now => "Recording now",
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::Earlier => "Earlier",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Header(Group),
    Item(String),
}

/// Day boundaries are UTC, like the store's ids and default titles.
pub fn group_of(e: &Entry, now: u64) -> Group {
    if e.running {
        return Group::Now;
    }
    let (day, today) = (e.sc.created_at / 86_400, now / 86_400);
    if day >= today {
        Group::Today
    } else if day + 1 == today {
        Group::Yesterday
    } else {
        Group::Earlier
    }
}

fn search_fields(e: &Entry) -> [String; 5] {
    [
        e.sc.title.clone(),
        e.sc.origin.name.clone(),
        e.sc.origin.process.clone(),
        e.sc.origin.cwd.clone(),
        mt_record::store::display_time(e.sc.created_at),
    ]
}

/// Indices of the entries matching `query`, best first (ties keep list order).
pub fn rank(entries: &[Entry], query: &str) -> Vec<usize> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(usize, u32)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let best = search_fields(e)
                .iter()
                .zip([4u32, 2, 2, 1, 1])
                .filter(|(t, _)| !t.is_empty())
                .filter_map(|(t, w)| {
                    pattern
                        .score(Utf32Str::new(t, &mut buf), &mut matcher)
                        .map(|s| s * w)
                })
                .max();
            best.map(|s| (i, s))
        })
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| i).collect()
}

/// The list: with an empty query, entries grouped under headers (running first, then newest
/// first); with a query, one flat ranked list.
pub fn rows(entries: &[Entry], query: &str, now: u64) -> Vec<Row> {
    if !query.trim().is_empty() {
        return rank(entries, query)
            .into_iter()
            .map(|i| Row::Item(entries[i].sc.id.clone()))
            .collect();
    }
    let mut order: Vec<&Entry> = entries.iter().collect();
    order.sort_by(|a, b| {
        b.running
            .cmp(&a.running)
            .then(b.sc.created_at.cmp(&a.sc.created_at))
            .then(b.sc.id.cmp(&a.sc.id))
    });
    let mut out = Vec::new();
    let mut last = None;
    for e in order {
        let g = group_of(e, now);
        if last != Some(g) {
            out.push(Row::Header(g));
            last = Some(g);
        }
        out.push(Row::Item(e.sc.id.clone()));
    }
    out
}

pub fn visible_ids(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Item(id) => Some(id.clone()),
            Row::Header(_) => None,
        })
        .collect()
}

/// Moves the selection by `delta` among `ids`, stopping at the ends. With nothing selected,
/// `+1` selects the first and `-1` the last.
pub fn move_selection(selected: &mut Option<String>, ids: &[String], delta: i32) {
    if ids.is_empty() {
        *selected = None;
        return;
    }
    let cur = selected
        .as_ref()
        .and_then(|s| ids.iter().position(|i| i == s));
    let next = match cur {
        None if delta >= 0 => 0,
        None => ids.len() - 1,
        Some(i) => (i as i64 + i64::from(delta)).clamp(0, ids.len() as i64 - 1) as usize,
    };
    *selected = Some(ids[next].clone());
}

/// Keeps the selection valid after the entries or the query changed.
pub fn reconcile(selected: &mut Option<String>, ids: &[String]) {
    if !selected.as_ref().is_some_and(|s| ids.contains(s)) {
        *selected = ids.first().cloned();
    }
}

/// `just now`, `5 min ago`, `3 h ago`, `2 d ago`.
pub fn relative_time(created: u64, now: u64) -> String {
    let d = now.saturating_sub(created);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} d ago", d / 86_400),
    }
}

/// `00:42 · 1.2 MB · 120×34`.
pub fn meta_line(sc: &Sidecar) -> String {
    format!(
        "{} · {} · {}×{}",
        format_elapsed(Duration::from_millis(sc.duration_ms)),
        format_size(sc.size_bytes),
        sc.cols,
        sc.rows
    )
}

/// `input`, `ssh: host`, `serial`, `recovered`, `input not redacted`.
pub fn badges(sc: &Sidecar) -> Vec<String> {
    let mut b = Vec::new();
    if sc.has_input {
        b.push("input".to_string());
        if sc.input_redacted == "none" {
            b.push("input not redacted".to_string());
        }
    }
    match sc.origin.kind.as_str() {
        "ssh" => b.push(format!("ssh: {}", sc.origin.name)),
        "serial" => b.push("serial".to_string()),
        _ => {}
    }
    if sc.recovered {
        b.push("recovered".to_string());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_792_678_503; // 2026-10-22 14:15:03 UTC

    fn entry(id: &str, title: &str, created: u64) -> Entry {
        Entry {
            sc: Sidecar {
                id: id.into(),
                title: title.into(),
                created_at: created,
                ..Sidecar::default()
            },
            running: false,
            pane: None,
        }
    }

    #[test]
    fn groups_running_today_yesterday_earlier() {
        let mut live = entry("live", "Live", NOW);
        live.running = true;
        let es = vec![
            entry("old", "Old", NOW - 5 * 86_400),
            entry("yest", "Yesterday", NOW - 86_400),
            entry("today", "Today", NOW - 60),
            live,
        ];
        let r = rows(&es, "", NOW);
        assert_eq!(
            r,
            vec![
                Row::Header(Group::Now),
                Row::Item("live".into()),
                Row::Header(Group::Today),
                Row::Item("today".into()),
                Row::Header(Group::Yesterday),
                Row::Item("yest".into()),
                Row::Header(Group::Earlier),
                Row::Item("old".into()),
            ]
        );
    }

    #[test]
    fn query_flattens_and_ranks_over_title_origin_and_date() {
        let mut ssh = entry("b", "deploy", NOW);
        ssh.sc.origin.name = "prod-box".into();
        let es = vec![entry("a", "build logs", NOW), ssh];
        assert_eq!(visible_ids(&rows(&es, "prod", NOW)), vec!["b"]);
        assert_eq!(visible_ids(&rows(&es, "build", NOW)), vec!["a"]);
        assert_eq!(visible_ids(&rows(&es, "2026-10-22", NOW)).len(), 2);
        assert!(rows(&es, "zzzz", NOW).is_empty());
        // A query has no headers.
        assert!(rows(&es, "d", NOW)
            .iter()
            .all(|r| matches!(r, Row::Item(_))));
    }

    #[test]
    fn selection_stops_at_the_ends_and_reconciles() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let mut sel = None;
        move_selection(&mut sel, &ids, 1);
        assert_eq!(sel.as_deref(), Some("a"));
        move_selection(&mut sel, &ids, -1);
        assert_eq!(sel.as_deref(), Some("a"));
        move_selection(&mut sel, &ids, 5);
        assert_eq!(sel.as_deref(), Some("c"));
        sel = Some("gone".into());
        reconcile(&mut sel, &ids);
        assert_eq!(sel.as_deref(), Some("a"));
        reconcile(&mut sel, &[]);
        assert_eq!(sel, None);
    }

    #[test]
    fn badges_and_meta() {
        let mut sc = Sidecar {
            duration_ms: 42_000,
            size_bytes: 2048,
            cols: 120,
            rows: 34,
            has_input: true,
            ..Sidecar::default()
        };
        sc.origin.kind = "ssh".into();
        sc.origin.name = "prod".into();
        sc.recovered = true;
        assert_eq!(meta_line(&sc), "00:42 · 2.0 KB · 120×34");
        assert_eq!(
            badges(&sc),
            ["input", "input not redacted", "ssh: prod", "recovered"]
        );
        assert_eq!(relative_time(NOW - 30, NOW), "just now");
        assert_eq!(relative_time(NOW - 300, NOW), "5 min ago");
        assert_eq!(relative_time(NOW - 7200, NOW), "2 h ago");
        assert_eq!(relative_time(NOW - 3 * 86_400, NOW), "3 d ago");
    }

    #[test]
    fn width_is_clamped_to_the_recordings_range() {
        assert_eq!(clamp_width(100.0), 280.0);
        assert_eq!(clamp_width(900.0), 600.0);
    }
}
