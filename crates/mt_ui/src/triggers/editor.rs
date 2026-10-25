//! The rule editor's state (SPEC §10.11.5 "Editor"): fields, the actions' parameters, dirty
//! check, validation and the rule it would write. No GPUI.

use mt_triggers::rules::{validate_rule, Field};
use mt_triggers::store::Base;
use mt_triggers::*;

use crate::components::TextEdit;

/// The groups of the **Add action** menu (SPEC §10.11.5).
pub const ACTION_GROUPS: [(&str, &[&str]); 4] = [
    ("Visual", &["highlight", "tab_color", "counter", "title"]),
    ("Alerts", &["notify", "status", "bell"]),
    ("Navigate", &["marker", "findings"]),
    (
        "Capture",
        &["clipboard", "recording", "snapshot", "screenshot"],
    ),
];

pub const EVENTS: [EventKind; 5] = [
    EventKind::CommandFinished,
    EventKind::CwdChanged,
    EventKind::PaneExited,
    EventKind::TitleChanged,
    EventKind::Bell,
];

/// How one action parameter is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Free text; `template` fields offer the placeholder list.
    Text {
        template: bool,
    },
    /// Click cycles through the choices.
    Choice(&'static [&'static str]),
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub input: Input,
}

const fn text(key: &'static str, label: &'static str) -> Param {
    Param {
        key,
        label,
        input: Input::Text { template: false },
    }
}
const fn tpl(key: &'static str, label: &'static str) -> Param {
    Param {
        key,
        label,
        input: Input::Text { template: true },
    }
}
const fn choice(key: &'static str, label: &'static str, c: &'static [&'static str]) -> Param {
    Param {
        key,
        label,
        input: Input::Choice(c),
    }
}
const fn flag(key: &'static str, label: &'static str) -> Param {
    Param {
        key,
        label,
        input: Input::Bool,
    }
}

const P_HIGHLIGHT: &[Param] = &[
    choice("scope", "Scope", &["match", "line"]),
    text("color", "Color"),
    choice("style", "Style", &["background", "underline"]),
    text("ttl_s", "Expire after (s, 0 = never)"),
];
const P_NOTIFY: &[Param] = &[
    choice("level", "Level", &["info", "attention", "error"]),
    tpl("title", "Title"),
    tpl("message", "Message"),
    flag("desktop", "Desktop notification"),
];
const P_STATUS: &[Param] = &[
    choice(
        "state",
        "State",
        &["idle", "working", "waiting", "done", "error"],
    ),
    tpl("progress", "Progress (0-100 or template)"),
    tpl("text", "Text"),
    text("ttl_s", "Clear after (s, 0 = never)"),
];
const P_TAB_COLOR: &[Param] = &[
    text("color", "Color"),
    text("ttl_s", "Revert after (s, 0 = never)"),
    flag("clear_on_focus", "Clear when focused"),
];
const P_CLIPBOARD: &[Param] = &[text("source", "Copy (match, a number or a name)")];
const P_RECORDING: &[Param] = &[
    choice("op", "Operation", &["start", "stop"]),
    text("stop_after_s", "Stop after (s, 0 = until stopped)"),
];
const P_MARKER: &[Param] = &[tpl("label", "Label"), text("color", "Color")];
const P_COUNTER: &[Param] = &[choice(
    "reset_on",
    "Reset on",
    &["focus", "command_start", "never"],
)];
const P_TITLE: &[Param] = &[
    tpl("template", "Title"),
    text("ttl_s", "Revert after (s, 0 = never)"),
];
const P_FINDINGS: &[Param] = &[tpl("label", "Label (default: the line)")];
const P_SNAPSHOT: &[Param] = &[
    text("before", "Lines before (0-50)"),
    text("after", "Lines after (0-50)"),
];
const P_SCREENSHOT: &[Param] = &[
    text("scale", "Scale (0.5-2)"),
    choice("chrome", "Chrome", &["none", "plain"]),
];

/// The parameters of an action kind, in the order the editor shows them.
pub fn params(kind: &str) -> &'static [Param] {
    match kind {
        "highlight" => P_HIGHLIGHT,
        "notify" => P_NOTIFY,
        "status" => P_STATUS,
        "tab_color" => P_TAB_COLOR,
        "clipboard" => P_CLIPBOARD,
        "recording" => P_RECORDING,
        "marker" => P_MARKER,
        "counter" => P_COUNTER,
        "title" => P_TITLE,
        "findings" => P_FINDINGS,
        "snapshot" => P_SNAPSHOT,
        "screenshot" => P_SCREENSHOT,
        _ => &[],
    }
}

