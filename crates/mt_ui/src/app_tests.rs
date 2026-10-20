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

fn tab_ids(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<u64> {
    view.read_with(cx, |t, _| t.tabs.iter().map(|t| t.id).collect())
}

#[gpui::test]
fn tabs_can_be_added_closed_and_switched(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), 1);
    cx.dispatch_action(SpawnTab);
    cx.dispatch_action(SpawnTab);
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
fn new_tab_opens_after_the_active_tab(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    cx.dispatch_action(SpawnTab);
    let before = tab_ids(&view, cx);
    cx.dispatch_action(ActivateTab(0));
    cx.dispatch_action(SpawnTab);
    let after = tab_ids(&view, cx);
    assert_eq!(after.len(), 4);
    assert_eq!(after[0], before[0]);
    assert_eq!(&after[2..], &before[1..]);
    assert_eq!(view.read_with(cx, |t, _| t.active_tab), 1);
}

#[gpui::test]
fn tabs_move_and_keep_the_active_one(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    cx.dispatch_action(SpawnTab);
    let ids = tab_ids(&view, cx);
    cx.dispatch_action(ActivateTab(0));
    cx.dispatch_action(MoveTabRelative(1));
    assert_eq!(tab_ids(&view, cx), vec![ids[1], ids[0], ids[2]]);
    assert_eq!(view.read_with(cx, |t, _| t.active_tab), 1);
    cx.dispatch_action(MoveTabRelative(-1));
    assert_eq!(tab_ids(&view, cx), ids);
    // Drag-to-reorder: drop the last tab onto the first.
    view.update_in(cx, |this, window, cx| {
        this.move_tab_onto(ids[2], ids[0], window, cx)
    });
    assert_eq!(tab_ids(&view, cx), vec![ids[2], ids[0], ids[1]]);
}

#[gpui::test]
fn tabs_are_renamed_inline(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(RenameTab);
    assert!(view.read_with(cx, |t, _| t.renaming.is_some()));
    // Clear the prefilled text, type a name, commit.
    for _ in 0..16 {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("api");
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |t, _| t.renaming.is_none()));
    assert_eq!(view.read_with(cx, |t, _| t.tabs[0].title.clone()), "api");
    // Escape cancels without changing the title.
    cx.dispatch_action(RenameTab);
    cx.simulate_input("zzz");
    cx.simulate_keystrokes("escape");
    assert_eq!(view.read_with(cx, |t, _| t.tabs[0].title.clone()), "api");
}

#[gpui::test]
fn closing_the_last_tab_closes_the_window(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(Theme::one_dark());
        let bindings = actions::key_bindings(Platform::current(), cx);
        cx.bind_keys(bindings);
    });
    let (view, window) = cx.add_window_view(AppWindow::new);
    window.run_until_parked();
    window.dispatch_action(CloseTab);
    // The test's own handle would keep the view (and so the mux window) alive.
    drop(view);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    cx.update(|cx| {
        let host = cx.global::<crate::mux_host::MuxHost>();
        assert!(host.mux.windows().is_empty(), "the mux window is gone too");
    });
}

#[gpui::test]
fn confirm_close_always_asks_first(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    view.update(cx, |this, _| {
        let mut c = (*this.cfg).clone();
        c.confirm_close = mt_config::ConfirmClose::Always;
        this.cfg = std::sync::Arc::new(c);
    });
    cx.dispatch_action(CloseTab);
    assert_eq!(top(&view, cx), Some(OverlayKind::CloseConfirm));
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), 2);
    cx.simulate_keystrokes("escape");
    assert_eq!(
        view.read_with(cx, |t, _| t.tabs.len()),
        2,
        "cancel keeps the tab"
    );
    cx.dispatch_action(CloseTab);
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), 1);
    assert_eq!(top(&view, cx), None);
}

#[gpui::test]
fn confirm_close_never_closes_immediately(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    view.update(cx, |this, _| {
        let mut c = (*this.cfg).clone();
        c.confirm_close = mt_config::ConfirmClose::Never;
        this.cfg = std::sync::Arc::new(c);
    });
    cx.dispatch_action(CloseTab);
    assert_eq!(top(&view, cx), None);
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), 1);
}

#[gpui::test]
fn workspace_prompt_creates_and_switches(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(CreateWorkspace);
    assert_eq!(top(&view, cx), Some(OverlayKind::NamePrompt));
    cx.simulate_input("api");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(top(&view, cx), None);
    cx.update(|_, cx| {
        let host = cx.global::<crate::mux_host::MuxHost>();
        assert_eq!(host.mux.active_workspace().name, "api");
    });
}

