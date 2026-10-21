//! Host-independent pieces of the IPC methods (SPEC §12.2), shared by the GUI host
//! (`mux_host`) and the headless host used by the CLI end-to-end tests: mux snapshots, event
//! conversion, text extraction, pasting and `record.start`/`record.stop`.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use mt_mux::ctl::{self, Live, PaneRow, Row, Rows, TabRow, Verb, WindowRow};
use mt_mux::{ConfirmClose, Id, Mux, MuxEvent, SpawnSpec};
use mt_proto::{
    CommandText, Event, GetTextParams, Request, RpcError, SendTextParams, SpawnParams,
    TabFocusParams, Target, Text,
};
use mt_term::Terminal;
use serde_json::Value;

pub fn event_of(e: &MuxEvent) -> Event {
    match *e {
        MuxEvent::PaneAdded { pane } => Event::PaneAdded { pane },
        MuxEvent::PaneRemoved { pane } => Event::PaneRemoved { pane },
        MuxEvent::PaneOutput { pane } => Event::PaneOutput { pane },
        MuxEvent::PaneFocused { pane } => Event::PaneFocused { pane },
        MuxEvent::LayoutChanged { tab } => Event::LayoutChanged { tab },
        MuxEvent::TitleChanged { pane } => Event::TitleChanged { pane },
        MuxEvent::CwdChanged { pane } => Event::CwdChanged { pane },
        MuxEvent::StatusChanged { pane } => Event::StatusChanged { pane },
        MuxEvent::Notification {
            pane,
            level,
            ref title,
            ref message,
            source,
        } => Event::Notification {
            pane,
            level,
            title: title.clone(),
            message: message.clone(),
            source: source.as_str().into(),
        },
        MuxEvent::PaneExited { pane, status } => Event::PaneExited { pane, status },
        MuxEvent::CommandFinished { pane, exit_code } => Event::CommandFinished { pane, exit_code },
        MuxEvent::MenuInvoked {
            pane,
            ref menu,
            ref item,
            ref kind,
            ref summary,
        } => Event::MenuInvoked {
            pane,
            menu: menu.clone(),
            item: item.clone(),
            kind: kind.clone(),
            summary: summary.clone(),
        },
        MuxEvent::MenuItemsChanged { pane } => Event::MenuItemsChanged { pane },
        MuxEvent::TabAdded { window, tab } => Event::TabAdded { window, tab },
        MuxEvent::TabRemoved { window, tab } => Event::TabRemoved { window, tab },
        MuxEvent::TabMoved {
            window,
            tab,
            from,
            to,
        } => Event::TabMoved {
            window,
            tab,
            from,
            to,
        },
        MuxEvent::WindowAdded { window } => Event::WindowAdded { window },
        MuxEvent::WindowRemoved { window } => Event::WindowRemoved { window },
        MuxEvent::WorkspaceAdded { workspace } => Event::WorkspaceAdded { workspace },
        MuxEvent::WorkspaceRenamed { workspace } => Event::WorkspaceRenamed { workspace },
        MuxEvent::WorkspaceSwitched { from, to } => Event::WorkspaceSwitched { from, to },
        MuxEvent::WorkspaceRemoved { workspace } => Event::WorkspaceRemoved { workspace },
        MuxEvent::DomainStateChanged { domain, ref state } => Event::DomainStateChanged {
            domain,
            state: state.label().into(),
            attempt: match state {
                mt_mux::DomainState::Reconnecting { attempt, .. } => Some(*attempt),
                _ => None,
            },
            reason: match state {
                mt_mux::DomainState::Disconnected { reason } => Some(reason.clone()),
                _ => None,
            },
        },
    }
}

/// The launch spec of a `tab.new` / `window.new`. The domain is resolved by the caller (it needs the app's
/// SSH domains); the spec starts out local.
pub fn spawn_spec(p: &SpawnParams) -> Result<SpawnSpec, RpcError> {
    let command = p.command.clone().filter(|c| !c.is_empty());
    Ok(SpawnSpec {
        cwd: p.cwd.as_deref().map(expand_tilde),
        command,
        title: p.title.clone(),
        ..Default::default()
    })
}

pub fn expand_tilde(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ if path == "~" => std::env::var_os("HOME").map_or_else(|| path.into(), Into::into),
        _ => path.into(),
    }
}

/// The `MICROTERM_*` variables exported into every **local** pane (SPEC §6.1, §12.3): the
/// pane's own ids, so a process inside can address itself, and the IPC socket. SSH-domain
/// panes get none of them (SPEC §12.6 S7).
/// Blanks the ctl variables for a process that must not reach the socket (a menu `spawn`
/// without `ctl = true`): an empty `MICROTERM_UNIX_SOCKET` also stops ctl's socket discovery.
pub fn no_ctl_env() -> Vec<(String, String)> {
    [
        "MICROTERM_UNIX_SOCKET",
        "MICROTERM_PANE",
        "MICROTERM_TAB",
        "MICROTERM_WINDOW",
    ]
    .into_iter()
    .map(|k| (k.to_string(), String::new()))
    .collect()
}

