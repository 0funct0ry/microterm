//! The GPUI side of triggers (SPEC §10.11): a global that owns the engine, the worker thread
//! and the line taps, follows the gate and `triggers.toml`, and routes worker hits and pane
//! events to the engine. With `triggers.enabled = false` it is inert: no tap, no worker thread,
//! no file read, and the mux listener forwards nothing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use gpui::{App, BorrowAppContext, Global};
use mt_config::edit::content_hash;
use mt_mux::ctl::{Live, PaneRow};
use mt_mux::MuxEvent;
use mt_term::{LineQueue, LineTapCfg};
use mt_triggers::{EventKind, LoadOpts};

use super::engine::{spawn_worker, Engine, EventInput, WorkerHit};
use super::gate::TriggerGate;
use crate::components::ToastKind;
use crate::mux_host;
use crate::settings::{Banner, Settings};

pub const FILE_NAME: &str = "triggers.toml";
/// Capacity of the queue between the PTY threads and the worker (SPEC §10.11.3).
pub const QUEUE_CAP: usize = 4096;
const HIT_CHANNEL_CAP: usize = 1024;
const TICK_MS: u64 = 100;

pub struct TriggerRuntime {
    pub(crate) engine: Engine,
    /// Shown under the config banner; names `triggers.toml`.
    pub banner: Option<Banner>,
    loaded_hash: Option<u64>,
    active: bool,
    queue: Arc<LineQueue>,
    stop: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
    /// Mirrors the gate for the mux listener, which cannot read GPUI globals.
    flag: Arc<AtomicBool>,
    tapped: HashSet<u64>,
    hit_tx: async_channel::Sender<WorkerHit>,
    /// What rules have set on tabs and panes (colors, titles, counters, recordings).
    pub(crate) fx: super::state::Effects,
    /// Highlights, markers and flashes per pane.
    pub(crate) deco: super::deco::Decorations,
    pub(crate) findings: super::findings::Findings,
    /// The last file each rule's capture actions wrote (the panel's **Reveal**).
    /// `line_base` of each decorated pane as of the last tick (the Findings "scrolled out").
    pub(crate) line_bases: std::collections::HashMap<u64, u64>,
    pub(crate) captures_last: std::collections::HashMap<String, std::path::PathBuf>,
}

impl Global for TriggerRuntime {}

/// `<config_dir>/triggers.toml` next to the active config file.
pub fn path(cx: &App) -> std::path::PathBuf {
    mt_config::load::triggers_path(&cx.global::<Settings>().path)
}

pub fn banner(cx: &App) -> Option<Banner> {
    cx.try_global::<TriggerRuntime>()
        .and_then(|r| r.banner.clone())
}

pub fn active(cx: &App) -> bool {
    cx.try_global::<TriggerRuntime>().is_some_and(|r| r.active)
}

/// Lines the PTY side dropped because the queue was full, plus hits the worker could not hand
/// over (the panel footer and `trigger list`).
pub fn dropped_lines(cx: &App) -> u64 {
    cx.try_global::<TriggerRuntime>().map_or(0, |r| {
        r.queue.dropped() + r.engine.overflow.load(Ordering::Relaxed)
    })
}

