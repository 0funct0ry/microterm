//! Command-line surface (SPEC §12.1). Everything except launching the GUI is a stub in M1.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "microterm",
    version,
    about = "A minimal GPU-accelerated terminal emulator"
)]
pub struct Cli {
    /// Path to the config file.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Start in this workspace.
    #[arg(long, value_name = "NAME")]
    pub workspace: Option<String>,
    /// Working directory of the first tab.
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    /// Do not reuse a running instance.
    #[arg(long)]
    pub new_instance: bool,
    /// Run the mux and IPC server without windows (used by the end-to-end tests).
    #[arg(long, hide = true)]
    pub headless: bool,
    /// Run a command in the first tab.
    #[arg(short = 'e', num_args = 1.., value_name = "CMD", trailing_var_arg = true, allow_hyphen_values = true)]
    pub exec: Vec<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Open an SSH tab.
    Ssh(SshArgs),
    /// Show which font renders each character.
    LsFonts {
        #[arg(long)]
        text: Option<String>,
    },
    /// List and inspect themes.
    #[command(subcommand)]
    Themes(ThemesCmd),
    /// Inspect and edit the config file.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Print shell completions.
    Completions { shell: Shell },
    /// Run a command in a new tab and record it (asciicast v2).
    ///
    /// The command starts only once recording is armed, so no output is lost, and the
    /// recording ends when the command exits.
    Record(RecordArgs),
    /// Script a running microterm instance.
    ///
    /// Every subcommand talks to the instance named by $MICROTERM_UNIX_SOCKET (set in every pane),
    /// or else the newest live instance of this user. Panes are addressed with --pane ID and default
    /// to $MICROTERM_PANE, so a command run inside a tab targets that tab. Use `microterm cli list`
    /// to find ids.
    ///
    /// With --json each command prints the raw result of the underlying IPC call (one JSON line)
    /// instead of text.
    ///
    /// EXIT CODES: 0 ok, 1 usage or other error, 2 no running instance, 3 target not found
    /// (pane, tab, window, workspace, file), 4 protocol version mismatch.
    Cli(CliArgs),
    /// Internal: plays a `.cast` file on stdout (what `cli replay` runs in the new tab).
    #[command(name = "__replay", hide = true)]
    ReplayInternal {
        file: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
    },
}

#[derive(Args, Debug)]
pub struct SshArgs {
    /// `[user@]host[:port]`; resolved through `~/.ssh/config` too.
    #[arg(required_unless_present = "domain")]
    pub target: Option<String>,
    /// Use the `[[ssh_domains]]` entry with this name instead of an ad-hoc destination.
    #[arg(long, value_name = "NAME")]
    pub domain: Option<String>,
}

impl SshArgs {
    /// The domain string the app resolves: a configured name or `ssh:<destination>`.
    pub fn domain_ref(&self) -> String {
        match (&self.domain, &self.target) {
            (Some(d), _) => d.clone(),
            (None, Some(t)) => format!("ssh:{t}"),
            (None, None) => String::new(),
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ThemesCmd {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    Path,
    Check,
    Edit,
    ShowDefaults,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Args, Debug)]
#[command(after_help = "\
EXAMPLES:
  microterm record -o build.cast -- cargo build
  microterm record -o demo.cast --cols 100 --rows 30 -- htop
  microterm record -o session.cast --input -- ssh-like-command   # also records keystrokes

Needs a running microterm (exit code 2 otherwise). Play the file back with
`microterm cli replay build.cast` or `asciinema play build.cast`.")]
pub struct RecordArgs {
    /// File to write (asciicast v2). Overwritten if it exists.
    #[arg(short = 'o', value_name = "FILE.cast")]
    pub output: PathBuf,
    /// Also record everything sent to the program (keys, pastes), not only its output.
    ///
    /// Off by default because input includes anything you type: passwords at a prompt, pasted
    /// tokens. Treat a recording made with --input like a keylogger's log.
    #[arg(long)]
    pub input: bool,
    /// Requested terminal width in columns (the GUI sizes panes from the window).
    #[arg(long)]
    pub cols: Option<u16>,
    /// Requested terminal height in rows (the GUI sizes panes from the window).
    #[arg(long)]
    pub rows: Option<u16>,
    /// The command to run and its arguments, after `--`.
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Args, Debug)]
#[command(after_help = "\
EXAMPLES:
  microterm cli list
  pane=$(microterm cli spawn --title build -- cargo watch -x test)
  microterm cli get-text --pane \"$pane\"
  microterm cli send-text --pane \"$pane\" 'echo hello'
  microterm cli workspace switch infra

Run `microterm cli <COMMAND> --help` for the details and examples of each command.")]
pub struct CliArgs {
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub action: CliAction,
}

#[derive(Subcommand, Debug)]
pub enum CliAction {
    /// Show workspaces, windows, tabs and panes.
    ///
    /// Prints a tree of everything the instance holds: the workspaces (the active one is
    /// marked), their windows, each window's tabs (the active tab is marked) and each tab's
    /// panes with their domain and working directory. This is how you find the ids the other
    /// commands take.
    ///
    /// With --json the result is
    /// {"workspaces":[{"id","name","active","windows":[{"id","title","tabs":[{"id","title",
    /// "active","panes":[{"id","title","cwd","domain"}]}]}]}]}.
    #[command(after_help = "\
EXAMPLES:
  microterm cli list
  microterm cli list --json | jq '.workspaces[].windows[].tabs[].panes[].id'")]
    List,

