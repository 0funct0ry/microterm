//! Window-level trigger tests: the gate matrix, the line tap on a live shell, event rules, hot
//! reload and loop safety (SPEC §10.11, M12.6 "Tests").

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::TestAppContext;
use mt_mux::MuxEvent;
use mt_term::SpawnOptions;

use super::runtime;
use crate::{
    app::AppWindow,
    mux_host,
    settings::{self, Settings},
};

pub(super) fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mt_trig_ui_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Writes `microterm.toml` (and `triggers.toml` when given) into a fresh directory.
pub(super) fn config(name: &str, toml: &str, rules: Option<&str>) -> PathBuf {
    let d = dir(name);
    std::fs::write(d.join("microterm.toml"), toml).unwrap();
    if let Some(r) = rules {
        std::fs::write(d.join("triggers.toml"), r).unwrap();
    }
    d.join("microterm.toml")
}

/// Boots the app state the way `main` does (settings, theme, mux host, a workspace window with
/// a `/bin/sh` tab) and returns the first pane.
pub(super) fn boot(cx: &mut TestAppContext, path: PathBuf) -> u64 {
    cx.update(|cx| {
        cx.set_global(Settings::load(path));
        settings::apply_theme(cx);
        mux_host::ensure(cx);
        runtime::sync(cx);
        mux_host::show_workspace(cx, "default", true, None).unwrap();
    });
    cx.run_until_parked();
    // Unit tests never start shells on their own (`reconcile` skips it under `cfg!(test)`).
    let handle = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find_map(|w| w.downcast::<AppWindow>())
            .expect("a window")
    });
    cx.update(|cx| {
        handle
            .update(cx, |this, window, cx| {
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
            })
            .expect("window update")
            .expect("spawn /bin/sh");
    });
    cx.run_until_parked();
    cx.update(|cx| mux_host::rows(cx).focused_pane.expect("a focused pane"))
}

pub(super) fn term(cx: &mut TestAppContext, pane: u64) -> Arc<mt_term::Terminal> {
    cx.update(|cx| mux_host::terminal(cx, pane).expect("terminal"))
}

pub(super) fn type_line(cx: &mut TestAppContext, pane: u64, line: &str) {
    term(cx, pane).write(format!("{line}\n").into_bytes());
}

pub(super) fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut f: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(30));
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
    }
}

pub(super) fn fired(cx: &mut TestAppContext, id: &str) -> u64 {
    cx.update(|cx| {
        cx.global::<runtime::TriggerRuntime>()
            .engine
            .stats(id)
            .fired
    })
}

pub(super) fn tapped(cx: &mut TestAppContext, pane: u64) -> bool {
    term(cx, pane).line_tap_installed()
}

pub(super) fn events(cx: &mut TestAppContext) -> Arc<Mutex<Vec<MuxEvent>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    cx.update(|cx| {
        mux_host::host(cx)
            .mux
            .subscribe(move |e| l2.lock().unwrap().push(e.clone()));
    });
    log
}

/// `microterm.toml` for the tests: the gate section plus a `/bin/sh` shell.
pub(super) fn cfg(triggers: &str) -> String {
    format!("shell = \"/bin/sh\"\n{triggers}")
}

pub(super) const ON: &str = "[triggers]\nenabled = true\n";

pub(super) const RULE: &str = r#"version = 1

[[trigger]]
id = "t_build"
name = "Build failed"
cooldown_ms = 0
[trigger.when]
kind = "text"
regex = 'error: (?P<msg>.+)'
[[trigger.actions]]
kind = "notify"
level = "error"
message = "{msg}"
"#;

#[gpui::test]
fn gate_off_is_inert(cx: &mut TestAppContext) {
    // A broken file would raise a banner if it were read.
    let pane = boot(cx, config("off", &cfg(""), Some("[[trigger]\nbroken")));
    assert!(!cx.update(|cx| runtime::active(cx)));
    assert!(!tapped(cx, pane), "no line tap while the gate is off");
    assert!(cx.update(|cx| runtime::banner(cx)).is_none());
    // Output goes by and nothing happens.
    type_line(cx, pane, "echo 'error: nothing'");
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| runtime::dropped_lines(cx)), 0);
}

