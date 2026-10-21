//! Wire types and codecs for the GUI↔CLI IPC protocol (SPEC §12.2): one JSON object per line over
//! a Unix socket, with a versioned envelope. The mux binary-frame protocol lives in [`mux`].
//!
//! ```text
//! → {"v":1,"id":7,"method":"pane.send_text","params":{"pane":12,"text":"ls\n","paste":true}}
//! ← {"v":1,"id":7,"result":{}}
//! ← {"v":1,"id":8,"error":{"code":"not_found","message":"pane 99 does not exist"}}
//! ```
//!
//! `subscribe` turns the connection into an event stream: notifications carry no `id`.
//!
//! Targets travel as `{pane|tab|window: ID, match: "selector", all, force}`; selectors are parsed
//! here ([`selector`]) and evaluated by the server so every client gets identical semantics.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Protocol major version. A peer speaking a different major is rejected.
pub const PROTO_VERSION: u32 = 1;
/// Longest accepted line, in bytes.
pub const MAX_LINE: usize = 1024 * 1024;

pub mod mux;
pub mod selector;
pub mod tools;
pub mod when;

pub use selector::{Resource, Selector};

/// CLI exit codes (SPEC §12.3).
pub mod exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 1;
    pub const NO_INSTANCE: i32 = 2;
    pub const NOT_FOUND: i32 = 3;
    pub const VERSION_MISMATCH: i32 = 4;
    pub const AMBIGUOUS: i32 = 5;
    /// `wait` or `run --timeout` ran out; the command keeps running.
    pub const TIMEOUT: i32 = 6;
    /// No OSC 133 marks seen in the pane.
    pub const NEEDS_SHELL_INTEGRATION: i32 = 7;
    pub const CONFIRM_REQUIRED: i32 = 8;
    /// `run --in` while a command is already running.
    pub const BUSY: i32 = 9;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    VersionMismatch,
    Internal,
    /// Several targets matched and `--all` was not given (exit 5).
    Ambiguous,
    /// `confirm_close` would prompt and `--force` was not given (exit 8).
    ConfirmRequired,
    /// A wait ran out of time (exit 6).
    Timeout,
    /// The pane has never sent OSC 133 marks (exit 7).
    NeedsShellIntegration,
    /// The pane is busy running a command (exit 9).
    Busy,
}

impl ErrorCode {
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::BadRequest | ErrorCode::Internal => exit::USAGE,
            ErrorCode::NotFound => exit::NOT_FOUND,
            ErrorCode::VersionMismatch => exit::VERSION_MISMATCH,
            ErrorCode::Ambiguous => exit::AMBIGUOUS,
            ErrorCode::ConfirmRequired => exit::CONFIRM_REQUIRED,
            ErrorCode::Timeout => exit::TIMEOUT,
            ErrorCode::NeedsShellIntegration => exit::NEEDS_SHELL_INTEGRATION,
            ErrorCode::Busy => exit::BUSY,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
    /// The matches of an ambiguous target, one description per line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            candidates: Vec::new(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
    pub fn confirm_required(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ConfirmRequired, message)
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, message)
    }
    pub fn needs_shell_integration(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NeedsShellIntegration, message)
    }
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Busy, message)
    }
    pub fn ambiguous(message: impl Into<String>, candidates: Vec<String>) -> Self {
        Self {
            candidates,
            ..Self::new(ErrorCode::Ambiguous, message)
        }
    }
}

// ---- request parameters -------------------------------------------------------------------

/// Which item a request addresses (SPEC §12.3). `pane`, `tab` and `window` are explicit IDs (the
/// verb's resource decides which one is read; `tab list` and `tab new` also use `window` as a
/// filter or parent), `match` is a selector. `all` and `force` apply to acting verbs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub force: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Target {
    pub fn pane(id: u64) -> Self {
        Target {
            pane: Some(id),
            ..Default::default()
        }
    }
    pub fn tab(id: u64) -> Self {
        Target {
            tab: Some(id),
            ..Default::default()
        }
    }
    pub fn window(id: u64) -> Self {
        Target {
            window: Some(id),
            ..Default::default()
        }
    }
    pub fn matching(expr: impl Into<String>) -> Self {
        Target {
            expr: Some(expr.into()),
            ..Default::default()
        }
    }

    /// The explicit id for `resource`.
    pub fn id_for(&self, resource: Resource) -> Option<u64> {
        match resource {
            Resource::Pane => self.pane,
            Resource::Tab => self.tab,
            Resource::Window => self.window,
            Resource::Workspace => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pane.is_none() && self.tab.is_none() && self.window.is_none() && self.expr.is_none()
    }
}

/// Where the caller runs, from `$MICROTERM_PANE|TAB|WINDOW` (SPEC §12.3 step 3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallerEnv {
    pub pane: Option<u64>,
    pub tab: Option<u64>,
    pub window: Option<u64>,
}

impl CallerEnv {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().and_then(|v| v.trim().parse().ok());
        CallerEnv {
            pane: get("MICROTERM_PANE"),
            tab: get("MICROTERM_TAB"),
            window: get("MICROTERM_WINDOW"),
        }
    }
}

impl Target {
    /// Client-side half of target resolution (SPEC §12.3): an explicit id or `--match` wins,
    /// otherwise the caller's own item from the environment fills in the id for `resource`.
    /// The focused-item fallback of read-only verbs is the server's job.
    pub fn with_env(mut self, resource: Resource, env: CallerEnv) -> Target {
        if self.id_for(resource).is_none() && self.expr.is_none() {
            match resource {
                Resource::Pane => self.pane = env.pane,
                Resource::Tab => self.tab = env.tab,
                Resource::Window => self.window = env.window,
                Resource::Workspace => {}
            }
        }
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    #[serde(default)]
    pub new_window: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Terminal size, used by `microterm record --cols/--rows`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendTextParams {
    #[serde(flatten)]
    pub target: Target,
    pub text: String,
    #[serde(default = "yes")]
    pub paste: bool,
}

fn yes() -> bool {
    true
}

/// Which neighbour of a pane (SPEC §17.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// `pane split`: the new pane inherits the target's domain and cwd unless `cwd` or `command`
/// say otherwise. `percent` is the new pane's share (default 50). `direction` (M11.1) says on
/// which side of the target the new pane goes; when set it overrides `axis`, which senders still
/// fill in (`left`/`right` = horizontal) so older servers split along the right axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSplitParams {
    #[serde(flatten)]
    pub target: Target,
    pub axis: SplitAxis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

/// `pane focus`: focus the target pane, or its neighbour in `direction`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneFocusParams {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
}

/// `pane resize`: move the target's border in `direction` by `cells` terminal cells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneResizeParams {
    #[serde(flatten)]
    pub target: Target,
    pub direction: Direction,
    pub cells: u32,
}

