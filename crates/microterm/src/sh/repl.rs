//! The interactive loop: a rustyline editor with completion, hints, highlighting and history over
//! the [`Engine`].

use std::borrow::Cow;
use std::sync::Arc;

use rustyline::completion::{Completer, Pair};
use rustyline::config::CompletionType;
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::FileHistory;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Config, Context, Editor, Helper};

use super::complete::{complete, selector_hint, CompCtx, SELECTOR_KEYS};
use super::engine::{Engine, Flow, Io};
use super::expand::Aliases;
use super::history::{history_path, prepare};
use super::lexer::{self, LexError};
use super::live::Shared;
use super::session::{history_allows, is_meta};
use crate::render::RenderOpts;

const GREEN: &str = "1;32";
const RED: &str = "31";
const BLUE: &str = "34";
const CYAN: &str = "36";
const YELLOW: &str = "33";
const DIM: &str = "2";

fn paint(s: &str, code: &str) -> String {
    format!("\x1b[{code}m{s}\x1b[0m")
}

struct ShHelper {
    shared: Arc<Shared>,
    aliases: Aliases,
    hinter: HistoryHinter,
    color: bool,
}

impl ShHelper {
    fn comp(&self, line: &str, pos: usize) -> super::complete::Completion {
        let snap = self.shared.snapshot();
        let snippets = self.shared.snippet_ids();
        let recordings = self.shared.recording_ids();
        complete(
            line,
            pos,
            &CompCtx {
                snap: snap.as_ref(),
                aliases: &self.aliases,
                snippets: &snippets,
                recordings: &recordings,
            },
        )
    }
}

impl Completer for ShHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let c = self.comp(line, pos);
        let items = c
            .items
            .into_iter()
            .map(|i| {
                // A finished word gets a space; a key, a directory or a clause does not.
                let more = [':', '/', ',']
                    .iter()
                    .any(|ch| i.replacement.ends_with(*ch));
                Pair {
                    display: i.display,
                    replacement: if more {
                        i.replacement
                    } else {
                        format!("{} ", i.replacement)
                    },
                }
            })
            .collect();
        Ok((c.start, items))
    }
}

impl Hinter for ShHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        if pos < line.len() {
            return None;
        }
        if let Some(sel) = self.comp(line, pos).selector {
            let snap = self.shared.snapshot();
            return selector_hint(&sel, snap.as_ref()).map(|h| format!("   {h}"));
        }
        self.hinter.hint(line, pos, ctx)
    }
}

/// Colours the line: a valid verb green, an unknown first word red, flags blue, selector keys
/// cyan and quoted strings yellow.
fn colorize(line: &str, aliases: &Aliases) -> String {
    let mut out = String::new();
    let mut words_in_stmt = 0usize;
    let mut first: Option<String> = None;
    let mut prev_word = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        if matches!(c, ';' | '>' | '|') {
            let two = (c == '&' || c == '|') && next == Some(c) || (c == '>' && next == Some('>'));
            let n = if two { 2 } else { 1 };
            out.extend(&chars[i..i + n]);
            i += n;
            if c == ';' || two && c == '|' {
                words_in_stmt = 0;
                first = None;
            }
            continue;
        }
        if c == '&' && next == Some('&') {
            out.push_str("&&");
            i += 2;
            words_in_stmt = 0;
            first = None;
            continue;
        }
        // One word, quote-aware.
        let start = i;
        let (mut single, mut double) = (false, false);
        while i < chars.len() {
            let ch = chars[i];
            if single {
                single = ch != '\'';
            } else if double {
                match ch {
                    '\\' => i += 1,
                    '"' => double = false,
                    _ => {}
                }
            } else if ch.is_whitespace()
                || matches!(ch, ';' | '|' | '>')
                || (ch == '&' && chars.get(i + 1) == Some(&'&'))
            {
                break;
            } else if ch == '\'' {
                single = true;
            } else if ch == '"' {
                double = true;
            } else if ch == '\\' {
                i += 1;
            }
            i += 1;
        }
        let i_end = i.min(chars.len());
        let raw: String = chars[start..i_end].iter().collect();
        i = i_end;
        let plain: String = raw.chars().filter(|c| !matches!(c, '\'' | '"')).collect();
        let style = if raw.starts_with('!') && start == 0 {
            None
        } else if words_in_stmt == 0 && plain == "ctl" {
            Some(DIM)
        } else {
            word_style(
                &plain,
                &raw,
                &prev_word,
                &mut first,
                &mut words_in_stmt,
                aliases,
            )
        };
        match style {
            Some(code) if raw.contains(':') && code == CYAN => out.push_str(&selector_keys(&raw)),
            Some(code) => out.push_str(&paint(&raw, code)),
            None if prev_word == "--match" || raw.contains(':') => {
                out.push_str(&selector_keys(&raw))
            }
            None => out.push_str(&raw),
        }
        prev_word = plain;
    }
    out
}

