//! Selector expressions for `--match` (SPEC §12.3).
//!
//! `EXPR := CLAUSE (',' CLAUSE)*`, `CLAUSE := ['!'] KEY ':' VALUE`; clauses are ANDed. `VALUE` is a
//! glob (`*`, `?`) unless prefixed with `re:` (unanchored Rust regex). Parsing lives here so every
//! client agrees on the grammar; evaluation lives in `mt_mux` against plain snapshot structs.
//!
//! Commas separate clauses, so a regex that needs a literal comma must write it as `\x2c`.

use regex::Regex;

use crate::RpcError;

/// What a selector (or target) addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Workspace,
    Window,
    Tab,
    Pane,
}

impl Resource {
    pub fn name(self) -> &'static str {
        match self {
            Resource::Workspace => "workspace",
            Resource::Window => "window",
            Resource::Tab => "tab",
            Resource::Pane => "pane",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Id,
    Title,
    Cwd,
    Cmd,
    Domain,
    Workspace,
    Window,
    Tab,
    Focused,
    Active,
    Status,
    Exited,
}

impl Key {
    fn parse(s: &str) -> Option<Key> {
        Some(match s {
            "id" => Key::Id,
            "title" => Key::Title,
            "cwd" => Key::Cwd,
            "cmd" => Key::Cmd,
            "domain" => Key::Domain,
            "workspace" => Key::Workspace,
            "window" => Key::Window,
            "tab" => Key::Tab,
            "focused" => Key::Focused,
            "active" => Key::Active,
            "status" => Key::Status,
            "exited" => Key::Exited,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Key::Id => "id",
            Key::Title => "title",
            Key::Cwd => "cwd",
            Key::Cmd => "cmd",
            Key::Domain => "domain",
            Key::Workspace => "workspace",
            Key::Window => "window",
            Key::Tab => "tab",
            Key::Focused => "focused",
            Key::Active => "active",
            Key::Status => "status",
            Key::Exited => "exited",
        }
    }

    /// The §12.3 "Applies to" column.
    pub fn applies_to(self, r: Resource) -> bool {
        use Resource::*;
        match self {
            Key::Id => true,
            Key::Title => matches!(r, Pane | Tab | Window),
            Key::Cwd | Key::Cmd | Key::Status | Key::Exited => r == Pane,
            Key::Domain => matches!(r, Pane | Tab),
            Key::Workspace => r != Workspace,
            Key::Window | Key::Tab => match self {
                Key::Window => matches!(r, Tab | Pane),
                _ => r == Pane,
            },
            Key::Focused | Key::Active => matches!(r, Pane | Tab | Window),
        }
    }
}

/// A compiled value pattern.
#[derive(Debug, Clone)]
pub enum Pattern {
    Glob(Regex),
    Re(Regex),
}

impl Pattern {
    pub fn is_match(&self, text: &str) -> bool {
        match self {
            Pattern::Glob(r) | Pattern::Re(r) => r.is_match(text),
        }
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Pattern::Glob(a), Pattern::Glob(b)) | (Pattern::Re(a), Pattern::Re(b)) => {
                a.as_str() == b.as_str()
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    pub negate: bool,
    pub key: Key,
    /// The value as written (after `re:` was stripped), used for numeric/boolean keys.
    pub raw: String,
    pub pattern: Pattern,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    pub clauses: Vec<Clause>,
}

fn glob_to_regex(glob: &str) -> Result<Regex, RpcError> {
    let mut re = String::from("^");
    for c in glob.chars() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Regex::new(&re).map_err(|e| RpcError::bad_request(format!("bad glob {glob:?}: {e}")))
}

/// `~` and `~/…` become `$HOME` (SPEC §12.3: `cwd` is matched with `~` expanded).
fn expand_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix('~') {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("{home}{rest}")
        }
        _ => path.to_string(),
    }
}

impl Selector {
    /// Parses `expr`. Errors are usage errors (exit 1).
    pub fn parse(expr: &str) -> Result<Selector, RpcError> {
        if expr.trim().is_empty() {
            return Err(RpcError::bad_request("empty selector"));
        }
        let mut clauses = Vec::new();
        for part in expr.split(',') {
            let part = part.trim();
            let (negate, rest) = match part.strip_prefix('!') {
                Some(r) => (true, r),
                None => (false, part),
            };
            let (key, value) = rest.split_once(':').ok_or_else(|| {
                RpcError::bad_request(format!("selector clause {part:?} is not KEY:VALUE"))
            })?;
            let key = Key::parse(key.trim()).ok_or_else(|| {
                RpcError::bad_request(format!(
                    "unknown selector key {:?} (keys: id, title, cwd, cmd, domain, workspace, \
                     window, tab, focused, active, status, exited)",
                    key.trim()
                ))
            })?;
            let expanded;
            let value = if key == Key::Cwd && !value.starts_with("re:") {
                expanded = expand_home(value);
                expanded.as_str()
            } else {
                value
            };
            let (raw, pattern) = match value.strip_prefix("re:") {
                Some(re) => (
                    re.to_string(),
                    Pattern::Re(Regex::new(re).map_err(|e| {
                        RpcError::bad_request(format!("bad regex {re:?} in selector: {e}"))
                    })?),
                ),
                None => (value.to_string(), Pattern::Glob(glob_to_regex(value)?)),
            };
            clauses.push(Clause {
                negate,
                key,
                raw,
                pattern,
            });
        }
        Ok(Selector { clauses })
    }

