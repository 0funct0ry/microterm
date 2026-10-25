//! Window-level tests for the Triggers panel (SPEC §10.11.5, M12.6): the gate, the toggle and
//! focus state machine, one right panel at a time, "Create Trigger from Selection", the editor's
//! save / delete / duplicate / Test, and the demo from the prompt, end to end.

use gpui::{Entity, TestAppContext, VisualTestContext};
use mt_term::SelectionKind;
use std::path::PathBuf;

use super::runtime::{self, TriggerRuntime};
use crate::{
    actions::*,
    app::AppWindow,
    overlay_stack::FocusTarget,
    recording::tests::{boot, config, palette_labels, serial, toasts, wait_for},
    right_panel::RightPanel,
};

const ON: &str = "shell = \"/bin/sh\"\n[triggers]\nenabled = true\n";

fn sync(cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        runtime::sync(cx);
    });
    cx.run_until_parked();
}

fn rules_path(cfg: &std::path::Path) -> PathBuf {
    cfg.with_file_name("triggers.toml")
}

fn state(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> (bool, FocusTarget) {
    view.read_with(cx, |t, _| (t.triggers.panel.open, t.focus_target()))
}

fn rule_ids(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| {
        cx.global::<TriggerRuntime>()
            .engine
            .rules()
            .map(|r| r.id.clone())
            .collect()
    })
}

#[gpui::test]
fn everything_is_inert_and_only_toasts_while_the_gate_is_off(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("trig-off", ""));
    macro_rules! only_toasts {
        ($($a:expr),+) => {$(
            cx.dispatch_action($a);
            assert!(!view.read_with(cx, |t, _| t.triggers_visible()));
            assert!(
                toasts(&view, cx).iter().any(|(t, open)| t.contains("Triggers are disabled") && *open),
                stringify!($a)
            );
        )+};
    }
    only_toasts!(
        ToggleTriggersPanel,
        FocusTriggersPanel,
        NewTrigger,
        CreateTriggerFromSelection,
        OpenTriggersFile,
        ToggleTriggersPaused,
        NextMarker,
        PreviousMarker,
        ClearMarkers
    );
    assert_eq!(view.read_with(cx, |t, _| t.right_panel()), None);
    assert!(view.read_with(cx, |t, _| t.triggers.editor.is_none()));
    // No palette rows, no markers provider.
    let labels = palette_labels(&view, cx);
    assert!(
        !labels
            .iter()
            .any(|l| l.to_lowercase().contains("trigger") || l.contains("marker")),
        "{labels:?}"
    );
    // No file was created.
    assert!(!cx.update(|_, cx| runtime::path(cx).exists()));
}

#[gpui::test]
fn toggle_and_focus_state_machine_and_one_panel_at_a_time(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(
        cx,
        config(
            "trig-toggle",
            &format!("{ON}[snippets]\nenabled = true\n[recording]\nenabled = true\n"),
        ),
    );
    sync(cx);
    assert_eq!(state(&view, cx), (false, FocusTarget::Viewport));
    cx.dispatch_action(ToggleTriggersPanel);
    assert_eq!(state(&view, cx), (true, FocusTarget::Panel));
    assert_eq!(
        view.read_with(cx, |t, _| t.right_panel()),
        Some(RightPanel::Triggers)
    );
    view.update_in(cx, |t, w, cx| {
        t.triggers.panel.focused = false;
        t.sync_focus(w);
        cx.notify();
    });
    assert_eq!(state(&view, cx), (true, FocusTarget::Viewport));
    cx.dispatch_action(ToggleTriggersPanel);
    assert_eq!(state(&view, cx), (true, FocusTarget::Panel));
    cx.dispatch_action(ToggleTriggersPanel);
    assert_eq!(state(&view, cx), (false, FocusTarget::Viewport));
    cx.dispatch_action(FocusTriggersPanel);
    cx.dispatch_action(FocusTriggersPanel);
    assert_eq!(state(&view, cx), (true, FocusTarget::Panel));
    cx.simulate_keystrokes("escape");
    assert_eq!(
        state(&view, cx),
        (true, FocusTarget::Viewport),
        "Esc hands focus to the terminal"
    );
    // The other panels take the slot and close this one, and the other way round.
    cx.dispatch_action(ToggleSnippetsPanel);
    assert_eq!(
        view.read_with(cx, |t, _| t.right_panel()),
        Some(RightPanel::Snippets)
    );
    assert!(!view.read_with(cx, |t, _| t.triggers.panel.open));
    cx.dispatch_action(ToggleTriggersPanel);
    assert_eq!(
        view.read_with(cx, |t, _| t.right_panel()),
        Some(RightPanel::Triggers)
    );
    assert!(!view.read_with(cx, |t, _| t.snippets.panel.open));
    cx.dispatch_action(ToggleRecordingsPanel);
    assert_eq!(
        view.read_with(cx, |t, _| t.right_panel()),
        Some(RightPanel::Recordings)
    );
    assert!(!view.read_with(cx, |t, _| t.triggers.panel.open));
    // Each panel keeps its own width.
    view.update_in(cx, |t, _, _| t.triggers.panel.width = 480.0);
    cx.dispatch_action(ToggleTriggersPanel);
    assert_eq!(view.read_with(cx, |t, _| t.triggers.panel.width), 480.0);
}

