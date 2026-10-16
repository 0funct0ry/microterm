//! Regex sets shared by hyperlinks (M7) and quick select (M8).
//!
//! A [`LinkSet`] holds the built-in implicit hyperlink rules followed by the user's
//! `[[hyperlink_rules]]`. It is pure text-in/matches-out so it has no UI or config
//! dependency; callers pass plain `(regex, format)` pairs.

use regex::Regex;
use std::collections::HashMap;
use std::ops::Range;

mod quick;
pub use quick::{QuickError, QuickMatch, QuickSet, BUILTIN_NAMES};

/// A hyperlink found in a row of text. `range` is a char-index range into the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkMatch {
    pub range: Range<usize>,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    pub index: usize,
    pub message: String,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hyperlink_rules[{}]: {}", self.index, self.message)
    }
}

impl std::error::Error for PatternError {}

struct Rule {
    re: Regex,
    format: String,
}

/// Built-in rules first, then user rules. Earlier rules win overlaps.
pub struct LinkSet {
    rules: Vec<Rule>,
}

pub(crate) const BUILTIN_URL: &str =
    r#"(?:https?://|ftp://|file://|mailto:)[^\s<>"'`\x00-\x1f]*[^\s<>"'`.,;:!?)\]}\x00-\x1f]"#;
const BUILTIN_LOCALHOST: &str = r"\blocalhost:\d{1,5}\b(?:/[^\s<>]*)?";

impl LinkSet {
    /// Compile built-ins plus `user` rules as `(regex, format)` pairs.
    pub fn new(user: &[(String, String)]) -> Result<Self, PatternError> {
        let mut rules = vec![
            Rule {
                re: Regex::new(BUILTIN_URL).unwrap(),
                format: "$0".into(),
            },
            Rule {
                re: Regex::new(BUILTIN_LOCALHOST).unwrap(),
                format: "http://$0".into(),
            },
        ];
        for (i, (re, format)) in user.iter().enumerate() {
            let re = Regex::new(re).map_err(|e| PatternError {
                index: i,
                message: e.to_string(),
            })?;
            rules.push(Rule {
                re,
                format: format.clone(),
            });
        }
        Ok(Self { rules })
    }

    /// Validate a single user regex (used by config diagnostics).
    pub fn check_regex(re: &str) -> Result<(), String> {
        Regex::new(re).map(|_| ()).map_err(|e| e.to_string())
    }

    /// All non-overlapping links in `text`, sorted by position. Char-indexed.
    pub fn find(&self, text: &str) -> Vec<LinkMatch> {
        let mut found: Vec<LinkMatch> = Vec::new();
        for rule in &self.rules {
            for caps in rule.re.captures_iter(text) {
                let m = caps.get(0).unwrap();
                if m.as_str().is_empty() {
                    continue;
                }
                let start = text[..m.start()].chars().count();
                let end = start + m.as_str().chars().count();
                if found
                    .iter()
                    .any(|f| f.range.start < end && start < f.range.end)
                {
                    continue;
                }
                let mut target = String::new();
                caps.expand(&rule.format, &mut target);
                found.push(LinkMatch {
                    range: start..end,
                    target,
                });
            }
        }
        found.sort_by_key(|f| f.range.start);
        found
    }
}

