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
    Record(RecordArgs),
    /// Script a running instance.
    Cli(CliArgs),
}

#[derive(Args, Debug)]
pub struct SshArgs {
    /// `[user@]host[:port]`
    pub target: String,
    #[arg(long)]
    pub domain: Option<String>,
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
pub struct RecordArgs {
    #[arg(short = 'o', value_name = "FILE.cast")]
    pub output: PathBuf,
    #[arg(long)]
    pub input: bool,
    #[arg(long)]
    pub cols: Option<u16>,
    #[arg(long)]
    pub rows: Option<u16>,
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Args, Debug)]
pub struct CliArgs {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub action: CliAction,
}

#[derive(Subcommand, Debug)]
pub enum CliAction {
    List,
    Spawn {
        #[arg(long)]
        window: Option<u64>,
        #[arg(long)]
        new_window: bool,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    SendText {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long)]
        no_paste: bool,
        text: Option<String>,
    },
    GetText {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long)]
        start_line: Option<i64>,
        #[arg(long)]
        end_line: Option<i64>,
        #[arg(long)]
        escapes: bool,
    },
    ActivateTab {
        #[arg(long)]
        tab_id: Option<u64>,
        #[arg(long)]
        tab_index: Option<usize>,
        #[arg(long, allow_hyphen_values = true)]
        tab_relative: Option<i32>,
    },
    SetTabTitle {
        #[arg(long)]
        tab_id: Option<u64>,
        title: String,
    },
    KillPane {
        #[arg(long)]
        pane: Option<u64>,
    },
    #[command(subcommand)]
    Workspace(WorkspaceCmd),
    Record {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(short = 'o')]
        output: PathBuf,
        #[arg(long)]
        duration: Option<u64>,
        #[arg(long)]
        input: bool,
    },
    Replay {
        file: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        #[arg(long)]
        new_window: bool,
    },
    ReloadConfig,
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceCmd {
    List,
    Switch { name: String },
    Create { name: String },
    Rename { old: String, new: String },
}

impl Command {
    /// The milestone that implements this subcommand.
    pub fn milestone(&self) -> &'static str {
        match self {
            Command::Ssh(_) => "M10",
            Command::LsFonts { .. } => "M4",
            Command::Themes(_) | Command::Config(_) => "M3",
            Command::Completions { .. } | Command::Record(_) | Command::Cli(_) => "M9",
        }
    }
}
