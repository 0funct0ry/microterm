mod cli;

use clap::Parser;

use crate::cli::Cli;

fn main() {
    // Usage errors exit 1 (SPEC §12.1); `--help` / `--version` still exit 0.
    let cli = Cli::try_parse().unwrap_or_else(|err| {
        let _ = err.print();
        std::process::exit(if err.use_stderr() { 1 } else { 0 });
    });
    match cli.command {
        Some(command) => {
            eprintln!("not implemented yet ({})", command.milestone());
            std::process::exit(1);
        }
        None => mt_ui::run(),
    }
}
