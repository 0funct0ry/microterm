//! Quick-select pattern set (SPEC §10.4): the built-in rules plus `quick_select.patterns`,
//! minus `quick_select.disable_builtin`. Pure text-in/matches-out like [`crate::LinkSet`].

use std::net::Ipv6Addr;
use std::ops::Range;

use regex::Regex;

/// Names usable in `quick_select.disable_builtin`, in matching-priority order.
pub const BUILTIN_NAMES: &[&str] = &[
    "git_sha",
    "url",
    "path",
    "uuid",
    "ipv4",
    "ipv6",
    "email",
    "hex_color",
    "number",
];

/// A match inside one logical line. `range` is a char-index range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickMatch {
    pub range: Range<usize>,
    pub text: String,
    /// Built-in name, or `"custom"` for a user pattern.
    pub rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickError {
    pub index: usize,
    pub message: String,
}

impl std::fmt::Display for QuickError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "quick_select.patterns[{}]: {}", self.index, self.message)
    }
}

impl std::error::Error for QuickError {}

/// Extra check on a candidate; also sees the text before it.
type Check = fn(before: Option<char>, m: &str) -> bool;

struct Rule {
    name: String,
    re: Regex,
    check: Option<Check>,
}

pub struct QuickSet {
    rules: Vec<Rule>,
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn sha_ok(_: Option<char>, m: &str) -> bool {
    m.chars().any(|c| c.is_ascii_digit()) && m.chars().any(|c| c.is_ascii_alphabetic())
}

fn path_ok(before: Option<char>, _: &str) -> bool {
    !before.is_some_and(|c| word(c) || matches!(c, ':' | '/' | '.' | '~'))
}

fn hex_color_ok(before: Option<char>, _: &str) -> bool {
    !before.is_some_and(word)
}

fn ipv6_ok(before: Option<char>, m: &str) -> bool {
    if before.is_some_and(|c| word(c) || c == ':') {
        return false;
    }
    let inner = if let Some(rest) = m.strip_prefix('[') {
        match rest.split_once(']') {
            Some((a, _)) => a,
            None => return false,
        }
    } else {
        m
    };
    inner.contains(':') && inner.parse::<Ipv6Addr>().is_ok()
}

fn builtin(name: &str) -> Option<(&'static str, Option<Check>)> {
    Some(match name {
        "git_sha" => (r"\b[0-9a-f]{7,40}\b", Some(sha_ok as Check)),
        "url" => (crate::BUILTIN_URL, None),
        "path" => (r"(?:~|\.\.?)?(?:/[\w.+@%~-]+)+/?", Some(path_ok as Check)),
        "uuid" => (
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
            None,
        ),
        "ipv4" => (
            r"\b(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(?::\d{1,5})?\b",
            None,
        ),
        "ipv6" => (
            r"\[[0-9A-Fa-f:.]+\](?::\d{1,5})?|[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}",
            Some(ipv6_ok as Check),
        ),
        "email" => (
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b",
            None,
        ),
        "hex_color" => (
            r"#(?:[0-9A-Fa-f]{8}|[0-9A-Fa-f]{6}|[0-9A-Fa-f]{3})\b",
            Some(hex_color_ok as Check),
        ),
        "number" => (r"\b\d{4,}\b", None),
        _ => return None,
    })
}

impl QuickSet {
    /// `user` are extra regexes (whole match is the copied text); `disabled` names built-ins to
    /// drop. Unknown names in `disabled` are ignored.
    pub fn new(user: &[String], disabled: &[String]) -> Result<Self, QuickError> {
        let mut rules = Vec::new();
        for name in BUILTIN_NAMES {
            if disabled.iter().any(|d| d == name) {
                continue;
            }
            let (re, check) = builtin(name).expect("known builtin");
            rules.push(Rule {
                name: (*name).into(),
                re: Regex::new(re).expect("builtin regex"),
                check,
            });
        }
        for (index, re) in user.iter().enumerate() {
            let re = Regex::new(re).map_err(|e| QuickError {
                index,
                message: e.to_string(),
            })?;
            rules.push(Rule {
                name: "custom".into(),
                re,
                check: None,
            });
        }
        Ok(Self { rules })
    }