pub fn pane_env(pane: Id, tab: Id, window: Id, socket: Option<&Path>) -> Vec<(String, String)> {
    let mut env = vec![
        ("MICROTERM_PANE".to_string(), pane.to_string()),
        ("MICROTERM_TAB".to_string(), tab.to_string()),
        ("MICROTERM_WINDOW".to_string(), window.to_string()),
    ];
    if let Some(s) = socket {
        env.push((
            "MICROTERM_UNIX_SOCKET".to_string(),
            s.to_string_lossy().into_owned(),
        ));
    }
    env
}

/// Live facts of a pane's terminal for `pane list|get` and the `process:`/`cmd:` selectors.
pub fn live_of(term: &Terminal) -> Live {
    let (cols, rows) = term.size();
    Live {
        process: term.foreground_process().map(|p| p.name),
        size: (cols as u16, rows as u16),
        ..Default::default()
    }
}

/// [`live_of`] plus how the pane's process ended, for panes kept open by `run --keep`.
pub fn live_with_exit(
    term: Option<&Terminal>,
    exit: Option<&crate::ipc_pending::ExitRecord>,
) -> Live {
    let mut live = term.map(live_of).unwrap_or_default();
    if let Some(e) = exit {
        live.exited = true;
        live.exit_code = e.status;
        live.process = None;
    }
    live
}

pub fn policy_of(c: &mt_config::ConfirmClose) -> ConfirmClose {
    match c {
        mt_config::ConfirmClose::Always => ConfirmClose::Always,
        mt_config::ConfirmClose::Process => ConfirmClose::Process,
        mt_config::ConfirmClose::Never => ConfirmClose::Never,
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::internal(e.to_string()))
}

/// The side-effect-free listing methods, shared by both hosts: `mux.list`, `workspace.list`,
/// `window.list`, `tab.list`, `pane.list` and `pane.get`. `None` for any other request.
pub fn read_only(rows: &Rows, req: &Request) -> Option<Result<Value, RpcError>> {
    fn infos<R: Row, T: serde::Serialize>(
        t: &Target,
        rows: &[R],
        f: impl Fn(&R) -> T,
    ) -> Result<Value, RpcError> {
        let items: Vec<T> = ctl::select(t, rows)?.into_iter().map(f).collect();
        to_json(&items)
    }
    Some(match req {
        Request::MuxList => to_json(&rows.tree()),
        Request::WorkspaceList => {
            to_json(&rows.workspaces.iter().map(|w| w.info()).collect::<Vec<_>>())
        }
        Request::WindowList(t) => infos(t, &rows.windows, WindowRow::info),
        Request::TabList(t) => infos(t, &rows.tabs, TabRow::info),
        Request::PaneList(t) => infos(t, &rows.panes, PaneRow::info),
        Request::PaneGet(t) => {
            let id = ctl::resolve_one(t, &rows.panes, rows.focused_pane, Verb::ReadOnly);
            id.and_then(|id| {
                let pane = rows.panes.iter().find(|p| p.id == id).expect("resolved");
                to_json(&pane.info())
            })
        }
        _ => return None,
    })
}

/// The panes a `pane.*` request addresses.
pub fn resolve_panes(rows: &Rows, t: &Target, verb: Verb) -> Result<Vec<Id>, RpcError> {
    ctl::resolve(t, &rows.panes, rows.focused_pane, verb)
}

/// A `pane.split` the model has applied: the new pane and where it came from.
pub struct SplitDone {
    pub result: mt_proto::PaneSplitResult,
    /// The pane that was split (resolved from `--pane`, `--match` or the caller's env).
    pub parent: Id,
    pub axis: mt_mux::Axis,
    /// The new pane's share in permille.
    pub permille: u16,
}