#[gpui::test]
fn workspace_switcher_lists_real_workspaces(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.update(|_, cx| {
        crate::mux_host::host(cx)
            .mux
            .create_workspace("infra")
            .unwrap();
    });
    cx.dispatch_action(ShowWorkspaceSwitcher);
    let labels: Vec<String> = view.read_with(cx, |t, _| {
        let picker = t.picker.as_ref().unwrap();
        picker
            .state
            .visible()
            .1
            .iter()
            .map(|m| picker.state.label(m).to_string())
            .collect()
    });
    assert_eq!(labels, ["default", "infra", "Create workspace…"]);
}

#[gpui::test]
fn copy_mode_moves_selects_and_yanks(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateCopyMode);
    let cursor = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.read_with(cx, |t, _| t.copy.mode.as_ref().unwrap().cursor)
    };
    let (row, col) = cursor(&view, cx);
    assert_eq!(col, 0);
    cx.simulate_keystrokes("k");
    assert_eq!(cursor(&view, cx), (row - 1, 0));
    cx.simulate_keystrokes("l l l");
    assert_eq!(cursor(&view, cx), (row - 1, 3));
    cx.simulate_keystrokes("h");
    assert_eq!(cursor(&view, cx), (row - 1, 2));
    cx.simulate_keystrokes("j");
    assert_eq!(cursor(&view, cx).0, row);
    cx.simulate_keystrokes("h h h h");
    assert_eq!(cursor(&view, cx).1, 0, "clamped at the left edge");
    cx.simulate_keystrokes("2 k");
    assert_eq!(cursor(&view, cx).0, row - 2, "numeric prefix");

    // V y yanks the whole row and leaves copy mode.
    let line = view.read_with(cx, |t, _| t.screen()[row - 2].text());
    cx.simulate_keystrokes("shift-v");
    assert_eq!(
        view.read_with(cx, |t, _| t.copy_pill().unwrap().0),
        "V-LINE"
    );
    cx.simulate_keystrokes("y");
    assert_eq!(top(&view, cx), None);
    assert!(viewport_focused(&view, cx));
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some(line.trim_end()));
}

#[gpui::test]
fn copy_mode_escape_clears_selection_before_exiting(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateCopyMode);
    cx.simulate_keystrokes("v");
    cx.simulate_keystrokes("escape");
    assert_eq!(top(&view, cx), Some(OverlayKind::CopyMode));
    assert!(view.read_with(cx, |t, _| t.copy.mode.as_ref().unwrap().sel.is_none()));
    cx.simulate_keystrokes("escape");
    assert_eq!(top(&view, cx), None);
    cx.dispatch_action(ActivateCopyMode);
    cx.simulate_keystrokes("q");
    assert_eq!(top(&view, cx), None, "q exits");
}

#[gpui::test]
fn copy_mode_slash_prompt_shows_in_the_pill_and_searches(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateCopyMode);
    cx.simulate_keystrokes("g g");
    cx.simulate_keystrokes("/");
    cx.simulate_input("main");
    let prompt = view.read_with(cx, |t, _| t.copy_pill().unwrap().1);
    assert_eq!(prompt.as_deref(), Some("/main"));
    cx.simulate_keystrokes("enter");
    let (cursor, line) = view.read_with(cx, |t, _| {
        let c = t.copy.mode.as_ref().unwrap().cursor;
        (c, t.screen()[c.0].text())
    });
    assert!(
        line[cursor.1..].starts_with("main"),
        "{line:?} at {cursor:?}"
    );
}

#[gpui::test]
fn quick_select_label_copies_the_match(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateQuickSelect);
    let (label, text) = view.read_with(cx, |t, _| {
        let h = &t.quick.hits[0];
        (h.label.clone(), h.text.clone())
    });
    cx.simulate_input(&label);
    assert_eq!(top(&view, cx), None, "a complete label closes quick select");
    assert!(viewport_focused(&view, cx));
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some(text.as_str()));
    assert!(view.read_with(cx, |t, _| t
        .toasts
        .iter()
        .any(|x| x.text.as_ref() == format!("Copied {text}"))));
}

