//! `microterm ctl …`, `microterm record` and the internal `__replay` player (SPEC §12.1).
//!
//! `dispatch` turns a parsed verb into an [`Output`] (or a [`CtlError`]) without printing or
//! exiting, so `microterm sh` runs the very same code. [`run`] renders the result: the result on
//! stdout (`--json`: the raw IPC `result`), errors on stderr (`--json`:
//! `{"error":{"code","message","candidates"?}}`), and the exit codes of SPEC §12.3: 0 ok, 1 usage,
//! 2 no running instance, 3 target not found, 4 protocol/version mismatch, 5 ambiguous target,
//! 6 timeout, 7 needs shell integration, 8 confirmation required, 9 pane busy.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

use mt_proto::{
    exit, CallerEnv, CommandText, Event, GetTextParams, MuxSnapshot, NotifyParams, PaneInfo,
    RecordStartParams, ReplayParams, Request, Resource, RunParams, RunResult, Selector,
    SendTextParams, SpawnParams, Spawned, StatusResult, StatusSetParams, SubscribeParams,
    TabFocusParams, TabInfo, TabRenameParams, Target, Text, WaitParams, WindowInfo, WorkspaceInfo,
};
use serde_json::Value;

use crate::cli::RecordArgs;
use crate::client::{Client, ClientError};
use crate::ctl::{
    CtlAction, CtlArgs, MenuBarCmd, MenuCmd, PaneCmd, PaneTarget, RecordingCmd, SnippetCmd,
    SnippetSource, StatusCmd, TabCmd, TabTarget, WindowCmd, WindowTarget, WorkspaceCmd,
};
use crate::output::{Body, ColKind, Column, CtlError, Output, Table};
use crate::render::{render, render_error, RenderOpts};

/// What a verb needs besides its arguments: who is calling, where to stream, how to connect.
pub struct Ctx<'a> {
    pub json: bool,
    pub env: CallerEnv,
    /// Where streaming verbs (`events`) write as they go.
    pub stream: &'a mut dyn Write,
    /// `send-text` without TEXT reads stdin; `sh` turns that into a usage error.
    pub allow_stdin: bool,
    /// An explicit socket (`sh --socket`); `None` finds the running instance.
    pub socket: Option<PathBuf>,
}

impl Ctx<'_> {
    fn connect(&self) -> Result<Client, ClientError> {
        connect(self.socket.as_deref())
    }
}

/// `menu show` rows as indented `id<TAB>label` lines; disabled rows are marked.
fn menu_rows_text(items: &Value, depth: usize) -> String {
    let mut out = String::new();
    for r in items.as_array().map(Vec::as_slice).unwrap_or_default() {
        let indent = "  ".repeat(depth);
        match r["type"].as_str() {
            Some("separator") => out.push_str(&format!("{indent}---\n")),
            Some("header") => out.push_str(&format!(
                "{indent}[{}]\n",
                r["label"].as_str().unwrap_or_default()
            )),
            _ => {
                let off = if r["enabled"].as_bool() == Some(false) {
                    "\t(disabled)"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "{indent}{}\t{}{off}\n",
                    r["id"].as_str().unwrap_or_default(),
                    r["label"].as_str().unwrap_or_default()
                ));
                if let Some(kids) = r.get("items") {
                    out.push_str(&menu_rows_text(kids, depth + 1));
                }
            }
        }
    }
    out
}

/// `menu icons`: the bundled icon names, one per line (no instance needed).
pub fn menu_icons(json: bool) -> Output {
    let names = mt_config::menu::ICON_NAMES;
    if json {
        Output::new(Body::Json(serde_json::json!(names)))
    } else {
        let mut text = names.join("\n");
        text.push('\n');
        Output::new(Body::Raw(text))
    }
}

/// `menu check`: the config's menu diagnostics, with no instance needed (SPEC §10.9.6).
pub fn menu_check(config: Option<&Path>, json: bool) -> Result<Output, CtlError> {
    let path = mt_config::load::config_path(config);
    let name = mt_config::load::display_name(&path);
    let render = |d: &mt_config::Diagnostic| d.render(&name);
    let is_menu = |d: &mt_config::Diagnostic| {
        [
            "context_menu",
            "menu_groups",
            "menu_security",
            "right_click",
        ]
        .iter()
        .any(|k| d.message.contains(k))
    };
    match mt_config::load::load(&path) {
        Err(diags) => {
            let lines: Vec<String> = diags.iter().map(render).collect();
            Err(CtlError::usage(lines.join("\n")))
        }
        Ok(loaded) => {
            let warnings: Vec<String> = loaded
                .warnings
                .iter()
                .filter(|d| is_menu(d))
                .map(|d| format!("warning: {}", render(d)))
                .collect();
            let menus = mt_config::menu::MENUS.len();
            let items: usize = mt_config::menu::MENUS
                .iter()
                .map(|m| mt_config::menu::entries(&loaded.config, m.id).len())
                .sum();
            let out = if json {
                Output::new(Body::Json(serde_json::json!({
                    "ok": true,
                    "menus": menus,
                    "items": items,
                    "warnings": warnings,
                })))
            } else {
                Output::text(format!("menus OK ({menus} menus, {items} rows)"))
            };
            Ok(warnings.into_iter().fold(out, Output::with_note))
        }
    }
}

/// Connects to `socket`, or to the running instance when there is none.
pub fn connect(socket: Option<&Path>) -> Result<Client, ClientError> {
    match socket {
        Some(p) => Client::connect_to(p),
        None => Client::connect(),
    }
}

/// Writes a verb's result to `stdout`/`stderr` and returns its exit code.
pub fn present(
    res: Result<Output, CtlError>,
    json: bool,
    opts: &RenderOpts,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match res {
        Ok(out) => {
            let _ = stdout.write_all(render(&out, opts).as_bytes());
            let _ = stdout.flush();
            for note in &out.notes {
                let _ = writeln!(stderr, "microterm: {note}");
            }
            out.code
        }
        Err(e) => {
            let _ = stderr.write_all(render_error(&e, json, opts).as_bytes());
            e.code
        }
    }
}