/// `pane.split`, model half: validates, resolves the target and adds the pane. The host then
/// starts a terminal for the returned pane (and closes it again if that fails).
pub fn split_model(
    mux: &mut Mux,
    rows: &Rows,
    p: &mt_proto::PaneSplitParams,
) -> Result<SplitDone, RpcError> {
    let percent = p.percent.unwrap_or(50);
    if !(1..=99).contains(&percent) {
        return Err(RpcError::bad_request("--percent must be between 1 and 99"));
    }
    let pane = resolve_panes(rows, &p.target, Verb::Acting { all_allowed: false })?[0];
    let dir = match (p.direction, p.axis) {
        (Some(d), _) => mt_mux::Dir::from(d),
        (None, mt_proto::SplitAxis::Horizontal) => mt_mux::Dir::Right,
        (None, mt_proto::SplitAxis::Vertical) => mt_mux::Dir::Down,
    };
    let cwd = p.cwd.as_deref().map(expand_tilde);
    let new = mux.split_pane_toward(pane, dir, f32::from(percent) / 100.0, None, cwd)?;
    let (window, index) = mux
        .locate_pane(new)
        .ok_or_else(|| RpcError::internal("lost pane"))?;
    let tab = mux.window(window).map_or(0, |w| w.tabs[index].id);
    Ok(SplitDone {
        result: mt_proto::PaneSplitResult {
            pane: new,
            tab,
            window,
        },
        parent: pane,
        axis: dir.axis(),
        permille: u16::from(percent) * 10,
    })
}

/// `pane.focus|zoom|resize|swap|rotate`: pure model operations shared by both hosts.
pub fn layout_op(mux: &mut Mux, rows: &Rows, req: &Request) -> Result<Value, RpcError> {
    let acting = Verb::Acting { all_allowed: false };
    match req {
        Request::PaneFocus(p) => {
            let pane = resolve_panes(rows, &p.target, acting)?[0];
            let target = match p.direction {
                None => {
                    mux.focus_pane(pane)?;
                    pane
                }
                Some(d) => mux.focus_direction(pane, d.into())?.ok_or_else(|| {
                    RpcError::not_found(format!("no pane {} of pane {pane}", dir_word(d)))
                })?,
            };
            Ok(serde_json::json!({ "pane": target }))
        }
        Request::PaneZoom(t) => {
            let pane = resolve_panes(rows, t, acting)?[0];
            let zoomed = mux.toggle_zoom(pane)?;
            Ok(serde_json::json!({ "pane": pane, "zoomed": zoomed }))
        }
        Request::PaneResize(p) => {
            let pane = resolve_panes(rows, &p.target, acting)?[0];
            if p.cells == 0 {
                return Err(RpcError::bad_request("--cells must be at least 1"));
            }
            let size = rows
                .panes
                .iter()
                .find(|r| r.id == pane)
                .map_or((0, 0), |r| r.size);
            // No live size (a pane that has not painted yet): assume the default 120x34 grid.
            let size = if size == (0, 0) { (120, 34) } else { size };
            if !mux.resize_pane_cells(pane, p.direction.into(), p.cells, size)? {
                return Err(RpcError::bad_request(format!(
                    "pane {pane} has no border to move {}",
                    dir_word(p.direction)
                )));
            }
            Ok(serde_json::json!({ "pane": pane }))
        }
        Request::PaneSwap(p) => {
            let pane = resolve_panes(rows, &p.target, acting)?[0];
            if !mux.swap_pane(pane, p.direction.into())? {
                return Err(RpcError::not_found(format!(
                    "no pane {} of pane {pane}",
                    dir_word(p.direction)
                )));
            }
            Ok(serde_json::json!({ "pane": pane }))
        }
        Request::PaneRotate(t) => {
            let pane = resolve_panes(rows, t, acting)?[0];
            mux.rotate_panes(pane)?;
            Ok(serde_json::json!({ "pane": pane }))
        }
        other => Err(RpcError::internal(format!(
            "{} is not a layout request",
            other.method()
        ))),
    }
}

fn dir_word(d: mt_proto::Direction) -> &'static str {
    match d {
        mt_proto::Direction::Left => "to the left",
        mt_proto::Direction::Right => "to the right",
        mt_proto::Direction::Up => "above",
        mt_proto::Direction::Down => "below",
    }
}

/// One notification that got through the rate limit, for the host to surface (badge, toast, OS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub pane: Id,
    pub level: mt_proto::NotifyLevel,
    pub title: Option<String>,
    pub message: String,
    pub desktop: bool,
}

/// `pane.status_set`: sanitises, applies and returns the affected panes.
pub fn status_set(
    mux: &mut Mux,
    rows: &Rows,
    p: &mt_proto::StatusSetParams,
    max_len: usize,
    now_ms: u64,
) -> Result<Value, RpcError> {
    if p.progress.is_some_and(|n| n > 100) {
        return Err(RpcError::bad_request(
            "--progress must be between 0 and 100",
        ));
    }
    let panes = resolve_panes(rows, &p.target, Verb::Acting { all_allowed: true })?;
    for &pane in &panes {
        mux.set_pane_status(
            pane,
            p.state.unwrap_or_default(),
            p.text.as_deref().unwrap_or(""),
            p.progress,
            p.ttl_ms,
            max_len,
            now_ms,
        );
    }
    to_json(&mt_proto::ActResult { panes, dropped: 0 })
}

