//! Window-level snippet tests: the gate matrix, the panel's focus machine, hot reload, and
//! insertion into a real `/bin/sh` pane (SPEC §10.8, M11.5 "Tests").

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{ClipboardItem, Entity, TestAppContext, VisualTestContext};
use mt_term::SpawnOptions;

use super::{library, panel_state::Gesture};
use crate::{
    actions::*,
    app::AppWindow,
    overlay_stack::{FocusTarget, OverlayKind},
    settings::{self, Settings},
};

const SNIPPETS: &str = r#"# my snippets
[[snippets]]
id = "uuid"
title = "Print a uuid"
body = "echo {{uuid}}"

[[snippets]]
id = "runme"
title = "Say hello"
body = "echo hello-run"
run = true
folder = "demo"

[[snippets]]
id = "cursor"
title = "Cursor demo"
body = "echo {{ cursor }}tail"

[[snippets]]
id = "ask"
title = "Ask for a word"
body = '''echo {{ input "Word" | upper }}-{{ choice "Env" "dev" "prod" }}'''

[[snippets]]
id = "clip"
title = "From the clipboard"
body = "echo {{ clipboard }}"
"#;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mt_snip_ui_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A config dir with `microterm.toml` and `snippets.toml`; returns the config path.
fn env(name: &str, config: &str, snippets: &str) -> PathBuf {
    let d = dir(name);
    std::fs::write(d.join("microterm.toml"), config).unwrap();
    std::fs::write(d.join("snippets.toml"), snippets).unwrap();
    d.join("microterm.toml")
}

fn boot(cx: &mut TestAppContext, config: PathBuf) -> (Entity<AppWindow>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Settings::load(config));
        settings::apply_theme(cx);
        library::sync(cx);
        settings::install_keymap(cx);
    });
    let (view, cx) = cx.add_window_view(AppWindow::new);
    view.update_in(cx, |this, window, cx| {
        this.attach_terminal(
            0,
            SpawnOptions {
                shell: "/bin/sh".into(),
                env: vec![("PS1".into(), "$ ".into())],
                ..Default::default()
            },
            window,
            cx,
        )
        .expect("spawn /bin/sh");
    });
    cx.run_until_parked();
    (view, cx)
}

fn screen(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |this, _| {
        let snap = &this.live().expect("live tab").snapshot;
        (0..snap.rows).map(|r| snap.row_text(r)).collect()
    })
}

fn wait_for(
    view: &Entity<AppWindow>,
    cx: &mut VisualTestContext,
    pred: impl Fn(&str) -> bool,
) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(30));
        cx.run_until_parked();
        let rows = screen(view, cx);
        if rows.iter().any(|r| pred(r)) || Instant::now() > deadline {
            return rows;
        }
    }
}

fn insert(view: &Entity<AppWindow>, cx: &mut VisualTestContext, id: &str, g: Gesture) {
    let id = id.to_string();
    view.update_in(cx, |this, w, cx| this.insert_snippet(&id, g, w, cx));
    cx.run_until_parked();
}

fn type_into_shell(view: &Entity<AppWindow>, cx: &mut VisualTestContext, text: &str) {
    let text = text.to_string();
    view.read_with(cx, |this, _| {
        this.live().unwrap().term.write(text.into_bytes())
    });
}

