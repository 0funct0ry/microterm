//! Rule model, serialised as `triggers.toml` (§10.11.2).

use serde::{Deserialize, Serialize};

pub const MAX_ACTIONS: usize = 4;
pub const MAX_REGEX_CHARS: usize = 1024;
pub const MAX_NAME_CHARS: usize = 60;
pub const MAX_TEMPLATE_CHARS: usize = 256;
pub const REGEX_SIZE_LIMIT: usize = 1 << 20;
pub const REGEX_DFA_SIZE_LIMIT: usize = 2 << 20;

fn yes() -> bool {
    true
}
fn is_yes(b: &bool) -> bool {
    *b
}
fn is_no(b: &bool) -> bool {
    !*b
}
fn any_panes() -> String {
    "*".into()
}
fn is_any_panes(s: &str) -> bool {
    s == "*"
}
fn default_cooldown() -> u32 {
    1000
}
fn is_default_cooldown(c: &u32) -> bool {
    *c == 1000
}
fn is_zero(n: &u32) -> bool {
    *n == 0
}
fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}
fn match_src() -> String {
    "match".into()
}
fn is_match_src(s: &str) -> bool {
    s == "match"
}
fn one() -> f32 {
    1.0
}
fn is_one(f: &f32) -> bool {
    (*f - 1.0).abs() < f32::EPSILON
}

/// One trigger rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Generated, stable, never reused. Empty in a file until the panel writes it back.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub name: String,
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
    /// §12.3 pane selector; `*` = every pane.
    #[serde(default = "any_panes", skip_serializing_if = "is_any_panes")]
    pub panes: String,
    #[serde(
        default = "default_cooldown",
        skip_serializing_if = "is_default_cooldown"
    )]
    pub cooldown_ms: u32,
    /// Text rules: which lines may match (default `output`: not the prompt or what you type).
    #[serde(default, skip_serializing_if = "is_default")]
    pub scope: MatchIn,
    pub when: When,
    #[serde(default)]
    pub actions: Vec<Action>,
}

impl Rule {
    /// Every action only decorates or counts (`highlight`, `marker`, `counter`): such a rule is
    /// exempt from the per-pane and global rate limits, so a burst is still marked and counted
    /// exactly. Its cooldown still applies (set `cooldown_ms = 0` to see every match).
    pub fn is_quiet(&self) -> bool {
        !self.actions.is_empty()
            && self
                .actions
                .iter()
                .all(|a| matches!(a.kind(), "highlight" | "marker" | "counter"))
    }

    /// A new enabled text rule with one `highlight` action.
    pub fn new_text(name: &str, regex: &str) -> Rule {
        Rule {
            id: String::new(),
            name: name.to_string(),
            enabled: true,
            panes: "*".into(),
            cooldown_ms: 1000,
            scope: MatchIn::Output,
            when: When::Text {
                regex: regex.to_string(),
                case_insensitive: false,
            },
            actions: vec![Action::Highlight {
                scope: Scope::Match,
                color: "warning".into(),
                style: Style::Background,
                ttl_s: 0,
            }],
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.when, When::Text { .. })
    }
}

/// Which lines a text rule may match (`scope` of a rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchIn {
    /// Command output only: lines of the shell prompt and of the command being typed (including
    /// an autosuggestion) are skipped. Needs shell integration (OSC 133) to tell them apart;
    /// without it every line counts as output.
    #[default]
    Output,
    /// Every line, input included.
    Any,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum When {
    Text {
        regex: String,
        #[serde(default, skip_serializing_if = "is_no")]
        case_insensitive: bool,
    },
    Event {
        event: EventKind,
        #[serde(default, skip_serializing_if = "is_default")]
        exit: ExitFilter,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    CommandFinished,
    CwdChanged,
    PaneExited,
    TitleChanged,
    Bell,
}

impl EventKind {
    pub fn name(self) -> &'static str {
        match self {
            EventKind::CommandFinished => "command_finished",
            EventKind::CwdChanged => "cwd_changed",
            EventKind::PaneExited => "pane_exited",
            EventKind::TitleChanged => "title_changed",
            EventKind::Bell => "bell",
        }
    }

    pub fn has_exit(self) -> bool {
        matches!(self, EventKind::CommandFinished | EventKind::PaneExited)
    }
}

/// `exit = "any" | "success" | "failure" | <integer>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExitFilter {
    #[default]
    Any,
    Success,
    Failure,
    Code(i64),
}

impl ExitFilter {
    pub fn matches(self, code: Option<i64>) -> bool {
        match (self, code) {
            (ExitFilter::Any, _) => true,
            (ExitFilter::Success, c) => c == Some(0),
            (ExitFilter::Failure, c) => c.is_some_and(|c| c != 0),
            (ExitFilter::Code(want), c) => c == Some(want),
        }
    }
}

impl Serialize for ExitFilter {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ExitFilter::Any => s.serialize_str("any"),
            ExitFilter::Success => s.serialize_str("success"),
            ExitFilter::Failure => s.serialize_str("failure"),
            ExitFilter::Code(c) => s.serialize_i64(*c),
        }
    }
}