/// `pane.status_clear`.
pub fn status_clear(mux: &mut Mux, rows: &Rows, t: &Target) -> Result<Value, RpcError> {
    let panes = resolve_panes(rows, t, Verb::Acting { all_allowed: true })?;
    for &pane in &panes {
        mux.clear_pane_status(pane);
    }
    to_json(&mt_proto::ActResult { panes, dropped: 0 })
}

/// `pane.status_get`: reads from the live mux, so a status that just expired is gone.
pub fn status_get(mux: &Mux, rows: &Rows, t: &Target) -> Result<Value, RpcError> {
    let pane = ctl::resolve_one(t, &rows.panes, rows.focused_pane, Verb::ReadOnly)?;
    to_json(&mt_proto::StatusResult {
        pane,
        status: mux
            .pane(pane)
            .and_then(|p| p.status.as_ref())
            .map(|s| s.to_wire()),
    })
}

/// `pane.notify`: runs each addressed pane through the rate limit, emits `Notification` events
/// and returns what the host should surface. The result counts rate-limited drops.
pub fn notify(
    mux: &mut Mux,
    rows: &Rows,
    p: &mt_proto::NotifyParams,
    max_len: usize,
    now_ms: u64,
) -> Result<(Value, Vec<Delivered>), RpcError> {
    let panes = resolve_panes(rows, &p.target, Verb::Acting { all_allowed: true })?;
    let message = mt_mux::sanitize(&p.message, max_len);
    let title = p
        .title
        .as_deref()
        .map(|t| mt_mux::sanitize(t, max_len))
        .filter(|t| !t.is_empty());
    let (mut delivered, mut dropped) = (Vec::new(), 0);
    for &pane in &panes {
        if mux.notify_pane(
            pane,
            p.level,
            title.clone(),
            message.clone(),
            mt_mux::NotifySource::Ctl,
            now_ms,
        ) {
            delivered.push(Delivered {
                pane,
                level: p.level,
                title: title.clone(),
                message: message.clone(),
                desktop: p.desktop,
            });
        } else {
            dropped += 1;
        }
    }
    let result = to_json(&mt_proto::ActResult { panes, dropped })?;
    Ok((result, delivered))
}

/// What a scripted close will remove (SPEC §12.3).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CloseSet {
    /// Panes to close; closing the last pane closes its tab, the last tab its window.
    pub panes: Vec<Id>,
    /// Windows that must be gone afterwards even when they hold no panes.
    pub windows: Vec<Id>,
}

/// Resolves a `*.close` request and applies `confirm_close`: when the policy would prompt and
/// `--force` is absent, nothing is closed and the error is [`ErrorCode::ConfirmRequired`]
/// (exit 8). Never opens a dialog.
pub fn plan_close(rows: &Rows, req: &Request, policy: ConfirmClose) -> Result<CloseSet, RpcError> {
    let acting = Verb::Acting { all_allowed: true };
    let (set, what, force) = match req {
        Request::PaneClose(t) => {
            let panes = resolve_panes(rows, t, acting)?;
            let what = describe("pane", &panes);
            (
                CloseSet {
                    panes,
                    windows: vec![],
                },
                what,
                t.force,
            )
        }
        Request::TabClose(t) => {
            let tabs = ctl::resolve(t, &rows.tabs, rows.focused_tab, acting)?;
            let panes = rows
                .tabs
                .iter()
                .filter(|tab| tabs.contains(&tab.id))
                .flat_map(|tab| tab.panes.clone())
                .collect();
            let what = describe("tab", &tabs);
            (
                CloseSet {
                    panes,
                    windows: vec![],
                },
                what,
                t.force,
            )
        }
        Request::WindowClose(t) => {
            let windows = ctl::resolve(t, &rows.windows, rows.focused_window, acting)?;
            let panes = rows
                .panes
                .iter()
                .filter(|p| windows.contains(&p.window))
                .map(|p| p.id)
                .collect();
            let what = describe("window", &windows);
            (CloseSet { panes, windows }, what, t.force)
        }
        other => {
            return Err(RpcError::internal(format!(
                "{} is not a close request",
                other.method()
            )))
        }
    };
    if !force && ctl::close_would_prompt(policy, busy_flags(rows, &set.panes)) {
        return Err(ctl::confirm_required(&what));
    }
    Ok(set)
}

fn describe(kind: &str, ids: &[Id]) -> String {
    let ids: Vec<String> = ids.iter().map(Id::to_string).collect();
    format!("{kind} {}", ids.join(", "))
}

fn busy_flags(rows: &Rows, panes: &[Id]) -> Vec<bool> {
    let mut flags: Vec<bool> = rows
        .panes
        .iter()
        .filter(|p| panes.contains(&p.id))
        .map(|p| p.process.is_some())
        .collect();
    // A close that holds no panes still "prompts" under `always`.
    if flags.is_empty() {
        flags.push(false);
    }
    flags
}