fn b(v: bool) -> String {
    v.to_string()
}

fn uint(s: &str, what: &str) -> Result<u32, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    s.parse::<u32>()
        .map_err(|_| format!("{what} must be a whole number"))
}

/// The values of `a`'s parameters as strings, in [`params`] order.
pub fn values_of(a: &Action) -> Vec<String> {
    match a {
        Action::Highlight {
            scope,
            color,
            style,
            ttl_s,
        } => vec![
            if *scope == Scope::Line {
                "line"
            } else {
                "match"
            }
            .into(),
            color.clone(),
            if *style == Style::Underline {
                "underline"
            } else {
                "background"
            }
            .into(),
            ttl_s.to_string(),
        ],
        Action::Notify {
            level,
            title,
            message,
            desktop,
        } => vec![
            match level {
                Level::Info => "info",
                Level::Attention => "attention",
                Level::Error => "error",
            }
            .into(),
            title.clone(),
            message.clone(),
            b(*desktop),
        ],
        Action::Status {
            state,
            progress,
            text,
            ttl_s,
        } => vec![
            match state {
                State::Idle => "idle",
                State::Working => "working",
                State::Waiting => "waiting",
                State::Done => "done",
                State::Error => "error",
            }
            .into(),
            match progress {
                None => String::new(),
                Some(Progress::Value(v)) => v.to_string(),
                Some(Progress::Template(t)) => t.clone(),
            },
            text.clone(),
            ttl_s.to_string(),
        ],
        Action::TabColor {
            color,
            ttl_s,
            clear_on_focus,
        } => {
            vec![color.clone(), ttl_s.to_string(), b(*clear_on_focus)]
        }
        Action::Bell {} => vec![],
        Action::Clipboard { source } => vec![source.clone()],
        Action::Recording { op, stop_after_s } => vec![
            if *op == RecOp::Stop { "stop" } else { "start" }.into(),
            stop_after_s.to_string(),
        ],
        Action::Marker { label, color } => vec![label.clone(), color.clone()],
        Action::Counter { reset_on } => vec![match reset_on {
            ResetOn::Focus => "focus",
            ResetOn::CommandStart => "command_start",
            ResetOn::Never => "never",
        }
        .into()],
        Action::Title { template, ttl_s } => vec![template.clone(), ttl_s.to_string()],
        Action::Findings { label } => vec![label.clone()],
        Action::Snapshot { before, after } => vec![before.to_string(), after.to_string()],
        Action::Screenshot { scale, chrome } => vec![
            scale.to_string(),
            if *chrome == ShotChrome::Plain {
                "plain"
            } else {
                "none"
            }
            .into(),
        ],
    }
}

/// The default values of a new action of `kind`.
pub fn default_action(kind: &str) -> Action {
    match kind {
        "highlight" => Action::Highlight {
            scope: Scope::Match,
            color: "warning".into(),
            style: Style::Background,
            ttl_s: 0,
        },
        "notify" => Action::Notify {
            level: Level::Info,
            title: String::new(),
            message: "{line}".into(),
            desktop: false,
        },
        "status" => Action::Status {
            state: State::Working,
            progress: None,
            text: String::new(),
            ttl_s: 0,
        },
        "tab_color" => Action::TabColor {
            color: "red".into(),
            ttl_s: 0,
            clear_on_focus: false,
        },
        "clipboard" => Action::Clipboard {
            source: "match".into(),
        },
        "recording" => Action::Recording {
            op: RecOp::Start,
            stop_after_s: 0,
        },
        "marker" => Action::Marker {
            label: "{rule}".into(),
            color: String::new(),
        },
        "counter" => Action::Counter {
            reset_on: ResetOn::Focus,
        },
        "title" => Action::Title {
            template: "{rule}".into(),
            ttl_s: 0,
        },
        "findings" => Action::Findings {
            label: String::new(),
        },
        "snapshot" => Action::Snapshot {
            before: 0,
            after: 0,
        },
        "screenshot" => Action::Screenshot {
            scale: 1.0,
            chrome: ShotChrome::None,
        },
        _ => Action::Bell {},
    }
}