fn toast_texts(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<(String, bool)> {
    view.read_with(cx, |t, _| {
        t.toasts
            .iter()
            .map(|x| (x.text.to_string(), x.open_config))
            .collect()
    })
}

const ON: &str = "[snippets]\nenabled = true\n";
const ON_TEMPLATES: &str = "[snippets]\nenabled = true\ntemplates = true\n";

// ------------------------------------------------------------------ the gate matrix

#[gpui::test]
fn default_config_is_inert(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("inert", "", SNIPPETS));
    assert!(cx.update(|_, c| library::set(c).snippets.is_empty()));
    cx.dispatch_action(ToggleSnippetsPanel);
    let toasts = toast_texts(&view, cx);
    assert!(
        toasts
            .iter()
            .any(|(t, open)| t.contains("Snippets are disabled") && *open),
        "{toasts:?}"
    );
    assert!(!view.read_with(cx, |t, _| t.panel_visible()));
    // Insert actions and the palette provider are absent.
    cx.dispatch_action(ShowCommandPalette);
    let labels: Vec<String> = view.read_with(cx, |t, _| {
        let p = t.picker.as_ref().unwrap();
        p.state
            .matches()
            .iter()
            .map(|m| p.state.label(m).to_string())
            .collect()
    });
    assert!(
        !labels
            .iter()
            .any(|l| l.contains("snippet") || l.contains("Snippet")),
        "{labels:?}"
    );
    view.update(cx, |t, _| t.picker.as_mut().unwrap().state.set_query("!"));
    let scope = view.read_with(cx, |t, _| t.picker.as_ref().unwrap().state.scope().0);
    assert_ne!(
        scope,
        Some(crate::app::group::SNIPPETS),
        "`!` is not a provider while disabled"
    );
}

#[gpui::test]
fn enabled_without_templates_inserts_literally(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("literal", ON, SNIPPETS));
    insert(&view, cx, "uuid", Gesture::default());
    let rows = wait_for(&view, cx, |r| r.contains("echo {{uuid}}"));
    assert!(
        rows.iter()
            .any(|r| r.trim_end().ends_with("$ echo {{uuid}}")),
        "{rows:?}"
    );
    // The fill dialog never opens when templates are off, even for a body with prompts.
    insert(&view, cx, "ask", Gesture::default());
    assert_eq!(view.read_with(cx, |t, _| t.overlays.top()), None);
}

#[gpui::test]
fn templates_expand_at_insert_time(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("templates", ON_TEMPLATES, SNIPPETS));
    insert(&view, cx, "uuid", Gesture::default());
    let rows = wait_for(&view, cx, |r| {
        r.contains("$ echo ") && r.trim_end().len() > 40
    });
    let line = rows
        .iter()
        .find(|r| r.contains("$ echo "))
        .expect("echo line")
        .trim_end()
        .to_string();
    let id = line.rsplit("echo ").next().unwrap();
    assert_eq!(id.len(), 36, "{line}");
    assert_eq!(id.matches('-').count(), 4);
    // Not run: the shell has not printed the uuid on its own line.
    assert_eq!(rows.iter().filter(|r| r.trim() == id).count(), 0);
}

// ------------------------------------------------------------------ run, cursor, sanitising

#[gpui::test]
fn run_sends_enter_and_the_gesture_overrides_it(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("run", ON_TEMPLATES, SNIPPETS));
    insert(&view, cx, "runme", Gesture::default());
    let rows = wait_for(&view, cx, |r| r.trim() == "hello-run");
    assert!(
        rows.iter().any(|r| r.trim() == "hello-run"),
        "the snippet's run = true: {rows:?}"
    );
    // Shift never runs, even for run = true.
    type_into_shell(&view, cx, "clear\r");
    wait_for(&view, cx, |r| r.trim() == "$");
    insert(
        &view,
        cx,
        "runme",
        Gesture {
            run: Some(false),
            broadcast: false,
        },
    );
    let rows = wait_for(&view, cx, |r| r.contains("$ echo hello-run"));
    assert!(
        rows.iter()
            .any(|r| r.trim_end().ends_with("$ echo hello-run")),
        "{rows:?}"
    );
    assert!(!rows.iter().any(|r| r.trim() == "hello-run"), "{rows:?}");
}

