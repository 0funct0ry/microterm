//! `microterm cli …`, `microterm record` and the internal `__replay` player (SPEC §12.1).
//!
//! Every command prints its result on stdout (`--json`: the raw IPC `result`) and errors on
//! stderr, and returns the exit codes of SPEC §12.1: 0 ok, 1 usage, 2 no running instance,
//! 3 target not found, 4 protocol/version mismatch.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mt_proto::{
    exit, ActivateTabParams, Event, GetTextParams, MuxSnapshot, RecordStartParams, ReplayParams,
    Request, SendTextParams, SetTabTitleParams, SpawnParams, Spawned, Text,
};
use serde_json::Value;

use crate::cli::{CliAction, CliArgs, RecordArgs, WorkspaceCmd};
use crate::client::{Client, ClientError};

fn fail(code: i32, msg: impl std::fmt::Display) -> i32 {
    eprintln!("microterm: {msg}");
    code
}

fn client_err(e: ClientError) -> i32 {
    fail(e.exit_code(), e)
}

fn abs(p: &Path) -> String {
    std::path::absolute(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// `--pane`, defaulting to `$MICROTERM_PANE` (set for every pane microterm starts).
fn pane_of(flag: Option<u64>) -> Result<u64, i32> {
    if let Some(p) = flag {
        return Ok(p);
    }
    match std::env::var("MICROTERM_PANE") {
        Ok(v) => v.parse().map_err(|_| {
            fail(
                exit::USAGE,
                format!("$MICROTERM_PANE is not a pane id: {v:?}"),
            )
        }),
        Err(_) => Err(fail(
            exit::USAGE,
            "no pane given: pass --pane ID (or run this inside a microterm pane)",
        )),
    }
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_default());
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, i32> {
    serde_json::from_value(v)
        .map_err(|e| fail(exit::VERSION_MISMATCH, format!("unexpected reply: {e}")))
}

fn print_tree(s: &MuxSnapshot) {
    for ws in &s.workspaces {
        println!(
            "workspace {}{}",
            ws.name,
            if ws.active { " (active)" } else { "" }
        );
        for w in &ws.windows {
            println!("  window {}", w.id);
            for t in &w.tabs {
                println!(
                    "    tab {} {:?}{}",
                    t.id,
                    t.title,
                    if t.active { " (active)" } else { "" }
                );
                for p in &t.panes {
                    println!(
                        "      pane {}  {}{}",
                        p.id,
                        p.domain,
                        p.cwd
                            .as_deref()
                            .map(|c| format!("  {c}"))
                            .unwrap_or_default()
                    );
                }
            }
        }
    }
}

pub fn run(args: CliArgs) -> i32 {
    let json = args.json;
    let mut c = match Client::connect() {
        Ok(c) => c,
        Err(e) => return client_err(e),
    };
    match dispatch(&mut c, args.action, json) {
        Ok(()) => exit::OK,
        Err(code) => code,
    }
}

fn dispatch(c: &mut Client, action: CliAction, json: bool) -> Result<(), i32> {
    let mut call = |req: Request| c.call(&req).map_err(client_err);
    match action {
        CliAction::List => {
            let v = call(Request::MuxList)?;
            if json {
                print_json(&v);
            } else {
                print_tree(&decode(v)?);
            }
        }
        CliAction::Spawn {
            window,
            new_window,
            workspace,
            domain,
            cwd,
            title,
            command,
        } => {
            let v = call(Request::PaneSpawn(SpawnParams {
                window,
                new_window,
                workspace,
                domain,
                cwd: cwd.as_deref().map(abs),
                title,
                command: (!command.is_empty()).then_some(command),
                cols: None,
                rows: None,
            }))?;
            if json {
                print_json(&v);
            } else {
                println!("{}", decode::<Spawned>(v)?.pane);
            }
        }
        CliAction::SendText {
            pane,
            no_paste,
            text,
        } => {
            let pane = pane_of(pane)?;
            let text = match text {
                Some(t) => t,
                None => {
                    let mut s = String::new();
                    std::io::stdin()
                        .read_to_string(&mut s)
                        .map_err(|e| fail(exit::USAGE, format!("cannot read stdin: {e}")))?;
                    s
                }
            };
            let v = call(Request::PaneSendText(SendTextParams {
                pane,
                text,
                paste: !no_paste,
            }))?;
            if json {
                print_json(&v);
            }
        }
        CliAction::GetText {
            pane,
            start_line,
            end_line,
            escapes,
        } => {
            let pane = pane_of(pane)?;
            let v = call(Request::PaneGetText(GetTextParams {
                pane,
                start_line,
                end_line,
                escapes,
            }))?;
            if json {
                print_json(&v);
            } else {
                let t: Text = decode(v)?;
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(t.text.as_bytes());
            }
        }
        CliAction::ActivateTab {
            tab_id,
            tab_index,
            tab_relative,
        } => {
            if tab_id.is_none() && tab_index.is_none() && tab_relative.is_none() {
                return Err(fail(
                    exit::USAGE,
                    "give one of --tab-id, --tab-index or --tab-relative",
                ));
            }
            let v = call(Request::TabActivate(ActivateTabParams {
                window: None,
                tab_id,
                tab_index,
                tab_relative: tab_relative.map(i64::from),
            }))?;
            if json {
                print_json(&v);
            }
        }
        CliAction::SetTabTitle { tab_id, title } => {
            let v = call(Request::TabSetTitle(SetTabTitleParams { tab_id, title }))?;
            if json {
                print_json(&v);
            }
        }
        CliAction::KillPane { pane } => {
            let v = call(Request::PaneKill {
                pane: pane_of(pane)?,
            })?;
            if json {
                print_json(&v);
            }
        }
        CliAction::Workspace(w) => match w {
            WorkspaceCmd::List => {
                let v = call(Request::WorkspaceList)?;
                if json {
                    print_json(&v);
                } else {
                    for ws in decode::<MuxSnapshot>(v)?.workspaces {
                        println!("{}{}", ws.name, if ws.active { " *" } else { "" });
                    }
                }
            }
            other => {
                let req = match other {
                    WorkspaceCmd::Switch { name } => Request::WorkspaceSwitch { name },
                    WorkspaceCmd::Create { name } => Request::WorkspaceCreate { name },
                    WorkspaceCmd::Rename { old, new } => Request::WorkspaceRename { old, new },
                    WorkspaceCmd::List => unreachable!(),
                };
                let v = call(req)?;
                if json {
                    print_json(&v);
                }
            }
        },
        CliAction::Record {
            pane,
            output,
            duration,
            input,
        } => {
            let pane = pane_of(pane)?;
            let path = abs(&output);
            call(Request::RecordStart(RecordStartParams {
                pane,
                path: path.clone(),
                input,
            }))?;
            let waited = wait_for_pane_exit(pane, duration.map(Duration::from_secs));
            // The pane may already be gone (its recording finished by itself).
            let stopped = c.call(&Request::RecordStop { pane });
            match (waited, stopped) {
                (Err(code), _) => return Err(code),
                (Ok(_), Err(e)) if e.exit_code() != exit::NOT_FOUND => return Err(client_err(e)),
                _ => {}
            }
            if json {
                print_json(&serde_json::json!({ "path": path }));
            }
        }
        CliAction::Replay {
            file,
            speed,
            new_window,
        } => {
            if !(speed.is_finite() && speed > 0.0) {
                return Err(fail(exit::USAGE, "--speed must be greater than 0"));
            }
            let v = call(Request::ReplayStart(ReplayParams {
                path: abs(&file),
                speed,
                new_window,
            }))?;
            if json {
                print_json(&v);
            } else {
                println!("{}", decode::<Spawned>(v)?.pane);
            }
        }
        CliAction::ReloadConfig => {
            let v = call(Request::ConfigReload)?;
            if json {
                print_json(&v);
            }
        }
    }
    Ok(())
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

fn catch_sigint() {
    // SAFETY: the handler only stores to an atomic.
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as libc::sighandler_t);
    }
}

/// Blocks until `pane` is removed, `limit` passes, or the user presses Ctrl-C.
fn wait_for_pane_exit(pane: u64, limit: Option<Duration>) -> Result<(), i32> {
    let mut events = Client::connect()
        .and_then(Client::subscribe)
        .map_err(client_err)?;
    catch_sigint();
    let t0 = Instant::now();
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) || limit.is_some_and(|l| t0.elapsed() >= l) {
            return Ok(());
        }
        match events.next_event().map_err(client_err)? {
            Some(Event::PaneRemoved { pane: p }) if p == pane => return Ok(()),
            _ => {}
        }
    }
}