#[gpui::test]
fn enabled_rule_fires_on_shell_output(cx: &mut TestAppContext) {
    let pane = boot(cx, config("on", &cfg(ON), Some(RULE)));
    assert!(cx.update(|cx| runtime::active(cx)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    let log = events(cx);
    type_line(cx, pane, "echo 'error: build failed'");
    wait_until(cx, "the rule to fire", |cx| fired(cx, "t_build") >= 1);
    let evs = log.lock().unwrap().clone();
    assert!(
        evs.iter()
            .any(|e| matches!(e, MuxEvent::TriggerFired { rule, actions, .. }
            if rule == "t_build" && actions == &["notify".to_string()])),
        "{evs:?}"
    );
}

#[gpui::test]
fn disabling_the_gate_removes_taps_and_stops_rules(cx: &mut TestAppContext) {
    let path = config("flip", &cfg(ON), Some(RULE));
    let pane = boot(cx, path.clone());
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    std::fs::write(&path, cfg("[triggers]\nenabled = false\n")).unwrap();
    cx.update(|cx| {
        settings::reload(cx);
    });
    cx.run_until_parked();
    assert!(!cx.update(|cx| runtime::active(cx)));
    assert!(!tapped(cx, pane));
    type_line(cx, pane, "echo 'error: gone'");
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(fired(cx, "t_build"), 0);
    // Turning it back on re-reads the rules and re-installs the tap, without a restart.
    std::fs::write(&path, cfg(ON)).unwrap();
    cx.update(|cx| {
        settings::reload(cx);
    });
    cx.run_until_parked();
    assert!(cx.update(|cx| runtime::active(cx)));
    wait_until(cx, "the tap again", |cx| tapped(cx, pane));
}

#[gpui::test]
fn a_broken_rules_file_keeps_the_last_valid_rules_and_names_the_file(cx: &mut TestAppContext) {
    let path = config("broken", &cfg(ON), Some(RULE));
    let _pane = boot(cx, path.clone());
    let rules = path.with_file_name("triggers.toml");
    std::fs::write(&rules, "[[trigger]]\nname = \"x\"\n[trigger.when]\nkind = \"text\"\nregex = '(?=x)'\n[[trigger.actions]]\nkind = \"bell\"\n").unwrap();
    cx.update(|cx| {
        runtime::sync(cx);
    });
    let banner = cx.update(|cx| runtime::banner(cx)).expect("banner");
    assert!(
        banner.text.starts_with("triggers.toml:5:"),
        "{}",
        banner.text
    );
    assert!(banner.text.contains("last valid"));
    let ids: Vec<String> = cx.update(|cx| {
        cx.global::<runtime::TriggerRuntime>()
            .engine
            .rules()
            .map(|r| r.id.clone())
            .collect()
    });
    assert_eq!(ids, ["t_build"]);
    // Fixing the file clears the banner.
    std::fs::write(&rules, RULE).unwrap();
    cx.update(|cx| {
        runtime::sync(cx);
    });
    assert!(cx.update(|cx| runtime::banner(cx)).is_none());
}

#[gpui::test]
fn event_rule_fires_on_bell_even_with_bell_mode_none(cx: &mut TestAppContext) {
    let rules = r#"[[trigger]]
id = "t_bell"
name = "Bell rule"
cooldown_ms = 0
[trigger.when]
kind = "event"
event = "bell"
[[trigger.actions]]
kind = "marker"
label = "{event}"
"#;
    let pane = boot(
        cx,
        config(
            "bell",
            &cfg("[triggers]\nenabled = true\n[bell]\nmode = \"none\"\n"),
            Some(rules),
        ),
    );
    // The pump that turns BEL into `MuxEvent::Bell` runs inside the pane's window.
    type_line(cx, pane, "printf '\\a'");
    wait_until(cx, "the bell rule", |cx| fired(cx, "t_bell") >= 1);
}

#[gpui::test]
fn the_bell_action_does_not_emit_bell(cx: &mut TestAppContext) {
    let rules = r#"[[trigger]]
id = "t_loop"
name = "Ring"
cooldown_ms = 0
[trigger.when]
kind = "text"
regex = 'ring-me'
[[trigger.actions]]
kind = "bell"
[[trigger]]
id = "t_watch"
name = "Watcher"
cooldown_ms = 0
[trigger.when]
kind = "event"
event = "bell"
[[trigger.actions]]
kind = "counter"
"#;
    let pane = boot(cx, config("loop", &cfg(ON), Some(rules)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    let log = events(cx);
    type_line(cx, pane, "echo ring-me");
    wait_until(cx, "the ringing rule", |cx| fired(cx, "t_loop") >= 1);
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    assert!(!log
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, MuxEvent::Bell { .. })));
    assert_eq!(
        fired(cx, "t_watch"),
        0,
        "a rule can never fire another through its own output"
    );
}

// ---------------------------------------------------------------- the reuse-path actions

use mt_triggers::{Intent as I, Level, ResetOn, State};

use super::engine::Firing;

fn firing(pane: u64, intents: Vec<I>) -> Firing {
    Firing {
        rule_id: "t_x".into(),
        rule_name: "Build failed".into(),
        pane,
        abs_line: Some(0),
        line: Some("error: boom".into()),
        intents,
    }
}

fn run(cx: &mut TestAppContext, f: Firing) {
    cx.update(|cx| super::apply::run(cx, f));
    cx.run_until_parked();
}

fn chrome(cx: &mut TestAppContext, pane: u64) -> crate::tab_chrome::TabChrome {
    cx.update(|cx| mux_host::tab_chrome(cx, pane))
}

fn tab_of(cx: &mut TestAppContext, pane: u64) -> u64 {
    cx.update(|cx| {
        let h = mux_host::host(cx);
        let (w, i) = h.mux.locate_pane(pane).unwrap();
        h.mux.window(w).unwrap().tabs[i].id
    })
}

fn tab_color(cx: &mut TestAppContext, pane: u64) -> (Option<String>, &'static str) {
    let tab = tab_of(cx, pane);
    cx.update(|cx| {
        let (c, s) = mux_host::host(cx).mux.tab(tab).unwrap().resolved_color();
        (c.map(|c| c.to_string()), s.name())
    })
}

#[gpui::test]
fn notify_raises_the_badge_and_publishes_a_trigger_notification(cx: &mut TestAppContext) {
    let pane = boot(cx, config("notify", &cfg(ON), None));
    let fake = Arc::new(crate::notifier::FakeNotifier::default());
    let f2 = fake.clone();
    cx.update(|cx| mux_host::host(cx).notifier = f2);
    let log = events(cx);
    run(
        cx,
        firing(
            pane,
            vec![I::Notify {
                level: Level::Error,
                title: "Build failed".into(),
                message: "boom".into(),
                desktop: true,
            }],
        ),
    );
    let evs = log.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(e, MuxEvent::Notification { source, level, message, .. }
        if source.as_str() == "trigger" && *level == mt_proto::NotifyLevel::Error && message == "boom")), "{evs:?}");
    assert_eq!(
        chrome(cx, pane).badge,
        Some(crate::components::StatusKind::Error)
    );
    assert_eq!(
        fake.sent.lock().unwrap()[0],
        ("Build failed".to_string(), "boom".to_string())
    );
    // The shared limit: 5 per second per pane.
    for _ in 0..8 {
        run(
            cx,
            firing(
                pane,
                vec![I::Notify {
                    level: Level::Info,
                    title: String::new(),
                    message: "x".into(),
                    desktop: false,
                }],
            ),
        );
    }
    let n = log
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, MuxEvent::Notification { .. }))
        .count();
    assert_eq!(n, 5);
}

