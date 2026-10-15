//! Behavioural tests for the window shell, driven through real keystrokes and actions.

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::{
    actions::{self, *},
    app::AppWindow,
    keychips::format_keystroke,
    overlay_stack::OverlayKind,
    platform::Platform,
    theme::Theme,
};

fn setup(cx: &mut TestAppContext) -> (Entity<AppWindow>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::one_dark());
        let bindings = actions::key_bindings(Platform::current(), cx);
        cx.bind_keys(bindings);
    });
    let (view, cx) = cx.add_window_view(AppWindow::new);
    cx.run_until_parked();
    (view, cx)
}

fn top(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Option<OverlayKind> {
    view.read_with(cx, |this, _| this.overlays.top())
}

fn viewport_focused(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> bool {
    view.update_in(cx, |this, window, _| this.viewport_focus.is_focused(window))
}

fn overlay_focused(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> bool {
    view.update_in(cx, |this, window, _| this.overlay_focus.is_focused(window))
}

fn first_binding(name: &str) -> &'static str {
    actions::ACTIONS
        .iter()
        .find(|a| a.name == name)
        .and_then(|a| a.bindings(Platform::current()).first().copied())
        .unwrap_or_else(|| panic!("{name} has no binding on this platform"))
}

#[gpui::test]
fn starts_with_focus_on_the_viewport(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    assert!(viewport_focused(&view, cx));
    assert_eq!(top(&view, cx), None);
}

#[gpui::test]
fn every_overlay_opens_from_its_keybinding_and_closes_with_escape(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let cases = [
        ("ShowCommandPalette", OverlayKind::CommandPalette),
        ("ShowLauncher", OverlayKind::Launcher),
        ("ShowWorkspaceSwitcher", OverlayKind::WorkspaceSwitcher),
        ("ActivateQuickSelect", OverlayKind::QuickSelect),
        ("ActivateCopyMode", OverlayKind::CopyMode),
        ("Search", OverlayKind::Search),
    ];
    for (name, kind) in cases {
        cx.simulate_keystrokes(first_binding(name));
        assert_eq!(top(&view, cx), Some(kind), "{name} did not open {kind:?}");
        assert!(
            overlay_focused(&view, cx),
            "{name}: overlay should own focus"
        );
        cx.simulate_keystrokes("escape");
        assert_eq!(top(&view, cx), None, "{name}: Esc should close it");
        assert!(
            viewport_focused(&view, cx),
            "{name}: focus should return to the viewport"
        );
    }
}

#[gpui::test]
fn palette_filters_navigates_and_runs_an_action(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("new tab");
    let before = view.read_with(cx, |this, _| this.tabs.len());
    let label = view.read_with(cx, |this, _| {
        let p = this.picker.as_ref().unwrap();
        p.state.label(p.state.selected_match().unwrap()).to_string()
    });
    assert_eq!(label, "New tab");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(top(&view, cx), None, "confirming closes the palette");
    assert!(viewport_focused(&view, cx));
    assert_eq!(view.read_with(cx, |this, _| this.tabs.len()), before + 1);
}

#[gpui::test]
fn palette_arrow_keys_move_the_selection(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ShowCommandPalette);
    let selected = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.read_with(cx, |this, _| this.picker.as_ref().unwrap().state.selected())
    };
    assert_eq!(selected(&view, cx), 0);
    cx.simulate_keystrokes("down down");
    assert_eq!(selected(&view, cx), 2);
    cx.simulate_keystrokes("up");
    assert_eq!(selected(&view, cx), 1);
}

#[gpui::test]
fn escape_closes_only_the_top_overlay(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateCopyMode);
    cx.dispatch_action(Search);
    assert_eq!(top(&view, cx), Some(OverlayKind::Search));
    cx.simulate_keystrokes("escape");
    assert_eq!(
        top(&view, cx),
        Some(OverlayKind::CopyMode),
        "copy mode stays open"
    );
    assert!(overlay_focused(&view, cx));
    cx.simulate_keystrokes("escape");
    assert_eq!(top(&view, cx), None);
    assert!(viewport_focused(&view, cx));
}