    /// Open a tab (or window) and print its pane id.
    ///
    /// Without --window the tab is added to the first window of the target workspace and
    /// becomes the active tab. --workspace targets another workspace, creating it if it does
    /// not exist, and switches the GUI to it (one workspace is active per instance). The first
    /// tab spawned into a workspace that has no windows becomes that workspace's first tab.
    ///
    /// The new pane id is printed (with --json: {"pane","tab","window"}), which is what the
    /// other commands take as --pane. The command after `--` runs instead of the shell; the
    /// tab closes when it exits.
    #[command(after_help = "\
EXAMPLES:
  microterm cli spawn                                  # a shell in a new tab
  microterm cli spawn --title logs -- tail -f /var/log/system.log
  microterm cli spawn --cwd ~/src/api -- cargo watch -x test
  microterm cli spawn --workspace infra --title k9s -- k9s
  microterm cli spawn --new-window -- htop
  pane=$(microterm cli spawn -- sh -c 'echo hi; sleep 60') && echo \"pane $pane\"")]
    Spawn {
        /// Add the tab to this window id (from `cli list`) instead of the first window.
        #[arg(long, value_name = "ID", conflicts_with = "new_window")]
        window: Option<u64>,
        /// Open a new window holding the tab.
        #[arg(long)]
        new_window: bool,
        /// Workspace to spawn into; created and switched to when needed.
        #[arg(long, value_name = "NAME")]
        workspace: Option<String>,
        /// Domain to start the pane in: `local`, a configured SSH domain, or `ssh:[user@]host[:port]`.
        #[arg(long, value_name = "NAME")]
        domain: Option<String>,
        /// Working directory of the new pane (default: the shell's default).
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        /// Fixed tab title (overrides the automatic title).
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// Command and arguments to run instead of the shell, after `--`.
        #[arg(last = true, value_name = "CMD")]
        command: Vec<String>,
    },

    /// Send text to a pane as if pasted (or typed with --no-paste).
    ///
    /// TEXT is sent to the pane; when TEXT is omitted it is read from stdin. By default the
    /// text is *pasted*: wrapped in bracketed-paste markers when the running program turned
    /// that mode on (so shells and editors treat it as one paste instead of running it line by
    /// line), and with newlines turned into carriage returns otherwise.
    ///
    /// --no-paste types the bytes exactly as given, which is what you want for control
    /// characters such as Ctrl-C ($'\x03') or to run a command by ending it with a newline.
    #[command(after_help = "\
EXAMPLES:
  microterm cli send-text --pane 3 'echo hello'          # pasted at the prompt, not run
  microterm cli send-text --no-paste --pane 3 $'ls -la\\n' # typed, so the newline runs it
  microterm cli send-text --no-paste --pane 3 $'\\x03'     # Ctrl-C
  cat script.sh | microterm cli send-text --pane 3       # TEXT from stdin
  microterm cli send-text 'date'                         # --pane defaults to $MICROTERM_PANE")]
    SendText {
        /// Pane id (default: $MICROTERM_PANE).
        #[arg(long, value_name = "ID")]
        pane: Option<u64>,
        /// Type the bytes exactly as given instead of pasting them.
        #[arg(long)]
        no_paste: bool,
        /// Text to send; read from stdin when omitted.
        text: Option<String>,
    },

    /// Print the text of a pane: the screen, or any range of its scrollback.
    ///
    /// Line 0 is the top visible row; negative numbers reach into scrollback (-1 is the row just
    /// above the screen). The default range is the visible screen. --end-line is inclusive.
    /// Rows that soft-wrap are joined into one line and trailing blank lines are dropped.
    ///
    /// --escapes keeps the colours and text styles, re-encoded as SGR escape sequences. With
    /// --json the output is {"text": "..."}.
    #[command(after_help = "\