#[gpui::test]
fn palette_lists_the_trigger_rows_only_while_on_and_create_needs_a_selection(
    cx: &mut TestAppContext,
) {
    let _s = serial();
    let (view, cx) = boot(cx, config("trig-palette", ON));
    sync(cx);
    let labels = palette_labels(&view, cx);
    for want in [
        "Toggle triggers panel",
        "New trigger",
        "Open triggers file",
        "Next marker",
        "Clear markers",
    ] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
    assert!(
        !labels.iter().any(|l| l == "Create trigger from selection"),
        "no selection yet"
    );
    view.read_with(cx, |t, _| t.live().unwrap().term.select_all());
    view.update_in(cx, |t, _, _| t.live_mut().unwrap().refresh());
    let labels = palette_labels(&view, cx);
    assert!(
        labels.iter().any(|l| l == "Create trigger from selection"),
        "{labels:?}"
    );
}

/// The first window, as a handle (these tests boot the way `main` does, so the mux host knows
/// the window and its terminal).
fn win(cx: &mut TestAppContext) -> gpui::WindowHandle<AppWindow> {
    cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find_map(|w| w.downcast::<AppWindow>())
            .expect("a window")
    })
}

fn with<R>(
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut AppWindow, &mut gpui::Window, &mut gpui::Context<AppWindow>) -> R,
) -> R {
    let h = win(cx);
    cx.update(|cx| h.update(cx, f).expect("window update"))
}

#[gpui::test]
fn the_prompt_demo_end_to_end(cx: &mut TestAppContext) {
    use super::tests::{boot as boot_real, term, type_line, wait_until};
    let _s = serial();
    let cfg = config("trig-demo", ON);
    let pane = boot_real(cx, cfg.clone());
    // Put the text on screen and select that line.
    type_line(cx, pane, "echo 'error: build failed'");
    let screen = |cx: &mut TestAppContext| {
        let s = term(cx, pane).snapshot();
        (0..s.rows).map(|r| s.row_text(r)).collect::<Vec<_>>()
    };
    wait_until(cx, "the output", |cx| {
        screen(cx).iter().any(|r| r == "error: build failed")
    });
    let row = screen(cx)
        .iter()
        .position(|r| r == "error: build failed")
        .unwrap();
    let t = term(cx, pane);
    t.selection_start(SelectionKind::Simple, row, 0, false, "");
    t.selection_update(row, 18, true);
    with(cx, |t, w, cx| t.create_trigger_from_selection(w, cx));
    // The editor has the escaped selection and a default highlight action.
    let (regex, actions) = with(cx, |t, _, _| {
        let e = t.triggers.editor.as_ref().expect("the editor opened");
        (
            e.regex.text().to_string(),
            e.actions.iter().map(|a| a.kind).collect::<Vec<_>>(),
        )
    });
    assert_eq!(regex, "error: build failed");
    assert_eq!(actions, ["highlight"]);
    // Name it, add `tab color red`, `notify` and `findings`, and Save.
    with(cx, |t, _, cx| {
        let e = t.triggers.editor.as_mut().unwrap();
        e.name = crate::components::TextEdit::new("Build failed");
        e.add_action("tab_color");
        e.add_action("notify");
        e.add_action("findings");
        e.actions[0].fields[0] = crate::components::TextEdit::new("line");
        e.actions[0].fields[1] = crate::components::TextEdit::new("red");
        cx.notify();
    });
    assert_eq!(
        with(cx, |t, _, _| t
            .triggers
            .editor
            .as_ref()
            .unwrap()
            .save_blocker()),
        None
    );
    with(cx, |t, _, cx| t.save_rule_editor(cx));
    cx.run_until_parked();
    assert!(
        with(cx, |t, _, _| t.triggers.editor.is_none()),
        "saving returns to the list"
    );
    let file = std::fs::read_to_string(rules_path(&cfg)).unwrap();
    assert!(
        file.starts_with("version = 1") && file.contains("name = \"Build failed\""),
        "{file}"
    );
    let id = cx.update(|cx| {
        cx.global::<TriggerRuntime>()
            .engine
            .rules()
            .next()
            .map(|r| r.id.clone())
    });
    let id = id.expect("the saved rule is loaded");
    wait_until(cx, "the tap", |cx| super::tests::tapped(cx, pane));
    // Run the failing output again: the line highlights, the tab turns red, a notification
    // arrives with its badge, a finding is added and clicking it scrolls back to the line.
    t.selection_clear();
    type_line(cx, pane, "echo 'error: build failed'");
    wait_until(cx, "the rule", |cx| super::tests::fired(cx, &id) >= 1);
    let (hl, findings) = cx.update(|cx| {
        let rt = cx.global::<TriggerRuntime>();
        (rt.deco.highlight_count(pane), rt.findings.len())
    });
    assert!(
        hl >= 1 && findings >= 1,
        "highlight {hl}, findings {findings}"
    );
    let chrome = cx.update(|cx| crate::mux_host::tab_chrome(cx, pane));
    assert_eq!(
        chrome.badge,
        Some(crate::components::StatusKind::Accent),
        "the notification's badge (info level)"
    );
    assert!(chrome
        .tooltip
        .unwrap()
        .contains("Color: red (trigger: Build failed)"));
    // Scroll the line out of view, then click the finding.
    type_line(cx, pane, "seq 1 200");
    wait_until(cx, "the output", |cx| screen(cx).iter().any(|r| r == "200"));
    assert_eq!(term(cx, pane).view_info().2, 0, "the live view");
    assert!(cx.update(|cx| runtime::jump_to_finding(cx, 0, true)));
    assert!(
        term(cx, pane).view_info().2 > 0,
        "scrolled back to the line"
    );
}

