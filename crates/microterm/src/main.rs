mod cli;
mod commands;
mod ipc;

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
        Some(Command::LsFonts { text }) => {
            std::process::exit(mt_ui::ls_fonts::run(cli.config, text))
        }
        Some(command) => {
            eprintln!("not implemented yet ({})", command.milestone());
            std::process::exit(1);
        }
        None => launch_gui(cli),
    }
}

/// No subcommand: open a window in the running instance, or become the instance.
fn launch_gui(cli: Cli) {
    let cwd = cli.cwd.map(|p| std::path::absolute(&p).unwrap_or(p));
    let command = (!cli.exec.is_empty()).then(|| cli.exec.clone());
    if !cli.new_instance {
        let req = mt_ui::NewWindowRequest {
            workspace: cli.workspace.clone(),
            cwd: cwd.clone(),
            command: command.clone(),
        };
        match ipc::send_new_window(&req) {
            Ok(ipc::Delivery::Delivered) => return,
            Ok(ipc::Delivery::NoInstance) => {}
            Err(e) => eprintln!("microterm: could not reach the running instance: {e}"),
        }
    }
    let (tx, rx) = async_channel::unbounded();
    let _server = match ipc::spawn_server(tx) {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("microterm: single-instance socket unavailable: {e}");
            None
        }
    };
    mt_ui::run(mt_ui::RunOptions {
        config: cli.config,
        workspace: cli.workspace,
        cwd,
        command,
        requests: _server.is_some().then_some(rx),
    });
}
