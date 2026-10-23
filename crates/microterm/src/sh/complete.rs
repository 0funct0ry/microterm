//! Completion for `sh` (SPEC §12.7): three layers merged into one candidate list.
//!
//! 1. *Static*, from the clap tree (the same source as `microterm completions`): resources,
//!    verbs, flags, value hints and help text, plus the alias and meta-command overlay.
//! 2. *Dynamic*, from a cached `mux.list` snapshot: pane/tab/window IDs with
//!    `id  title  cwd` descriptions, workspace names, domains, and filesystem paths.
//! 3. *Selector-aware*, for `--match` values and bare selectors: keys, `!` negation, per-key
//!    values, and a live `matches N panes` hint from `mt_proto::selector`.
//!
//! Everything here is a pure function of the line, the cursor and the snapshot, so it is tested
//! without a terminal. Without a snapshot (no instance running) only layer 1 applies.

use std::sync::OnceLock;

use clap::{builder::ValueHint, ArgAction, Command, CommandFactory};
use mt_mux::ctl::{matches as selector_matches, PaneRow};
use mt_proto::{MuxSnapshot, Resource, Selector};

use super::expand::{find_verb, Aliases, Kind, BUILTIN_ALIASES, LIST_VERBS};
use super::session::META_WORDS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub replacement: String,
    pub display: String,
}

