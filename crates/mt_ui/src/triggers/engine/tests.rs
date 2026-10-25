use super::*;
use mt_triggers::{Action, ExitFilter, Level, Scope, Style};

fn row(id: u64, domain: &str, title: &str) -> PaneRow {
    PaneRow {
        id,
        tab: 1,
        window: 1,
        workspace: "default".into(),
        domain: domain.into(),
        title: title.into(),
        cwd: Some("/tmp".into()),
        process: None,
        size: (80, 24),
        focused: false,
        active: false,
        zoomed: false,
        exited: false,
        exit_code: None,
        status: None,
        color: None,
    }
}

const ON: TriggerGate = TriggerGate {
    enabled: true,
    allow_clipboard: false,
    allow_capture: false,
};

fn text_rule(id: &str, re: &str, panes: &str, actions: Vec<Action>) -> Rule {
    let mut r = Rule::new_text(id, re);
    r.id = id.into();
    r.panes = panes.into();
    r.cooldown_ms = 1000;
    r.actions = actions;
    r
}

fn bell() -> Action {
    Action::Bell {}
}

fn worker_hit(e: &Engine, rule: &str, pane: u64, line: &str) -> WorkerHit {
    let snap = e.snapshot();
    let snap = snap.read().unwrap().clone();
    let hit = snap
        .matcher
        .scan(line)
        .into_iter()
        .find(|h| snap.ids[h.rule] == rule)
        .expect("matches");
    WorkerHit {
        rule: rule.into(),
        pane,
        abs_line: 5,
        partial: false,
        input: false,
        line: line.into(),
        hit,
    }
}

#[test]
fn text_match_becomes_intents_and_counts() {
    let mut e = Engine::default();
    e.load(vec![text_rule(
        "a",
        r"error: (?P<m>.+)",
        "*",
        vec![
            Action::Notify {
                level: Level::Error,
                title: String::new(),
                message: "{m}".into(),
                desktop: false,
            },
            Action::Highlight {
                scope: Scope::Line,
                color: "red".into(),
                style: Style::Background,
                ttl_s: 0,
            },
        ],
    )]);
    let h = worker_hit(&e, "a", 3, "error: boom");
    let f = e
        .accept_text(&h, &row(3, "local", "zsh"), ON, 1000)
        .unwrap();
    assert_eq!((f.rule_id.as_str(), f.pane, f.abs_line), ("a", 3, Some(5)));
    assert_eq!(f.intents.len(), 2);
    let s = e.stats("a");
    assert_eq!((s.fired, s.last_fired_ms), (1, Some(1000)));
    // Inside the cooldown nothing runs and nothing is counted.
    assert!(e
        .accept_text(&h, &row(3, "local", "zsh"), ON, 1500)
        .is_none());
    assert_eq!(e.stats("a").fired, 1);
    // Another pane is independent.
    assert!(e
        .accept_text(&h, &row(4, "local", "zsh"), ON, 1500)
        .is_some());
}

#[test]
fn selector_is_rechecked_against_current_facts() {
    let mut e = Engine::default();
    e.load(vec![text_rule("a", "x", "title:build*", vec![bell()])]);
    let h = worker_hit(&e, "a", 1, "x");
    assert!(e.accept_text(&h, &row(1, "local", "vim"), ON, 0).is_none());
    assert!(e
        .accept_text(&h, &row(1, "local", "build-api"), ON, 0)
        .is_some());
}

#[test]
fn taps_are_wanted_by_static_selector_clauses_only() {
    let mut e = Engine::default();
    e.load(vec![
        text_rule("a", "x", "domain:prod", vec![bell()]),
        text_rule("b", "y", "title:never*", vec![bell()]),
    ]);
    assert!(e.wants_pane(&row(1, "prod", "zsh")));
    // `b` has only a volatile clause, so it could match any pane later.
    assert!(e.wants_pane(&row(1, "local", "zsh")));
    e.set_session_enabled("b", false);
    assert!(!e.wants_pane(&row(1, "local", "zsh")));
    e.set_session_enabled("b", true);
    e.load(vec![text_rule("c", "x", "domain:prod,!id:7", vec![bell()])]);
    assert!(e.wants_pane(&row(1, "prod", "zsh")));
    assert!(!e.wants_pane(&row(7, "prod", "zsh")));
    assert!(!e.wants_pane(&row(1, "local", "zsh")));
    e.load(vec![]);
    assert!(!e.wants_pane(&row(1, "prod", "zsh")));
}

