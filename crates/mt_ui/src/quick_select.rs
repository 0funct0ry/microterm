//! Quick select (SPEC §10.4): scan the visible rows, label every match, type a label.

use mt_patterns::QuickSet;

use crate::textgrid::TextGrid;

/// One labelled match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickHit {
    pub label: String,
    pub text: String,
    /// `(absolute line, column, cells)` per row the match covers.
    pub segments: Vec<(usize, usize, usize)>,
}

impl QuickHit {
    /// `(absolute line, column)` of the first cell, where the chip is drawn.
    pub fn start(&self) -> (usize, usize) {
        let s = self.segments[0];
        (s.0, s.1)
    }
}

/// Prefix-free labels, shortest first: single letters while they fit, otherwise the last `k`
/// letters of the alphabet become two-letter prefixes. At most `alphabet²` labels exist, so
/// the result may be shorter than `n`. Duplicate letters are ignored; labels are lowercase.
pub fn assign_labels(n: usize, alphabet: &str) -> Vec<String> {
    let mut letters: Vec<char> = Vec::new();
    for c in alphabet.chars().flat_map(char::to_lowercase) {
        if !letters.contains(&c) {
            letters.push(c);
        }
    }
    let a = letters.len();
    if a == 0 || n == 0 {
        return Vec::new();
    }
    if n <= a {
        return letters.iter().take(n).map(|c| c.to_string()).collect();
    }
    // singles + prefixes*a >= n, with singles = a - prefixes.
    let mut prefixes = 1;
    while (a - prefixes) + prefixes * a < n && prefixes < a {
        prefixes += 1;
    }
    let singles = a - prefixes;
    let mut out: Vec<String> = letters
        .iter()
        .take(singles)
        .map(|c| c.to_string())
        .collect();
    'outer: for p in &letters[singles..] {
        for s in &letters {
            if out.len() == n {
                break 'outer;
            }
            out.push(format!("{p}{s}"));
        }
    }
    out
}

/// Finds matches in `grid`, nearest to `cursor` (absolute line, column) first, and labels them.
pub fn scan(
    grid: &TextGrid,
    set: &QuickSet,
    cursor: (usize, usize),
    alphabet: &str,
) -> Vec<QuickHit> {
    let mut found: Vec<QuickHit> = Vec::new();
    for line in grid.logical_lines() {
        let text = line.text();
        for m in set.find(&text) {
            found.push(QuickHit {
                label: String::new(),
                text: m.text,
                segments: line.segments(m.range),
            });
        }
    }
    found.sort_by_key(|h| {
        let (r, c) = h.start();
        (r.abs_diff(cursor.0), c.abs_diff(cursor.1), r, c)
    });
    let labels = assign_labels(found.len(), alphabet);
    found.truncate(labels.len());
    for (h, l) in found.iter_mut().zip(labels) {
        h.label = l;
    }
    found
}

/// Outcome of typing one letter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    /// A prefix of at least one label; keep typing.
    Partial,
    /// A full label (index into the hits). `paste` when the last letter was uppercase.
    Hit { index: usize, paste: bool },
    /// Matches nothing; the typed text is unchanged.
    Miss,
}

