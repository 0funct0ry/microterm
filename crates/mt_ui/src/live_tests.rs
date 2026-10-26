//! Live-terminal behaviour: a real `/bin/sh` on a PTY behind the first tab.

use std::time::{Duration, Instant};

use gpui::{ClipboardItem, Entity, TestAppContext, VisualTestContext};
use mt_term::SpawnOptions;

use crate::{
    actions::{self, *},
    app::AppWindow,
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

/// Polls (letting the PTY thread run) until a row satisfies `pred`.
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

fn write(view: &Entity<AppWindow>, cx: &mut VisualTestContext, text: &str) {
    let text = text.to_string();
    view.read_with(cx, |this, _| {
        this.live().unwrap().term.write(text.into_bytes())
    });
}

#[gpui::test]
fn first_tab_runs_a_shell_and_typed_keys_reach_it(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.simulate_input("echo live-$((6*7))");
    cx.simulate_keystrokes("enter");
    let rows = wait_for(&view, cx, |r| r == "live-42");
    assert!(rows.iter().any(|r| r == "live-42"), "{rows:?}");
}

#[gpui::test]
fn ctrl_c_interrupts_a_running_command(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.simulate_input("sleep 30");
    cx.simulate_keystrokes("enter");
    std::thread::sleep(Duration::from_millis(200));
    cx.simulate_keystrokes("ctrl-c");
    write(&view, cx, "echo back\n");
    let rows = wait_for(&view, cx, |r| r == "back");
    assert!(rows.iter().any(|r| r == "back"), "{rows:?}");
}

#[gpui::test]
fn osc_title_updates_the_tab_title(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "printf '\\033]0;my-title\\007'\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(30));
        cx.run_until_parked();
        let title = view.read_with(cx, |this, _| this.tabs[0].title.clone());
        if title == "my-title" || Instant::now() > deadline {
            assert_eq!(title, "my-title");
            break;
        }
    }
}

#[gpui::test]
fn single_line_paste_goes_straight_to_the_shell(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("echo pasted-ok".into()));
    cx.dispatch_action(Paste);
    assert_eq!(
        view.read_with(cx, |t, _| t.overlays.top()),
        None,
        "no confirmation for one line"
    );
    cx.simulate_keystrokes("enter");
    let rows = wait_for(&view, cx, |r| r == "pasted-ok");
    assert!(rows.iter().any(|r| r == "pasted-ok"), "{rows:?}");
}

#[gpui::test]
fn multiline_paste_asks_first_and_escape_cancels(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("echo one\necho two".into()));
    cx.dispatch_action(Paste);
    assert_eq!(
        view.read_with(cx, |t, _| t.overlays.top()),
        Some(OverlayKind::PasteConfirm)
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(view.read_with(cx, |t, _| t.overlays.top()), None);
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    assert!(!screen(&view, cx).iter().any(|r| r == "one"));
}

#[gpui::test]
fn multiline_paste_confirmed_with_enter_runs_every_line(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("echo one\necho two\n".into()));
    cx.dispatch_action(Paste);
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |t, _| t.overlays.top()), None);
    // Without line editing (dash) the tty echoes the second line ahead of the prompt,
    // so its output lands after "$ ".
    let is_two = |r: &str| r == "two" || r == "$ two";
    let rows = wait_for(&view, cx, is_two);
    assert!(
        rows.iter().any(|r| r == "one") && rows.iter().any(|r| is_two(r)),
        "{rows:?}"
    );
}