    /// Whether `expr` reads as a pane selector (`title:dev`, `cwd:~/x,!exited:true`) rather than
    /// a regular expression: it parses and every key applies to a pane. `pane wait` uses this
    /// to tell its two kinds of `--match` apart.
    pub fn is_pane_selector(expr: &str) -> bool {
        Selector::parse(expr)
            .and_then(|s| s.check(Resource::Pane))
            .is_ok()
    }

    /// Fails with a usage error when a clause uses a key that does not apply to `resource`.
    pub fn check(&self, resource: Resource) -> Result<(), RpcError> {
        for c in &self.clauses {
            if !c.key.applies_to(resource) {
                return Err(RpcError::bad_request(format!(
                    "selector key {:?} does not apply to a {}",
                    c.key.name(),
                    resource.name()
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clauses_negation_and_regex() {
        let s = Selector::parse("title:build*,domain:local").unwrap();
        assert_eq!(s.clauses.len(), 2);
        assert_eq!(s.clauses[0].key, Key::Title);
        assert!(s.clauses[0].pattern.is_match("build-api"));
        assert!(!s.clauses[0].pattern.is_match("rebuild"));
        let s = Selector::parse("cmd:re:^cargo (watch|run),!exited:true").unwrap();
        assert!(!s.clauses[0].negate && s.clauses[1].negate);
        assert!(s.clauses[0].pattern.is_match("cargo watch -x test"));
        assert!(!s.clauses[0].pattern.is_match("sudo cargo watch"));
    }

    #[test]
    fn selectors_are_told_apart_from_regexes() {
        assert!(Selector::is_pane_selector("title:dev"));
        assert!(Selector::is_pane_selector("cwd:~/x,!exited:true"));
        assert!(!Selector::is_pane_selector("ready on"));
        assert!(!Selector::is_pane_selector("error: failed"), "unknown key");
        assert!(!Selector::is_pane_selector("^\\d+ passed"));
        assert!(!Selector::is_pane_selector("title:re:("), "bad regex");
    }

    #[test]
    fn glob_is_anchored_and_escapes_regex_chars() {
        let s = Selector::parse("title:a.b?").unwrap();
        assert!(s.clauses[0].pattern.is_match("a.bc"));
        assert!(!s.clauses[0].pattern.is_match("axbc"));
        assert!(!s.clauses[0].pattern.is_match("a.bcd"));
    }

    #[test]
    fn regex_is_unanchored() {
        let s = Selector::parse("title:re:build").unwrap();
        assert!(s.clauses[0].pattern.is_match("my build tab"));
    }

    #[test]
    fn rejects_malformed_selectors() {
        for bad in ["", "title", "nope:1", "title:re:(", ":x"] {
            let e = Selector::parse(bad).unwrap_err();
            assert_eq!(e.code, crate::ErrorCode::BadRequest, "{bad:?}");
        }
    }

    #[test]
    fn every_key_applies_to_the_right_resources() {
        use Resource::*;
        let table: &[(&str, &[Resource])] = &[
            ("id:1", &[Workspace, Window, Tab, Pane]),
            ("title:x", &[Window, Tab, Pane]),
            ("cwd:x", &[Pane]),
            ("cmd:x", &[Pane]),
            ("domain:x", &[Tab, Pane]),
            ("workspace:x", &[Window, Tab, Pane]),
            ("window:1", &[Tab, Pane]),
            ("tab:1", &[Pane]),
            ("focused:true", &[Window, Tab, Pane]),
            ("active:true", &[Window, Tab, Pane]),
            ("status:idle", &[Pane]),
            ("exited:false", &[Pane]),
        ];
        for (expr, ok) in table {
            let s = Selector::parse(expr).unwrap();
            for r in [Workspace, Window, Tab, Pane] {
                assert_eq!(s.check(r).is_ok(), ok.contains(&r), "{expr} on {r:?}");
            }
        }
    }
}