#[gpui::test]
fn quick_select_labels_come_from_the_configured_alphabet_and_esc_cancels(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ActivateQuickSelect);
    let alphabet = view.read_with(cx, |t, _| t.cfg.quick_select.alphabet.clone());
    view.read_with(cx, |t, _| {
        assert!(
            !t.quick.hits.is_empty(),
            "the mock screen has hashes and IPs"
        );
        for h in &t.quick.hits {
            assert!(h.label.chars().all(|c| alphabet.contains(c)), "{}", h.label);
        }
    });
    // A letter that starts no label is ignored.
    cx.simulate_keystrokes("escape");
    assert_eq!(top(&view, cx), None);
    assert!(cx.read_from_clipboard().and_then(|c| c.text()).is_none());
}

#[gpui::test]
fn search_counts_matches_and_steps(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(Search);
    cx.simulate_input("main");
    let hits = view.read_with(cx, |t, _| t.search.matches.len());
    assert!(hits >= 2, "mock screen has several 'main'");
    let status = view.read_with(cx, |t, _| t.search_status());
    assert_eq!(status, format!("1 of {hits}"));
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |t, _| t.search.current), 1);
    cx.simulate_keystrokes("shift-enter");
    assert_eq!(view.read_with(cx, |t, _| t.search.current), 0);
    cx.simulate_keystrokes("shift-enter");
    assert_eq!(
        view.read_with(cx, |t, _| t.search.current),
        hits - 1,
        "wraps backwards"
    );
    cx.simulate_keystrokes("backspace");
    assert_eq!(view.read_with(cx, |t, _| t.search.query.clone()), "mai");
}

#[gpui::test]
fn search_toggles_change_the_results(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(Search);
    cx.simulate_input("MAIN");
    let insensitive = view.read_with(cx, |t, _| t.search.matches.len());
    assert!(insensitive > 0);
    view.update(cx, |t, cx| t.toggle_search_case(cx));
    assert_eq!(view.read_with(cx, |t, _| t.search.matches.len()), 0);
    assert_eq!(view.read_with(cx, |t, _| t.search_status()), "no results");
    view.update(cx, |t, cx| t.toggle_search_case(cx));
    view.update(cx, |t, cx| t.toggle_search_regex(cx));
    cx.simulate_keystrokes("backspace backspace backspace backspace");
    cx.simulate_input("(");
    assert_eq!(
        view.read_with(cx, |t, _| t.search_status()),
        "invalid regex"
    );
    cx.simulate_keystrokes("backspace");
    cx.simulate_input("ma.n");
    assert!(
        view.read_with(cx, |t, _| t.search.matches.len()) > 0,
        "regex mode"
    );
    // Marks cover the matched cells.
    assert!(view.read_with(cx, |t, _| !t.search_marks().is_empty()));
}

/// Registers an SSH domain and opens a tab in it (tests have no terminals or connections).
fn open_ssh_tab(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> mt_mux::DomainId {
    view.update_in(cx, |this, window, cx| {
        let domain = crate::mux_host::host(cx).mux.register_ssh_domain("prod");
        let spec = mt_mux::SpawnSpec {
            domain,
            ..Default::default()
        };
        this.spawn_tab(Some(spec), window, cx);
        domain
    })
}

fn set_domain(cx: &mut VisualTestContext, domain: mt_mux::DomainId, s: mt_mux::DomainState) {
    cx.update(|_, cx| {
        crate::mux_host::host(cx)
            .mux
            .set_domain_state(domain, s)
            .unwrap();
    });
}

fn card_state(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Option<mt_mux::DomainState> {
    view.update(cx, |this, cx| this.ssh_card(cx).map(|c| c.state))
}

#[gpui::test]
fn reconnect_card_follows_the_active_tabs_domain_state(cx: &mut TestAppContext) {
    use mt_mux::DomainState as S;
    let (view, cx) = setup(cx);
    assert_eq!(card_state(&view, cx), None, "a local tab has no card");
    let domain = open_ssh_tab(&view, cx);
    assert_eq!(card_state(&view, cx), Some(S::Connecting));
    assert!(
        view.update(cx, |t, cx| t.ssh_blocks_input(cx)),
        "connecting blocks keys"
    );

    set_domain(cx, domain, S::Connected);
    assert!(!view.update(cx, |t, cx| t.ssh_blocks_input(cx)));

    set_domain(
        cx,
        domain,
        S::Reconnecting {
            attempt: 3,
            next_in_secs: 4,
        },
    );
    assert_eq!(
        card_state(&view, cx),
        Some(S::Reconnecting {
            attempt: 3,
            next_in_secs: 4
        })
    );
    assert!(view.update(cx, |t, cx| t.ssh_blocks_input(cx)));

    set_domain(
        cx,
        domain,
        S::Disconnected {
            reason: "host key changed".into(),
        },
    );
    let card = view.update(cx, |t, cx| t.ssh_card(cx)).unwrap();
    assert_eq!((card.name.as_str(), card.domain), ("prod", domain));

    // The first tab is local again: the card goes away with the switch.
    view.update(cx, |t, cx| t.activate_tab(0, cx));
    assert_eq!(card_state(&view, cx), None);
    assert!(!view.update(cx, |t, cx| t.ssh_blocks_input(cx)));
}

#[gpui::test]
fn closing_the_tab_from_the_reconnect_card_removes_it(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let domain = open_ssh_tab(&view, cx);
    set_domain(
        cx,
        domain,
        mt_mux::DomainState::Reconnecting {
            attempt: 1,
            next_in_secs: 1,
        },
    );
    let before = view.read_with(cx, |t, _| t.tabs.len());
    view.update_in(cx, |t, w, cx| t.close_ssh_tab(w, cx));
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), before - 1);
    assert_eq!(card_state(&view, cx), None);
}

