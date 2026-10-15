mod cli;
mod commands;

use clap::Parser;

use crate::cli::{Cli, Command};

fn main() {
    // Usage errors exit 1 (SPEC §12.1); `--help` / `--version` still exit 0.
    let cli = Cli::try_parse().unwrap_or_else(|err| {
        let _ = err.print();
        std::process::exit(if err.use_stderr() { 1 } else { 0 });
    });
    match cli.command {
        Some(Command::Config(cmd)) => {
            std::process::exit(commands::config(&cmd, cli.config.as_deref()))
        }
        Some(Command::Themes(cmd)) => {
            std::process::exit(commands::themes(&cmd, cli.config.as_deref()))
        }
        Some(command) => {
            eprintln!("not implemented yet ({})", command.milestone());
            std::process::exit(1);
        }
        None => mt_ui::run(cli.config),
    }
}