#[gpui::test]
fn status_sets_the_dot_progress_and_tooltip(cx: &mut TestAppContext) {
    let pane = boot(cx, config("status", &cfg(ON), None));
    run(
        cx,
        firing(
            pane,
            vec![I::Status {
                state: State::Working,
                progress: Some(40),
                text: "building".into(),
                ttl_s: 0,
            }],
        ),
    );
    let c = chrome(cx, pane);
    assert_eq!(c.dot, Some(crate::components::StatusKind::Accent));
    assert_eq!(c.progress, Some(40));
    assert_eq!(c.tooltip.as_deref(), Some("working 40%: building"));
}

#[gpui::test]
fn tab_color_has_the_trigger_source_and_manual_wins(cx: &mut TestAppContext) {
    let pane = boot(cx, config("color", &cfg(ON), None));
    run(
        cx,
        firing(
            pane,
            vec![I::TabColor {
                color: "red".into(),
                ttl_s: 0,
                clear_on_focus: false,
            }],
        ),
    );
    assert_eq!(tab_color(cx, pane), (Some("red".into()), "trigger"));
    let c = chrome(cx, pane);
    assert!(c
        .tooltip
        .unwrap()
        .contains("Color: red (trigger: Build failed)"));
    // A manual color wins; clearing it brings the trigger color back.
    let tab = tab_of(cx, pane);
    cx.update(|cx| {
        mux_host::host(cx)
            .mux
            .set_tab_color(tab, Some(mt_mux::TabColor::parse("blue").unwrap()))
            .unwrap();
    });
    assert_eq!(tab_color(cx, pane), (Some("blue".into()), "manual"));
    cx.update(|cx| {
        mux_host::host(cx).mux.set_tab_color(tab, None).unwrap();
    });
    assert_eq!(tab_color(cx, pane), (Some("red".into()), "trigger"));
    // A later firing replaces the color.
    run(
        cx,
        firing(
            pane,
            vec![I::TabColor {
                color: "green".into(),
                ttl_s: 0,
                clear_on_focus: false,
            }],
        ),
    );
    assert_eq!(tab_color(cx, pane), (Some("green".into()), "trigger"));
}

#[gpui::test]
fn tab_color_reverts_on_ttl_focus_and_exit(cx: &mut TestAppContext) {
    let pane = boot(cx, config("color2", &cfg(ON), None));
    let ev = |cx: &mut TestAppContext| tab_color(cx, pane).1;
    // ttl_s
    run(
        cx,
        firing(
            pane,
            vec![I::TabColor {
                color: "red".into(),
                ttl_s: 1,
                clear_on_focus: false,
            }],
        ),
    );
    assert_eq!(ev(cx), "trigger");
    std::thread::sleep(Duration::from_millis(1100));
    cx.update(super::apply::tick);
    assert_eq!(ev(cx), "none");
    // clear_on_focus
    run(
        cx,
        firing(
            pane,
            vec![I::TabColor {
                color: "red".into(),
                ttl_s: 0,
                clear_on_focus: true,
            }],
        ),
    );
    cx.update(|cx| super::on_focus(cx, pane));
    assert_eq!(ev(cx), "none");
    // Without either it stays until the pane exits.
    run(
        cx,
        firing(
            pane,
            vec![I::TabColor {
                color: "red".into(),
                ttl_s: 0,
                clear_on_focus: false,
            }],
        ),
    );
    cx.update(|cx| super::on_focus(cx, pane));
    assert_eq!(ev(cx), "trigger");
    cx.update(|cx| super::apply::pane_exited(cx, pane));
    assert_eq!(ev(cx), "none");
}