/// `pane swap`: exchange the target with its neighbour in `direction`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneDirParams {
    #[serde(flatten)]
    pub target: Target,
    pub direction: Direction,
}

/// Result of `pane split`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSplitResult {
    pub pane: u64,
    pub tab: u64,
    pub window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetTextParams {
    #[serde(flatten)]
    pub target: Target,
    /// Absolute-from-screen-top line numbers: `0` is the first visible row, negative values reach
    /// into scrollback (`-1` is the row just above the screen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<i64>,
    /// Inclusive. Defaults to the last screen row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default)]
    pub escapes: bool,
    /// The output of the N-th most recent finished command (OSC 133) instead of screen rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command: Option<u32>,
}

/// `pane.run` (SPEC §12.4). Without `in`, a new tab runs `spawn.command` as argv and the call
/// returns when the process exits. With `in`, `line` is typed into that shell pane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunParams {
    #[serde(flatten)]
    pub spawn: SpawnParams,
    /// Keep the tab after the process exits (it then shows `[process exited with status N]`).
    #[serde(default)]
    pub keep: bool,
    /// Return `{pane, tab}` immediately.
    #[serde(default)]
    pub detach: bool,
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_pane: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// `pane.wait` (SPEC §12.4): the first condition that holds wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitParams {
    #[serde(flatten)]
    pub target: Target,
    /// Regular expressions over the rendered, unwrapped text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<String>,
    /// No PTY output for this long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<u64>,
    #[serde(default)]
    pub exit: bool,
    #[serde(default)]
    pub command_finished: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Ignore what is already on screen or in scrollback.
    #[serde(default)]
    pub new_only: bool,
}

/// Pane status state (SPEC §12.4). `idle` shows no dot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusState {
    Idle,
    #[default]
    Working,
    Waiting,
    Done,
    Error,
}

impl StatusState {
    pub fn as_str(self) -> &'static str {
        match self {
            StatusState::Idle => "idle",
            StatusState::Working => "working",
            StatusState::Waiting => "waiting",
            StatusState::Done => "done",
            StatusState::Error => "error",
        }
    }
}

impl std::str::FromStr for StatusState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "idle" => StatusState::Idle,
            "working" => StatusState::Working,
            "waiting" => StatusState::Waiting,
            "done" => StatusState::Done,
            "error" => StatusState::Error,
            other => {
                return Err(format!(
                    "unknown state {other:?} (idle, working, waiting, done, error)"
                ))
            }
        })
    }
}

/// `pane notify --level` (SPEC §12.4); the badge colour follows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyLevel {
    #[default]
    Info,
    Attention,
    Error,
}

impl NotifyLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyLevel::Info => "info",
            NotifyLevel::Attention => "attention",
            NotifyLevel::Error => "error",
        }
    }
}

impl std::str::FromStr for NotifyLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "info" => NotifyLevel::Info,
            "attention" => NotifyLevel::Attention,
            "error" => NotifyLevel::Error,
            other => return Err(format!("unknown level {other:?} (info, attention, error)")),
        })
    }
}

/// `pane.status_set` (SPEC §12.4). Without `state` the call means `working`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSetParams {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StatusState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 0–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// The status clears itself after this long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// `pane.notify` (SPEC §12.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyParams {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub level: NotifyLevel,
    /// Force an OS notification regardless of `[notifications].desktop`.
    #[serde(default)]
    pub desktop: bool,
    #[serde(default)]
    pub message: String,
}

/// `pane.subscribe` filters: event types (`pane.exited`, `workspace.*`), a selector over the
/// pane/tab/window the event concerns, and an initial snapshot of the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    #[serde(default)]
    pub snapshot: bool,
}

impl SubscribeParams {
    /// Whether `name` (a dotted event type) passes the `types` filter.
    ///
    /// With no filter every event passes except `pane.output`, which is too noisy to stream by
    /// default (SPEC §12.4); ask for it by name.
    pub fn wants(&self, name: &str) -> bool {
        if self.types.is_empty() {
            return name != "pane.output";
        }
        self.types.iter().any(|t| match t.strip_suffix(".*") {
            Some(prefix) => name
                .strip_prefix(prefix)
                .is_some_and(|r| r.starts_with('.')),
            None => t == name,
        })
    }
}

/// Default `pane wait` timeout.
pub const DEFAULT_WAIT_TIMEOUT_MS: u64 = 60_000;

/// `tab.focus`: a target tab, or an `index` / `relative` move inside `window` (default: the
/// caller's or focused window).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabFocusParams {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabRenameParams {
    #[serde(flatten)]
    pub target: Target,
    /// An empty title clears the override.
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordStartParams {
    pub pane: u64,
    pub path: String,
    #[serde(default)]
    pub input: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayParams {
    pub path: String,
    #[serde(default = "one")]
    pub speed: f64,
    #[serde(default)]
    pub new_window: bool,
}

fn one() -> f64 {
    1.0
}

/// `menu.show`: the rows of `menu` for a pane (SPEC §10.9.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuShowParams {
    #[serde(flatten)]
    pub target: Target,
    pub menu: String,
}

/// `menu.invoke`: run `item` (`MENU/ITEM`) on a pane. `target.force` accepts a confirmation;
/// `dry_run` only resolves and expands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuInvokeParams {
    #[serde(flatten)]
    pub target: Target,
    pub item: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dry_run: bool,
}

/// Every IPC request (SPEC §12.2).
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    MuxList,
    WorkspaceList,
    WorkspaceCreate {
        name: String,
    },
    WorkspaceSwitch {
        name: String,
    },
    WorkspaceRename {
        old: String,
        new: String,
    },
    WorkspaceDelete {
        name: String,
        force: bool,
    },
    WindowList(Target),
    /// Also the M5 single-instance request: open a window in the running instance.
    WindowNew(SpawnParams),
    WindowFocus(Target),
    WindowClose(Target),
    TabList(Target),
    TabNew(SpawnParams),
    TabFocus(TabFocusParams),
    TabRename(TabRenameParams),
    TabClose(Target),
    PaneList(Target),
    PaneGet(Target),
    PaneSendText(SendTextParams),
    PaneGetText(GetTextParams),
    PaneRun(RunParams),
    PaneWait(WaitParams),
    PaneNotify(NotifyParams),
    PaneStatusSet(StatusSetParams),
    PaneStatusClear(Target),
    PaneStatusGet(Target),
    PaneClose(Target),
    PaneSplit(PaneSplitParams),
    PaneFocus(PaneFocusParams),
    PaneZoom(Target),
    PaneResize(PaneResizeParams),
    PaneSwap(PaneDirParams),
    PaneRotate(Target),
    RecordStart(RecordStartParams),
    RecordStop {
        pane: u64,
    },
    ReplayStart(ReplayParams),
    ConfigReload,
    Subscribe(SubscribeParams),
    MenuList,
    MenuShow(MenuShowParams),
    MenuInvoke(MenuInvokeParams),
}