#[test]
fn event_rules_are_not_taps_but_fire_on_events() {
    let mut e = Engine::default();
    let mut r = Rule::new_text("ev", "x");
    r.id = "ev".into();
    r.panes = "!focused:true".into();
    r.when = When::Event {
        event: EventKind::CommandFinished,
        exit: ExitFilter::Failure,
    };
    r.actions = vec![Action::Notify {
        level: Level::Info,
        title: String::new(),
        message: "{title} exited with {exit}".into(),
        desktop: false,
    }];
    e.load(vec![r]);
    let bg = row(2, "local", "make");
    assert!(!e.wants_pane(&bg), "event rules need no line tap");
    let ev = |exit| EventInput {
        kind: EventKind::CommandFinished,
        pane: 2,
        exit,
    };
    assert!(e.accept_event(&ev(Some(0)), &bg, ON, 0).is_empty());
    let f = e.accept_event(&ev(Some(2)), &bg, ON, 0);
    assert_eq!(f.len(), 1);
    assert!(
        matches!(&f[0].intents[0], Intent::Notify { message, .. } if message == "make exited with 2")
    );
    let mut focused = row(2, "local", "make");
    focused.focused = true;
    assert!(e
        .accept_event(&ev(Some(2)), &focused, ON, 10_000)
        .is_empty());
    let other = EventInput {
        kind: EventKind::Bell,
        pane: 2,
        exit: None,
    };
    assert!(e.accept_event(&other, &bg, ON, 20_000).is_empty());
}

#[test]
fn gate_off_and_pause_stop_everything() {
    let mut e = Engine::default();
    e.load(vec![text_rule("a", "x", "*", vec![bell()])]);
    let h = worker_hit(&e, "a", 1, "x");
    let off = TriggerGate {
        enabled: false,
        ..ON
    };
    assert!(e.accept_text(&h, &row(1, "local", ""), off, 0).is_none());
    e.set_paused(true);
    assert!(e.paused() && e.accept_text(&h, &row(1, "local", ""), ON, 0).is_none());
    e.set_paused(false);
    assert!(e.accept_text(&h, &row(1, "local", ""), ON, 0).is_some());
}

#[test]
fn second_gates_skip_actions_but_keep_the_rule() {
    let mut e = Engine::default();
    e.load(vec![text_rule(
        "a",
        "x",
        "*",
        vec![
            Action::Clipboard {
                source: "match".into(),
            },
            Action::Snapshot {
                before: 0,
                after: 0,
            },
            Action::Screenshot {
                scale: 1.0,
                chrome: Default::default(),
            },
            bell(),
        ],
    )]);
    let h = worker_hit(&e, "a", 1, "x");
    let f = e.accept_text(&h, &row(1, "local", ""), ON, 0).unwrap();
    assert_eq!(
        f.intents.iter().map(Intent::kind).collect::<Vec<_>>(),
        ["bell"]
    );
    let s = e.stats("a");
    assert_eq!(s.skipped_gate, 3);
    assert!(s
        .last_error
        .as_deref()
        .unwrap()
        .contains("triggers.allow_clipboard"));
    // Opening the clipboard gate lets that action through on the next firing.
    let clip = TriggerGate {
        allow_clipboard: true,
        ..ON
    };
    let f = e.accept_text(&h, &row(1, "local", ""), clip, 5000).unwrap();
    assert_eq!(
        f.intents.iter().map(Intent::kind).collect::<Vec<_>>(),
        ["clipboard", "bell"]
    );
    // The capture gate opens snapshot and screenshot.
    let cap = TriggerGate {
        allow_capture: true,
        ..ON
    };
    let f = e
        .accept_text(&h, &row(1, "local", ""), cap, 10_000)
        .unwrap();
    assert_eq!(
        f.intents.iter().map(Intent::kind).collect::<Vec<_>>(),
        ["snapshot", "screenshot", "bell"]
    );
}