impl Candidate {
    fn new(replacement: impl Into<String>, description: &str) -> Self {
        let replacement = replacement.into();
        let display = if description.is_empty() {
            replacement.clone()
        } else {
            format!("{replacement}  {description}")
        };
        Candidate {
            replacement,
            display,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Completion {
    /// Byte offset in the line where the replaced word starts.
    pub start: usize,
    pub items: Vec<Candidate>,
    /// The selector being typed, when the cursor is inside one (for the live hint).
    pub selector: Option<String>,
}

pub struct CompCtx<'a> {
    pub snap: Option<&'a MuxSnapshot>,
    pub aliases: &'a Aliases,
    /// Snippet ids, completed after `snippet show|expand|insert|rm` (empty while disabled).
    pub snippets: &'a [String],
    /// Recording ids, completed after `recording show|stop|rename|rm|replay|export`.
    pub recordings: &'a [String],
}

/// The selector keys of SPEC §12.3 with their meaning.
pub const SELECTOR_KEYS: &[(&str, &str)] = &[
    ("id", "numeric ID"),
    ("title", "title (glob)"),
    ("cwd", "working directory (glob, ~ expanded)"),
    ("cmd", "foreground process"),
    ("domain", "local or an SSH domain"),
    ("workspace", "workspace name"),
    ("window", "window ID"),
    ("tab", "tab ID"),
    ("focused", "true | false"),
    ("active", "true | false"),
    ("status", "idle | working | waiting | done | error"),
    ("exited", "true | false"),
];

const STATES: &[&str] = &["idle", "working", "waiting", "done", "error"];

/// The `ctl` command tree with global flags propagated.
fn ctl_command() -> &'static Command {
    static CMD: OnceLock<Command> = OnceLock::new();
    CMD.get_or_init(|| {
        let mut c = crate::ctl::CtlArgs::command();
        c.build();
        c
    })
}

/// Quotes `s` for the shell grammar of `sh` when it needs it.
pub fn quote_arg(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@%+,~!".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The text of the statement the cursor is in, and its byte offset in the line. `None` when the
/// cursor is in a host command (`!cmd` or after a `|`).
fn current_statement(upto: &str) -> Option<(usize, &str)> {
    let trimmed = upto.trim_start();
    if trimmed.starts_with('!') {
        return None;
    }
    let bytes: Vec<(usize, char)> = upto.char_indices().collect();
    let mut start = 0;
    let (mut single, mut double, mut esc) = (false, false, false);
    let mut i = 0;
    while i < bytes.len() {
        let (idx, c) = bytes[i];
        let next = bytes.get(i + 1).map(|(_, n)| *n);
        if esc {
            esc = false;
        } else if single {
            single = c != '\'';
        } else if double {
            match c {
                '\\' => esc = true,
                '"' => double = false,
                _ => {}
            }
        } else {
            match c {
                '\\' => esc = true,
                '\'' => single = true,
                '"' => double = true,
                ';' => start = idx + 1,
                '&' | '|' if next == Some(c) => {
                    start = idx + 2;
                    i += 1;
                }
                '|' => return None,
                _ => {}
            }
        }
        i += 1;
    }
    Some((start, &upto[start..]))
}

struct Partial {
    words: Vec<String>,
    cur: String,
    /// Byte offset of the word being completed within the statement.
    cur_start: usize,
}

/// Splits a statement into finished words and the word under the cursor, undoing quotes.
fn split_partial(text: &str) -> Partial {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut cur_start = text.len();
    let (mut single, mut double, mut esc) = (false, false, false);
    for (idx, c) in text.char_indices() {
        if esc {
            esc = false;
            cur.push(c);
            continue;
        }
        if single {
            if c == '\'' {
                single = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        if double {
            match c {
                '\\' => esc = true,
                '"' => double = false,
                c => cur.push(c),
            }
            continue;
        }
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            _ => {
                if !in_word {
                    in_word = true;
                    cur_start = idx;
                }
                match c {
                    '\\' => esc = true,
                    '\'' => single = true,
                    '"' => double = true,
                    c => cur.push(c),
                }
            }
        }
    }
    if !in_word {
        cur_start = text.len();
    }
    Partial {
        words,
        cur,
        cur_start,
    }
}

fn filter(items: Vec<Candidate>, prefix: &str, quote: bool) -> Vec<Candidate> {
    items
        .into_iter()
        .filter(|c| c.replacement.starts_with(prefix))
        .map(|mut c| {
            if quote {
                let was = c.replacement.clone();
                c.replacement = quote_arg(&was);
                c.display = c.display.replacen(&was, &c.replacement, 1);
            }
            c
        })
        .collect()
}

fn first_line(help: Option<&clap::builder::StyledStr>) -> String {
    help.map(|h| h.to_string().lines().next().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn subcommand_candidates(cmd: &Command) -> Vec<Candidate> {
    cmd.get_subcommands()
        .filter(|s| !s.is_hide_set() && s.get_name() != "help")
        .map(|s| Candidate::new(s.get_name(), &first_line(s.get_about())))
        .collect()
}

fn flag_candidates(cmd: &Command) -> Vec<Candidate> {
    let mut out = Vec::new();
    for a in cmd.get_arguments() {
        if a.is_hide_set() || matches!(a.get_id().as_str(), "help" | "version") {
            continue;
        }
        if let Some(long) = a.get_long() {
            out.push(Candidate::new(
                format!("--{long}"),
                &first_line(a.get_help()),
            ));
        }
    }
    out
}

fn arg_takes_value(a: &clap::Arg) -> bool {
    matches!(a.get_action(), ArgAction::Set | ArgAction::Append)
}

fn find_flag<'a>(cmd: &'a Command, word: &str) -> Option<&'a clap::Arg> {
    let long = word.strip_prefix("--")?;
    cmd.get_arguments().find(|a| a.get_long() == Some(long))
}

fn short_flag<'a>(cmd: &'a Command, word: &str) -> Option<&'a clap::Arg> {
    let mut chars = word.strip_prefix('-')?.chars();
    let c = chars.next()?;
    chars
        .next()
        .is_none()
        .then(|| cmd.get_arguments().find(|a| a.get_short() == Some(c)))?
}

fn describe_pane(p: &mt_proto::PaneInfo) -> String {
    let mut s = p.title.clone();
    if let Some(cwd) = &p.cwd {
        s.push_str("  ");
        s.push_str(cwd);
    }
    s
}

fn pane_ids(snap: &MuxSnapshot) -> Vec<Candidate> {
    snap.workspaces
        .iter()
        .flat_map(|w| &w.windows)
        .flat_map(|w| &w.tabs)
        .flat_map(|t| &t.panes)
        .map(|p| Candidate::new(p.id.to_string(), &describe_pane(p)))
        .collect()
}

fn tab_ids(snap: &MuxSnapshot) -> Vec<Candidate> {
    snap.workspaces
        .iter()
        .flat_map(|w| &w.windows)
        .flat_map(|w| &w.tabs)
        .map(|t| Candidate::new(t.id.to_string(), &t.title))
        .collect()
}

fn window_ids(snap: &MuxSnapshot) -> Vec<Candidate> {
    snap.workspaces
        .iter()
        .flat_map(|ws| ws.windows.iter().map(move |w| (ws, w)))
        .map(|(ws, w)| Candidate::new(w.id.to_string(), &format!("{}  {}", w.title, ws.name)))
        .collect()
}

fn workspace_names(snap: &MuxSnapshot) -> Vec<Candidate> {
    snap.workspaces
        .iter()
        .map(|w| {
            let n: usize = w.windows.iter().map(|w| w.tabs.len()).sum();
            Candidate::new(w.name.clone(), &format!("{n} tabs"))
        })
        .collect()
}

fn dedup(values: impl Iterator<Item = String>) -> Vec<Candidate> {
    let mut seen = std::collections::BTreeSet::new();
    values
        .filter(|v| !v.is_empty() && seen.insert(v.clone()))
        .map(|v| Candidate::new(v, ""))
        .collect()
}

fn panes(snap: &MuxSnapshot) -> impl Iterator<Item = &mt_proto::PaneInfo> {
    snap.workspaces
        .iter()
        .flat_map(|w| &w.windows)
        .flat_map(|w| &w.tabs)
        .flat_map(|t| &t.panes)
}

fn domains(snap: &MuxSnapshot) -> Vec<Candidate> {
    dedup(panes(snap).map(|p| p.domain.clone()))
}

fn id_candidates(kind: Kind, snap: Option<&MuxSnapshot>) -> Vec<Candidate> {
    match (snap, kind) {
        (None, _) => vec![],
        (Some(s), Kind::Pane) => pane_ids(s),
        (Some(s), Kind::Tab) => tab_ids(s),
        (Some(s), Kind::Window) => window_ids(s),
    }
}

/// Values for `key:` in a selector, from the snapshot or a fixed set.
fn selector_values(key: &str, snap: Option<&MuxSnapshot>) -> Vec<Candidate> {
    let fixed = |v: &[&str]| v.iter().map(|s| Candidate::new(*s, "")).collect();
    match key {
        "status" => fixed(STATES),
        "exited" | "focused" | "active" => fixed(&["true", "false"]),
        _ => {
            let Some(s) = snap else { return vec![] };
            match key {
                "id" => pane_ids(s),
                "tab" => tab_ids(s),
                "window" => window_ids(s),
                "workspace" => workspace_names(s)
                    .into_iter()
                    .map(|c| Candidate::new(c.replacement, ""))
                    .collect(),
                "domain" => domains(s),
                "title" => dedup(panes(s).map(|p| p.title.clone())),
                "cwd" => dedup(panes(s).filter_map(|p| p.cwd.clone())),
                "cmd" => dedup(panes(s).filter_map(|p| p.process.clone())),
                _ => vec![],
            }
        }
    }
}

/// Completes the clause being typed in a selector word. `cur` is the whole (unquoted) word.
fn selector_candidates(cur: &str, snap: Option<&MuxSnapshot>) -> Vec<Candidate> {
    let clause_at = cur.rfind(',').map_or(0, |i| i + 1);
    let (head, clause) = cur.split_at(clause_at);
    let (neg, body) = match clause.strip_prefix('!') {
        Some(b) => ("!", b),
        None => ("", clause),
    };
    match body.split_once(':') {
        None => SELECTOR_KEYS
            .iter()
            .filter(|(k, _)| k.starts_with(body))
            .map(|(k, d)| Candidate {
                replacement: format!("{head}{neg}{k}:"),
                display: format!("{k}:  {d}"),
            })
            .collect(),
        Some((key, value)) => {
            let value = value.strip_prefix("re:").unwrap_or(value);
            let re = if body.contains(":re:") { "re:" } else { "" };
            selector_values(key, snap)
                .into_iter()
                .filter(|c| c.replacement.starts_with(value))
                .map(|c| Candidate {
                    replacement: format!("{head}{neg}{key}:{re}{}", c.replacement),
                    display: c.display,
                })
                .collect()
        }
    }
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{home}{rest}")
        }
        _ => path.to_string(),
    }
}

