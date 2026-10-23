//! Window-level tests for the Recordings panel and the export dialog (SPEC §10.10.5, M12.4):
//! the gate, the toggle/focus state machine, one right panel at a time, list actions and a live
//! record → export check against `/bin/sh`.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::tests::{boot, casts, config, serial, store_dir, toasts, wait_for};
use crate::{
    actions::*,
    app::AppWindow,
    overlay_stack::{FocusTarget, OverlayKind},
    right_panel::RightPanel,
};

const NOW_BACK: u64 = 5 * 86_400;

fn store_path() -> PathBuf {
    store_dir().join("microterm").join("recordings")
}

/// Writes `<id>.cast` into the shared store; the sidecar is rebuilt on the first listing.
fn seed(id: &str, title: &str, age_secs: u64, input: bool) -> PathBuf {
    std::fs::create_dir_all(store_path()).unwrap();
    let ts = mt_record::store::unix_now() - age_secs;
    let mut text = format!(
        "{{\"version\":2,\"width\":100,\"height\":30,\"timestamp\":{ts},\"title\":\"{title}\"}}\n\
         [0.5,\"o\",\"hello\"]\n"
    );
    if input {
        text.push_str("[0.9,\"i\",\"ls\\r\"]\n");
    }
    let p = store_path().join(format!("{id}.cast"));
    std::fs::write(&p, text).unwrap();
    let _ = std::fs::remove_file(store_path().join(format!("{id}.json")));
    p
}

fn ids(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |t, c| {
        crate::recording::panel_state::visible_ids(&t.recording_rows(c))
    })
}

fn select(view: &Entity<AppWindow>, cx: &mut VisualTestContext, id: &str) {
    view.update_in(cx, |t, _, cx| {
        t.recordings.panel.selected = Some(id.into());
        cx.notify();
    });
}