impl<'de> Deserialize<'de> for ExitFilter {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(i64),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(ExitFilter::Code(n)),
            Raw::S(s) => match s.as_str() {
                "any" => Ok(ExitFilter::Any),
                "success" => Ok(ExitFilter::Success),
                "failure" => Ok(ExitFilter::Failure),
                other => Err(serde::de::Error::custom(format!(
                    "exit must be \"any\", \"success\", \"failure\" or an integer, not {other:?}"
                ))),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Match,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    #[default]
    Background,
    Underline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Info,
    Attention,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Idle,
    Working,
    Waiting,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetOn {
    #[default]
    Focus,
    CommandStart,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecOp {
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotChrome {
    #[default]
    None,
    Plain,
}

/// The closed set of actions (§10.11.4). Nothing here runs a process, types into a pane or uses
/// the network.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Highlight {
        #[serde(default, skip_serializing_if = "is_default")]
        scope: Scope,
        color: String,
        #[serde(default, skip_serializing_if = "is_default")]
        style: Style,
        #[serde(default, skip_serializing_if = "is_zero")]
        ttl_s: u32,
    },
    Notify {
        #[serde(default, skip_serializing_if = "is_default")]
        level: Level,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        title: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
        #[serde(default, skip_serializing_if = "is_no")]
        desktop: bool,
    },
    Status {
        state: State,
        /// Integer 0-100 or a template that must expand to one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<Progress>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        ttl_s: u32,
    },
    TabColor {
        color: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        ttl_s: u32,
        #[serde(default, skip_serializing_if = "is_no")]
        clear_on_focus: bool,
    },
    Bell {},
    Clipboard {
        /// `match` (default), a capture number or a capture name.
        #[serde(default = "match_src", skip_serializing_if = "is_match_src")]
        source: String,
    },
    Recording {
        op: RecOp,
        #[serde(default, skip_serializing_if = "is_zero")]
        stop_after_s: u32,
    },
    Marker {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        color: String,
    },
    Counter {
        #[serde(default, skip_serializing_if = "is_default")]
        reset_on: ResetOn,
    },
    Title {
        template: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        ttl_s: u32,
    },
    Findings {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
    },
    Snapshot {
        #[serde(default, skip_serializing_if = "is_zero")]
        before: u32,
        #[serde(default, skip_serializing_if = "is_zero")]
        after: u32,
    },
    Screenshot {
        #[serde(default = "one", skip_serializing_if = "is_one")]
        scale: f32,
        #[serde(default, skip_serializing_if = "is_default")]
        chrome: ShotChrome,
    },
}

/// A `status` progress value: a literal number or a template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Progress {
    Value(i64),
    Template(String),
}

/// Which second gate (if any) an action needs (§10.11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Clipboard,
    Capture,
}

impl Gate {
    /// The `microterm.toml` key that opens the gate.
    pub fn key(self) -> &'static str {
        match self {
            Gate::Clipboard => "triggers.allow_clipboard",
            Gate::Capture => "triggers.allow_capture",
        }
    }

    pub fn hint(self) -> String {
        format!("Set `{}` = true in microterm.toml", self.key())
    }
}

impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Highlight { .. } => "highlight",
            Action::Notify { .. } => "notify",
            Action::Status { .. } => "status",
            Action::TabColor { .. } => "tab_color",
            Action::Bell {} => "bell",
            Action::Clipboard { .. } => "clipboard",
            Action::Recording { .. } => "recording",
            Action::Marker { .. } => "marker",
            Action::Counter { .. } => "counter",
            Action::Title { .. } => "title",
            Action::Findings { .. } => "findings",
            Action::Snapshot { .. } => "snapshot",
            Action::Screenshot { .. } => "screenshot",
        }
    }

    pub fn gate(&self) -> Option<Gate> {
        match self {
            Action::Clipboard { .. } => Some(Gate::Clipboard),
            Action::Snapshot { .. } | Action::Screenshot { .. } => Some(Gate::Capture),
            _ => None,
        }
    }

    /// Kinds a rule may list at most once.
    pub fn is_singleton(&self) -> bool {
        matches!(
            self,
            Action::Title { .. }
                | Action::TabColor { .. }
                | Action::Status { .. }
                | Action::Counter { .. }
        )
    }

    /// Every template field of the action (for placeholder validation and length limits).
    pub fn templates(&self) -> Vec<(&'static str, &str)> {
        match self {
            Action::Notify { title, message, .. } => vec![("title", title), ("message", message)],
            Action::Status { progress, text, .. } => {
                let mut v = vec![("text", text.as_str())];
                if let Some(Progress::Template(t)) = progress {
                    v.push(("progress", t));
                }
                v
            }
            Action::Marker { label, .. } => vec![("label", label)],
            Action::Title { template, .. } => vec![("template", template)],
            Action::Findings { label } => vec![("label", label)],
            _ => Vec::new(),
        }
    }
}

/// Names of the highlight theme tokens besides the nine tab colors and `#rrggbb`.
pub const THEME_TOKENS: [&str; 5] = ["accent", "warning", "error", "success", "info"];

/// Whether `s` is a valid highlight/marker color.
pub fn valid_color(s: &str) -> bool {
    THEME_TOKENS.contains(&s) || mt_proto::color::TabColor::parse(s).is_ok()
}
