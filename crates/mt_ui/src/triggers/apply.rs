//! Applies the intents of a firing to the window (SPEC §10.11.4). Nothing here writes to a PTY,
//! calls `ctl` or edits configuration, and no action produces trigger input (§10.11.3): `notify`
//! and `status` render in app chrome, `bell` does not publish `Bell`, and title and color changes
//! publish no event a rule listens to.

use gpui::{App, BorrowAppContext};
use mt_proto::{NotifyLevel, StatusState};
use mt_triggers::{Intent, Level, State};

use super::deco::{Highlight, Marker};
use super::engine::Firing;
use super::findings::Finding;
use super::runtime::TriggerRuntime;
use super::state::{ColorFx, RecFx, TitleFx};
use crate::app::AppWindow;
use crate::mux_host;
use crate::recording::gate::RecordingGate;

/// Runs one firing: publishes `TriggerFired` and applies each intent in the order written. An
/// intent that fails or is skipped never stops the others; the reason lands on the rule's row.
pub(crate) fn run(cx: &mut App, f: Firing) {
    let kinds: Vec<String> = f.intents.iter().map(|i| i.kind().to_string()).collect();
    mux_host::host(cx)
        .mux
        .trigger_fired(&f.rule_id, f.pane, kinds);
    for intent in &f.intents {
        if let Err(why) = apply_one(cx, &f, intent) {
            cx.update_global::<TriggerRuntime, _>(|rt, _| {
                rt.engine
                    .note_error(&f.rule_id, format!("{} skipped: {why}", intent.kind()));
            });
        }
    }
    mux_host::refresh_windows(cx);
}

fn tab_of(cx: &mut App, pane: u64) -> Option<u64> {
    let h = mux_host::host(cx);
    let (window, index) = h.mux.locate_pane(pane)?;
    Some(h.mux.window(window)?.tabs.get(index)?.id)
}

/// Runs `f` in the OS window showing `pane` without switching workspaces.
fn in_window(
    cx: &mut App,
    pane: u64,
    f: impl FnOnce(&mut AppWindow, &mut gpui::Context<AppWindow>),
) {
    let window = mux_host::host(cx).mux.locate_pane(pane).map(|(w, _)| w);
    if let Some(handle) = window.and_then(|w| mux_host::open_handle(cx, w)) {
        let _ = handle.update(cx, |view, _, cx| f(view, cx));
    }
}

