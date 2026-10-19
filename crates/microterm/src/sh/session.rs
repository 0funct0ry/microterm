//! Session state of `sh` (SPEC §12.7): variables, aliases, the sticky context and the meta
//! commands that are not `ctl` verbs.

use std::collections::BTreeMap;

use mt_proto::Selector;

use super::expand::{Aliases, Sticky};

/// Words that `sh` itself handles. None of them is a `ctl` subcommand (a test checks that).
pub const META_WORDS: &[(&str, &str)] = &[
    ("help", "Show help for sh or for a command"),
    ("exit", "Leave the shell"),
    ("quit", "Leave the shell"),
    (
        "use",
        "Set a sticky default target: use pane|tab|window|workspace ID|NAME",
    ),
    ("set", "Set a variable: set NAME=VALUE"),
    ("unset", "Remove a variable"),
    ("env", "List variables and the context"),
    ("history", "Show the command history"),
    ("json", "Turn JSON output on or off"),
    ("notify", "Turn async event notices on or off"),
    ("pager", "Turn the pager on or off"),
    ("refresh", "Reload the completion snapshot"),
    ("source", "Run the commands of a file"),
    ("alias", "Define or list aliases"),
    ("reconnect", "Connect to the running instance again"),
    ("clear", "Clear the screen"),
    ("version", "Show the microterm version"),
];

/// Detailed help for each shell command (`help use`).
pub const META_HELP: &[(&str, &str)] = &[
    ("help", "help [COMMAND…]\n\nWithout arguments: the shell commands and the ctl commands. With a shell\ncommand (`help use`): its details. With a ctl command (`help pane`, `help pane get`,\n`help p`): that command's own help, as `--help` would print it."),
    ("exit", "exit\n\nLeaves the shell with status 0. Also `quit` and Ctrl-D."),
    ("quit", "quit\n\nSame as `exit`."),
    ("use", "use pane|tab|window ID\nuse workspace NAME\nuse --match EXPR\nuse            show the context\nuse -          clear it\n\nSets a sticky default target. It ranks below an explicit ID and --match, and alongside\n$MICROTERM_*. It is never applied to close, delete or send-text. `use workspace` feeds\n`tab new` and `window new`; `use --match` pins a selector that is evaluated again for each\ncommand. The prompt shows the context, e.g. microterm[pane 7]>."),
    ("set", "set NAME=VALUE\nset            list the variables\n\nSets a variable; use it as $NAME or ${NAME}. $? is the last exit code, and $tab, $pane and\n$window hold the IDs from the last `new` or `replay`. Unknown names fall back to the\nenvironment, then to the empty string. Single quotes prevent expansion."),
    ("unset", "unset NAME\n\nRemoves a variable."),
    ("env", "env\n\nShows the context, the json / readonly settings, $? and every variable."),
    ("history", "history\n\nPrints the numbered commands of this session. Lines containing send-text (and the `type`\nalias) are not recorded unless [sh] history_send_text = true. Ctrl-R searches the saved\nhistory."),
    ("json", "json on|off\njson           show the setting\n\nWith json on every command prints the raw JSON result, like `ctl --json`."),
    ("notify", "notify on|off\nnotify         show the setting\n\nControls the `[event] pane 12 exited` lines (exits, finished commands, notifications,\ndomain changes) that are printed above the next prompt."),
    ("pager", "pager on|off\npager         show the setting\n\nWith the pager on, output longer than the screen goes through $PAGER (default `less -R`)."),
    ("refresh", "refresh\n\nReloads the snapshot behind completion (IDs, workspaces, domains). It is refreshed\nautomatically when microterm reports a change."),
    ("source", "source FILE\n\nRuns the commands of FILE as a script, in this session (variables and context are kept)."),
    ("alias", "alias NAME=COMMAND…\nalias          list the aliases\n\nDefines an alias, expanded once on the first word: `alias ll='pane list --json'`. Built in:\nls=list ws=workspace w=window t=tab p=pane type=pane send-text cat=pane get-text."),
    ("reconnect", "reconnect\n\nConnects to the running instance again (the prompt shows ⚠ while there is none). The next\ncommand also reconnects by itself."),
    ("clear", "clear\n\nClears the screen."),
    ("version", "version\n\nPrints the microterm version."),
];