fn path_candidates(prefix: &str) -> Vec<Candidate> {
    let (dir_text, name) = match prefix.rfind('/') {
        Some(i) => (&prefix[..=i], &prefix[i + 1..]),
        None => ("", prefix),
    };
    let dir = if dir_text.is_empty() {
        ".".to_string()
    } else {
        expand_home(dir_text)
    };
    let Ok(read) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<Candidate> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            if !file.starts_with(name) || (file.starts_with('.') && !name.starts_with('.')) {
                return None;
            }
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            Some(Candidate::new(
                format!("{dir_text}{file}{}", if is_dir { "/" } else { "" }),
                "",
            ))
        })
        .collect();
    out.sort_by(|a, b| a.replacement.cmp(&b.replacement));
    out.truncate(200);
    out
}

fn on_off() -> Vec<Candidate> {
    ["on", "off"]
        .iter()
        .map(|s| Candidate::new(*s, ""))
        .collect()
}

fn command_names() -> Vec<Candidate> {
    let mut v = subcommand_candidates(ctl_command());
    v.retain(|c| !matches!(c.replacement.as_str(), "mcp"));
    v
}

fn first_word_candidates(aliases: &Aliases) -> Vec<Candidate> {
    let mut out = command_names();
    out.extend(META_WORDS.iter().map(|(w, d)| Candidate::new(*w, d)));
    let mut alias_names: Vec<(String, String)> = BUILTIN_ALIASES
        .iter()
        .map(|(n, v)| (n.to_string(), v.join(" ")))
        .collect();
    for (n, v) in &aliases.0 {
        alias_names.retain(|(x, _)| x != n);
        alias_names.push((n.clone(), v.join(" ")));
    }
    out.extend(
        alias_names
            .into_iter()
            .map(|(n, v)| Candidate::new(n, &format!("alias for {v}"))),
    );
    out.push(Candidate::new("ctl", "optional prefix"));
    out
}