    /// Non-overlapping matches sorted by position. Overlaps resolve to the longest match;
    /// on a tie the earlier rule (built-ins in table order, then user patterns) wins.
    pub fn find(&self, text: &str) -> Vec<QuickMatch> {
        // Byte offset -> char index.
        let mut cands: Vec<(usize, usize, usize, &Rule)> = Vec::new(); // (start, end, rule idx) chars
        for (ri, rule) in self.rules.iter().enumerate() {
            let mut pos = 0;
            while pos <= text.len() {
                let Some(m) = rule.re.find_at(text, pos) else {
                    break;
                };
                let first_len = text[m.start()..].chars().next().map_or(1, char::len_utf8);
                if m.as_str().is_empty() {
                    pos = m.start() + first_len;
                    continue;
                }
                let before = text[..m.start()].chars().next_back();
                if rule.check.is_some_and(|c| !c(before, m.as_str())) {
                    pos = m.start() + first_len;
                    continue;
                }
                let start = text[..m.start()].chars().count();
                let end = start + m.as_str().chars().count();
                cands.push((start, end, ri, rule));
                pos = m.end();
            }
        }
        // Longest first, then earlier rule, then leftmost.
        cands.sort_by_key(|&(s, e, ri, _)| (std::cmp::Reverse(e - s), ri, s));
        let mut kept: Vec<(usize, usize, &Rule)> = Vec::new();
        for (s, e, _, rule) in cands {
            if kept.iter().all(|&(ks, ke, _)| e <= ks || ke <= s) {
                kept.push((s, e, rule));
            }
        }
        kept.sort_by_key(|k| k.0);
        let chars: Vec<char> = text.chars().collect();
        kept.into_iter()
            .map(|(s, e, rule)| QuickMatch {
                range: s..e,
                text: chars[s..e].iter().collect(),
                rule: rule.name.clone(),
            })
            .collect()
    }

