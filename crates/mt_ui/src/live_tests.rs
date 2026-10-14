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
    let rows = wait_for(&view, cx, |r| r == "two");
    assert!(
        rows.iter().any(|r| r == "one") && rows.iter().any(|r| r == "two"),
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
