//! The line matcher: one `RegexSet` pass over all enabled text rules, then `captures` only for
//! the rules that hit (§10.11.3).

use regex::{Regex, RegexSet, RegexSetBuilder};

use crate::model::{Rule, When};
use crate::rules::compile;

/// One rule's match on a line. Offsets are **character** offsets into the line (so decorations
/// survive reflow), `captures[0]` is the whole match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Index into the rule slice the matcher was built from.
    pub rule: usize,
    pub start: usize,
    pub end: usize,
    pub captures: Vec<Option<String>>,
    pub names: Vec<(String, usize)>,
}

impl Hit {
    pub fn whole(&self) -> &str {
        self.captures[0].as_deref().unwrap_or("")
    }

    pub fn named(&self, name: &str) -> Option<&str> {
        let i = self.names.iter().find(|(n, _)| n == name)?.1;
        self.captures.get(i)?.as_deref()
    }
}

pub struct Matcher {
    set: Option<RegexSet>,
    /// Compiled rules, parallel to the set's patterns: (rule index, regex).
    rules: Vec<(usize, Regex)>,
}

impl Matcher {
    /// Builds a matcher over the enabled text rules of `rules` (disabled and event rules are
    /// skipped; rules whose regex does not compile are skipped too — validation reports them).
    pub fn new(rules: &[Rule]) -> Matcher {
        let mut compiled = Vec::new();
        let mut patterns = Vec::new();
        for (i, r) in rules.iter().enumerate() {
            if !r.enabled {
                continue;
            }
            if let When::Text {
                regex,
                case_insensitive,
            } = &r.when
            {
                if let Ok(re) = compile(regex, *case_insensitive) {
                    patterns.push(if *case_insensitive {
                        format!("(?i:{regex})")
                    } else {
                        format!("(?:{regex})")
                    });
                    compiled.push((i, re));
                }
            }
        }
        // The set holds up to 64 rules at the per-rule limits, so its own limit is raised.
        let set = (!patterns.is_empty())
            .then(|| {
                RegexSetBuilder::new(&patterns)
                    .size_limit(256 << 20)
                    .dfa_size_limit(256 << 20)
                    .build()
                    .ok()
            })
            .flatten();
        Matcher {
            set,
            rules: compiled,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Every rule that matches `line`, in rule order.
    pub fn scan(&self, line: &str) -> Vec<Hit> {
        let Some(set) = &self.set else {
            return Vec::new();
        };
        set.matches(line)
            .into_iter()
            .filter_map(|k| {
                let (rule, re) = &self.rules[k];
                hit_of(*rule, re, line)
            })
            .collect()
    }

    /// Match every rule one by one (reference implementation for tests and the editor preview).
    pub fn scan_each(&self, line: &str) -> Vec<Hit> {
        self.rules
            .iter()
            .filter_map(|(rule, re)| hit_of(*rule, re, line))
            .collect()
    }
}

fn hit_of(rule: usize, re: &Regex, line: &str) -> Option<Hit> {
    let caps = re.captures(line)?;
    let m = caps.get(0)?;
    let char_off = |b: usize| line[..b].chars().count();
    Some(Hit {
        rule,
        start: char_off(m.start()),
        end: char_off(m.end()),
        captures: (0..caps.len())
            .map(|i| caps.get(i).map(|c| c.as_str().to_string()))
            .collect(),
        names: re
            .capture_names()
            .enumerate()
            .filter_map(|(i, n)| n.map(|n| (n.to_string(), i)))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn text(name: &str, re: &str, ci: bool) -> Rule {
        let mut r = Rule::new_text(name, re);
        r.when = When::Text {
            regex: re.to_string(),
            case_insensitive: ci,
        };
        r
    }

    #[test]
    fn captures_and_char_offsets() {
        let rules = [text("a", r"error(\[E\d+\])?: (?P<msg>.+)", false)];
        let m = Matcher::new(&rules);
        let hits = m.scan("ünï error[E0308]: mismatched types");
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!((h.start, h.end), (4, 34));
        assert_eq!(h.named("msg"), Some("mismatched types"));
        assert_eq!(h.captures[1].as_deref(), Some("[E0308]"));
        assert!(m.scan("all fine").is_empty());
    }

    #[test]
    fn disabled_event_and_broken_rules_are_skipped() {
        let mut off = text("off", "x", false);
        off.enabled = false;
        let mut ev = text("ev", "x", false);
        ev.when = When::Event {
            event: crate::EventKind::Bell,
            exit: Default::default(),
        };
        let m = Matcher::new(&[off, ev, text("bad", "(", false)]);
        assert!(m.is_empty() && m.scan("x").is_empty());
    }

    #[test]
    fn case_insensitive_applies_per_rule() {
        let rules = [text("a", "fail", true), text("b", "fail", false)];
        let hits = Matcher::new(&rules).scan("FAIL");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule, 0);
    }

    /// `make bench` (SPEC §10.11.10): the worker sustains at least 200 000 lines/s with 8 rules.
    /// Run with `cargo test --release -p mt_triggers worker_bench -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark: run through `make bench`"]
    fn worker_bench() {
        let res = [
            r"error(\[E\d+\])?: (?P<msg>.+)",
            r"(?i)fail(ed|ure)?",
            r"status=ok",
            r"panicked at",
            r"Continue\? \[y/N\]",
            r"password",
            r"\bINFO\b.*request served in (\d+)ms",
            r"(?i)warning: .+",
        ];
        let rules: Vec<Rule> = res.iter().map(|r| text("b", r, false)).collect();
        let m = Matcher::new(&rules);
        let lines: Vec<String> = (0..400)
            .map(|i| match i % 5 {
                0 => format!("The quick brown fox {i} jumps over the lazy dog"),
                1 => format!(
                    "{i:08} | {:>12.3} | status=ok\tpath=/usr/local/lib/{i}",
                    f64::from(i) * 1.37
                ),
                2 => "INFO  request served in 12ms  id=abc123".to_string(),
                3 => "a".repeat(150),
                _ => "-".repeat(80),
            })
            .collect();
        let total = 2_000_000usize;
        let start = std::time::Instant::now();
        let mut hits = 0usize;
        for i in 0..total {
            hits += m.scan(&lines[i % lines.len()]).len();
        }
        let secs = start.elapsed().as_secs_f64();
        let rate = total as f64 / secs;
        println!("triggers_bench: rules=8 lines={total} seconds={secs:.3} lines_per_s={rate:.0} hits={hits}");
        if !cfg!(debug_assertions) {
            assert!(
                rate >= 200_000.0,
                "{rate:.0} lines/s is under the 200 000 target"
            );
        }
    }

    proptest! {
        #[test]
        fn set_equals_per_rule_matching(line in "[a-zA-Z0-9 :\\[\\]_.-]{0,60}") {
            let rules = [
                text("e", r"error(\[E\d+\])?: (?P<msg>.+)", false),
                text("w", "warn", true),
                text("p", r"Continue\? \[y/N\]", false),
                text("d", r"\d+", false),
                text("all", ".", false),
            ];
            let m = Matcher::new(&rules);
            prop_assert_eq!(m.scan(&line), m.scan_each(&line));
        }
    }
}