#[gpui::test]
fn title_overrides_osc_but_not_a_manual_rename_and_never_emits_title_changed(
    cx: &mut TestAppContext,
) {
    let pane = boot(cx, config("title", &cfg(ON), None));
    let log = events(cx);
    run(
        cx,
        firing(
            pane,
            vec![I::Title {
                text: "BUILD FAILED".into(),
                ttl_s: 0,
            }],
        ),
    );
    let tab = tab_of(cx, pane);
    let shown = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let h = mux_host::host(cx);
            let t = h.mux.tab(tab).unwrap();
            mt_mux::format_title(
                &mt_mux::TitleParts {
                    title_override: t.title_override.as_deref(),
                    trigger_title: t.trigger_title.as_deref(),
                    pane_title: Some("zsh"),
                    ..Default::default()
                },
                "{title}",
            )
        })
    };
    assert_eq!(shown(cx), "BUILD FAILED");
    assert!(chrome(cx, pane)
        .tooltip
        .unwrap()
        .contains("Title set by a trigger"));
    assert!(!log
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, MuxEvent::TitleChanged { .. })));
    // The user's rename ends it.
    cx.update(|cx| {
        let h = mux_host::host(cx);
        let (w, _) = h.mux.locate_pane(pane).unwrap();
        h.mux.rename_tab(w, tab, Some("mine".into())).unwrap();
    });
    assert_eq!(shown(cx), "mine");
    assert!(cx.update(|cx| mux_host::host(cx)
        .mux
        .tab(tab)
        .unwrap()
        .trigger_title
        .is_none()));
}

#[gpui::test]
fn counter_pill_sums_and_resets_on_focus(cx: &mut TestAppContext) {
    let pane = boot(cx, config("counter", &cfg(ON), None));
    for _ in 0..3 {
        run(
            cx,
            firing(
                pane,
                vec![I::Counter {
                    reset_on: ResetOn::Focus,
                }],
            ),
        );
    }
    let c = chrome(cx, pane);
    assert_eq!(c.counter.as_deref(), Some("3"));
    assert!(c.tooltip.unwrap().contains("t_x: 3"));
    cx.update(|cx| super::on_focus(cx, pane));
    assert_eq!(chrome(cx, pane).counter, None);
    for _ in 0..120 {
        run(
            cx,
            firing(
                pane,
                vec![I::Counter {
                    reset_on: ResetOn::CommandStart,
                }],
            ),
        );
    }
    assert_eq!(chrome(cx, pane).counter.as_deref(), Some("99+"));
    cx.update(|cx| super::on_command_start(cx, pane));
    assert_eq!(chrome(cx, pane).counter, None);
}

#[gpui::test]
fn the_bell_action_runs_without_a_bell_event(cx: &mut TestAppContext) {
    let pane = boot(cx, config("bellact", &cfg(ON), None));
    let log = events(cx);
    run(cx, firing(pane, vec![I::Bell]));
    assert!(!log
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, MuxEvent::Bell { .. })));
    assert!(log
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, MuxEvent::TriggerFired { .. })));
}

#[gpui::test]
fn recording_start_needs_the_recording_gate_and_stops_only_trigger_recordings(
    cx: &mut TestAppContext,
) {
    let _g = crate::recording::tests::serial();
    crate::recording::tests::store_dir();
    // Gate off: skipped with a reason on the rule's row.
    let pane = boot(cx, config("rec0", &cfg(ON), Some(RULE)));
    run(
        cx,
        firing(pane, vec![I::RecordingStart { stop_after_s: 0 }]),
    );
    assert!(!cx.update(|cx| mux_host::host(cx).store_recorders.is_recording(pane)));
    let err = cx.update(|cx| {
        cx.global::<runtime::TriggerRuntime>()
            .engine
            .stats("t_x")
            .last_error
    });
    assert_eq!(
        err.as_deref(),
        Some("recording skipped: recording disabled")
    );

    let pane = boot(
        cx,
        config(
            "rec1",
            &cfg(&format!("{ON}[recording]\nenabled = true\n")),
            Some(RULE),
        ),
    );
    let before = crate::recording::tests::casts().len();
    run(
        cx,
        firing(pane, vec![I::RecordingStart { stop_after_s: 0 }]),
    );
    assert!(cx.update(|cx| mux_host::host(cx).store_recorders.is_recording(pane)));
    // A second start is a no-op while the pane is recording.
    run(
        cx,
        firing(pane, vec![I::RecordingStart { stop_after_s: 0 }]),
    );
    run(cx, firing(pane, vec![I::RecordingStop]));
    assert!(!cx.update(|cx| mux_host::host(cx).store_recorders.is_recording(pane)));
    let casts = crate::recording::tests::casts();
    assert_eq!(casts.len(), before + 1);
    // The store is shared with other tests: find the sidecar this rule wrote.
    let side = casts
        .iter()
        .filter_map(|c| std::fs::read_to_string(c.with_extension("json")).ok())
        .find(|t| t.contains("\"title\": \"Trigger: Build failed\""))
        .expect("the trigger recording's sidecar");
    // The origin names the rule, a marker carries its name, and keystrokes are never captured.
    assert!(side.contains("\"trigger\": \"trigger:t_x\""), "{side}");
    assert!(side.contains("\"has_input\": false"), "{side}");
    assert!(side.contains("\"label\": \"Build failed\""), "{side}");
}