#[gpui::test]
fn pressing_a_shortcut_twice_closes_its_overlay(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.simulate_keystrokes(first_binding("ShowCommandPalette"));
    cx.simulate_keystrokes(first_binding("ShowCommandPalette"));
    assert_eq!(top(&view, cx), None);
    assert!(viewport_focused(&view, cx));
}

#[gpui::test]
fn theme_selector_previews_live_and_escape_reverts(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SelectTheme);
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "One Dark");
    cx.simulate_keystrokes("down");
    assert_eq!(
        cx.update(|_, cx| cx.global::<Theme>().name),
        "One Light",
        "selection previews"
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(
        cx.update(|_, cx| cx.global::<Theme>().name),
        "One Dark",
        "Esc reverts"
    );
    assert_eq!(top(&view, cx), None);
}

#[gpui::test]
fn theme_selector_enter_keeps_the_theme(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SelectTheme);
    cx.simulate_keystrokes("down enter");
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "One Light");
    assert_eq!(top(&view, cx), None);
    // Reopening and cancelling must not revert to the older theme.
    cx.dispatch_action(SelectTheme);
    cx.simulate_keystrokes("escape");
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "One Light");
}

#[gpui::test]
fn tabs_can_be_added_closed_and_switched(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), 3);
    cx.dispatch_action(SpawnTab);
    assert_eq!(
        view.read_with(cx, |t, _| (t.tabs.len(), t.active_tab)),
        (4, 3)
    );
    cx.dispatch_action(ActivateTab(0));
    assert_eq!(view.read_with(cx, |t, _| t.active_tab), 0);
    cx.dispatch_action(ActivateTabRelative(-1));
    assert_eq!(view.read_with(cx, |t, _| t.active_tab), 3, "wraps around");
    cx.dispatch_action(CloseTab);
    assert_eq!(
        view.read_with(cx, |t, _| (t.tabs.len(), t.active_tab)),
        (3, 2)
    );
}

#[gpui::test]
fn copy_mode_cursor_moves_with_hjkl(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateCopyMode);
    let pos = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.read_with(cx, |t, _| (t.copy.row, t.copy.col))
    };
    let (row, col) = pos(&view, cx);
    assert_eq!(col, 0);
    cx.simulate_keystrokes("k");
    assert_eq!(pos(&view, cx), (row - 1, 0));
    cx.simulate_keystrokes("l l l");
    assert_eq!(pos(&view, cx), (row - 1, 3));
    cx.simulate_keystrokes("h");
    assert_eq!(pos(&view, cx), (row - 1, 2));
    cx.simulate_keystrokes("j");
    assert_eq!(pos(&view, cx).0, row);
    cx.simulate_keystrokes("h h h h");
    assert_eq!(pos(&view, cx).1, 0, "clamped at the left edge");
}

#[gpui::test]
fn quick_select_label_copies_the_match(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateQuickSelect);
    let (label, text) = view.read_with(cx, |t, _| {
        let (l, m) = &t.quick.matches[0];
        (l.clone(), m.text.clone())
    });
    cx.simulate_input(&label);
    assert_eq!(top(&view, cx), None, "a complete label closes quick select");
    assert!(viewport_focused(&view, cx));
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some(text.as_str()));
    assert!(view.read_with(cx, |t, _| t
        .toasts
        .iter()
        .any(|x| x.text.contains("Copied"))));
}

#[gpui::test]
fn search_counts_matches_and_steps(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(Search);
    cx.simulate_input("main");
    let hits = view.read_with(cx, |t, _| t.search_hits().len());
    assert!(hits >= 2, "mock screen has several 'main'");
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |t, _| t.search.current), 1);
    cx.simulate_keystrokes("shift-enter");
    assert_eq!(view.read_with(cx, |t, _| t.search.current), 0);
    cx.simulate_keystrokes("backspace");
    assert_eq!(view.read_with(cx, |t, _| t.search.query.clone()), "mai");
}

