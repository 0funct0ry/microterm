//! Splits one line of a snippet body into styled segments: template tags, template errors,
//! the selection and the caret. Pure, so panel rows and the editor share it.

use std::ops::Range;

use mt_snippets::Template;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    /// Inside `{{ … }}`.
    Tag,
    /// Covered by a template diagnostic.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub style: Style,
    pub selected: bool,
    /// The caret sits immediately before this segment.
    pub caret_before: bool,
}

/// Byte ranges of `{{ … }}` tags (an escaped `\{{` is text). An unterminated tag runs to the end
/// of its line.
pub fn tag_ranges(body: &str) -> Vec<Range<usize>> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'\\' && b[i + 1] == b'{' && b.get(i + 2) == Some(&b'{') {
            i += 3;
        } else if b[i] == b'{' && b[i + 1] == b'{' {
            let end = body[i + 2..]
                .find("}}")
                .map(|p| i + 2 + p + 2)
                .filter(|e| !body[i..*e].contains('\n'))
                .unwrap_or_else(|| body[i..].find('\n').map_or(body.len(), |p| i + p));
            out.push(i..end);
            i = end.max(i + 2);
        } else {
            i += 1;
        }
    }
    out
}

/// Byte ranges the template diagnostics point at.
pub fn error_ranges(body: &str) -> Vec<Range<usize>> {
    Template::cached(body)
        .diagnostics()
        .into_iter()
        .map(|d| {
            d.span.start.min(body.len())
                ..d.span.end.min(body.len()).max(d.span.start.min(body.len()))
        })
        .collect()
}

/// The pieces of `line` (a byte range of `body`), in order. A caret at the end of the line
/// yields a trailing empty segment so it can be drawn.
pub fn segments(
    body: &str,
    line: Range<usize>,
    selection: Option<(usize, usize)>,
    caret: Option<usize>,
    tags: &[Range<usize>],
    errors: &[Range<usize>],
) -> Vec<Seg> {
    let mut cuts: Vec<usize> = vec![line.start, line.end];
    let mut push = |p: usize| {
        if p > line.start && p < line.end {
            cuts.push(p);
        }
    };
    for r in tags.iter().chain(errors) {
        push(r.start);
        push(r.end);
    }
    if let Some((a, b)) = selection {
        push(a);
        push(b);
    }
    if let Some(c) = caret {
        push(c);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let covers = |ranges: &[Range<usize>], a: usize, b: usize| {
        ranges.iter().any(|r| r.start <= a && b <= r.end)
    };
    let mut out: Vec<Seg> = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        out.push(Seg {
            text: body[a..b].to_string(),
            style: if covers(errors, a, b) {
                Style::Error
            } else if covers(tags, a, b) {
                Style::Tag
            } else {
                Style::Plain
            },
            selected: selection.is_some_and(|(s, e)| s <= a && b <= e),
            caret_before: caret == Some(a),
        });
    }
    if let Some(c) = caret.filter(|c| *c >= line.start && *c <= line.end) {
        let drawn = out.iter().any(|s| s.caret_before);
        if !drawn {
            out.push(Seg {
                text: String::new(),
                style: Style::Plain,
                selected: false,
                caret_before: true,
            });
        }
        let _ = c;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tags_and_ignores_escapes() {
        let b = "a {{ uuid }} b \\{{ not }} {{ x }}";
        let r = tag_ranges(b);
        assert_eq!(
            r.iter().map(|r| &b[r.clone()]).collect::<Vec<_>>(),
            ["{{ uuid }}", "{{ x }}"]
        );
        assert_eq!(tag_ranges("no tags").len(), 0);
    }

    #[test]
    fn unterminated_tags_end_at_the_line() {
        let b = "x {{ uuid\nnext }}";
        let r = tag_ranges(b);
        assert_eq!(&b[r[0].clone()], "{{ uuid");
    }

    #[test]
    fn segments_cover_the_line_exactly() {
        let body = "echo {{ uuid }} done";
        let tags = tag_ranges(body);
        let segs = segments(body, 0..body.len(), None, None, &tags, &[]);
        let joined: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, body);
        let styles: Vec<_> = segs.iter().map(|s| s.style).collect();
        assert_eq!(styles, [Style::Plain, Style::Tag, Style::Plain]);
    }

    #[test]
    fn selection_caret_and_errors_split_segments() {
        let body = "ab{{ x }}cd";
        let tags = tag_ranges(body);
        #[allow(clippy::single_range_in_vec_init)]
        let errs = [5..6];
        let segs = segments(body, 0..body.len(), Some((1, 4)), Some(4), &tags, &errs);
        let joined: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, body);
        assert!(segs
            .iter()
            .any(|s| s.style == Style::Error && s.text == "x"));
        assert!(
            segs.iter()
                .filter(|s| s.selected)
                .map(|s| s.text.as_str())
                .collect::<String>()
                == "b{{"
        );
        assert_eq!(segs.iter().filter(|s| s.caret_before).count(), 1);
    }

    #[test]
    fn caret_at_the_end_of_a_line_gets_an_empty_segment() {
        let segs = segments("abc", 0..3, None, Some(3), &[], &[]);
        assert_eq!(segs.last().unwrap().text, "");
        assert!(segs.last().unwrap().caret_before);
        // An empty line with the caret.
        let segs = segments("", 0..0, None, Some(0), &[], &[]);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].caret_before);
    }

    #[test]
    fn only_the_given_line_is_used() {
        let body = "one {{ a }}\ntwo";
        let segs = segments(body, 12..15, None, None, &tag_ranges(body), &[]);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "two");
    }

    #[test]
    fn error_ranges_follow_diagnostics() {
        let r = error_ranges("a {{ nope }}");
        assert_eq!(r.len(), 1);
        assert_eq!(&"a {{ nope }}"[r[0].clone()], "nope");
        assert!(error_ranges("a {{ uuid }}").is_empty());
    }
}