fn word_style(
    plain: &str,
    raw: &str,
    prev: &str,
    first: &mut Option<String>,
    n: &mut usize,
    aliases: &Aliases,
) -> Option<&'static str> {
    let idx = *n;
    *n += 1;
    if plain.starts_with("--") {
        return Some(BLUE);
    }
    if idx == 0 {
        let known = is_meta(plain)
            || aliases.get(plain).is_some()
            || crate::ctl::CtlArgs::command_names()
                .iter()
                .any(|c| c == plain);
        *first = Some(plain.to_string());
        return Some(if known { GREEN } else { RED });
    }
    let resource = first.as_ref().and_then(|f| {
        let exp = aliases.get(f).unwrap_or_else(|| vec![f.clone()]);
        (exp.len() == 1).then(|| exp[0].clone())
    });
    if idx == 1 && !plain.starts_with('-') {
        if let Some(r) = resource {
            let subs = crate::ctl::CtlArgs::subcommand_names(&r);
            if !subs.is_empty() {
                return Some(if subs.iter().any(|s| s == plain) {
                    GREEN
                } else {
                    RED
                });
            }
        }
    }
    if prev == "--match" {
        return Some(CYAN);
    }
    if raw.starts_with('\'') || raw.starts_with('"') {
        return Some(if raw.contains(':') { CYAN } else { YELLOW });
    }
    if raw.contains(':') {
        return Some(CYAN);
    }
    None
}

/// Selector keys in `raw` cyan, the rest unchanged.
fn selector_keys(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut at_clause = true;
    while i < chars.len() {
        let c = chars[i];
        if at_clause && c.is_ascii_alphabetic() {
            let end = chars[i..]
                .iter()
                .position(|ch| !ch.is_ascii_alphabetic())
                .map_or(chars.len(), |p| i + p);
            let key: String = chars[i..end].iter().collect();
            if chars.get(end) == Some(&':') && SELECTOR_KEYS.iter().any(|(k, _)| *k == key) {
                out.push_str(&paint(&key, CYAN));
                i = end;
                at_clause = false;
                continue;
            }
        }
        at_clause = matches!(c, ',' | '!' | '\'' | '"');
        out.push(c);
        i += 1;
    }
    out
}

impl Highlighter for ShHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if self.color {
            Cow::Owned(colorize(line, &self.aliases))
        } else {
            Cow::Borrowed(line)
        }
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        if !self.color {
            return Cow::Borrowed(hint);
        }
        let code = if hint.contains('✖') { RED } else { DIM };
        Cow::Owned(paint(hint, code))
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        if !self.color {
            return Cow::Borrowed(prompt);
        }
        let code = if prompt.starts_with('⚠') {
            YELLOW
        } else {
            "1;36"
        };
        Cow::Owned(paint(prompt, code))
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        self.color
    }
}

impl Validator for ShHelper {
    fn validate(&self, ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        Ok(match lexer::parse(ctx.input(), &|_| None) {
            Err(LexError::Incomplete(_)) => ValidationResult::Incomplete,
            _ => ValidationResult::Valid(None),
        })
    }
}

impl Helper for ShHelper {}

/// `microterm[pane 7]> `, with `⚠ ` in front while no instance is reachable.
fn prompt(engine: &Engine, unicode: bool) -> String {
    let warn = if engine.is_connected() {
        ""
    } else if unicode {
        "⚠ "
    } else {
        "! "
    };
    let ctx = if engine.session.sticky.is_empty() {
        String::new()
    } else {
        format!("[{}]", engine.session.sticky.describe().replace(", ", " "))
    };
    format!("{warn}microterm{ctx}> ")
}