/// Every method name, in a stable order; a test pins this list.
pub const METHODS: &[&str] = &[
    "mux.list",
    "workspace.list",
    "workspace.create",
    "workspace.switch",
    "workspace.rename",
    "workspace.delete",
    "window.list",
    "window.new",
    "window.focus",
    "window.close",
    "tab.list",
    "tab.new",
    "tab.focus",
    "tab.rename",
    "tab.close",
    "pane.list",
    "pane.get",
    "pane.send_text",
    "pane.get_text",
    "pane.run",
    "pane.wait",
    "pane.notify",
    "pane.status_set",
    "pane.status_clear",
    "pane.status_get",
    "pane.close",
    "pane.split",
    "pane.focus",
    "pane.zoom",
    "pane.resize",
    "pane.swap",
    "pane.rotate",
    "record.start",
    "record.stop",
    "replay.start",
    "config.reload",
    "subscribe",
    "menu.list",
    "menu.show",
    "menu.invoke",
];

impl Request {
    pub fn method(&self) -> &'static str {
        match self {
            Request::MuxList => "mux.list",
            Request::WorkspaceList => "workspace.list",
            Request::WorkspaceCreate { .. } => "workspace.create",
            Request::WorkspaceSwitch { .. } => "workspace.switch",
            Request::WorkspaceRename { .. } => "workspace.rename",
            Request::WorkspaceDelete { .. } => "workspace.delete",
            Request::WindowList(_) => "window.list",
            Request::WindowNew(_) => "window.new",
            Request::WindowFocus(_) => "window.focus",
            Request::WindowClose(_) => "window.close",
            Request::TabList(_) => "tab.list",
            Request::TabNew(_) => "tab.new",
            Request::TabFocus(_) => "tab.focus",
            Request::TabRename(_) => "tab.rename",
            Request::TabClose(_) => "tab.close",
            Request::PaneList(_) => "pane.list",
            Request::PaneGet(_) => "pane.get",
            Request::PaneSendText(_) => "pane.send_text",
            Request::PaneGetText(_) => "pane.get_text",
            Request::PaneRun(_) => "pane.run",
            Request::PaneWait(_) => "pane.wait",
            Request::PaneNotify(_) => "pane.notify",
            Request::PaneStatusSet(_) => "pane.status_set",
            Request::PaneStatusClear(_) => "pane.status_clear",
            Request::PaneStatusGet(_) => "pane.status_get",
            Request::PaneClose(_) => "pane.close",
            Request::PaneSplit(_) => "pane.split",
            Request::PaneFocus(_) => "pane.focus",
            Request::PaneZoom(_) => "pane.zoom",
            Request::PaneResize(_) => "pane.resize",
            Request::PaneSwap(_) => "pane.swap",
            Request::PaneRotate(_) => "pane.rotate",
            Request::RecordStart(_) => "record.start",
            Request::RecordStop { .. } => "record.stop",
            Request::ReplayStart(_) => "replay.start",
            Request::ConfigReload => "config.reload",
            Request::MenuList => "menu.list",
            Request::MenuShow(_) => "menu.show",
            Request::MenuInvoke(_) => "menu.invoke",
            Request::Subscribe(_) => "subscribe",
        }
    }

    /// Requests that may legitimately take a long time to answer (the host replies when its
    /// condition holds or its own timeout fires), so the connection does not apply the usual
    /// reply deadline. They are cancelled when the client disconnects.
    pub fn is_long_running(&self) -> bool {
        matches!(self, Request::PaneRun(_) | Request::PaneWait(_))
    }

    pub fn params(&self) -> Value {
        let to = |v: &dyn erased::Ser| v.to_value();
        match self {
            Request::MuxList
            | Request::WorkspaceList
            | Request::ConfigReload
            | Request::MenuList => json!({}),
            Request::MenuShow(p) => to(p),
            Request::MenuInvoke(p) => to(p),
            Request::Subscribe(p) => to(p),
            Request::WorkspaceCreate { name } | Request::WorkspaceSwitch { name } => {
                json!({ "name": name })
            }
            Request::WorkspaceRename { old, new } => json!({ "old": old, "new": new }),
            Request::WorkspaceDelete { name, force } => json!({ "name": name, "force": force }),
            Request::WindowList(t)
            | Request::WindowFocus(t)
            | Request::WindowClose(t)
            | Request::TabList(t)
            | Request::TabClose(t)
            | Request::PaneList(t)
            | Request::PaneGet(t)
            | Request::PaneStatusClear(t)
            | Request::PaneStatusGet(t)
            | Request::PaneClose(t)
            | Request::PaneZoom(t)
            | Request::PaneRotate(t) => to(t),
            Request::PaneSplit(p) => to(p),
            Request::PaneFocus(p) => to(p),
            Request::PaneResize(p) => to(p),
            Request::PaneSwap(p) => to(p),
            Request::WindowNew(p) | Request::TabNew(p) => to(p),
            Request::TabFocus(p) => to(p),
            Request::TabRename(p) => to(p),
            Request::PaneSendText(p) => to(p),
            Request::PaneGetText(p) => to(p),
            Request::PaneRun(p) => to(p),
            Request::PaneWait(p) => to(p),
            Request::PaneNotify(p) => to(p),
            Request::PaneStatusSet(p) => to(p),
            Request::RecordStop { pane } => json!({ "pane": pane }),
            Request::RecordStart(p) => to(p),
            Request::ReplayStart(p) => to(p),
        }
    }

    pub fn from_parts(method: &str, params: Value) -> Result<Request, RpcError> {
        fn p<T: for<'de> Deserialize<'de>>(method: &str, v: Value) -> Result<T, RpcError> {
            serde_json::from_value(v)
                .map_err(|e| RpcError::bad_request(format!("invalid params for {method}: {e}")))
        }
        #[derive(Deserialize)]
        struct Pane {
            pane: u64,
        }
        #[derive(Deserialize)]
        struct Name {
            name: String,
        }
        #[derive(Deserialize)]
        struct Delete {
            name: String,
            #[serde(default)]
            force: bool,
        }
        #[derive(Deserialize)]
        struct Rename {
            old: String,
            new: String,
        }
        let params = if params.is_null() { json!({}) } else { params };
        Ok(match method {
            "mux.list" => Request::MuxList,
            "workspace.list" => Request::WorkspaceList,
            "workspace.create" => Request::WorkspaceCreate {
                name: p::<Name>(method, params)?.name,
            },
            "workspace.switch" => Request::WorkspaceSwitch {
                name: p::<Name>(method, params)?.name,
            },
            "workspace.rename" => {
                let r: Rename = p(method, params)?;
                Request::WorkspaceRename {
                    old: r.old,
                    new: r.new,
                }
            }
            "workspace.delete" => {
                let d: Delete = p(method, params)?;
                Request::WorkspaceDelete {
                    name: d.name,
                    force: d.force,
                }
            }
            "window.list" => Request::WindowList(p(method, params)?),
            "window.new" => Request::WindowNew(p(method, params)?),
            "window.focus" => Request::WindowFocus(p(method, params)?),
            "window.close" => Request::WindowClose(p(method, params)?),
            "tab.list" => Request::TabList(p(method, params)?),
            "tab.new" => Request::TabNew(p(method, params)?),
            "tab.focus" => Request::TabFocus(p(method, params)?),
            "tab.rename" => Request::TabRename(p(method, params)?),
            "tab.close" => Request::TabClose(p(method, params)?),
            "pane.list" => Request::PaneList(p(method, params)?),
            "pane.get" => Request::PaneGet(p(method, params)?),
            "pane.send_text" => Request::PaneSendText(p(method, params)?),
            "pane.get_text" => Request::PaneGetText(p(method, params)?),
            "pane.run" => Request::PaneRun(p(method, params)?),
            "pane.wait" => Request::PaneWait(p(method, params)?),
            "pane.notify" => Request::PaneNotify(p(method, params)?),
            "pane.status_set" => Request::PaneStatusSet(p(method, params)?),
            "pane.status_clear" => Request::PaneStatusClear(p(method, params)?),
            "pane.status_get" => Request::PaneStatusGet(p(method, params)?),
            "pane.close" => Request::PaneClose(p(method, params)?),
            "pane.split" => Request::PaneSplit(p(method, params)?),
            "pane.focus" => Request::PaneFocus(p(method, params)?),
            "pane.zoom" => Request::PaneZoom(p(method, params)?),
            "pane.resize" => Request::PaneResize(p(method, params)?),
            "pane.swap" => Request::PaneSwap(p(method, params)?),
            "pane.rotate" => Request::PaneRotate(p(method, params)?),
            "record.start" => Request::RecordStart(p(method, params)?),
            "record.stop" => Request::RecordStop {
                pane: p::<Pane>(method, params)?.pane,
            },
            "replay.start" => Request::ReplayStart(p(method, params)?),
            "config.reload" => Request::ConfigReload,
            "subscribe" => Request::Subscribe(p(method, params)?),
            "menu.list" => Request::MenuList,
            "menu.show" => Request::MenuShow(p(method, params)?),
            "menu.invoke" => Request::MenuInvoke(p(method, params)?),
            other => return Err(RpcError::bad_request(format!("unknown method {other:?}"))),
        })
    }
}