fn apply_one(cx: &mut App, f: &Firing, intent: &Intent) -> Result<(), String> {
    let pane = f.pane;
    let now = mt_term::now_ms();
    match intent {
        Intent::Notify {
            level,
            title,
            message,
            desktop,
        } => {
            let max = mux_host::status_max_len(cx);
            let level = match level {
                Level::Info => NotifyLevel::Info,
                Level::Attention => NotifyLevel::Attention,
                Level::Error => NotifyLevel::Error,
            };
            let title = Some(mt_mux::sanitize(title, max)).filter(|t| !t.is_empty());
            let message = mt_mux::sanitize(message, max);
            let delivered = mux_host::host(cx).mux.notify_pane(
                pane,
                level,
                title.clone(),
                message.clone(),
                mt_mux::NotifySource::Trigger,
                now,
            );
            if !delivered {
                return Err("notification rate limit".into());
            }
            mux_host::deliver(
                cx,
                crate::ipc_ops::Delivered {
                    pane,
                    level,
                    title,
                    message,
                    desktop: *desktop,
                },
            );
        }
        Intent::Status {
            state,
            progress,
            text,
            ttl_s,
        } => {
            let max = mux_host::status_max_len(cx);
            let state = match state {
                State::Idle => StatusState::Idle,
                State::Working => StatusState::Working,
                State::Waiting => StatusState::Waiting,
                State::Done => StatusState::Done,
                State::Error => StatusState::Error,
            };
            let ttl = (*ttl_s > 0).then(|| u64::from(*ttl_s) * 1000);
            if mux_host::host(cx)
                .mux
                .set_pane_status(pane, state, text, *progress, ttl, max, now)
            {
                cx.update_global::<TriggerRuntime, _>(|rt, _| {
                    rt.fx.statuses.insert(pane, ttl.map(|t| now + t));
                });
            }
        }
        Intent::TabColor {
            color,
            ttl_s,
            clear_on_focus,
        } => {
            let color = mt_mux::TabColor::parse(color).map_err(|e| e.to_string())?;
            let tab = tab_of(cx, pane).ok_or("pane has no tab")?;
            let _ = mux_host::host(cx)
                .mux
                .set_trigger_color(tab, Some((color, f.rule_name.clone())));
            cx.update_global::<TriggerRuntime, _>(|rt, _| {
                rt.fx.colors.insert(
                    tab,
                    ColorFx {
                        pane,
                        expires_ms: (*ttl_s > 0).then(|| now + u64::from(*ttl_s) * 1000),
                        clear_on_focus: *clear_on_focus,
                    },
                );
            });
        }
        Intent::Bell => in_window(cx, pane, |view, cx| view.flash_bell(cx)),
        Intent::Counter { reset_on } => {
            cx.update_global::<TriggerRuntime, _>(|rt, _| {
                rt.fx.bump(&f.rule_id, pane, *reset_on);
            });
        }
        Intent::Title { text, ttl_s } => {
            let tab = tab_of(cx, pane).ok_or("pane has no tab")?;
            let _ = mux_host::host(cx)
                .mux
                .set_trigger_title(tab, Some(text.clone()));
            cx.update_global::<TriggerRuntime, _>(|rt, _| {
                rt.fx.titles.insert(
                    tab,
                    TitleFx {
                        pane,
                        expires_ms: (*ttl_s > 0).then(|| now + u64::from(*ttl_s) * 1000),
                    },
                );
            });
            in_window(cx, pane, |view, cx| view.refresh_titles(cx));
        }
        Intent::RecordingStart { stop_after_s } => recording_start(cx, f, *stop_after_s, now)?,
        Intent::RecordingStop => recording_stop(cx, pane)?,
        Intent::Highlight {
            span,
            expect,
            color,
            style,
            ttl_s,
        } => {
            let line = line_of(cx, f).ok_or("no line")?;
            let max = cx
                .global::<crate::settings::Settings>()
                .config
                .triggers
                .highlights_max as usize;
            let h = Highlight {
                line,
                span: *span,
                expect: Some(expect.clone()).filter(|e| !e.is_empty()),
                color: color.clone(),
                style: *style,
                expires_ms: (*ttl_s > 0).then(|| now + u64::from(*ttl_s) * 1000),
                rule: f.rule_id.clone(),
            };
            cx.update_global::<TriggerRuntime, _>(|rt, _| rt.deco.add_highlight(pane, h, max));
        }
        Intent::Marker { label, color } => add_marker(cx, f, label, color, false)?,
        Intent::Findings { label, line, span } => {
            let line_id = line_of(cx, f).ok_or("no line")?;
            // A finding is also a marker, so the jump uses the same machinery.
            add_marker(cx, f, label, "", true)?;
            let max = cx
                .global::<crate::settings::Settings>()
                .config
                .triggers
                .findings_max as usize;
            let finding = Finding {
                at_ms: now,
                rule_id: f.rule_id.clone(),
                rule_name: f.rule_name.clone(),
                pane,
                label: label.clone(),
                line: line.clone(),
                line_id,
                span: *span,
                count: 1,
            };
            cx.update_global::<TriggerRuntime, _>(|rt, _| rt.findings.push(finding, max));
        }
        // The gated actions (their second gate was already checked by the engine).
        Intent::Clipboard { .. } | Intent::Snapshot { .. } | Intent::Screenshot { .. } => {
            let line = line_of(cx, f);
            super::gated::apply(cx, f, intent, line)?;
        }
    }
    Ok(())
}

/// The stable id of the line a firing is about: the matched line of a text rule, the cursor's
/// line for an event rule.
fn line_of(cx: &mut App, f: &Firing) -> Option<u64> {
    f.abs_line
        .or_else(|| mux_host::terminal(cx, f.pane).map(|t| t.cursor_line_id()))
}