/// Builds the action of `kind` from edited `values` (the inverse of [`values_of`]).
pub fn action_from(kind: &str, v: &[String]) -> Result<Action, String> {
    let get = |i: usize| v.get(i).map_or("", String::as_str);
    Ok(match kind {
        "highlight" => Action::Highlight {
            scope: if get(0) == "line" {
                Scope::Line
            } else {
                Scope::Match
            },
            color: get(1).trim().to_string(),
            style: if get(2) == "underline" {
                Style::Underline
            } else {
                Style::Background
            },
            ttl_s: uint(get(3), "Expire after")?,
        },
        "notify" => Action::Notify {
            level: match get(0) {
                "attention" => Level::Attention,
                "error" => Level::Error,
                _ => Level::Info,
            },
            title: get(1).to_string(),
            message: get(2).to_string(),
            desktop: get(3) == "true",
        },
        "status" => Action::Status {
            state: match get(0) {
                "idle" => State::Idle,
                "waiting" => State::Waiting,
                "done" => State::Done,
                "error" => State::Error,
                _ => State::Working,
            },
            progress: match get(1).trim() {
                "" => None,
                t => Some(
                    t.parse::<i64>()
                        .map_or_else(|_| Progress::Template(t.to_string()), Progress::Value),
                ),
            },
            text: get(2).to_string(),
            ttl_s: uint(get(3), "Clear after")?,
        },
        "tab_color" => Action::TabColor {
            color: get(0).trim().to_string(),
            ttl_s: uint(get(1), "Revert after")?,
            clear_on_focus: get(2) == "true",
        },
        "bell" => Action::Bell {},
        "clipboard" => Action::Clipboard {
            source: match get(0).trim() {
                "" => "match".into(),
                s => s.to_string(),
            },
        },
        "recording" => Action::Recording {
            op: if get(0) == "stop" {
                RecOp::Stop
            } else {
                RecOp::Start
            },
            stop_after_s: uint(get(1), "Stop after")?,
        },
        "marker" => Action::Marker {
            label: get(0).to_string(),
            color: get(1).trim().to_string(),
        },
        "counter" => Action::Counter {
            reset_on: match get(0) {
                "command_start" => ResetOn::CommandStart,
                "never" => ResetOn::Never,
                _ => ResetOn::Focus,
            },
        },
        "title" => Action::Title {
            template: get(0).to_string(),
            ttl_s: uint(get(1), "Revert after")?,
        },
        "findings" => Action::Findings {
            label: get(0).to_string(),
        },
        "snapshot" => Action::Snapshot {
            before: uint(get(0), "Lines before")?,
            after: uint(get(1), "Lines after")?,
        },
        "screenshot" => Action::Screenshot {
            scale: match get(0).trim() {
                "" => 1.0,
                s => s
                    .parse::<f32>()
                    .map_err(|_| "Scale must be a number".to_string())?,
            },
            chrome: if get(1) == "plain" {
                ShotChrome::Plain
            } else {
                ShotChrome::None
            },
        },
        other => return Err(format!("unknown action {other:?}")),
    })
}

/// One action row of the editor: its kind and the editable parameter values.
#[derive(Clone)]
pub struct ActionEdit {
    pub kind: &'static str,
    /// One per [`params`] entry; text parameters are edited in place, choices and flags are
    /// stored as their value.
    pub fields: Vec<TextEdit>,
}

impl ActionEdit {
    pub fn new(a: &Action) -> Self {
        Self {
            kind: a.kind(),
            fields: values_of(a).into_iter().map(TextEdit::new).collect(),
        }
    }

    pub fn values(&self) -> Vec<String> {
        self.fields.iter().map(|f| f.text().to_string()).collect()
    }

    pub fn to_action(&self) -> Result<Action, String> {
        action_from(self.kind, &self.values())
    }

    /// Cycles a choice parameter or flips a flag.
    pub fn cycle(&mut self, param: usize) {
        let Some(p) = params(self.kind).get(param) else {
            return;
        };
        let Some(field) = self.fields.get_mut(param) else {
            return;
        };
        let next = match p.input {
            Input::Choice(c) => {
                let i = c
                    .iter()
                    .position(|x| *x == field.text())
                    .map_or(0, |i| (i + 1) % c.len());
                c[i].to_string()
            }
            Input::Bool => b(field.text() != "true"),
            Input::Text { .. } => return,
        };
        *field = TextEdit::new(next);
    }
}

/// Which single-line text field has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Focus {
    Name,
    Panes,
    Regex,
    Exit,
    Cooldown,
    /// Action index and parameter index.
    Param(usize, usize),
}

pub struct Editor {
    /// The rule being edited; `None` for a new one.
    pub id: Option<String>,
    pub base: Base,
    pub original: Rule,
    pub name: TextEdit,
    pub panes: TextEdit,
    pub regex: TextEdit,
    pub case_insensitive: bool,
    pub is_text: bool,
    pub event: EventKind,
    pub exit: TextEdit,
    pub cooldown: TextEdit,
    /// Text rules: match command output only (default) or every line.
    pub scope: mt_triggers::MatchIn,
    pub actions: Vec<ActionEdit>,
    pub enabled: bool,
    pub focus: Focus,
    pub confirm_discard: bool,
    pub confirm_delete: bool,
    pub stale: bool,
    pub error: Option<String>,
    /// The Add-action menu is open.
    pub add_menu: bool,
    /// Result of **Test**: per firing, what would run (nothing was run).
    pub test: Option<Vec<DryFiring>>,
    /// "4 matches in the last 200 lines of the focused pane"; refreshed while the regex changes.
    pub preview: Option<String>,
}