mod erased {
    use serde::Serialize;
    use serde_json::Value;
    pub trait Ser {
        fn to_value(&self) -> Value;
    }
    impl<T: Serialize> Ser for T {
        fn to_value(&self) -> Value {
            serde_json::to_value(self).unwrap_or(Value::Null)
        }
    }
}

// ---- results ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

/// Which way a split divides its area (SPEC §17.1): `horizontal` puts `a` left of `b`,
/// `vertical` puts `a` above `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

/// A tab's pane layout, as shown by `ctl list --json` and `tab list` (SPEC §17.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LayoutNode {
    Leaf {
        pane: u64,
    },
    Split {
        axis: SplitAxis,
        /// Share of the area given to `a`, 1–99.
        percent: u8,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

/// `pane list|get` (SPEC §12.3). `exited`/`exit_code` are always `false`/`null` today: a pane is
/// removed from the mux when its process exits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u64,
    pub tab: u64,
    pub window: u64,
    pub workspace: String,
    pub domain: String,
    pub title: String,
    pub cwd: Option<String>,
    pub process: Option<String>,
    pub size: Size,
    pub focused: bool,
    /// The pane fills its tab (`pane zoom`).
    #[serde(default)]
    pub zoomed: bool,
    pub exited: bool,
    pub exit_code: Option<i32>,
    /// `null` until something sets a status (`pane status set`).
    #[serde(default)]
    pub status: Option<PaneStatus>,
}

/// A pane's status as reported by `pane list|get` and `pane status get` (SPEC §12.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneStatus {
    pub state: StatusState,
    pub text: String,
    pub progress: Option<u8>,
    pub updated_at: String,
}

/// `pane status get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResult {
    pub pane: u64,
    pub status: Option<PaneStatus>,
}

/// Result of the acting verbs `pane status set|clear` and `pane notify`: the panes affected;
/// `dropped` counts notifications swallowed by the per-pane rate limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActResult {
    pub panes: Vec<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dropped: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `tab list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: u64,
    pub title: String,
    pub window: u64,
    pub workspace: String,
    pub domain: String,
    /// Active tab of its window.
    pub active: bool,
    /// Active tab of the focused window.
    pub focused: bool,
    pub panes: Vec<u64>,
    /// How the panes are split; `null` from older servers.
    #[serde(default)]
    pub layout: Option<LayoutNode>,
}

/// `window list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub workspace: String,
    /// Belongs to the active workspace.
    pub active: bool,
    pub focused: bool,
    pub tabs: Vec<u64>,
}