pub fn run(
    mut engine: Engine,
    shared: Arc<Shared>,
    render: RenderOpts,
    use_history: bool,
    history_send_text: bool,
) -> i32 {
    let config = Config::builder()
        .history_ignore_space(true)
        .completion_type(CompletionType::List)
        .auto_add_history(false)
        .history_ignore_dups(true)
        .and_then(|b| b.max_history_size(1000))
        .map(|b| b.build())
        .unwrap_or_default();
    let mut rl: Editor<ShHelper, FileHistory> = match Editor::with_config(config) {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("microterm: cannot start the line editor: {e}");
            return mt_proto::exit::USAGE;
        }
    };
    rl.set_helper(Some(ShHelper {
        shared: shared.clone(),
        aliases: Aliases::default(),
        hinter: HistoryHinter::new(),
        color: render.color,
    }));
    let history = use_history.then(history_path).flatten();
    if let Some(path) = &history {
        if prepare(path).is_ok() {
            let _ = rl.load_history(path);
        }
    }
    shared.spawn_listener();
    println!(
        "microterm sh {} · help for commands · Ctrl-D to exit",
        env!("CARGO_PKG_VERSION")
    );
    let mut code = 0;
    loop {
        for line in shared.take_notices() {
            println!(
                "{}",
                if render.color {
                    paint(&line, DIM)
                } else {
                    line
                }
            );
        }
        match rl.readline(&prompt(&engine, render.unicode)) {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                if history_allows(&line, &engine.session.aliases, history_send_text) {
                    let _ = rl.add_history_entry(line.as_str());
                    engine.history.push(line.clone());
                }
                let flow = {
                    let mut out = std::io::stdout();
                    let mut err = std::io::stderr();
                    let mut io = Io {
                        out: &mut out,
                        err: &mut err,
                        render,
                        rich: true,
                    };
                    engine.run_line(&line, &mut io)
                };
                if let Some(h) = rl.helper_mut() {
                    h.aliases = engine.session.aliases.clone();
                }
                if let Flow::Exit(c) = flow {
                    code = c;
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => {}
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("microterm: {e}");
                code = mt_proto::exit::USAGE;
                break;
            }
        }
    }
    if let Some(path) = &history {
        let _ = rl.save_history(path);
        let _ = prepare(path);
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_verbs_are_green_and_unknown_words_red() {
        let a = Aliases::default();
        let good = colorize("pane list", &a);
        assert!(good.contains("\x1b[1;32mpane\x1b[0m"), "{good:?}");
        assert!(good.contains("\x1b[1;32mlist\x1b[0m"), "{good:?}");
        let bad = colorize("frob", &a);
        assert!(bad.contains("\x1b[31mfrob"), "{bad:?}");
        let bad_verb = colorize("pane frob", &a);
        assert!(bad_verb.contains("\x1b[31mfrob"), "{bad_verb:?}");
        assert!(colorize("use pane 7", &a).contains("\x1b[1;32muse"));
    }

    #[test]
    fn selector_keys_flags_and_strings_are_styled() {
        let a = Aliases::default();
        let s = colorize("pane list --match 'title:x,!exited:true'", &a);
        assert!(s.contains("\x1b[34m--match"), "{s:?}");
        assert!(s.contains("\x1b[36mtitle\x1b[0m"), "{s:?}");
        assert!(s.contains("\x1b[36mexited\x1b[0m"), "{s:?}");
        let s = colorize("tab rename 3 'a b'", &a);
        assert!(s.contains("\x1b[33m'a b'"), "{s:?}");
        // Text is preserved exactly.
        let plain = "pane list --match 'title:x'";
        let stripped: String = {
            let mut out = String::new();
            let colored = colorize(plain, &a);
            let mut it = colored.chars().peekable();
            while let Some(c) = it.next() {
                if c == '\x1b' {
                    for n in it.by_ref() {
                        if n == 'm' {
                            break;
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            out
        };
        assert_eq!(stripped, plain);
    }

    #[test]
    fn statements_restart_the_word_count() {
        let a = Aliases::default();
        let s = colorize("list; pane list", &a);
        assert!(s.contains("\x1b[1;32mpane"), "{s:?}");
    }
}
