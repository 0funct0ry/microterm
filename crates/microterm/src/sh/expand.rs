//! Rewrites what was typed into canonical `ctl` argv (SPEC §12.7): aliases, the positional target
//! shorthand and the sticky context of `use`. All of it is sugar over the clap tree, which stays
//! the single source of truth for what a command means.

use std::collections::HashMap;

/// Sh-only aliases, expanded on the first word.
pub const BUILTIN_ALIASES: &[(&str, &[&str])] = &[
    ("ls", &["list"]),
    ("ws", &["workspace"]),
    ("w", &["window"]),
    ("t", &["tab"]),
    ("p", &["pane"]),
    ("type", &["pane", "send-text"]),
    ("cat", &["pane", "get-text"]),
];

#[derive(Debug, Clone, Default)]
pub struct Aliases(pub HashMap<String, Vec<String>>);

impl Aliases {
    pub fn get(&self, name: &str) -> Option<Vec<String>> {
        self.0.get(name).cloned().or_else(|| {
            BUILTIN_ALIASES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.iter().map(|s| s.to_string()).collect())
        })
    }
}

/// Drops a leading `ctl` and expands an alias on the first word (once; no recursion).
pub fn expand_aliases(mut words: Vec<String>, aliases: &Aliases) -> Vec<String> {
    if words.first().map(String::as_str) == Some("ctl") {
        words.remove(0);
    }
    if let Some(first) = words.first() {
        if let Some(expansion) = aliases.get(first) {
            words.splice(0..1, expansion);
        }
    }
    words
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pane,
    Tab,
    Window,
}

impl Kind {
    pub fn flag(self) -> &'static str {
        match self {
            Kind::Pane => "--pane",
            Kind::Tab => "--tab",
            Kind::Window => "--window",
        }
    }
}

/// How the sugar treats one verb.
pub struct Verb {
    /// Words that name it: `["pane", "status", "set"]`.
    pub path: &'static [&'static str],
    /// The resource its target flag names.
    pub kind: Kind,
    /// A trailing positional (the text) follows the target, so the shorthand needs two
    /// positionals to be unambiguous.
    pub trailing_text: bool,
    /// Whether `use` supplies a default target (never for close, delete and send-text).
    pub sticky: bool,
}

pub const TARGET_VERBS: &[Verb] = &[
    Verb {
        path: &["pane", "get"],
        kind: Kind::Pane,
        trailing_text: false,
        sticky: true,
    },
    Verb {
        path: &["pane", "get-text"],
        kind: Kind::Pane,
        trailing_text: false,
        sticky: true,
    },
    Verb {
        path: &["pane", "send-text"],
        kind: Kind::Pane,
        trailing_text: true,
        sticky: false,
    },
    Verb {
        path: &["pane", "notify"],
        kind: Kind::Pane,
        trailing_text: true,
        sticky: true,
    },
    Verb {
        path: &["pane", "close"],
        kind: Kind::Pane,
        trailing_text: false,
        sticky: false,
    },
    Verb {
        path: &["pane", "status", "get"],
        kind: Kind::Pane,
        trailing_text: false,
        sticky: true,
    },
    Verb {
        path: &["pane", "status", "set"],
        kind: Kind::Pane,
        trailing_text: true,
        sticky: true,
    },
    Verb {
        path: &["pane", "status", "clear"],
        kind: Kind::Pane,
        trailing_text: false,
        sticky: true,
    },
    Verb {
        path: &["tab", "focus"],
        kind: Kind::Tab,
        trailing_text: false,
        sticky: true,
    },
    Verb {
        path: &["tab", "rename"],
        kind: Kind::Tab,
        trailing_text: true,
        sticky: true,
    },
    Verb {
        path: &["tab", "close"],
        kind: Kind::Tab,
        trailing_text: false,
        sticky: false,
    },
    Verb {
        path: &["window", "focus"],
        kind: Kind::Window,
        trailing_text: false,
        sticky: true,
    },
    Verb {
        path: &["window", "close"],
        kind: Kind::Window,
        trailing_text: false,
        sticky: false,
    },
];