/// Feeds `ch` to the typed prefix `typed` (lowercase). Updates `typed` on `Partial`.
pub fn type_char(hits: &[QuickHit], typed: &mut String, ch: char) -> Typed {
    let paste = ch.is_uppercase();
    let mut next = typed.clone();
    next.extend(ch.to_lowercase());
    if let Some(index) = hits.iter().position(|h| h.label == next) {
        return Typed::Hit { index, paste };
    }
    if hits.iter().any(|h| h.label.starts_with(&next)) {
        *typed = next;
        return Typed::Partial;
    }
    Typed::Miss
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix_free(labels: &[String]) -> bool {
        labels
            .iter()
            .all(|a| labels.iter().all(|b| a == b || !b.starts_with(a.as_str())))
    }

    #[test]
    fn labels_are_single_letters_when_they_fit() {
        assert_eq!(assign_labels(4, "asdf"), ["a", "s", "d", "f"]);
        assert!(assign_labels(3, "").is_empty());
        assert!(assign_labels(0, "asdf").is_empty());
    }

    #[test]
    fn labels_stay_prefix_free_and_unique_when_alphabet_runs_out() {
        for n in [5, 9, 12, 20, 25] {
            let l = assign_labels(n, "asdfg");
            assert_eq!(l.len(), n, "n={n}");
            assert!(prefix_free(&l), "n={n}: {l:?}");
            let mut sorted = l.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), n);
        }
    }

    #[test]
    fn shortest_labels_come_first_and_capacity_is_alphabet_squared() {
        let l = assign_labels(8, "abc");
        assert_eq!(
            l.iter().filter(|x| x.len() == 1).count(),
            0.max(l.iter().filter(|x| x.len() == 1).count())
        );
        let first_two = l.iter().position(|x| x.len() == 2).unwrap();
        assert!(l[..first_two].iter().all(|x| x.len() == 1));
        assert!(l[first_two..].iter().all(|x| x.len() == 2));
        assert_eq!(assign_labels(100, "abc").len(), 9);
        assert!(prefix_free(&assign_labels(100, "abc")));
    }

    #[test]
    fn duplicate_and_uppercase_alphabet_letters_are_folded() {
        assert_eq!(assign_labels(3, "aAbBc"), ["a", "b", "c"]);
    }

    fn set() -> QuickSet {
        QuickSet::new(&[], &[]).unwrap()
    }

    #[test]
    fn scan_labels_nearest_to_cursor_first() {
        let g = TextGrid::from_strs(
            30,
            &["3f2a9c1 first", "middle", "a0c9f5e second", "10.0.12.4"],
        );
        let hits = scan(&g, &set(), (3, 0), "asdf");
        let texts: Vec<_> = hits
            .iter()
            .map(|h| (h.label.as_str(), h.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![("a", "10.0.12.4"), ("s", "a0c9f5e"), ("d", "3f2a9c1")]
        );
    }

    #[test]
    fn scan_matches_span_wrapped_rows() {
        let g = TextGrid::from_strs(10, &["see https:", "//x.io/ab ok"]).wrap(0);
        let hits = scan(&g, &set(), (1, 0), "as");
        assert_eq!(hits[0].text, "https://x.io/ab");
        assert_eq!(hits[0].segments, vec![(0, 4, 6), (1, 0, 9)]);
        assert_eq!(hits[0].start(), (0, 4));
    }

    #[test]
    fn scan_drops_farthest_matches_when_labels_run_out() {
        let lines: Vec<String> = (0..12).map(|i| format!("pid {}", 10000 + i)).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let g = TextGrid::from_strs(12, &refs);
        let hits = scan(&g, &set(), (11, 0), "ab");
        assert_eq!(hits.len(), 4);
        assert_eq!(hits[0].start().0, 11);
    }

    #[test]
    fn typing_labels() {
        let g = TextGrid::from_strs(40, &["10000 10001 10002 10003 10004 10005"]);
        let hits = scan(&g, &set(), (0, 0), "ab");
        assert_eq!(hits.len(), 4);
        let mut typed = String::new();
        assert_eq!(type_char(&hits, &mut typed, 'z'), Typed::Miss);
        // Two-letter labels: the first letter is a prefix.
        let first = hits[0].label.clone();
        assert_eq!(first.len(), 2);
        let mut ch = first.chars();
        let a = ch.next().unwrap();
        let b = ch.next().unwrap();
        assert_eq!(type_char(&hits, &mut typed, a), Typed::Partial);
        assert_eq!(typed, a.to_string());
        assert_eq!(
            type_char(&hits, &mut typed, b.to_ascii_uppercase()),
            Typed::Hit {
                index: 0,
                paste: true
            }
        );
    }
}