#[gpui::test]
fn editor_save_delete_duplicate_and_the_test_dry_run(cx: &mut TestAppContext) {
    let _s = serial();
    let cfg = config("trig-editor", ON);
    let (view, cx) = boot(cx, cfg.clone());
    sync(cx);
    view.update_in(cx, |t, w, cx| t.new_trigger(w, cx));
    assert!(view.read_with(cx, |t, _| t.triggers.editor.is_some()
        && t.triggers_visible()));
    // A blank rule cannot be saved.
    view.update_in(cx, |t, _, cx| t.save_rule_editor(cx));
    assert!(view.read_with(cx, |t, _| t
        .triggers
        .editor
        .as_ref()
        .unwrap()
        .error
        .is_some()));
    view.update_in(cx, |t, _, cx| {
        let e = t.triggers.editor.as_mut().unwrap();
        e.name = crate::components::TextEdit::new("Prompt");
        e.regex = crate::components::TextEdit::new(r"Continue\? \[y/N\]");
        e.error = None;
        e.actions[0].fields[0] = crate::components::TextEdit::new("error");
        cx.notify();
    });
    // Test is a dry run: it lists what would run and runs nothing.
    view.read_with(cx, |t, _| {
        t.live()
            .unwrap()
            .term
            .write(b"echo 'Continue? [y/N]'\n".to_vec())
    });
    assert!(wait_for(&view, cx, "Continue? [y/N]"));
    view.update_in(cx, |t, _, cx| t.test_rule_editor(cx));
    let test = view.read_with(cx, |t, _| {
        t.triggers.editor.as_ref().unwrap().test.clone().unwrap()
    });
    assert!(!test.is_empty() && test.iter().all(|f| f.input.contains("Continue? [y/N]")));
    assert!(test[0].intents.iter().any(|i| i.kind() == "highlight"));
    assert_eq!(
        cx.update(|_, cx| cx.global::<TriggerRuntime>().deco.highlight_count(1)),
        0
    );
    assert!(!rules_path(&cfg).exists(), "Test writes nothing");
    // Save, reopen, duplicate, delete.
    view.update_in(cx, |t, _, cx| t.save_rule_editor(cx));
    cx.run_until_parked();
    let ids = rule_ids(cx);
    assert_eq!(ids.len(), 1);
    view.update_in(cx, |t, w, cx| t.edit_trigger(&ids[0], w, cx));
    assert!(view.read_with(cx, |t, _| !t.triggers.editor.as_ref().unwrap().dirty()));
    view.update_in(cx, |t, _, cx| t.duplicate_rule_editor(cx));
    let (id, name) = view.read_with(cx, |t, _| {
        let e = t.triggers.editor.as_ref().unwrap();
        (e.id.clone(), e.name.text().to_string())
    });
    assert_eq!((id, name.as_str()), (None, "Prompt copy"));
    view.update_in(cx, |t, _, cx| t.save_rule_editor(cx));
    cx.run_until_parked();
    assert_eq!(rule_ids(cx).len(), 2);
    // Delete asks first (triggers.confirm_delete), then removes just that rule.
    let victim = rule_ids(cx)[1].clone();
    view.update_in(cx, |t, w, cx| t.edit_trigger(&victim, w, cx));
    view.update_in(cx, |t, _, cx| t.delete_rule_editor(cx));
    assert_eq!(rule_ids(cx).len(), 2, "first click only asks");
    view.update_in(cx, |t, _, cx| t.delete_rule_editor(cx));
    cx.run_until_parked();
    assert_eq!(rule_ids(cx), vec![ids[0].clone()]);
    let file = std::fs::read_to_string(rules_path(&cfg)).unwrap();
    assert!(
        file.contains("name = \"Prompt\"") && !file.contains("Prompt copy"),
        "{file}"
    );
}

