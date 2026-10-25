//! `7720` progress bars as the UI shows them (SPEC §6.4.6, §6.4.7a): the aggregate line of a tab,
//! the tooltip rows and the time format. Pure data, asserted without rendering.

use mt_term::private_osc::Bar;

/// What a tab's bottom-edge line shows for its bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    /// Mean of the determinate bars, rounded; `None` while every bar is indeterminate.
    pub pct: Option<u8>,
}

/// The aggregate of the bars of every pane of a tab: the rounded mean of the determinate bars,
/// indeterminate when there are bars and none is determinate, nothing without bars.
pub fn aggregate(bars: &[Bar]) -> Option<Line> {
    if bars.is_empty() {
        return None;
    }
    let det: Vec<u32> = bars.iter().filter_map(|b| b.pct.map(u32::from)).collect();
    if det.is_empty() {
        return Some(Line { pct: None });
    }
    let sum: u32 = det.iter().sum();
    let n = det.len() as u32;
    Some(Line {
        pct: Some(((sum + n / 2) / n) as u8),
    })
}

/// `45 s`, `1 m 20 s`, `1 h 05 m`.
pub fn fmt_eta(secs: u32) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} m {:02} s", secs / 60, secs % 60)
    } else {
        format!("{} h {:02} m", secs / 3600, (secs % 3600) / 60)
    }
}

/// `Build 42 % · 1 m 20 s left`; `Build …` while indeterminate; `Build done` for a finished bar.
pub fn bar_text(b: &Bar) -> String {
    let mut s = b.label.clone();
    match b.pct {
        _ if b.done => s.push_str(" done"),
        Some(p) => {
            s.push_str(&format!(" {p} %"));
            if let Some(e) = b.eta_s {
                s.push_str(&format!(" · {} left", fmt_eta(e)));
            }
        }
        None => {
            s.push_str(" …");
            if let Some(e) = b.eta_s {
                s.push_str(&format!(" · {} left", fmt_eta(e)));
            }
        }
    }
    s
}

/// One tooltip row per bar.
pub fn tooltip_lines(bars: &[Bar]) -> Vec<String> {
    bars.iter().map(bar_text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_term::private_osc::{parse, PrivCmd, ProgressCmd};

    fn bar(pct: &str, eta: &str) -> Bar {
        let b64 = mt_term::menu_reg::b64_encode(b"Build");
        let p = if eta.is_empty() {
            format!("7720;progress;set;a;{pct};{b64}")
        } else {
            format!("7720;progress;set;a;{pct};{b64};{eta}")
        };
        match parse(p.as_bytes()) {
            Some(PrivCmd::Progress(ProgressCmd::Set { bar, .. })) => bar,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn aggregate_mixes_determinate_and_ignores_indeterminate() {
        assert_eq!(aggregate(&[]), None);
        assert_eq!(
            aggregate(&[bar("20", "1"), bar("-", "1"), bar("41", "1")]),
            Some(Line { pct: Some(31) })
        );
        assert_eq!(
            aggregate(&[bar("-", "1"), bar("-", "2")]),
            Some(Line { pct: None })
        );
    }

    #[test]
    fn eta_format() {
        assert_eq!(fmt_eta(0), "0 s");
        assert_eq!(fmt_eta(45), "45 s");
        assert_eq!(fmt_eta(80), "1 m 20 s");
        assert_eq!(fmt_eta(3600), "1 h 00 m");
        assert_eq!(fmt_eta(3900), "1 h 05 m");
        assert_eq!(fmt_eta(359_999), "99 h 59 m");
    }

    #[test]
    fn tooltip_text() {
        assert_eq!(bar_text(&bar("42", "80")), "Build 42 % · 1 m 20 s left");
        assert_eq!(bar_text(&bar("42", "")), "Build 42 %");
        assert_eq!(bar_text(&bar("-", "5")), "Build … · 5 s left");
    }
}