#[gpui::test]
fn bracketed_paste_skips_the_confirmation(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "printf '\\033[?2004h'\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !view.read_with(cx, |t, _| t.live().unwrap().term.modes().bracketed_paste) {
        assert!(
            Instant::now() < deadline,
            "shell never enabled bracketed paste"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    cx.write_to_clipboard(ClipboardItem::new_string("a\nb".into()));
    cx.dispatch_action(Paste);
    assert_eq!(view.read_with(cx, |t, _| t.overlays.top()), None);
}

#[gpui::test]
fn scroll_actions_move_through_scrollback(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(
        &view,
        cx,
        "i=0; while [ $i -lt 300 ]; do echo row$i; i=$((i+1)); done; echo done-marker\n",
    );
    wait_for(&view, cx, |r| r == "done-marker");
    let offset = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.read_with(cx, |t, _| t.live().unwrap().snapshot.display_offset)
    };
    assert_eq!(offset(&view, cx), 0);
    cx.dispatch_action(ScrollToTop);
    let top = offset(&view, cx);
    assert!(top > 100, "history should be deep, got {top}");
    cx.dispatch_action(ScrollToBottom);
    assert_eq!(offset(&view, cx), 0);
    cx.dispatch_action(ScrollByPage(-1));
    assert!(offset(&view, cx) > 0);
    cx.dispatch_action(ScrollByPage(1));
    assert_eq!(offset(&view, cx), 0);
    cx.dispatch_action(ScrollByLine(-1));
    assert_eq!(offset(&view, cx), 1);
}

#[gpui::test]
fn clear_scrollback_drops_history(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(
        &view,
        cx,
        "i=0; while [ $i -lt 200 ]; do echo r$i; i=$((i+1)); done; echo cleared-marker\n",
    );
    wait_for(&view, cx, |r| r == "cleared-marker");
    let history = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.read_with(cx, |t, _| t.live().unwrap().snapshot.history_size)
    };
    assert!(history(&view, cx) > 100);
    cx.dispatch_action(ClearScrollback);
    assert_eq!(history(&view, cx), 0);
}

#[gpui::test]
fn painting_sizes_the_pty_to_the_viewport(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let (cols, rows) = view.read_with(cx, |t, _| t.live().unwrap().size.get());
    assert!(cols > 1 && rows > 1, "painted size {cols}x{rows}");
    // The PTY resize is asynchronous, so ask again until the shell reports the new size.
    let want = format!("{rows} {cols}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        write(&view, cx, "stty size\n");
        let found = wait_for(&view, cx, |r| r == want);
        if found.contains(&want) {
            break;
        }
        assert!(Instant::now() < deadline, "wanted {want}: {found:?}");
    }
}

#[gpui::test]
fn shell_exit_closes_the_tab(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let before = view.read_with(cx, |t, _| t.tabs.len());
    write(&view, cx, "exit\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(30));
        cx.run_until_parked();
        let n = view.read_with(cx, |t, _| t.tabs.len());
        if n < before || Instant::now() > deadline {
            assert_eq!(n, before - 1);
            break;
        }
    }
}

#[gpui::test]
fn bell_flashes_the_viewport(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "printf '\\007'\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(20));
        cx.run_until_parked();
        if view.read_with(cx, |t, _| t.bell_flash) {
            break;
        }
        assert!(Instant::now() < deadline, "bell never flashed");
    }
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(!view.read_with(cx, |t, _| t.bell_flash));
}

mod mouse {
    use super::*;
    use gpui::{point, px, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};

    fn run(view: &Entity<AppWindow>, cx: &mut VisualTestContext, cmd: &str) {
        view.update(cx, |this, _| {
            this.live()
                .unwrap()
                .term
                .write(format!("{cmd}\n").into_bytes());
        });
    }

    /// Window position of the middle of cell (`row`, `col`).
    fn cell_pos(
        view: &Entity<AppWindow>,
        cx: &mut VisualTestContext,
        row: usize,
        col: usize,
    ) -> gpui::Point<gpui::Pixels> {
        view.read_with(cx, |this, _| {
            let (ox, oy) = this.live().unwrap().grid_origin.get();
            point(
                px(ox + (col as f32 + 0.5) * this.cell.width),
                px(oy + (row as f32 + 0.5) * this.cell.height),
            )
        })
    }