#[gpui::test]
fn the_list_switch_writes_enabled_back_and_pause_is_session_only(cx: &mut TestAppContext) {
    let _s = serial();
    let cfg = config("trig-switch", ON);
    std::fs::write(
        rules_path(&cfg),
        "# my rules\nversion = 1\n\n[[trigger]]\nid = \"t_a\"\nname = \"A\" # keep\n[trigger.when]\nkind = \"text\"\nregex = 'x'\n[[trigger.actions]]\nkind = \"bell\"\n",
    )
    .unwrap();
    let (view, cx) = boot(cx, cfg.clone());
    sync(cx);
    assert_eq!(rule_ids(cx), ["t_a"]);
    view.update_in(cx, |t, _, cx| t.set_trigger_enabled("t_a", false, cx));
    cx.run_until_parked();
    let file = std::fs::read_to_string(rules_path(&cfg)).unwrap();
    assert!(
        file.contains("enabled = false") && file.contains("# my rules") && file.contains("# keep"),
        "{file}"
    );
    assert!(!cx.update(|_, cx| cx.global::<TriggerRuntime>().engine.is_enabled("t_a")));
    // Pause never touches the file.
    let before = std::fs::read_to_string(rules_path(&cfg)).unwrap();
    cx.dispatch_action(ToggleTriggersPaused);
    assert!(cx.update(|_, cx| cx.global::<TriggerRuntime>().engine.paused()));
    cx.dispatch_action(ToggleTriggersPaused);
    assert!(!cx.update(|_, cx| cx.global::<TriggerRuntime>().engine.paused()));
    assert_eq!(std::fs::read_to_string(rules_path(&cfg)).unwrap(), before);
}

#[gpui::test]
fn turning_the_gate_off_closes_the_panel_and_discards_the_draft(cx: &mut TestAppContext) {
    let _s = serial();
    let cfg = config("trig-gateflip", ON);
    let app = cx.clone();
    let (view, cx) = boot(cx, cfg.clone());
    sync(cx);
    view.update_in(cx, |t, w, cx| t.new_trigger(w, cx));
    assert!(view.read_with(cx, |t, _| t.triggers_visible()
        && t.triggers.editor.is_some()));
    std::fs::write(&cfg, "shell = \"/bin/sh\"\n").unwrap();
    app.update(crate::settings::reload);
    cx.run_until_parked();
    assert!(!view.read_with(cx, |t, _| t.triggers_visible()));
    assert!(view.read_with(cx, |t, _| t.triggers.editor.is_none()));
    assert_eq!(view.read_with(cx, |t, _| t.right_panel()), None);
    assert!(toasts(&view, cx)
        .iter()
        .any(|(t, _)| t.contains("draft discarded")));
}

