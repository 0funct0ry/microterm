//! `microterm ctl <resource> <verb>` (SPEC §12.1): the clap definitions. The behaviour lives in
//! `ctl_cmd`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use mt_proto::{NotifyLevel, StatusState};

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

    /// Panes: list, get, send-text, get-text, run, wait, notify, status, close.
    #[command(subcommand)]
    Pane(PaneCmd),

    /// Stream mux events as JSON lines until interrupted.
    ///
    /// One object per line: {"type":"pane.command_finished","pane":12,"exit_code":1,"ts":"…"}.
    /// --type takes dotted names (workspace.*, window.*, tab.*, pane.added, pane.removed, pane.focused,
    /// pane.title_changed, pane.cwd_changed, pane.exited, pane.command_finished,
    /// pane.status_changed, pane.notification, domain.state_changed, menu.invoked); --match keeps events whose
    /// pane, tab or window the selector matches; --snapshot first prints the current tree as
    /// {"type":"snapshot","tree":{…}}. pane.output is not streamed (use `pane wait`). Exits 2
    /// when the GUI quits.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl events --type pane.command_finished,pane.notification
  microterm ctl events --match 'title:build*' --snapshot
  microterm ctl events --type 'pane.status_changed' | jq -r '.pane'")]
    Events {
        /// Comma-separated event types; a trailing `.*` matches a whole family.
        #[arg(long = "type", value_name = "T", value_delimiter = ',')]
        types: Vec<String>,
        /// Selector for the pane, tab or window an event concerns.
        #[arg(long = "match", value_name = "EXPR")]
        expr: Option<String>,
        /// Print the current tree first.
        #[arg(long)]
        snapshot: bool,
    },

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

    /// Install, remove or print the agent skill for Claude Code, Codex and others.
    ///
    /// Writes instructions that teach a coding agent to drive microterm with `ctl`: a skill
    /// directory for Claude Code, or a managed block (between `<!-- microterm:begin -->` and
    /// `<!-- microterm:end -->`) in AGENTS.md. Idempotent; content it did not write is never
    /// touched; every file written or removed is printed. Needs no running microterm.
    #[command(subcommand)]
    Skill(SkillCmd),

    /// Run an MCP server on stdio that exposes ctl as tools.
    ///
    /// For harnesses that prefer tools to shell commands. Register it as the command
    /// `microterm ctl mcp`. It talks to the running GUI over the IPC socket; tool results are the
    /// same JSON as `--json`. Stdout carries only the protocol.
    #[command(after_help = "\
EXAMPLES:
  claude mcp add microterm -- microterm ctl mcp
  # Codex, in ~/.codex/config.toml:
  #   [mcp_servers.microterm]
  #   command = \"microterm\"
  #   args = [\"ctl\", \"mcp\"]")]
    Mcp,

    /// Context menus: list, show, invoke, check (SPEC §10.9).
    #[command(subcommand)]
    Menu(MenuCmd),

    /// The macOS main menu bar: list the resolved menus (SPEC §10.7).
    #[command(subcommand, name = "menu-bar")]
    MenuBar(MenuBarCmd),
}

#[derive(Subcommand, Debug)]
pub enum MenuBarCmd {
    /// List every menu row: path, action, enabled state and shortcut chip.
    ///
    /// The GUI computes the rows from its live state; a headless server has no menu bar and
    /// prints nothing.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl menu-bar list
  microterm ctl menu-bar list --json | jq '.items[] | select(.enabled | not)'")]
    List,
}