    fn down(
        view: &Entity<AppWindow>,
        cx: &mut VisualTestContext,
        pos: gpui::Point<gpui::Pixels>,
        clicks: usize,
        mods: Modifiers,
    ) {
        view.update_in(cx, |this, window, cx| {
            this.mouse_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: pos,
                    modifiers: mods,
                    click_count: clicks,
                    first_mouse: false,
                },
                window,
                cx,
            );
        });
    }

    fn up(view: &Entity<AppWindow>, cx: &mut VisualTestContext, pos: gpui::Point<gpui::Pixels>) {
        view.update(cx, |this, cx| {
            this.mouse_up(
                &MouseUpEvent {
                    button: MouseButton::Left,
                    position: pos,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                },
                cx,
            );
        });
    }

    fn row_of(rows: &[String], needle: &str) -> usize {
        rows.iter().position(|r| r.contains(needle)).expect(needle)
    }

    #[gpui::test]
    fn double_click_selects_a_word_and_copy_reads_it(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        run(&view, cx, "echo alpha-beta gamma");
        let rows = wait_for(&view, cx, |r| r == "alpha-beta gamma");
        let row = rows.iter().rposition(|r| r == "alpha-beta gamma").unwrap();
        let pos = cell_pos(&view, cx, row, 13);
        down(&view, cx, pos, 2, Modifiers::default());
        up(&view, cx, pos);
        let copied = view.update(cx, |this, cx| this.copy_selection(cx));
        assert!(copied);
        assert_eq!(
            cx.read_from_clipboard().and_then(|i| i.text()).as_deref(),
            Some("gamma")
        );
    }

    #[gpui::test]
    fn drag_selects_text(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        run(&view, cx, "echo abcdefgh");
        let rows = wait_for(&view, cx, |r| r == "abcdefgh");
        let row = rows.iter().rposition(|r| r == "abcdefgh").unwrap();
        let a = cell_pos(&view, cx, row, 2);
        let b = cell_pos(&view, cx, row, 5);
        down(&view, cx, a, 1, Modifiers::default());
        view.update(cx, |this, cx| {
            this.mouse_move(
                &gpui::MouseMoveEvent {
                    position: b,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                },
                true,
                cx,
            );
        });
        up(&view, cx, b);
        view.update(cx, |this, cx| this.copy_selection(cx));
        assert_eq!(
            cx.read_from_clipboard().and_then(|i| i.text()).as_deref(),
            Some("cde")
        );
    }

    #[gpui::test]
    fn osc8_link_is_found_and_blocked_scheme_toasts(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        run(
            &view,
            cx,
            r"printf '\033]8;;ftp://example.com/x\033\\LINKTEXT\033]8;;\033\\\n'",
        );
        let rows = wait_for(&view, cx, |r| r == "LINKTEXT");
        let row = rows.iter().rposition(|r| r == "LINKTEXT").unwrap();
        let link = view
            .update(cx, |this, _| this.link_at(row, 3))
            .expect("link");
        assert_eq!((link.start, link.end), (0, 8));
        assert_eq!(link.target, "ftp://example.com/x");
        view.update(cx, |this, cx| this.open_link("ftp://example.com/x", cx));
        let toast = view.read_with(cx, |this, _| this.toasts.last().map(|t| t.text.to_string()));
        assert!(toast.unwrap().contains("Blocked link scheme"));
        let _ = row_of(&rows, "LINKTEXT");
    }

    #[gpui::test]
    fn implicit_url_and_user_rule(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        view.update(cx, |this, _| {
            let mut c = (*this.cfg).clone();
            c.hyperlink_rules.push(mt_config::HyperlinkRule {
                regex: r"\b([A-Z]{2,}-\d+)\b".into(),
                format: "https://jira.example/browse/$1".into(),
            });
            this.cfg = std::sync::Arc::new(c);
            this.links = crate::mouse_input::build_links(&this.cfg);
        });
        run(&view, cx, "echo see PAY-12 and https://example.com/a.");
        let rows = wait_for(&view, cx, |r| r == "see PAY-12 and https://example.com/a.");
        let row = rows
            .iter()
            .rposition(|r| r == "see PAY-12 and https://example.com/a.")
            .unwrap();
        let jira = view
            .update(cx, |this, _| this.link_at(row, 5))
            .expect("jira link");
        assert_eq!(jira.target, "https://jira.example/browse/PAY-12");
        assert_eq!((jira.start, jira.end), (4, 10));
        let url = view
            .update(cx, |this, _| this.link_at(row, 20))
            .expect("url");
        assert_eq!(url.target, "https://example.com/a");
        assert!(view.update(cx, |this, _| this.link_at(row, 0)).is_none());
    }

    #[gpui::test]
    fn a_selection_item_from_the_config_shows_only_with_a_selection(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("mt_live_menu_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("microterm.toml");
        std::fs::write(
            &f,
            r#"
[[context_menu.viewport.items]]
id    = "git_log_s"
label = "git log -S selection"
icon  = "search"
when  = "selection"
spawn = { command = ["git", "-C", "{cwd}", "log", "-S", "{selection}", "--oneline"], where = "split-right" }
"#,
        )
        .unwrap();
        cx.update(|cx| cx.set_global(crate::settings::Settings::load(f)));
        let (view, cx) = setup(cx);
        run(&view, cx, "echo needle-text");
        let rows = wait_for(&view, cx, |r| r.starts_with("needle-text"));
        let row = row_of(&rows, "needle-text");
        let rows_of_menu = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
            view.read_with(cx, |this, _| {
                this.open_menu
                    .as_ref()
                    .map(|m| {
                        m.nav
                            .rows
                            .iter()
                            .map(|r| (r.id.clone(), r.enabled))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
        };
        let right = |view: &Entity<AppWindow>, cx: &mut VisualTestContext, pos| {
            view.update_in(cx, |this, window, cx| {
                this.open_menu = None;
                this.mouse_down(
                    &MouseDownEvent {
                        button: MouseButton::Right,
                        position: pos,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    },
                    window,
                    cx,
                );
            });
        };
        // No selection: hidden.
        let pos = cell_pos(&view, cx, row, 2);
        right(&view, cx, pos);
        let items = rows_of_menu(&view, cx);
        assert!(!items.is_empty(), "the menu opened");
        assert!(!items.iter().any(|(id, _)| id == "git_log_s"), "{items:?}");
        // Double-click selects a word; right-click on it shows the item, enabled.
        view.update(cx, |this, _| this.open_menu = None);
        down(&view, cx, pos, 2, Modifiers::default());
        up(&view, cx, pos);
        right(&view, cx, pos);
        let items = rows_of_menu(&view, cx);
        assert!(
            items.iter().any(|(id, on)| id == "git_log_s" && *on),
            "{items:?}"
        );
    }

    #[gpui::test]
    fn right_click_opens_context_menu(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        let pos = cell_pos(&view, cx, 0, 0);
        view.update_in(cx, |this, window, cx| {
            this.mouse_down(
                &MouseDownEvent {
                    button: MouseButton::Right,
                    position: pos,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                },
                window,
                cx,
            );
        });
        let (menu, selection, link) = view.read_with(cx, |this, _| {
            let m = this.open_menu.as_ref().expect("menu");
            (
                m.menu,
                m.facts.values.get("selection").map(str::to_string),
                m.facts.values.get("link").map(str::to_string),
            )
        });
        assert_eq!(menu, "viewport");
        assert!(selection.is_none());
        assert!(link.is_none());
    }
}

fn top(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Option<OverlayKind> {
    view.read_with(cx, |this, _| this.overlays.top())
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

#[gpui::test]
fn quick_select_copies_a_git_sha_from_the_live_screen(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "printf 'abc1234 fix bug\\n'\n");
    wait_for(&view, cx, |r| r == "abc1234 fix bug");
    cx.dispatch_action(ActivateQuickSelect);
    let (label, text) = view.read_with(cx, |t, _| {
        let h = t
            .quick
            .hits
            .iter()
            .find(|h| h.text == "abc1234")
            .expect("sha is labelled");
        (h.label.clone(), h.text.clone())
    });
    cx.simulate_input(&label);
    assert_eq!(top(&view, cx), None);
    assert_eq!(clipboard(cx).as_deref(), Some(text.as_str()));
}

#[gpui::test]
fn uppercase_quick_select_label_pastes_into_the_pane(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "printf 'abc1234 fix bug\\n'\n");
    wait_for(&view, cx, |r| r == "abc1234 fix bug");
    cx.dispatch_action(ActivateQuickSelect);
    let label = view.read_with(cx, |t, _| {
        t.quick
            .hits
            .iter()
            .find(|h| h.text == "abc1234")
            .unwrap()
            .label
            .clone()
    });
    cx.simulate_input(&label.to_uppercase());
    assert_eq!(clipboard(cx).as_deref(), Some("abc1234"));
    let pasted = |r: &str| r.ends_with("$ abc1234");
    let rows = wait_for(&view, cx, pasted);
    assert!(rows.iter().any(|r| pasted(r)), "{rows:?}");
}

#[gpui::test]
fn copy_mode_yanks_a_block_from_scrollback_with_the_keyboard(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "seq 1 200\n");
    wait_for(&view, cx, |r| r == "200");
    assert!(view.read_with(cx, |t, _| t.live().unwrap().snapshot.history_size) > 0);
    cx.dispatch_action(ActivateCopyMode);
    // Find where the output starts: the prompt may wrap, so do not assume a row.
    let first = view.read_with(cx, |t, _| {
        t.grid_full()
            .rows
            .iter()
            .position(|r| r.text(0, 9, true) == "1")
            .expect("row with 1")
    });
    cx.simulate_keystrokes("g g");
    for d in first.to_string().chars() {
        cx.simulate_keystrokes(&d.to_string());
    }
    cx.simulate_keystrokes("j");
    cx.simulate_keystrokes("ctrl-v j j l");
    cx.simulate_keystrokes("y");
    assert_eq!(top(&view, cx), None);
    assert_eq!(clipboard(cx).as_deref(), Some("1\n2\n3"));
    // The view followed the cursor into scrollback while selecting.
    assert!(view.read_with(cx, |t, _| t.live().unwrap().snapshot.display_offset) > 0);
}

#[gpui::test]
fn search_finds_matches_in_scrollback_and_scrolls_to_them(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "seq 1 200\n");
    wait_for(&view, cx, |r| r == "200");
    cx.dispatch_action(Search);
    cx.simulate_input("150");
    assert_eq!(view.read_with(cx, |t, _| t.search_status()), "1 of 1");
    let visible = view.read_with(cx, |t, _| {
        let (top, rows) = t.view_geometry();
        let line = t.search.matches[0].start().0;
        (line >= top && line < top + rows, t.search_marks().len())
    });
    assert_eq!(visible, (true, 1), "the match was scrolled into view");
}

#[gpui::test]
fn copy_mode_selection_is_mirrored_into_the_terminal(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "echo hello-world\n");
    wait_for(&view, cx, |r| r == "hello-world");
    cx.dispatch_action(ActivateCopyMode);
    cx.simulate_keystrokes("k");
    cx.simulate_keystrokes("v $");
    let sel = view.read_with(cx, |t, _| t.live().unwrap().snapshot.selection);
    assert!(
        sel.is_some(),
        "selection is highlighted like a mouse selection"
    );
    cx.simulate_keystrokes("escape");
    let sel = view.read_with(cx, |t, _| t.live().unwrap().snapshot.selection);
    assert!(sel.is_none());
}