fn meta_candidates(word: &str, args: &[String], cx: &CompCtx<'_>) -> (Vec<Candidate>, bool) {
    let snap = cx.snap;
    let kinds = |_: ()| -> Vec<Candidate> {
        ["pane", "tab", "window", "workspace", "--match", "-"]
            .iter()
            .map(|s| Candidate::new(*s, ""))
            .collect()
    };
    match (word, args.len()) {
        ("help", 0) => {
            let mut v = command_names();
            v.extend(META_WORDS.iter().map(|(w, d)| Candidate::new(*w, d)));
            (v, false)
        }
        ("use", 0) => (kinds(()), false),
        ("use", 1) => match args[0].as_str() {
            "pane" => (id_candidates(Kind::Pane, snap), false),
            "tab" => (id_candidates(Kind::Tab, snap), false),
            "window" => (id_candidates(Kind::Window, snap), false),
            "workspace" => (snap.map(workspace_names).unwrap_or_default(), false),
            "--match" => (vec![], true),
            _ => (vec![], false),
        },
        ("json" | "notify" | "pager", 0) => (on_off(), false),
        ("source", 0) => (vec![], false),
        _ => (vec![], false),
    }
}

/// What a bare argument of this verb is.
enum Positional {
    Selector(Kind),
    Workspace,
    SnippetId,
    RecordingId,
    Path,
    None,
}

fn positional_kind(argv: &[String], positionals_before: usize) -> Positional {
    if positionals_before == 0 {
        if let Some(v) = find_verb(argv) {
            return Positional::Selector(v.kind);
        }
        if let Some(p) = LIST_VERBS
            .iter()
            .find(|p| argv.len() >= p.len() && argv.iter().zip(p.iter()).all(|(a, b)| a == b))
        {
            return Positional::Selector(match p[0] {
                "tab" => Kind::Tab,
                "window" => Kind::Window,
                _ => Kind::Pane,
            });
        }
        let path: Vec<&str> = argv.iter().take(2).map(String::as_str).collect();
        match path.as_slice() {
            ["pane", "wait"] => return Positional::Selector(Kind::Pane),
            ["workspace", "create" | "switch" | "rename" | "delete"] => {
                return Positional::Workspace
            }
            ["replay", ..] => return Positional::Path,
            ["snippet", "show" | "expand" | "insert" | "rm"] => return Positional::SnippetId,
            ["recording", "show" | "stop" | "rename" | "rm" | "replay" | "export"] => {
                return Positional::RecordingId
            }
            _ => {}
        }
    }
    if matches!(argv.first().map(String::as_str), Some("replay")) && positionals_before == 0 {
        return Positional::Path;
    }
    Positional::None
}

