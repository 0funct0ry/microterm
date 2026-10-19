//! `microterm ctl …`, `microterm record` and the internal `__replay` player (SPEC §12.1).
//!
//! Every command prints its result on stdout (`--json`: the raw IPC `result`) and errors on
//! stderr (`--json`: `{"error":{"code","message","candidates"?}}`), and returns the exit codes
//! of SPEC §12.3: 0 ok, 1 usage, 2 no running instance, 3 target not found, 4 protocol/version
//! mismatch, 5 ambiguous target, 8 confirmation required.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mt_proto::{
    exit, CallerEnv, ErrorCode, Event, GetTextParams, MuxSnapshot, PaneInfo, RecordStartParams,
    ReplayParams, Request, Resource, SendTextParams, SpawnParams, Spawned, TabFocusParams, TabInfo,
    TabRenameParams, Target, Text, WindowInfo, WorkspaceInfo,
};
use serde_json::{json, Value};

use crate::cli::RecordArgs;
use crate::client::{Client, ClientError};
use crate::ctl::{
    CtlAction, CtlArgs, PaneCmd, PaneTarget, TabCmd, TabTarget, WindowCmd, WindowTarget,
    WorkspaceCmd,
};

static JSON_ERRORS: AtomicBool = AtomicBool::new(false);

fn fail(code: i32, msg: impl std::fmt::Display) -> i32 {
    if JSON_ERRORS.load(Ordering::Relaxed) {
        let name = match code {
            exit::NO_INSTANCE => "no_instance",
            exit::NOT_FOUND => "not_found",
            exit::VERSION_MISMATCH => "version_mismatch",
            _ => "bad_request",
        };
        eprintln!(
            "{}",
            json!({ "error": { "code": name, "message": msg.to_string() } })
        );
    } else {
        eprintln!("microterm: {msg}");
    }
    code
}

fn client_err(e: ClientError) -> i32 {
    match &e {
        ClientError::Rpc(rpc) if JSON_ERRORS.load(Ordering::Relaxed) => {
            eprintln!("{}", json!({ "error": rpc }));
            e.exit_code()
        }
        ClientError::Rpc(rpc) if !rpc.candidates.is_empty() => {
            eprintln!("microterm: {}", rpc.message);
            for c in &rpc.candidates {
                eprintln!("  {c}");
            }
            e.exit_code()
        }
        ClientError::Rpc(rpc) if rpc.code == ErrorCode::ConfirmRequired => {
            eprintln!("microterm: {}", rpc.message);
            e.exit_code()
        }
        _ => fail(e.exit_code(), &e),
    }
}