#[gpui::test]
fn quick_select_on_ls_la_can_be_typed_and_escaped(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "ls -la /tmp; echo DONE\n");
    wait_for(&view, cx, |r| r == "DONE");
    cx.dispatch_action(ActivateQuickSelect);
    assert_eq!(top(&view, cx), Some(OverlayKind::QuickSelect));
    let n = view.read_with(cx, |t, _| t.quick.hits.len());
    eprintln!("hits: {n}");
    cx.simulate_keystrokes("escape");
    assert_eq!(top(&view, cx), None, "Esc closes");
    cx.dispatch_action(ActivateQuickSelect);
    let label = view.read_with(cx, |t, _| t.quick.hits.first().map(|h| h.label.clone()));
    if let Some(label) = label {
        cx.simulate_input(&label);
        assert_eq!(top(&view, cx), None, "label closes");
    }
}

#[gpui::test]
fn overlay_keys_still_work_if_focus_is_stuck_on_the_viewport(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    write(&view, cx, "printf 'abc1234\\n'\n");
    wait_for(&view, cx, |r| r == "abc1234");
    cx.dispatch_action(ActivateQuickSelect);
    // Simulate focus landing on the pane while quick select is open.
    view.update_in(cx, |this, window, _| window.focus(&this.viewport_focus));
    cx.simulate_keystrokes("escape");
    assert_eq!(
        top(&view, cx),
        None,
        "Esc closes even without overlay focus"
    );
    cx.dispatch_action(ActivateQuickSelect);
    view.update_in(cx, |this, window, _| window.focus(&this.viewport_focus));
    let label = view.read_with(cx, |t, _| t.quick.hits[0].label.clone());
    cx.simulate_input(&label);
    assert_eq!(top(&view, cx), None, "labels work too");
}