/// `microterm record -o FILE -- CMD`: runs CMD in a new tab and records it until it exits.
pub fn record(args: RecordArgs) -> i32 {
    let mut c = match Client::connect() {
        Ok(c) => c,
        Err(e) => return client_err(e),
    };
    let path = abs(&args.output);
    // CMD starts only once the recording is armed: the tab first runs a gate that waits for a
    // file, so not a byte of output is lost and no keystroke is injected.
    let gate = format!("{path}.go");
    let _ = std::fs::remove_file(&gate);
    let mut command: Vec<String> = [
        "sh",
        "-c",
        "while [ ! -e \"$1\" ]; do sleep 0.02; done; rm -f \"$1\"; shift; exec \"$@\"",
        "microterm-record",
        gate.as_str(),
    ]
    .map(String::from)
    .to_vec();
    command.extend(args.command.iter().cloned());
    let mut events = match Client::connect().and_then(Client::subscribe) {
        Ok(e) => e,
        Err(e) => return client_err(e),
    };
    let spawned = c
        .call(&Request::PaneSpawn(SpawnParams {
            command: Some(command),
            title: args.command.first().cloned(),
            cols: args.cols,
            rows: args.rows,
            ..Default::default()
        }))
        .map_err(client_err)
        .and_then(decode::<Spawned>);
    let pane = match spawned {
        Ok(s) => s.pane,
        Err(code) => return code,
    };
    let started = c.call(&Request::RecordStart(RecordStartParams {
        pane,
        path,
        input: args.input,
    }));
    let _ = std::fs::write(&gate, b"");
    if let Err(e) = started {
        let _ = c.call(&Request::PaneKill { pane });
        return client_err(e);
    }
    catch_sigint();
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            let _ = c.call(&Request::RecordStop { pane });
            return exit::OK;
        }
        match events.next_event() {
            Ok(Some(Event::PaneRemoved { pane: p })) if p == pane => return exit::OK,
            Ok(_) => {}
            Err(e) => return client_err(e),
        }
    }
}