/// `workspace.delete`: validates (the `default` and the active workspace stay), applies
/// `confirm_close` and returns the panes to stop before `Mux::delete_workspace`.
pub fn plan_workspace_delete(
    rows: &Rows,
    name: &str,
    force: bool,
    policy: ConfirmClose,
) -> Result<Vec<Id>, RpcError> {
    let ws = rows
        .workspace_by_name(name)
        .ok_or_else(|| RpcError::not_found(format!("no workspace named \"{name}\"")))?;
    if name == mt_mux::DEFAULT_WORKSPACE {
        return Err(RpcError::bad_request(format!(
            "the '{name}' workspace cannot be deleted"
        )));
    }
    if ws.active {
        return Err(RpcError::bad_request(format!(
            "'{name}' is the active workspace; switch to another one before deleting it"
        )));
    }
    let panes: Vec<Id> = rows
        .panes
        .iter()
        .filter(|p| p.workspace == name)
        .map(|p| p.id)
        .collect();
    if !force && ctl::close_would_prompt(policy, busy_flags(rows, &panes)) {
        return Err(ctl::confirm_required(&format!("workspace \"{name}\"")));
    }
    Ok(panes)
}

/// `(window, tab index)` a `tab.focus` refers to: a target tab, or `index` / `relative` inside
/// `window` (default: the focused window).
pub fn resolve_focus(mux: &Mux, rows: &Rows, p: &TabFocusParams) -> Result<(Id, usize), RpcError> {
    if p.index.is_none() && p.relative.is_none() {
        let tab = ctl::resolve_one(
            &p.target,
            &rows.tabs,
            None,
            Verb::Acting { all_allowed: false },
        )?;
        return mux
            .tab_location(tab)
            .ok_or_else(|| RpcError::not_found(format!("tab {tab} does not exist")));
    }
    let w = match p.target.window {
        Some(w) => w,
        None => rows
            .focused_window
            .ok_or_else(|| RpcError::not_found("no window"))?,
    };
    let win = mux
        .window(w)
        .ok_or_else(|| RpcError::not_found(format!("window {w} does not exist")))?;
    match (p.index, p.relative) {
        (Some(i), _) if i < win.tabs.len() => Ok((w, i)),
        (Some(i), _) => Err(RpcError::not_found(format!("window {w} has no tab {i}"))),
        (None, Some(d)) => {
            let n = win.tabs.len() as i64;
            if n == 0 {
                return Err(RpcError::not_found(format!("window {w} has no tabs")));
            }
            Ok((w, (win.active as i64 + d).rem_euclid(n) as usize))
        }
        (None, None) => unreachable!("handled above"),
    }
}

/// The tab a `tab.rename` addresses, as `(window, tab)`.
pub fn resolve_rename(mux: &Mux, rows: &Rows, t: &Target) -> Result<(Id, Id), RpcError> {
    let tab = ctl::resolve_one(t, &rows.tabs, None, Verb::Acting { all_allowed: false })?;
    let (w, _) = mux
        .tab_location(tab)
        .ok_or_else(|| RpcError::not_found(format!("tab {tab} does not exist")))?;
    Ok((w, tab))
}

/// The window a `window.focus` addresses.
pub fn resolve_window(rows: &Rows, t: &Target) -> Result<Id, RpcError> {
    ctl::resolve_one(t, &rows.windows, None, Verb::Acting { all_allowed: false })
}