#[gpui::test]
fn host_key_prompt_is_answered_with_enter_and_cancelled_with_escape(cx: &mut TestAppContext) {
    use mt_ssh::prompt::{Answered, HostKeyPrompt, PromptKind, PromptRequest};
    let (view, cx) = setup(cx);
    let hk = || {
        PromptKind::HostKey(HostKeyPrompt {
            host: "prod.example.com".into(),
            port: 22,
            fingerprint: "SHA256:abc".into(),
            key_type: "ssh-ed25519".into(),
        })
    };
    let (req, mut a1) = PromptRequest::for_test("prod", hk());
    let (req2, mut a2) = PromptRequest::for_test("prod", hk());
    view.update_in(cx, |t, w, cx| {
        t.enqueue_ssh_prompt(req, w, cx);
        t.enqueue_ssh_prompt(req2, w, cx);
    });
    assert_eq!(top(&view, cx), Some(OverlayKind::SshPrompt));
    assert_eq!(a1.poll(), Answered::Pending);
    cx.simulate_keystrokes("enter");
    assert_eq!(a1.poll(), Answered::Confirm(true));
    // The second question is now on screen.
    assert_eq!(top(&view, cx), Some(OverlayKind::SshPrompt));
    cx.simulate_keystrokes("escape");
    assert_eq!(a2.poll(), Answered::Cancelled, "Esc refuses");
    assert_eq!(top(&view, cx), None);
    assert!(viewport_focused(&view, cx));
}

#[gpui::test]
fn password_prompt_collects_typed_text_and_never_echoes_it(cx: &mut TestAppContext) {
    use mt_ssh::prompt::{Answered, PromptKind, PromptRequest, SecretPrompt};
    let (view, cx) = setup(cx);
    let (req, mut answer) = PromptRequest::for_test(
        "prod",
        PromptKind::Secret(SecretPrompt::Password {
            user: "bruce".into(),
            host: "prod".into(),
        }),
    );
    view.update_in(cx, |t, w, cx| t.enqueue_ssh_prompt(req, w, cx));
    cx.simulate_input("hunter2");
    cx.simulate_keystrokes("backspace");
    cx.simulate_keystrokes("enter");
    assert_eq!(answer.poll(), Answered::Secret(Some("hunter".into())));
    assert_eq!(top(&view, cx), None);
    assert!(
        view.read_with(cx, |t, _| t.ssh_input.is_empty()),
        "nothing is kept"
    );
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

#[gpui::test]
fn font_size_actions_resize_the_cell_and_reset(cx: &mut TestAppContext) {
    let f = temp_config("fontsize", "");
    let (view, cx) = setup_with_config(cx, f);
    let size = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |t, _| t.cell.font_size);
    let width = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |t, _| t.cell.width);
    assert_eq!(size(cx), 13.0);
    let w0 = width(cx);
    cx.dispatch_action(IncreaseFontSize);
    cx.dispatch_action(IncreaseFontSize);
    assert_eq!(size(cx), 15.0);
    assert!(width(cx) > w0, "wider cells at a larger size");
    cx.dispatch_action(DecreaseFontSize);
    assert_eq!(size(cx), 14.0);
    cx.dispatch_action(ResetFontSize);
    assert_eq!(size(cx), 13.0);
    assert_eq!(width(cx), w0);
    assert_eq!(
        view.read_with(cx, |t, _| t.cfg.font.size),
        13.0,
        "config untouched"
    );
}