mod vt_sequences {
    use super::*;
    use gpui::{CursorStyle, Modifiers};

    fn pointer(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> CursorStyle {
        view.read_with(cx, |this, _| this.pointer_style())
    }

    fn set_shift(view: &Entity<AppWindow>, cx: &mut VisualTestContext, on: bool) {
        view.update_in(cx, |this, _, _| {
            this.mouse.mods = Modifiers {
                shift: on,
                ..Default::default()
            };
        });
    }

    #[gpui::test]
    fn osc_22_sets_the_pointer_and_ends_with_mouse_reporting(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        assert_eq!(pointer(&view, cx), CursorStyle::IBeam);
        write(&view, cx, "printf '\\033]22;pointer\\007one'\n");
        wait_for(&view, cx, |r| r.ends_with("one"));
        assert_eq!(pointer(&view, cx), CursorStyle::PointingHand);
        // A name GPUI has no cursor for shows the arrow.
        write(&view, cx, "printf '\\033]22;wait\\007two'\n");
        wait_for(&view, cx, |r| r.ends_with("two"));
        assert_eq!(pointer(&view, cx), CursorStyle::Arrow);
        // Empty name resets.
        write(&view, cx, "printf '\\033]22;\\007three'\n");
        wait_for(&view, cx, |r| r.ends_with("three"));
        assert_eq!(pointer(&view, cx), CursorStyle::IBeam);
        // It beats the arrow shown while the program has mouse reporting on, and ends when the
        // program turns reporting off.
        write(
            &view,
            cx,
            "printf '\\033[?1000h\\033]22;crosshair\\007four'\n",
        );
        wait_for(&view, cx, |r| r.ends_with("four"));
        assert_eq!(pointer(&view, cx), CursorStyle::Crosshair);
        write(&view, cx, "printf '\\033[?1000lfive'\n");
        wait_for(&view, cx, |r| r.ends_with("five"));
        assert_eq!(pointer(&view, cx), CursorStyle::IBeam);
    }

    #[gpui::test]
    fn hyperlink_hover_beats_osc_22(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        write(&view, cx, "printf '\\033]22;crosshair\\007six'\n");
        wait_for(&view, cx, |r| r.ends_with("six"));
        view.update_in(cx, |this, _, _| {
            this.mouse.hover = Some(crate::mouse_input::HoverLink {
                row: 0,
                start: 0,
                end: 3,
                target: "https://example.com".into(),
                show_target: false,
            });
            this.mouse.mods = Modifiers {
                #[cfg(target_os = "macos")]
                platform: true,
                #[cfg(not(target_os = "macos"))]
                control: true,
                ..Default::default()
            };
        });
        assert_eq!(pointer(&view, cx), CursorStyle::PointingHand);
    }

    #[gpui::test]
    fn xtshiftescape_decides_whether_shift_bypasses_mouse_reporting(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx);
        write(&view, cx, "printf '\\033[?1000hseven'\n");
        wait_for(&view, cx, |r| r.ends_with("seven"));
        // Default: reporting on (arrow), Shift bypasses it (I-beam).
        assert_eq!(pointer(&view, cx), CursorStyle::Arrow);
        set_shift(&view, cx, true);
        assert_eq!(pointer(&view, cx), CursorStyle::IBeam);
        // `CSI > 1 s`: Shift-modified events go to the program.
        write(&view, cx, "printf '\\033[>1seight'\n");
        wait_for(&view, cx, |r| r.ends_with("eight"));
        assert_eq!(pointer(&view, cx), CursorStyle::Arrow);
        // `CSI > 0 s` restores the bypass.
        write(&view, cx, "printf '\\033[>0snine'\n");
        wait_for(&view, cx, |r| r.ends_with("nine"));
        assert_eq!(pointer(&view, cx), CursorStyle::IBeam);
    }
}

