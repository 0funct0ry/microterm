//! MCP tool definitions (SPEC §12.5). Each tool names the IPC method it wraps, carries its JSON
//! input schema, and turns its JSON arguments into the same [`Request`] the matching `ctl`
//! command sends, so the result is the same JSON as `--json`.
//!
//! The argument structs below deserialize the tool input; tests pin the schemas and check that
//! every schema property is accepted by its struct, so the two cannot drift.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    CallerEnv, GetTextParams, NotifyLevel, NotifyParams, Request, Resource, RunParams,
    SendTextParams, SpawnParams, StatusSetParams, StatusState, SubscribeParams, Target, WaitParams,
};

/// How a tool is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// One IPC request/response.
    Call,
    /// Long-poll over a `subscribe` stream (MCP has no streaming).
    Events,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// The IPC method this tool wraps (an entry of [`crate::METHODS`]).
    pub method: &'static str,
    pub kind: ToolKind,
    schema: fn() -> Value,
}

impl ToolDef {
    pub fn input_schema(&self) -> Value {
        (self.schema)()
    }
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":props,"required":required,"additionalProperties":false})
}

/// Properties shared by every tool that addresses one pane.
fn pane_props(extra: Value) -> Value {
    let mut p = json!({
        "pane": {"type":"integer","description":"Pane id. Default: the caller's own pane ($MICROTERM_PANE) if the MCP server runs inside one."},
        "match": {"type":"string","description":"Selector instead of an id, e.g. 'title:build*,domain:local' (see the selectors docs). Must match exactly one pane unless `all` is allowed."}
    });
    if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
        a.extend(b.clone());
    }
    p
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "microterm_list",
        description: "List the whole tree: workspaces, windows, tabs and panes (ids, titles, cwd, status).",
        method: "mux.list",
        kind: ToolKind::Call,
        schema: || obj(json!({}), &[]),
    },
    ToolDef {
        name: "microterm_tab_new",
        description: "Open a new tab, optionally running a command (argv, no shell parsing). Returns the new tab, pane and window ids.",
        method: "tab.new",
        kind: ToolKind::Call,
        schema: || {
            obj(
                json!({
                    "title": {"type":"string"},
                    "cwd": {"type":"string"},
                    "command": {"type":"array","items":{"type":"string"},"description":"argv; use [\"sh\",\"-c\",\"…\"] for shell syntax"},
                    "window": {"type":"integer"},
                    "workspace": {"type":"string"},
                    "domain": {"type":"string"}
                }),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_tab_close",
        description: "Close tabs by id or selector. Fails with confirmation_required unless `force` is true when closing would prompt.",
        method: "tab.close",
        kind: ToolKind::Call,
        schema: || {
            obj(
                json!({
                    "tab": {"type":"integer"},
                    "match": {"type":"string"},
                    "force": {"type":"boolean"},
                    "all": {"type":"boolean"}
                }),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_pane_run",
        description: "Run a command and return its output and exit status. Spawn mode (give `command`): opens a tab, blocks until exit unless `detach`. In-pane mode (give `in_pane`/`in_match` and `line`): types the line into a shell pane with shell integration and returns that command's output.",
        method: "pane.run",
        kind: ToolKind::Call,
        schema: || {
            obj(
                json!({
                    "command": {"type":"array","items":{"type":"string"}},
                    "title": {"type":"string"},
                    "cwd": {"type":"string"},
                    "domain": {"type":"string"},
                    "window": {"type":"integer"},
                    "new_window": {"type":"boolean"},
                    "keep": {"type":"boolean"},
                    "detach": {"type":"boolean"},
                    "timeout_ms": {"type":"integer"},
                    "in_pane": {"type":"integer"},
                    "in_match": {"type":"string"},
                    "line": {"type":"string"}
                }),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_pane_wait",
        description: "Block until a pane produces text matching a regex, goes idle, exits, or finishes a command (first condition wins). Times out with a timeout error; the command keeps running.",
        method: "pane.wait",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({
                    "matches": {"type":"array","items":{"type":"string"},"description":"Regexes over rendered text"},
                    "idle_ms": {"type":"integer"},
                    "exit": {"type":"boolean"},
                    "command_finished": {"type":"boolean"},
                    "timeout_ms": {"type":"integer","description":"Default 60000"},
                    "new_only": {"type":"boolean"}
                })),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_pane_read",
        description: "Read a pane's text: screen rows, a line range into scrollback, or the output of the N-th last finished command (`last_command`, needs shell integration).",
        method: "pane.get_text",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({
                    "start_line": {"type":"integer","description":"0 = first visible row; negative reaches into scrollback"},
                    "end_line": {"type":"integer"},
                    "escapes": {"type":"boolean"},
                    "last_command": {"type":"integer","minimum":1}
                })),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_pane_send_text",
        description: "Type text into a pane (bracketed paste unless `paste` is false). Include a trailing newline to press Enter. Never falls back to the focused pane.",
        method: "pane.send_text",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({
                    "text": {"type":"string"},
                    "paste": {"type":"boolean","default":true},
                    "all": {"type":"boolean"}
                })),
                &["text"],
            )
        },
    },
    ToolDef {
        name: "microterm_pane_close",
        description: "Close panes by id or selector. Needs `force` when closing would prompt.",
        method: "pane.close",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({"force":{"type":"boolean"},"all":{"type":"boolean"}})),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_notify",
        description: "Raise an attention badge (and toast / OS notification per config) on a pane's tab to tell the human something.",
        method: "pane.notify",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({
                    "message": {"type":"string"},
                    "title": {"type":"string"},
                    "level": {"type":"string","enum":["info","attention","error"]},
                    "desktop": {"type":"boolean"},
                    "all": {"type":"boolean"}
                })),
                &["message"],
            )
        },
    },
    ToolDef {
        name: "microterm_status_set",
        description: "Set a pane's status (dot, progress line, tooltip). Use state `waiting` to say a human is needed.",
        method: "pane.status_set",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({
                    "state": {"type":"string","enum":["idle","working","waiting","done","error"]},
                    "text": {"type":"string"},
                    "progress": {"type":"integer","minimum":0,"maximum":100},
                    "ttl_ms": {"type":"integer"},
                    "all": {"type":"boolean"}
                })),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_status_clear",
        description: "Clear a pane's status.",
        method: "pane.status_clear",
        kind: ToolKind::Call,
        schema: || obj(pane_props(json!({"all":{"type":"boolean"}})), &[]),
    },
    ToolDef {
        name: "microterm_snippet_list",
        description: "List the user's saved snippets (id, title, folder, tags, run, and the prompts each one asks for). Only available while snippets are enabled in the config.",
        method: "snippet.list",
        kind: ToolKind::Call,
        schema: || {
            obj(
                json!({
                    "folder": {"type":"string"},
                    "tag": {"type":"string"},
                    "query": {"type":"string","description":"Fuzzy search over title, body, tags, folder and description"}
                }),
                &[],
            )
        },
    },
    ToolDef {
        name: "microterm_snippet_expand",
        description: "Show what a snippet expands to for a pane, without typing it. Give a value in `vars` for every prompt (see microterm_snippet_list). Snippets that ask for a secret are refused.",
        method: "snippet.expand",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({
                    "id": {"type":"string"},
                    "vars": {"type":"object","additionalProperties":{"type":"string"},"description":"Prompt label -> value"}
                })),
                &["id"],
            )
        },
    },
    ToolDef {
        name: "microterm_snippet_insert",
        description: "Type a saved snippet into a pane, optionally pressing Enter. Never falls back to the focused pane. Give a value in `vars` for every prompt. Snippets that ask for a secret are refused.",
        method: "snippet.insert",
        kind: ToolKind::Call,
        schema: || {
            obj(
                pane_props(json!({
                    "id": {"type":"string"},
                    "vars": {"type":"object","additionalProperties":{"type":"string"},"description":"Prompt label -> value"},
                    "run": {"type":"boolean","description":"Press Enter afterwards (default: the snippet's own setting)"},
                    "all": {"type":"boolean"}
                })),
                &["id"],
            )
        },
    },
    ToolDef {
        name: "microterm_events_next",
        description: "Long-poll for mux events (command finished, notification, status changed, …). Returns the events seen within `timeout_ms` as a JSON array (possibly empty). The first call starts the subscription; events between calls are buffered.",
        method: "subscribe",
        kind: ToolKind::Events,
        schema: || {
            obj(
                json!({
                    "types": {"type":"array","items":{"type":"string"},"description":"Dotted event types, e.g. pane.command_finished; trailing .* matches a family"},
                    "match": {"type":"string"},
                    "snapshot": {"type":"boolean"},
                    "timeout_ms": {"type":"integer","description":"Default 30000, max 120000"},
                    "max": {"type":"integer","description":"Return at most this many events (default 50)"}
                }),
                &[],
            )
        },
    },
];