#[gpui::test]
fn config_font_size_edit_replaces_window_zoom(cx: &mut TestAppContext) {
    use crate::settings::reload;
    let f = temp_config("fontzoom", "");
    let app = cx.clone();
    let (view, vcx) = setup_with_config(cx, f.clone());
    vcx.dispatch_action(IncreaseFontSize);
    assert_eq!(view.read_with(vcx, |t, _| t.cell.font_size), 14.0);
    std::fs::write(&f, "[font]\nsize = 18\n").unwrap();
    app.update(reload);
    assert_eq!(view.read_with(vcx, |t, _| t.cell.font_size), 18.0);
}

// ------------------------------------------------------------------ M6: palette and launcher

fn picker_labels(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |t, _| {
        let p = t.picker.as_ref().unwrap();
        p.state
            .matches()
            .iter()
            .map(|m| p.state.label(m).to_string())
            .collect()
    })
}

#[gpui::test]
fn every_palette_action_is_reachable_by_its_label(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ShowCommandPalette);
    for (i, def) in actions::ACTIONS
        .iter()
        .enumerate()
        .filter(|(_, d)| d.palette)
    {
        view.update(cx, |t, _| {
            t.picker.as_mut().unwrap().state.set_query(def.label)
        });
        let found = view.read_with(cx, |t, _| {
            let p = t.picker.as_ref().unwrap();
            p.state.matches().iter().any(
                |m| matches!(p.state.item(m.index).target, crate::app::Target::Action(a) if a == i),
            )
        });
        assert!(found, "{} not reachable from the palette", def.name);
    }
}

#[gpui::test]
fn palette_lists_actions_by_default_and_hides_argument_actions(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ShowCommandPalette);
    let labels = picker_labels(&view, cx);
    assert!(labels.contains(&"New tab".to_string()));
    assert!(
        !labels.iter().any(|l| l.starts_with("Theme: ")),
        "{labels:?}"
    );
    assert!(!labels.iter().any(|l| l == "Send string"));
}

#[gpui::test]
fn at_prefix_jumps_to_a_workspace(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.update(|_, cx| {
        crate::mux_host::host(cx)
            .mux
            .create_workspace("infra")
            .unwrap();
    });
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("@infra");
    assert_eq!(
        picker_labels(&view, cx),
        ["Switch to workspace: infra".to_string()]
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::mux_host::MuxHost>()
                .mux
                .active_workspace()
                .name,
            "infra"
        );
    });
}

#[gpui::test]
fn hash_prefix_lists_and_activates_tabs(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    cx.run_until_parked();
    view.update_in(cx, |t, _, cx| t.activate_tab(0, cx));
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("#");
    let labels = picker_labels(&view, cx);
    assert_eq!(labels.len(), 2);
    assert!(labels.iter().all(|l| l.starts_with("Go to tab: ")));
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |t, _| t.active_tab), 1);
}

#[gpui::test]
fn tilde_prefix_previews_a_theme_and_escape_reverts(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("~tokyo");
    let first = picker_labels(&view, cx).first().cloned().unwrap();
    assert!(first.starts_with("Theme: Tokyo Night"), "{first}");
    cx.simulate_keystrokes("down up");
    let name = cx.update(|_, cx| cx.global::<Theme>().name);
    assert!(name.starts_with("Tokyo Night"), "previewed {name}");
    cx.simulate_keystrokes("escape");
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "One Dark");
    assert_eq!(top(&view, cx), None);
}

#[gpui::test]
fn tilde_enter_keeps_the_theme_but_other_rows_revert_a_preview(cx: &mut TestAppContext) {
    let (_view, cx) = setup(cx);
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("~nord");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "Nord");

    // Preview a theme, then drop the prefix and run an action: the preview must not stick.
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("~dracula");
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "Dracula");
    for _ in 0..8 {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("new tab");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.global::<Theme>().name), "Nord");
}

#[gpui::test]
fn launcher_has_no_mock_rows_and_lists_real_ones(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ShowLauncher);
    let labels = picker_labels(&view, cx);
    assert_eq!(
        labels,
        ["New tab in local", "Connect to host…", "Workspace: default"]
    );
}