/// Draws the window (which renders the open panel) so a layout or element-id mistake in any
/// of its states panics here instead of in front of the user.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn every_state_of_the_panel_renders(cx: &mut TestAppContext) {
    let _s = serial();
    let cfg = config(
        "trig-render",
        &format!("{ON}allow_capture = false\n[recording]\nenabled = true\n"),
    );
    std::fs::write(
        rules_path(&cfg),
        r#"version = 1
[[trigger]]
id = "t_a"
name = "Text rule"
[trigger.when]
kind = "text"
regex = 'error: (?P<m>.+)'
[[trigger.actions]]
kind = "notify"
message = "{m}"
[[trigger.actions]]
kind = "snapshot"
[[trigger.actions]]
kind = "clipboard"

[[trigger]]
id = "t_b"
name = "Event rule"
panes = "!focused:true"
enabled = false
[trigger.when]
kind = "event"
event = "command_finished"
exit = "failure"
[[trigger.actions]]
kind = "marker"
"#,
    )
    .unwrap();
    let (view, cx) = boot(cx, cfg);
    sync(cx);
    draw(cx);
    // Closed, then the rules list, then the findings tab.
    cx.dispatch_action(ToggleTriggersPanel);
    draw(cx);
    cx.update(|_, cx| {
        let rt = cx.global_mut::<TriggerRuntime>();
        rt.findings.push(
            super::findings::Finding {
                at_ms: 1,
                rule_id: "t_a".into(),
                rule_name: "Text rule".into(),
                pane: 1,
                label: "hit".into(),
                line: "error: boom".into(),
                line_id: 3,
                span: None,
                count: 1,
            },
            10,
        );
    });
    view.update_in(cx, |t, _, cx| {
        t.triggers.tab = super::ui::PanelTab::Findings;
        cx.notify();
    });
    draw(cx);
    view.update_in(cx, |t, _, cx| {
        t.triggers.tab = super::ui::PanelTab::Rules;
        cx.notify();
    });
    // Every editor state: text and event rules, each action kind, the add menu, Test results,
    // errors, the discard and delete confirmations.
    view.update_in(cx, |t, w, cx| t.edit_trigger("t_a", w, cx));
    draw(cx);
    view.update_in(cx, |t, _, cx| {
        let e = t.triggers.editor.as_mut().unwrap();
        for k in [
            "highlight",
            "tab_color",
            "bell",
            "clipboard",
            "recording",
            "marker",
            "counter",
            "title",
            "findings",
            "snapshot",
            "screenshot",
            "status",
        ] {
            e.actions.clear();
            e.add_action(k);
            e.focus = super::editor::Focus::Param(0, 0);
        }
        e.add_menu = true;
        e.error = Some("could not save".into());
        e.confirm_discard = true;
        e.confirm_delete = true;
        e.stale = true;
        e.regex = crate::components::TextEdit::new("(?=bad");
        cx.notify();
    });
    draw(cx);
    view.update_in(cx, |t, _, cx| {
        let e = t.triggers.editor.as_mut().unwrap();
        e.regex = crate::components::TextEdit::new("x");
        e.set_text_kind(false);
        e.set_event(mt_triggers::EventKind::CommandFinished);
        e.panes = crate::components::TextEdit::new("nokey:x");
        e.preview = Some("2 matches in the last 200 lines".into());
        cx.notify();
    });
    draw(cx);
    view.update_in(cx, |t, w, cx| {
        t.edit_trigger("t_b", w, cx);
        t.test_rule_editor(cx);
    });
    draw(cx);
    view.update_in(cx, |t, w, cx| {
        t.new_trigger(w, cx);
    });
    draw(cx);
    view.update_in(cx, |t, _, cx| t.close_rule_editor(cx));
    draw(cx);
}

#[gpui::test]
fn clicking_the_terminal_takes_the_keyboard_back_from_the_panel(cx: &mut TestAppContext) {
    let _s = serial();
    let (view, cx) = boot(cx, config("trig-clickfocus", ON));
    sync(cx);
    cx.dispatch_action(ToggleTriggersPanel);
    assert_eq!(state(&view, cx), (true, FocusTarget::Panel));
    // A click inside the terminal area (well below the tab bar, left of the panel).
    cx.simulate_click(
        gpui::point(gpui::px(60.), gpui::px(200.)),
        gpui::Modifiers::none(),
    );
    assert_eq!(
        state(&view, cx),
        (true, FocusTarget::Viewport),
        "the panel stays open, the keys go to the terminal"
    );
    // The same for the other panels.
    cx.dispatch_action(ToggleSnippetsPanel);
    view.update_in(cx, |t, _, _| {
        t.snippets.panel.focused = true;
        assert!(t.release_panel_focus());
        assert!(!t.release_panel_focus());
    });
}

#[gpui::test]
fn refreshing_the_preview_and_closing_the_editor_never_re_enter_the_window(
    cx: &mut TestAppContext,
) {
    let _s = serial();
    let cfg = config("trig-reenter", ON);
    let (view, cx) = boot(cx, cfg);
    sync(cx);
    view.update_in(cx, |t, w, cx| t.new_trigger(w, cx));
    // The preview timer's body runs inside the window update; it used to update every window,
    // this one included, and aborted on the double lease.
    view.update_in(cx, |t, _, cx| {
        t.triggers.editor.as_mut().unwrap().set_text_kind(false);
        t.refresh_trigger_preview(cx);
        t.close_rule_editor(cx);
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |t, _| t.triggers.editor.is_none()));
}
