//! `when` predicates for context-menu items (SPEC §10.9.3).
//!
//! `WHEN := CLAUSE (',' CLAUSE)*` (ANDed), `CLAUSE := ['!'] KEY ( ':' VALUE | OP INT )?` with
//! `OP := = != > >= < <=`. `VALUE` is the selector grammar (glob, or `re:` regex). A bare `KEY`
//! means the fact is present and true. A missing fact fails a positive clause and satisfies a
//! negated one, as in selectors. Evaluation is pure: the caller supplies the facts.

use regex::Regex;

use crate::selector::{expand_home, glob_to_regex, Pattern};

/// The shape of a fact's value, which decides the tests it accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    Text,
    Bool,
    Number,
}

macro_rules! when_keys {
    ($($variant:ident => $name:literal, $kind:ident;)*) => {
        /// Every key a `when` clause may use. Which keys a menu provides is in its registry entry.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum WhenKey {
            $($variant,)*
        }

        impl WhenKey {
            pub const ALL: &'static [WhenKey] = &[$(WhenKey::$variant,)*];

            pub fn name(self) -> &'static str {
                match self {
                    $(WhenKey::$variant => $name,)*
                }
            }

            pub fn kind(self) -> FactKind {
                match self {
                    $(WhenKey::$variant => FactKind::$kind,)*
                }
            }

            pub fn parse(s: &str) -> Option<WhenKey> {
                match s {
                    $($name => Some(WhenKey::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

when_keys! {
    // Pane (the §12.3 selector keys).
    Title => "title", Text;
    Cwd => "cwd", Text;
    Cmd => "cmd", Text;
    Domain => "domain", Text;
    Status => "status", Text;
    Exited => "exited", Bool;
    Focused => "focused", Bool;
    // Click (viewport).
    Link => "link", Text;
    Selection => "selection", Text;
    Word => "word", Text;
    Line => "line", Text;
    OnPrompt => "on_prompt", Bool;
    InOutput => "in_output", Bool;
    AltScreen => "alt_screen", Bool;
    MouseReporting => "mouse_reporting", Bool;
    // Mux.
    Panes => "panes", Number;
    Tabs => "tabs", Number;
    Zoomed => "zoomed", Bool;
    Remote => "remote", Bool;
    Local => "local", Bool;
    // Shell (OSC 133).
    LastExit => "last_exit", Number;
    Idle => "idle", Bool;
    Running => "running", Bool;
}

/// A fact's value at evaluation time.
#[derive(Debug, Clone, PartialEq)]
pub enum FactValue {
    Text(String),
    Bool(bool),
    Number(i64),
}

impl FactValue {
    fn truthy(&self) -> bool {
        match self {
            FactValue::Text(s) => !s.is_empty(),
            FactValue::Bool(b) => *b,
            FactValue::Number(n) => *n != 0,
        }
    }

    fn as_text(&self) -> String {
        match self {
            FactValue::Text(s) => s.clone(),
            FactValue::Bool(b) => b.to_string(),
            FactValue::Number(n) => n.to_string(),
        }
    }

    fn as_number(&self) -> Option<i64> {
        match self {
            FactValue::Number(n) => Some(*n),
            FactValue::Bool(b) => Some(i64::from(*b)),
            FactValue::Text(s) => s.trim().parse().ok(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn apply(self, a: i64, b: i64) -> bool {
        match self {
            Op::Eq => a == b,
            Op::Ne => a != b,
            Op::Gt => a > b,
            Op::Ge => a >= b,
            Op::Lt => a < b,
            Op::Le => a <= b,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Test {
    /// Bare key: present and true (non-empty text, `true`, non-zero).
    Present,
    Match(Pattern),
    Compare(Op, i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhenClause {
    pub negate: bool,
    pub key: WhenKey,
    pub test: Test,
}

#[derive(Debug, Clone, PartialEq)]
pub struct When {
    pub clauses: Vec<WhenClause>,
}

/// A parse error; `col` is the 1-based character column in the expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhenError {
    pub col: usize,
    pub message: String,
}

impl std::fmt::Display for WhenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "column {}: {}", self.col, self.message)
    }
}

fn key_list() -> String {
    WhenKey::ALL
        .iter()
        .map(|k| k.name())
        .collect::<Vec<_>>()
        .join(", ")
}

impl When {
    pub fn parse(expr: &str) -> Result<When, WhenError> {
        let err = |byte: usize, message: String| WhenError {
            col: expr[..byte].chars().count() + 1,
            message,
        };
        if expr.trim().is_empty() {
            return Err(err(0, "empty predicate".into()));
        }
        let mut clauses = Vec::new();
        let mut start = 0;
        for part in expr.split(',') {
            let lead = part.len() - part.trim_start().len();
            let at = start + lead;
            start += part.len() + 1;
            let part = part.trim();
            if part.is_empty() {
                return Err(err(at, "empty clause".into()));
            }
            let (negate, rest, at) = match part.strip_prefix('!') {
                Some(r) => (true, r, at + 1),
                None => (false, part, at),
            };
            let key_len = rest
                .find(|c: char| !(c.is_ascii_lowercase() || c == '_'))
                .unwrap_or(rest.len());
            let (name, tail) = rest.split_at(key_len);
            let key = WhenKey::parse(name)
                .ok_or_else(|| err(at, format!("unknown key {name:?} (keys: {})", key_list())))?;
            let tail_at = at + key_len;
            let test = if tail.is_empty() {
                Test::Present
            } else if let Some(value) = tail.strip_prefix(':') {
                Test::Match(Self::pattern(key, value).map_err(|m| err(tail_at + 1, m))?)
            } else {
                let (op, num) = [
                    ("!=", Op::Ne),
                    (">=", Op::Ge),
                    ("<=", Op::Le),
                    ("=", Op::Eq),
                    (">", Op::Gt),
                    ("<", Op::Lt),
                ]
                .iter()
                .find_map(|(sym, op)| tail.strip_prefix(sym).map(|n| (*op, n)))
                .ok_or_else(|| {
                    err(
                        tail_at,
                        format!("expected ':' or a comparison after {name:?}, found {tail:?}"),
                    )
                })?;
                if key.kind() != FactKind::Number {
                    return Err(err(
                        tail_at,
                        format!("{name:?} is not a number; use {name}:VALUE"),
                    ));
                }
                let n = num
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| err(tail_at, format!("{num:?} is not an integer")))?;
                Test::Compare(op, n)
            };
            clauses.push(WhenClause { negate, key, test });
        }
        Ok(When { clauses })
    }

    fn pattern(key: WhenKey, value: &str) -> Result<Pattern, String> {
        if let Some(re) = value.strip_prefix("re:") {
            return Regex::new(re)
                .map(Pattern::Re)
                .map_err(|e| format!("bad regex {re:?}: {e}"));
        }
        let value = if key == WhenKey::Cwd {
            expand_home(value)
        } else {
            value.to_string()
        };
        glob_to_regex(&value)
            .map(Pattern::Glob)
            .map_err(|e| e.message)
    }

    /// The keys this predicate reads, for per-menu validation.
    pub fn keys(&self) -> impl Iterator<Item = WhenKey> + '_ {
        self.clauses.iter().map(|c| c.key)
    }

    /// Evaluates against `fact`, which returns `None` for a fact that is absent.
    pub fn eval(&self, fact: &dyn Fn(WhenKey) -> Option<FactValue>) -> bool {
        self.clauses.iter().all(|c| {
            let hit = fact(c.key).is_some_and(|v| match &c.test {
                Test::Present => v.truthy(),
                Test::Match(p) => p.is_match(&v.as_text()),
                Test::Compare(op, n) => v.as_number().is_some_and(|x| op.apply(x, *n)),
            });
            hit != c.negate
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(pairs: &[(WhenKey, FactValue)]) -> impl Fn(WhenKey) -> Option<FactValue> + '_ {
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.clone())
        }
    }

    fn eval(expr: &str, pairs: &[(WhenKey, FactValue)]) -> bool {
        When::parse(expr).unwrap().eval(&facts(pairs))
    }

    use FactValue::{Bool, Number, Text};
    use WhenKey::*;

    #[test]
    fn bare_keys_test_presence_and_truth() {
        assert!(eval("selection", &[(Selection, Text("x".into()))]));
        assert!(!eval("selection", &[(Selection, Text(String::new()))]));
        assert!(!eval("selection", &[]));
        assert!(eval("!selection", &[]));
        assert!(eval("zoomed", &[(Zoomed, Bool(true))]));
        assert!(!eval("zoomed", &[(Zoomed, Bool(false))]));
        assert!(eval("!zoomed", &[(Zoomed, Bool(false))]));
    }

    #[test]
    fn values_match_globs_and_regexes() {
        let f = [
            (Title, Text("prod-db".into())),
            (Link, Text("https://x.io".into())),
        ];
        assert!(eval("title:*prod*", &f));
        assert!(!eval("title:prod", &f), "globs are anchored");
        assert!(eval("link:re:^https?://", &f));
        assert!(eval("!title:*staging*", &f));
        assert!(
            !eval("word:re:.", &f),
            "a missing fact fails a positive match"
        );
        assert!(eval("!word:re:.", &f), "and satisfies a negated one");
        assert!(eval("focused:true", &[(Focused, Bool(true))]));
    }

    #[test]
    fn numbers_compare() {
        let f = [(Panes, Number(3)), (LastExit, Number(1))];
        for (expr, want) in [
            ("panes>1", true),
            ("panes>=3", true),
            ("panes<3", false),
            ("panes<=3", true),
            ("panes=3", true),
            ("panes!=3", false),
            ("last_exit!=0", true),
            ("!last_exit=0", true),
            ("tabs>0", false),
        ] {
            assert_eq!(eval(expr, &f), want, "{expr}");
        }
    }

    #[test]
    fn clauses_are_anded() {
        let f = [
            (LastExit, Number(2)),
            (Idle, Bool(true)),
            (Remote, Bool(false)),
        ];
        assert!(eval("last_exit!=0, idle", &f));
        assert!(eval("last_exit!=0,idle,!remote", &f));
        assert!(!eval("last_exit!=0, idle, remote", &f));
    }

    #[test]
    fn errors_point_at_the_column() {
        let e = When::parse("selection, nope").unwrap_err();
        assert_eq!(e.col, 12);
        assert!(e.message.contains("unknown key \"nope\""), "{}", e.message);
        let e = When::parse("title>2").unwrap_err();
        assert_eq!(e.col, 6);
        assert!(e.message.contains("not a number"));
        let e = When::parse("panes>x").unwrap_err();
        assert!(e.message.contains("not an integer"));
        let e = When::parse("link:re:(").unwrap_err();
        assert_eq!(e.col, 6);
        assert!(e.message.contains("bad regex"));
        assert!(When::parse("").is_err());
        assert_eq!(When::parse("a,,b").unwrap_err().message, "unknown key \"a\" (keys: title, cwd, cmd, domain, status, exited, focused, link, selection, word, line, on_prompt, in_output, alt_screen, mouse_reporting, panes, tabs, zoomed, remote, local, last_exit, idle, running)");
        assert_eq!(When::parse("idle,,remote").unwrap_err().col, 6);
    }

    #[test]
    fn reports_keys_used() {
        let w = When::parse("panes>1,!remote,link:x").unwrap();
        assert_eq!(w.keys().collect::<Vec<_>>(), vec![Panes, Remote, Link]);
    }
}