EXAMPLES:
  microterm cli get-text --pane 3                        # the visible screen
  microterm cli get-text --pane 3 --start-line -200      # last 200 scrollback lines + screen
  microterm cli get-text --pane 3 --start-line -10 --end-line -1   # just above the screen
  microterm cli get-text --pane 3 --start-line -1000 --escapes | less -R
  microterm cli get-text --json | jq -r .text            # this pane (via $MICROTERM_PANE)")]
    GetText {
        /// Pane id (default: $MICROTERM_PANE).
        #[arg(long, value_name = "ID")]
        pane: Option<u64>,
        /// First line; 0 is the top visible row, negative numbers reach into scrollback.
        #[arg(long, allow_hyphen_values = true, value_name = "N")]
        start_line: Option<i64>,
        /// Last line (inclusive).
        #[arg(long, allow_hyphen_values = true, value_name = "N")]
        end_line: Option<i64>,
        /// Keep colours and styles, encoded as SGR escape sequences.
        #[arg(long)]
        escapes: bool,
    },

    /// Make a tab the active one.
    ///
    /// Choose the tab by id (--tab-id, from `cli list`), by position in the window (--tab-index,
    /// counting from 0) or relative to the active tab (--tab-relative; negative goes left, and
    /// both directions wrap around). Exactly one of the three is required. If the tab belongs
    /// to another workspace, the GUI switches to it.
    #[command(after_help = "\
EXAMPLES:
  microterm cli activate-tab --tab-index 0               # the first tab
  microterm cli activate-tab --tab-relative 1            # next tab, wrapping
  microterm cli activate-tab --tab-relative -1           # previous tab
  microterm cli activate-tab --tab-id 7")]
    ActivateTab {
        /// Tab id (from `cli list`).
        #[arg(long, value_name = "ID")]
        tab_id: Option<u64>,
        /// Position in the window, counting from 0.
        #[arg(long, value_name = "N")]
        tab_index: Option<usize>,
        /// Offset from the active tab; negative goes left. Wraps around.
        #[arg(long, allow_hyphen_values = true, value_name = "N")]
        tab_relative: Option<i32>,
    },

    /// Give a tab a fixed title (or clear it with an empty title).
    ///
    /// Without --tab-id the active tab of the first window is renamed. The title replaces the
    /// automatic one (shell title / process name) until it is cleared by passing an empty
    /// string.
    #[command(after_help = "\
EXAMPLES:
  microterm cli set-tab-title 'api server'               # the active tab
  microterm cli set-tab-title --tab-id 7 deploy
  microterm cli set-tab-title --tab-id 7 ''              # back to the automatic title")]
    SetTabTitle {
        /// Tab id (default: the active tab).
        #[arg(long, value_name = "ID")]
        tab_id: Option<u64>,
        /// The new title; empty removes the override.
        title: String,
    },

    /// Close a pane and kill its process.
    ///
    /// Closes the pane's tab, and its window when it was the last tab. Any recording of the pane
    /// is finished first. Exits with code 3 when the pane does not exist.
    #[command(after_help = "\