/// The snippet tools are listed only while `snippets.enabled = true` (SPEC §12.5).
pub fn is_snippet_tool(name: &str) -> bool {
    name.starts_with("microterm_snippet_")
}

pub fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

#[derive(Deserialize, Default)]
struct PaneSel {
    pane: Option<u64>,
    #[serde(rename = "match")]
    expr: Option<String>,
}

impl PaneSel {
    fn target(&self, env: CallerEnv, all: bool, force: bool) -> Target {
        Target {
            pane: self.pane,
            expr: self.expr.clone(),
            all,
            force,
            ..Default::default()
        }
        .with_env(Resource::Pane, env)
    }
}

/// Rejects argument names the tool's schema does not list (serde cannot do this across
/// `flatten`).
fn check_keys(def: &ToolDef, args: &Value) -> Result<(), String> {
    let (Some(given), schema) = (args.as_object(), def.input_schema()) else {
        return Ok(());
    };
    for k in given.keys() {
        if schema["properties"].get(k).is_none() {
            return Err(format!("invalid arguments: unknown argument {k:?}"));
        }
    }
    Ok(())
}

fn parse<T: for<'de> Deserialize<'de>>(args: &Value) -> Result<T, String> {
    let v = if args.is_null() {
        json!({})
    } else {
        args.clone()
    };
    serde_json::from_value(v).map_err(|e| format!("invalid arguments: {e}"))
}