fn abs(p: &Path) -> String {
    std::path::absolute(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_default());
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, i32> {
    serde_json::from_value(v)
        .map_err(|e| fail(exit::VERSION_MISMATCH, format!("unexpected reply: {e}")))
}

/// Rows as an aligned table on a terminal, tab-separated when piped (SPEC §12.3).
fn print_table(headers: &[&str], rows: Vec<Vec<String>>) {
    if std::io::stdout().is_terminal() {
        let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
        for r in &rows {
            for (i, c) in r.iter().enumerate() {
                widths[i] = widths[i].max(c.chars().count());
            }
        }
        let line = |cells: Vec<&str>| {
            let parts: Vec<String> = cells
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
                .collect();
            println!("{}", parts.join("  ").trim_end());
        };
        line(headers.to_vec());
        for r in &rows {
            line(r.iter().map(String::as_str).collect());
        }
    } else {
        for r in rows {
            println!("{}", r.join("\t"));
        }
    }
}

fn flag(b: bool) -> String {
    if b { "*" } else { "" }.to_string()
}

fn print_tree(s: &MuxSnapshot) {
    for ws in &s.workspaces {
        println!(
            "workspace {}{}",
            ws.name,
            if ws.active { " (active)" } else { "" }
        );
        for w in &ws.windows {
            println!(
                "  window {}{}",
                w.id,
                if w.focused { " (focused)" } else { "" }
            );
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

/// `tab=5 pane=6 window=2`, safe to feed to `eval`.
fn print_spawned(s: &Spawned) {
    println!("tab={} pane={} window={}", s.tab, s.pane, s.window);
}

pub fn run(args: CtlArgs) -> i32 {
    let json = args.json;
    JSON_ERRORS.store(json, Ordering::Relaxed);
    let mut c = match Client::connect() {
        Ok(c) => c,
        Err(e) => return client_err(e),
    };
    match dispatch(&mut c, args.action, json, CallerEnv::from_env()) {
        Ok(()) => exit::OK,
        Err(code) => code,
    }
}

fn pane_target(t: PaneTarget, env: CallerEnv) -> Target {
    Target {
        pane: t.pane,
        expr: t.expr,
        ..Default::default()
    }
    .with_env(Resource::Pane, env)
}

fn tab_target(t: TabTarget, env: CallerEnv) -> Target {
    Target {
        tab: t.tab,
        expr: t.expr,
        ..Default::default()
    }
    .with_env(Resource::Tab, env)
}

fn window_target(t: WindowTarget, env: CallerEnv) -> Target {
    Target {
        window: t.window,
        expr: t.expr,
        ..Default::default()
    }
    .with_env(Resource::Window, env)
}

fn dispatch(c: &mut Client, action: CtlAction, json: bool, env: CallerEnv) -> Result<(), i32> {
    let mut call = |req: Request| c.call(&req).map_err(client_err);
    // Verbs whose success output is nothing (text) or the raw result (--json).
    let quiet = |v: Value| {
        if json {
            print_json(&v);
        }
    };
    match action {
        CtlAction::List => {
            let v = call(Request::MuxList)?;
            if json {
                print_json(&v);
            } else {
                print_tree(&decode(v)?);
            }
        }
        CtlAction::Workspace(w) => match w {
            WorkspaceCmd::List => {
                let v = call(Request::WorkspaceList)?;
                if json {
                    print_json(&v);
                } else {
                    let rows = decode::<Vec<WorkspaceInfo>>(v)?
                        .into_iter()
                        .map(|w| vec![flag(w.active), w.name, w.windows.len().to_string()])
                        .collect();
                    print_table(&["ACTIVE", "NAME", "WINDOWS"], rows);
                }
            }
            WorkspaceCmd::Switch { name } => quiet(call(Request::WorkspaceSwitch { name })?),
            WorkspaceCmd::Create { name } => quiet(call(Request::WorkspaceCreate { name })?),
            WorkspaceCmd::Rename { old, new } => {
                quiet(call(Request::WorkspaceRename { old, new })?)
            }
            WorkspaceCmd::Delete { name, force } => {
                quiet(call(Request::WorkspaceDelete { name, force })?)
            }
        },
        CtlAction::Window(w) => match w {
            WindowCmd::List { expr } => {
                let v = call(Request::WindowList(Target {
                    expr,
                    ..Default::default()
                }))?;
                if json {
                    print_json(&v);
                } else {
                    let rows = decode::<Vec<WindowInfo>>(v)?
                        .into_iter()
                        .map(|w| {
                            vec![
                                flag(w.focused),
                                w.id.to_string(),
                                w.workspace,
                                w.tabs.len().to_string(),
                                w.title,
                            ]
                        })
                        .collect();
                    print_table(&["FOCUSED", "ID", "WORKSPACE", "TABS", "TITLE"], rows);
                }
            }
            WindowCmd::New {
                workspace,
                cwd,
                command,
            } => {
                let v = call(Request::WindowNew(SpawnParams {
                    workspace,
                    cwd: cwd.as_deref().map(abs),
                    command: (!command.is_empty()).then_some(command),
                    ..Default::default()
                }))?;
                if json {
                    print_json(&v);
                } else {
                    print_spawned(&decode(v)?);
                }
            }
            WindowCmd::Focus(t) => quiet(call(Request::WindowFocus(window_target(t, env)))?),
            WindowCmd::Close { target, force, all } => {
                let t = Target {
                    force,
                    all,
                    ..window_target(target, env)
                };
                quiet(call(Request::WindowClose(t))?);
            }
        },
        CtlAction::Tab(t) => match t {
            TabCmd::List { window, expr } => {
                let v = call(Request::TabList(Target {
                    window,
                    expr,
                    ..Default::default()
                }))?;
                if json {
                    print_json(&v);
                } else {
                    let rows = decode::<Vec<TabInfo>>(v)?
                        .into_iter()
                        .map(|t| {
                            vec![
                                flag(t.focused),
                                t.id.to_string(),
                                t.window.to_string(),
                                t.domain,
                                t.title,
                            ]
                        })
                        .collect();
                    print_table(&["FOCUSED", "ID", "WINDOW", "DOMAIN", "TITLE"], rows);
                }
            }
            TabCmd::New {
                window,
                workspace,
                domain,
                cwd,
                title,
                command,
            } => {
                let v = call(Request::TabNew(SpawnParams {
                    window,
                    workspace,
                    domain,
                    cwd: cwd.as_deref().map(abs),
                    title,
                    command: (!command.is_empty()).then_some(command),
                    ..Default::default()
                }))?;
                if json {
                    print_json(&v);
                } else {
                    print_spawned(&decode(v)?);
                }
            }
            TabCmd::Focus {
                target,
                index,
                relative,
                window,
            } => {
                let params = if index.is_some() || relative.is_some() {
                    TabFocusParams {
                        target: Target {
                            window,
                            ..Default::default()
                        }
                        .with_env(Resource::Window, env),
                        index,
                        relative,
                    }
                } else {
                    TabFocusParams {
                        target: tab_target(target, env),
                        ..Default::default()
                    }
                };
                quiet(call(Request::TabFocus(params))?);
            }
            TabCmd::Rename { target, title } => quiet(call(Request::TabRename(TabRenameParams {
                target: tab_target(target, env),
                title,
            }))?),
            TabCmd::Close { target, force, all } => {
                let t = Target {
                    force,
                    all,
                    ..tab_target(target, env)
                };
                quiet(call(Request::TabClose(t))?);
            }
        },
        CtlAction::Pane(p) => match p {
            PaneCmd::List { window, tab, expr } => {
                let v = call(Request::PaneList(Target {
                    window,
                    tab,
                    expr,
                    ..Default::default()
                }))?;
                if json {
                    print_json(&v);
                } else {
                    let rows = decode::<Vec<PaneInfo>>(v)?
                        .into_iter()
                        .map(|p| {
                            vec![
                                flag(p.focused),
                                p.id.to_string(),
                                p.tab.to_string(),
                                p.window.to_string(),
                                p.domain,
                                p.process.unwrap_or_default(),
                                p.title,
                            ]
                        })
                        .collect();
                    print_table(
                        &[
                            "FOCUSED", "ID", "TAB", "WINDOW", "DOMAIN", "PROCESS", "TITLE",
                        ],
                        rows,
                    );
                }
            }
            PaneCmd::Get(t) => {
                let v = call(Request::PaneGet(pane_target(t, env)))?;
                if json {
                    print_json(&v);
                } else {
                    let p: PaneInfo = decode(v)?;
                    println!("id\t{}", p.id);
                    println!("tab\t{}", p.tab);
                    println!("window\t{}", p.window);
                    println!("workspace\t{}", p.workspace);
                    println!("domain\t{}", p.domain);
                    println!("title\t{}", p.title);
                    println!("cwd\t{}", p.cwd.unwrap_or_default());
                    println!("process\t{}", p.process.unwrap_or_default());
                    println!("size\t{}x{}", p.size.cols, p.size.rows);
                    println!("focused\t{}", p.focused);
                    println!("exited\t{}", p.exited);
                }
            }
            PaneCmd::SendText {
                target,
                no_paste,
                all,
                text,
            } => {
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
                quiet(call(Request::PaneSendText(SendTextParams {
                    target: Target {
                        all,
                        ..pane_target(target, env)
                    },
                    text,
                    paste: !no_paste,
                }))?);
            }
            PaneCmd::GetText {
                target,
                start_line,
                end_line,
                escapes,
            } => {
                let v = call(Request::PaneGetText(GetTextParams {
                    target: pane_target(target, env),
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
            PaneCmd::Close { target, force, all } => {
                let t = Target {
                    force,
                    all,
                    ..pane_target(target, env)
                };
                quiet(call(Request::PaneClose(t))?);
            }
        },
        CtlAction::Record {
            pane,
            output,
            duration,
            input,
        } => {
            let pane = match pane.or(env.pane) {
                Some(p) => p,
                None => {
                    return Err(fail(
                        exit::USAGE,
                        "no pane given: pass --pane ID (or run this inside a microterm pane)",
                    ))
                }
            };
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
        CtlAction::Replay {
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
                print_spawned(&decode(v)?);
            }
        }
        CtlAction::ReloadConfig => quiet(call(Request::ConfigReload)?),
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
        .call(&Request::TabNew(SpawnParams {
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
        let _ = c.call(&Request::PaneClose(Target {
            force: true,
            ..Target::pane(pane)
        }));
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