/// Completes `line` at byte offset `pos`.
pub fn complete(line: &str, pos: usize, cx: &CompCtx<'_>) -> Completion {
    let upto = &line[..pos.min(line.len())];
    let Some((off, stmt)) = current_statement(upto) else {
        return Completion {
            start: pos,
            ..Completion::default()
        };
    };
    let p = split_partial(stmt);
    let start = off + p.cur_start;
    let cur = p.cur.as_str();
    let mut done = Completion {
        start,
        ..Completion::default()
    };
    if p.words.is_empty() {
        done.items = filter(first_word_candidates(cx.aliases), cur, false);
        return done;
    }
    let words = super::expand::expand_aliases(p.words.clone(), cx.aliases);
    if words.is_empty() {
        // Only `ctl` was typed.
        done.items = filter(command_names(), cur, false);
        return done;
    }
    if let Some(first) = words.first().filter(|w| super::session::is_meta(w)) {
        let (items, selector) = meta_candidates(first, &words[1..], cx);
        if first == "source" && words.len() == 1 {
            done.items = filter(path_candidates(cur), "", true);
        } else if selector {
            done.items = filter(selector_candidates(cur, cx.snap), "", true);
            done.selector = Some(cur.to_string());
        } else {
            done.items = filter(items, cur, true);
        }
        return done;
    }

    // Walk the command tree as far as the words go; what is left are flags and positionals.
    let mut cmd = ctl_command();
    let mut after: Vec<&String> = Vec::new();
    let mut positionals_before = 0;
    let mut skip_value = false;
    for w in &words {
        if skip_value {
            skip_value = false;
            after.push(w);
            continue;
        }
        if w.starts_with('-') {
            skip_value = !w.contains('=')
                && find_flag(cmd, w)
                    .or_else(|| short_flag(cmd, w))
                    .is_some_and(arg_takes_value);
            after.push(w);
            continue;
        }
        match cmd.find_subcommand(w).filter(|s| s.get_name() != "help") {
            Some(sub) if positionals_before == 0 => cmd = sub,
            _ => {
                positionals_before += 1;
                after.push(w);
            }
        }
    }

    // A flag that takes a value: complete the value.
    if let Some(prev) = after.last() {
        let flag = find_flag(cmd, prev).or_else(|| short_flag(cmd, prev));
        if let Some(a) = flag.filter(|a| arg_takes_value(a)) {
            let long = a.get_long().unwrap_or("");
            let hint = a.get_value_hint();
            let possible: Vec<Candidate> = a
                .get_possible_values()
                .iter()
                .filter(|v| !v.is_hide_set())
                .map(|v| Candidate::new(v.get_name(), ""))
                .collect();
            match long {
                "pane" => done.items = filter(id_candidates(Kind::Pane, cx.snap), cur, true),
                "tab" => done.items = filter(id_candidates(Kind::Tab, cx.snap), cur, true),
                "window" => done.items = filter(id_candidates(Kind::Window, cx.snap), cur, true),
                "workspace" => {
                    done.items = filter(cx.snap.map(workspace_names).unwrap_or_default(), cur, true)
                }
                "domain" => {
                    done.items = filter(cx.snap.map(domains).unwrap_or_default(), cur, true)
                }
                "match" => {
                    done.items = filter(selector_candidates(cur, cx.snap), "", true);
                    done.selector = Some(cur.to_string());
                }
                // These parse through FromStr, so clap knows no possible values for them.
                "state" => {
                    let v = STATES.iter().map(|s| Candidate::new(*s, "")).collect();
                    done.items = filter(v, cur, true)
                }
                "format" => {
                    let v = mt_export::registry()
                        .iter()
                        .map(|e| Candidate::new(e.id(), e.label()))
                        .collect();
                    done.items = filter(v, cur, true)
                }
                "opt" => {
                    let chosen = after
                        .iter()
                        .position(|w| w.as_str() == "--format")
                        .and_then(|i| after.get(i + 1))
                        .map(|w| w.as_str());
                    let v = mt_export::registry()
                        .iter()
                        .filter(|e| chosen.is_none_or(|c| c == e.id()))
                        .flat_map(|e| e.options())
                        .map(|o| Candidate::new(format!("{}=", o.key), o.help))
                        .collect();
                    done.items = filter(v, cur, true)
                }
                "level" => {
                    let v = ["info", "attention", "error"]
                        .iter()
                        .map(|s| Candidate::new(*s, ""))
                        .collect();
                    done.items = filter(v, cur, true)
                }
                _ if !possible.is_empty() => done.items = filter(possible, cur, true),
                _ if matches!(
                    hint,
                    ValueHint::AnyPath | ValueHint::FilePath | ValueHint::DirPath
                ) || matches!(long, "cwd" | "output" | "file")
                    || a.get_short() == Some('o') =>
                {
                    done.items = filter(path_candidates(cur), "", true)
                }
                _ => {}
            }
            return done;
        }
    }

    if cur.starts_with('-') {
        done.items = filter(flag_candidates(cmd), cur, false);
        return done;
    }
    if cmd.has_subcommands() && positionals_before == 0 {
        done.items = filter(subcommand_candidates(cmd), cur, false);
        return done;
    }
    match positional_kind(&words, positionals_before) {
        Positional::Selector(kind) => {
            let mut items = filter(id_candidates(kind, cx.snap), cur, true);
            items.extend(filter(selector_candidates(cur, cx.snap), "", true));
            if cur.contains(':') || cur.contains(',') || cur.starts_with('!') {
                done.selector = Some(cur.to_string());
            }
            done.items = items;
        }
        Positional::Workspace => {
            done.items = filter(cx.snap.map(workspace_names).unwrap_or_default(), cur, true)
        }
        Positional::SnippetId => {
            done.items = filter(
                cx.snippets
                    .iter()
                    .map(|id| Candidate::new(id.clone(), ""))
                    .collect(),
                cur,
                true,
            )
        }
        Positional::RecordingId => {
            done.items = filter(
                cx.recordings
                    .iter()
                    .map(|id| Candidate::new(id.clone(), ""))
                    .collect(),
                cur,
                true,
            )
        }
        Positional::Path => done.items = filter(path_candidates(cur), "", true),
        Positional::None => {}
    }
    done
}