/// The IPC request for a [`ToolKind::Call`] tool. `env` is the MCP server's own
/// `$MICROTERM_*` environment (SPEC §12.3 step 3).
pub fn request(name: &str, args: &Value, env: CallerEnv) -> Result<Request, String> {
    let def = find(name).ok_or_else(|| format!("unknown tool {name:?}"))?;
    check_keys(def, args)?;
    #[derive(Deserialize, Default)]
    struct TabNew {
        title: Option<String>,
        cwd: Option<String>,
        command: Option<Vec<String>>,
        window: Option<u64>,
        workspace: Option<String>,
        domain: Option<String>,
    }
    #[derive(Deserialize, Default)]
    struct TabClose {
        tab: Option<u64>,
        #[serde(rename = "match")]
        expr: Option<String>,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        all: bool,
    }
    #[derive(Deserialize, Default)]
    struct Run {
        command: Option<Vec<String>>,
        title: Option<String>,
        cwd: Option<String>,
        domain: Option<String>,
        window: Option<u64>,
        #[serde(default)]
        new_window: bool,
        #[serde(default)]
        keep: bool,
        #[serde(default)]
        detach: bool,
        timeout_ms: Option<u64>,
        in_pane: Option<u64>,
        in_match: Option<String>,
        line: Option<String>,
    }
    #[derive(Deserialize, Default)]
    struct Wait {
        #[serde(flatten)]
        sel: PaneSel,
        #[serde(default)]
        matches: Vec<String>,
        idle_ms: Option<u64>,
        #[serde(default)]
        exit: bool,
        #[serde(default)]
        command_finished: bool,
        timeout_ms: Option<u64>,
        #[serde(default)]
        new_only: bool,
    }
    #[derive(Deserialize, Default)]
    struct Read {
        #[serde(flatten)]
        sel: PaneSel,
        start_line: Option<i64>,
        end_line: Option<i64>,
        #[serde(default)]
        escapes: bool,
        last_command: Option<u32>,
    }
    #[derive(Deserialize)]
    struct Send {
        #[serde(flatten)]
        sel: PaneSel,
        text: String,
        #[serde(default = "yes")]
        paste: bool,
        #[serde(default)]
        all: bool,
    }
    #[derive(Deserialize, Default)]
    struct Close {
        #[serde(flatten)]
        sel: PaneSel,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        all: bool,
    }
    #[derive(Deserialize)]
    struct Notify {
        #[serde(flatten)]
        sel: PaneSel,
        message: String,
        title: Option<String>,
        level: Option<NotifyLevel>,
        #[serde(default)]
        desktop: bool,
        #[serde(default)]
        all: bool,
    }
    #[derive(Deserialize, Default)]
    struct StatusSet {
        #[serde(flatten)]
        sel: PaneSel,
        state: Option<StatusState>,
        text: Option<String>,
        progress: Option<u8>,
        ttl_ms: Option<u64>,
        #[serde(default)]
        all: bool,
    }
    #[derive(Deserialize, Default)]
    struct StatusClear {
        #[serde(flatten)]
        sel: PaneSel,
        #[serde(default)]
        all: bool,
    }
    #[derive(Deserialize, Default)]
    struct SnippetList {
        folder: Option<String>,
        tag: Option<String>,
        query: Option<String>,
    }
    #[derive(Deserialize)]
    struct SnippetUse {
        #[serde(flatten)]
        sel: PaneSel,
        id: String,
        #[serde(default)]
        vars: std::collections::BTreeMap<String, String>,
        run: Option<bool>,
        #[serde(default)]
        all: bool,
    }
    fn yes() -> bool {
        true
    }

    Ok(match name {
        "microterm_list" => Request::MuxList,
        "microterm_tab_new" => {
            let a: TabNew = parse(args)?;
            Request::TabNew(SpawnParams {
                window: a.window,
                workspace: a.workspace,
                domain: a.domain,
                cwd: a.cwd,
                title: a.title,
                command: a.command.filter(|c| !c.is_empty()),
                ..Default::default()
            })
        }
        "microterm_tab_close" => {
            let a: TabClose = parse(args)?;
            Request::TabClose(
                Target {
                    tab: a.tab,
                    expr: a.expr,
                    all: a.all,
                    force: a.force,
                    ..Default::default()
                }
                .with_env(Resource::Tab, env),
            )
        }
        "microterm_pane_run" => {
            let a: Run = parse(args)?;
            let mut p = RunParams {
                keep: a.keep,
                detach: a.detach,
                timeout_ms: a.timeout_ms,
                ..Default::default()
            };
            if a.in_pane.is_some() || a.in_match.is_some() {
                let line = a.line.ok_or("`line` is required with in_pane / in_match")?;
                p.in_pane = Some(Target {
                    pane: a.in_pane,
                    expr: a.in_match,
                    ..Default::default()
                });
                p.line = Some(line);
            } else {
                let command = a
                    .command
                    .filter(|c| !c.is_empty())
                    .ok_or("give `command` (spawn mode) or `in_pane`/`in_match` with `line`")?;
                p.spawn = SpawnParams {
                    window: a.window,
                    new_window: a.new_window,
                    domain: a.domain,
                    cwd: a.cwd,
                    title: a.title,
                    command: Some(command),
                    ..Default::default()
                };
            }
            Request::PaneRun(p)
        }
        "microterm_pane_wait" => {
            let a: Wait = parse(args)?;
            Request::PaneWait(WaitParams {
                target: a.sel.target(env, false, false),
                matches: a.matches,
                idle_ms: a.idle_ms,
                exit: a.exit,
                command_finished: a.command_finished,
                timeout_ms: a.timeout_ms,
                new_only: a.new_only,
            })
        }
        "microterm_pane_read" => {
            let a: Read = parse(args)?;
            Request::PaneGetText(GetTextParams {
                target: a.sel.target(env, false, false),
                start_line: a.start_line,
                end_line: a.end_line,
                escapes: a.escapes,
                last_command: a.last_command,
            })
        }
        "microterm_pane_send_text" => {
            let a: Send = parse(args)?;
            Request::PaneSendText(SendTextParams {
                target: a.sel.target(env, a.all, false),
                text: a.text,
                paste: a.paste,
            })
        }
        "microterm_pane_close" => {
            let a: Close = parse(args)?;
            Request::PaneClose(a.sel.target(env, a.all, a.force))
        }
        "microterm_notify" => {
            let a: Notify = parse(args)?;
            if a.message.trim().is_empty() && a.title.as_deref().unwrap_or("").trim().is_empty() {
                return Err("nothing to say: give a message or title".into());
            }
            Request::PaneNotify(NotifyParams {
                target: a.sel.target(env, a.all, false),
                title: a.title,
                level: a.level.unwrap_or_default(),
                desktop: a.desktop,
                message: a.message,
            })
        }
        "microterm_status_set" => {
            let a: StatusSet = parse(args)?;
            Request::PaneStatusSet(StatusSetParams {
                target: a.sel.target(env, a.all, false),
                state: a.state,
                text: a.text,
                progress: a.progress,
                ttl_ms: a.ttl_ms,
            })
        }
        "microterm_status_clear" => {
            let a: StatusClear = parse(args)?;
            Request::PaneStatusClear(a.sel.target(env, a.all, false))
        }
        "microterm_snippet_list" => {
            let a: SnippetList = parse(args)?;
            Request::SnippetList(crate::SnippetListParams {
                folder: a.folder,
                tag: a.tag,
                query: a.query,
            })
        }
        "microterm_snippet_expand" | "microterm_snippet_insert" => {
            let a: SnippetUse = parse(args)?;
            let insert = name == "microterm_snippet_insert";
            if !insert && (a.run.is_some() || a.all) {
                return Err("invalid arguments: `run` and `all` only apply to insert".into());
            }
            let p = crate::SnippetUseParams {
                target: a.sel.target(env, a.all, false),
                id: Some(a.id),
                body: None,
                vars: a.vars,
                run: a.run,
                // Agents never pass secrets through (SPEC §12.5).
                agent: true,
            };
            if insert {
                Request::SnippetInsert(p)
            } else {
                Request::SnippetExpand(p)
            }
        }
        other => return Err(format!("unknown tool {other:?}")),
    })
}

