//! `trigger.*` IPC (SPEC §10.11.6): list, get, enable, disable, pause, resume and test. There is
//! **no** method that creates, edits or deletes a rule, and `enable`, `disable`, `pause` and
//! `resume` change the running session only — they are never written back to `triggers.toml`, so
//! a script cannot leave a rule permanently off. While the gate is off every method answers
//! `disabled` (ctl exit code 10). The MCP server exposes none of this.

use gpui::{App, BorrowAppContext};
use mt_mux::ctl::Verb;
use mt_proto::{Request, RpcError};
use mt_triggers::{FireCtx, Rule, When};
use serde_json::{json, Value};

use super::engine::Engine;
use super::gate::TriggerGate;
use super::runtime::{self, TriggerRuntime};
use crate::mux_host;

pub fn disabled() -> RpcError {
    RpcError::disabled("triggers are disabled: set `triggers.enabled = true` in the config")
}

fn not_found(id: &str) -> RpcError {
    RpcError::not_found(format!("no trigger with id \"{id}\""))
}

fn kind_of(r: &Rule) -> &'static str {
    match r.when {
        When::Text { .. } => "text",
        When::Event { .. } => "event",
    }
}

fn row(e: &Engine, r: &Rule) -> Value {
    let s = e.stats(&r.id);
    json!({
        "id": r.id,
        "name": r.name,
        "enabled": e.is_enabled(&r.id),
        "kind": kind_of(r),
        "panes": r.panes,
        "fired": s.fired,
        "suppressed": e.suppressed(&r.id),
        "last_error": s.last_error,
    })
}

/// Handles one `trigger.*` request.
pub fn handle(cx: &mut App, request: Request) -> Result<Value, RpcError> {
    if !TriggerGate::of(cx).enabled || !runtime::active(cx) {
        return Err(disabled());
    }
    match request {
        Request::TriggerList => {
            let dropped = runtime::dropped_lines(cx);
            let rt = cx.global::<TriggerRuntime>();
            Ok(json!({
                "triggers": rt.engine.rules().map(|r| row(&rt.engine, r)).collect::<Vec<_>>(),
                "paused": rt.engine.paused(),
                "dropped_lines": dropped,
            }))
        }
        Request::TriggerGet(p) => {
            let rt = cx.global::<TriggerRuntime>();
            let r = rt.engine.rule(&p.id).ok_or_else(|| not_found(&p.id))?;
            let mut v = row(&rt.engine, r);
            v["cooldown_ms"] = json!(r.cooldown_ms);
            v["rule"] = serde_json::to_value(r).map_err(|e| RpcError::internal(e.to_string()))?;
            v["last_fired_ms"] = json!(rt.engine.stats(&r.id).last_fired_ms);
            Ok(v)
        }
        Request::TriggerEnable(p) => set_enabled(cx, &p.id, true),
        Request::TriggerDisable(p) => set_enabled(cx, &p.id, false),
        Request::TriggerPause | Request::TriggerResume => {
            let on = matches!(request, Request::TriggerPause);
            cx.update_global::<TriggerRuntime, _>(|rt, _| rt.engine.set_paused(on));
            Ok(json!({"paused": on}))
        }
        Request::TriggerTest(p) => {
            let rule = cx
                .global::<TriggerRuntime>()
                .engine
                .rule(&p.id)
                .cloned()
                .ok_or_else(|| not_found(&p.id))?;
            let rows = mux_host::rows(cx);
            let pane = mt_mux::ctl::resolve_one(
                &p.target,
                &rows.panes,
                rows.focused_pane,
                Verb::ReadOnly,
            )?;
            let row = rows.panes.iter().find(|r| r.id == pane).cloned();
            let base = row.map_or_else(FireCtx::default, |r| FireCtx {
                pane: r.id.to_string(),
                title: r.title,
                cwd: r.cwd.unwrap_or_default(),
                domain: r.domain,
                ..FireCtx::default()
            });
            let firings = if rule.is_text() {
                let term = mux_host::terminal(cx, pane)
                    .ok_or_else(|| RpcError::not_found(format!("pane {pane} has no terminal")))?;
                let lines: Vec<String> = runtime::recent_lines(&term, rule.scope)
                    .into_iter()
                    .map(|l| l.1)
                    .collect();
                mt_triggers::dry_run_text(&rule, &lines, &base)
            } else {
                mt_triggers::dry_run_event(&rule, &base)
            };
            Ok(json!({
                "id": rule.id,
                "pane": pane,
                "ran": false,
                "firings": firings.iter().map(|f| json!({
                    "input": f.input,
                    "actions": f.intents.iter().map(|i| i.describe()).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }))
        }
        other => Err(RpcError::bad_request(format!(
            "not a trigger method: {}",
            other.method()
        ))),
    }
}

/// `enable` / `disable`: this session only, never written to `triggers.toml`.
fn set_enabled(cx: &mut App, id: &str, on: bool) -> Result<Value, RpcError> {
    if !cx.update_global::<TriggerRuntime, _>(|rt, _| rt.engine.set_session_enabled(id, on)) {
        return Err(not_found(id));
    }
    runtime::sync_taps(cx, false);
    Ok(json!({"id": id, "enabled": on, "session_only": true}))
}