/// `workspace list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub name: String,
    pub active: bool,
    pub windows: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabNode {
    pub id: u64,
    pub title: String,
    pub active: bool,
    pub focused: bool,
    pub panes: Vec<PaneInfo>,
    #[serde(default)]
    pub layout: Option<LayoutNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowNode {
    pub id: u64,
    pub title: String,
    pub active: bool,
    pub focused: bool,
    pub tabs: Vec<TabNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceNode {
    pub id: u64,
    pub name: String,
    pub active: bool,
    pub windows: Vec<WindowNode>,
}

/// Result of `mux.list`: workspaces → windows → tabs → panes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuxSnapshot {
    pub workspaces: Vec<WorkspaceNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawned {
    pub pane: u64,
    pub tab: u64,
    pub window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub text: String,
    /// The capture cap cut the start of the text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

/// Result of `pane get-text --last-command`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandText {
    pub command: String,
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    pub text: String,
    /// ISO 8601, UTC.
    pub finished_at: String,
    /// The start of the output scrolled out of the history, or the capture cap cut it.
    #[serde(default)]
    pub truncated: bool,
}

/// Result of `pane run`. Detached runs carry only `pane` and `tab`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResult {
    pub pane: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default)]
    pub truncated: bool,
    /// The timeout ran out first; `output` is what the command printed so far.
    #[serde(default, skip_serializing_if = "is_false")]
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitMatch {
    pub text: String,
    /// Capture groups (`null` for groups that did not take part).
    pub groups: Vec<Option<String>>,
    /// Index of the matching line, `0` being the oldest line of scrollback.
    pub line: usize,
}

/// Result of `pane wait`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitResult {
    /// `match`, `idle`, `exit` or `command_finished`.
    pub reason: String,
    pub waited_ms: u64,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub matched: Option<WaitMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// `2026-05-17T12:34:56.789Z` from Unix milliseconds.
pub fn iso8601_utc(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

fn ctl_source() -> String {
    "ctl".into()
}

/// A `MuxEvent` as streamed to `subscribe`rs (SPEC §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    PaneAdded {
        pane: u64,
    },
    PaneRemoved {
        pane: u64,
    },
    PaneOutput {
        pane: u64,
    },
    /// The tab's focused pane changed (splits, `pane focus`).
    PaneFocused {
        pane: u64,
    },
    /// Splits, ratios, pane order or zoom of a tab changed.
    LayoutChanged {
        tab: u64,
    },
    TitleChanged {
        pane: u64,
    },
    CwdChanged {
        pane: u64,
    },
    /// `pane status set|clear` or a TTL expiry changed the pane's status.
    StatusChanged {
        pane: u64,
    },
    /// `pane notify` (after the rate limit).
    Notification {
        pane: u64,
        level: NotifyLevel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        message: String,
        /// `ctl` or `osc9`.
        #[serde(default = "ctl_source")]
        source: String,
    },
    /// The pane's process ended (`None` when killed by a signal).
    PaneExited {
        pane: u64,
        status: Option<i32>,
    },
    /// OSC 133 `D`: a command finished.
    CommandFinished {
        pane: u64,
        exit_code: Option<i32>,
    },
    TabAdded {
        window: u64,
        tab: u64,
    },
    TabRemoved {
        window: u64,
        tab: u64,
    },
    TabMoved {
        window: u64,
        tab: u64,
        from: usize,
        to: usize,
    },
    WindowAdded {
        window: u64,
    },
    WindowRemoved {
        window: u64,
    },
    WorkspaceAdded {
        workspace: u64,
    },
    WorkspaceRenamed {
        workspace: u64,
    },
    WorkspaceSwitched {
        from: u64,
        to: u64,
    },
    WorkspaceRemoved {
        workspace: u64,
    },
    /// `state` is `connecting`, `connected`, `reconnecting` or `disconnected`; `attempt` is set
    /// while reconnecting, `reason` when disconnected.
    DomainStateChanged {
        domain: u64,
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A context-menu item with a side effect ran (SPEC §10.9.5); `summary` is truncated.
    MenuInvoked {
        pane: u64,
        menu: String,
        item: String,
        kind: String,
        summary: String,
    },
    /// The pane's shell-registered menu items changed (SPEC §10.9.7).
    MenuItemsChanged {
        pane: u64,
    },
    /// First line of a `subscribe` with `snapshot`: the whole tree (the `mux.list` result).
    Snapshot {
        tree: Value,
    },
}

/// What an event concerns, for `ctl events --match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Pane(u64),
    Tab(u64),
    Window(u64),
    None,
}

impl Event {
    /// The dotted name used by `ctl events --type` (SPEC §12.4).
    pub fn type_name(&self) -> &'static str {
        match self {
            Event::PaneAdded { .. } => "pane.added",
            Event::PaneRemoved { .. } => "pane.removed",
            Event::PaneOutput { .. } => "pane.output",
            Event::PaneFocused { .. } => "pane.focused",
            Event::LayoutChanged { .. } => "tab.layout_changed",
            Event::TitleChanged { .. } => "pane.title_changed",
            Event::CwdChanged { .. } => "pane.cwd_changed",
            Event::StatusChanged { .. } => "pane.status_changed",
            Event::Notification { .. } => "pane.notification",
            Event::PaneExited { .. } => "pane.exited",
            Event::CommandFinished { .. } => "pane.command_finished",
            Event::TabAdded { .. } => "tab.added",
            Event::TabRemoved { .. } => "tab.removed",
            Event::TabMoved { .. } => "tab.moved",
            Event::WindowAdded { .. } => "window.added",
            Event::WindowRemoved { .. } => "window.removed",
            Event::WorkspaceAdded { .. } => "workspace.added",
            Event::WorkspaceRenamed { .. } => "workspace.renamed",
            Event::WorkspaceSwitched { .. } => "workspace.switched",
            Event::WorkspaceRemoved { .. } => "workspace.removed",
            Event::DomainStateChanged { .. } => "domain.state_changed",
            Event::MenuInvoked { .. } => "menu.invoked",
            Event::MenuItemsChanged { .. } => "menu.items_changed",
            Event::Snapshot { .. } => "snapshot",
        }
    }

    pub fn subject(&self) -> Subject {
        match self {
            Event::PaneAdded { pane }
            | Event::PaneRemoved { pane }
            | Event::PaneOutput { pane }
            | Event::PaneFocused { pane }
            | Event::TitleChanged { pane }
            | Event::CwdChanged { pane }
            | Event::StatusChanged { pane }
            | Event::Notification { pane, .. }
            | Event::PaneExited { pane, .. }
            | Event::CommandFinished { pane, .. }
            | Event::MenuInvoked { pane, .. }
            | Event::MenuItemsChanged { pane } => Subject::Pane(*pane),
            Event::TabAdded { tab, .. }
            | Event::TabRemoved { tab, .. }
            | Event::TabMoved { tab, .. }
            | Event::LayoutChanged { tab } => Subject::Tab(*tab),
            Event::WindowAdded { window } | Event::WindowRemoved { window } => {
                Subject::Window(*window)
            }
            _ => Subject::None,
        }
    }

    /// Whether the subject is gone once this event fires (it can no longer be matched live).
    pub fn is_removal(&self) -> bool {
        matches!(
            self,
            Event::PaneRemoved { .. }
                | Event::PaneExited { .. }
                | Event::TabRemoved { .. }
                | Event::WindowRemoved { .. }
        )
    }

    /// The `ctl events` line: `{"type":"pane.command_finished","pane":12,…,"ts":"…"}`.
    pub fn stream_json(&self, ts_ms: u64) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            m.remove("event");
            let mut out = serde_json::Map::new();
            out.insert("type".into(), json!(self.type_name()));
            out.extend(std::mem::take(m));
            out.insert("ts".into(), json!(iso8601_utc(ts_ms)));
            return Value::Object(out);
        }
        v
    }
}