fn state(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> (bool, FocusTarget, bool) {
    view.read_with(cx, |t, _| {
        (
            t.recordings.panel.open,
            t.focus_target(),
            t.overlays.is_empty(),
        )
    })
}

const ON: &str = "[recording]\nenabled = true\n";

#[gpui::test]
fn panel_is_gated_and_actions_only_toast_while_disabled(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-off", ""));
    cx.dispatch_action(ToggleRecordingsPanel);
    cx.dispatch_action(FocusRecordingsPanel);
    cx.dispatch_action(ExportRecording);
    assert!(!view.read_with(cx, |t, _| t.recordings_visible()));
    assert_eq!(view.read_with(cx, |t, _| t.right_panel()), None);
    assert!(toasts(&view, cx)
        .iter()
        .any(|(t, open)| t.contains("Recording is disabled") && *open));
    assert!(view.read_with(cx, |t, _| t.recordings.dialog.is_none()));
}

#[gpui::test]
fn toggle_and_focus_state_machine(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-toggle", ON));
    assert_eq!(state(&view, cx), (false, FocusTarget::Viewport, true));
    cx.dispatch_action(ToggleRecordingsPanel);
    assert_eq!(state(&view, cx), (true, FocusTarget::Panel, true));
    view.update_in(cx, |t, w, cx| {
        t.recordings.panel.focused = false;
        t.sync_focus(w);
        cx.notify();
    });
    assert_eq!(state(&view, cx), (true, FocusTarget::Viewport, true));
    cx.dispatch_action(ToggleRecordingsPanel);
    assert_eq!(state(&view, cx), (true, FocusTarget::Panel, true));
    cx.dispatch_action(ToggleRecordingsPanel);
    assert_eq!(state(&view, cx), (false, FocusTarget::Viewport, true));
    cx.dispatch_action(FocusRecordingsPanel);
    cx.dispatch_action(FocusRecordingsPanel);
    assert_eq!(state(&view, cx), (true, FocusTarget::Panel, true));
    // Esc with an empty search hands focus to the terminal and keeps the panel open.
    cx.simulate_keystrokes("escape");
    assert_eq!(state(&view, cx), (true, FocusTarget::Viewport, true));
}

#[gpui::test]
fn only_one_right_panel_at_a_time(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(
        cx,
        config(
            "panel-one",
            "[recording]\nenabled = true\n[snippets]\nenabled = true\n",
        ),
    );
    cx.dispatch_action(ToggleRecordingsPanel);
    assert_eq!(
        view.read_with(cx, |t, _| t.right_panel()),
        Some(RightPanel::Recordings)
    );
    cx.dispatch_action(ToggleSnippetsPanel);
    assert_eq!(
        view.read_with(cx, |t, _| t.right_panel()),
        Some(RightPanel::Snippets)
    );
    assert!(!view.read_with(cx, |t, _| t.recordings.panel.open));
    // Each panel keeps its own width.
    view.update_in(cx, |t, _, _| {
        t.recordings.panel.width = 480.0;
        t.snippets.panel.width = 260.0;
    });
    cx.dispatch_action(ToggleRecordingsPanel);
    assert!(!view.read_with(cx, |t, _| t.snippets.panel.open));
    assert_eq!(
        view.read_with(cx, |t, _| (
            t.recordings.panel.width,
            t.snippets.panel.width
        )),
        (480.0, 260.0)
    );
}

#[gpui::test]
fn hot_reload_off_closes_the_panel_and_the_dialog(cx: &mut TestAppContext) {
    let _s = serial();
    let path = config("panel-reload", ON);
    let app = cx.clone();
    let (view, cx) = boot(cx, path.clone());
    seed("r_ui_reload", "Reload me", 60, false);
    cx.dispatch_action(ToggleRecordingsPanel);
    select(&view, cx, "r_ui_reload");
    cx.dispatch_action(ExportRecording);
    assert!(view.read_with(cx, |t, _| t.overlays.top()
        == Some(OverlayKind::ExportDialog)));
    std::fs::write(&path, "[recording]\nenabled = false\n").unwrap();
    app.update(crate::settings::reload);
    cx.run_until_parked();
    assert!(!view.read_with(cx, |t, _| t.recordings_visible()));
    assert!(view.read_with(cx, |t, _| t.recordings.dialog.is_none()));
    assert!(view.read_with(cx, |t, _| t.overlays.is_empty()));
}

#[gpui::test]
fn list_groups_searches_and_selects(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-list", ON));
    seed("r_ui_today", "Deploy staging", 120, false);
    seed("r_ui_old", "Old kernel build", NOW_BACK, false);
    cx.dispatch_action(ToggleRecordingsPanel);
    let all = ids(&view, cx);
    let (today, old) = (
        all.iter().position(|i| i == "r_ui_today").unwrap(),
        all.iter().position(|i| i == "r_ui_old").unwrap(),
    );
    assert!(today < old, "newest first: {all:?}");
    // Searching flattens and ranks over the title.
    cx.simulate_keystrokes("k e r n e l");
    assert_eq!(ids(&view, cx), vec!["r_ui_old".to_string()]);
    assert_eq!(
        view.read_with(cx, |t, _| t.recordings.panel.selected.clone()),
        Some("r_ui_old".into())
    );
    cx.simulate_keystrokes("escape");
    assert!(ids(&view, cx).len() >= 2);
    cx.simulate_keystrokes("down");
    assert!(view
        .read_with(cx, |t, _| t.recordings.panel.selected.clone())
        .is_some());
}

#[gpui::test]
fn f2_renames_the_sidecar_and_the_cast_header(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-rename", ON));
    let cast = seed("r_ui_rename", "Before", 30, false);
    cx.dispatch_action(ToggleRecordingsPanel);
    select(&view, cx, "r_ui_rename");
    cx.simulate_keystrokes("f2");
    assert!(view.read_with(cx, |t, _| t.recordings.rename.is_some()));
    cx.simulate_keystrokes("A f t e r");
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |t, _| t.recordings.rename.is_none()));
    let sc = mt_record::Store::new(store_path())
        .load("r_ui_rename")
        .unwrap();
    assert_eq!(sc.title, "After");
    let header = std::fs::read_to_string(cast).unwrap();
    assert!(header
        .lines()
        .next()
        .unwrap()
        .contains("\"title\":\"After\""));
    // Esc abandons a rename.
    cx.simulate_keystrokes("f2");
    cx.simulate_keystrokes("x");
    cx.simulate_keystrokes("escape");
    assert_eq!(
        mt_record::Store::new(store_path())
            .load("r_ui_rename")
            .unwrap()
            .title,
        "After"
    );
}

