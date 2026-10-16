//! Find-in-terminal (SPEC §10.6): all matches across scrollback and the screen.

use regex::RegexBuilder;

use crate::textgrid::TextGrid;

/// SPEC §10.6: only the first 10,000 matches are kept.
pub const MAX_MATCHES: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    /// `(absolute line, column, cells)` per row the match covers.
    pub segments: Vec<(usize, usize, usize)>,
}

impl SearchMatch {
    pub fn start(&self) -> (usize, usize) {
        let s = self.segments[0];
        (s.0, s.1)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    /// More than `MAX_MATCHES` exist.
    pub truncated: bool,
    /// The query is not a valid regex.
    pub error: Option<String>,
}

pub fn find_all(grid: &TextGrid, query: &str, case_sensitive: bool, regex: bool) -> SearchResult {
    let mut result = SearchResult::default();
    if query.is_empty() {
        return result;
    }
    let pattern = if regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let re = match RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(re) => re,
        Err(e) => {
            result.error = Some(e.to_string());
            return result;
        }
    };
    for line in grid.logical_lines() {
        let text = line.text();
        // Byte offset of every char, to map regex byte ranges back to char indices.
        let offsets: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let char_of = |byte: usize| offsets.partition_point(|&o| o < byte);
        for m in re.find_iter(&text) {
            if m.is_empty() {
                continue;
            }
            if result.matches.len() == MAX_MATCHES {
                result.truncated = true;
                return result;
            }
            let range = char_of(m.start())..char_of(m.end());
            result.matches.push(SearchMatch {
                segments: line.segments(range),
            });
        }
    }
    result
}

/// Index of the first match starting at or after absolute line `line`, else the last one.
pub fn first_from(matches: &[SearchMatch], line: usize) -> usize {
    matches
        .iter()
        .position(|m| m.start().0 >= line)
        .unwrap_or(matches.len().saturating_sub(1))
}

/// Smart case: case-sensitive only when the query has an uppercase letter.
pub fn smart_case(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> TextGrid {
        TextGrid::from_strs(12, &["Main main", "xmainx", "no", "MAIN"])
    }

    #[test]
    fn plain_search_is_literal_and_case_insensitive_by_default() {
        let r = find_all(&grid(), "main", false, false);
        let starts: Vec<_> = r.matches.iter().map(SearchMatch::start).collect();
        assert_eq!(starts, vec![(0, 0), (0, 5), (1, 1), (3, 0)]);
        assert!(r.error.is_none() && !r.truncated);
    }

    #[test]
    fn case_sensitive_toggle() {
        let r = find_all(&grid(), "main", true, false);
        assert_eq!(r.matches.len(), 2);
        let r = find_all(&grid(), "Main", true, false);
        assert_eq!(r.matches.len(), 1);
    }

    #[test]
    fn plain_mode_escapes_regex_metacharacters() {
        let g = TextGrid::from_strs(10, &["a.c abc"]);
        assert_eq!(find_all(&g, "a.c", false, false).matches.len(), 1);
        assert_eq!(find_all(&g, "a.c", false, true).matches.len(), 2);
    }

    #[test]
    fn regex_mode_and_errors() {
        let g = TextGrid::from_strs(20, &["PAY-12 PAY-345"]);
        let r = find_all(&g, r"PAY-\d+", true, true);
        assert_eq!(r.matches.len(), 2);
        assert_eq!(r.matches[1].segments, vec![(0, 7, 7)]);
        let bad = find_all(&g, "(", false, true);
        assert!(bad.error.is_some() && bad.matches.is_empty());
        // Empty matches are skipped.
        assert!(find_all(&g, "x*", false, true).matches.is_empty());
        assert!(find_all(&g, "", false, false).matches.is_empty());
    }

    #[test]
    fn matches_span_wrapped_rows_and_scrollback_offsets() {
        let mut g = TextGrid::from_strs(6, &["abcdef", "ghijkl"]).wrap(0);
        g.top = 100; // rows are scrollback lines 100 and 101
        g = TextGrid {
            rows: g.rows,
            cols: 6,
            top: 100,
        };
        let r = find_all(&g, "efgh", true, false);
        assert_eq!(r.matches[0].segments, vec![(100, 4, 2), (101, 0, 2)]);
    }

    #[test]
    fn wide_chars_report_cell_widths() {
        let g = TextGrid::from_strs(8, &["a漢\0字\0b"]);
        let r = find_all(&g, "漢字", true, false);
        assert_eq!(r.matches[0].segments, vec![(0, 1, 4)]);
    }

    #[test]
    fn caps_at_ten_thousand() {
        let rows: Vec<String> = (0..10_050).map(|_| "x".to_string()).collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let g = TextGrid::from_strs(2, &refs);
        let r = find_all(&g, "x", true, false);
        assert_eq!(r.matches.len(), MAX_MATCHES);
        assert!(r.truncated);
    }

    #[test]
    fn first_from_and_smart_case() {
        let r = find_all(&grid(), "main", false, false);
        assert_eq!(first_from(&r.matches, 1), 2);
        assert_eq!(first_from(&r.matches, 99), 3);
        assert_eq!(first_from(&[], 0), 0);
        assert!(smart_case("Foo") && !smart_case("foo"));
    }
}