#[gpui::test]
fn turning_the_gate_off_clears_colors_titles_status_counters_and_stops_trigger_recordings(
    cx: &mut TestAppContext,
) {
    let _g = crate::recording::tests::serial();
    crate::recording::tests::store_dir();
    let path = config(
        "cleanup",
        &cfg(&format!("{ON}[recording]\nenabled = true\n")),
        Some(RULE),
    );
    let pane = boot(cx, path.clone());
    run(
        cx,
        firing(
            pane,
            vec![
                I::TabColor {
                    color: "red".into(),
                    ttl_s: 0,
                    clear_on_focus: false,
                },
                I::Title {
                    text: "T".into(),
                    ttl_s: 0,
                },
                I::Status {
                    state: State::Working,
                    progress: None,
                    text: "s".into(),
                    ttl_s: 0,
                },
                I::Counter {
                    reset_on: ResetOn::Never,
                },
                I::RecordingStart { stop_after_s: 0 },
            ],
        ),
    );
    assert!(cx.update(|cx| mux_host::host(cx).store_recorders.is_recording(pane)));
    std::fs::write(
        &path,
        cfg("[triggers]\nenabled = false\n[recording]\nenabled = true\n"),
    )
    .unwrap();
    cx.update(|cx| {
        settings::reload(cx);
    });
    cx.run_until_parked();
    assert_eq!(tab_color(cx, pane).1, "none");
    let tab = tab_of(cx, pane);
    assert!(cx.update(|cx| mux_host::host(cx)
        .mux
        .tab(tab)
        .unwrap()
        .trigger_title
        .is_none()));
    let c = chrome(cx, pane);
    assert_eq!((c.dot, c.counter), (None, None));
    assert!(!cx.update(|cx| mux_host::host(cx).store_recorders.is_recording(pane)));
}

// -------------------------------------------------------- highlights, markers, findings

const DECO_RULE: &str = r#"[[trigger]]
id = "t_deco"
name = "Deco"
cooldown_ms = 0
[trigger.when]
kind = "text"
regex = 'FOUND-(?P<n>\d+)'
[[trigger.actions]]
kind = "highlight"
color = "red"
[[trigger.actions]]
kind = "marker"
label = "found {n}"
color = "error"
[[trigger.actions]]
kind = "findings"
label = "hit {n}"
"#;

fn deco_state(cx: &mut TestAppContext, pane: u64) -> (usize, usize, usize) {
    cx.update(|cx| {
        let rt = cx.global::<runtime::TriggerRuntime>();
        (
            rt.deco.highlight_count(pane),
            rt.deco.markers(pane).len(),
            rt.findings.len(),
        )
    })
}