/// Expand `$0`–`$9` in `format` using `caps` (unmatched groups become empty).
pub fn substitute(format: &str, groups: &[&str]) -> String {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            if let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                chars.next();
                out.push_str(groups.get(d as usize).copied().unwrap_or(""));
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// True when `uri`'s scheme is in the allowlist (case-insensitive). Scheme-less
/// strings are never allowed.
pub fn is_allowed(uri: &str, schemes: &[String]) -> bool {
    match scheme_of(uri) {
        Some(s) => schemes.iter().any(|a| a.eq_ignore_ascii_case(s)),
        None => false,
    }
}

pub fn scheme_of(uri: &str) -> Option<&str> {
    let (s, _) = uri.split_once(':')?;
    let ok = !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    ok.then_some(s)
}

/// Per-row cache keyed by row hash so scrolling does not rescan unchanged rows.
#[derive(Default)]
pub struct LinkCache {
    rows: HashMap<u64, Vec<LinkMatch>>,
}

impl LinkCache {
    pub fn clear(&mut self) {
        self.rows.clear();
    }

    pub fn get_or_scan(&mut self, hash: u64, text: &str, set: &LinkSet) -> &[LinkMatch] {
        if self.rows.len() > 4096 {
            self.rows.clear();
        }
        self.rows.entry(hash).or_insert_with(|| set.find(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(user: &[(&str, &str)]) -> LinkSet {
        let u: Vec<_> = user
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        LinkSet::new(&u).unwrap()
    }

    #[test]
    fn builtin_urls_positive() {
        let s = set(&[]);
        for u in [
            "https://example.com/a?b=1#c",
            "http://x.io",
            "ftp://h/f",
            "file:///tmp/a.txt",
            "mailto:a@b.co",
        ] {
            let m = s.find(&format!("see {u} now"));
            assert_eq!(m.len(), 1, "{u}");
            assert_eq!(m[0].target, u);
            assert_eq!(m[0].range, 4..4 + u.chars().count());
        }
    }

    #[test]
    fn trailing_punctuation_excluded() {
        let m = set(&[]).find("(go to https://a.com/x).");
        assert_eq!(m[0].target, "https://a.com/x");
    }

    #[test]
    fn builtin_negative() {
        assert!(set(&[])
            .find("nothing here, just words: and http alone")
            .is_empty());
    }

    #[test]
    fn localhost_gets_http_scheme() {
        let m = set(&[]).find("listening on localhost:3000/api");
        assert_eq!(m[0].target, "http://localhost:3000/api");
    }

    #[test]
    fn user_rule_substitution() {
        let s = set(&[(
            r"\b([A-Z]{2,})-(\d+)\b",
            "https://j.example/$1/browse/$1-$2",
        )]);
        let m = s.find("fix PAY-12 today");
        assert_eq!(m[0].range, 4..10);
        assert_eq!(m[0].target, "https://j.example/PAY/browse/PAY-12");
    }

    #[test]
    fn user_rule_does_not_overlap_builtin() {
        let s = set(&[(r"PAY-\d+", "x://$0")]);
        let m = s.find("https://a.com/PAY-1");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].target, "https://a.com/PAY-1");
    }

    #[test]
    fn substitute_digits() {
        assert_eq!(substitute("$1-$0-$9-$$", &["all", "one"]), "one-all--$$");
    }

    #[test]
    fn invalid_regex_reports_index() {
        let e = LinkSet::new(&[("ok".into(), "$0".into()), ("(".into(), "$0".into())])
            .err()
            .unwrap();
        assert_eq!(e.index, 1);
    }

    #[test]
    fn allowlist() {
        let a: Vec<String> = ["http", "https", "mailto", "file"]
            .map(String::from)
            .to_vec();
        assert!(is_allowed("https://a", &a));
        assert!(is_allowed("HTTP://a", &a));
        assert!(is_allowed("mailto:x@y", &a));
        assert!(!is_allowed("ftp://a", &a));
        assert!(!is_allowed("javascript:alert(1)", &a));
        assert!(!is_allowed("no-scheme", &a));
        assert!(!is_allowed("//a.com", &a));
    }

    #[test]
    fn char_indices_with_wide_text() {
        let m = set(&[]).find("漢字 https://a.com");
        assert_eq!(m[0].range, 3..16);
    }

    #[test]
    fn cache_reuses_rows() {
        let s = set(&[]);
        let mut c = LinkCache::default();
        assert_eq!(c.get_or_scan(1, "https://a.com", &s).len(), 1);
        assert_eq!(c.get_or_scan(1, "ignored", &s).len(), 1);
    }
}