#[gpui::test]
fn launcher_lists_ssh_domains_with_status_dots(cx: &mut TestAppContext) {
    let f = temp_config(
        "sshdomains",
        "[[ssh_domains]]\nname = \"prod\"\nhost = \"prod.example.com\"\n",
    );
    let (view, cx) = setup_with_config(cx, f);
    cx.dispatch_action(ShowLauncher);
    assert!(picker_labels(&view, cx).contains(&"New tab in prod".to_string()));
    // Not connected yet (never registered): idle; once registered and connected: green.
    let dot = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.update(cx, |t, cx| t.domain_status("prod", cx))
    };
    assert_eq!(dot(&view, cx), crate::components::StatusKind::Idle);
    let id = cx.update(|_, cx| crate::mux_host::host(cx).mux.register_ssh_domain("prod"));
    assert_eq!(
        dot(&view, cx),
        crate::components::StatusKind::Reconnecting,
        "connecting"
    );
    set_domain(cx, id, mt_mux::DomainState::Connected);
    assert_eq!(dot(&view, cx), crate::components::StatusKind::Connected);
    set_domain(
        cx,
        id,
        mt_mux::DomainState::Disconnected { reason: "x".into() },
    );
    assert_eq!(dot(&view, cx), crate::components::StatusKind::Error);
}

#[gpui::test]
fn launch_profiles_appear_after_a_hot_reload_even_while_open(cx: &mut TestAppContext) {
    use crate::settings::reload;
    let f = temp_config("launch", "");
    let app = cx.clone();
    let (view, vcx) = setup_with_config(cx, f.clone());
    vcx.dispatch_action(ShowLauncher);
    assert!(!picker_labels(&view, vcx).contains(&"htop".to_string()));

    std::fs::write(
        &f,
        "[[launch]]\nname = \"htop\"\ncommand = [\"htop\"]\ncwd = \"~\"\n",
    )
    .unwrap();
    app.update(reload);
    vcx.run_until_parked();
    let labels = picker_labels(&view, vcx);
    assert_eq!(labels[0], "htop", "{labels:?}");

    // A closed-and-reopened launcher reads the same config.
    vcx.simulate_keystrokes("escape");
    vcx.dispatch_action(ShowLauncher);
    assert_eq!(picker_labels(&view, vcx)[0], "htop");
}

#[gpui::test]
fn confirming_a_launch_profile_opens_a_tab_running_its_command(cx: &mut TestAppContext) {
    let f = temp_config(
        "launch-run",
        "[[launch]]\nname = \"sleeper\"\ncommand = [\"sleep\", \"5\"]\n",
    );
    let (view, cx) = setup_with_config(cx, f);
    let before = view.read_with(cx, |t, _| t.tabs.len());
    cx.dispatch_action(ShowLauncher);
    cx.simulate_input("sleeper");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), before + 1);
    assert_eq!(top(&view, cx), None);
}

#[gpui::test]
fn recently_run_items_rank_first(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("rename");
    let first = picker_labels(&view, cx)[0].clone();
    let second = picker_labels(&view, cx)[1].clone();
    // Run the runner-up, then the same query must rank it first.
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("rename");
    let labels = picker_labels(&view, cx);
    assert_eq!(labels[0], second, "was {first} before");
}

#[gpui::test]
fn quick_select_can_be_turned_off(cx: &mut TestAppContext) {
    let f = temp_config("qs_off", "[quick_select]\nenabled = false\n");
    let (view, cx) = setup_with_config(cx, f);
    cx.dispatch_action(ActivateQuickSelect);
    assert_eq!(top(&view, cx), None, "the action does nothing");
    assert!(viewport_focused(&view, cx));
    assert!(view.read_with(cx, |t, _| t
        .toasts
        .iter()
        .any(|x| x.text.contains("turned off"))));
    cx.dispatch_action(ShowCommandPalette);
    cx.simulate_input("quick select");
    let labels: Vec<String> = view.read_with(cx, |t, _| {
        let p = t.picker.as_ref().unwrap();
        p.state
            .visible()
            .1
            .iter()
            .map(|m| p.state.label(m).to_string())
            .collect()
    });
    assert!(
        !labels
            .iter()
            .any(|l| l.to_lowercase().contains("quick select")),
        "{labels:?}"
    );
    // Copy mode and search are unaffected.
    cx.simulate_keystrokes("escape");
    cx.dispatch_action(Search);
    assert_eq!(top(&view, cx), Some(OverlayKind::Search));
}

#[gpui::test]
fn switching_tabs_ends_copy_mode_and_refreshes_search(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    cx.dispatch_action(ActivateCopyMode);
    cx.simulate_keystrokes("v");
    view.update(cx, |t, cx| t.activate_tab(0, cx));
    cx.run_until_parked();
    assert_eq!(top(&view, cx), None, "copy mode ends when the tab changes");
    assert!(view.read_with(cx, |t, _| t.copy.mode.is_none()));
    assert!(viewport_focused(&view, cx));

    cx.dispatch_action(Search);
    cx.simulate_input("main");
    let fresh = view.read_with(cx, |t, _| t.search.matches.len());
    view.update(cx, |t, _| {
        let stale = t.search.matches[0].clone();
        t.search.matches.push(stale);
    });
    view.update(cx, |t, cx| t.activate_tab(1, cx));
    assert_eq!(
        top(&view, cx),
        Some(OverlayKind::Search),
        "the find bar stays"
    );
    assert_eq!(
        view.read_with(cx, |t, _| t.search.matches.len()),
        fresh,
        "matches were recomputed for the new tab"
    );
}

