//! `microterm ctl <resource> <verb>` (SPEC §12.1): the clap definitions. The behaviour lives in
//! `ctl_cmd`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "ctl",
    after_help = "\
EXAMPLES:
  microterm ctl list
  eval \"$(microterm ctl tab new --title build -- cargo watch -x test)\"   # sets tab= pane= window=
  microterm ctl pane get-text --match 'title:build*'
  microterm ctl pane send-text --pane \"$pane\" 'echo hello'
  microterm ctl tab close --match 'title:build*' --force
  microterm ctl tab close --force        # inside a pane: closes its own tab
  microterm ctl workspace switch infra

Run `microterm ctl <RESOURCE> --help` for the verbs of each resource."
)]
pub struct CtlArgs {
    /// Print machine-readable JSON instead of text (errors too, on stderr).
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub action: CtlAction,
}

#[derive(Subcommand, Debug)]
pub enum CtlAction {
    /// Show workspaces, windows, tabs and panes as a tree.
    ///
    /// With --json the result is
    /// {"workspaces":[{"id","name","active","windows":[{"id","title","active","focused",
    /// "tabs":[{"id","title","active","focused","panes":[<pane>]}]}]}]}
    /// where <pane> has the shape printed by `ctl pane get --json`.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl list
  microterm ctl list --json | jq '.workspaces[].windows[].tabs[].panes[].id'")]
    List,

    /// Workspaces: list, create, switch, rename, delete.
    #[command(subcommand)]
    Workspace(WorkspaceCmd),

    /// Windows: list, new, focus, close.
    #[command(subcommand)]
    Window(WindowCmd),

    /// Tabs: list, new, focus, rename, close.
    #[command(subcommand)]
    Tab(TabCmd),

    /// Panes: list, get, send-text, get-text, close.
    #[command(subcommand)]
    Pane(PaneCmd),

    /// Record a running pane to an asciicast v2 file.
    ///
    /// Records until the pane's process exits, --duration passes or Ctrl-C. Output only unless
    /// --input is given; input recording captures keystrokes, including anything typed at a
    /// password prompt, so it is opt-in.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl record --pane \"$pane\" -o session.cast --duration 30
  microterm ctl record -o inside.cast        # inside a pane: records that pane")]
    Record {
        /// Pane to record (default: $MICROTERM_PANE).
        #[arg(long, value_name = "ID")]
        pane: Option<u64>,
        /// File to write (asciicast v2). Overwritten if it exists.
        #[arg(short = 'o', value_name = "FILE.cast")]
        output: PathBuf,
        /// Stop after this many seconds.
        #[arg(long, value_name = "SECS")]
        duration: Option<u64>,
        /// Also record everything sent to the program (keys, pastes).
        #[arg(long)]
        input: bool,
    },

    /// Play a .cast file into a new tab.
    ///
    /// Prints `tab= pane= window=` of the new tab. The tab stays open after the replay ends.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl replay session.cast
  microterm ctl replay session.cast --speed 2 --new-window")]
    Replay {
        file: PathBuf,
        /// Playback speed multiplier.
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Open the replay in a new window instead of a new tab.
        #[arg(long)]
        new_window: bool,
    },

    /// Reload the config file now.
    ReloadConfig,
}

/// How a verb names its item: an explicit ID or a selector.
#[derive(Args, Debug, Default, Clone)]
pub struct PaneTarget {
    /// Pane ID (default: $MICROTERM_PANE).
    #[arg(long, value_name = "ID")]
    pub pane: Option<u64>,
    /// Selector instead of an ID, e.g. 'title:build*,domain:local' (see `ctl --help`).
    #[arg(long = "match", value_name = "EXPR")]
    pub expr: Option<String>,
}

#[derive(Args, Debug, Default, Clone)]
pub struct TabTarget {
    /// Tab ID (default: $MICROTERM_TAB).
    #[arg(long, value_name = "ID")]
    pub tab: Option<u64>,
    /// Selector instead of an ID, e.g. 'title:build*,domain:local' (see `ctl --help`).
    #[arg(long = "match", value_name = "EXPR")]
    pub expr: Option<String>,
}