#[gpui::test]
fn a_match_adds_a_highlight_a_marker_and_a_finding_on_its_line(cx: &mut TestAppContext) {
    let pane = boot(cx, config("deco", &cfg(ON), Some(DECO_RULE)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(cx, pane, "echo FOUND-42");
    wait_until(cx, "the decorations", |cx| {
        let (h, m, f) = deco_state(cx, pane);
        h >= 1 && m >= 1 && f >= 1
    });
    let (marker, finding) = cx.update(|cx| {
        let rt = cx.global::<runtime::TriggerRuntime>();
        let ms = rt.deco.markers(pane);
        (
            ms.last().cloned().unwrap(),
            rt.findings.items().last().cloned().unwrap(),
        )
    });
    assert_eq!(finding.label, "hit 42");
    assert!(marker.label.starts_with("found "));
    // The decoration sits on the output line (or the echoed command), both contain the text.
    assert!(finding.line.contains("FOUND-42"));
    let rows = cx.update(|cx| {
        let t = mux_host::terminal(cx, pane).unwrap();
        let snap = t.snapshot();
        let theme = *cx.global::<crate::theme::Theme>();
        let deco = crate::triggers::paint::for_pane(cx, pane, &snap, &theme);
        (0..snap.rows)
            .filter(|r| deco.rects.iter().any(|d| d.row == *r))
            .map(|r| snap.row_text(r))
            .collect::<Vec<_>>()
    });
    assert!(rows.iter().any(|r| r.contains("FOUND-42")), "{rows:?}");
}

#[gpui::test]
fn clicking_a_finding_scrolls_back_to_its_line_and_flashes_it(cx: &mut TestAppContext) {
    let pane = boot(cx, config("jump", &cfg(ON), Some(DECO_RULE)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(cx, pane, "echo FOUND-7; seq 1 300");
    wait_until(cx, "the finding and the output", |cx| {
        deco_state(cx, pane).2 >= 1 && {
            let t = term(cx, pane);
            let s = t.snapshot();
            (0..s.rows).any(|r| s.row_text(r).trim() == "300")
        }
    });
    let index = cx.update(|cx| {
        let rt = cx.global::<runtime::TriggerRuntime>();
        rt.findings
            .items()
            .position(|f| f.line == "FOUND-7")
            .expect("the output line")
    });
    // Live view: the line is far above the viewport.
    assert_eq!(term(cx, pane).snapshot().display_offset, 0);
    assert!(cx.update(|cx| runtime::jump_to_finding(cx, index, true)));
    let snap = term(cx, pane).snapshot();
    assert!(snap.display_offset > 0, "scrolled back");
    let rows: Vec<String> = (0..snap.rows).map(|r| snap.row_text(r)).collect();
    assert!(rows.iter().any(|r| r == "FOUND-7"), "{rows:?}");
    // The flash is drawn now and gone after 1.2 s.
    let flashing = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let t = mux_host::terminal(cx, pane).unwrap();
            let s = t.snapshot();
            cx.global::<runtime::TriggerRuntime>()
                .deco
                .rects(pane, &s, mt_term::now_ms())
                .iter()
                .any(|r| r.flash)
        })
    };
    assert!(flashing(cx));
    // "Mark whole line" off: only the matched characters flash.
    let width = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let t = mux_host::terminal(cx, pane).unwrap();
            let s = t.snapshot();
            cx.global::<runtime::TriggerRuntime>()
                .deco
                .rects(pane, &s, mt_term::now_ms())
                .iter()
                .filter(|r| r.flash)
                .map(|r| r.end - r.start)
                .sum::<usize>()
        })
    };
    let whole = width(cx);
    assert!(cx.update(|cx| runtime::jump_to_finding(cx, index, false)));
    assert!(width(cx) <= whole);
    std::thread::sleep(Duration::from_millis(1300));
    assert!(!flashing(cx));
}