fn pane_of(view: &Entity<AppWindow>, cx: &mut VisualTestContext, tab: usize) -> u64 {
    view.read_with(cx, |t, _| t.tabs[tab].pane)
}

#[gpui::test]
fn focusing_a_tab_clears_its_attention_badge(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    cx.run_until_parked();
    let (background, shown) = (pane_of(&view, cx, 0), pane_of(&view, cx, 1));
    cx.update(|_, cx| {
        let mux = &mut crate::mux_host::host(cx).mux;
        mux.raise_attention(background, mt_proto::NotifyLevel::Attention);
        mux.raise_attention(shown, mt_proto::NotifyLevel::Error);
    });
    // Clicking (activating) the badged tab clears that badge only.
    view.update_in(cx, |t, _, cx| t.activate_tab(0, cx));
    cx.update(|_, cx| {
        assert_eq!(crate::mux_host::tab_chrome(cx, background).badge, None);
        assert!(crate::mux_host::tab_chrome(cx, shown).badge.is_some());
    });
    view.update_in(cx, |t, _, cx| t.activate_tab(1, cx));
    cx.update(|_, cx| assert_eq!(crate::mux_host::tab_chrome(cx, shown).badge, None));
}

#[gpui::test]
fn the_tab_palette_shows_each_tabs_status(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SpawnTab);
    cx.run_until_parked();
    let pane = pane_of(&view, cx, 0);
    cx.update(|_, cx| {
        crate::mux_host::host(cx).mux.set_pane_status(
            pane,
            mt_proto::StatusState::Waiting,
            "approve?",
            None,
            None,
            120,
            0,
        );
    });
    let entries = view.update(cx, |t, cx| t.palette_entries(cx));
    let tab = entries
        .iter()
        .find(|(l, _)| l.starts_with("Go to tab:"))
        .map(|(_, e)| e.clone())
        .expect("a tab entry");
    assert_eq!(tab.detail.as_deref(), Some("tab 1 · waiting: approve?"));
    assert_eq!(tab.status, Some(crate::components::StatusKind::Warning));
}

// ------------------------------------------------------------------------ splits (M11)