#[gpui::test]
fn the_cursor_marker_leaves_the_shell_cursor_in_place(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("cursor", ON_TEMPLATES, SNIPPETS));
    insert(&view, cx, "cursor", Gesture::default());
    wait_for(&view, cx, |r| r.contains("echo tail"));
    // Typing now lands at the marker.
    type_into_shell(&view, cx, "X");
    let rows = wait_for(&view, cx, |r| r.contains("echo Xtail"));
    assert!(rows.iter().any(|r| r.contains("echo Xtail")), "{rows:?}");
}

#[gpui::test]
fn esc_in_clipboard_content_is_stripped(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("esc", ON_TEMPLATES, SNIPPETS));
    cx.write_to_clipboard(ClipboardItem::new_string("a\u{1b}[31mb".into()));
    insert(&view, cx, "clip", Gesture::default());
    let rows = wait_for(&view, cx, |r| r.contains("echo a"));
    let line = rows
        .iter()
        .find(|r| r.contains("echo a"))
        .expect("line")
        .trim_end()
        .to_string();
    assert!(line.ends_with("echo a[31mb"), "{line}");
}

// ------------------------------------------------------------------ the fill dialog

#[gpui::test]
fn fill_dialog_preview_equals_the_inserted_bytes(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("fill", ON_TEMPLATES, SNIPPETS));
    insert(&view, cx, "ask", Gesture::default());
    assert_eq!(
        view.read_with(cx, |t, _| t.overlays.top()),
        Some(OverlayKind::SnippetFill)
    );
    view.update(cx, |t, _| {
        let f = t.snippets.fill.as_mut().unwrap();
        assert_eq!(f.fields.len(), 2, "field order follows the template");
        f.type_text("hello");
    });
    let preview = view
        .update(cx, |t, cx| t.fill_preview(cx))
        .expect("preview")
        .text;
    assert_eq!(preview, "echo HELLO-dev");
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    // Enter on the first field inserts (the dialog closes).
    assert_eq!(view.read_with(cx, |t, _| t.overlays.top()), None);
    let rows = wait_for(&view, cx, |r| r.contains("echo HELLO-"));
    let line = rows
        .iter()
        .find(|r| r.contains("echo HELLO-"))
        .unwrap()
        .trim_end()
        .to_string();
    assert!(
        line.ends_with("echo HELLO-dev") || line.ends_with("echo HELLO-prod"),
        "{line}"
    );
}

#[gpui::test]
fn esc_in_the_fill_dialog_sends_nothing(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("fillesc", ON_TEMPLATES, SNIPPETS));
    insert(&view, cx, "ask", Gesture::default());
    view.update(cx, |t, _| {
        t.snippets.fill.as_mut().unwrap().type_text("zzz")
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |t, _| t.overlays.top()), None);
    assert!(view.read_with(cx, |t, _| t.snippets.fill.is_none()));
    std::thread::sleep(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(!screen(&view, cx).iter().any(|r| r.contains("ZZZ")));
}