#[test]
fn action_limits_skip_individual_actions() {
    let mut e = Engine::default();
    let mut r = text_rule("a", "x", "*", vec![bell()]);
    r.cooldown_ms = 0;
    e.load(vec![r]);
    let h = worker_hit(&e, "a", 1, "x");
    let r1 = row(1, "local", "");
    assert_eq!(e.accept_text(&h, &r1, ON, 0).unwrap().intents.len(), 1);
    // Within a second the bell is rate limited but the rule still counts as fired.
    let f = e.accept_text(&h, &r1, ON, 100).unwrap();
    assert!(f.intents.is_empty());
    assert_eq!(e.stats("a").fired, 2);
    assert!(e.stats("a").last_error.unwrap().contains("rate limit"));
    assert!(e.suppressed("a") >= 1);
}

#[test]
fn a_rule_matching_every_line_is_bounded_and_counted() {
    let mut e = Engine::default();
    let mut r = text_rule(
        "all",
        ".",
        "*",
        vec![
            Action::Counter {
                reset_on: Default::default(),
            },
            bell(),
        ],
    );
    r.cooldown_ms = 0;
    e.load(vec![r]);
    let h = worker_hit(&e, "all", 1, "line");
    let mut ran = 0;
    for ms in 0..2000 {
        if e.accept_text(&h, &row(1, "local", ""), ON, ms).is_some() {
            ran += 1;
        }
    }
    assert!(ran <= 10 + 5, "{ran}");
    assert!(e.suppressed("all") > 1900);
}

#[test]
fn a_quiet_rule_counts_every_match_in_a_burst() {
    let mut e = Engine::default();
    let mut r = text_rule(
        "all",
        ".",
        "*",
        vec![Action::Counter {
            reset_on: Default::default(),
        }],
    );
    r.cooldown_ms = 0;
    e.load(vec![r]);
    let h = worker_hit(&e, "all", 1, "line");
    let ran = (0..100)
        .filter(|_| e.accept_text(&h, &row(1, "local", ""), ON, 0).is_some())
        .count();
    assert_eq!(ran, 100);
    assert_eq!(e.suppressed("all"), 0);
}

#[test]
fn session_enable_disable_never_touches_the_rule() {
    let mut e = Engine::default();
    e.load(vec![text_rule("a", "x", "*", vec![bell()])]);
    assert!(e.is_enabled("a"));
    assert!(e.set_session_enabled("a", false) && !e.is_enabled("a"));
    assert!(
        e.rule("a").unwrap().enabled,
        "the file's value is untouched"
    );
    assert!(!e.set_session_enabled("nope", true));
    // A reload keeps the session override of rules that remain.
    e.load(vec![text_rule("a", "x", "*", vec![bell()])]);
    assert!(!e.is_enabled("a"));
    e.clear();
    assert!(!e.is_enabled("a") && e.rules().next().is_none());
}

#[test]
fn stats_survive_reloads_of_the_same_rule() {
    let mut e = Engine::default();
    e.load(vec![text_rule("a", "x", "*", vec![bell()])]);
    let h = worker_hit(&e, "a", 1, "x");
    e.accept_text(&h, &row(1, "local", ""), ON, 0);
    e.load(vec![
        text_rule("a", "x", "*", vec![bell()]),
        text_rule("b", "y", "*", vec![bell()]),
    ]);
    assert_eq!(e.stats("a").fired, 1);
    e.load(vec![text_rule("b", "y", "*", vec![bell()])]);
    assert_eq!(e.stats("a").fired, 0);
}