/// `pane.get_text`: rows `start_line..=end_line`, where line `0` is the top visible row and
/// negative numbers reach into scrollback, or with `last_command` the output of the N-th most
/// recent finished command. Soft-wrapped rows are joined; trailing blank lines are dropped.
/// The result is capped at `cap` bytes, keeping the tail.
pub fn get_text(term: &Terminal, p: &GetTextParams, cap: usize) -> Result<Value, RpcError> {
    if let Some(n) = p.last_command {
        if p.start_line.is_some() || p.end_line.is_some() {
            return Err(RpcError::bad_request(
                "--last-command cannot be combined with --start-line/--end-line",
            ));
        }
        if p.escapes {
            return Err(RpcError::bad_request(
                "--last-command cannot be combined with --escapes",
            ));
        }
        if !term.has_marks() {
            return Err(RpcError::needs_shell_integration(
                "no OSC 133 marks seen in this pane; add `eval \"$(microterm shell-integration \
                 <shell>)\"` to the shell's rc file",
            ));
        }
        let n = n.max(1) as usize;
        let info = term.last_command(n).ok_or_else(|| {
            RpcError::not_found(format!("the pane has no finished command number {n}"))
        })?;
        let (text, cut) = mt_mux::wait::cap_tail(info.output, cap);
        return to_json(&CommandText {
            command: info.command,
            exit_code: info.exit_code,
            cwd: info.cwd,
            text,
            finished_at: mt_proto::iso8601_utc(info.finished_ms),
            truncated: info.truncated || cut,
        });
    }
    let history = term.history_size() as i64;
    let (_, rows) = term.size();
    let start = p.start_line.unwrap_or(0);
    let end = p.end_line.unwrap_or(rows as i64 - 1);
    if end < start {
        return Err(RpcError::bad_request("end_line is before start_line"));
    }
    let total = term.total_lines() as i64;
    let from = (history + start).clamp(0, total) as usize;
    let to = (history + end + 1).clamp(0, total) as usize;
    let mut out = String::new();
    if p.escapes {
        let lines: Vec<String> = term
            .styled_lines(from, to)
            .iter()
            .map(|r| mt_term::encode_row(r))
            .collect();
        out = lines.join("\n");
        out.truncate(out.trim_end_matches('\n').len());
    } else {
        for row in term.lines_range(from, to) {
            if row.wrapped {
                out.extend(row.chars.iter().filter(|c| **c != '\0'));
            } else {
                out.push_str(&row.text());
                out.push('\n');
            }
        }
        out.truncate(out.trim_end_matches('\n').len());
    }
    if !out.is_empty() {
        out.push('\n');
    }
    let (text, truncated) = mt_mux::wait::cap_tail(out, cap);
    to_json(&Text { text, truncated })
}

/// `pane.send_text`: pasted (bracketed when the program asked for it) unless `paste` is off,
/// in which case the bytes are typed as they are.
pub fn send_text(term: &Terminal, p: &SendTextParams) {
    if p.paste {
        term.paste(&p.text);
    } else {
        term.write(p.text.clone().into_bytes());
    }
}

struct Recording {
    done: Option<JoinHandle<()>>,
}

/// Active `record.start` sessions, keyed by pane id.
#[derive(Default)]
pub struct Recorders {
    active: HashMap<Id, Recording>,
}