fn add_marker(
    cx: &mut App,
    f: &Firing,
    label: &str,
    color: &str,
    keep: bool,
) -> Result<(), String> {
    let line = line_of(cx, f).ok_or("no line")?;
    let max = cx
        .global::<crate::settings::Settings>()
        .config
        .triggers
        .markers_max as usize;
    let m = Marker {
        line,
        label: label.to_string(),
        color: color.to_string(),
        rule: f.rule_id.clone(),
    };
    cx.update_global::<TriggerRuntime, _>(|rt, _| {
        if keep {
            rt.deco.add_marker_if_absent(f.pane, m, max);
        } else {
            rt.deco.add_marker(f.pane, m, max);
        }
    });
    // A recording of the pane gets the marker too.
    let recording = mux_host::host(cx).store_recorders.is_recording(f.pane);
    if recording {
        if let Some(term) = mux_host::terminal(cx, f.pane) {
            let _ = mux_host::host(cx)
                .store_recorders
                .marker(f.pane, &term, Some(label));
        }
    }
    Ok(())
}

fn recording_start(cx: &mut App, f: &Firing, stop_after_s: u32, now: u64) -> Result<(), String> {
    if !RecordingGate::of(cx).enabled {
        return Err("recording disabled".into());
    }
    let pane = f.pane;
    // A pane that is already being recorded (by anyone) is left alone.
    if mux_host::host(cx).store_recorders.is_recording(pane) {
        return Ok(());
    }
    let cfg = cx.global::<crate::settings::Settings>().config.clone();
    crate::recording::ipc::start_with(
        &mut crate::recording::ipc::GuiHost(cx),
        &cfg,
        pane,
        Some(false),
        Some(format!("Trigger: {}", f.rule_name)),
        Some(&f.rule_id),
    )
    .map_err(|e| e.message)?;
    if let Some(term) = mux_host::terminal(cx, pane) {
        let _ = mux_host::host(cx)
            .store_recorders
            .marker(pane, &term, Some(&f.rule_name));
    }
    cx.update_global::<TriggerRuntime, _>(|rt, _| {
        rt.fx.recs.insert(
            pane,
            RecFx {
                rule: f.rule_id.clone(),
                stop_at_ms: (stop_after_s > 0).then(|| now + u64::from(stop_after_s) * 1000),
            },
        );
    });
    Ok(())
}

/// Stops the recording of `pane` if a trigger started it (never one the user started).
fn recording_stop(cx: &mut App, pane: u64) -> Result<(), String> {
    let ours = cx.update_global::<TriggerRuntime, _>(|rt, _| rt.fx.recs.remove(&pane).is_some());
    if !ours {
        return Ok(());
    }
    stop_recording(cx, pane)
}

fn stop_recording(cx: &mut App, pane: u64) -> Result<(), String> {
    let term = mux_host::terminal(cx, pane);
    mux_host::host(cx)
        .store_recorders
        .stop(pane, term.as_deref())
        .map(|_| ())
        .map_err(|e| e.message)
}

/// A pane's process ended: whatever rules set for it ends too.
pub(crate) fn pane_exited(cx: &mut App, pane: u64) {
    let (colors, titles) = cx.update_global::<TriggerRuntime, _>(|rt, _| {
        rt.deco.forget_pane(pane);
        rt.fx.recs.remove(&pane);
        rt.fx.pane_exited(pane)
    });
    let h = mux_host::host(cx);
    for tab in colors {
        let _ = h.mux.set_trigger_color(tab, None);
    }
    for tab in titles {
        let _ = h.mux.set_trigger_title(tab, None);
    }
    mux_host::refresh_windows(cx);
}