#[gpui::test]
fn delete_asks_first_and_confirm_delete_false_skips_the_question(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-delete", ON));
    let cast = seed("r_ui_del", "Delete me", 30, false);
    cx.dispatch_action(ToggleRecordingsPanel);
    select(&view, cx, "r_ui_del");
    cx.simulate_keystrokes("delete");
    assert_eq!(
        view.read_with(cx, |t, _| t.recordings.delete_confirm.clone()),
        Some("r_ui_del".into())
    );
    assert!(cast.is_file(), "nothing is deleted before the answer");
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |t, _| t.recordings.delete_confirm.is_none()));
    assert!(cast.is_file());
    cx.simulate_keystrokes("delete");
    cx.simulate_keystrokes("enter");
    assert!(!cast.exists());
    assert!(!ids(&view, cx).contains(&"r_ui_del".to_string()));

    let (view, cx) = boot(
        cx,
        config(
            "panel-delete-quiet",
            "[recording]\nenabled = true\nconfirm_delete = false\n",
        ),
    );
    let cast = seed("r_ui_del2", "Delete quietly", 30, false);
    cx.dispatch_action(ToggleRecordingsPanel);
    select(&view, cx, "r_ui_del2");
    cx.simulate_keystrokes("delete");
    assert!(!cast.exists());
}

#[gpui::test]
fn import_copies_a_cast_into_the_store(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-import", ON));
    let src = std::env::temp_dir().join(format!("mt_ui_import_{}.cast", std::process::id()));
    std::fs::write(
        &src,
        "{\"version\":2,\"width\":90,\"height\":20,\"title\":\"Imported one\"}\n[1.0,\"o\",\"x\"]\n",
    )
    .unwrap();
    cx.dispatch_action(ToggleRecordingsPanel);
    view.update_in(cx, |t, _, cx| t.import_cast(&src, cx));
    let id = view
        .read_with(cx, |t, _| t.recordings.panel.selected.clone())
        .expect("the import is selected");
    assert!(store_path().join(format!("{id}.cast")).is_file());
    assert!(store_path().join(format!("{id}.json")).is_file());
    assert_eq!(
        view.read_with(cx, |t, _| t
            .recordings
            .entries
            .iter()
            .find(|e| e.sc.id == id)
            .map(|e| e.sc.title.clone())),
        Some("Imported one".into())
    );
    let _ = std::fs::remove_file(src);
}

#[gpui::test]
fn export_dialog_disables_vhs_without_input_and_exports_cast(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-export", ON));
    seed("r_ui_exp", "Export me", 30, false);
    let out = std::env::temp_dir().join(format!("mt_ui_export_{}.cast", std::process::id()));
    let _ = std::fs::remove_file(&out);
    cx.dispatch_action(ToggleRecordingsPanel);
    select(&view, cx, "r_ui_exp");
    cx.simulate_keystrokes("secondary-e");
    assert_eq!(
        view.read_with(cx, |t, _| t.overlays.top()),
        Some(OverlayKind::ExportDialog)
    );
    let (reason, format) = view.read_with(cx, |t, _| {
        let d = t.recordings.dialog.as_ref().unwrap();
        let vhs = mt_export::registry()
            .iter()
            .position(|e| e.id() == "vhs")
            .unwrap();
        (d.unavailable(vhs), d.exporter().id())
    });
    assert!(reason.unwrap().contains("Needs input"));
    assert_eq!(format, "cast");
    // A bad option stays in the dialog with a message.
    view.update_in(cx, |t, _, _| {
        let d = t.recordings.dialog.as_mut().unwrap();
        let ix = d
            .specs()
            .iter()
            .position(|s| s.key == "idle_limit")
            .unwrap();
        d.fields[d.format][ix] = crate::components::TextEdit::new("nope");
    });
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |t, _| t
        .recordings
        .dialog
        .as_ref()
        .is_some_and(|d| d.error.as_deref().is_some_and(|e| e.contains("idle_limit")))));
    view.update_in(cx, |t, _, _| {
        let d = t.recordings.dialog.as_mut().unwrap();
        let ix = d
            .specs()
            .iter()
            .position(|s| s.key == "idle_limit")
            .unwrap();
        d.fields[d.format][ix] = crate::components::TextEdit::new("0");
        d.path = crate::components::TextEdit::new(out.to_string_lossy().into_owned());
        d.path_dirty = true;
    });
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |t, _| t.recordings.dialog.is_none()));
    finish_exports(&view, cx);
    assert!(out.is_file(), "the cast was exported");
    assert!(toasts(&view, cx)
        .iter()
        .any(|(t, _)| t.starts_with("Exported Export me as cast")));
    let buttons = view.read_with(cx, |t, _| {
        t.toasts
            .iter()
            .any(|x| x.buttons.iter().map(|b| b.label()).eq(["Reveal", "Open"]))
    });
    assert!(buttons);
    // Exporting onto it again asks before replacing.
    cx.dispatch_action(ExportRecording);
    view.update_in(cx, |t, _, _| {
        let d = t.recordings.dialog.as_mut().unwrap();
        d.path = crate::components::TextEdit::new(out.to_string_lossy().into_owned());
        d.path_dirty = true;
    });
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |t, _| t.recordings.dialog.is_some()));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |t, _| t.recordings.dialog.is_none()));
    let _ = std::fs::remove_file(out);
}