/// Creates the (inert) runtime, the mux listener and the tasks that deliver worker hits and
/// events to the main thread. Called once, when the mux host is created.
pub(crate) fn install(cx: &mut App) {
    if cx.has_global::<TriggerRuntime>() {
        return;
    }
    let (hit_tx, hit_rx) = async_channel::bounded::<WorkerHit>(HIT_CHANNEL_CAP);
    let (ev_tx, ev_rx) = async_channel::unbounded::<MuxEvent>();
    let flag = Arc::new(AtomicBool::new(false));
    cx.set_global(TriggerRuntime {
        engine: Engine::default(),
        banner: None,
        loaded_hash: None,
        active: false,
        queue: LineQueue::new(QUEUE_CAP),
        stop: None,
        worker: None,
        flag: flag.clone(),
        tapped: HashSet::new(),
        hit_tx,
        fx: Default::default(),
        deco: Default::default(),
        findings: Default::default(),
        captures_last: Default::default(),
        line_bases: Default::default(),
    });
    mux_host::host(cx).mux.subscribe(move |e| {
        if flag.load(Ordering::Relaxed)
            && matches!(
                e,
                MuxEvent::Bell { .. }
                    | MuxEvent::CommandFinished { .. }
                    | MuxEvent::CwdChanged { .. }
                    | MuxEvent::TitleChanged { .. }
                    | MuxEvent::PaneExited { .. }
            )
        {
            let _ = ev_tx.try_send(e.clone());
        }
    });
    cx.spawn(async move |cx| {
        while let Ok(hit) = hit_rx.recv().await {
            if cx.update(|cx| handle_hit(cx, hit)).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(ev) = ev_rx.recv().await {
            if cx.update(|cx| handle_event(cx, ev)).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(TICK_MS))
            .await;
        if cx.update(tick).is_err() {
            break;
        }
    })
    .detach();
}

fn tap_cfg(cx: &App) -> LineTapCfg {
    let t = &cx.global::<Settings>().config.triggers;
    LineTapCfg {
        max_line_chars: t.max_line_chars as usize,
        scan_alt_screen: t.scan_alt_screen,
        partial_after_ms: t.scan_partial_after_ms,
    }
}

fn start_worker(rt: &mut TriggerRuntime) {
    if rt.worker.is_some() {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    rt.worker = spawn_worker(
        rt.queue.clone(),
        rt.engine.snapshot(),
        rt.hit_tx.clone(),
        rt.engine.overflow.clone(),
        stop.clone(),
    )
    .ok();
    rt.stop = Some(stop);
}

fn stop_worker(rt: &mut TriggerRuntime) {
    if let Some(stop) = rt.stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    rt.queue.wake();
    if let Some(h) = rt.worker.take() {
        let _ = h.join();
    }
    // Lines queued for a worker that is gone must not reach the next one.
    rt.queue.recv_batch(usize::MAX, Duration::ZERO);
}

/// Brings the runtime in line with the gate and `triggers.toml`. Returns whether anything
/// visible changed. Call after a config reload, from the file watcher and after our own writes;
/// reloading identical text is a no-op (which also covers the panel's own write-back).
pub fn sync(cx: &mut App) -> bool {
    install(cx);
    let gate = TriggerGate::of(cx);
    if !gate.enabled {
        return teardown(cx);
    }
    let path = path(cx);
    let max_rules = cx.global::<Settings>().config.triggers.max_rules as usize;
    let mut changed = false;
    cx.update_global::<TriggerRuntime, _>(|rt, _| {
        if !rt.active {
            rt.active = true;
            rt.flag.store(true, Ordering::Relaxed);
            start_worker(rt);
            changed = true;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                rt.banner = Some(Banner {
                    kind: ToastKind::Error,
                    text: format!(
                        "{FILE_NAME} — cannot read the file: {e}. Using the last valid rules."
                    ),
                });
                return;
            }
        };
        let hash = content_hash(&text);
        if rt.loaded_hash == Some(hash) {
            return;
        }
        rt.loaded_hash = Some(hash);
        changed = true;
        match mt_triggers::load_str(&text, LoadOpts { max_rules }) {
            Ok(loaded) => {
                rt.engine.load(loaded.rules);
                rt.banner = None;
            }
            Err(diags) => {
                let mut first = diags[0].render(FILE_NAME);
                if diags.len() > 1 {
                    first.push_str(&format!(" (+{} more)", diags.len() - 1));
                }
                rt.banner = Some(Banner {
                    kind: ToastKind::Error,
                    text: format!("{first}. Using the last valid rules."),
                });
            }
        }
    });
    sync_taps(cx, false);
    changed
}

/// Hot-reload entry for `triggers.*` config changes (a gate flipped, a scan setting changed):
/// installed taps are redone with the new settings.
pub fn config_changed(cx: &mut App) {
    if !TriggerGate::of(cx).enabled {
        teardown(cx);
        return;
    }
    sync(cx);
    sync_taps(cx, true);
}

/// Removes everything triggers installed: taps, worker, rules, banner.
fn teardown(cx: &mut App) -> bool {
    let was_active = cx.try_global::<TriggerRuntime>().is_some_and(|r| r.active);
    if !was_active {
        return false;
    }
    // Everything a rule set on a tab or pane goes first, while the effects are still known.
    super::apply::cleanup(cx);
    let tapped: Vec<u64> = cx
        .global::<TriggerRuntime>()
        .tapped
        .iter()
        .copied()
        .collect();
    let terms = mux_host::pane_terminals(cx);
    cx.update_global::<TriggerRuntime, _>(|rt, _| {
        for pane in tapped {
            if let Some(t) = terms.get(&pane) {
                t.set_line_tap(None, rt.queue.clone(), pane);
            }
        }
        rt.tapped.clear();
        rt.flag.store(false, Ordering::Relaxed);
        stop_worker(rt);
        rt.engine.clear();
        rt.banner = None;
        rt.loaded_hash = None;
        rt.active = false;
    });
    true
}

/// The panes' facts without live terminal data: enough for the static selector keys.
fn static_rows(cx: &mut App) -> Vec<PaneRow> {
    mux_host::host(cx).mux.rows(&|_| Live::default()).panes
}

/// Installs the line tap on panes some enabled text rule could match and removes it from the
/// rest. `force` redoes taps that already exist (settings changed).
pub(crate) fn sync_taps(cx: &mut App, force: bool) {
    if !active(cx) {
        return;
    }
    let cfg = tap_cfg(cx);
    let terms = mux_host::pane_terminals(cx);
    let rows = static_rows(cx);
    cx.update_global::<TriggerRuntime, _>(|rt, _| {
        for row in &rows {
            let Some(term) = terms.get(&row.id) else {
                continue;
            };
            let want = rt.engine.wants_pane(row);
            let has = rt.tapped.contains(&row.id);
            if want && (!has || force) {
                term.set_line_tap(Some(cfg), rt.queue.clone(), row.id);
                rt.tapped.insert(row.id);
            } else if !want && has {
                term.set_line_tap(None, rt.queue.clone(), row.id);
                rt.tapped.remove(&row.id);
            }
        }
        rt.tapped.retain(|p| terms.contains_key(p));
    });
}

/// Pane facts for selectors and templates: the mux model plus the pane's live terminal data.
pub(crate) fn pane_row(cx: &mut App, pane: u64) -> Option<PaneRow> {
    let live = {
        let term = mux_host::terminal(cx, pane);
        crate::ipc_ops::live_with_exit(term.as_deref(), mux_host::host(cx).runtime.exit_of(pane))
    };
    mux_host::host(cx)
        .mux
        .rows(&|id| {
            if id == pane {
                live.clone()
            } else {
                Live::default()
            }
        })
        .panes
        .into_iter()
        .find(|p| p.id == pane)
}

fn handle_hit(cx: &mut App, hit: WorkerHit) {
    let gate = TriggerGate::of(cx);
    if !gate.enabled {
        return;
    }
    let now = mt_term::now_ms();
    let ok = cx.update_global::<TriggerRuntime, _>(|rt, _| rt.engine.precheck(&hit, now));
    if !ok {
        return;
    }
    // The selector sees the pane's *current* facts, not the ones the tap was installed with.
    let Some(row) = pane_row(cx, hit.pane) else {
        return;
    };
    let firing =
        cx.update_global::<TriggerRuntime, _>(|rt, _| rt.engine.accept_text(&hit, &row, gate, now));
    if let Some(f) = firing {
        super::apply::run(cx, f);
    }
}

fn event_input(ev: &MuxEvent) -> Option<EventInput> {
    Some(match *ev {
        MuxEvent::Bell { pane } => EventInput {
            kind: EventKind::Bell,
            pane,
            exit: None,
        },
        MuxEvent::CwdChanged { pane } => EventInput {
            kind: EventKind::CwdChanged,
            pane,
            exit: None,
        },
        MuxEvent::TitleChanged { pane } => EventInput {
            kind: EventKind::TitleChanged,
            pane,
            exit: None,
        },
        MuxEvent::CommandFinished { pane, exit_code } => EventInput {
            kind: EventKind::CommandFinished,
            pane,
            exit: exit_code.map(i64::from),
        },
        MuxEvent::PaneExited { pane, status } => EventInput {
            kind: EventKind::PaneExited,
            pane,
            exit: status.map(i64::from),
        },
        _ => return None,
    })
}

fn handle_event(cx: &mut App, ev: MuxEvent) {
    let gate = TriggerGate::of(cx);
    if !gate.enabled {
        return;
    }
    let Some(input) = event_input(&ev) else {
        return;
    };
    let pane = input.pane;
    if let Some(row) = pane_row(cx, pane) {
        let now = mt_term::now_ms();
        let firings = cx.update_global::<TriggerRuntime, _>(|rt, _| {
            rt.engine.accept_event(&input, &row, gate, now)
        });
        for f in firings {
            super::apply::run(cx, f);
        }
    }
    if input.kind == EventKind::PaneExited {
        cx.update_global::<TriggerRuntime, _>(|rt, _| {
            rt.engine.forget_pane(pane);
            rt.tapped.remove(&pane);
        });
        super::apply::pane_exited(cx, pane);
    }
}

/// 100 ms housekeeping: new panes get their tap, and timed effects expire.
fn tick(cx: &mut App) {
    if !active(cx) {
        return;
    }
    sync_taps(cx, false);
    super::apply::tick(cx);
}

/// The last [`preview::LAST_LINES`](super::preview::LAST_LINES) logical lines of `term` with
/// their stable ids (the editor's preview, **Test** and `trigger test`).
pub fn recent_lines(term: &mt_term::Terminal, scope: mt_triggers::MatchIn) -> Vec<(u64, String)> {
    use super::preview::{logical_lines, LAST_LINES};
    let total = term.total_lines();
    let first = total.saturating_sub(LAST_LINES * 2);
    let rows = term.lines_range(first, total);
    let mut lines = logical_lines(&rows, first, term.line_base());
    if scope == mt_triggers::MatchIn::Output {
        // A rule with `scope = "output"` never sees the prompt or the command being typed.
        let regions = term.input_line_regions();
        lines.retain(|(id, _)| !mt_term::in_regions(&regions, *id));
    }
    let skip = lines.len().saturating_sub(LAST_LINES);
    lines.drain(..skip);
    lines
}

/// `NextMarker` / `PreviousMarker`: scrolls the focused pane to the next or previous marker.
/// Returns whether it moved.
pub fn step_marker(cx: &mut App, forward: bool) -> bool {
    if !active(cx) {
        return false;
    }
    let Some(pane) = mux_host::rows(cx).focused_pane else {
        return false;
    };
    let Some(term) = mux_host::terminal(cx, pane) else {
        return false;
    };
    let (line_base, history, offset, rows) = term.view_info();
    let view = super::deco::View {
        line_base,
        history,
        offset,
        rows,
    };
    let target =
        cx.update_global::<TriggerRuntime, _>(|rt, _| rt.deco.step_marker(pane, view, forward));
    let moved = target.is_some_and(|id| term.scroll_to_line_id(id));
    if moved {
        mux_host::refresh_windows(cx);
    }
    moved
}

/// `ClearMarkers`: removes the focused pane's markers.
pub fn clear_markers(cx: &mut App) {
    if let Some(pane) = mux_host::rows(cx).focused_pane {
        cx.update_global::<TriggerRuntime, _>(|rt, _| rt.deco.clear_markers(pane));
        mux_host::refresh_windows(cx);
    }
}

/// A click on a finding: focuses its pane, scrolls to the line and flashes the whole line or just the match for 1.2 s. A
/// finding whose line has left the scrollback only focuses the pane. Returns whether the line
/// was found.
pub fn jump_to_finding(cx: &mut App, index: usize, whole_line: bool) -> bool {
    let Some(f) = cx
        .try_global::<TriggerRuntime>()
        .and_then(|r| r.findings.get(index).cloned())
    else {
        return false;
    };
    let focus = mt_proto::Request::PaneFocus(mt_proto::PaneFocusParams {
        target: mt_proto::Target::pane(f.pane),
        direction: None,
    });
    let _ = mux_host::handle_call(cx, focus);
    let Some(term) = mux_host::terminal(cx, f.pane) else {
        return false;
    };
    if !term.scroll_to_line_id(f.line_id) {
        return false;
    }
    let now = mt_term::now_ms();
    cx.update_global::<TriggerRuntime, _>(|rt, _| {
        rt.deco
            .flash(f.pane, f.line_id, now, f.span.filter(|_| !whole_line))
    });
    mux_host::refresh_windows(cx);
    true
}

/// The palette's "Marker: …" rows: scrolls `pane` to the row with stable id `line`.
pub fn jump_to_line(cx: &mut App, pane: u64, line: u64) {
    if let Some(term) = mux_host::terminal(cx, pane) {
        if term.scroll_to_line_id(line) {
            mux_host::refresh_windows(cx);
        }
    }
}