#[gpui::test]
fn ssh_overlay_counts_attempts_and_can_close_the_tab(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(DebugShowSshOverlay);
    assert_eq!(top(&view, cx), Some(OverlayKind::SshReconnect));
    assert_eq!(view.read_with(cx, |t, _| t.ssh_attempt), 1);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |t, _| t.ssh_attempt) >= 3,
        "attempts keep counting"
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(top(&view, cx), None);
}

#[gpui::test]
fn toasts_stack_three_and_expire(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    for _ in 0..5 {
        cx.dispatch_action(DebugShowToast);
    }
    assert_eq!(view.read_with(cx, |t, _| t.toasts.len()), 3);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |t, _| t.toasts.len()), 0);
}

#[gpui::test]
fn config_error_banner_shows_and_dismisses(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(DebugShowConfigError);
    assert!(view.read_with(cx, |t, _| t.config_error.is_some()));
    view.update(cx, |t, cx| {
        t.config_error = None;
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |t, _| t.config_error.is_none()));
}

#[gpui::test]
fn registry_chips_match_the_platform_keystrokes(_cx: &mut TestAppContext) {
    for def in actions::ACTIONS {
        if let (Some(chip), Some(first)) = (
            def.chip(Platform::current()),
            def.bindings(Platform::current()).first(),
        ) {
            assert_eq!(chip, format_keystroke(first, Platform::current()));
        }
    }
}

// ------------------------------------------------------------------ configuration (M3)

fn temp_config(name: &str, body: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mt_ui_cfg_{}_{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("microterm.toml");
    std::fs::write(&f, body).unwrap();
    f
}

fn setup_with_config(
    cx: &mut TestAppContext,
    path: std::path::PathBuf,
) -> (Entity<AppWindow>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(crate::settings::Settings::load(path));
        crate::settings::apply_theme(cx);
        crate::settings::install_keymap(cx);
    });
    let (view, cx) = cx.add_window_view(AppWindow::new);
    cx.run_until_parked();
    (view, cx)
}

#[gpui::test]
fn startup_config_is_applied(cx: &mut TestAppContext) {
    let f = temp_config(
        "startup",
        "theme = \"Nord\"\n[window]\npadding = { x = 20, y = 10 }\n",
    );
    let (view, cx) = setup_with_config(cx, f);
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "Nord");
    assert_eq!(view.read_with(cx, |t, _| t.cfg.window.padding.x), 20.0);
}

#[gpui::test]
fn hot_reload_applies_changes_and_keeps_last_valid_config(cx: &mut TestAppContext) {
    use crate::settings::{reload, ReloadOutcome};
    let f = temp_config("reload", "");
    let app = cx.clone();
    let (view, vcx) = setup_with_config(cx, f.clone());
    assert_eq!(view.read_with(vcx, |t, _| t.cfg.font.size), 13.0);

    std::fs::write(
        &f,
        "theme = \"Dracula\"\n[font]\nsize = 16\n[window]\npadding = { x = 30, y = 2 }\n",
    )
    .unwrap();
    let outcome = app.update(reload);
    assert!(matches!(outcome, ReloadOutcome::Applied(_)), "{outcome:?}");
    assert_eq!(app.update(|cx| cx.global::<Theme>().name), "Dracula");
    assert_eq!(view.read_with(vcx, |t, _| t.cfg.font.size), 16.0);
    assert_eq!(
        view.read_with(vcx, |t, _| t.cell.font_size),
        16.0,
        "re-layout re-measures"
    );
    assert!(view.read_with(vcx, |t, _| t.config_error.is_none()));

    // A syntax error keeps everything as it was and shows the banner with line:column.
    std::fs::write(&f, "theme = \"Nord\"\n[font]\nsize = = 1\n").unwrap();
    assert_eq!(app.update(reload), ReloadOutcome::Rejected);
    assert_eq!(app.update(|cx| cx.global::<Theme>().name), "Dracula");
    assert_eq!(view.read_with(vcx, |t, _| t.cfg.font.size), 16.0);
    let banner = view
        .read_with(vcx, |t, _| t.config_error.clone())
        .expect("banner");
    assert!(banner.contains("microterm.toml:3:"), "{banner}");
    assert!(banner.contains("last valid config"), "{banner}");

    // Fixing the file clears the banner.
    std::fs::write(&f, "theme = \"Nord\"\n").unwrap();
    assert!(matches!(app.update(reload), ReloadOutcome::Applied(_)));
    assert_eq!(app.update(|cx| cx.global::<Theme>().name), "Nord");
    assert!(view.read_with(vcx, |t, _| t.config_error.is_none()));
}