#[gpui::test]
fn remembered_values_prefill_the_next_fill_but_not_secrets(cx: &mut TestAppContext) {
    let toml = "[[snippets]]\nid = \"s\"\ntitle = \"S\"\nbody = '''echo {{ input \"A\" }} {{ secret \"T\" }}'''\n";
    let (view, cx) = boot(cx, env("remember", ON_TEMPLATES, toml));
    insert(&view, cx, "s", Gesture::default());
    view.update(cx, |t, _| {
        let f = t.snippets.fill.as_mut().unwrap();
        f.type_text("keep");
        f.focus_next(false);
        f.type_text("hunter2");
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    wait_for(&view, cx, |r| r.contains("echo keep hunter2"));
    insert(&view, cx, "s", Gesture::default());
    let (a, t) = view.read_with(cx, |t, _| {
        let f = t.snippets.fill.as_ref().unwrap();
        (
            f.value(0).expose().to_string(),
            f.value(1).expose().to_string(),
        )
    });
    assert_eq!((a.as_str(), t.as_str()), ("keep", ""));
}

// ------------------------------------------------------------------ the panel

#[gpui::test]
fn toggle_and_focus_state_machine(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("toggle", ON, SNIPPETS));
    let state = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.read_with(cx, |t, _| {
            (
                t.snippets.panel.open,
                t.focus_target(),
                t.overlays.is_empty(),
            )
        })
    };
    assert_eq!(state(&view, cx), (false, FocusTarget::Viewport, true));
    cx.dispatch_action(ToggleSnippetsPanel);
    assert_eq!(
        state(&view, cx),
        (true, FocusTarget::Panel, true),
        "closed -> open + focused"
    );
    cx.dispatch_action(ToggleSnippetsPanel);
    assert_eq!(
        state(&view, cx),
        (false, FocusTarget::Viewport, true),
        "focused -> closed"
    );
    cx.dispatch_action(ToggleSnippetsPanel);
    view.update_in(cx, |t, w, cx| {
        t.snippets.panel.focused = false;
        t.sync_focus(w);
        cx.notify();
    });
    assert_eq!(state(&view, cx), (true, FocusTarget::Viewport, true));
    cx.dispatch_action(ToggleSnippetsPanel);
    assert_eq!(
        state(&view, cx),
        (true, FocusTarget::Panel, true),
        "open but unfocused -> focused"
    );
    cx.dispatch_action(FocusSnippetsPanel);
    assert_eq!(
        state(&view, cx),
        (true, FocusTarget::Panel, true),
        "focus never closes"
    );
}

#[gpui::test]
fn esc_clears_the_query_then_returns_to_the_terminal(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("esc_panel", ON, SNIPPETS));
    cx.dispatch_action(ToggleSnippetsPanel);
    cx.simulate_keystrokes("c l");
    assert_eq!(
        view.read_with(cx, |t, _| t.snippets.panel.query.text().to_string()),
        "cl"
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(
        view.read_with(cx, |t, _| t.snippets.panel.query.text().to_string()),
        ""
    );
    assert!(view.read_with(cx, |t, _| t.focus_target() == FocusTarget::Panel));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |t, _| t.snippets.panel.open
        && t.focus_target() == FocusTarget::Viewport));
}

#[gpui::test]
fn enter_inserts_the_selected_snippet(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("enter", ON, SNIPPETS));
    cx.dispatch_action(ToggleSnippetsPanel);
    cx.simulate_keystrokes("c u r s o r");
    assert_eq!(
        view.read_with(cx, |t, _| t.snippets.panel.selected.clone())
            .as_deref(),
        Some("cursor")
    );
    cx.simulate_keystrokes("enter");
    let rows = wait_for(&view, cx, |r| r.contains("echo {{ cursor }}tail"));
    assert!(
        rows.iter().any(|r| r.contains("echo {{ cursor }}tail")),
        "{rows:?}"
    );
    // focus_terminal_after_insert (default true) hands focus back.
    assert!(view.read_with(cx, |t, _| t.focus_target() == FocusTarget::Viewport));
}

#[gpui::test]
fn hot_reload_toggles_both_gates_in_an_open_window(cx: &mut TestAppContext) {
    let f = env("hot", "", SNIPPETS);
    let app = cx.clone();
    let (view, vcx) = boot(cx, f.clone());
    assert!(!view.read_with(vcx, |t, _| t.panel_visible()));

    std::fs::write(&f, ON).unwrap();
    app.update(settings::reload);
    vcx.run_until_parked();
    vcx.dispatch_action(ToggleSnippetsPanel);
    assert!(
        view.read_with(vcx, |t, _| t.panel_visible()),
        "enabling needs no restart"
    );
    assert_eq!(app.update(|cx| library::set(cx).snippets.len()), 5);

    // Per-snippet keys register with the feature.
    std::fs::write(f.with_file_name("snippets.toml"), format!("{SNIPPETS}\n[[snippets]]\nid = \"k\"\ntitle = \"K\"\nbody = \"x\"\nkeys = \"cmd+alt+7\"\n")).unwrap();
    app.update(library::sync_and_notify);
    vcx.run_until_parked();
    assert_eq!(app.update(|cx| library::set(cx).snippets.len()), 6);

    // Templates: the next insert expands.
    std::fs::write(&f, ON_TEMPLATES).unwrap();
    app.update(settings::reload);
    insert(&view, vcx, "uuid", Gesture::default());
    let rows = wait_for(&view, vcx, |r| {
        r.contains("$ echo ") && r.trim_end().len() > 40
    });
    assert!(!rows.iter().any(|r| r.contains("{{uuid}}")), "{rows:?}");

    // Turning the feature off closes the panel and empties the library.
    std::fs::write(&f, "").unwrap();
    app.update(settings::reload);
    vcx.run_until_parked();
    assert!(!view.read_with(vcx, |t, _| t.panel_visible()));
    assert!(app.update(|cx| library::set(cx).snippets.is_empty()));
}