// ---- envelopes and line codec -------------------------------------------------------------

/// Parses one request line. On failure returns the request `id` (if it could be read) with the
/// error to send back.
pub fn decode_request(line: &str) -> Result<(u64, Request), (Option<u64>, RpcError)> {
    if line.len() > MAX_LINE {
        return Err((None, RpcError::bad_request("request too large")));
    }
    let v: Value = serde_json::from_str(line)
        .map_err(|e| (None, RpcError::bad_request(format!("invalid JSON: {e}"))))?;
    let id = v.get("id").and_then(Value::as_u64);
    if v.get("v").and_then(Value::as_u64) != Some(u64::from(PROTO_VERSION)) {
        return Err((
            id,
            RpcError::new(ErrorCode::VersionMismatch, "unsupported protocol version"),
        ));
    }
    let Some(id) = id else {
        return Err((None, RpcError::bad_request("missing request id")));
    };
    let method = v
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| (Some(id), RpcError::bad_request("missing method")))?;
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    Request::from_parts(method, params)
        .map(|r| (id, r))
        .map_err(|e| (Some(id), e))
}

pub fn encode_request(id: u64, req: &Request) -> String {
    let mut s = json!({
        "v": PROTO_VERSION, "id": id, "method": req.method(), "params": req.params()
    })
    .to_string();
    s.push('\n');
    s
}

pub fn encode_response(id: Option<u64>, res: Result<Value, RpcError>) -> String {
    let mut v = json!({ "v": PROTO_VERSION, "id": id });
    match res {
        Ok(result) => v["result"] = result,
        Err(e) => {
            v["error"] = json!({ "code": e.code, "message": e.message });
            if !e.candidates.is_empty() {
                v["error"]["candidates"] = json!(e.candidates);
            }
        }
    }
    let mut s = v.to_string();
    s.push('\n');
    s
}

pub fn encode_event(event: &Event) -> String {
    let mut s = json!({ "v": PROTO_VERSION, "event": event }).to_string();
    s.push('\n');
    s
}

/// A line read by a client.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response(Result<Value, RpcError>),
    Event(Event),
}

pub fn decode_incoming(line: &str) -> Result<Incoming, RpcError> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| RpcError::bad_request(format!("invalid JSON from server: {e}")))?;
    if v.get("v").and_then(Value::as_u64) != Some(u64::from(PROTO_VERSION)) {
        return Err(RpcError::new(
            ErrorCode::VersionMismatch,
            "server speaks a different protocol version",
        ));
    }
    if let Some(ev) = v.get("event") {
        return serde_json::from_value(ev.clone())
            .map(Incoming::Event)
            .map_err(|e| RpcError::bad_request(format!("bad event: {e}")));
    }
    if let Some(err) = v.get("error") {
        return serde_json::from_value(err.clone())
            .map(|e| Incoming::Response(Err(e)))
            .map_err(|e| RpcError::bad_request(format!("bad error: {e}")));
    }
    Ok(Incoming::Response(Ok(v
        .get("result")
        .cloned()
        .unwrap_or(Value::Null))))
}

// ---- in-process plumbing shared by the GUI and headless hosts ------------------------------

pub use hub::{Call, EventHub};

mod hub {
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    use serde_json::Value;

    use super::{Event, Request, RpcError};

    /// One request travelling from an IPC connection thread to the thread that owns the mux
    /// (the GPUI main thread, or the headless loop). The owner sends the answer on `reply`.
    pub struct Call {
        pub request: Request,
        pub reply: Sender<Result<Value, RpcError>>,
        /// Set by the connection when the client went away while the request was pending.
        /// Long-running requests (`pane.run`, `pane.wait`) stop waiting; commands keep running.
        pub cancel: Arc<AtomicBool>,
    }

    impl Call {
        pub fn new(request: Request, reply: Sender<Result<Value, RpcError>>) -> Self {
            Call {
                request,
                reply,
                cancel: Arc::default(),
            }
        }
    }

    /// Fan-out of [`Event`]s to `subscribe` connections. Cheap to clone behind an `Arc`;
    /// `publish` never blocks, and dead subscribers are dropped.
    #[derive(Default)]
    pub struct EventHub {
        subscribers: Mutex<Vec<Sender<Event>>>,
    }

    impl EventHub {
        pub fn subscribe(&self) -> Receiver<Event> {
            let (tx, rx) = channel();
            self.subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(tx);
            rx
        }