#[gpui::test]
fn split_panes_each_get_their_own_shell_and_size(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let left = view.read_with(cx, |t, _| t.tabs[0].pane);
    cx.dispatch_action(SplitHorizontal);
    cx.run_until_parked();
    let right = view.read_with(cx, |t, _| t.tabs[0].pane);
    assert_ne!(left, right);
    // Tests do not start shells on reconcile; give the new focused pane one.
    view.update_in(cx, |this, window, cx| {
        this.attach_terminal(
            0,
            SpawnOptions {
                shell: "/bin/sh".into(),
                env: vec![("PS1".into(), "R$ ".into())],
                ..Default::default()
            },
            window,
            cx,
        )
        .expect("spawn /bin/sh");
    });
    cx.run_until_parked();

    // Painting sized each PTY to its own half of the window.
    let sizes = |view: &Entity<AppWindow>, cx: &mut VisualTestContext| {
        view.read_with(cx, |t, _| {
            let tab = &t.tabs[0];
            (
                tab.pane_view(left)
                    .unwrap()
                    .session
                    .as_ref()
                    .unwrap()
                    .size
                    .get(),
                tab.pane_view(right)
                    .unwrap()
                    .session
                    .as_ref()
                    .unwrap()
                    .size
                    .get(),
            )
        })
    };
    let (l, r) = sizes(&view, cx);
    assert!(l.0 > 0 && r.0 > 0, "both panes were laid out: {l:?} {r:?}");
    assert!(
        l.0.abs_diff(r.0) <= 2,
        "an even split gives equal widths: {l:?} {r:?}"
    );

    // Keys go to the focused (right) pane only.
    cx.simulate_input("echo only-right");
    cx.simulate_keystrokes("enter");
    let deadline = Instant::now() + Duration::from_secs(10);
    let text = |view: &Entity<AppWindow>, cx: &mut VisualTestContext, pane: u64| -> String {
        view.read_with(cx, |t, _| {
            let s = t.tabs[0].pane_view(pane).unwrap().session.as_ref().unwrap();
            (0..s.snapshot.rows)
                .map(|r| s.snapshot.row_text(r))
                .collect::<Vec<_>>()
                .join("\n")
        })
    };
    loop {
        std::thread::sleep(Duration::from_millis(30));
        cx.run_until_parked();
        if text(&view, cx, right).contains("only-right") || Instant::now() > deadline {
            break;
        }
    }
    assert!(text(&view, cx, right).contains("only-right"));
    assert!(!text(&view, cx, left).contains("only-right"));

    // Closing the focused pane hands the tab back to the left shell.
    cx.dispatch_action(ClosePane);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |t, _| t.tabs[0].panes.len()), 1);
    assert_eq!(view.read_with(cx, |t, _| t.tabs[0].pane), left);
    assert!(view.read_with(cx, |t, _| t.live().is_some()));
}