fn abs(p: &Path) -> String {
    std::path::absolute(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, CtlError> {
    serde_json::from_value(v)
        .map_err(|e| CtlError::new(exit::VERSION_MISMATCH, format!("unexpected reply: {e}")))
}

fn table(columns: Vec<Column>, rows: Vec<Vec<String>>, noun: &'static str) -> Output {
    Output::new(Body::Table(Table {
        columns,
        rows,
        dim_rows: Vec::new(),
        noun,
    }))
}

fn flag(b: bool) -> String {
    if b { "*" } else { "" }.to_string()
}

pub fn run(args: CtlArgs) -> i32 {
    let json = args.json;
    // These two need no running instance (or connect on demand).
    match &args.action {
        CtlAction::Skill(cmd) => return microterm::agent::run_skill(cmd, json),
        CtlAction::Mcp => return crate::mcp::run(),
        CtlAction::Menu(MenuCmd::Icons) => {
            let opts = RenderOpts::detect(false, json);
            return present(
                Ok(menu_icons(json)),
                json,
                &opts,
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            );
        }
        CtlAction::Menu(MenuCmd::Check { config }) => {
            let opts = RenderOpts::detect(false, json);
            return present(
                menu_check(config.as_deref(), json),
                json,
                &opts,
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            );
        }
        _ => {}
    }
    let opts = RenderOpts::detect(false, json);
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let res = match Client::connect() {
        Ok(mut c) => {
            let mut ctx = Ctx {
                json,
                env: CallerEnv::from_env(),
                stream: &mut stdout,
                allow_stdin: true,
                socket: None,
            };
            dispatch(&mut c, args.action, &mut ctx)
        }
        Err(e) => Err(e.into()),
    };
    present(res, json, &opts, &mut stdout, &mut stderr)
}

/// Prompt values from `--vars-file` (a JSON object; `-` is stdin) overlaid by `--var`.
fn snippet_vars(
    source: &SnippetSource,
    ctx: &Ctx<'_>,
) -> Result<std::collections::BTreeMap<String, String>, CtlError> {
    let mut vars = std::collections::BTreeMap::new();
    if let Some(file) = &source.vars_file {
        let text = if file == "-" {
            if !ctx.allow_stdin {
                return Err(CtlError::usage(
                    "--vars-file - needs stdin, which is not available here",
                ));
            }
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| CtlError::usage(format!("cannot read stdin: {e}")))?;
            s
        } else {
            std::fs::read_to_string(file)
                .map_err(|e| CtlError::usage(format!("cannot read {file}: {e}")))?
        };
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| CtlError::usage(format!("--vars-file is not JSON: {e}")))?;
        let Value::Object(map) = v else {
            return Err(CtlError::usage(
                "--vars-file must hold a JSON object {label: value}",
            ));
        };
        for (k, v) in map {
            vars.insert(
                k,
                match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                },
            );
        }
    }
    for (k, v) in &source.vars {
        vars.insert(k.clone(), v.clone());
    }
    Ok(vars)
}

fn snippet_warnings(v: &Value) {
    for w in v["warnings"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if let Some(w) = w.as_str() {
            eprintln!("microterm: warning: {w}");
        }
    }
}

fn snippet_verb(
    call: &mut dyn FnMut(Request) -> Result<Value, CtlError>,
    cmd: SnippetCmd,
    ctx: &mut Ctx<'_>,
) -> Result<Output, CtlError> {
    let json = ctx.json;
    let env = ctx.env;
    let result = |v: Value, plain: &dyn Fn(Value) -> Result<Output, CtlError>| {
        if json {
            Ok(Output::new(Body::Json(v)))
        } else {
            plain(v)
        }
    };
    let quiet = |v: Value| -> Result<Output, CtlError> {
        Ok(if json {
            Output::new(Body::Json(v))
        } else {
            Output::none()
        })
    };
    match cmd {
        SnippetCmd::List { folder, tag, query } => {
            let v = call(Request::SnippetList(mt_proto::SnippetListParams {
                folder,
                tag,
                query,
            }))?;
            result(v, &|v| {
                let rows = v
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|s| {
                        let t = |k: &str| s[k].as_str().unwrap_or_default().to_string();
                        vec![
                            t("id"),
                            t("title"),
                            t("folder"),
                            if s["run"].as_bool().unwrap_or(false) {
                                "run".into()
                            } else {
                                String::new()
                            },
                            t("key"),
                        ]
                    })
                    .collect();
                Ok(table(
                    vec![
                        Column::new("ID", ColKind::Text),
                        Column::new("TITLE", ColKind::Text),
                        Column::low("FOLDER", ColKind::Text, 1),
                        Column::low("RUN", ColKind::Text, 2),
                        Column::low("KEY", ColKind::Text, 3),
                    ],
                    rows,
                    "snippet",
                ))
            })
        }
        SnippetCmd::Show { id } => {
            let v = call(Request::SnippetGet(mt_proto::SnippetIdParams { id }))?;
            result(v, &|v| {
                let t = |k: &str| v[k].as_str().unwrap_or_default().to_string();
                let mut kv = vec![
                    ("id".to_string(), t("id")),
                    ("title".into(), t("title")),
                    ("folder".into(), t("folder")),
                    (
                        "tags".into(),
                        v["tags"].as_array().map_or(String::new(), |a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        }),
                    ),
                    (
                        "run".into(),
                        v["run"].as_bool().unwrap_or(false).to_string(),
                    ),
                    ("key".into(), t("key")),
                ];
                for p in v["prompts"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    kv.push((
                        "prompt".into(),
                        format!(
                            "{} ({})",
                            p["label"].as_str().unwrap_or(""),
                            p["kind"].as_str().unwrap_or("")
                        ),
                    ));
                }
                kv.push(("body".into(), t("body")));
                Ok(Output::new(Body::Card(kv)))
            })
        }
        SnippetCmd::Expand { source, target } => {
            let vars = snippet_vars(&source, ctx)?;
            let v = call(Request::SnippetExpand(mt_proto::SnippetUseParams {
                target: pane_target(target, env),
                id: source.id,
                body: source.body,
                vars,
                ..Default::default()
            }))?;
            snippet_warnings(&v);
            result(v, &|v| {
                Ok(Output::new(Body::Raw(
                    v["text"].as_str().unwrap_or_default().to_string(),
                )))
            })
        }
        SnippetCmd::Insert {
            source,
            target,
            run,
            no_run,
            all,
        } => {
            let vars = snippet_vars(&source, ctx)?;
            let v = call(Request::SnippetInsert(mt_proto::SnippetUseParams {
                target: Target {
                    all,
                    ..pane_target(target, env)
                },
                id: source.id,
                body: source.body,
                vars,
                run: if run {
                    Some(true)
                } else if no_run {
                    Some(false)
                } else {
                    None
                },
                agent: false,
            }))?;
            snippet_warnings(&v);
            quiet(v)
        }
        SnippetCmd::Add {
            title,
            id,
            folder,
            tags,
            description,
            run,
            key,
            replace,
            body,
        } => {
            let body = match body {
                Some(b) => b,
                None if !ctx.allow_stdin => {
                    return Err(CtlError::usage(
                        "snippet add needs BODY here (stdin is not available)",
                    ))
                }
                None => {
                    let mut s = String::new();
                    std::io::stdin()
                        .read_to_string(&mut s)
                        .map_err(|e| CtlError::usage(format!("cannot read stdin: {e}")))?;
                    s.strip_suffix('\n').map(str::to_string).unwrap_or(s)
                }
            };
            let v = call(Request::SnippetAdd(mt_proto::SnippetAddParams {
                id,
                title,
                body,
                folder,
                tags,
                description,
                run,
                key,
                replace,
            }))?;
            result(v, &|v| {
                Ok(Output::new(Body::Raw(format!(
                    "{}\n",
                    v["id"].as_str().unwrap_or_default()
                ))))
            })
        }
        SnippetCmd::Rm { id } => quiet(call(Request::SnippetRemove(mt_proto::SnippetIdParams {
            id,
        }))?),
    }
}