fn exit_text(e: ExitFilter) -> String {
    match e {
        ExitFilter::Any => "any".into(),
        ExitFilter::Success => "success".into(),
        ExitFilter::Failure => "failure".into(),
        ExitFilter::Code(c) => c.to_string(),
    }
}

fn parse_exit(s: &str) -> Result<ExitFilter, String> {
    match s.trim() {
        "" | "any" => Ok(ExitFilter::Any),
        "success" => Ok(ExitFilter::Success),
        "failure" => Ok(ExitFilter::Failure),
        n => n
            .parse::<i64>()
            .map(ExitFilter::Code)
            .map_err(|_| "exit must be any, success, failure or a number".to_string()),
    }
}

impl Editor {
    pub fn open(rule: Rule, id: Option<String>, base: Base) -> Self {
        let (regex, ci, is_text, event, exit) = match &rule.when {
            When::Text {
                regex,
                case_insensitive,
            } => (
                regex.clone(),
                *case_insensitive,
                true,
                EventKind::CommandFinished,
                ExitFilter::Any,
            ),
            When::Event { event, exit } => (String::new(), false, false, *event, *exit),
        };
        Self {
            id,
            base,
            name: TextEdit::new(rule.name.clone()),
            panes: TextEdit::new(if rule.panes == "*" {
                String::new()
            } else {
                rule.panes.clone()
            }),
            regex: TextEdit::new(regex),
            case_insensitive: ci,
            is_text,
            event,
            exit: TextEdit::new(exit_text(exit)),
            cooldown: TextEdit::new(rule.cooldown_ms.to_string()),
            scope: rule.scope,
            actions: rule.actions.iter().map(ActionEdit::new).collect(),
            enabled: rule.enabled,
            focus: Focus::Name,
            confirm_discard: false,
            confirm_delete: false,
            stale: false,
            error: None,
            add_menu: false,
            test: None,
            preview: None,
            original: rule,
        }
    }

    /// A blank text rule with one `highlight` action (the **+** button).
    pub fn blank() -> Self {
        let mut r = Rule::new_text("", "");
        r.name = String::new();
        Self::open(r, None, Base::new("", None))
    }

    /// The rule the fields describe. `id` is empty for a new rule until the caller assigns one.
    pub fn to_rule(&self) -> Result<Rule, String> {
        let when = if self.is_text {
            When::Text {
                regex: self.regex.text().to_string(),
                case_insensitive: self.case_insensitive,
            }
        } else {
            When::Event {
                event: self.event,
                exit: parse_exit(self.exit.text())?,
            }
        };
        let cooldown = self.cooldown.text().trim();
        let mut actions = Vec::new();
        for a in &self.actions {
            actions.push(a.to_action()?);
        }
        Ok(Rule {
            id: self.id.clone().unwrap_or_default(),
            name: self.name.text().trim().to_string(),
            enabled: self.enabled,
            panes: match self.panes.text().trim() {
                "" => "*".into(),
                p => p.to_string(),
            },
            cooldown_ms: if cooldown.is_empty() {
                1000
            } else {
                cooldown
                    .parse()
                    .map_err(|_| "cooldown must be a whole number of milliseconds".to_string())?
            },
            scope: if self.is_text {
                self.scope
            } else {
                mt_triggers::MatchIn::Output
            },
            when,
            actions,
        })
    }

    /// Validation problems by field, for the inline errors. Empty when the rule can be saved.
    pub fn diagnostics(&self) -> Vec<(Field, String)> {
        match self.to_rule() {
            Err(e) => vec![(Field::Rule, e)],
            Ok(r) => validate_rule(&r)
                .into_iter()
                .map(|i| (i.at, i.message))
                .collect(),
        }
    }

    pub fn issue_at(&self, at: Field) -> Option<String> {
        self.diagnostics()
            .into_iter()
            .find(|(f, _)| *f == at)
            .map(|(_, m)| m)
    }

    /// Why Save is disabled, if it is.
    pub fn save_blocker(&self) -> Option<String> {
        self.diagnostics().into_iter().next().map(|(_, m)| m)
    }

