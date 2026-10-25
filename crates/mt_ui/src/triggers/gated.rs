//! The actions that leave the window: `clipboard`, `snapshot` and `screenshot` (SPEC §10.11.4,
//! §10.11.7). Each sits behind a second gate that only `microterm.toml` sets; the engine already
//! dropped these intents while their gate is closed. Files go to the captures directory only,
//! and are written (and the PNG rendered) off the main thread.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, BorrowAppContext, ClipboardItem};
use mt_triggers::capture::{CaptureDir, Ext, Limits};
use mt_triggers::template::sanitize;
use mt_triggers::{Intent, ShotChrome};

use super::engine::Firing;
use super::runtime::TriggerRuntime;
use crate::components::ToastKind;
use crate::mux_host;

/// How long a `snapshot` waits for its `after` lines (SPEC §10.11.4).
const AFTER_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(200);
const LINE_CHARS: usize = 4096;

pub(super) fn apply(
    cx: &mut App,
    f: &Firing,
    intent: &Intent,
    line: Option<u64>,
) -> Result<(), String> {
    match intent {
        Intent::Clipboard { text } => clipboard(cx, f, text),
        Intent::Snapshot { before, after } => snapshot(
            cx,
            f,
            *before as usize,
            *after as usize,
            line.ok_or("no line")?,
        ),
        Intent::Screenshot { scale, chrome } => screenshot(cx, f, *scale, *chrome),
        _ => Ok(()),
    }
}

/// System clipboard only (never the PRIMARY selection), and it always says so.
fn clipboard(cx: &mut App, f: &Firing, text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("nothing to copy".into());
    }
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    mux_host::toast_all(
        cx,
        ToastKind::Info,
        format!(
            "Trigger \"{}\" copied {} characters to the clipboard",
            sanitize(&f.rule_name),
            text.chars().count()
        ),
    );
    Ok(())
}

/// The captures directory per `[triggers]`; refuses an unsafe one.
pub(crate) fn open_dir(cx: &App) -> Result<CaptureDir, String> {
    let t = &cx.global::<crate::settings::Settings>().config.triggers;
    let dir = if t.capture_dir.is_empty() {
        mt_triggers::capture::default_dir().ok_or("no home directory")?
    } else {
        PathBuf::from(&t.capture_dir)
    };
    CaptureDir::open(
        &dir,
        Limits {
            max_text_bytes: t.capture_max_kb as usize * 1024,
            max_files: t.capture_max_files as usize,
            max_total_bytes: u64::from(t.capture_max_total_mb) * 1024 * 1024,
        },
    )
    .map_err(|e| e.to_string())
}

fn finish(cx: &mut App, rule_id: String, kind: &'static str, result: Result<PathBuf, String>) {
    cx.update_global::<TriggerRuntime, _>(|rt, _| match result {
        Ok(path) => {
            rt.captures_last.insert(rule_id, path);
        }
        Err(e) => rt
            .engine
            .note_error(&rule_id, format!("{kind} skipped: {e}")),
    });
    mux_host::refresh_windows(cx);
}

fn header(f: &Firing, domain: &str, now_ms: u64) -> String {
    format!(
        "# microterm trigger capture\n# rule: {}\n# time: {}\n# pane: {}\n# domain: {}\n\n",
        sanitize(&f.rule_name),
        mt_proto::iso8601_utc(now_ms),
        f.pane,
        sanitize(domain),
    )
}

fn snapshot(
    cx: &mut App,
    f: &Firing,
    before: usize,
    after: usize,
    line: u64,
) -> Result<(), String> {
    let dir = open_dir(cx)?;
    let term = mux_host::terminal(cx, f.pane).ok_or("pane has no terminal")?;
    let redact = cx
        .global::<crate::settings::Settings>()
        .config
        .triggers
        .capture_redact;
    let domain = mux_host::host(cx)
        .mux
        .rows(&|_| mt_mux::ctl::Live::default())
        .panes
        .into_iter()
        .find(|p| p.id == f.pane)
        .map_or_else(String::new, |p| p.domain);
    let (rule_id, head) = (f.rule_id.clone(), header(f, &domain, mt_term::now_ms()));
    // The match and what came before are read now; the lines after it are waited for (at most
    // 5 s), so the file shows what the match led to.
    cx.spawn(async move |cx| {
        let started = std::time::Instant::now();
        let captured = loop {
            let Some(c) = term.capture_lines(line, before, after, LINE_CHARS) else {
                break None;
            };
            if c.after.len() >= after || started.elapsed() >= AFTER_WAIT {
                break Some(c);
            }
            cx.background_executor().timer(POLL).await;
        };
        let result = match captured {
            None => Err("the line left the scrollback".to_string()),
            Some(c) => {
                let mut text = head;
                for l in c
                    .before
                    .iter()
                    .chain(std::iter::once(&c.line))
                    .chain(&c.after)
                {
                    text.push_str(l);
                    text.push('\n');
                }
                let id = rule_id.clone();
                cx.background_executor()
                    .spawn(async move {
                        let body = if redact {
                            mt_patterns::redact(&text)
                        } else {
                            text
                        };
                        dir.write(&id, Ext::Txt, body.as_bytes(), mt_term::now_ms() / 1000)
                            .map_err(|e| e.to_string())
                    })
                    .await
            }
        };
        let _ = cx.update(|cx| finish(cx, rule_id, "snapshot", result));
    })
    .detach();
    Ok(())
}

/// A PNG of the pane's visible grid: the cell snapshot is taken here, the picture is drawn and
/// written on a worker.
fn screenshot(cx: &mut App, f: &Firing, scale: f32, chrome: ShotChrome) -> Result<(), String> {
    let dir = open_dir(cx)?;
    let term = mux_host::terminal(cx, f.pane).ok_or("pane has no terminal")?;
    let snap = term.peek_snapshot();
    let theme = {
        let ui = cx.global::<crate::theme::Theme>().name;
        let reg = &cx.global::<crate::settings::Settings>().registry;
        reg.get(ui)
            .cloned()
            .unwrap_or_else(|| reg.resolve(ui, true).clone())
    };
    let font_size = cx.global::<crate::settings::Settings>().config.font.size;
    let title = mux_host::host(cx)
        .mux
        .pane(f.pane)
        .map_or_else(String::new, |p| p.title.clone());
    let rule_id = f.rule_id.clone();
    cx.spawn(async move |cx| {
        let id = rule_id.clone();
        let result = cx
            .background_executor()
            .spawn(async move {
                let opts = mt_export::screenshot::FrameOpts {
                    scale,
                    plain_chrome: chrome == ShotChrome::Plain,
                    font_size,
                };
                let frame =
                    mt_export::screenshot::png_frame(&snap, &theme, &sanitize(&title), &opts)
                        .map_err(|e| e.to_string())?;
                dir.write(&id, Ext::Png, &frame.png, mt_term::now_ms() / 1000)
                    .map_err(|e| e.to_string())
            })
            .await;
        let _ = cx.update(|cx| finish(cx, rule_id, "screenshot", result));
    })
    .detach();
    Ok(())
}
