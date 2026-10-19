mod cli;
mod client;
mod commands;
mod ctl;
mod ctl_cmd;
mod headless;
mod ipc;

use std::sync::Arc;

use clap::Parser;
use mt_proto::SpawnParams;

use crate::cli::{Cli, Command};

fn main() {
    // Usage errors exit 1 (SPEC §12.1); `--help` / `--version` still exit 0.
    let mut cli = Cli::try_parse().unwrap_or_else(|err| {
        let _ = err.print();
        std::process::exit(if err.use_stderr() { 1 } else { 0 });
    });
    if cli.headless {
        std::process::exit(headless::run(cli.config));
    }
    match cli.command.take() {
        Some(Command::Config(cmd)) => {
            std::process::exit(commands::config(&cmd, cli.config.as_deref()))
        }
        Some(Command::Themes(cmd)) => {
            std::process::exit(commands::themes(&cmd, cli.config.as_deref()))
        }
        Some(Command::LsFonts { text }) => {
            std::process::exit(mt_ui::ls_fonts::run(cli.config, text))
        }
        Some(Command::Completions { shell }) => std::process::exit(ctl_cmd::completions(shell)),
        Some(Command::Ctl(args)) => std::process::exit(ctl_cmd::run(args)),
        Some(Command::Record(args)) => std::process::exit(ctl_cmd::record(args)),
        Some(Command::ReplayInternal { file, speed }) => {
            std::process::exit(ctl_cmd::replay_internal(&file, speed))
        }
        Some(Command::Ssh(args)) => {
            let domain = args.domain_ref();
            launch_gui(cli, Some(domain))
        }
        None => launch_gui(cli, None),
    }
}

/// No subcommand: open a window in the running instance, or become the instance.
fn launch_gui(cli: Cli, domain: Option<String>) {
    let cwd = cli.cwd.map(|p| std::path::absolute(&p).unwrap_or(p));
    let command = (!cli.exec.is_empty()).then(|| cli.exec.clone());
    if !cli.new_instance {
        let params = SpawnParams {
            workspace: cli.workspace.clone(),
            cwd: cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
            command: command.clone(),
            domain: domain.clone(),
            ..Default::default()
        };
        match client::Client::connect()
            .and_then(|mut c| c.call(&mt_proto::Request::WindowNew(params)))
        {
            Ok(_) => return,
            Err(client::ClientError::NoInstance) => {}
            Err(e) => eprintln!("microterm: could not reach the running instance: {e}"),
        }
    }
    let (tx, rx) = async_channel::unbounded();
    let hub = Arc::new(mt_proto::EventHub::default());
    let server = match ipc::start(tx, hub.clone()) {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("microterm: IPC socket unavailable: {e}");
            None
        }
    };
    let ipc = server.as_ref().map(|s| mt_ui::IpcHandle {
        calls: rx,
        hub,
        socket: s.path().to_path_buf(),
    });
    mt_ui::run(mt_ui::RunOptions {
        config: cli.config,
        workspace: cli.workspace,
        cwd,
        command,
        domain,
        ipc,
    });
    drop(server); // removes the socket file
}
