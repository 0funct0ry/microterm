//! Table-driven VT conformance: every case of `vt_conformance.toml` is fed to an emulator and
//! its grid, cursor, modes, events or reply bytes are checked.

use std::sync::{Arc, Mutex};

use mt_term::vt_doc::{rows, Case};
use mt_term::{ColorReport, CursorShape, Emulator, ProgressState, TermEvent, VtConfig};

fn event_name(e: &TermEvent) -> Option<String> {
    Some(match e {
        TermEvent::Bell => "bell".into(),
        TermEvent::Title(t) => format!("title:{t}"),
        TermEvent::ClipboardStore(_, t) => format!("clipboard:{t}"),
        TermEvent::Notify(t) => format!("notify:{t}"),
        TermEvent::Progress { state, value } => {
            let s = match state {
                ProgressState::Remove => "Remove",
                ProgressState::Normal => "Normal",
                ProgressState::Error => "Error",
                ProgressState::Indeterminate => "Indeterminate",
                ProgressState::Paused => "Paused",
            };
            match value {
                Some(v) => format!("progress:{s}:{v}"),
                None => format!("progress:{s}"),
            }
        }
        _ => return None,
    })
}

fn run(case: &Case) -> Result<(), String> {
    let mut e = Emulator::new(20, 5, 100);
    let replies = Arc::new(Mutex::new(Vec::<u8>::new()));
    let r = replies.clone();
    e.set_reply_sink(move |b| r.lock().unwrap().extend(b));
    e.set_vt_config(VtConfig {
        report: match case.report.as_deref() {
            Some(s) => ColorReport::parse(s).ok_or("bad report")?,
            None => ColorReport::Bit16,
        },
        osc_notifications: case.osc_notifications.unwrap_or(true),
        allow_shift_capture: case.allow_shift_capture.unwrap_or(true),
    });
    e.advance(case.input.as_bytes());
    if let Some(want) = &case.reply {
        let want = &want.replace("{version}", env!("CARGO_PKG_VERSION"));
        let got = String::from_utf8_lossy(&replies.lock().unwrap()).into_owned();
        if &got != want {
            return Err(format!("reply: want {want:?}, got {got:?}"));
        }
    }
    let events: Vec<String> = e.drain_events().iter().filter_map(event_name).collect();
    if let Some(want) = &case.events {
        if &events != want {
            return Err(format!("events: want {want:?}, got {events:?}"));
        }
    }
    let snap = e.snapshot();
    if let Some(want) = &case.lines {
        let got: Vec<String> = (0..want.len()).map(|r| snap.row_text(r)).collect();
        if &got != want {
            return Err(format!("lines: want {want:?}, got {got:?}"));
        }
    }
    if let Some([row, col]) = case.cursor {
        let c = snap.cursor;
        if (c.row, c.col) != (row, col) {
            return Err(format!(
                "cursor: want {:?}, got {:?}",
                (row, col),
                (c.row, c.col)
            ));
        }
    }
    if let Some(shape) = &case.cursor_shape {
        let got = match snap.cursor.shape {
            CursorShape::Block => "block",
            CursorShape::Beam => "beam",
            CursorShape::Underline => "underline",
        };
        if got != shape {
            return Err(format!("cursor shape: want {shape}, got {got}"));
        }
    }
    let mode = |name: &str| -> Result<bool, String> {
        Ok(match name {
            "app_keypad" => snap.modes.app_keypad,
            "app_cursor" => snap.modes.app_cursor,
            "bracketed_paste" => snap.modes.bracketed_paste,
            "alt_screen" => snap.modes.alt_screen,
            "shift_capture" => snap.vt.shift_capture,
            other => return Err(format!("unknown mode {other}")),
        })
    };
    for m in &case.modes_on {
        if !mode(m)? {
            return Err(format!("mode {m} should be on"));
        }
    }
    for m in &case.modes_off {
        if mode(m)? {
            return Err(format!("mode {m} should be off"));
        }
    }
    if let Some(p) = &case.pointer {
        let got = snap.vt.pointer_shape.clone().unwrap_or_default();
        if &got != p {
            return Err(format!("pointer: want {p:?}, got {got:?}"));
        }
    }
    if let Some(cwd) = &case.cwd {
        if e.cwd().as_deref() != Some(cwd.as_str()) {
            return Err(format!("cwd: want {cwd:?}, got {:?}", e.cwd()));
        }
    }
    Ok(())
}

#[test]
fn vt_conformance() {
    let mut failures = Vec::new();
    let mut ran = 0;
    for row in rows() {
        for (i, case) in row.case.iter().enumerate() {
            ran += 1;
            if let Err(why) = run(case) {
                failures.push(format!(
                    "{} ({}) case {i}: {why}\n    input {:?}",
                    row.code, row.name, case.input
                ));
            }
        }
    }
    assert!(ran > 150, "only {ran} cases ran");
    assert!(
        failures.is_empty(),
        "{} of {ran} cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
