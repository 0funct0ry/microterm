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
    /// Open a serial port in a tab (`--list` shows the ports on this machine).
    Serial(SerialArgs),
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
    /// Print the snippet that makes a shell emit OSC 133 marks and OSC 7.
    ///
    /// Marks let `microterm ctl pane run --in`, `pane wait --command-finished` and
    /// `pane get-text --last-command` tell a command's output from the rest of the screen, and
    /// power prompt jumps in copy mode. Add one line to your shell's rc file:
    ///
    ///   bash: eval "$(microterm shell-integration bash)"   (~/.bashrc)
    ///   zsh:  eval "$(microterm shell-integration zsh)"    (~/.zshrc)
    ///   fish: microterm shell-integration fish | source    (~/.config/fish/config.fish)
    ShellIntegration { shell: Shell },
    /// Run a command in a new tab and record it (asciicast v2).
    ///
    /// The command starts only once recording is armed, so no output is lost, and the
    /// recording ends when the command exits.
    Record(RecordArgs),
    /// Script a running microterm instance, one resource at a time.
    ///
    /// `ctl` is organised by resource: `workspace`, `window`, `tab` and `pane`, plus the top-level
    /// `list`, `record`, `replay` and `reload-config`; `pane run` and `pane wait` run a command
    /// and wait for it or for output. Every command talks to the instance named
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
    /// destructive verb), 2 no running instance, 3 target not found (or the pane went away
    /// while waiting), 4 protocol version mismatch, 5 ambiguous target (several matches, no
    /// --all), 6 timeout (`pane wait`, `pane run --timeout`; the command keeps running),
    /// 7 needs shell integration (no OSC 133 marks seen in that pane), 8 confirmation required
    /// (confirm_close would prompt and --force was not given; nothing was closed), 9 pane busy
    /// (`pane run --in` while a command is running).
    Ctl(CtlArgs),
    /// Interactive control shell: a REPL over `ctl` with completion (SPEC §12.7).
    ///
    /// Everything `microterm ctl` can do, without retyping the boilerplate: `pane get 7`,
    /// `use pane 7`, tab completion of IDs and selectors, history, tables and trees on a terminal.
    /// `sh -c 'CMDS'`, `sh FILE` and `sh < FILE` run commands without a prompt. Exit status is the
    /// last command's `ctl` exit code (2 when no instance is running).
    #[cfg(feature = "sh")]
    Sh(ShArgs),
    /// Internal: plays a `.cast` file on stdout (what `ctl replay` runs in the new tab).
    #[command(name = "__replay", hide = true)]
    ReplayInternal {
        file: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
    },
}

#[cfg(feature = "sh")]
#[derive(Args, Debug, Default)]
pub struct ShArgs {
    /// Run these commands and exit.
    #[arg(short = 'c', value_name = "CMDS", conflicts_with = "file")]
    pub commands: Option<String>,
    /// Run the commands in this file (`-` or no FILE with piped stdin: read stdin).
    pub file: Option<PathBuf>,
    /// Print machine-readable JSON, as `ctl --json`.
    #[arg(long)]
    pub json: bool,
    /// Talk to this socket instead of the newest running instance.
    #[arg(long, value_name = "PATH")]
    pub socket: Option<PathBuf>,
    /// Do not read or write the history file.
    #[arg(long)]
    pub no_history: bool,
    /// No colour (NO_COLOR is honoured too).
    #[arg(long)]
    pub no_color: bool,
    /// Refuse every command that changes state.
    #[arg(long)]
    pub readonly: bool,
    /// Abort a script at the first failing command.
    #[arg(short = 'e')]
    pub errexit: bool,
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

#[derive(Args, Debug)]
pub struct SerialArgs {
    /// Device path, e.g. `/dev/ttyUSB0` or `/dev/cu.usbmodem1101`.
    #[arg(required_unless_present_any = ["domain", "list"])]
    pub port: Option<String>,
    /// Use the `[[serial_ports]]` entry with this name instead of an ad-hoc port.
    #[arg(long, value_name = "NAME", conflicts_with = "port")]
    pub domain: Option<String>,
    /// List serial ports (with USB VID:PID and product names) and exit.
    #[arg(long)]
    pub list: bool,
    /// Baud rate.
    #[arg(long, default_value_t = mt_serial::opts::DEFAULT_BAUD)]
    pub baud: u32,
    /// 5, 6, 7 or 8.
    #[arg(long, default_value = "8")]
    pub data_bits: String,
    /// none, odd or even.
    #[arg(long, default_value = "none")]
    pub parity: String,
    /// 1 or 2.
    #[arg(long, default_value = "1")]
    pub stop_bits: String,
    /// none, software (XON/XOFF) or hardware (RTS/CTS).
    #[arg(long, default_value = "none")]
    pub flow: String,
    /// Show typed characters locally (for devices that do not echo).
    #[arg(long)]
    pub local_echo: bool,
    /// What Enter is sent as: cr, lf or crlf.
    #[arg(long, default_value = "cr")]
    pub line_ending: String,
}

impl SerialArgs {
    /// The domain string the app resolves: a configured name or `serial:PORT?...`.
    pub fn domain_ref(&self) -> Result<String, String> {
        if let Some(d) = &self.domain {
            return Ok(d.clone());
        }
        let port = self.port.clone().unwrap_or_default();
        let mut o = mt_serial::SerialOpts::new(port);
        o.baud = mt_serial::opts::parse_baud(&self.baud.to_string()).map_err(|e| e.to_string())?;
        o.data_bits = self
            .data_bits
            .parse()
            .map_err(|e: mt_serial::OptsError| e.to_string())?;
        o.parity = self
            .parity
            .parse()
            .map_err(|e: mt_serial::OptsError| e.to_string())?;
        o.stop_bits = self
            .stop_bits
            .parse()
            .map_err(|e: mt_serial::OptsError| e.to_string())?;
        o.flow = self
            .flow
            .parse()
            .map_err(|e: mt_serial::OptsError| e.to_string())?;
        o.line_ending = self
            .line_ending
            .parse()
            .map_err(|e: mt_serial::OptsError| e.to_string())?;
        o.local_echo = self.local_echo;
        Ok(o.to_domain_ref())
    }
}

/// `microterm serial --list`: one line per port.
pub fn print_serial_ports() {
    let ports = mt_serial::discover();
    if ports.is_empty() {
        println!("no serial ports found");
    }
    for p in ports {
        let d = p.detail();
        if d.is_empty() {
            println!("{}", p.path);
        } else {
            println!("{:<28} {d}", p.path);
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