impl Recorders {
    pub fn start(
        &mut self,
        pane: Id,
        term: &Arc<Terminal>,
        path: &str,
        input: bool,
    ) -> Result<(), RpcError> {
        self.active
            .retain(|_, r| r.done.as_ref().is_some_and(|h| !h.is_finished()));
        if self.active.contains_key(&pane) {
            return Err(RpcError::bad_request(format!(
                "pane {pane} is already being recorded"
            )));
        }
        let (cols, rows) = term.size();
        let file = File::create(path)
            .map_err(|e| RpcError::bad_request(format!("cannot create {path}: {e}")))?;
        let mut header = mt_record::Header::new(cols as u16, rows as u16);
        header.timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        for k in ["TERM", "SHELL"] {
            if let Ok(v) = std::env::var(k) {
                header.env.insert(k.into(), v);
            }
        }
        let mut writer = mt_record::Writer::new(BufWriter::new(file), &header)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let (otx, orx) = mpsc::channel::<Vec<u8>>();
        let (itx, irx) = mpsc::channel::<Vec<u8>>();
        let t0 = Instant::now();
        // One thread drains both streams in arrival order.
        let (merged_tx, merged_rx) = mpsc::channel::<(bool, Vec<u8>)>();
        for (rx, is_input) in [(orx, false), (irx, true)] {
            let tx = merged_tx.clone();
            std::thread::spawn(move || {
                while let Ok(b) = rx.recv() {
                    if tx.send((is_input, b)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(merged_tx);
        let handle = std::thread::Builder::new()
            .name("microterm-record".into())
            .spawn(move || {
                let mut out_join = mt_record::Utf8Joiner::default();
                let mut in_join = mt_record::Utf8Joiner::default();
                while let Ok((is_input, bytes)) = merged_rx.recv() {
                    let t = t0.elapsed().as_secs_f64();
                    let (join, write): (_, fn(&mut _, f64, &[u8]) -> _) = if is_input {
                        (
                            &mut in_join,
                            |w: &mut mt_record::Writer<BufWriter<File>>, t, b| w.input(t, b),
                        )
                    } else {
                        (&mut out_join, |w, t, b| w.output(t, b))
                    };
                    let chunk = join.push(&bytes);
                    if !chunk.is_empty() {
                        let _ = write(&mut writer, t, &chunk);
                    }
                }
                let t = t0.elapsed().as_secs_f64();
                let rest = out_join.finish();
                if !rest.is_empty() {
                    let _ = writer.output(t, &rest);
                }
                let _ = writer.flush();
            })
            .map_err(|e| RpcError::internal(e.to_string()))?;
        term.set_output_tap(Some(otx));
        term.set_input_tap(input.then_some(itx));
        self.active.insert(pane, Recording { done: Some(handle) });
        Ok(())
    }

    /// Ends the recording and waits until the file is complete. Succeeds silently when `term`
    /// is gone (the pane exited and the recording finished by itself).
    pub fn stop(&mut self, pane: Id, term: Option<&Terminal>) -> Result<(), RpcError> {
        let Some(mut rec) = self.active.remove(&pane) else {
            return Err(RpcError::not_found(format!(
                "pane {pane} is not being recorded"
            )));
        };
        if let Some(t) = term {
            t.set_output_tap(None);
            t.set_input_tap(None);
        }
        if let Some(h) = rec.done.take() {
            let _ = h.join();
        }
        Ok(())
    }

    /// Like [`Recorders::stop`], for a pane that is closing; ignores panes not recorded.
    pub fn finish(&mut self, pane: Id, term: Option<&Terminal>) {
        let _ = self.stop(pane, term);
    }
}

/// OSC 9;4 progress as pane status (SPEC §12.4): normal and indeterminate show as working, an
/// error as error, paused as waiting; removal clears only a status that came from OSC. A live
/// `ctl pane status` is never overwritten. Returns whether the pane's status changed.
pub fn osc_progress(
    mux: &mut mt_mux::Mux,
    pane: u64,
    state: mt_term::ProgressState,
    value: Option<u8>,
    now_ms: u64,
) -> bool {
    use mt_proto::StatusState;
    use mt_term::ProgressState as P;
    let (state, value) = match state {
        P::Remove => return mux.clear_osc_progress(pane),
        P::Normal => (StatusState::Working, value),
        P::Indeterminate => (StatusState::Working, None),
        P::Error => (StatusState::Error, value),
        P::Paused => (StatusState::Waiting, value),
    };
    mux.set_osc_progress(pane, state, value, now_ms)
}

/// OSC 9 text as a notification: sanitised, truncated and rate-limited like `pane notify`, level
/// `info`, source `osc9`. `None` when it was empty or dropped by the limit.
pub fn osc_notify(
    mux: &mut mt_mux::Mux,
    pane: u64,
    text: &str,
    max_len: usize,
    now_ms: u64,
) -> Option<Delivered> {
    let message = mt_mux::sanitize(text, max_len);
    if message.is_empty() {
        return None;
    }
    let level = mt_proto::NotifyLevel::Info;
    mux.notify_pane(
        pane,
        level,
        None,
        message.clone(),
        mt_mux::NotifySource::Osc9,
        now_ms,
    )
    .then_some(Delivered {
        pane,
        level,
        title: None,
        message,
        desktop: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_of(mux: &Mux) -> Rows {
        mux.rows(&|_| Live::default())
    }

    #[test]
    fn mux_list_is_the_tree_with_titles() {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let (tab, pane) = mux.spawn_tab(w, None, Some("srv".into())).unwrap();
        mux.set_pane_title(pane, "zsh");
        let s = rows_of(&mux).tree();
        let ws = &s.workspaces[0];
        assert!(ws.active);
        assert_eq!(ws.windows[0].tabs[0].id, tab);
        assert_eq!(ws.windows[0].tabs[0].title, "srv");
        assert_eq!(ws.windows[0].tabs[0].panes[0].id, pane);
        assert_eq!(ws.windows[0].tabs[0].panes[0].title, "zsh");
        assert!(ws.windows[0].tabs[0].panes[0].focused);
    }

    fn mux_with(n: usize) -> (Mux, Id, Vec<Id>) {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let tabs = (0..n)
            .map(|_| mux.spawn_tab(w, None, None).unwrap().0)
            .collect();
        (mux, w, tabs)
    }

    #[test]
    fn focus_resolves_ids_indexes_and_relative() {
        let (mux, w, tabs) = mux_with(3);
        let rows = rows_of(&mux);
        let by = |p: TabFocusParams| resolve_focus(&mux, &rows, &p);
        let tab = |t: Target| TabFocusParams {
            target: t,
            ..Default::default()
        };
        assert_eq!(by(tab(Target::tab(tabs[1]))).unwrap(), (w, 1));
        let idx = |i| TabFocusParams {
            index: Some(i),
            ..Default::default()
        };
        assert_eq!(by(idx(2)).unwrap(), (w, 2));
        assert!(by(idx(9)).is_err());
        // Spawning left tab 2 active: -1 goes to 1, +1 wraps to 0.
        let rel = |d| TabFocusParams {
            relative: Some(d),
            ..Default::default()
        };
        assert_eq!(by(rel(-1)).unwrap(), (w, 1));
        assert_eq!(by(rel(1)).unwrap(), (w, 0));
        assert!(
            by(TabFocusParams::default()).is_err(),
            "no target, no focus fallback"
        );
    }

    fn policy_rows(busy: bool) -> (Rows, Vec<Id>) {
        let (mux, _, _) = mux_with(2);
        let panes: Vec<Id> = rows_of(&mux).panes.iter().map(|p| p.id).collect();
        let rows = mux.rows(&|id| Live {
            process: (busy && id == panes[0]).then(|| "vim".into()),
            ..Default::default()
        });
        (rows, panes)
    }

    #[test]
    fn close_matrix_policy_by_force() {
        use ConfirmClose::*;
        for (policy, busy, prompts) in [
            (Always, false, true),
            (Always, true, true),
            (Process, false, false),
            (Process, true, true),
            (Never, false, false),
            (Never, true, false),
        ] {
            let (rows, panes) = policy_rows(busy);
            let tab = rows.panes[0].tab;
            for req in [
                Request::PaneClose(Target::pane(panes[0])),
                Request::TabClose(Target::tab(tab)),
                Request::WindowClose(Target::window(rows.panes[0].window)),
            ] {
                let got = plan_close(&rows, &req, policy);
                if prompts {
                    let e = got.unwrap_err();
                    assert_eq!(e.code, mt_proto::ErrorCode::ConfirmRequired, "{req:?}");
                } else {
                    assert!(got.is_ok(), "{policy:?} busy={busy} {req:?}");
                }
                // --force closes immediately, whatever the policy says.
                let forced = match req {
                    Request::PaneClose(t) => Request::PaneClose(Target { force: true, ..t }),
                    Request::TabClose(t) => Request::TabClose(Target { force: true, ..t }),
                    Request::WindowClose(t) => Request::WindowClose(Target { force: true, ..t }),
                    _ => unreachable!(),
                };
                assert!(plan_close(&rows, &forced, policy).is_ok());
            }
        }
    }

    #[test]
    fn closes_never_fall_back_to_the_focused_item() {
        let (rows, _) = policy_rows(false);
        let e = plan_close(
            &rows,
            &Request::TabClose(Target::default()),
            ConfirmClose::Never,
        )
        .unwrap_err();
        assert_eq!(e.code, mt_proto::ErrorCode::BadRequest);
    }

    #[test]
    fn a_window_close_covers_all_its_panes() {
        let (rows, panes) = policy_rows(false);
        let w = rows.panes[0].window;
        let set = plan_close(
            &rows,
            &Request::WindowClose(Target::window(w)),
            ConfirmClose::Never,
        )
        .unwrap();
        assert_eq!(set.panes, panes);
        assert_eq!(set.windows, [w]);
    }

    #[test]
    fn workspace_delete_rules() {
        let mut mux = Mux::new();
        mux.create_workspace("api").unwrap();
        mux.create_workspace("infra").unwrap();
        mux.switch_workspace("api").unwrap();
        let rows = rows_of(&mux);
        let plan = |name| plan_workspace_delete(&rows, name, false, ConfirmClose::Never);
        assert!(plan("default").is_err(), "default is protected");
        assert!(plan("api").is_err(), "the active workspace stays");
        assert_eq!(
            plan("nope").unwrap_err().code,
            mt_proto::ErrorCode::NotFound
        );
        assert!(plan("infra").unwrap().is_empty());
        let e = plan_workspace_delete(&rows, "infra", false, ConfirmClose::Always).unwrap_err();
        assert_eq!(e.code, mt_proto::ErrorCode::ConfirmRequired);
        assert!(plan_workspace_delete(&rows, "infra", true, ConfirmClose::Always).is_ok());
    }

    #[test]
    fn pane_env_names_the_pane_tab_window_and_socket() {
        let env = pane_env(12, 4, 2, Some(Path::new("/run/m.sock")));
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("MICROTERM_PANE"), Some("12"));
        assert_eq!(get("MICROTERM_TAB"), Some("4"));
        assert_eq!(get("MICROTERM_WINDOW"), Some("2"));
        assert_eq!(get("MICROTERM_UNIX_SOCKET"), Some("/run/m.sock"));
        assert!(pane_env(1, 1, 1, None)
            .iter()
            .all(|(k, _)| k != "MICROTERM_UNIX_SOCKET"));
    }

    #[test]
    fn spawn_spec_is_local_until_the_caller_resolves_the_domain() {
        let p = SpawnParams {
            domain: Some("prod".into()),
            cwd: Some("~/x".into()),
            ..Default::default()
        };
        let spec = spawn_spec(&p).unwrap();
        assert_eq!(spec.domain, mt_mux::LOCAL_DOMAIN_ID);
        assert!(spec.cwd.is_some());
    }
}