        pub fn publish(&self, event: Event) {
            self.subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|tx| tx.send(event.clone()).is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_fans_out_and_drops_dead_subscribers() {
        let hub = EventHub::default();
        let a = hub.subscribe();
        let b = hub.subscribe();
        drop(b);
        hub.publish(Event::PaneAdded { pane: 1 });
        hub.publish(Event::PaneRemoved { pane: 1 });
        assert_eq!(a.try_recv().unwrap(), Event::PaneAdded { pane: 1 });
        assert_eq!(a.try_recv().unwrap(), Event::PaneRemoved { pane: 1 });
    }

    fn roundtrip(req: Request) {
        let line = encode_request(7, &req);
        assert!(line.ends_with('\n'));
        let (id, back) = decode_request(&line).unwrap();
        assert_eq!(id, 7);
        assert_eq!(back, req);
    }

    #[test]
    fn every_method_round_trips() {
        let t = Target {
            pane: Some(3),
            tab: Some(2),
            window: Some(1),
            expr: Some("title:x*".into()),
            all: true,
            force: true,
        };
        let spawn = SpawnParams {
            window: Some(2),
            workspace: Some("api".into()),
            cwd: Some("/tmp".into()),
            title: Some("t".into()),
            command: Some(vec!["sh".into(), "-c".into(), "echo hi".into()]),
            cols: Some(100),
            rows: Some(30),
            ..Default::default()
        };
        let reqs = vec![
            Request::MuxList,
            Request::WorkspaceList,
            Request::WorkspaceCreate { name: "a".into() },
            Request::WorkspaceSwitch { name: "a".into() },
            Request::WorkspaceRename {
                old: "a".into(),
                new: "b".into(),
            },
            Request::WorkspaceDelete {
                name: "a".into(),
                force: true,
            },
            Request::WindowList(t.clone()),
            Request::WindowNew(spawn.clone()),
            Request::WindowFocus(t.clone()),
            Request::WindowClose(t.clone()),
            Request::TabList(t.clone()),
            Request::TabNew(spawn),
            Request::TabFocus(TabFocusParams {
                target: Target::default(),
                index: Some(1),
                relative: Some(-1),
            }),
            Request::TabRename(TabRenameParams {
                target: Target::tab(5),
                title: "x".into(),
            }),
            Request::TabClose(t.clone()),
            Request::PaneList(t.clone()),
            Request::PaneGet(t.clone()),
            Request::PaneSendText(SendTextParams {
                target: Target::pane(12),
                text: "ls\n".into(),
                paste: false,
            }),
            Request::PaneGetText(GetTextParams {
                target: Target::pane(1),
                start_line: Some(-5),
                end_line: Some(10),
                escapes: true,
                last_command: Some(2),
            }),
            Request::PaneRun(RunParams {
                spawn: SpawnParams {
                    command: Some(vec!["cargo".into(), "test".into()]),
                    title: Some("t".into()),
                    ..Default::default()
                },
                keep: true,
                detach: false,
                in_pane: Some(Target::pane(4)),
                line: Some("ls -l".into()),
                timeout_ms: Some(2000),
            }),
            Request::PaneWait(WaitParams {
                target: Target::matching("title:dev"),
                matches: vec!["ready on".into()],
                idle_ms: Some(500),
                exit: true,
                command_finished: true,
                timeout_ms: Some(1000),
                new_only: true,
            }),
            Request::PaneNotify(NotifyParams {
                target: Target::pane(3),
                title: Some("t".into()),
                level: NotifyLevel::Attention,
                desktop: true,
                message: "done".into(),
            }),
            Request::PaneStatusSet(StatusSetParams {
                target: Target::pane(3),
                state: Some(StatusState::Waiting),
                text: Some("approve?".into()),
                progress: Some(40),
                ttl_ms: Some(30_000),
            }),
            Request::PaneStatusClear(Target::pane(3)),
            Request::PaneStatusGet(Target::pane(3)),
            Request::PaneClose(t),
            Request::PaneSplit(PaneSplitParams {
                target: Target::pane(2),
                axis: SplitAxis::Vertical,
                direction: Some(Direction::Up),
                percent: Some(30),
                cwd: Some("/tmp".into()),
                command: Some(vec!["htop".into()]),
            }),
            Request::PaneFocus(PaneFocusParams {
                target: Target::pane(2),
                direction: Some(Direction::Left),
            }),
            Request::PaneZoom(Target::pane(2)),
            Request::PaneResize(PaneResizeParams {
                target: Target::pane(2),
                direction: Direction::Down,
                cells: 5,
            }),
            Request::PaneSwap(PaneDirParams {
                target: Target::pane(2),
                direction: Direction::Right,
            }),
            Request::PaneRotate(Target::pane(2)),
            Request::RecordStart(RecordStartParams {
                pane: 1,
                path: "x.cast".into(),
                input: true,
            }),
            Request::RecordStop { pane: 1 },
            Request::ReplayStart(ReplayParams {
                path: "x.cast".into(),
                speed: 2.0,
                new_window: true,
            }),
            Request::ConfigReload,
            Request::Subscribe(SubscribeParams {
                types: vec!["pane.*".into()],
                expr: Some("title:x*".into()),
                snapshot: true,
            }),
            Request::MenuList,
            Request::MenuShow(MenuShowParams {
                target: Target::pane(2),
                menu: "viewport".into(),
            }),
            Request::MenuInvoke(MenuInvokeParams {
                target: Target {
                    force: true,
                    ..Target::pane(3)
                },
                item: "viewport/zoom".into(),
                dry_run: true,
            }),
        ];
        let methods: Vec<&str> = reqs.iter().map(Request::method).collect();
        assert_eq!(methods, METHODS, "METHODS lists every request in order");
        for r in reqs {
            roundtrip(r);
        }
    }

    /// The method list is the public IPC surface (SPEC §12.2): changing it is a deliberate act.
    #[test]
    fn method_list_is_pinned() {
        assert_eq!(
            METHODS.join(" "),
            "mux.list workspace.list workspace.create workspace.switch workspace.rename \
             workspace.delete window.list window.new window.focus window.close tab.list tab.new \
             tab.focus tab.rename tab.close pane.list pane.get pane.send_text pane.get_text \
             pane.run pane.wait pane.notify pane.status_set pane.status_clear pane.status_get \
             pane.close pane.split pane.focus pane.zoom pane.resize pane.swap pane.rotate \
             record.start record.stop replay.start config.reload subscribe menu.list menu.show \
             menu.invoke"
        );
        for old in ["pane.spawn", "pane.kill", "tab.activate", "tab.set_title"] {
            assert!(
                Request::from_parts(old, json!({})).is_err(),
                "{old} is gone"
            );
        }
    }

    #[test]
    fn event_types_use_dotted_names_and_filters() {
        let all = SubscribeParams::default();
        assert!(all.wants("pane.exited"));
        let f = SubscribeParams {
            types: vec!["pane.command_finished".into(), "workspace.*".into()],
            ..Default::default()
        };
        assert!(f.wants("pane.command_finished"));
        assert!(f.wants("workspace.switched"));
        assert!(!all.wants("pane.output"), "pane.output is opt-in");
        assert!(SubscribeParams {
            types: vec!["pane.output".into()],
            ..Default::default()
        }
        .wants("pane.output"));
        assert!(!f.wants("pane.exited"));
        assert!(!f.wants("workspacex.added"));
        let ev = Event::CommandFinished {
            pane: 12,
            exit_code: Some(1),
        };
        assert_eq!(ev.type_name(), "pane.command_finished");
        assert_eq!(ev.subject(), Subject::Pane(12));
        assert_eq!(
            ev.stream_json(0),
            json!({"type":"pane.command_finished","pane":12,"exit_code":1,
                   "ts":"1970-01-01T00:00:00.000Z"})
        );
        let n = Event::Notification {
            pane: 2,
            level: NotifyLevel::Error,
            title: None,
            message: "x".into(),
            source: "osc9".into(),
        };
        assert_eq!(n.stream_json(0)["type"], "pane.notification");
        assert_eq!(n.stream_json(0)["level"], "error");
        assert_eq!(n.stream_json(0)["source"], "osc9");
        assert!(Event::PaneExited {
            pane: 1,
            status: None
        }
        .is_removal());
    }

    #[test]
    fn status_state_and_level_parse() {
        assert_eq!("waiting".parse::<StatusState>(), Ok(StatusState::Waiting));
        assert!("busy".parse::<StatusState>().is_err());
        assert_eq!(
            "attention".parse::<NotifyLevel>(),
            Ok(NotifyLevel::Attention)
        );
        assert!("loud".parse::<NotifyLevel>().is_err());
    }

    #[test]
    fn env_fills_the_target_only_when_nothing_explicit_is_given() {
        let env = CallerEnv {
            pane: Some(7),
            tab: Some(4),
            window: Some(2),
        };
        let r = Resource::Tab;
        assert_eq!(Target::default().with_env(r, env).tab, Some(4));
        assert_eq!(Target::tab(9).with_env(r, env).tab, Some(9));
        let m = Target::matching("title:x").with_env(r, env);
        assert_eq!(m.tab, None, "--match beats the environment");
        assert_eq!(
            Target::default().with_env(Resource::Pane, env).pane,
            Some(7)
        );
        assert_eq!(
            Target::default().with_env(Resource::Window, env).window,
            Some(2)
        );
    }

    #[test]
    fn spec_example_decodes_with_defaults() {
        let line = r#"{"v":1,"id":7,"method":"pane.send_text","params":{"pane":12,"text":"ls\n"}}"#;
        let (_, r) = decode_request(line).unwrap();
        assert_eq!(
            r,
            Request::PaneSendText(SendTextParams {
                target: Target::pane(12),
                text: "ls\n".into(),
                paste: true
            })
        );
    }

    #[test]
    fn errors_carry_codes_and_ids() {
        let (id, e) = decode_request(r#"{"v":2,"id":9,"method":"mux.list"}"#).unwrap_err();
        assert_eq!((id, e.code), (Some(9), ErrorCode::VersionMismatch));
        assert_eq!(e.code.exit_code(), exit::VERSION_MISMATCH);
        let (id, e) = decode_request("{nope").unwrap_err();
        assert_eq!((id, e.code), (None, ErrorCode::BadRequest));
        let (_, e) = decode_request(r#"{"v":1,"id":1,"method":"zzz"}"#).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
        let (_, e) =
            decode_request(r#"{"v":1,"id":1,"method":"pane.send_text","params":{}}"#).unwrap_err();
        assert!(e.message.contains("pane.send_text"));
        assert_eq!(ErrorCode::NotFound.exit_code(), exit::NOT_FOUND);
        assert_eq!(ErrorCode::Ambiguous.exit_code(), 5);
        assert_eq!(ErrorCode::ConfirmRequired.exit_code(), 8);
        assert_eq!(ErrorCode::Timeout.exit_code(), 6);
        assert_eq!(ErrorCode::NeedsShellIntegration.exit_code(), 7);
        assert_eq!(ErrorCode::Busy.exit_code(), 9);
        let line = encode_response(Some(1), Err(RpcError::needs_shell_integration("x")));
        assert!(line.contains(r#""code":"needs_shell_integration""#));
        assert_eq!(
            decode_incoming(&line).unwrap(),
            Incoming::Response(Err(RpcError::needs_shell_integration("x")))
        );
    }

    #[test]
    fn long_running_requests_are_run_and_wait() {
        assert!(Request::PaneRun(RunParams::default()).is_long_running());
        assert!(Request::PaneWait(WaitParams::default()).is_long_running());
        assert!(!Request::PaneGetText(GetTextParams {
            target: Target::default(),
            start_line: None,
            end_line: None,
            escapes: false,
            last_command: None
        })
        .is_long_running());
    }

    #[test]
    fn run_and_wait_params_have_stable_json_shapes() {
        let run = Request::PaneRun(RunParams {
            spawn: SpawnParams {
                command: Some(vec!["npm".into(), "run".into(), "dev".into()]),
                title: Some("dev".into()),
                ..Default::default()
            },
            detach: true,
            ..Default::default()
        });
        assert_eq!(
            run.params(),
            json!({"command": ["npm","run","dev"], "title": "dev", "new_window": false,
                   "keep": false, "detach": true})
        );
        let wait = Request::PaneWait(WaitParams {
            target: Target::matching("title:dev"),
            matches: vec!["ready on".into()],
            ..Default::default()
        });
        assert_eq!(
            wait.params(),
            json!({"match": "title:dev", "matches": ["ready on"], "exit": false,
                   "command_finished": false, "new_only": false})
        );
    }

    #[test]
    fn iso8601_formats_unix_milliseconds() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601_utc(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
        assert_eq!(iso8601_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn ambiguous_errors_carry_candidates_over_the_wire() {
        let e = RpcError::ambiguous(
            "2 panes match",
            vec!["pane 1 \"a\"".into(), "pane 2".into()],
        );
        let line = encode_response(Some(1), Err(e.clone()));
        assert!(line.contains("candidates"));
        assert_eq!(decode_incoming(&line).unwrap(), Incoming::Response(Err(e)));
    }

    #[test]
    fn responses_and_events_decode() {
        let ok = encode_response(Some(1), Ok(json!({"text": "hi"})));
        assert_eq!(
            decode_incoming(&ok).unwrap(),
            Incoming::Response(Ok(json!({"text": "hi"})))
        );
        let err = encode_response(Some(2), Err(RpcError::not_found("pane 99 does not exist")));
        assert!(err.contains(r#""code":"not_found""#));
        assert_eq!(
            decode_incoming(&err).unwrap(),
            Incoming::Response(Err(RpcError::not_found("pane 99 does not exist")))
        );
        let ev = Event::TabMoved {
            window: 1,
            tab: 2,
            from: 0,
            to: 1,
        };
        let line = encode_event(&ev);
        assert!(!line.contains("\"id\""));
        assert_eq!(decode_incoming(&line).unwrap(), Incoming::Event(ev));
        assert_eq!(
            decode_incoming(r#"{"v":3,"id":1,"result":{}}"#)
                .unwrap_err()
                .code,
            ErrorCode::VersionMismatch
        );
    }
}