/// The pane got focus: counters that reset on focus restart, `clear_on_focus` colors clear.
pub(crate) fn on_focus(cx: &mut App, pane: u64) {
    if !super::runtime::active(cx) {
        return;
    }
    let tabs = cx.update_global::<TriggerRuntime, _>(|rt, _| {
        rt.fx.reset_focus(pane);
        rt.fx.colors_cleared_by_focus(pane)
    });
    for tab in tabs {
        let _ = mux_host::host(cx).mux.set_trigger_color(tab, None);
    }
}

/// OSC 133 `C`: a command started in the pane.
pub(crate) fn on_command_start(cx: &mut App, pane: u64) {
    if super::runtime::active(cx) {
        cx.update_global::<TriggerRuntime, _>(|rt, _| rt.fx.reset_command_start(pane));
    }
}

/// Housekeeping for timed effects, called every tick.
pub(crate) fn tick(cx: &mut App) {
    let now = mt_term::now_ms();
    let (colors, titles, recs) = cx.update_global::<TriggerRuntime, _>(|rt, _| {
        rt.fx.expired_statuses(now);
        (
            rt.fx.expired_colors(now),
            rt.fx.expired_titles(now),
            rt.fx.recs_due(now),
        )
    });
    let mut changed = !colors.is_empty() || !titles.is_empty();
    // Decorations expire, and leave with the scrollback their line scrolled out of.
    let decorated: Vec<u64> = cx.global::<TriggerRuntime>().deco.panes();
    for pane in decorated {
        let Some(base) = mux_host::terminal(cx, pane).map(|t| t.line_base()) else {
            continue;
        };
        changed |= cx.update_global::<TriggerRuntime, _>(|rt, _| {
            rt.line_bases.insert(pane, base);
            rt.deco.prune(pane, base, now)
        });
    }
    {
        let h = mux_host::host(cx);
        for tab in colors {
            let _ = h.mux.set_trigger_color(tab, None);
        }
        for tab in titles {
            let _ = h.mux.set_trigger_title(tab, None);
        }
        changed |= !h.mux.expire_statuses(now).is_empty();
    }
    for pane in recs {
        cx.update_global::<TriggerRuntime, _>(|rt, _| rt.fx.recs.remove(&pane));
        let _ = stop_recording(cx, pane);
        changed = true;
    }
    // Recordings that ended by themselves (limit, pane exit) are no longer ours.
    let panes: Vec<u64> = cx
        .global::<TriggerRuntime>()
        .fx
        .recs
        .keys()
        .copied()
        .collect();
    for pane in panes {
        if !mux_host::host(cx).store_recorders.is_recording(pane) {
            cx.update_global::<TriggerRuntime, _>(|rt, _| rt.fx.recs.remove(&pane));
        }
    }
    if changed {
        mux_host::refresh_windows(cx);
    }
}

/// The feature was turned off: everything a rule set goes away and trigger recordings are
/// stopped and saved (SPEC §10.11.1).
pub(crate) fn cleanup(cx: &mut App) {
    let (colors, titles, statuses, recs) = cx.update_global::<TriggerRuntime, _>(|rt, _| {
        let colors: Vec<u64> = rt.fx.colors.keys().copied().collect();
        let titles: Vec<u64> = rt.fx.titles.keys().copied().collect();
        let statuses: Vec<u64> = rt.fx.statuses.keys().copied().collect();
        let recs: Vec<u64> = rt.fx.recs.keys().copied().collect();
        rt.fx.clear();
        rt.deco.clear();
        rt.findings.clear();
        rt.line_bases.clear();
        (colors, titles, statuses, recs)
    });
    let h = mux_host::host(cx);
    for tab in colors {
        let _ = h.mux.set_trigger_color(tab, None);
    }
    for tab in titles {
        let _ = h.mux.set_trigger_title(tab, None);
    }
    for pane in statuses {
        h.mux.clear_pane_status(pane);
    }
    let mut stopped = false;
    for pane in recs {
        stopped |= stop_recording(cx, pane).is_ok();
    }
    if stopped {
        mux_host::toast_all(
            cx,
            crate::components::ToastKind::Info,
            "Recording stopped: triggers disabled".into(),
        );
    }
    mux_host::refresh_windows(cx);
}