#[derive(Subcommand, Debug)]
pub enum MenuCmd {
    /// List every menu with its facts, placeholders, allowed kinds and items.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl menu list
  microterm ctl menu list --json | jq '.[] | {id, placeholders}'")]
    List,
    /// Show a menu's rows for a pane, with `when` evaluated (no click facts).
    ///
    /// The pane defaults to $MICROTERM_PANE, else the focused pane.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl menu show viewport --pane 3")]
    Show {
        /// Menu id: viewport or tab.
        menu: String,
        #[command(flatten)]
        target: PaneTarget,
    },
    /// Run a menu item on a pane as if it was chosen.
    ///
    /// MENU/ITEM names the item (viewport/zoom, tab/close, or a configured id). The pane is
    /// --pane, --match or $MICROTERM_PANE; it never falls back to the focused pane. There are
    /// no click facts; {selection} is the pane's current selection. Exits 3 for an unknown
    /// item, 1 when its `when` does not hold, and 8 when it would ask for confirmation
    /// (pass --force). --dry-run prints the expanded argv or text and runs nothing.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl menu invoke viewport/zoom --pane 3
  microterm ctl menu invoke viewport/split_left            # inside a pane: split it
  microterm ctl menu invoke viewport/blame --pane 3 --dry-run --json")]
    Invoke {
        /// MENU/ITEM, e.g. viewport/zoom.
        #[arg(value_name = "MENU/ITEM")]
        item: String,
        #[command(flatten)]
        target: PaneTarget,
        /// Resolve and expand, but do not run.
        #[arg(long)]
        dry_run: bool,
        /// Run an item that would ask for confirmation.
        #[arg(long)]
        force: bool,
    },
    /// List the icon names an item can use (`icon = "…"`); needs no running instance.
    ///
    /// One name per line; --json prints an array.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl menu icons
  microterm ctl menu icons | grep git")]
    Icons,
    /// Validate the menu config of a config file; needs no running instance.
    ///
    /// Prints every diagnostic with its line and column and exits 1 on an error, like
    /// `microterm config check`.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl menu check
  microterm ctl menu check --config ./microterm.toml")]
    Check {
        /// Config file to check (default: the one microterm loads).
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SkillCmd {
    /// Write the skill (or its managed block) for the chosen agent(s).
    Install(SkillOpts),
    /// Remove exactly what `install` wrote.
    Uninstall(SkillOpts),
    /// Print the rendered skill without writing anything.
    Show(SkillOpts),
}

#[derive(Args, Debug, Clone)]
pub struct SkillOpts {
    /// Which agent to write for. antigravity currently falls back to generic (AGENTS.md).
    #[arg(long, value_enum, default_value_t = crate::agent::Agent::Claude)]
    pub agent: crate::agent::Agent,
    /// user (your home directory) or project (the current directory). Default: user for
    /// claude and codex, project for generic and antigravity.
    #[arg(long, value_enum)]
    pub scope: Option<crate::agent::Scope>,
    /// Use this directory instead of the scope's: the skills directory for claude, the
    /// directory holding AGENTS.md for the others.
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    /// Print what would be written instead of writing it.
    #[arg(long)]
    pub print: bool,
}

#[derive(Subcommand, Debug)]
pub enum StatusCmd {
    /// Set the status (default: $MICROTERM_PANE, never the focused pane).
    ///
    /// States: idle (no dot), working (accent), waiting (warning: "needs a human"), done
    /// (success), error. Text is stripped of control characters and cut to ctl.status_max_len.
    /// With --ttl the status clears itself, so a crashed agent leaves no stale marker. A status
    /// also clears when the pane's process exits.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl pane status set --state working --progress 40 'running tests'
  microterm ctl pane status set --state waiting --ttl 30s 'approve the deploy?'")]
    Set {
        #[command(flatten)]
        target: PaneTarget,
        /// idle, working, waiting, done or error (default: working).
        #[arg(long, value_name = "STATE")]
        state: Option<StatusState>,
        /// 0-100; draws a progress line along the tab's bottom edge.
        #[arg(long, value_name = "PCT", value_parser = clap::value_parser!(u8).range(0..=100))]
        progress: Option<u8>,
        /// Clear the status after DUR (e.g. 30s, 5m).
        #[arg(long, value_name = "DUR", value_parser = parse_duration_ms)]
        ttl: Option<u64>,
        /// Set the status of every pane the selector matches.
        #[arg(long)]
        all: bool,
        text: Option<String>,
    },
    /// Remove the status.
    Clear {
        #[command(flatten)]
        target: PaneTarget,
        /// Clear every pane the selector matches.
        #[arg(long)]
        all: bool,
    },
    /// Print the status (default: $MICROTERM_PANE, else the focused pane).
    ///
    /// With --json: {"pane":12,"status":{"state","text","progress","updated_at"}|null}.
    Get(PaneTarget),
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
        /// Color the tab: blue, purple, pink, red, orange, yellow, green, teal, gray or '#rrggbb'.
        #[arg(long, value_name = "COLOR")]
        color: Option<String>,
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
    /// Color a tab: a name (blue, purple, pink, red, orange, yellow, green, teal, gray) or
    /// '#rrggbb'.
    ///
    /// Sets the manual color, which beats colors declared in the config and `color_rules`.
    /// --clear removes only the manual color and reports what applies now.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl tab color orange
  microterm ctl tab color --match 'title:*build*' teal
  microterm ctl tab color '#f59e42'
  microterm ctl tab color --clear")]
    Color {
        #[command(flatten)]
        target: TabTarget,
        /// A color name or '#rrggbb'.
        #[arg(
            value_name = "COLOR",
            conflicts_with = "clear",
            required_unless_present = "clear"
        )]
        color: Option<String>,
        /// Remove the manual color.
        #[arg(long)]
        clear: bool,
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
        /// Print the output of the N-th most recent finished command (default 1), cut out
        /// with the shell's OSC 133 marks. Needs shell integration (exit 7 without it).
        #[arg(
            long,
            value_name = "N",
            num_args = 0..=1,
            default_missing_value = "1",
            conflicts_with_all = ["start_line", "end_line", "escapes"]
        )]
        last_command: Option<u32>,
    },
    /// Run a command and return its output and exit status.
    ///
    /// Without --in a new tab runs the command directly (argv; use `-- sh -c '…'` for shell
    /// syntax) and the call returns when it exits; the tab closes unless --keep is given.
    /// With --in, the command line is typed into an existing shell pane that has shell
    /// integration (exit 7 without it, 9 while a command is running) and the output is exactly
    /// what that command printed. A timeout (exit 6) never kills the command.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl pane run --json -- cargo test
  microterm ctl pane run --detach --title dev -- npm run dev
  microterm ctl pane run --in 12 -- git status --short
  microterm ctl pane run --propagate-exit -- make check")]
    Run {
        /// Window for the new tab.
        #[arg(long, value_name = "ID", conflicts_with = "new_window")]
        window: Option<u64>,
        /// Open a new window for the tab.
        #[arg(long)]
        new_window: bool,
        #[arg(long, value_name = "NAME")]
        workspace: Option<String>,
        /// An `[[ssh_domains]]` name (or `ssh:HOST`); default: local.
        #[arg(long, value_name = "NAME")]
        domain: Option<String>,
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// Keep the tab after the process exits, showing `[process exited with status N]`.
        #[arg(long)]
        keep: bool,
        /// Return `{pane, tab}` at once; follow with `pane wait`.
        #[arg(long)]
        detach: bool,
        /// Give up waiting after DUR (500ms, 2s, 5m); the command keeps running (exit 6).
        #[arg(long, value_name = "DUR", value_parser = parse_duration_ms)]
        timeout: Option<u64>,
        /// Exit with the command's own status instead of 0 (statuses that collide with the
        /// codes of `microterm ctl` become 1; the real value stays in --json).
        #[arg(long)]
        propagate_exit: bool,
        /// Type the command line into this shell pane (an id or a selector; default:
        /// $MICROTERM_PANE) instead of opening a tab.
        #[arg(
            long = "in",
            value_name = "TARGET",
            num_args = 0..=1,
            default_missing_value = "",
            conflicts_with_all = ["window", "new_window", "workspace", "domain", "cwd", "title", "keep", "detach"]
        )]
        in_pane: Option<String>,
        /// The command and its arguments, after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Wait until a pane output, idle, exit or finished-command condition holds.
    ///
    /// Conditions may be combined; the first to hold wins. `--match` takes a regular
    /// expression over the pane's rendered text (soft wraps joined, scrollback and screen, then
    /// new output) -- or, when the value reads as a pane selector such as `title:dev`, picks
    /// the pane (as TARGET, `--pane` and `--select` do). Use `--text` for a regular expression
    /// that looks like a selector. Exit 6 on timeout (default 60s), 3 when the pane closes
    /// first, 7 when --command-finished needs shell integration the pane does not have.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl pane wait --match title:dev --match 'ready on' --timeout 120s
  microterm ctl pane wait --pane 12 --command-finished
  microterm ctl pane wait --idle 2s --new-only --match 'error|warning'
  microterm ctl pane wait --exit --json")]
    Wait {
        /// Pane id or selector (default: $MICROTERM_PANE, else the focused pane).
        #[arg(value_name = "TARGET")]
        target: Option<String>,
        #[arg(long, value_name = "ID")]
        pane: Option<u64>,
        /// A selector for the pane.
        #[arg(long, value_name = "EXPR")]
        select: Option<String>,
        /// A regular expression to wait for (or, if it reads as a selector, the pane).
        #[arg(long = "match", value_name = "REGEX")]
        matches: Vec<String>,
        /// A regular expression to wait for, never read as a selector.
        #[arg(long, value_name = "REGEX")]
        text: Vec<String>,
        /// No PTY output for DUR.
        #[arg(long, value_name = "DUR", value_parser = parse_duration_ms)]
        idle: Option<u64>,
        /// The pane's process has exited.
        #[arg(long)]
        exit: bool,
        /// The next command finishes (OSC 133 `D`).
        #[arg(long)]
        command_finished: bool,
        /// Give up after DUR (default 60s).
        #[arg(long, value_name = "DUR", value_parser = parse_duration_ms)]
        timeout: Option<u64>,
        /// Ignore what is already on screen or in scrollback.
        #[arg(long)]
        new_only: bool,
    },
    /// Tell the human something: attention badge, toast and OS notification.
    ///
    /// Raises a badge on the pane's tab (colour by level) until the pane is focused, and per
    /// [notifications] shows a toast when the pane is not visible and an OS notification when
    /// microterm is unfocused; --desktop forces the latter. Limited to 5 per second per pane;
    /// extras are dropped and counted. Defaults to $MICROTERM_PANE, never the focused pane.
    #[command(after_help = "\
