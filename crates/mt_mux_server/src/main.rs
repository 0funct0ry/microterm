use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mt_proto::mux::{self, Msg};

#[derive(Parser)]
#[command(
    name = "microterm-mux",
    version,
    about = "Persistent terminal sessions for microterm"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the per-user daemon (detaches unless --foreground).
    Serve {
        #[arg(long)]
        foreground: bool,
    },
    /// Bridge stdin/stdout to the daemon, starting it if needed (used over SSH).
    Proxy,
    /// List panes.
    List,
    /// Kill a pane, or every pane of a session.
    Kill {
        pane: Option<u64>,
        #[arg(long, conflicts_with = "pane")]
        session: Option<String>,
    },
}

fn client() -> Result<(UnixStream, BufReader<UnixStream>), String> {
    let dir = mt_mux_server::paths::socket_dir();
    let sock = mt_mux_server::paths::socket_path(&dir);
    let mut s =
        UnixStream::connect(&sock).map_err(|e| format!("no daemon at {} ({e})", sock.display()))?;
    mux::write_frame(&mut s, &mux::hello()).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(s.try_clone().map_err(|e| e.to_string())?);
    match mux::read_frame(&mut r).map_err(|e| e.to_string())? {
        Some(m) => {
            mux::check_hello(&m)?;
        }
        None => return Err("daemon closed the connection".into()),
    }
    Ok((s, r))
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let dir = mt_mux_server::paths::socket_dir();
    match cli.cmd {
        Cmd::Serve { foreground } => match mt_mux_server::daemon::serve(&dir, foreground) {
            Ok(()) | Err(mt_mux_server::daemon::BindError::AlreadyRunning) => Ok(()),
            Err(mt_mux_server::daemon::BindError::Io(e)) => Err(e.to_string()),
        },
        Cmd::Proxy => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            mt_mux_server::proxy::run(&dir, &exe).map_err(|e| e.to_string())
        }
        Cmd::List => {
            let (mut s, mut r) = client()?;
            mux::write_frame(&mut s, &Msg::List).map_err(|e| e.to_string())?;
            match mux::read_frame(&mut r).map_err(|e| e.to_string())? {
                Some(Msg::Listing { panes }) => {
                    println!(
                        "{:<6} {:<16} {:<9} {:<10} TITLE",
                        "PANE", "SESSION", "SIZE", "STATE"
                    );
                    for p in panes {
                        let state = match p.exited {
                            None => "running".to_string(),
                            Some(x) => format!(
                                "exited({})",
                                x.status.map_or("-".into(), |s| s.to_string())
                            ),
                        };
                        println!(
                            "{:<6} {:<16} {:<9} {:<10} {}",
                            p.id,
                            p.session,
                            format!("{}x{}", p.cols, p.rows),
                            state,
                            p.title
                        );
                    }
                    Ok(())
                }
                other => Err(format!("unexpected reply: {other:?}")),
            }
        }
        Cmd::Kill { pane, session } => {
            let msg = match (pane, session) {
                (Some(pane), None) => Msg::Kill { pane },
                (None, Some(session)) => Msg::KillSession { session },
                _ => return Err("give a PANE_ID or --session NAME".into()),
            };
            let (mut s, mut r) = client()?;
            mux::write_frame(&mut s, &msg).map_err(|e| e.to_string())?;
            // A Ping round trip guarantees the daemon processed the kill (or reported an error).
            mux::write_frame(&mut s, &Msg::Ping(1)).map_err(|e| e.to_string())?;
            loop {
                match mux::read_frame(&mut r).map_err(|e| e.to_string())? {
                    Some(Msg::Pong(_)) => return Ok(()),
                    Some(Msg::Error { message }) => return Err(message),
                    Some(_) => {}
                    None => return Err("daemon closed the connection".into()),
                }
            }
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("microterm-mux: {e}");
            ExitCode::FAILURE
        }
    }
}