#[test]
fn the_worker_scans_batches_and_reports_hits() {
    let q = LineQueue::new(100);
    let mut e = Engine::default();
    e.load(vec![
        text_rule("e", r"error: (?P<m>.+)", "*", vec![bell()]),
        text_rule("p", r"Continue\? \[y/N\]", "*", vec![bell()]),
    ]);
    let (tx, rx) = async_channel::bounded(16);
    let stop = Arc::new(AtomicBool::new(false));
    let h = spawn_worker(
        q.clone(),
        e.snapshot(),
        tx,
        e.overflow.clone(),
        stop.clone(),
    )
    .unwrap();
    for (i, t) in ["fine", "error: boom", "Continue? [y/N]"]
        .iter()
        .enumerate()
    {
        q.push(mt_term::CompletedLine {
            pane: 7,
            abs_line: i as u64,
            text: (*t).into(),
            partial: i == 2,
            input: false,
        });
    }
    let a = rx.recv_blocking().unwrap();
    let b = rx.recv_blocking().unwrap();
    assert_eq!((a.rule.as_str(), a.pane, a.abs_line), ("e", 7, 1));
    assert_eq!(a.hit.named("m"), Some("boom"));
    assert_eq!((b.rule.as_str(), b.partial), ("p", true));
    // A rules reload is picked up without restarting the worker.
    e.load(vec![text_rule("z", "zzz", "*", vec![bell()])]);
    q.push(mt_term::CompletedLine {
        pane: 7,
        abs_line: 9,
        text: "zzz".into(),
        partial: false,
        input: false,
    });
    assert_eq!(rx.recv_blocking().unwrap().rule, "z");
    stop.store(true, Ordering::Relaxed);
    q.wake();
    h.join().unwrap();
}

#[test]
fn a_slow_consumer_overflows_without_blocking_the_worker() {
    let q = LineQueue::new(1000);
    let mut e = Engine::default();
    e.load(vec![text_rule("all", ".", "*", vec![bell()])]);
    let (tx, rx) = async_channel::bounded(4);
    let stop = Arc::new(AtomicBool::new(false));
    let h = spawn_worker(
        q.clone(),
        e.snapshot(),
        tx,
        e.overflow.clone(),
        stop.clone(),
    )
    .unwrap();
    for i in 0..200 {
        q.push(mt_term::CompletedLine {
            pane: 1,
            abs_line: i,
            text: "x".into(),
            partial: false,
            input: false,
        });
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while e.overflow.load(Ordering::Relaxed) < 100 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(e.overflow.load(Ordering::Relaxed) >= 100);
    assert!(rx.len() <= 4);
    stop.store(true, Ordering::Relaxed);
    q.wake();
    h.join().unwrap();
}

#[test]
fn input_lines_never_fire_an_output_scoped_rule_but_an_any_rule_sees_them() {
    let mut e = Engine::default();
    let mut any = text_rule("any", "x", "*", vec![bell()]);
    any.scope = mt_triggers::MatchIn::Any;
    e.load(vec![text_rule("out", "x", "*", vec![bell()]), any]);
    let row = row(1, "local", "");
    for (id, input, fires) in [
        ("out", false, true),
        ("out", true, false),
        ("any", true, true),
        ("any", false, true),
    ] {
        let mut h = worker_hit(&e, id, 1, "x");
        h.input = input;
        // A fresh engine per case keeps the cooldown out of the way.
        let mut fresh = Engine::default();
        let mut any = text_rule("any", "x", "*", vec![bell()]);
        any.scope = mt_triggers::MatchIn::Any;
        fresh.load(vec![text_rule("out", "x", "*", vec![bell()]), any]);
        assert_eq!(fresh.precheck(&h, 0), fires, "{id} input={input}");
        assert_eq!(
            fresh.accept_text(&h, &row, ON, 0).is_some(),
            fires,
            "{id} input={input}"
        );
    }
}