fn pane_ids(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<u64> {
    view.read_with(cx, |t, _| {
        t.tabs[t.active_tab].panes.iter().map(|p| p.pane).collect()
    })
}

fn focused(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> u64 {
    view.read_with(cx, |t, _| t.tabs[t.active_tab].pane)
}

#[gpui::test]
fn split_actions_add_panes_and_focus_follows_the_new_one(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let first = focused(&view, cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    let ids = pane_ids(&view, cx);
    assert_eq!(ids.len(), 2);
    let second = focused(&view, cx);
    assert_ne!(first, second);
    assert_eq!(
        view.read_with(cx, |t, _| t.tabs.len()),
        1,
        "a split is not a tab"
    );

    cx.dispatch_action(SplitVertical);
    cx.run_until_parked();
    assert_eq!(pane_ids(&view, cx).len(), 3);
}

#[gpui::test]
fn the_default_split_keys_are_bound(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let key = format_keystroke(first_binding("SplitHorizontal"), Platform::current());
    assert!(!key.is_empty());
    cx.simulate_keystrokes(first_binding("SplitHorizontal"));
    cx.run_until_parked();
    assert_eq!(pane_ids(&view, cx).len(), 2);
}

#[gpui::test]
fn pane_direction_actions_move_focus_and_stop_at_edges(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let left = focused(&view, cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    let right = focused(&view, cx);
    cx.dispatch_action(ActivatePaneDirection(mt_mux::Dir::Left));
    cx.run_until_parked();
    assert_eq!(focused(&view, cx), left);
    cx.dispatch_action(ActivatePaneDirection(mt_mux::Dir::Left));
    assert_eq!(focused(&view, cx), left, "no wrap-around");
    cx.dispatch_action(ActivatePaneDirection(mt_mux::Dir::Right));
    assert_eq!(focused(&view, cx), right);
}

#[gpui::test]
fn closing_panes_keeps_the_tab_until_the_last_one(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    cx.update(|_, cx| {
        // No process runs in tests, so confirm_close = "process" does not prompt.
        let _ = cx;
    });
    cx.dispatch_action(ClosePane);
    cx.run_until_parked();
    assert_eq!(pane_ids(&view, cx).len(), 1);
    assert_eq!(view.read_with(cx, |t, _| t.tabs.len()), 1);
}

#[gpui::test]
fn zoom_swap_and_rotate_update_the_view(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let a = focused(&view, cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    let b = focused(&view, cx);
    cx.dispatch_action(TogglePaneZoom);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |t, _| t.tabs[0].zoomed), Some(b));
    cx.dispatch_action(TogglePaneZoom);
    assert_eq!(view.read_with(cx, |t, _| t.tabs[0].zoomed), None);

    let order = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| -> Vec<u64> {
        view.read_with(cx, |t, _| {
            t.tabs[0].tree.panes().iter().map(|p| p.id).collect()
        })
    };
    assert_eq!(order(&view, cx), vec![a, b]);
    cx.dispatch_action(SwapPane(mt_mux::Dir::Left));
    assert_eq!(order(&view, cx), vec![b, a]);
    cx.dispatch_action(RotatePanes);
    assert_eq!(order(&view, cx), vec![a, b]);
    cx.dispatch_action(ResizePane(mt_mux::Dir::Left, 5));
}

#[gpui::test]
fn hovering_a_pane_focuses_it_only_when_enabled(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let a = focused(&view, cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    // Off by default: hovering changes nothing.
    view.update_in(cx, |this, window, cx| this.hover_pane(a, window, cx));
    assert_ne!(focused(&view, cx), a);

    let mut config = mt_config::Config::default();
    config.mouse.focus_follows_mouse = true;
    cx.update(|_, cx| {
        let mut s = crate::settings::Settings::defaults();
        s.config = std::sync::Arc::new(config);
        cx.set_global(s);
    });
    view.update_in(cx, |this, window, cx| this.hover_pane(a, window, cx));
    cx.run_until_parked();
    assert_eq!(focused(&view, cx), a, "delay 0 focuses at once");
}

#[gpui::test]
fn a_hover_delay_waits_and_is_cancelled_by_moving_back(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let a = focused(&view, cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    let b = focused(&view, cx);
    let mut config = mt_config::Config::default();
    config.mouse.focus_follows_mouse = true;
    config.panes.focus_delay_ms = 200;
    cx.update(|_, cx| {
        let mut s = crate::settings::Settings::defaults();
        s.config = std::sync::Arc::new(config.clone());
        cx.set_global(s);
    });
    view.update_in(cx, |this, _, _| this.cfg = std::sync::Arc::new(config));
    view.update_in(cx, |this, window, cx| this.hover_pane(a, window, cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(focused(&view, cx), b, "still waiting");
    // The pointer returns to the focused pane before the delay ends.
    view.update_in(cx, |this, window, cx| this.hover_pane(b, window, cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(focused(&view, cx), b);

    view.update_in(cx, |this, window, cx| this.hover_pane(a, window, cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(focused(&view, cx), a, "the delay elapsed");
}

#[gpui::test]
fn overlays_block_focus_follows_mouse(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let a = focused(&view, cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    let b = focused(&view, cx);
    let mut config = mt_config::Config::default();
    config.mouse.focus_follows_mouse = true;
    cx.update(|_, cx| {
        let mut s = crate::settings::Settings::defaults();
        s.config = std::sync::Arc::new(config);
        cx.set_global(s);
    });
    cx.dispatch_action(ShowCommandPalette);
    view.update_in(cx, |this, window, cx| this.hover_pane(a, window, cx));
    assert_eq!(focused(&view, cx), b);
}

#[gpui::test]
fn divider_drag_changes_the_ratio(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    view.update_in(cx, |this, window, cx| {
        this.pane_area.set((0.0, 0.0, 1000.0, 600.0));
        this.begin_divider_drag(
            vec![],
            mt_mux::Axis::Horizontal,
            mt_mux::Rect::new(0.0, 0.0, 1.0, 1.0),
        );
        this.drag_divider_to((250.0, 100.0), window, cx);
        this.end_divider_drag(cx);
    });
    let ratio = view.read_with(cx, |t, _| match &t.tabs[0].tree {
        mt_mux::PaneTree::Split { ratio, .. } => *ratio,
        _ => panic!("split"),
    });
    assert!((ratio - 0.25).abs() < 1e-4);
    assert!(view.read_with(cx, |t, _| t.divider_drag.is_none()));
}