#[gpui::test]
fn unknown_key_is_a_warning_banner_but_still_applies(cx: &mut TestAppContext) {
    use crate::settings::reload;
    let f = temp_config("warn", "");
    let app = cx.clone();
    let (view, vcx) = setup_with_config(cx, f.clone());
    std::fs::write(&f, "fnt = 1\n[cursor]\nstyle = \"beam\"\n").unwrap();
    app.update(reload);
    assert_eq!(
        view.read_with(vcx, |t, _| t.cfg.cursor.style),
        mt_config::CursorStyle::Beam
    );
    let banner = view
        .read_with(vcx, |t, _| t.config_error.clone())
        .expect("banner");
    assert!(banner.contains("did you mean \"font\""), "{banner}");
    assert_eq!(
        view.read_with(vcx, |t, _| t.config_banner_kind),
        crate::components::ToastKind::Warning
    );
}

#[gpui::test]
fn reloaded_keybindings_replace_defaults_and_chips(cx: &mut TestAppContext) {
    use crate::settings::{reload, Settings};
    let f = temp_config("keys", "");
    let app = cx.clone();
    let (view, vcx) = setup_with_config(cx, f.clone());
    let palette = actions::ACTIONS
        .iter()
        .find(|a| a.name == "ShowCommandPalette")
        .unwrap();
    let default_chip = app.update(|cx| cx.global::<Settings>().chip(palette));
    assert!(default_chip.is_some());

    std::fs::write(
        &f,
        "[[keys]]\nkey = \"ctrl+alt+k\"\naction = \"ShowCommandPalette\"\n",
    )
    .unwrap();
    app.update(reload);
    let chip = app
        .update(|cx| cx.global::<Settings>().chip(palette))
        .unwrap();
    assert!(
        chip.contains('K') && chip != default_chip.unwrap(),
        "{chip}"
    );
    vcx.simulate_keystrokes("ctrl-alt-k");
    assert_eq!(top(&view, vcx), Some(OverlayKind::CommandPalette));
    vcx.simulate_keystrokes("escape");

    // Disabling the original binding frees the keystroke.
    let original = first_binding("ShowCommandPalette");
    std::fs::write(
        &f,
        format!(
            "[[keys]]\nkey = \"{}\"\naction = \"Disable\"\n",
            original.replace('-', "+")
        ),
    )
    .unwrap();
    app.update(reload);
    vcx.simulate_keystrokes(original);
    assert_eq!(
        top(&view, vcx),
        None,
        "disabled default must not open the palette"
    );
}

#[gpui::test]
fn theme_selector_lists_every_bundled_theme(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SelectTheme);
    let count = view.read_with(cx, |t, _| {
        t.picker.as_ref().map_or(0, |p| p.state.matches().len())
    });
    assert_eq!(count, 53, "every bundled theme is browsable");
    cx.update(|_, cx| {
        assert_eq!(cx.global::<crate::settings::Settings>().registry.len(), 53);
    });
}