/// `list` verbs: a bare argument is a selector.
pub const LIST_VERBS: &[&[&str]] = &[&["pane", "list"], &["tab", "list"], &["window", "list"]];

fn path_matches(argv: &[String], path: &[&str]) -> bool {
    argv.len() >= path.len() && argv.iter().zip(path).all(|(a, p)| a == p)
}

pub fn find_verb(argv: &[String]) -> Option<&'static Verb> {
    TARGET_VERBS.iter().find(|v| path_matches(argv, v.path))
}

fn is_list(argv: &[String]) -> Option<&'static [&'static str]> {
    LIST_VERBS.iter().copied().find(|p| path_matches(argv, p))
}

fn is_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Arguments after the verb path, up to a `--`.
fn args_after(argv: &[String], skip: usize) -> &[String] {
    let rest = &argv[skip.min(argv.len())..];
    let end = rest.iter().position(|a| a == "--").unwrap_or(rest.len());
    &rest[..end]
}

/// `pane get 7` → `pane get --pane 7`; `pane list 'title:x'` → `pane list --match 'title:x'`.
pub fn shorthand(argv: Vec<String>) -> Vec<String> {
    let mut argv = argv;
    if let Some(v) = find_verb(&argv) {
        let n = v.path.len();
        let args = args_after(&argv, n);
        let Some(first) = args.first().filter(|a| !a.starts_with('-')) else {
            return argv;
        };
        let positionals = args.iter().filter(|a| !a.starts_with('-')).count();
        let needed = if v.trailing_text { 2 } else { 1 };
        if positionals < needed {
            return argv;
        }
        let replacement: Vec<String> = if is_id(first) {
            vec![v.kind.flag().into(), first.clone()]
        } else {
            vec!["--match".into(), first.clone()]
        };
        argv.splice(n..n + 1, replacement);
    } else if let Some(path) = is_list(&argv) {
        let n = path.len();
        let Some(first) = args_after(&argv, n)
            .first()
            .filter(|a| !a.starts_with('-'))
            .cloned()
        else {
            return argv;
        };
        let expr = if is_id(&first) {
            format!("id:{first}")
        } else {
            first
        };
        argv.splice(n..n + 1, ["--match".to_string(), expr]);
    }
    argv
}

/// The sticky default target of `use`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sticky {
    pub pane: Option<u64>,
    pub tab: Option<u64>,
    pub window: Option<u64>,
    pub workspace: Option<String>,
    /// A pinned selector, re-evaluated by the server for each command.
    pub expr: Option<String>,
}

impl Sticky {
    pub fn is_empty(&self) -> bool {
        *self == Sticky::default()
    }

    fn id(&self, kind: Kind) -> Option<u64> {
        match kind {
            Kind::Pane => self.pane,
            Kind::Tab => self.tab,
            Kind::Window => self.window,
        }
    }

    /// One line for `use` with no argument.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(p) = self.pane {
            parts.push(format!("pane {p}"));
        }
        if let Some(t) = self.tab {
            parts.push(format!("tab {t}"));
        }
        if let Some(w) = self.window {
            parts.push(format!("window {w}"));
        }
        if let Some(w) = &self.workspace {
            parts.push(format!("workspace {w}"));
        }
        if let Some(e) = &self.expr {
            parts.push(format!("match {e}"));
        }
        if parts.is_empty() {
            "no context (explicit targets, $MICROTERM_* or the focused item apply)".into()
        } else {
            parts.join(", ")
        }
    }
}

fn has_flag(argv: &[String], names: &[&str]) -> bool {
    argv.iter().take_while(|a| *a != "--").any(|a| {
        names
            .iter()
            .any(|n| a == n || a.starts_with(&format!("{n}=")))
    })
}