EXAMPLES:
  microterm ctl pane notify --level attention 'tests done'
  microterm ctl pane notify --title build --level error --desktop 'failed'")]
    Notify {
        #[command(flatten)]
        target: PaneTarget,
        #[arg(long)]
        title: Option<String>,
        /// info (accent badge), attention (warning) or error.
        #[arg(long, value_name = "LEVEL", default_value = "info")]
        level: NotifyLevel,
        /// Always raise an OS notification.
        #[arg(long)]
        desktop: bool,
        /// Notify every pane the selector matches.
        #[arg(long)]
        all: bool,
        message: Option<String>,
    },
    /// Show what a process is doing on its tab: status dot, progress line and tooltip.
    #[command(subcommand)]
    Status(StatusCmd),
    /// Split a pane in two; the new pane takes focus (SPEC §17.1).
    ///
    /// --horizontal puts the new pane to the right, --vertical below; --direction picks any
    /// side (left and up put it before the target). --percent is the new pane's share
    /// (default 50). The new pane inherits the target's domain and cwd. Prints the new pane's
    /// ids like `tab new`.
    #[command(
        group = clap::ArgGroup::new("axis").required(true),
        after_help = "\
EXAMPLES:
  microterm ctl pane split --horizontal                    # inside a pane: split it
  microterm ctl pane split --pane 3 --direction left       # new pane left of pane 3
  eval \"$(microterm ctl pane split --vertical --percent 30 -- htop)\"   # sets tab= pane= window="
    )]
    Split {
        #[command(flatten)]
        target: PaneTarget,
        /// Side by side: the new pane goes to the right.
        #[arg(long, group = "axis")]
        horizontal: bool,
        /// One above the other: the new pane goes below.
        #[arg(long, group = "axis")]
        vertical: bool,
        /// The side of the target the new pane goes on.
        #[arg(long, value_enum, group = "axis")]
        direction: Option<DirectionArg>,
        /// Share of the space the new pane takes, 1 to 99.
        #[arg(long, value_name = "N")]
        percent: Option<u8>,
        /// Working directory of the new pane (default: the target's).
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        /// Run this instead of the shell, after `--`.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Focus a pane, or the neighbour of one in a direction.
    ///
    /// Exits 3 when there is no pane that way.
    Focus {
        #[command(flatten)]
        target: PaneTarget,
        /// Move focus to the pane on this side of the target (default: focus the target).
        #[arg(long, value_enum)]
        direction: Option<DirectionArg>,
    },
    /// Show a pane full-size in its tab, or restore the layout.
    Zoom(PaneTarget),
    /// Move a pane's border in a direction by a number of cells.
    Resize {
        #[command(flatten)]
        target: PaneTarget,
        #[arg(long, value_enum)]
        direction: DirectionArg,
        /// How far to move it, in terminal cells.
        #[arg(long, value_name = "N", default_value_t = 1)]
        cells: u32,
    },
    /// Swap a pane with its neighbour in a direction.
    Swap {
        #[command(flatten)]
        target: PaneTarget,
        #[arg(long, value_enum)]
        direction: DirectionArg,
    },
    /// Rotate the positions of the panes in a tab by one.
    Rotate(PaneTarget),
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

/// A side of a pane.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionArg {
    Left,
    Right,
    Up,
    Down,
}