    pub fn dirty(&self) -> bool {
        match self.to_rule() {
            Ok(mut r) => {
                if self.id.is_none() {
                    r.id = self.original.id.clone();
                }
                r != self.original
            }
            Err(_) => true,
        }
    }

    pub fn add_action(&mut self, kind: &str) {
        if self.actions.len() >= MAX_ACTIONS {
            return;
        }
        self.actions.push(ActionEdit::new(&default_action(kind)));
        self.add_menu = false;
        self.fit_actions_to_kind();
    }

    /// Click on a choice parameter or flag of action `ai`. An event rule has no matched text,
    /// so a highlight's scope stays `line`.
    pub fn cycle_param(&mut self, ai: usize, pi: usize) {
        let Some(a) = self.actions.get_mut(ai) else {
            return;
        };
        if a.kind == "highlight" && pi == 0 && !self.is_text {
            return;
        }
        a.cycle(pi);
    }

    /// Whether the focused field is a template (the placeholder list belongs to those only).
    pub fn focus_is_template(&self) -> bool {
        let Focus::Param(i, j) = self.focus else {
            return false;
        };
        self.actions
            .get(i)
            .and_then(|a| params(a.kind).get(j))
            .is_some_and(|p| matches!(p.input, Input::Text { template: true }))
    }

    /// Keeps the actions valid for the kind of rule: a highlight of an event rule marks the line.
    fn fit_actions_to_kind(&mut self) {
        if self.is_text {
            return;
        }
        for a in self.actions.iter_mut().filter(|a| a.kind == "highlight") {
            if let Some(f) = a.fields.get_mut(0) {
                *f = TextEdit::new("line");
            }
        }
    }

    pub fn remove_action(&mut self, i: usize) {
        if i < self.actions.len() {
            self.actions.remove(i);
            self.focus = Focus::Name;
        }
    }

    /// Moves action `i` up (`up`) or down; the actions run in this order.
    pub fn move_action(&mut self, i: usize, up: bool) {
        let j = if up { i.checked_sub(1) } else { Some(i + 1) };
        if let Some(j) = j.filter(|j| *j < self.actions.len() && i < self.actions.len()) {
            self.actions.swap(i, j);
        }
    }

    /// Every focusable text field in Tab order.
    pub fn fields(&self) -> Vec<Focus> {
        let mut v = vec![Focus::Name, Focus::Panes];
        v.push(if self.is_text {
            Focus::Regex
        } else {
            Focus::Exit
        });
        v.push(Focus::Cooldown);
        for (i, a) in self.actions.iter().enumerate() {
            for (j, p) in params(a.kind).iter().enumerate() {
                if matches!(p.input, Input::Text { .. }) {
                    v.push(Focus::Param(i, j));
                }
            }
        }
        if !self.is_text && !self.event.has_exit() {
            v.retain(|f| *f != Focus::Exit);
        }
        v
    }

    pub fn focus_next(&mut self, back: bool) {
        let f = self.fields();
        let i = f.iter().position(|x| *x == self.focus).unwrap_or(0);
        let n = f.len();
        self.focus = f[if back { (i + n - 1) % n } else { (i + 1) % n }];
    }

    pub fn line_edit(&mut self) -> Option<&mut TextEdit> {
        match self.focus {
            Focus::Name => Some(&mut self.name),
            Focus::Panes => Some(&mut self.panes),
            Focus::Regex => Some(&mut self.regex),
            Focus::Exit => Some(&mut self.exit),
            Focus::Cooldown => Some(&mut self.cooldown),
            Focus::Param(i, j) => self.actions.get_mut(i).and_then(|a| a.fields.get_mut(j)),
        }
    }

    /// The placeholder names the templates may use now (from the regex's capture groups).
    pub fn placeholders(&self) -> Vec<String> {
        match self.to_rule() {
            Ok(mut r) => {
                r.actions.clear();
                mt_triggers::rules::available_placeholders(&r)
            }
            Err(_) => Vec::new(),
        }
    }

    pub fn set_text_kind(&mut self, text: bool) {
        self.is_text = text;
        self.fit_actions_to_kind();
        self.test = None;
        self.preview = None;
        if !text && matches!(self.focus, Focus::Regex) {
            self.focus = Focus::Name;
        }
        if text && matches!(self.focus, Focus::Exit) {
            self.focus = Focus::Name;
        }
    }

    pub fn set_event(&mut self, e: EventKind) {
        self.event = e;
        if !e.has_exit() {
            self.exit = TextEdit::new("any");
            if self.focus == Focus::Exit {
                self.focus = Focus::Name;
            }
        }
    }
}

#[cfg(test)]
mod tests;