fn recording_verb(
    call: &mut dyn FnMut(Request) -> Result<Value, CtlError>,
    cmd: RecordingCmd,
    ctx: &mut Ctx<'_>,
) -> Result<Output, CtlError> {
    let json = ctx.json;
    let env = ctx.env;
    let need_pane = |pane: Option<u64>| {
        pane.or(env.pane).ok_or_else(|| {
            CtlError::usage("no pane given: pass --pane ID (or run this inside a microterm pane)")
        })
    };
    let secs = |ms: u64| {
        let s = ms / 1000;
        format!("{:02}:{:02}", s / 60, s % 60)
    };
    let mb = |b: u64| {
        if b >= 1024 * 1024 {
            format!("{:.1} MB", b as f64 / 1_048_576.0)
        } else {
            format!("{:.1} KB", b as f64 / 1024.0)
        }
    };
    match cmd {
        RecordingCmd::List => {
            let v = call(Request::RecordingList)?;
            if json {
                return Ok(Output::new(Body::Json(v)));
            }
            let rows = v["recordings"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|r| {
                    let t = |k: &str| r[k].as_str().unwrap_or_default().to_string();
                    let mut flags = Vec::new();
                    if r["has_input"].as_bool().unwrap_or(false) {
                        flags.push("input");
                    }
                    if r["recovered"].as_bool().unwrap_or(false) {
                        flags.push("recovered");
                    }
                    vec![
                        t("id"),
                        t("title"),
                        secs(r["duration_ms"].as_u64().unwrap_or(0)),
                        mb(r["size_bytes"].as_u64().unwrap_or(0)),
                        flags.join(","),
                    ]
                })
                .collect();
            Ok(table(
                vec![
                    Column::new("ID", ColKind::Text),
                    Column::new("TITLE", ColKind::Text),
                    Column::low("DURATION", ColKind::Text, 1),
                    Column::low("SIZE", ColKind::Text, 2),
                    Column::low("FLAGS", ColKind::Text, 3),
                ],
                rows,
                "recording",
            ))
        }
        RecordingCmd::Show { id } => {
            let v = call(Request::RecordingGet(mt_proto::RecordingIdParams { id }))?;
            if json {
                return Ok(Output::new(Body::Json(v)));
            }
            let t = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            let kv = vec![
                ("id".to_string(), t("id")),
                ("title".into(), t("title")),
                ("path".into(), t("path")),
                (
                    "duration".into(),
                    secs(v["duration_ms"].as_u64().unwrap_or(0)),
                ),
                ("size".into(), mb(v["size_bytes"].as_u64().unwrap_or(0))),
                (
                    "terminal".into(),
                    format!(
                        "{}x{}",
                        v["cols"].as_u64().unwrap_or(0),
                        v["rows"].as_u64().unwrap_or(0)
                    ),
                ),
                (
                    "input".into(),
                    if v["has_input"].as_bool().unwrap_or(false) {
                        format!("recorded (redaction: {})", t("input_redacted"))
                    } else {
                        "not recorded".into()
                    },
                ),
                ("origin".into(), {
                    let o = &v["origin"];
                    let s = |k: &str| o[k].as_str().unwrap_or_default().to_string();
                    format!("{} {} {}", s("kind"), s("name"), s("process"))
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                }),
                (
                    "markers".into(),
                    v["markers"].as_array().map_or(0, Vec::len).to_string(),
                ),
                (
                    "recovered".into(),
                    v["recovered"].as_bool().unwrap_or(false).to_string(),
                ),
            ];
            Ok(Output::new(Body::Card(kv)))
        }
        RecordingCmd::Start {
            pane,
            input,
            no_input,
            title,
        } => {
            let v = call(Request::RecordingStart(mt_proto::RecordingStartParams {
                pane: need_pane(pane)?,
                input: if input {
                    Some(true)
                } else if no_input {
                    Some(false)
                } else {
                    None
                },
                title,
            }))?;
            Ok(if json {
                Output::new(Body::Json(v))
            } else {
                Output::new(Body::Raw(format!(
                    "{}\n",
                    v["id"].as_str().unwrap_or_default()
                )))
            })
        }
        RecordingCmd::Stop { id, pane, all } => {
            let params = if all {
                mt_proto::RecordingStopParams {
                    all: true,
                    ..Default::default()
                }
            } else if let Some(id) = id {
                mt_proto::RecordingStopParams {
                    id: Some(id),
                    ..Default::default()
                }
            } else {
                mt_proto::RecordingStopParams {
                    pane: Some(need_pane(pane)?),
                    ..Default::default()
                }
            };
            let v = call(Request::RecordingStop(params))?;
            if json {
                return Ok(Output::new(Body::Json(v)));
            }
            let lines: String = v["stopped"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|r| format!("{}\n", r["id"].as_str().unwrap_or_default()))
                .collect();
            Ok(Output::new(Body::Raw(lines)))
        }
        RecordingCmd::Marker { pane, label } => {
            let v = call(Request::RecordingMarker(mt_proto::RecordingMarkerParams {
                pane: need_pane(pane)?,
                label,
            }))?;
            Ok(if json {
                Output::new(Body::Json(v))
            } else {
                Output::none()
            })
        }
        RecordingCmd::Rename { id, title } => {
            let v = call(Request::RecordingRename(mt_proto::RecordingRenameParams {
                id,
                title,
            }))?;
            Ok(if json {
                Output::new(Body::Json(v))
            } else {
                Output::none()
            })
        }
        RecordingCmd::Rm { ids, force } => {
            let v = call(Request::RecordingRemove(mt_proto::RecordingRemoveParams {
                ids,
                force,
            }))?;
            Ok(if json {
                Output::new(Body::Json(v))
            } else {
                Output::none()
            })
        }
        RecordingCmd::Replay {
            id,
            speed,
            new_window,
        } => {
            if !(speed.is_finite() && speed > 0.0) {
                return Err(CtlError::usage("--speed must be greater than 0"));
            }
            let rec = call(Request::RecordingGet(mt_proto::RecordingIdParams { id }))?;
            let path = rec["path"].as_str().unwrap_or_default().to_string();
            let v = call(Request::ReplayStart(ReplayParams {
                path,
                speed,
                new_window,
            }))?;
            if json {
                Ok(Output::new(Body::Json(v)))
            } else {
                spawned("replay", v)
            }
        }
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

/// Runs one verb against the instance behind `c`.
pub fn dispatch(c: &mut Client, action: CtlAction, ctx: &mut Ctx<'_>) -> Result<Output, CtlError> {
    let json = ctx.json;
    let env = ctx.env;
    let mut call = |req: Request| c.call(&req).map_err(CtlError::from);
    // `--json` prints the raw result; the plain forms are built by `plain`.
    let result = |v: Value, plain: &dyn Fn(Value) -> Result<Output, CtlError>| {
        if json {
            Ok(Output::new(Body::Json(v)))
        } else {
            plain(v)
        }
    };
    // Verbs whose success output is nothing (text) or the raw result (--json).
    let quiet = |v: Value| -> Result<Output, CtlError> {
        Ok(if json {
            Output::new(Body::Json(v))
        } else {
            Output::none()
        })
    };
    match action {
        CtlAction::List => {
            let v = call(Request::MuxList)?;
            result(v, &|v| {
                Ok(Output::new(Body::Tree(decode::<MuxSnapshot>(v)?)))
            })
        }
        CtlAction::Workspace(w) => match w {
            WorkspaceCmd::List => {
                let v = call(Request::WorkspaceList)?;
                result(v, &|v| {
                    let rows = decode::<Vec<WorkspaceInfo>>(v)?
                        .into_iter()
                        .map(|w| vec![flag(w.active), w.name, w.windows.len().to_string()])
                        .collect();
                    Ok(table(
                        vec![
                            Column::new("ACTIVE", ColKind::Flag),
                            Column::new("NAME", ColKind::Text),
                            Column::new("WINDOWS", ColKind::Id),
                        ],
                        rows,
                        "workspace",
                    ))
                })
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
                result(v, &|v| {
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
                    Ok(table(
                        vec![
                            Column::new("FOCUSED", ColKind::Flag),
                            Column::new("ID", ColKind::Id),
                            Column::low("WORKSPACE", ColKind::Text, 1),
                            Column::new("TABS", ColKind::Id),
                            Column::new("TITLE", ColKind::Text),
                        ],
                        rows,
                        "window",
                    ))
                })
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
                result(v, &|v| spawned("window", v))
            }
            WindowCmd::Focus(t) => quiet(call(Request::WindowFocus(window_target(t, env)))?),
            WindowCmd::Close { target, force, all } => {
                let t = Target {
                    force,
                    all,
                    ..window_target(target, env)
                };
                quiet(call(Request::WindowClose(t))?)
            }
        },
        CtlAction::Tab(t) => match t {
            TabCmd::List { window, expr } => {
                let v = call(Request::TabList(Target {
                    window,
                    expr,
                    ..Default::default()
                }))?;
                result(v, &|v| {
                    let rows = decode::<Vec<TabInfo>>(v)?
                        .into_iter()
                        .map(|t| {
                            vec![
                                flag(t.focused),
                                t.id.to_string(),
                                t.window.to_string(),
                                t.domain,
                                t.color.unwrap_or_default(),
                                t.title,
                            ]
                        })
                        .collect();
                    Ok(table(
                        vec![
                            Column::new("FOCUSED", ColKind::Flag),
                            Column::new("ID", ColKind::Id),
                            Column::new("WINDOW", ColKind::Id),
                            Column::low("DOMAIN", ColKind::Text, 1),
                            Column::low("COLOR", ColKind::Text, 1),
                            Column::new("TITLE", ColKind::Text),
                        ],
                        rows,
                        "tab",
                    ))
                })
            }
            TabCmd::New {
                window,
                workspace,
                domain,
                cwd,
                title,
                color,
                command,
            } => {
                let v = call(Request::TabNew(SpawnParams {
                    window,
                    workspace,
                    domain,
                    cwd: cwd.as_deref().map(abs),
                    title,
                    color,
                    command: (!command.is_empty()).then_some(command),
                    ..Default::default()
                }))?;
                result(v, &|v| spawned("tab", v))
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
                quiet(call(Request::TabFocus(params))?)
            }
            TabCmd::Rename { target, title } => quiet(call(Request::TabRename(TabRenameParams {
                target: tab_target(target, env),
                title,
            }))?),
            TabCmd::Color {
                target,
                color,
                clear,
            } => {
                let v = call(Request::TabSetColor(mt_proto::TabColorParams {
                    target: tab_target(target, env),
                    color,
                    clear,
                }))?;
                result(v, &|v| {
                    let text = match v.get("color").and_then(Value::as_str) {
                        Some(c) => format!(
                            "{c} ({})",
                            v.get("color_source")
                                .and_then(Value::as_str)
                                .unwrap_or("manual")
                        ),
                        None => "none".to_string(),
                    };
                    Ok(Output::new(Body::Text(text)))
                })
            }
            TabCmd::Close { target, force, all } => {
                let t = Target {
                    force,
                    all,
                    ..tab_target(target, env)
                };
                quiet(call(Request::TabClose(t))?)
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
                result(v, &|v| {
                    let panes = decode::<Vec<PaneInfo>>(v)?;
                    let dim_rows = panes.iter().map(|p| p.exited).collect();
                    let rows = panes
                        .into_iter()
                        .map(|p| {
                            vec![
                                flag(p.focused),
                                p.id.to_string(),
                                p.tab.to_string(),
                                p.window.to_string(),
                                p.domain,
                                p.process.unwrap_or_default(),
                                p.status
                                    .map_or_else(|| "-".into(), |s| s.state.as_str().to_string()),
                                p.title,
                            ]
                        })
                        .collect();
                    Ok(Output::new(Body::Table(Table {
                        columns: vec![
                            Column::new("FOCUSED", ColKind::Flag),
                            Column::new("ID", ColKind::Id),
                            Column::new("TAB", ColKind::Id),
                            Column::new("WINDOW", ColKind::Id),
                            Column::low("DOMAIN", ColKind::Text, 1),
                            Column::new("PROCESS", ColKind::Text),
                            Column::new("STATUS", ColKind::Status),
                            Column::new("TITLE", ColKind::Text),
                        ],
                        rows,
                        dim_rows,
                        noun: "pane",
                    })))
                })
            }
            PaneCmd::Get(t) => {
                let v = call(Request::PaneGet(pane_target(t, env)))?;
                result(v, &|v| {
                    let p: PaneInfo = decode(v)?;
                    let mut kv = vec![
                        ("id".to_string(), p.id.to_string()),
                        ("tab".into(), p.tab.to_string()),
                        ("window".into(), p.window.to_string()),
                        ("workspace".into(), p.workspace),
                        ("domain".into(), p.domain),
                        ("title".into(), p.title),
                        ("cwd".into(), p.cwd.unwrap_or_default()),
                        ("process".into(), p.process.unwrap_or_default()),
                        ("size".into(), format!("{}x{}", p.size.cols, p.size.rows)),
                        ("focused".into(), p.focused.to_string()),
                        ("exited".into(), p.exited.to_string()),
                    ];
                    if let Some(st) = p.status {
                        kv.push(("status".into(), status_line(&st)));
                    }
                    Ok(Output::new(Body::Card(kv)))
                })
            }
            PaneCmd::SendText {
                target,
                no_paste,
                all,
                text,
            } => {
                let text = match text {
                    Some(t) => t,
                    None if !ctx.allow_stdin => {
                        return Err(CtlError::usage(
                            "send-text needs TEXT here (use -f FILE or < FILE)",
                        ))
                    }
                    None => {
                        let mut s = String::new();
                        std::io::stdin()
                            .read_to_string(&mut s)
                            .map_err(|e| CtlError::usage(format!("cannot read stdin: {e}")))?;
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
                }))?)
            }
            PaneCmd::GetText {
                target,
                start_line,
                end_line,
                escapes,
                last_command,
            } => {
                let v = call(Request::PaneGetText(GetTextParams {
                    target: pane_target(target, env),
                    start_line,
                    end_line,
                    escapes,
                    last_command,
                }))?;
                result(v, &|v| {
                    let text = if last_command.is_some() {
                        decode::<CommandText>(v)?.text
                    } else {
                        decode::<Text>(v)?.text
                    };
                    Ok(Output::new(Body::Raw(text)))
                })
            }
            PaneCmd::Run {
                window,
                new_window,
                workspace,
                domain,
                cwd,
                title,
                keep,
                detach,
                timeout,
                propagate_exit,
                in_pane,
                command,
            } => {
                let mut params = RunParams {
                    keep,
                    detach,
                    timeout_ms: timeout,
                    ..Default::default()
                };
                match in_pane {
                    Some(t) => {
                        params.in_pane = Some(in_target(&t, env));
                        params.line = Some(command.join(" "));
                    }
                    None => {
                        params.spawn = SpawnParams {
                            window,
                            new_window,
                            workspace,
                            domain,
                            cwd: cwd.as_deref().map(abs),
                            title,
                            command: Some(command),
                            ..Default::default()
                        };
                    }
                }
                let v = call(Request::PaneRun(params))?;
                let r: RunResult = decode(v.clone())?;
                let mut out = if json {
                    Output::new(Body::Json(v))
                } else if detach {
                    Output::text(format!("pane={} tab={}", r.pane, r.tab.unwrap_or_default()))
                } else if let Some(o) = &r.output {
                    Output::new(Body::Raw(o.clone()))
                } else {
                    Output::none()
                };
                // The output is printed first; the exit code says how the command went.
                if r.timed_out {
                    out.code = exit::TIMEOUT;
                } else if propagate_exit {
                    if let Err(code) = propagated(&r) {
                        out.code = code;
                    }
                }
                Ok(out)
            }
            PaneCmd::Wait {
                target,
                pane,
                select,
                matches,
                text,
                idle,
                exit: wait_exit,
                command_finished,
                timeout,
                new_only,
            } => {
                let params = wait_params(
                    WaitArgs {
                        target,
                        pane,
                        select,
                        matches,
                        text,
                        idle,
                        exit: wait_exit,
                        command_finished,
                        timeout,
                        new_only,
                    },
                    env,
                );
                quiet(call(Request::PaneWait(params))?)
            }
            PaneCmd::Notify {
                target,
                title,
                level,
                desktop,
                all,
                message,
            } => {
                let message = message.unwrap_or_default();
                if message.trim().is_empty() && title.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(CtlError::usage("nothing to say: give a MESSAGE or --title"));
                }
                let v = call(Request::PaneNotify(NotifyParams {
                    target: Target {
                        all,
                        ..pane_target(target, env)
                    },
                    title,
                    level,
                    desktop,
                    message,
                }))?;
                let dropped = v["dropped"].as_u64().unwrap_or(0);
                let mut out = quiet(v)?;
                if !json && dropped > 0 {
                    out = out.with_note(format!("{dropped} notification(s) dropped (rate limit)"));
                }
                Ok(out)
            }
            PaneCmd::Status(cmd) => match cmd {
                StatusCmd::Set {
                    target,
                    state,
                    progress,
                    ttl,
                    all,
                    text,
                } => quiet(call(Request::PaneStatusSet(StatusSetParams {
                    target: Target {
                        all,
                        ..pane_target(target, env)
                    },
                    state,
                    text,
                    progress,
                    ttl_ms: ttl,
                }))?),
                StatusCmd::Clear { target, all } => {
                    quiet(call(Request::PaneStatusClear(Target {
                        all,
                        ..pane_target(target, env)
                    }))?)
                }
                StatusCmd::Get(t) => {
                    let v = call(Request::PaneStatusGet(pane_target(t, env)))?;
                    result(v, &|v| {
                        let r: StatusResult = decode(v)?;
                        Ok(Output::text(match r.status {
                            Some(st) => status_line(&st),
                            None => "idle".to_string(),
                        }))
                    })
                }
            },
            PaneCmd::Split {
                target,
                horizontal,
                vertical: _,
                direction,
                percent,
                cwd,
                command,
            } => {
                let direction = direction.map(mt_proto::Direction::from);
                let side_by_side = horizontal
                    || matches!(
                        direction,
                        Some(mt_proto::Direction::Left | mt_proto::Direction::Right)
                    );
                let v = call(Request::PaneSplit(mt_proto::PaneSplitParams {
                    target: pane_target(target, env),
                    axis: if side_by_side {
                        mt_proto::SplitAxis::Horizontal
                    } else {
                        mt_proto::SplitAxis::Vertical
                    },
                    direction,
                    percent,
                    cwd: cwd.as_deref().map(abs),
                    command: (!command.is_empty()).then_some(command),
                }))?;
                result(v, &|v| {
                    let r: mt_proto::PaneSplitResult = decode(v)?;
                    Ok(Output::new(Body::Spawned {
                        kind: "pane",
                        spawned: Spawned {
                            pane: r.pane,
                            tab: r.tab,
                            window: r.window,
                        },
                    }))
                })
            }
            PaneCmd::Focus { target, direction } => {
                quiet(call(Request::PaneFocus(mt_proto::PaneFocusParams {
                    target: pane_target(target, env),
                    direction: direction.map(Into::into),
                }))?)
            }
            PaneCmd::Zoom(t) => {
                let v = call(Request::PaneZoom(pane_target(t, env)))?;
                result(v, &|v| {
                    let zoomed = v.get("zoomed").and_then(Value::as_bool).unwrap_or(false);
                    Ok(Output::new(Body::Text(
                        if zoomed { "zoomed" } else { "unzoomed" }.into(),
                    )))
                })
            }
            PaneCmd::Resize {
                target,
                direction,
                cells,
            } => quiet(call(Request::PaneResize(mt_proto::PaneResizeParams {
                target: pane_target(target, env),
                direction: direction.into(),
                cells,
            }))?),
            PaneCmd::Swap { target, direction } => {
                quiet(call(Request::PaneSwap(mt_proto::PaneDirParams {
                    target: pane_target(target, env),
                    direction: direction.into(),
                }))?)
            }
            PaneCmd::Rotate(t) => quiet(call(Request::PaneRotate(pane_target(t, env)))?),
            PaneCmd::Close { target, force, all } => {
                let t = Target {
                    force,
                    all,
                    ..pane_target(target, env)
                };
                quiet(call(Request::PaneClose(t))?)
            }
        },
        CtlAction::Events {
            types,
            expr,
            snapshot,
        } => {
            events(
                ctx,
                SubscribeParams {
                    types,
                    expr,
                    snapshot,
                },
            )?;
            Ok(Output::none())
        }
        CtlAction::Record {
            pane,
            output,
            duration,
            input,
        } => {
            let pane = match pane.or(env.pane) {
                Some(p) => p,
                None => {
                    return Err(CtlError::usage(
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
            let waited = wait_for_pane_exit(ctx, pane, duration.map(Duration::from_secs));
            // The pane may already be gone (its recording finished by itself).
            let stopped = c.call(&Request::RecordStop { pane });
            match (waited, stopped) {
                (Err(e), _) => return Err(e),
                (Ok(_), Err(e)) if e.exit_code() != exit::NOT_FOUND => return Err(e.into()),
                _ => {}
            }
            Ok(if json {
                Output::new(Body::Json(serde_json::json!({ "path": path })))
            } else {
                Output::none()
            })
        }
        CtlAction::Replay {
            file,
            speed,
            new_window,
        } => {
            if !(speed.is_finite() && speed > 0.0) {
                return Err(CtlError::usage("--speed must be greater than 0"));
            }
            let v = call(Request::ReplayStart(ReplayParams {
                path: abs(&file),
                speed,
                new_window,
            }))?;
            result(v, &|v| spawned("replay", v))
        }
        CtlAction::ReloadConfig => quiet(call(Request::ConfigReload)?),
        CtlAction::Skill(_) | CtlAction::Mcp => unreachable!("handled in run()"),
        CtlAction::Menu(MenuCmd::Check { config }) => menu_check(config.as_deref(), json),
        CtlAction::Menu(MenuCmd::Icons) => Ok(menu_icons(json)),
        CtlAction::Menu(MenuCmd::List) => {
            let v = call(Request::MenuList)?;
            result(v, &|v| {
                let rows = v
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|m| {
                        let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
                        vec![
                            s("id"),
                            s("trigger"),
                            m["items"].as_array().map_or(0, Vec::len).to_string(),
                            s("description"),
                        ]
                    })
                    .collect();
                Ok(table(
                    vec![
                        Column::new("MENU", ColKind::Text),
                        Column::new("TRIGGER", ColKind::Text),
                        Column::new("ITEMS", ColKind::Id),
                        Column::low("DESCRIPTION", ColKind::Text, 1),
                    ],
                    rows,
                    "menu",
                ))
            })
        }
        CtlAction::MenuBar(MenuBarCmd::List) => {
            let v = call(Request::MenuBarList)?;
            result(v, &|v| {
                let rows = v["items"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|m| {
                        let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
                        vec![
                            s("path"),
                            m["action"].as_str().unwrap_or("-").to_string(),
                            if m["enabled"].as_bool().unwrap_or(false) {
                                "enabled".into()
                            } else {
                                "disabled".into()
                            },
                            s("chip"),
                        ]
                    })
                    .collect();
                Ok(table(
                    vec![
                        Column::new("ITEM", ColKind::Text),
                        Column::new("ACTION", ColKind::Text),
                        Column::new("STATE", ColKind::Text),
                        Column::low("SHORTCUT", ColKind::Text, 1),
                    ],
                    rows,
                    "menu row",
                ))
            })
        }
        CtlAction::Snippet(cmd) => snippet_verb(&mut call, cmd, ctx),
        CtlAction::Recording(cmd) => recording_verb(&mut call, cmd, ctx),
        CtlAction::Menu(MenuCmd::Show { menu, target }) => {
            let v = call(Request::MenuShow(mt_proto::MenuShowParams {
                target: pane_target(target, env),
                menu,
            }))?;
            result(v, &|v| {
                Ok(Output::new(Body::Raw(menu_rows_text(&v["items"], 0))))
            })
        }
        CtlAction::Menu(MenuCmd::Invoke {
            item,
            target,
            dry_run,
            force,
        }) => {
            let v = call(Request::MenuInvoke(mt_proto::MenuInvokeParams {
                target: Target {
                    force,
                    ..pane_target(target, env)
                },
                item,
                dry_run,
            }))?;
            result(v, &|v| {
                if dry_run {
                    let mut card = Vec::new();
                    if let Value::Object(m) = &v {
                        for (k, val) in m {
                            if val.is_null() {
                                continue;
                            }
                            let text = match val {
                                Value::String(s) => s.clone(),
                                Value::Array(a) => a
                                    .iter()
                                    .map(|x| {
                                        x.as_str().map_or_else(|| x.to_string(), str::to_string)
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                other => other.to_string(),
                            };
                            card.push((k.clone(), text));
                        }
                    }
                    Ok(Output::new(Body::Card(card)))
                } else {
                    Ok(Output::text(format!(
                        "ran {}/{} on pane {}",
                        v["menu"].as_str().unwrap_or_default(),
                        v["item"].as_str().unwrap_or_default(),
                        v["pane"]
                    )))
                }
            })
        }
    }
}

/// `new` / `replay` results: `tab=5 pane=6 window=2`, safe to feed to `eval`.
fn spawned(kind: &'static str, v: Value) -> Result<Output, CtlError> {
    let s: Spawned = decode(v)?;
    // A replay opens a tab, so it reads as one.
    let kind = if kind == "replay" { "tab" } else { kind };
    Ok(Output::new(Body::Spawned { kind, spawned: s }))
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
/// The connection a foreground `sh` verb is blocked on; Ctrl-C shuts it down so the read returns
/// (the server then cancels a wait, never the command it waits for).
static INTERRUPT_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
    let fd = INTERRUPT_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: shutdown(2) is async-signal-safe and only affects that socket.
        #[allow(unsafe_code)]
        unsafe {
            libc::shutdown(fd, libc::SHUT_RDWR);
        }
    }
}

fn catch_sigint() {
    INTERRUPTED.store(false, Ordering::Relaxed);
    // SAFETY: the handler only stores to an atomic.
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as libc::sighandler_t);
    }
}

/// Starts a foreground verb in an interactive shell: Ctrl-C sets the interrupted flag and, when
/// `fd` is given, shuts that connection down instead of killing the process.
pub fn begin_foreground(fd: Option<std::os::unix::io::RawFd>) {
    INTERRUPTED.store(false, Ordering::Relaxed);
    INTERRUPT_FD.store(fd.unwrap_or(-1), Ordering::Relaxed);
    // SAFETY: the handler only touches atomics and calls shutdown(2).
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as libc::sighandler_t);
    }
}