#[gpui::test]
fn an_invalid_edit_keeps_the_old_set_and_shows_a_banner(cx: &mut TestAppContext) {
    let f = env("invalid", ON, SNIPPETS);
    let app = cx.clone();
    let (_view, vcx) = boot(cx, f.clone());
    let snip = f.with_file_name("snippets.toml");
    assert_eq!(app.update(|cx| library::set(cx).snippets.len()), 5);
    std::fs::write(&snip, "[[snippets]\ntitle = ").unwrap();
    app.update(library::sync_and_notify);
    vcx.run_until_parked();
    assert_eq!(
        app.update(|cx| library::set(cx).snippets.len()),
        5,
        "last valid set stays"
    );
    let banner = app.update(|cx| library::banner(cx)).expect("banner");
    assert!(
        banner.text.contains("snippets.toml:1:") && banner.text.contains("last valid snippets"),
        "{}",
        banner.text
    );
    std::fs::write(&snip, SNIPPETS).unwrap();
    app.update(library::sync_and_notify);
    assert!(app.update(|cx| library::banner(cx)).is_none());
}

#[gpui::test]
fn the_palette_lists_snippets_with_a_bang_prefix(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("palette", ON, SNIPPETS));
    cx.dispatch_action(ShowCommandPalette);
    view.update(cx, |t, _| {
        t.picker.as_mut().unwrap().state.set_query("!cursor")
    });
    let labels: Vec<String> = view.read_with(cx, |t, _| {
        let p = t.picker.as_ref().unwrap();
        p.state
            .matches()
            .iter()
            .map(|m| p.state.label(m).to_string())
            .collect()
    });
    assert_eq!(labels, ["Snippet: Cursor demo"]);
    cx.simulate_keystrokes("enter");
    let rows = wait_for(&view, cx, |r| r.contains("echo {{ cursor }}tail"));
    assert!(
        rows.iter().any(|r| r.contains("echo {{ cursor }}tail")),
        "{rows:?}"
    );
}

#[gpui::test]
fn broadcast_targets_every_pane_of_the_tab(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("broadcast", ON, SNIPPETS));
    assert_eq!(view.read_with(cx, |t, _| t.insert_targets(true).len()), 1);
    view.update_in(cx, |t, w, cx| t.split_focused(mt_mux::Dir::Right, w, cx));
    cx.run_until_parked();
    let (all, one) = view.read_with(cx, |t, _| (t.insert_targets(true), t.insert_targets(false)));
    assert_eq!(all.len(), 2);
    assert_eq!(one.len(), 1);
    assert!(all.contains(&one[0]));
}

// ------------------------------------------------------------------ the editor