/// Subscription filters of a `microterm_events_next` call plus its poll options.
pub struct EventsArgs {
    pub params: SubscribeParams,
    pub timeout_ms: u64,
    pub max: usize,
}

pub fn events_args(args: &Value) -> Result<EventsArgs, String> {
    #[derive(Deserialize, Default)]
    struct A {
        #[serde(default)]
        types: Vec<String>,
        #[serde(rename = "match")]
        expr: Option<String>,
        #[serde(default)]
        snapshot: bool,
        timeout_ms: Option<u64>,
        max: Option<usize>,
    }
    check_keys(find("microterm_events_next").unwrap(), args)?;
    let a: A = parse(args)?;
    Ok(EventsArgs {
        params: SubscribeParams {
            types: a.types,
            expr: a.expr,
            snapshot: a.snapshot,
        },
        timeout_ms: a.timeout_ms.unwrap_or(30_000).min(120_000),
        max: a.max.unwrap_or(50).clamp(1, 500),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_wraps_a_known_method_with_unique_name() {
        let mut names = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(names.insert(t.name), "duplicate {}", t.name);
            assert!(
                crate::METHODS.contains(&t.method),
                "{} -> {}",
                t.name,
                t.method
            );
            assert_eq!(t.input_schema()["type"], "object");
        }
    }

    /// Every schema property must be accepted by the argument struct (`deny_unknown_fields`),
    /// and every required one must be listed.
    #[test]
    fn schema_properties_are_accepted_by_the_argument_structs() {
        let sample = |schema: &Value| -> Value {
            let mut o = serde_json::Map::new();
            for (k, p) in schema["properties"].as_object().unwrap() {
                let v = match p["type"].as_str().unwrap() {
                    "integer" => json!(1),
                    "boolean" => json!(false),
                    "string" => match p.get("enum") {
                        Some(e) => e[0].clone(),
                        None => json!("x"),
                    },
                    "array" => json!(["x"]),
                    "object" => json!({"k": "v"}),
                    t => panic!("type {t}"),
                };
                o.insert(k.clone(), v);
            }
            Value::Object(o)
        };
        for t in TOOLS {
            let schema = t.input_schema();
            let args = sample(&schema);
            match t.kind {
                ToolKind::Call => {
                    // pane_run's modes are exclusive; send the spawn-mode subset.
                    let args = if t.name == "microterm_pane_run" {
                        let mut a = args.as_object().unwrap().clone();
                        a.remove("in_pane");
                        a.remove("in_match");
                        a.remove("line");
                        Value::Object(a)
                    } else {
                        args
                    };
                    request(t.name, &args, CallerEnv::default())
                        .unwrap_or_else(|e| panic!("{}: {e}", t.name));
                }
                ToolKind::Events => {
                    events_args(&args).unwrap();
                }
            }
            for r in schema["required"].as_array().unwrap() {
                assert!(schema["properties"].get(r.as_str().unwrap()).is_some());
            }
        }
    }

    #[test]
    fn unknown_arguments_and_missing_modes_are_rejected() {
        let env = CallerEnv::default();
        assert!(request("microterm_pane_read", &json!({"bogus":1}), env).is_err());
        assert!(request("microterm_pane_run", &json!({}), env).is_err());
        assert!(request("microterm_pane_run", &json!({"in_pane":1}), env).is_err());
        assert!(request("microterm_pane_send_text", &json!({}), env).is_err());
        assert!(request("nope", &json!({}), env).is_err());
    }

    #[test]
    fn targets_fall_back_to_the_callers_pane() {
        let env = CallerEnv {
            pane: Some(7),
            ..Default::default()
        };
        match request("microterm_notify", &json!({"message":"hi"}), env).unwrap() {
            Request::PaneNotify(n) => assert_eq!(n.target.pane, Some(7)),
            r => panic!("{r:?}"),
        }
        match request("microterm_pane_read", &json!({"pane": 3}), env).unwrap() {
            Request::PaneGetText(g) => assert_eq!(g.target.pane, Some(3)),
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn events_args_clamp() {
        let a = events_args(&json!({"timeout_ms": 9_999_999, "max": 0})).unwrap();
        assert_eq!((a.timeout_ms, a.max), (120_000, 1));
        let a = events_args(&Value::Null).unwrap();
        assert_eq!((a.timeout_ms, a.max), (30_000, 50));
    }

    #[test]
    fn tool_list_is_pinned() {
        let names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "microterm_list",
                "microterm_tab_new",
                "microterm_tab_close",
                "microterm_pane_run",
                "microterm_pane_wait",
                "microterm_pane_read",
                "microterm_pane_send_text",
                "microterm_pane_close",
                "microterm_notify",
                "microterm_status_set",
                "microterm_status_clear",
                "microterm_snippet_list",
                "microterm_snippet_expand",
                "microterm_snippet_insert",
                "microterm_events_next",
            ]
        );
    }

    #[test]
    fn snippet_tools_are_flagged_as_agent_requests() {
        let env = CallerEnv {
            pane: Some(4),
            ..Default::default()
        };
        match request(
            "microterm_snippet_insert",
            &json!({"id":"a","vars":{"X":"1"},"run":true,"all":true}),
            env,
        )
        .unwrap()
        {
            Request::SnippetInsert(p) => {
                assert!(p.agent && p.run == Some(true) && p.target.all);
                assert_eq!(p.target.pane, Some(4));
                assert_eq!(p.vars["X"], "1");
                assert_eq!(p.id.as_deref(), Some("a"));
            }
            r => panic!("{r:?}"),
        }
        match request("microterm_snippet_expand", &json!({"id":"a"}), env).unwrap() {
            Request::SnippetExpand(p) => assert!(p.agent),
            r => panic!("{r:?}"),
        }
        assert!(request(
            "microterm_snippet_expand",
            &json!({"id":"a","run":true}),
            env
        )
        .is_err());
        assert!(request("microterm_snippet_insert", &json!({}), env).is_err());
        assert!(is_snippet_tool("microterm_snippet_list") && !is_snippet_tool("microterm_list"));
    }
}