#[derive(Args, Debug, Default, Clone)]
pub struct WindowTarget {
    /// Window ID (default: $MICROTERM_WINDOW).
    #[arg(long, value_name = "ID")]
    pub window: Option<u64>,
    /// Selector instead of an ID, e.g. 'workspace:api' (see `ctl --help`).
    #[arg(long = "match", value_name = "EXPR")]
    pub expr: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceCmd {
    /// List workspaces (the active one is marked).
    List,
    /// Create a workspace (it stays empty until something is opened in it).
    Create { name: String },
    /// Make a workspace the active one, opening its windows.
    Switch { name: String },
    /// Rename a workspace. The `default` workspace cannot be renamed.
    Rename { old: String, new: String },
    /// Delete a workspace and close everything in it.
    ///
    /// The `default` and the active workspace cannot be deleted. If `confirm_close` would ask
    /// first, this exits 8 without closing anything unless --force is given.
    Delete {
        name: String,
        /// Delete without regard to confirm_close.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum WindowCmd {
    /// List windows.
    List {
        /// Only windows matching this selector.
        #[arg(long = "match", value_name = "EXPR")]
        expr: Option<String>,
    },
    /// Open a new window with one tab and print `tab= pane= window=`.
    New {
        /// Open it in this workspace (created if missing, and switched to).
        #[arg(long, value_name = "NAME")]
        workspace: Option<String>,
        /// Working directory of the first tab.
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        /// Command to run instead of the shell, after `--`.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Bring a window to the front.
    Focus(WindowTarget),
    /// Close a window and everything in it.
    ///
    /// Exits 8 without closing anything when confirm_close would ask, unless --force is given.
    Close {
        #[command(flatten)]
        target: WindowTarget,
        /// Close without regard to confirm_close.
        #[arg(long)]
        force: bool,
        /// Close every window the selector matches.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum TabCmd {
    /// List tabs.
    List {
        /// Only tabs of this window.
        #[arg(long, value_name = "ID")]
        window: Option<u64>,
        /// Only tabs matching this selector.
        #[arg(long = "match", value_name = "EXPR")]
        expr: Option<String>,
    },
    /// Open a tab and print `tab= pane= window=` (safe to `eval`).
    ///
    /// Without --window the tab goes to the focused window of the target workspace and becomes
    /// its active tab. --workspace targets another workspace, creating it if needed.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl tab new
  eval \"$(microterm ctl tab new --title build -- cargo watch -x test)\"
  microterm ctl tab new --domain prod --cwd /srv/app")]
    New {
        #[arg(long, value_name = "ID")]
        window: Option<u64>,
        #[arg(long, value_name = "NAME")]
        workspace: Option<String>,
        /// An `[[ssh_domains]]` name (or `ssh:HOST`); default: local.
        #[arg(long, value_name = "NAME")]
        domain: Option<String>,
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// Command to run instead of the shell, after `--`.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Make a tab active: by target, by position, or relative to the current one.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl tab focus --match 'title:build*'
  microterm ctl tab focus --index 0
  microterm ctl tab focus --relative -1")]
    Focus {
        #[command(flatten)]
        target: TabTarget,
        /// Zero-based position within the window.
        #[arg(long, value_name = "N", conflicts_with_all = ["relative", "tab", "expr"])]
        index: Option<usize>,
        /// Move by N tabs from the active one (wraps around).
        #[arg(long, value_name = "N", allow_hyphen_values = true, conflicts_with_all = ["tab", "expr"])]
        relative: Option<i64>,
        /// Window for --index/--relative (default: $MICROTERM_WINDOW, else the focused one).
        #[arg(long, value_name = "ID")]
        window: Option<u64>,
    },
    /// Set the title of a tab. An empty title restores the automatic one.
    Rename {
        #[command(flatten)]
        target: TabTarget,
        title: String,
    },
    /// Close a tab.
    ///
    /// Exits 8 without closing anything when confirm_close would ask, unless --force is given.
    /// Inside a pane, `microterm ctl tab close --force` closes the pane's own tab.
    Close {
        #[command(flatten)]
        target: TabTarget,
        /// Close without regard to confirm_close.
        #[arg(long)]
        force: bool,
        /// Close every tab the selector matches.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PaneCmd {
    /// List panes.
    ///
    /// With --json: [{"id","tab","window","workspace","domain","title","cwd","process",
    /// "size":{"cols","rows"},"focused","exited","exit_code"}].
    List {
        /// Only panes of this window.
        #[arg(long, value_name = "ID")]
        window: Option<u64>,
        /// Only panes of this tab.
        #[arg(long, value_name = "ID")]
        tab: Option<u64>,
        /// Only panes matching this selector.
        #[arg(long = "match", value_name = "EXPR")]
        expr: Option<String>,
    },
    /// Show one pane (default: $MICROTERM_PANE, else the focused pane).
    Get(PaneTarget),
    /// Type text into a pane (default: $MICROTERM_PANE; never the focused pane).
    ///
    /// The text is pasted (bracketed when the program asked for it) unless --no-paste is given,
    /// which types the bytes as they are. Reads stdin when TEXT is omitted.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl pane send-text --pane 12 'cargo test'
  printf 'ls\\n' | microterm ctl pane send-text --pane 12 --no-paste
  microterm ctl pane send-text --match 'title:build*' --all $'\\x03'")]
    SendText {
        #[command(flatten)]
        target: PaneTarget,
        /// Type the bytes instead of pasting them.
        #[arg(long)]
        no_paste: bool,
        /// Send to every pane the selector matches.
        #[arg(long)]
        all: bool,
        /// Text to send; stdin when omitted.
        text: Option<String>,
    },
    /// Print a pane's text (default: $MICROTERM_PANE, else the focused pane).
    ///
    /// Lines are numbered from the top visible row (0); negative numbers reach into scrollback.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl pane get-text --pane 12
  microterm ctl pane get-text --pane 12 --start-line -200 --end-line -1
  microterm ctl pane get-text --pane 12 --escapes > styled.txt")]
    GetText {
        #[command(flatten)]
        target: PaneTarget,
        #[arg(long, value_name = "N", allow_hyphen_values = true)]
        start_line: Option<i64>,
        #[arg(long, value_name = "N", allow_hyphen_values = true)]
        end_line: Option<i64>,
        /// Re-encode colours and styles as SGR escapes.
        #[arg(long)]
        escapes: bool,
    },
    /// Close a pane; closing a tab's last pane closes the tab, a window's last tab the window.
    ///
    /// Exits 8 without closing anything when confirm_close would ask, unless --force is given.
    Close {
        #[command(flatten)]
        target: PaneTarget,
        /// Close without regard to confirm_close.
        #[arg(long)]
        force: bool,
        /// Close every pane the selector matches.
        #[arg(long)]
        all: bool,
    },
}