pub fn is_meta(word: &str) -> bool {
    META_WORDS.iter().any(|(w, _)| *w == word)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    On,
    Off,
    Show,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UseArg {
    Show,
    Clear,
    Pane(u64),
    Tab(u64),
    Window(u64),
    Workspace(String),
    Match(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Meta {
    Help(Vec<String>),
    Exit,
    Use(UseArg),
    Set(Option<(String, String)>),
    Unset(String),
    Env,
    History,
    Json(Switch),
    Notify(Switch),
    Pager(Switch),
    Refresh,
    Source(String),
    Alias(Option<(String, Vec<String>)>),
    Reconnect,
    Clear,
    Version,
}

fn switch(args: &[String]) -> Result<Switch, String> {
    match args.first().map(String::as_str) {
        None => Ok(Switch::Show),
        Some("on") => Ok(Switch::On),
        Some("off") => Ok(Switch::Off),
        Some(o) => Err(format!("expected on or off, not {o:?}")),
    }
}

fn id(s: &str, what: &str) -> Result<u64, String> {
    s.parse()
        .map_err(|_| format!("{what} IDs are numbers, not {s:?}"))
}

/// `None` when the first word is not a meta command.
pub fn parse_meta(words: &[String]) -> Option<Result<Meta, String>> {
    let first = words.first()?;
    if !is_meta(first) {
        return None;
    }
    let args = &words[1..];
    Some(match first.as_str() {
        "help" => Ok(Meta::Help(args.to_vec())),
        "exit" | "quit" => Ok(Meta::Exit),
        "use" => parse_use(args).map(Meta::Use),
        "set" => match args {
            [] => Ok(Meta::Set(None)),
            [kv] => match kv.split_once('=') {
                Some((k, v)) if valid_name(k) => Ok(Meta::Set(Some((k.into(), v.into())))),
                _ => Err("usage: set NAME=VALUE".into()),
            },
            [k, eq, v] if eq == "=" && valid_name(k) => Ok(Meta::Set(Some((k.clone(), v.clone())))),
            _ => Err("usage: set NAME=VALUE (quote values with spaces)".into()),
        },
        "unset" => match args {
            [n] if valid_name(n) => Ok(Meta::Unset(n.clone())),
            _ => Err("usage: unset NAME".into()),
        },
        "env" => Ok(Meta::Env),
        "history" => Ok(Meta::History),
        "json" => switch(args).map(Meta::Json),
        "notify" => switch(args).map(Meta::Notify),
        "pager" => switch(args).map(Meta::Pager),
        "refresh" => Ok(Meta::Refresh),
        "source" => match args {
            [f] => Ok(Meta::Source(f.clone())),
            _ => Err("usage: source FILE".into()),
        },
        "alias" => match args {
            [] => Ok(Meta::Alias(None)),
            [def, rest @ ..] if def.contains('=') => {
                let (name, value) = def.split_once('=').expect("checked");
                let mut words: Vec<String> = value.split_whitespace().map(String::from).collect();
                words.extend(rest.iter().cloned());
                alias_def(name, words)
            }
            [name, rest @ ..] if valid_name(name) && !rest.is_empty() => {
                alias_def(name, rest.to_vec())
            }
            _ => Err("usage: alias NAME=COMMAND…".into()),
        },
        "reconnect" => Ok(Meta::Reconnect),
        "clear" => Ok(Meta::Clear),
        "version" => Ok(Meta::Version),
        _ => unreachable!("is_meta"),
    })
}

fn alias_def(name: &str, value: Vec<String>) -> Result<Meta, String> {
    if !valid_name(name) || value.is_empty() {
        return Err("usage: alias NAME=COMMAND…".into());
    }
    if is_meta(name) {
        return Err(format!(
            "{name:?} is a shell command and cannot be an alias"
        ));
    }
    Ok(Meta::Alias(Some((name.into(), value))))
}

fn valid_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_use(args: &[String]) -> Result<UseArg, String> {
    match args {
        [] => Ok(UseArg::Show),
        [d] if d == "-" => Ok(UseArg::Clear),
        [m, expr] if m == "--match" => {
            Selector::parse(expr).map_err(|e| e.message)?;
            Ok(UseArg::Match(expr.clone()))
        }
        [kind, value] => match kind.as_str() {
            "pane" => Ok(UseArg::Pane(id(value, "pane")?)),
            "tab" => Ok(UseArg::Tab(id(value, "tab")?)),
            "window" => Ok(UseArg::Window(id(value, "window")?)),
            "workspace" => Ok(UseArg::Workspace(value.clone())),
            _ => {
                Err("usage: use pane|tab|window|workspace ID|NAME, use --match EXPR, use -".into())
            }
        },
        _ => Err("usage: use pane|tab|window|workspace ID|NAME, use --match EXPR, use -".into()),
    }
}

/// Everything a session remembers between lines.
#[derive(Debug, Default)]
pub struct Session {
    pub sticky: Sticky,
    pub vars: BTreeMap<String, String>,
    pub aliases: Aliases,
    /// Exit code of the last statement (`$?`).
    pub last: i32,
}

impl Session {
    /// `$NAME`: `?`, then session variables, then the environment.
    pub fn lookup(&self, name: &str) -> Option<String> {
        if name == "?" {
            return Some(self.last.to_string());
        }
        self.vars
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    }

    pub fn apply_use(&mut self, u: UseArg) -> String {
        match u {
            UseArg::Show => {}
            UseArg::Clear => self.sticky = Sticky::default(),
            UseArg::Pane(n) => self.sticky.pane = Some(n),
            UseArg::Tab(n) => self.sticky.tab = Some(n),
            UseArg::Window(n) => self.sticky.window = Some(n),
            UseArg::Workspace(w) => self.sticky.workspace = Some(w),
            UseArg::Match(e) => self.sticky.expr = Some(e),
        }
        self.sticky.describe()
    }

    /// `$tab`, `$pane` and `$window` from the last `new` or `replay`.
    pub fn remember_spawned(&mut self, s: &mt_proto::Spawned) {
        self.vars.insert("tab".into(), s.tab.to_string());
        self.vars.insert("pane".into(), s.pane.to_string());
        self.vars.insert("window".into(), s.window.to_string());
    }
}

/// Whether `argv` (already alias-expanded) may be stored in the history: lines that type into a
/// pane can carry secrets, so they are dropped unless `[sh] history_send_text` is set.
pub fn history_allows(line: &str, aliases: &Aliases, allow_send_text: bool) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    if allow_send_text {
        return true;
    }
    let words: Vec<String> = line.split_whitespace().map(String::from).collect();
    let expanded = super::expand::expand_aliases(words, aliases);
    !expanded.iter().any(|w| w == "send-text") && !line.contains("send-text")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn meta(s: &str) -> Meta {
        parse_meta(&w(s)).expect("meta").expect("valid")
    }

    #[test]
    fn use_forms() {
        assert_eq!(meta("use"), Meta::Use(UseArg::Show));
        assert_eq!(meta("use -"), Meta::Use(UseArg::Clear));
        assert_eq!(meta("use pane 7"), Meta::Use(UseArg::Pane(7)));
        assert_eq!(
            meta("use workspace api"),
            Meta::Use(UseArg::Workspace("api".into()))
        );
        assert_eq!(
            meta("use --match title:build*"),
            Meta::Use(UseArg::Match("title:build*".into()))
        );
        assert!(parse_meta(&w("use pane x")).unwrap().is_err());
        assert!(parse_meta(&w("use --match bogus:1")).unwrap().is_err());
        assert!(parse_meta(&w("use galaxy 1")).unwrap().is_err());
    }

    #[test]
    fn set_forms_and_switches() {
        assert_eq!(meta("set A=1"), Meta::Set(Some(("A".into(), "1".into()))));
        assert_eq!(meta("set"), Meta::Set(None));
        assert!(parse_meta(&w("set 1x=3")).unwrap().is_err());
        assert_eq!(meta("json on"), Meta::Json(Switch::On));
        assert_eq!(meta("pager"), Meta::Pager(Switch::Show));
        assert!(parse_meta(&w("json maybe")).unwrap().is_err());
    }

    #[test]
    fn alias_definitions() {
        assert_eq!(
            meta("alias ll=pane list --json"),
            Meta::Alias(Some(("ll".into(), w("pane list --json"))))
        );
        assert_eq!(
            meta("alias ll pane list"),
            Meta::Alias(Some(("ll".into(), w("pane list"))))
        );
        assert!(parse_meta(&w("alias exit=list")).unwrap().is_err());
    }

    #[test]
    fn ctl_words_are_not_meta() {
        for word in [
            "list",
            "workspace",
            "window",
            "tab",
            "pane",
            "events",
            "record",
            "replay",
        ] {
            assert!(parse_meta(&w(word)).is_none(), "{word}");
        }
    }

    #[test]
    fn variables_resolve_in_order() {
        let mut s = Session {
            last: 4,
            ..Session::default()
        };
        s.vars.insert("HOME".into(), "/session".into());
        assert_eq!(s.lookup("?").as_deref(), Some("4"));
        assert_eq!(s.lookup("HOME").as_deref(), Some("/session"));
        assert_eq!(s.lookup("MICROTERM_SURELY_UNSET_VAR"), None);
        s.remember_spawned(&mt_proto::Spawned {
            pane: 12,
            tab: 9,
            window: 1,
        });
        assert_eq!(s.lookup("pane").as_deref(), Some("12"));
        assert_eq!(s.lookup("tab").as_deref(), Some("9"));
        assert_eq!(s.lookup("window").as_deref(), Some("1"));
    }

    #[test]
    fn use_updates_and_describes_the_context() {
        let mut s = Session::default();
        assert!(s.apply_use(UseArg::Show).contains("no context"));
        assert_eq!(s.apply_use(UseArg::Pane(7)), "pane 7");
        assert_eq!(s.apply_use(UseArg::Tab(3)), "pane 7, tab 3");
        s.apply_use(UseArg::Clear);
        assert!(s.sticky.is_empty());
    }

    #[test]
    fn send_text_lines_are_not_stored_unless_allowed() {
        let a = Aliases::default();
        assert!(history_allows("pane list", &a, false));
        assert!(!history_allows(
            "pane send-text --pane 7 'secret'",
            &a,
            false
        ));
        assert!(!history_allows("type 7 hunter2", &a, false));
        assert!(!history_allows("ctl pane send-text hi", &a, false));
        assert!(history_allows("pane send-text hi", &a, true));
        assert!(!history_allows("   ", &a, true));
    }
}