#[gpui::test]
fn next_and_previous_marker_walk_the_focused_panes_markers(cx: &mut TestAppContext) {
    let pane = boot(cx, config("walk", &cfg(ON), Some(DECO_RULE)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(
        cx,
        pane,
        "echo FOUND-1; seq 1 100; echo FOUND-2; seq 1 100; echo FOUND-3; seq 1 100",
    );
    wait_until(cx, "three output markers", |cx| {
        cx.update(|cx| {
            let rt = cx.global::<runtime::TriggerRuntime>();
            rt.findings
                .items()
                .filter(|f| f.line.starts_with("FOUND-"))
                .count()
                >= 3
        })
    });
    let top = |cx: &mut TestAppContext| {
        let (_, h, o, _) = term(cx, pane).view_info();
        h - o
    };
    let mut seen = Vec::new();
    for _ in 0..4 {
        cx.update(|cx| runtime::step_marker(cx, false));
        seen.push(top(cx));
    }
    assert!(
        seen[0] > seen[1] && seen[1] > seen[2],
        "walking up visits earlier markers: {seen:?}"
    );
    // Walking back down passes the markers again, ending below where the walk up ended.
    for _ in 0..2 {
        assert!(cx.update(|cx| runtime::step_marker(cx, true)));
    }
    assert!(top(cx) > *seen.last().unwrap(), "{seen:?} then {}", top(cx));
    cx.update(runtime::clear_markers);
    assert_eq!(
        cx.update(|cx| cx
            .global::<runtime::TriggerRuntime>()
            .deco
            .markers(pane)
            .len()),
        0
    );
    assert!(!cx.update(|cx| runtime::step_marker(cx, true)));
}

#[gpui::test]
fn turning_the_gate_off_removes_decorations_markers_and_findings(cx: &mut TestAppContext) {
    let path = config("decooff", &cfg(ON), Some(DECO_RULE));
    let pane = boot(cx, path.clone());
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(cx, pane, "echo FOUND-9");
    wait_until(cx, "decorations", |cx| deco_state(cx, pane) != (0, 0, 0));
    std::fs::write(&path, cfg("[triggers]\nenabled = false\n")).unwrap();
    cx.update(|cx| {
        settings::reload(cx);
    });
    cx.run_until_parked();
    assert_eq!(deco_state(cx, pane), (0, 0, 0));
}

// ------------------------------------------------------------ the gated actions

const SNAP_RULE: &str = r#"[[trigger]]
id = "t_snap"
name = "Snap"
cooldown_ms = 0
[trigger.when]
kind = "text"
regex = '^SNAPME'
[[trigger.actions]]
kind = "snapshot"
before = 2
after = 0
[[trigger.actions]]
kind = "screenshot"
[[trigger.actions]]
kind = "clipboard"
"#;

fn captures(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

#[gpui::test]
fn capture_and_clipboard_actions_do_nothing_until_their_second_gates_are_set(
    cx: &mut TestAppContext,
) {
    let caps = dir("caps_closed");
    let cfg = cfg(&format!(
        "[triggers]\nenabled = true\ncapture_dir = \"{}\"\n",
        caps.join("c").display()
    ));
    let pane = boot(cx, config("gates_closed", &cfg, Some(SNAP_RULE)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("untouched".into())));
    type_line(cx, pane, "echo SNAPME");
    wait_until(cx, "the rule", |cx| fired(cx, "t_snap") >= 1);
    cx.run_until_parked();
    assert!(
        captures(&caps.join("c")).is_empty(),
        "no file, no directory contents"
    );
    assert!(
        !caps.join("c").exists(),
        "the directory is not even created"
    );
    let (skipped, err) = cx.update(|cx| {
        let s = cx
            .global::<runtime::TriggerRuntime>()
            .engine
            .stats("t_snap");
        (s.skipped_gate, s.last_error)
    });
    assert!(skipped >= 3, "{skipped}");
    assert!(err.unwrap().contains("triggers.allow_"));
    let clip = cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(clip.as_deref(), Some("untouched"));
}

#[gpui::test]
fn open_gates_write_only_into_the_captures_directory_with_private_modes(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let caps = dir("caps_open").join("c");
    let toml = cfg(&format!(
        "[triggers]\nenabled = true\nallow_capture = true\nallow_clipboard = true\ncapture_dir = \"{}\"\n",
        caps.display()
    ));
    let pane = boot(cx, config("gates_open", &toml, Some(SNAP_RULE)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(
        cx,
        pane,
        "echo before-one; echo 'password=hunter2'; echo SNAPME",
    );
    wait_until(cx, "a text and a png capture", |_cx| {
        let f = captures(&caps);
        f.iter().any(|p| p.extension().is_some_and(|e| e == "txt"))
            && f.iter().any(|p| p.extension().is_some_and(|e| e == "png"))
    });
    for p in captures(&caps) {
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        assert!(
            mt_triggers::capture::is_ours(&name) && name.starts_with("t_snap-"),
            "{name}"
        );
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::metadata(&caps).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let txt = captures(&caps)
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "txt"))
        .unwrap();
    let body = std::fs::read_to_string(txt).unwrap();
    assert!(
        body.starts_with("# microterm trigger capture\n# rule: Snap\n"),
        "{body}"
    );
    assert!(body.contains("SNAPME"));
    assert!(
        body.contains("password=‹redacted›") && !body.contains("hunter2"),
        "{body}"
    );
    let png = captures(&caps)
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "png"))
        .unwrap();
    assert!(std::fs::read(png).unwrap().starts_with(b"\x89PNG"));
    // The clipboard action always says so, whatever the window state.
    let clip = cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert!(
        clip.as_deref().is_some_and(|t| t.starts_with("SNAPME")),
        "{clip:?}"
    );
}

#[gpui::test]
fn an_unsafe_captures_directory_is_refused_and_reported_on_the_rule(cx: &mut TestAppContext) {
    let d = dir("caps_unsafe");
    let real = d.join("real");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, d.join("link")).unwrap();
    let toml = cfg(&format!(
        "[triggers]\nenabled = true\nallow_capture = true\ncapture_dir = \"{}\"\n",
        d.join("link").display()
    ));
    let pane = boot(cx, config("unsafe", &toml, Some(SNAP_RULE)));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(cx, pane, "echo SNAPME");
    wait_until(cx, "the rule", |cx| fired(cx, "t_snap") >= 1);
    cx.run_until_parked();
    assert!(captures(&real).is_empty());
    let err = cx.update(|cx| {
        cx.global::<runtime::TriggerRuntime>()
            .engine
            .stats("t_snap")
            .last_error
    });
    assert!(err.unwrap().contains("unsafe captures dir"));
}

// ------------------------------------------------------------------------------ trigger.* IPC

fn rpc(
    cx: &mut TestAppContext,
    r: mt_proto::Request,
) -> Result<serde_json::Value, mt_proto::RpcError> {
    cx.update(|cx| mux_host::handle_call(cx, r))
}

fn all_requests(id: &str, pane: u64) -> Vec<mt_proto::Request> {
    use mt_proto::{Request as R, Target, TriggerIdParams as I, TriggerTestParams};
    vec![
        R::TriggerList,
        R::TriggerGet(I { id: id.into() }),
        R::TriggerEnable(I { id: id.into() }),
        R::TriggerDisable(I { id: id.into() }),
        R::TriggerPause,
        R::TriggerResume,
        R::TriggerTest(TriggerTestParams {
            id: id.into(),
            target: Target::pane(pane),
        }),
    ]
}

#[gpui::test]
fn ipc_answers_disabled_with_exit_code_10_while_the_gate_is_off(cx: &mut TestAppContext) {
    let _pane = boot(cx, config("ipc_off", &cfg(""), Some(RULE)));
    for r in all_requests("t_build", 1) {
        let e = rpc(cx, r).unwrap_err();
        assert_eq!(e.code, mt_proto::ErrorCode::Disabled);
        assert_eq!(e.code.exit_code(), 10);
        assert!(e.message.contains("triggers.enabled"));
    }
    assert!(!cx.update(|cx| runtime::active(cx)));
}

#[gpui::test]
fn ipc_lists_gets_and_tests_rules_and_enable_disable_are_session_only(cx: &mut TestAppContext) {
    let path = config("ipc_on", &cfg(ON), Some(RULE));
    let pane = boot(cx, path.clone());
    let file = path.with_file_name("triggers.toml");
    let before = std::fs::read(&file).unwrap();
    let list = rpc(cx, mt_proto::Request::TriggerList).unwrap();
    let rows = list["triggers"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0]["id"].as_str(),
            rows[0]["kind"].as_str(),
            rows[0]["enabled"].as_bool()
        ),
        (Some("t_build"), Some("text"), Some(true))
    );
    assert_eq!(list["paused"], false);
    let get = rpc(
        cx,
        mt_proto::Request::TriggerGet(mt_proto::TriggerIdParams {
            id: "t_build".into(),
        }),
    )
    .unwrap();
    assert_eq!(get["rule"]["name"], "Build failed");
    // Unknown rules are "not found" (exit 3).
    let e = rpc(
        cx,
        mt_proto::Request::TriggerGet(mt_proto::TriggerIdParams { id: "nope".into() }),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.code.exit_code()),
        (mt_proto::ErrorCode::NotFound, 3)
    );
    // Disable and enable change the session only and never the file.
    let off = rpc(
        cx,
        mt_proto::Request::TriggerDisable(mt_proto::TriggerIdParams {
            id: "t_build".into(),
        }),
    )
    .unwrap();
    assert_eq!(off["session_only"], true);
    assert_eq!(
        rpc(cx, mt_proto::Request::TriggerList).unwrap()["triggers"][0]["enabled"],
        false
    );
    type_line(cx, pane, "echo 'error: while off'");
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(fired(cx, "t_build"), 0);
    rpc(
        cx,
        mt_proto::Request::TriggerEnable(mt_proto::TriggerIdParams {
            id: "t_build".into(),
        }),
    )
    .unwrap();
    assert_eq!(
        rpc(cx, mt_proto::Request::TriggerPause).unwrap()["paused"],
        true
    );
    assert_eq!(
        rpc(cx, mt_proto::Request::TriggerList).unwrap()["paused"],
        true
    );
    rpc(cx, mt_proto::Request::TriggerResume).unwrap();
    assert_eq!(
        std::fs::read(&file).unwrap(),
        before,
        "ctl never writes triggers.toml"
    );
    // Test: the dry run lists what would run on the pane's recent lines and runs nothing.
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(cx, pane, "echo 'error: for the test'");
    wait_until(cx, "the output", |cx| {
        let s = term(cx, pane).peek_snapshot();
        (0..s.rows).any(|r| s.row_text(r) == "error: for the test")
    });
    let fired_before = fired(cx, "t_build");
    let t = rpc(
        cx,
        mt_proto::Request::TriggerTest(mt_proto::TriggerTestParams {
            id: "t_build".into(),
            target: mt_proto::Target::pane(pane),
        }),
    )
    .unwrap();
    assert_eq!(t["ran"], false);
    let firings = t["firings"].as_array().unwrap();
    assert!(
        firings.iter().any(|f| f["input"] == "error: for the test"
            && f["actions"][0].as_str().unwrap().contains("for the test")),
        "{t}"
    );
    cx.run_until_parked();
    assert_eq!(fired(cx, "t_build"), fired_before, "Test fires nothing");
}