    pub fn check_regex(re: &str) -> Result<(), String> {
        Regex::new(re).map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> QuickSet {
        QuickSet::new(&[], &[]).unwrap()
    }

    /// `(rule, text)` pairs found in `s`.
    fn found(s: &str) -> Vec<(String, String)> {
        set()
            .find(s)
            .into_iter()
            .map(|m| (m.rule, m.text))
            .collect()
    }

    fn only(s: &str, rule: &str) -> Option<String> {
        let v = found(s);
        v.iter().find(|(r, _)| r == rule).map(|(_, t)| t.clone())
    }

    #[test]
    fn git_sha_positive_and_negative() {
        assert_eq!(
            only("3f2a9c1 Fix bug", "git_sha").as_deref(),
            Some("3f2a9c1")
        );
        let full = "3f2a9c1d8e7b6a5f4c3b2a190817263544332211";
        assert_eq!(only(full, "git_sha").as_deref(), Some(full));
        assert_eq!(
            only("commit abc1234def", "git_sha").as_deref(),
            Some("abc1234def")
        );
        // Too short, non-hex, all digits (that is a number), too long.
        assert!(only("3f2a9c", "git_sha").is_none());
        assert!(only("deadbeeg1", "git_sha").is_none());
        assert!(only("1234567", "git_sha").is_none());
        assert!(only("abcdefg", "git_sha").is_none());
        assert!(only(&format!("{full}ab"), "git_sha").is_none());
    }

    #[test]
    fn url_positive_and_negative() {
        assert_eq!(
            only("see https://example.com/a?b=1.", "url").as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(only("ftp://h/x", "url").as_deref(), Some("ftp://h/x"));
        assert!(only("example.com/a", "url").is_none());
    }

    #[test]
    fn path_positive_and_negative() {
        assert_eq!(
            only("edit ~/src/api/main.rs now", "path").as_deref(),
            Some("~/src/api/main.rs")
        );
        assert_eq!(
            only("./build/out.log", "path").as_deref(),
            Some("./build/out.log")
        );
        assert_eq!(
            only("cat /etc/hosts", "path").as_deref(),
            Some("/etc/hosts")
        );
        assert_eq!(only("../x/y", "path").as_deref(), Some("../x/y"));
        assert!(only("and/or", "path").is_none());
        assert!(only("plain words", "path").is_none());
        // Inside a URL the url rule wins.
        let v = found("https://github.com/a/b");
        assert_eq!(v, vec![("url".into(), "https://github.com/a/b".into())]);
    }

    #[test]
    fn uuid_positive_and_negative() {
        let u = "5f0c1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";
        assert_eq!(found(&format!("id={u}")), vec![("uuid".into(), u.into())]);
        assert!(only("5f0c1a2b-3c4d-4e5f-8a9b", "uuid").is_none());
        assert!(only("zf0c1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b", "uuid").is_none());
    }

    #[test]
    fn ipv4_positive_and_negative() {
        assert_eq!(
            only("pod 10.0.12.4:8080 up", "ipv4").as_deref(),
            Some("10.0.12.4:8080")
        );
        assert_eq!(only("192.168.1.1", "ipv4").as_deref(), Some("192.168.1.1"));
        // The port digits do not also become a number.
        assert_eq!(found("10.0.12.4:8080").len(), 1);
        assert!(only("256.1.1.1", "ipv4").is_none());
        assert!(only("1.2.3", "ipv4").is_none());
    }

    #[test]
    fn ipv6_positive_and_negative() {
        assert_eq!(only("via fe80::1 ok", "ipv6").as_deref(), Some("fe80::1"));
        assert_eq!(only("::1", "ipv6").as_deref(), Some("::1"));
        assert_eq!(
            only("[2001:db8::1]:443", "ipv6").as_deref(),
            Some("[2001:db8::1]:443")
        );
        assert_eq!(
            only("2001:db8:0:0:0:0:0:1", "ipv6").as_deref(),
            Some("2001:db8:0:0:0:0:0:1")
        );
        // Times and MAC addresses are not IPv6.
        assert!(only("at 12:30:45 today", "ipv6").is_none());
        assert!(only("aa:bb:cc:dd:ee:ff", "ipv6").is_none());
    }

    #[test]
    fn email_positive_and_negative() {
        assert_eq!(
            only("mail dev@example.com.", "email").as_deref(),
            Some("dev@example.com")
        );
        assert_eq!(
            only("a.b+c@sub.example.org", "email").as_deref(),
            Some("a.b+c@sub.example.org")
        );
        assert!(only("@handle", "email").is_none());
        assert!(only("a@b", "email").is_none());
    }

    #[test]
    fn hex_color_positive_and_negative() {
        assert_eq!(only("fg #74ade8;", "hex_color").as_deref(), Some("#74ade8"));
        assert_eq!(only("#fff", "hex_color").as_deref(), Some("#fff"));
        assert_eq!(only("#74ade8cc", "hex_color").as_deref(), Some("#74ade8cc"));
        assert!(only("#12", "hex_color").is_none());
        assert!(only("#gggggg", "hex_color").is_none());
        assert!(only("a#fff", "hex_color").is_none());
    }

    #[test]
    fn number_positive_and_negative() {
        assert_eq!(
            only("pid 48213 running", "number").as_deref(),
            Some("48213")
        );
        assert!(only("port 80", "number").is_none());
        assert!(only("v1234x", "number").is_none());
    }

    #[test]
    fn overlaps_resolve_to_longest() {
        // The uuid's first segment is also a 8-hex git sha candidate.
        let u = "5f0c1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";
        assert_eq!(found(u).len(), 1);
        // Email beats the path-looking tail.
        assert_eq!(
            found("dev@example.com"),
            vec![("email".into(), "dev@example.com".into())]
        );
    }

    #[test]
    fn matches_are_sorted_and_char_indexed() {
        let m = set().find("漢字 3f2a9c1 10.0.0.1");
        assert_eq!(m[0].range, 3..10);
        assert_eq!(m[1].range, 11..19);
    }

    #[test]
    fn disable_builtin_and_user_patterns() {
        let s = QuickSet::new(
            &["PAY-\\d+".to_string()],
            &["number".to_string(), "nope".to_string()],
        )
        .unwrap();
        let m = s.find("48213 PAY-12");
        assert_eq!(m.len(), 1);
        assert_eq!(
            (m[0].rule.as_str(), m[0].text.as_str()),
            ("custom", "PAY-12")
        );
        let err = QuickSet::new(&["(".to_string()], &[]).err().unwrap();
        assert_eq!(err.index, 0);
    }

    #[test]
    fn user_pattern_longer_than_builtin_wins() {
        let s = QuickSet::new(&["build-\\d+-x".to_string()], &[]).unwrap();
        let m = s.find("build-1234-x");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].rule, "custom");
    }
}
