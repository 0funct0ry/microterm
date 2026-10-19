//! Command-line surface (SPEC §12.1).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

pub use crate::ctl::CtlArgs;

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
    /// Script a running microterm instance, one resource at a time.
    ///
    /// `ctl` is organised by resource: `workspace`, `window`, `tab` and `pane`, plus the top-level
    /// `list`, `record`, `replay` and `reload-config`. Every command talks to the instance named
    /// by $MICROTERM_UNIX_SOCKET (set in every local pane), or else the newest live instance of
    /// this user.
    ///
    /// TARGETS: name an item with `--pane|--tab|--window ID` or `--match EXPR`. With neither, a
    /// command run inside a pane targets that pane, tab or window through $MICROTERM_PANE,
    /// $MICROTERM_TAB and $MICROTERM_WINDOW. Read-only verbs (`get`, `get-text`) then fall back to
    /// the focused item; input and destructive verbs (`send-text`, `close`, `rename`, `focus`)
    /// never do. Selectors: KEY:VALUE clauses joined with commas (AND), a leading `!` negates,
    /// values are globs or `re:REGEX`. Keys: id title cwd cmd domain workspace window tab focused
    /// active status exited. See `microterm ctl pane --help`.
    ///
    /// With --json each command prints one JSON value on stdout; errors go to stderr as
    /// {"error":{"code","message","candidates"?}}.
    ///
    /// EXIT CODES: 0 ok, 1 usage or other error (including a missing target for an input or
    /// destructive verb), 2 no running instance, 3 target not found, 4 protocol version
    /// mismatch, 5 ambiguous target (several matches, no --all), 8 confirmation required
    /// (confirm_close would prompt and --force was not given; nothing was closed).
    Ctl(CtlArgs),
    /// Internal: plays a `.cast` file on stdout (what `ctl replay` runs in the new tab).
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
`microterm ctl replay build.cast` or `asciinema play build.cast`.")]
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