/// `microterm __replay FILE --speed S`: prints the output events with their original timing,
/// then idles so the tab stays open for reading.
pub fn replay_internal(file: &Path, speed: f64) -> i32 {
    let f = match std::fs::File::open(file) {
        Ok(f) => f,
        Err(e) => {
            return fail(
                exit::NOT_FOUND,
                format!("cannot open {}: {e}", file.display()),
            )
        }
    };
    let reader = match mt_record::Reader::new(std::io::BufReader::new(f)) {
        Ok(r) => r,
        Err(e) => return fail(exit::USAGE, format!("{}: {e}", file.display())),
    };
    let speed = if speed.is_finite() && speed > 0.0 {
        speed
    } else {
        1.0
    };
    let t0 = Instant::now();
    let mut out = std::io::stdout().lock();
    for ev in reader {
        let ev = match ev {
            Ok(e) => e,
            Err(e) => return fail(exit::USAGE, e),
        };
        if let mt_record::EventKind::Output(data) = ev.kind {
            let due = Duration::from_secs_f64(ev.time / speed);
            if let Some(wait) = due.checked_sub(t0.elapsed()) {
                std::thread::sleep(wait);
            }
            let _ = out.write_all(data.as_bytes());
            let _ = out.flush();
        }
    }
    drop(out);
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

pub fn completions(shell: crate::cli::Shell) -> i32 {
    use clap::CommandFactory;
    use clap_complete::Shell as S;
    let shell = match shell {
        crate::cli::Shell::Bash => S::Bash,
        crate::cli::Shell::Zsh => S::Zsh,
        crate::cli::Shell::Fish => S::Fish,
    };
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "microterm", &mut std::io::stdout());
    exit::OK
}