EXAMPLES:
  microterm cli kill-pane --pane 3
  microterm cli kill-pane                                # the pane this command runs in")]
    KillPane {
        /// Pane id (default: $MICROTERM_PANE).
        #[arg(long, value_name = "ID")]
        pane: Option<u64>,
    },

    /// List, switch, create and rename workspaces.
    ///
    /// Workspaces group windows, like tmux sessions: one is active per instance, and switching
    /// hides the windows of the old one (their shells keep running) and shows the new one's,
    /// in the same place on screen. `default` always exists and cannot be renamed.
    #[command(
        subcommand,
        after_help = "\
EXAMPLES:
  microterm cli workspace list
  microterm cli workspace create infra
  microterm cli workspace switch infra
  microterm cli workspace rename infra ops"
    )]
    Workspace(WorkspaceCmd),

    /// Record a running pane to an asciicast v2 file.
    ///
    /// Recording starts when the command is sent and ends when the pane closes, after
    /// --duration seconds, or when you press Ctrl-C; the file is complete when the command
    /// returns. Only output is recorded unless you pass --input, which also records every key
    /// and paste sent to the pane. Think twice before using --input: it captures typed
    /// passwords and pasted tokens.
    ///
    /// To record a command from its very first byte of output use `microterm record` instead.
    /// Play recordings with `microterm cli replay FILE` or `asciinema play FILE`.
    #[command(after_help = "\
EXAMPLES:
  microterm cli record --pane 3 -o logs.cast --duration 30
  microterm cli record --pane 3 -o session.cast          # until the pane closes or Ctrl-C
  microterm cli record -o here.cast --duration 10        # this pane ($MICROTERM_PANE)
  microterm cli record --pane 3 -o typed.cast --input    # also record keystrokes")]
    Record {
        /// Pane id (default: $MICROTERM_PANE).
        #[arg(long, value_name = "ID")]
        pane: Option<u64>,
        /// File to write (asciicast v2). Overwritten if it exists.
        #[arg(short = 'o', value_name = "FILE.cast")]
        output: PathBuf,
        /// Stop after this many seconds (default: until the pane closes or Ctrl-C).
        #[arg(long, value_name = "SECS")]
        duration: Option<u64>,
        /// Also record input (keys and pastes). Off by default: it captures typed secrets.
        #[arg(long)]
        input: bool,
    },

    /// Play a recording in a new tab.
    ///
    /// Opens a new tab (or window) that plays the recording's output with its original timing
    /// divided by --speed, and prints the new pane id. The tab stays open afterwards so the
    /// final screen can be read; close it like any tab. Accepts any asciicast v2 file, such as
    /// those made by `cli record`, `microterm record` or asciinema.
    #[command(after_help = "\
EXAMPLES:
  microterm cli replay demo.cast
  microterm cli replay demo.cast --speed 2               # twice as fast
  microterm cli replay demo.cast --speed 0.5 --new-window
  pane=$(microterm cli replay demo.cast --speed 50); microterm cli get-text --pane \"$pane\"")]
    Replay {
        /// The .cast file to play.
        file: PathBuf,
        /// Playback speed multiplier; must be greater than 0.
        #[arg(long, default_value_t = 1.0, value_name = "FACTOR")]
        speed: f64,
        /// Play in a new window instead of a new tab.
        #[arg(long)]
        new_window: bool,
    },

    /// Re-read the config file now.
    ///
    /// Same as the Reload config action: the file is validated and applied, or the previous
    /// config stays active and the error banner shows why. Normally unnecessary because the
    /// file is watched and reloaded automatically.
    #[command(after_help = "\
EXAMPLES:
  microterm cli reload-config")]
    ReloadConfig,
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceCmd {
    /// Print the workspace names; the active one is marked with `*`.
    #[command(after_help = "\
EXAMPLES:
  microterm cli workspace list
  microterm cli workspace list --json")]
    List,
    /// Make a workspace the active one (it must exist).
    ///
    /// The windows of the current workspace are hidden (their shells keep running) and the new
    /// workspace's windows appear in the same place. Exits with code 3 for an unknown name.
    #[command(after_help = "\
EXAMPLES:
  microterm cli workspace switch infra
  microterm cli workspace switch default")]
    Switch {
        /// Workspace name.
        name: String,
    },
    /// Create an empty workspace without switching to it.
    ///
    /// A workspace gets its first window when it is first shown or spawned into
    /// (`microterm cli spawn --workspace NAME`). Names must be non-empty and unique.
    #[command(after_help = "\
EXAMPLES:
  microterm cli workspace create api
  microterm cli spawn --workspace api -- cargo run")]
    Create {
        /// Name of the new workspace.
        name: String,
    },
    /// Rename a workspace. `default` cannot be renamed.
    #[command(after_help = "\
EXAMPLES:
  microterm cli workspace rename api backend")]
    Rename {
        /// Current name.
        old: String,
        /// New name (must not exist yet).
        new: String,
    },
}