// ------------------------------------------------------------------- scope = output / any

const FAKE_PROMPT: &str = r"printf '\033]133;A\007$ \033]133;B\007echo error: boom\n\033]133;C\007error: boom\n\033]133;D;0\007'";

fn scope_rules(scope: &str) -> String {
    format!(
        r#"[[trigger]]
id = "t_in"
name = "Typed"
cooldown_ms = 0
{scope}
[trigger.when]
kind = "text"
regex = '^\$ echo error: boom$'
[[trigger.actions]]
kind = "counter"
[[trigger]]
id = "t_out"
name = "Output"
cooldown_ms = 0
[trigger.when]
kind = "text"
regex = '^error: boom$'
[[trigger.actions]]
kind = "counter"
"#
    )
}

#[gpui::test]
fn rules_skip_the_prompt_and_typed_command_by_default(cx: &mut TestAppContext) {
    let pane = boot(cx, config("scope_out", &cfg(ON), Some(&scope_rules(""))));
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(cx, pane, FAKE_PROMPT);
    wait_until(cx, "the output rule", |cx| fired(cx, "t_out") >= 1);
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(
        fired(cx, "t_in"),
        0,
        "the prompt and typed command are input, not output"
    );
}

#[gpui::test]
fn scope_any_also_matches_the_typed_command(cx: &mut TestAppContext) {
    let pane = boot(
        cx,
        config("scope_any", &cfg(ON), Some(&scope_rules("scope = \"any\""))),
    );
    wait_until(cx, "the tap", |cx| tapped(cx, pane));
    type_line(cx, pane, FAKE_PROMPT);
    wait_until(cx, "both rules", |cx| {
        fired(cx, "t_in") >= 1 && fired(cx, "t_out") >= 1
    });
}