/// Ends a foreground verb: restores SIGINT and says whether Ctrl-C arrived meanwhile.
pub fn end_foreground() -> bool {
    INTERRUPT_FD.store(-1, Ordering::Relaxed);
    // SAFETY: restoring the default disposition.
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
    INTERRUPTED.swap(false, Ordering::Relaxed)
}

/// `state [progress%] [text]`, the plain form of a pane status.
fn status_line(st: &mt_proto::PaneStatus) -> String {
    let mut line = st.state.as_str().to_string();
    if let Some(p) = st.progress {
        line.push_str(&format!(" {p}%"));
    }
    if !st.text.is_empty() {
        line.push(' ');
        line.push_str(&st.text);
    }
    line
}

/// `ctl events`: writes one JSON line per event until interrupted. Fails with exit 2 (silently)
/// when the GUI quits.
fn events(ctx: &mut Ctx<'_>, params: SubscribeParams) -> Result<(), CtlError> {
    let mut events = ctx.connect().and_then(|c| c.subscribe_with(params))?;
    catch_sigint();
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Ok(());
        }
        match events.next_event() {
            Ok(Some(ev)) => {
                let line = ev.stream_json(mt_term::now_ms());
                // A closed pipe (`| head -1`) ends the stream quietly.
                if writeln!(ctx.stream, "{line}")
                    .and_then(|()| ctx.stream.flush())
                    .is_err()
                {
                    return Ok(());
                }
            }
            Ok(None) => {}
            // The server hung up: the GUI quit.
            Err(ClientError::Io(_)) => return Err(CtlError::silent(exit::NO_INSTANCE)),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Blocks until `pane` is removed, `limit` passes, or the user presses Ctrl-C.
fn wait_for_pane_exit(ctx: &Ctx<'_>, pane: u64, limit: Option<Duration>) -> Result<(), CtlError> {
    let mut events = ctx.connect().and_then(Client::subscribe)?;
    catch_sigint();
    let t0 = Instant::now();
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) || limit.is_some_and(|l| t0.elapsed() >= l) {
            return Ok(());
        }
        match events.next_event()? {
            Some(Event::PaneRemoved { pane: p }) if p == pane => return Ok(()),
            _ => {}
        }
    }
}