/// Lets the 50 ms poll task run until every export job has reported.
fn finish_exports(view: &Entity<AppWindow>, cx: &mut VisualTestContext) {
    for _ in 0..200 {
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        if view.read_with(cx, |t, _| t.recordings.jobs.is_empty()) {
            return;
        }
    }
    panic!("exports did not finish");
}

#[gpui::test]
fn toast_buttons_open_the_panel_and_the_dialog(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("panel-toast", ON));
    cx.dispatch_action(ToggleRecording);
    cx.dispatch_action(ToggleRecording);
    let (open_panel, export) = view.read_with(cx, |t, _| {
        let b = &t
            .toasts
            .iter()
            .rev()
            .find(|x| !x.buttons.is_empty())
            .unwrap()
            .buttons;
        (b[0].clone(), b[1].clone())
    });
    assert_eq!(open_panel.label(), "Open panel");
    view.update_in(cx, |t, w, cx| t.toast_button(open_panel, w, cx));
    assert!(view.read_with(cx, |t, _| t.recordings_visible()));
    view.update_in(cx, |t, w, cx| t.toast_button(export, w, cx));
    assert_eq!(
        view.read_with(cx, |t, _| t.overlays.top()),
        Some(OverlayKind::ExportDialog)
    );
    cx.simulate_keystrokes("escape");
}

#[gpui::test]
fn live_record_then_export_cast_and_vhs(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(
        cx,
        config(
            "panel-live",
            "[recording]\nenabled = true\ncapture_input = true\n",
        ),
    );
    let before = casts();
    cx.dispatch_action(ToggleRecordingsPanel);
    cx.dispatch_action(ToggleRecording);
    // The running recording is the first row, with a Stop button's pane.
    let running = view.read_with(cx, |t, c| {
        t.recording_entries(c)
            .into_iter()
            .filter(|e| e.running)
            .count()
    });
    assert_eq!(running, 1);
    view.read_with(cx, |t, _| {
        t.live()
            .expect("live tab")
            .term
            .write(b"echo panel-live-ok\r".to_vec())
    });
    assert!(wait_for(&view, cx, "panel-live-ok"));
    cx.dispatch_action(ToggleRecording);
    let after = casts();
    let cast = after
        .iter()
        .find(|p| !before.contains(p))
        .expect("new cast")
        .clone();
    let id = cast.file_stem().unwrap().to_string_lossy().into_owned();
    view.update_in(cx, |t, _, cx| {
        t.recordings_changed(Some(id.clone()), cx);
    });
    assert_eq!(
        view.read_with(cx, |t, _| t.recordings.panel.selected.clone()),
        Some(id.clone())
    );
    let dir = std::env::temp_dir().join(format!("mt_ui_live_export_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (format, ext) in [("cast", "cast"), ("vhs", "tape")] {
        cx.dispatch_action(ExportRecording);
        let out = dir.join(format!("out.{ext}"));
        view.update_in(cx, |t, _, _| {
            let d = t.recordings.dialog.as_mut().unwrap();
            let ix = mt_export::registry()
                .iter()
                .position(|e| e.id() == format)
                .unwrap();
            assert!(d.select_format(ix), "{format} is available with input");
            d.path = crate::components::TextEdit::new(out.to_string_lossy().into_owned());
            d.path_dirty = true;
        });
        cx.simulate_keystrokes("enter");
        finish_exports(&view, cx);
        let text = std::fs::read_to_string(&out).unwrap();
        if format == "vhs" {
            assert!(text.contains("echo panel-live-ok"), "{text}");
        } else {
            assert!(text.contains("panel-live-ok"), "{text}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}
