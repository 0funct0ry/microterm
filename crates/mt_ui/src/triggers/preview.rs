//! The editor's live regex preview and the **Test** dry run read the last lines of the focused
//! pane (SPEC §10.11.5). Pure over rows read from the terminal.

use mt_term::LineCells;
use mt_triggers::{Matcher, Rule, When};

/// How many logical lines the preview and **Test** look at.
pub const LAST_LINES: usize = 200;

/// Logical lines of `rows` (soft-wrapped rows joined, trailing blanks trimmed) with the stable
/// id of each line's first row: `line_base + first_abs + index`.
pub fn logical_lines(rows: &[LineCells], first_abs: usize, line_base: u64) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    for (i, r) in rows.iter().enumerate() {
        if cur.is_empty() {
            start = i;
        }
        cur.extend(r.chars.iter().filter(|c| **c != '\0'));
        if !r.wrapped {
            let text = cur.trim_end().to_string();
            cur.clear();
            if !text.is_empty() {
                out.push((line_base + (first_abs + start) as u64, text));
            }
        }
    }
    out
}

/// What a regex finds in `lines`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preview {
    /// Lines with a match.
    pub count: usize,
    /// `(line id, start char, end char)` of the first match on each such line.
    pub hits: Vec<(u64, usize, usize)>,
}

impl Preview {
    /// `4 matches in the last 200 lines`.
    pub fn summary(&self, looked_at: usize) -> String {
        let n = self.count;
        format!(
            "{n} match{} in the last {looked_at} line{}",
            if n == 1 { "" } else { "es" },
            if looked_at == 1 { "" } else { "s" }
        )
    }
}

pub fn scan(lines: &[(u64, String)], regex: &str, case_insensitive: bool) -> Option<Preview> {
    if regex.is_empty() {
        return Some(Preview::default());
    }
    let mut rule = Rule::new_text("preview", regex);
    rule.when = When::Text {
        regex: regex.to_string(),
        case_insensitive,
    };
    mt_triggers::rules::compile(regex, case_insensitive).ok()?;
    let m = Matcher::new(std::slice::from_ref(&rule));
    let mut p = Preview::default();
    for (id, text) in lines {
        if let Some(h) = m.scan(text).first() {
            p.count += 1;
            p.hits.push((*id, h.start, h.end));
        }
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(s: &str, wrapped: bool) -> LineCells {
        LineCells {
            chars: s.chars().collect(),
            wrapped,
        }
    }

    #[test]
    fn wrapped_rows_join_and_ids_follow_the_first_row() {
        let rows = [
            row("abc ", true),
            row("def   ", false),
            row("   ", false),
            row("ghi ", false),
        ];
        let lines = logical_lines(&rows, 10, 1000);
        assert_eq!(
            lines,
            vec![(1010, "abc def".to_string()), (1013, "ghi".to_string())]
        );
    }

    #[test]
    fn wide_spacers_are_skipped() {
        let rows = [row("a\u{6f22}\0b", false)];
        assert_eq!(logical_lines(&rows, 0, 0)[0].1, "a\u{6f22}b");
    }

    #[test]
    fn scan_counts_lines_and_reports_the_first_match_span() {
        let lines: Vec<(u64, String)> = vec![
            (1, "ok".into()),
            (2, "error: one and error: two".into()),
            (3, "ERROR: three".into()),
        ];
        let p = scan(&lines, "error: ", false).unwrap();
        assert_eq!((p.count, p.hits.clone()), (1, vec![(2, 0, 7)]));
        let p = scan(&lines, "error: ", true).unwrap();
        assert_eq!(p.count, 2);
        assert_eq!(p.summary(200), "2 matches in the last 200 lines");
        assert_eq!(
            Preview {
                count: 1,
                hits: vec![]
            }
            .summary(1),
            "1 match in the last 1 line"
        );
        assert!(
            scan(&lines, "(", false).is_none(),
            "a broken regex has no preview"
        );
        assert_eq!(scan(&lines, "", false).unwrap().count, 0);
    }
}
