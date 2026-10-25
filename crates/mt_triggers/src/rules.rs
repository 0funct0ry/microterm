//! `triggers.toml` loading and validation with `triggers.toml:LINE:COL` diagnostics (§10.11.2).

use std::collections::HashSet;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use toml_edit::{ImDocument, Item};

use crate::model::*;
use crate::template;

pub const FILE_NAME: &str = "triggers.toml";
pub const FILE_VERSION: i64 = 1;
pub const DEFAULT_MAX_RULES: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct LoadOpts {
    pub max_rules: usize,
}

impl Default for LoadOpts {
    fn default() -> Self {
        Self {
            max_rules: DEFAULT_MAX_RULES,
        }
    }
}

/// Where in a rule a validation problem sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Rule,
    Id,
    Name,
    Panes,
    Cooldown,
    Scope,
    When,
    Regex,
    Event,
    Exit,
    Actions,
    /// Action index and field name.
    Action(usize, &'static str),
}

/// A validation problem of one rule, without a file position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub at: Field,
    pub message: String,
}

fn issue(at: Field, message: impl Into<String>) -> Issue {
    Issue {
        at,
        message: message.into(),
    }
}

/// A problem in `triggers.toml`; `line` and `col` are 1-based (`line == 0`: no position).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl Diagnostic {
    pub fn render(&self, file: &str) -> String {
        if self.line == 0 {
            format!("{file}: {}", self.message)
        } else {
            format!("{file}:{}:{}: {}", self.line, self.col, self.message)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    version: Option<i64>,
    #[serde(default)]
    trigger: Vec<Rule>,
}

fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(src.len());
    let before = &src.as_bytes()[..offset];
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    (line, src[start..offset].chars().count() + 1)
}