impl From<DirectionArg> for mt_proto::Direction {
    fn from(d: DirectionArg) -> Self {
        match d {
            DirectionArg::Left => Self::Left,
            DirectionArg::Right => Self::Right,
            DirectionArg::Up => Self::Up,
            DirectionArg::Down => Self::Down,
        }
    }
}

/// `500ms`, `2s`, `5m` or `1h`; a bare number is seconds.
pub fn parse_duration_ms(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num
        .parse()
        .map_err(|_| format!("{s:?} is not a duration (try 500ms, 2s or 5m)"))?;
    let factor = match unit {
        "ms" => 1.0,
        "" | "s" => 1_000.0,
        "m" => 60_000.0,
        "h" => 3_600_000.0,
        _ => {
            return Err(format!(
                "unknown duration unit {unit:?} in {s:?} (ms, s, m, h)"
            ))
        }
    };
    Ok((n * factor).round() as u64)
}

impl CtlArgs {
    /// The top-level command names, for the shell's syntax highlighting.
    pub fn command_names() -> Vec<String> {
        use clap::CommandFactory;
        Self::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect()
    }

    /// The verbs of a resource (`pane` → `list`, `get`, …); empty for a leaf command.
    pub fn subcommand_names(resource: &str) -> Vec<String> {
        use clap::CommandFactory;
        Self::command()
            .find_subcommand(resource)
            .map(|c| {
                c.get_subcommands()
                    .map(|s| s.get_name().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_with_units() {
        assert_eq!(parse_duration_ms("500ms"), Ok(500));
        assert_eq!(parse_duration_ms("2s"), Ok(2_000));
        assert_eq!(parse_duration_ms("5m"), Ok(300_000));
        assert_eq!(parse_duration_ms("1.5s"), Ok(1_500));
        assert_eq!(parse_duration_ms("2"), Ok(2_000));
        assert_eq!(parse_duration_ms("1h"), Ok(3_600_000));
        assert!(parse_duration_ms("fast").is_err());
        assert!(parse_duration_ms("5x").is_err());
        assert!(parse_duration_ms("").is_err());
    }
}