/// The hint for a selector being typed: `matches N panes`, or the parse error with its column.
pub fn selector_hint(word: &str, snap: Option<&MuxSnapshot>) -> Option<String> {
    if word.is_empty() {
        return None;
    }
    // A last clause without `:` is still being typed (a key prefix); judge what came before it.
    let word = match word.rsplit_once(',') {
        Some((head, last)) if !last.contains(':') => head,
        Some(_) => word,
        None if !word.contains(':') => return None,
        None => word,
    };
    if word.is_empty() {
        return None;
    }
    // Locate the first clause that does not parse, for the error column.
    let mut offset = 0;
    for clause in word.split(',') {
        if let Err(e) = Selector::parse(clause) {
            if !clause.is_empty() {
                return Some(format!("✖ column {}: {}", offset + 1, e.message));
            }
        }
        offset += clause.len() + 1;
    }
    let sel = match Selector::parse(word) {
        Ok(s) => s,
        Err(e) => return Some(format!("✖ {}", e.message)),
    };
    if let Err(e) = sel.check(Resource::Pane) {
        return Some(format!("✖ {}", e.message));
    }
    let snap = snap?;
    let mut n = 0;
    for ws in &snap.workspaces {
        for w in &ws.windows {
            for t in &w.tabs {
                for p in &t.panes {
                    let row = PaneRow {
                        id: p.id,
                        tab: p.tab,
                        window: p.window,
                        workspace: p.workspace.clone(),
                        domain: p.domain.clone(),
                        title: p.title.clone(),
                        cwd: p.cwd.clone(),
                        process: p.process.clone(),
                        size: (p.size.cols, p.size.rows),
                        focused: p.focused,
                        active: t.active,
                        zoomed: p.zoomed,
                        exited: p.exited,
                        exit_code: p.exit_code,
                        status: p.status.clone(),
                        color: t.color.clone(),
                    };
                    if selector_matches(&sel, &row) {
                        n += 1;
                    }
                }
            }
        }
    }
    Some(format!("matches {n} pane{}", if n == 1 { "" } else { "s" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> MuxSnapshot {
        serde_json::from_value(serde_json::json!({
            "workspaces": [{
                "id": 1, "name": "default", "active": true,
                "windows": [{
                    "id": 1, "title": "w", "active": true, "focused": true,
                    "tabs": [{
                        "id": 3, "window": 1, "title": "build", "active": true,
                        "focused": true, "domain": "local",
                        "panes": [
                            {"id": 5, "tab": 3, "window": 1, "workspace": "default",
                             "domain": "local", "title": "cargo watch", "cwd": "/src/app",
                             "size": {"cols": 80, "rows": 24}, "focused": true, "exited": false},
                            {"id": 6, "tab": 3, "window": 1, "workspace": "default",
                             "domain": "ssh:prod", "title": "zsh", "cwd": null,
                             "size": {"cols": 80, "rows": 24}, "focused": false,
                             "exited": true, "exit_code": 1}
                        ]
                    }]
                }]
            }, {
                "id": 2, "name": "infra", "active": false, "windows": []
            }]
        }))
        .expect("fixture matches the protocol types")
    }

    fn run(line: &str, snap: Option<&MuxSnapshot>) -> Vec<String> {
        let aliases = Aliases::default();
        let cx = CompCtx {
            snap,
            aliases: &aliases,
            snippets: &[],
            recordings: &[],
        };
        complete(line, line.len(), &cx)
            .items
            .into_iter()
            .map(|c| c.replacement)
            .collect()
    }

    fn has(items: &[String], what: &str) -> bool {
        items.iter().any(|i| i == what)
    }

    #[test]
    fn first_word_offers_resources_aliases_and_meta() {
        let items = run("", None);
        for w in ["pane", "tab", "list", "use", "help", "ls", "type", "ctl"] {
            assert!(has(&items, w), "{w} in {items:?}");
        }
        assert!(!has(&items, "mcp"), "mcp does not run inside sh");
        assert_eq!(run("wo", None), ["workspace"]);
    }

    #[test]
    fn verbs_come_from_the_clap_tree() {
        let items = run("pane ", None);
        for v in [
            "list",
            "get",
            "get-text",
            "send-text",
            "run",
            "wait",
            "notify",
            "status",
            "close",
        ] {
            assert!(has(&items, v), "{v} in {items:?}");
        }
        assert_eq!(run("pane st", None), ["status"]);
        assert!(has(&run("pane status ", None), "set"));
        assert!(has(&run("p stat", None), "status"), "aliases expand first");
        assert!(has(&run("ctl tab ", None), "rename"));
    }

    #[test]
    fn flags_come_from_the_clap_tree() {
        let items = run("tab new --", None);
        for f in [
            "--title",
            "--cwd",
            "--domain",
            "--window",
            "--workspace",
            "--json",
        ] {
            assert!(has(&items, f), "{f} in {items:?}");
        }
        assert_eq!(run("pane get --pa", None), ["--pane"]);
    }

    #[test]
    fn ids_complete_with_title_and_cwd_descriptions() {
        let snap = snapshot();
        let aliases = Aliases::default();
        let cx = CompCtx {
            snap: Some(&snap),
            aliases: &aliases,
            snippets: &[],
            recordings: &[],
        };
        let c = complete("pane get --pane ", 16, &cx);
        assert_eq!(c.start, 16);
        let five = c
            .items
            .iter()
            .find(|c| c.replacement == "5")
            .expect("pane 5");
        assert_eq!(five.display, "5  cargo watch  /src/app");
        assert!(
            has(&run("pane get ", Some(&snap)), "6"),
            "positional shorthand"
        );
        assert_eq!(run("pane get --pane 6", Some(&snap)), ["6"]);
    }

    #[test]
    fn tabs_windows_workspaces_and_domains() {
        let snap = snapshot();
        let s = Some(&snap);
        assert_eq!(run("tab focus --tab ", s), ["3"]);
        assert_eq!(run("window focus --window ", s), ["1"]);
        assert_eq!(run("workspace switch ", s), ["default", "infra"]);
        assert_eq!(run("tab new --workspace i", s), ["infra"]);
        assert_eq!(run("tab new --domain ", s), ["local", "ssh:prod"]);
    }

    #[test]
    fn flag_values_use_possible_values() {
        let items = run("pane status set --state ", None);
        for s in STATES {
            assert!(has(&items, s), "{s} in {items:?}");
        }
        assert!(has(&run("pane notify --level ", None), "attention"));
    }

    #[test]
    fn selector_keys_values_and_negation() {
        let snap = snapshot();
        let s = Some(&snap);
        let keys = run("pane list --match ", s);
        assert!(has(&keys, "title:") && has(&keys, "exited:"), "{keys:?}");
        assert_eq!(run("pane list --match sta", s), ["status:"]);
        assert!(has(&run("pane list --match !ex", s), "!exited:"));
        let st = run("pane list --match status:w", s);
        assert_eq!(st, ["status:working", "status:waiting"]);
        assert_eq!(run("pane list --match exited:t", s), ["exited:true"]);
        // After a comma the next clause is completed and the head is kept.
        assert_eq!(
            run("pane list --match exited:false,sta", s),
            ["exited:false,status:"]
        );
        // Values come from the snapshot, quoted when needed.
        assert_eq!(run("pane list --match title:c", s), ["'title:cargo watch'"]);
        assert_eq!(run("pane list --match domain:s", s), ["domain:ssh:prod"]);
    }

    #[test]
    fn bare_selectors_complete_like_match_values() {
        let snap = snapshot();
        let items = run("pane list sta", Some(&snap));
        assert!(has(&items, "status:"), "{items:?}");
        let c = {
            let aliases = Aliases::default();
            let cx = CompCtx {
                snap: Some(&snap),
                aliases: &aliases,
                snippets: &[],
                recordings: &[],
            };
            complete("pane list status:wor", 20, &cx)
        };
        assert_eq!(c.selector.as_deref(), Some("status:wor"));
    }

    #[test]
    fn without_a_snapshot_only_static_candidates_remain() {
        assert!(run("pane get --pane ", None).is_empty());
        assert!(has(&run("pane ", None), "list"));
        assert!(has(&run("pane list --match ", None), "title:"));
        assert!(run("pane list --match status:", None).len() == 5);
        assert!(run("pane list --match title:", None).is_empty());
    }

    #[test]
    fn meta_commands_complete() {
        let snap = snapshot();
        let s = Some(&snap);
        assert!(has(&run("use ", s), "pane"));
        assert_eq!(run("use pane ", s), ["5", "6"]);
        assert_eq!(run("use workspace ", s), ["default", "infra"]);
        assert!(has(&run("use --match ", s), "title:"));
        assert_eq!(run("json o", None), ["on", "off"]);
        assert!(has(&run("help ", None), "pane"));
    }

    #[test]
    fn completion_works_inside_the_current_statement_only() {
        assert_eq!(run("list; wo", None), ["workspace"]);
        assert_eq!(run("list && pane st", None), ["status"]);
        assert!(
            run("pane get-text 5 | gr", None).is_empty(),
            "host commands are not completed"
        );
        assert!(run("!ls pa", None).is_empty());
        // A quote that is still open still completes, and the replacement closes it.
        let snap = snapshot();
        assert_eq!(
            run("pane list --match 'title:ca", Some(&snap)),
            ["'title:cargo watch'"]
        );
    }

    #[test]
    fn paths_complete_for_cwd_and_replay() {
        let dir = std::env::temp_dir().join(format!("mt-sh-complete-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("session.cast"), "").unwrap();
        let base = dir.display().to_string();
        let items = run(&format!("replay {base}/se"), None);
        assert_eq!(items, [format!("{base}/session.cast")]);
        let items = run(&format!("tab new --cwd {base}/s"), None);
        assert!(has(&items, &format!("{base}/sub/")), "{items:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn user_aliases_are_offered_and_expanded() {
        let mut aliases = Aliases::default();
        aliases
            .0
            .insert("ll".into(), vec!["pane".into(), "list".into()]);
        let cx = CompCtx {
            snap: None,
            aliases: &aliases,
            snippets: &[],
            recordings: &[],
        };
        let c = complete("l", 1, &cx);
        assert!(c.items.iter().any(|i| i.replacement == "ll"));
        let c = complete("ll --j", 6, &cx);
        assert!(c.items.iter().any(|i| i.replacement == "--json"));
    }

    #[test]
    fn selector_hint_counts_matches_and_reports_errors() {
        let snap = snapshot();
        let s = Some(&snap);
        assert_eq!(
            selector_hint("title:cargo*", s).as_deref(),
            Some("matches 1 pane")
        );
        assert_eq!(
            selector_hint("!exited:true", s).as_deref(),
            Some("matches 1 pane")
        );
        assert_eq!(
            selector_hint("domain:*", s).as_deref(),
            Some("matches 2 panes")
        );
        assert_eq!(
            selector_hint("title:nothing", s).as_deref(),
            Some("matches 0 panes")
        );
        let err = selector_hint("exited:true,bogus:1", s).unwrap();
        assert!(err.starts_with("✖ column 13"), "{err}");
        assert!(selector_hint("title:x", None).is_none());
        assert!(selector_hint("", s).is_none());
        // A key prefix still being typed is not an error, and does not change the count.
        assert!(selector_hint("sta", s).is_none());
        assert_eq!(
            selector_hint("exited:true,sta", s).as_deref(),
            Some("matches 1 pane")
        );
    }

    #[test]
    fn recording_export_completes_ids_formats_and_option_keys() {
        let ids = vec!["r_20260101_000000_aaaaaa".to_string()];
        let aliases = Aliases::default();
        let cx = CompCtx {
            snap: None,
            aliases: &aliases,
            snippets: &[],
            recordings: &ids,
        };
        let c = complete("recording export r_2", 20, &cx);
        assert!(c.items[0].replacement.starts_with(&ids[0]));
        let c = complete("recording export x --format v", 29, &cx);
        assert!(c.items[0].replacement.starts_with("vhs"));
        let c = complete("recording export x --format vhs --opt type", 42, &cx);
        assert!(c.items[0].replacement.starts_with("type_gap_ms="));
        let c = complete("recording export x --format cast --opt type", 43, &cx);
        assert!(c.items.is_empty());
    }

    #[test]
    fn quoting_round_trips_through_the_lexer() {
        for raw in ["plain", "with space", "it's", "a;b", "title:x*", "~/x", ""] {
            let q = quote_arg(raw);
            let home = |n: &str| (n == "HOME").then(|| "/home/me".to_string());
            let parsed = super::super::lexer::parse(&format!("w {q}"), &home)
                .unwrap()
                .remove(0)
                .words;
            let want = if raw == "~/x" {
                "/home/me/x".to_string()
            } else {
                raw.to_string()
            };
            assert_eq!(parsed[1], want, "{raw:?} → {q}");
        }
    }

    #[test]
    fn snippet_ids_complete_after_the_verbs_that_take_one() {
        let aliases = Aliases::default();
        let ids = vec!["k8s-logs".to_string(), "git-undo".to_string()];
        let cx = CompCtx {
            snap: None,
            aliases: &aliases,
            snippets: &ids,
            recordings: &[],
        };
        for verb in ["show", "expand", "insert", "rm"] {
            let line = format!("snippet {verb} k8");
            let c = complete(&line, line.len(), &cx);
            let got: Vec<_> = c.items.iter().map(|i| i.replacement.as_str()).collect();
            assert_eq!(got, ["k8s-logs"], "{verb}");
        }
        let c = complete("snippet list k", 14, &cx);
        assert!(c.items.is_empty(), "list takes no id");
    }
}