/// Prints `e` on stderr and returns its exit code, for the commands that are not `dispatch`ed.
fn fail(e: impl Into<CtlError>) -> i32 {
    let e = e.into();
    let opts = RenderOpts::detect(false, false);
    let _ = std::io::stderr().write_all(render_error(&e, false, &opts).as_bytes());
    e.code
}

/// `microterm record -o FILE -- CMD`: runs CMD in a new tab and records it until it exits.
pub fn record(args: RecordArgs) -> i32 {
    let mut c = match Client::connect() {
        Ok(c) => c,
        Err(e) => return fail(e),
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
        Err(e) => return fail(e),
    };
    let spawned = c
        .call(&Request::TabNew(SpawnParams {
            command: Some(command),
            title: args.command.first().cloned(),
            cols: args.cols,
            rows: args.rows,
            ..Default::default()
        }))
        .map_err(CtlError::from)
        .and_then(decode::<Spawned>);
    let pane = match spawned {
        Ok(s) => s.pane,
        Err(e) => return fail(e),
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
        return fail(e);
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
            Err(e) => return fail(e),
        }
    }
}

/// `microterm __replay FILE --speed S`: prints the output events with their original timing,
/// then idles so the tab stays open for reading.
pub fn replay_internal(file: &Path, speed: f64) -> i32 {
    let f = match std::fs::File::open(file) {
        Ok(f) => f,
        Err(e) => {
            return fail(CtlError::new(
                exit::NOT_FOUND,
                format!("cannot open {}: {e}", file.display()),
            ))
        }
    };
    let reader = match mt_record::Reader::new(std::io::BufReader::new(f)) {
        Ok(r) => r,
        Err(e) => return fail(CtlError::usage(format!("{}: {e}", file.display()))),
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
            Err(e) => return fail(CtlError::usage(e)),
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

/// A `pane run --in` TARGET: a pane id, a selector, or (empty) the caller's own pane.
fn in_target(t: &str, env: CallerEnv) -> Target {
    let t = t.trim();
    match t.parse::<u64>() {
        Ok(id) => Target::pane(id),
        Err(_) if t.is_empty() => Target::default().with_env(Resource::Pane, env),
        Err(_) => Target::matching(t),
    }
}

/// The exit status `--propagate-exit` asks for: the command's own, except that statuses which
/// would read as ctl's own exit codes (1-9) become 1.
fn propagated(r: &RunResult) -> Result<(), i32> {
    let code = match (r.exit_code, r.signal) {
        (Some(c), _) => c,
        (None, Some(sig)) => 128 + sig,
        (None, None) => 1,
    };
    match code {
        0 => Ok(()),
        1..=9 => Err(1),
        c => Err(c.clamp(10, 255)),
    }
}

/// The flags of `pane wait`, before they are sorted into a target and conditions.
struct WaitArgs {
    target: Option<String>,
    pane: Option<u64>,
    select: Option<String>,
    matches: Vec<String>,
    text: Vec<String>,
    idle: Option<u64>,
    exit: bool,
    command_finished: bool,
    timeout: Option<u64>,
    new_only: bool,
}

/// `--match` values that read as pane selectors choose the pane; the others are regexes.
fn wait_params(a: WaitArgs, env: CallerEnv) -> WaitParams {
    let mut selectors: Vec<String> = a.select.into_iter().collect();
    let mut regexes = a.text;
    let mut pane = a.pane;
    if let Some(t) = a.target {
        match t.trim().parse::<u64>() {
            Ok(id) => pane = pane.or(Some(id)),
            Err(_) => selectors.push(t),
        }
    }
    for m in a.matches {
        if Selector::is_pane_selector(&m) {
            selectors.push(m);
        } else {
            regexes.push(m);
        }
    }
    let target = Target {
        pane,
        expr: (!selectors.is_empty()).then(|| selectors.join(",")),
        ..Default::default()
    }
    .with_env(Resource::Pane, env);
    WaitParams {
        target,
        matches: regexes,
        idle_ms: a.idle,
        exit: a.exit,
        command_finished: a.command_finished,
        timeout_ms: a.timeout,
        new_only: a.new_only,
    }
}

#[cfg(test)]
mod wait_arg_tests {
    use super::*;

    fn args(matches: &[&str]) -> WaitArgs {
        WaitArgs {
            target: None,
            pane: None,
            select: None,
            matches: matches.iter().map(|s| s.to_string()).collect(),
            text: vec![],
            idle: None,
            exit: false,
            command_finished: false,
            timeout: None,
            new_only: false,
        }
    }

    #[test]
    fn match_values_are_sorted_into_selector_and_regexes() {
        let p = wait_params(
            args(&["title:dev", "ready on", "cwd:~/x"]),
            CallerEnv::default(),
        );
        assert_eq!(p.target.expr.as_deref(), Some("title:dev,cwd:~/x"));
        assert_eq!(p.matches, vec!["ready on".to_string()]);
    }

    #[test]
    fn explicit_text_is_never_a_selector_and_the_caller_pane_is_the_default() {
        let mut a = args(&[]);
        a.text = vec!["title:dev".into()];
        let env = CallerEnv {
            pane: Some(7),
            ..Default::default()
        };
        let p = wait_params(a, env);
        assert_eq!(p.matches, vec!["title:dev".to_string()]);
        assert_eq!(p.target.pane, Some(7));
        assert!(p.target.expr.is_none());
        // An explicit selector beats the environment.
        let p = wait_params(args(&["title:dev"]), env);
        assert_eq!(p.target.pane, None);
    }

    #[test]
    fn positional_target_is_an_id_or_a_selector() {
        let mut a = args(&[]);
        a.target = Some("12".into());
        assert_eq!(wait_params(a, CallerEnv::default()).target.pane, Some(12));
        let mut a = args(&[]);
        a.target = Some("title:x*".into());
        assert_eq!(
            wait_params(a, CallerEnv::default()).target.expr.as_deref(),
            Some("title:x*")
        );
    }

    #[test]
    fn propagated_status_avoids_the_ctl_exit_codes() {
        let r = |c, s| RunResult {
            pane: 1,
            tab: None,
            exit_code: c,
            signal: s,
            duration_ms: None,
            output: None,
            truncated: false,
            timed_out: false,
        };
        assert_eq!(propagated(&r(Some(0), None)), Ok(()));
        assert_eq!(propagated(&r(Some(1), None)), Err(1));
        assert_eq!(
            propagated(&r(Some(6), None)),
            Err(1),
            "collides with timeout"
        );
        assert_eq!(propagated(&r(Some(42), None)), Err(42));
        assert_eq!(propagated(&r(None, Some(9)),), Err(137));
    }

    #[test]
    fn in_targets_accept_ids_selectors_and_the_environment() {
        let env = CallerEnv {
            pane: Some(3),
            ..Default::default()
        };
        assert_eq!(in_target("12", env).pane, Some(12));
        assert_eq!(in_target("title:x", env).expr.as_deref(), Some("title:x"));
        assert_eq!(in_target("", env).pane, Some(3));
    }
}