/// Compiles a rule regex with the §10.11.2 limits. Errors name lookaround and backreferences.
pub fn compile(regex: &str, case_insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(regex)
        .case_insensitive(case_insensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .map_err(|e| {
            let text = e.to_string();
            let lower = text.to_lowercase();
            if lower.contains("look-around") || lower.contains("look-ahead") {
                "lookahead and lookbehind are not supported (the regex engine guarantees linear \
                 time, so it has no backtracking features)"
                    .to_string()
            } else if lower.contains("backreference") {
                "backreferences are not supported (the regex engine guarantees linear time, so \
                 it has no backtracking features)"
                    .to_string()
            } else if lower.contains("size limit") || lower.contains("too big") {
                "regex is too complex (compiled size limit exceeded)".to_string()
            } else {
                // Keep only the final "error: …" line of the multi-line message.
                let last = text
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or(&text)
                    .trim();
                format!("invalid regex: {}", last.trim_start_matches("error: "))
            }
        })
}

/// Placeholder names available to the templates of `rule` (for validation and the editor).
pub fn available_placeholders(rule: &Rule) -> Vec<String> {
    let mut v: Vec<String> = ["rule", "pane", "title", "cwd", "domain"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    match &rule.when {
        When::Text {
            regex,
            case_insensitive,
        } => {
            v.push("line".into());
            v.push("match".into());
            if let Ok(re) = compile(regex, *case_insensitive) {
                for i in 1..re.captures_len().min(10) {
                    v.push(i.to_string());
                }
                for n in re.capture_names().flatten() {
                    v.push(n.to_string());
                }
            }
        }
        When::Event { .. } => {
            v.push("event".into());
            v.push("exit".into());
        }
    }
    v
}

/// Validates one rule. Pure: the same checks run on load and live in the editor.
pub fn validate_rule(rule: &Rule) -> Vec<Issue> {
    let mut out = Vec::new();
    if rule.name.trim().is_empty() {
        out.push(issue(Field::Name, "name is required"));
    } else if rule.name.chars().count() > MAX_NAME_CHARS {
        out.push(issue(
            Field::Name,
            format!("name is longer than {MAX_NAME_CHARS} characters"),
        ));
    }
    if rule.panes != "*" {
        match mt_proto::selector::Selector::parse(&rule.panes) {
            Ok(s) => {
                if let Err(e) = s.check(mt_proto::selector::Resource::Pane) {
                    out.push(issue(Field::Panes, e.message));
                }
            }
            Err(e) => out.push(issue(Field::Panes, e.message)),
        }
    }
    if rule.cooldown_ms > 60_000 {
        out.push(issue(
            Field::Cooldown,
            "cooldown_ms must be between 0 and 60000",
        ));
    }
    if rule.scope != MatchIn::Output && !rule.is_text() {
        out.push(issue(Field::Scope, "scope only applies to text rules"));
    }
    match &rule.when {
        When::Text {
            regex,
            case_insensitive,
        } => {
            if regex.is_empty() {
                out.push(issue(Field::Regex, "regex must not be empty"));
            } else if regex.chars().count() > MAX_REGEX_CHARS {
                out.push(issue(
                    Field::Regex,
                    format!("regex is longer than {MAX_REGEX_CHARS} characters"),
                ));
            } else if let Err(e) = compile(regex, *case_insensitive) {
                out.push(issue(Field::Regex, e));
            }
        }
        When::Event { event, exit } => {
            if !event.has_exit() && *exit != ExitFilter::Any {
                out.push(issue(
                    Field::Exit,
                    "exit only applies to command_finished and pane_exited",
                ));
            }
        }
    }
    if rule.actions.is_empty() {
        out.push(issue(Field::Actions, "a rule needs at least one action"));
    }
    if rule.actions.len() > MAX_ACTIONS {
        out.push(issue(
            Field::Actions,
            format!("at most {MAX_ACTIONS} actions per rule"),
        ));
    }
    let avail = available_placeholders(rule);
    let (ncaps, names) = match &rule.when {
        When::Text {
            regex,
            case_insensitive,
        } => compile(regex, *case_insensitive)
            .map(|re| {
                (
                    re.captures_len() - 1,
                    re.capture_names()
                        .flatten()
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or((0, Vec::new())),
        When::Event { .. } => (0, Vec::new()),
    };
    let mut seen: HashSet<&'static str> = HashSet::new();
    for (i, a) in rule.actions.iter().enumerate() {
        if a.is_singleton() && !seen.insert(a.kind()) {
            out.push(issue(
                Field::Action(i, "kind"),
                format!("a rule may have only one {} action", a.kind()),
            ));
        }
        for (field, text) in a.templates() {
            if text.chars().count() > MAX_TEMPLATE_CHARS {
                out.push(issue(
                    Field::Action(i, field),
                    format!("template is longer than {MAX_TEMPLATE_CHARS} characters"),
                ));
                continue;
            }
            if let Err(e) = template::parse(text) {
                out.push(issue(Field::Action(i, field), e.to_string()));
                continue;
            }
            for name in template::placeholders(text) {
                if !avail.contains(&name) {
                    out.push(issue(
                        Field::Action(i, field),
                        format!(
                            "unknown placeholder {{{name}}}; available: {}",
                            avail
                                .iter()
                                .map(|n| format!("{{{n}}}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        ),
                    ));
                }
            }
        }
        match a {
            Action::Highlight { scope, color, .. } => {
                if !valid_color(color) {
                    out.push(issue(Field::Action(i, "color"), bad_color(color)));
                }
                if *scope == Scope::Match && !rule.is_text() {
                    out.push(issue(
                        Field::Action(i, "scope"),
                        "scope = \"match\" needs a text rule; use \"line\" for event rules",
                    ));
                }
            }
            Action::TabColor { color, .. } => {
                if mt_proto::color::TabColor::parse(color).is_err() {
                    out.push(issue(Field::Action(i, "color"), bad_color(color)));
                }
            }
            Action::Marker { color, .. } if !color.is_empty() && !valid_color(color) => {
                out.push(issue(Field::Action(i, "color"), bad_color(color)));
            }
            Action::Snapshot { before, after } => {
                if *before > 50 || *after > 50 {
                    out.push(issue(
                        Field::Action(i, if *before > 50 { "before" } else { "after" }),
                        "before and after must be between 0 and 50",
                    ));
                }
            }
            Action::Screenshot { scale, .. } if !(0.5..=2.0).contains(scale) => {
                out.push(issue(
                    Field::Action(i, "scale"),
                    "scale must be between 0.5 and 2.0",
                ));
            }
            Action::Clipboard { source } if source != "match" => {
                let ok = match source.parse::<usize>() {
                    Ok(n) => (1..=9).contains(&n) && n <= ncaps,
                    Err(_) => names.iter().any(|n| n == source),
                };
                if !ok {
                    out.push(issue(
                        Field::Action(i, "source"),
                        format!(
                            "source {source:?} is not \"match\" or a capture the regex can produce"
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
    out
}

fn bad_color(c: &str) -> String {
    format!(
        "{c:?} is not a color: use accent, warning, error, success, info, one of blue purple \
         pink red orange yellow green teal gray, or #rrggbb"
    )
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Byte offset of the problem inside `doc`, falling back from field to entry to rule.
fn locate(doc: &ImDocument<&str>, rule: usize, at: Field) -> Option<usize> {
    let aot = doc.get("trigger")?.as_array_of_tables()?;
    let t = aot.get(rule)?;
    let rule_span = t.span().map(|s| s.start);
    let field = |key: &str| t.get(key).and_then(Item::span).map(|s| s.start);
    let when = |key: &str| {
        t.get("when")
            .and_then(Item::as_table_like)
            .and_then(|w| w.get(key))
            .and_then(Item::span)
            .map(|s| s.start)
            .or_else(|| field("when"))
    };
    let action = |j: usize, key: &str| -> Option<usize> {
        let a = t.get("actions")?;
        let entry = a.as_array_of_tables()?.get(j)?;
        entry
            .get(key)
            .and_then(Item::span)
            .or_else(|| entry.span())
            .map(|s| s.start)
    };
    match at {
        Field::Rule => rule_span,
        Field::Id => field("id"),
        Field::Name => field("name"),
        Field::Panes => field("panes"),
        Field::Cooldown => field("cooldown_ms"),
        Field::Scope => field("scope"),
        Field::When => field("when"),
        Field::Regex => when("regex"),
        Field::Event => when("event"),
        Field::Exit => when("exit"),
        Field::Actions => field("actions"),
        Field::Action(j, key) => action(j, key),
    }
    .or(rule_span)
}

/// Loads `src`. All-or-nothing: any problem fails the whole file (the caller keeps the last
/// valid rule set, §8.2).
pub fn load_str(src: &str, opts: LoadOpts) -> Result<Loaded, Vec<Diagnostic>> {
    let at = |off: Option<usize>, message: String| {
        let (line, col) = off.map_or((0, 0), |o| line_col(src, o));
        Diagnostic { message, line, col }
    };
    let doc = ImDocument::parse(src)
        .map_err(|e| vec![at(e.span().map(|s| s.start), e.message().to_string())])?;
    let raw: RawFile = toml::from_str(src).map_err(|e| {
        vec![at(
            e.span().map(|s| s.start),
            e.message().trim().to_string(),
        )]
    })?;
    let mut diags = Vec::new();
    if let Some(v) = raw.version {
        if v != FILE_VERSION {
            diags.push(at(
                doc.get("version").and_then(Item::span).map(|s| s.start),
                format!("unsupported version {v} (this build reads version {FILE_VERSION})"),
            ));
        }
    }
    let mut rules = raw.trigger;
    if rules.len() > opts.max_rules {
        diags.push(at(
            locate(&doc, opts.max_rules, Field::Rule),
            format!(
                "too many rules: at most {} (triggers.max_rules)",
                opts.max_rules
            ),
        ));
    }
    let mut used: HashSet<String> = rules
        .iter()
        .filter(|r| !r.id.is_empty())
        .map(|r| r.id.clone())
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    for (i, rule) in rules.iter_mut().enumerate() {
        for iss in validate_rule(rule) {
            diags.push(at(
                locate(&doc, i, iss.at),
                format!("trigger[{i}]: {}", iss.message),
            ));
        }
        if rule.id.is_empty() {
            rule.id = derived_id(&rule.name, &mut used);
        } else if !valid_id(&rule.id) {
            diags.push(at(
                locate(&doc, i, Field::Id),
                format!(
                    "trigger[{i}]: id {:?} must be 1-32 letters, digits, `_` or `-`",
                    rule.id
                ),
            ));
        } else if !seen.insert(rule.id.clone()) {
            diags.push(at(
                locate(&doc, i, Field::Id),
                format!("trigger[{i}]: duplicate id {:?}", rule.id),
            ));
        }
    }
    if diags.is_empty() {
        Ok(Loaded { rules })
    } else {
        Err(diags)
    }
}

pub fn load(path: &Path) -> Result<Loaded, Vec<Diagnostic>> {
    match std::fs::read_to_string(path) {
        Ok(src) => load_str(&src, LoadOpts::default()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded::default()),
        Err(e) => Err(vec![Diagnostic {
            message: e.to_string(),
            line: 0,
            col: 0,
        }]),
    }
}

/// A stable id for a hand-written rule without one: `t_` + 6 hex digits of the name.
fn derived_id(name: &str, used: &mut HashSet<String>) -> String {
    let mut h: u32 = 0x811c9dc5;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16777619);
    }
    let mut id = format!("t_{:06x}", h & 0xff_ffff);
    let mut n = 2;
    while !used.insert(id.clone()) {
        id = format!("t_{:06x}_{n}", h & 0xff_ffff);
        n += 1;
    }
    id
}

/// A fresh random id (`t_` + 6 hex digits) not in `used`.
pub fn new_id(used: &HashSet<String>) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    loop {
        let id = format!("t_{:06x}", rng.gen_range(0..0x100_0000u32));
        if !used.contains(&id) {
            return id;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