/// Adds the sticky context where the command names no target of its own. An explicit ID or
/// `--match` always wins; `close`, `delete` and `send-text` never get a sticky target.
pub fn apply_sticky(argv: Vec<String>, sticky: &Sticky) -> Vec<String> {
    if sticky.is_empty() {
        return argv;
    }
    let mut inject: Vec<String> = Vec::new();
    let at;
    if let Some(v) = find_verb(&argv) {
        at = v.path.len();
        if !v.sticky {
            return argv;
        }
        let own = [v.kind.flag(), "--match"];
        if !has_flag(&argv, &own) {
            if let Some(id) = sticky.id(v.kind) {
                inject.extend([v.kind.flag().to_string(), id.to_string()]);
            } else if let Some(e) = &sticky.expr {
                inject.extend(["--match".to_string(), e.clone()]);
            }
        }
    } else if let Some(path) = is_list(&argv) {
        at = path.len();
        if !has_flag(&argv, &["--match"]) {
            if let Some(e) = &sticky.expr {
                inject.extend(["--match".to_string(), e.clone()]);
            }
        }
        if path[0] == "pane" && !has_flag(&argv, &["--tab"]) {
            if let Some(t) = sticky.tab {
                inject.extend(["--tab".to_string(), t.to_string()]);
            }
        }
        if matches!(path[0], "pane" | "tab") && !has_flag(&argv, &["--window"]) {
            if let Some(w) = sticky.window {
                inject.extend(["--window".to_string(), w.to_string()]);
            }
        }
    } else if (path_matches(&argv, &["tab", "new"]) || path_matches(&argv, &["window", "new"]))
        && !has_flag(&argv, &["--workspace"])
    {
        at = 2;
        if let Some(ws) = &sticky.workspace {
            inject.extend(["--workspace".to_string(), ws.clone()]);
        }
    } else {
        return argv;
    }
    let mut argv = argv;
    argv.splice(at..at, inject);
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn expand(s: &str) -> Vec<String> {
        expand_aliases(v(s), &Aliases::default())
    }

    #[test]
    fn builtin_aliases_expand_on_the_first_word_only() {
        assert_eq!(expand("ls"), ["list"]);
        assert_eq!(expand("p list"), ["pane", "list"]);
        assert_eq!(expand("w close 3"), ["window", "close", "3"]);
        assert_eq!(expand("type hello"), ["pane", "send-text", "hello"]);
        assert_eq!(expand("cat 7"), ["pane", "get-text", "7"]);
        assert_eq!(expand("pane list t"), ["pane", "list", "t"]);
        assert_eq!(expand("ctl pane list"), ["pane", "list"]);
    }

    #[test]
    fn user_aliases_win_and_do_not_recurse() {
        let mut a = Aliases::default();
        a.0.insert("ls".into(), v("pane list --json"));
        a.0.insert("x".into(), v("ls"));
        assert_eq!(expand_aliases(v("ls"), &a), ["pane", "list", "--json"]);
        assert_eq!(expand_aliases(v("x"), &a), ["ls"]);
    }

    #[test]
    fn positional_target_shorthand() {
        assert_eq!(shorthand(v("pane get 7")), ["pane", "get", "--pane", "7"]);
        assert_eq!(
            shorthand(v("pane get title:build*")),
            ["pane", "get", "--match", "title:build*"]
        );
        assert_eq!(
            shorthand(v("tab close 3 --force")),
            ["tab", "close", "--tab", "3", "--force"]
        );
        assert_eq!(
            shorthand(v("pane status get 4")),
            ["pane", "status", "get", "--pane", "4"]
        );
        // Flags and an explicit target are left alone.
        assert_eq!(
            shorthand(v("pane get --pane 7")),
            ["pane", "get", "--pane", "7"]
        );
        assert_eq!(shorthand(v("pane get")), ["pane", "get"]);
    }

    #[test]
    fn text_verbs_need_a_target_and_a_text() {
        // One positional is the text, not a target.
        assert_eq!(shorthand(v("pane send-text 7")), ["pane", "send-text", "7"]);
        assert_eq!(
            shorthand(v("pane send-text 7 hello")),
            ["pane", "send-text", "--pane", "7", "hello"]
        );
        assert_eq!(
            shorthand(v("tab rename 3 build")),
            ["tab", "rename", "--tab", "3", "build"]
        );
        assert_eq!(shorthand(v("tab rename build")), ["tab", "rename", "build"]);
    }

    #[test]
    fn bare_selectors_filter_list_verbs() {
        assert_eq!(
            shorthand(v("pane list title:*cargo*,!exited")),
            ["pane", "list", "--match", "title:*cargo*,!exited"]
        );
        assert_eq!(
            shorthand(v("pane list 7")),
            ["pane", "list", "--match", "id:7"]
        );
        assert_eq!(shorthand(v("pane list --json")), ["pane", "list", "--json"]);
        assert_eq!(shorthand(v("tab list")), ["tab", "list"]);
    }

    #[test]
    fn arguments_after_double_dash_are_never_rewritten() {
        assert_eq!(shorthand(v("pane get -- 7")), ["pane", "get", "--", "7"]);
    }

    fn sticky_pane(id: u64) -> Sticky {
        Sticky {
            pane: Some(id),
            ..Sticky::default()
        }
    }

    #[test]
    fn sticky_context_fills_a_missing_target() {
        assert_eq!(
            apply_sticky(v("pane get-text --escapes"), &sticky_pane(7)),
            ["pane", "get-text", "--pane", "7", "--escapes"]
        );
        assert_eq!(
            apply_sticky(v("pane status set hi"), &sticky_pane(7)),
            ["pane", "status", "set", "--pane", "7", "hi"]
        );
    }

    #[test]
    fn sticky_ranks_below_explicit_ids_and_matches() {
        let s = sticky_pane(7);
        assert_eq!(
            apply_sticky(v("pane get --pane 9"), &s),
            ["pane", "get", "--pane", "9"]
        );
        assert_eq!(
            apply_sticky(v("pane get --match title:x"), &s),
            ["pane", "get", "--match", "title:x"]
        );
        assert_eq!(
            apply_sticky(v("pane get --pane=9"), &s),
            ["pane", "get", "--pane=9"]
        );
    }

    #[test]
    fn sticky_never_reaches_close_delete_or_send_text() {
        let s = Sticky {
            pane: Some(7),
            tab: Some(3),
            window: Some(1),
            workspace: Some("api".into()),
            expr: Some("title:x".into()),
        };
        for line in [
            "pane close",
            "tab close",
            "window close",
            "pane send-text hi",
            "workspace delete old",
        ] {
            assert_eq!(apply_sticky(v(line), &s), v(line), "{line}");
        }
    }

    #[test]
    fn a_pinned_selector_applies_to_any_resource() {
        let s = Sticky {
            expr: Some("title:build*".into()),
            ..Sticky::default()
        };
        assert_eq!(
            apply_sticky(v("tab focus"), &s),
            ["tab", "focus", "--match", "title:build*"]
        );
        assert_eq!(
            apply_sticky(v("pane list"), &s),
            ["pane", "list", "--match", "title:build*"]
        );
    }

    #[test]
    fn tab_and_window_context_narrow_lists_and_workspace_feeds_new() {
        let s = Sticky {
            tab: Some(3),
            window: Some(1),
            workspace: Some("api".into()),
            ..Sticky::default()
        };
        assert_eq!(
            apply_sticky(v("pane list"), &s),
            ["pane", "list", "--tab", "3", "--window", "1"]
        );
        assert_eq!(
            apply_sticky(v("tab list"), &s),
            ["tab", "list", "--window", "1"]
        );
        assert_eq!(
            apply_sticky(v("tab new --title x"), &s),
            ["tab", "new", "--workspace", "api", "--title", "x"]
        );
        assert_eq!(
            apply_sticky(v("tab new --workspace other"), &s),
            ["tab", "new", "--workspace", "other"]
        );
        assert_eq!(
            apply_sticky(v("tab focus"), &s),
            ["tab", "focus", "--tab", "3"]
        );
    }

    #[test]
    fn an_empty_context_changes_nothing() {
        assert_eq!(
            apply_sticky(v("pane get"), &Sticky::default()),
            v("pane get")
        );
    }
}