#[gpui::test]
fn the_editor_saves_surgically_and_blocks_template_errors(cx: &mut TestAppContext) {
    let f = env("editor", ON_TEMPLATES, SNIPPETS);
    let snip = f.with_file_name("snippets.toml");
    let (view, cx) = boot(cx, f);
    view.update_in(cx, |t, w, cx| t.new_snippet(w, cx));
    assert!(view.read_with(cx, |t, _| t.snippets.editor.is_some()));
    view.update(cx, |t, _| {
        let ed = t.snippets.editor.as_mut().unwrap();
        ed.title.insert("Brand new");
        ed.body.insert("echo {{ nope }}");
    });
    view.update_in(cx, |t, w, cx| t.save_editor(w, cx));
    let err = view
        .read_with(cx, |t, _| t.snippets.editor.as_ref().unwrap().error.clone())
        .expect("blocked");
    assert!(err.contains("Template error at 1:9"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&snip).unwrap(),
        SNIPPETS,
        "nothing written"
    );

    view.update(cx, |t, _| {
        let ed = t.snippets.editor.as_mut().unwrap();
        ed.body = crate::components::TextAreaEdit::new("echo {{ uuid }}");
    });
    view.update_in(cx, |t, w, cx| t.save_editor(w, cx));
    cx.run_until_parked();
    let after = std::fs::read_to_string(&snip).unwrap();
    assert!(
        after.starts_with(SNIPPETS),
        "other entries and comments are byte-identical"
    );
    assert!(after.contains("id = \"brand-new\""), "{after}");
    assert!(std::fs::read_to_string(snip.with_file_name("snippets.toml.bak")).unwrap() == SNIPPETS);
    assert!(view.read_with(cx, |t, _| t.snippets.editor.is_none()));
    assert!(
        cx.update(|_, c| library::set(c).get("brand-new").is_some()),
        "the panel sees it at once"
    );
}

#[gpui::test]
fn closing_a_dirty_editor_asks_first(cx: &mut TestAppContext) {
    let (view, cx) = boot(cx, env("dirty", ON, SNIPPETS));
    view.update_in(cx, |t, w, cx| t.edit_snippet("uuid", w, cx));
    assert!(view.read_with(cx, |t, _| !t.snippets.editor.as_ref().unwrap().dirty()));
    view.update(cx, |t, _| {
        t.snippets.editor.as_mut().unwrap().title.insert("!")
    });
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |t, _| t
        .snippets
        .editor
        .as_ref()
        .unwrap()
        .confirm_discard));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |t, _| t.snippets.editor.is_some()
        && !t.snippets.editor.as_ref().unwrap().confirm_discard));
    cx.simulate_keystrokes("escape enter");
    assert!(
        view.read_with(cx, |t, _| t.snippets.editor.is_none()),
        "Enter discards"
    );
}

#[gpui::test]
fn a_stale_save_is_refused_until_reloaded(cx: &mut TestAppContext) {
    let f = env("stale", ON, SNIPPETS);
    let snip = f.with_file_name("snippets.toml");
    let (view, cx) = boot(cx, f);
    view.update_in(cx, |t, w, cx| t.edit_snippet("uuid", w, cx));
    view.update(cx, |t, _| {
        t.snippets
            .editor
            .as_mut()
            .unwrap()
            .title
            .insert(" (edited)")
    });
    // Someone edits the same entry on disk.
    std::fs::write(&snip, SNIPPETS.replace("Print a uuid", "Changed elsewhere")).unwrap();
    view.update_in(cx, |t, w, cx| t.save_editor(w, cx));
    assert!(view.read_with(cx, |t, _| t.snippets.editor.as_ref().unwrap().stale));
    assert!(std::fs::read_to_string(&snip)
        .unwrap()
        .contains("Changed elsewhere"));
    // "Reload and re-apply", then Save applies the edits to the fresh file.
    view.update(cx, |t, cx_| {
        let _ = cx_;
        t.snippets.editor.as_mut().unwrap().stale = true;
    });
    view.update_in(cx, |t, _, cx| t.reload_editor_base(cx));
    view.update_in(cx, |t, w, cx| t.save_editor(w, cx));
    assert!(std::fs::read_to_string(&snip)
        .unwrap()
        .contains("Print a uuid (edited)"));
}
