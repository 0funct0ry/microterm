use super::*;
use crate::rules::tests::SPEC_FILE;
use crate::{Level, Scope};

fn rules(src: &str) -> Vec<Rule> {
    load_str(src, LoadOpts::default()).unwrap().rules
}

#[test]
fn saving_an_unchanged_rule_is_byte_identical() {
    for r in rules(SPEC_FILE) {
        assert_eq!(upsert(SPEC_FILE, &r).unwrap(), SPEC_FILE, "rule {}", r.id);
    }
}

#[test]
fn editing_one_field_changes_only_that_line() {
    let mut r = rules(SPEC_FILE).remove(0);
    r.name = "Build broke".into();
    let out = upsert(SPEC_FILE, &r).unwrap();
    let diff: Vec<_> = SPEC_FILE
        .lines()
        .zip(out.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(diff.len(), 1, "{diff:?}");
    assert!(diff[0].1.contains("Build broke"));
    assert_eq!(SPEC_FILE.lines().count(), out.lines().count());
    assert!(out.contains("# generated, stable, never reused"));
}

#[test]
fn changing_actions_keeps_the_other_rule_intact() {
    let mut r = rules(SPEC_FILE).remove(0);
    r.actions.truncate(1);
    if let crate::Action::Highlight { scope, .. } = &mut r.actions[0] {
        *scope = Scope::Match;
    }
    let out = upsert(SPEC_FILE, &r).unwrap();
    let back = rules(&out);
    assert_eq!(back[0], r);
    let tail = &SPEC_FILE[SPEC_FILE.find("[[trigger]]\nid = \"t_77be02\"").unwrap()..];
    assert!(out.ends_with(tail));
}

#[test]
fn new_rules_append_with_a_version_and_round_trip() {
    let mut r = Rule::new_text("Err", r"error: (?P<m>.+)");
    r.id = "t_000001".into();
    r.actions.push(crate::Action::Notify {
        level: Level::Error,
        title: "T".into(),
        message: "{m}".into(),
        desktop: true,
    });
    let out = upsert("", &r).unwrap();
    assert!(out.starts_with("version = 1"));
    assert_eq!(rules(&out), vec![r.clone()]);
    let mut b = Rule::new_text("B", "b");
    b.id = "t_000002".into();
    b.enabled = false;
    b.panes = "domain:local".into();
    b.cooldown_ms = 0;
    let out2 = upsert(&out, &b).unwrap();
    assert_eq!(rules(&out2), vec![r, b]);
    assert!(out2.starts_with(&out));
}

#[test]
fn every_action_and_event_form_round_trips() {
    use crate::*;
    let mut r = Rule::new_text("All", r"(?P<n>\d+)");
    r.id = "t_a".into();
    r.when = When::Text {
        regex: r"(?P<n>\d+)".into(),
        case_insensitive: true,
    };
    r.actions = vec![Action::Highlight {
        scope: Scope::Line,
        color: "#ff0000".into(),
        style: Style::Underline,
        ttl_s: 9,
    }];
    let all = vec![
        Action::Notify {
            level: Level::Attention,
            title: "a".into(),
            message: "b".into(),
            desktop: true,
        },
        Action::Status {
            state: State::Working,
            progress: Some(Progress::Value(5)),
            text: "t".into(),
            ttl_s: 3,
        },
        Action::Status {
            state: State::Done,
            progress: Some(Progress::Template("{n}".into())),
            text: String::new(),
            ttl_s: 0,
        },
        Action::TabColor {
            color: "red".into(),
            ttl_s: 4,
            clear_on_focus: true,
        },
        Action::Bell {},
        Action::Clipboard { source: "n".into() },
        Action::Recording {
            op: RecOp::Start,
            stop_after_s: 30,
        },
        Action::Recording {
            op: RecOp::Stop,
            stop_after_s: 0,
        },
        Action::Marker {
            label: "m".into(),
            color: "info".into(),
        },
        Action::Counter {
            reset_on: ResetOn::CommandStart,
        },
        Action::Title {
            template: "x".into(),
            ttl_s: 1,
        },
        Action::Findings { label: "f".into() },
        Action::Snapshot {
            before: 2,
            after: 3,
        },
        Action::Screenshot {
            scale: 1.5,
            chrome: ShotChrome::Plain,
        },
    ];
    for a in all {
        let mut x = r.clone();
        x.actions = vec![a.clone()];
        let out = upsert("", &x).unwrap();
        assert_eq!(rules(&out), vec![x], "{a:?}");
    }
    for (event, exit) in [
        (EventKind::CommandFinished, ExitFilter::Code(130)),
        (EventKind::PaneExited, ExitFilter::Failure),
        (EventKind::CwdChanged, ExitFilter::Any),
        (EventKind::TitleChanged, ExitFilter::Any),
        (EventKind::Bell, ExitFilter::Any),
    ] {
        let mut x = r.clone();
        x.when = When::Event { event, exit };
        x.actions = vec![Action::Bell {}];
        let out = upsert("", &x).unwrap();
        assert_eq!(rules(&out), vec![x]);
    }
}

#[test]
fn remove_and_reorder() {
    let out = remove(SPEC_FILE, "t_3f9a1c").unwrap();
    assert_eq!(rules(&out).len(), 1);
    assert!(out.contains("t_77be02") && !out.contains("t_3f9a1c"));
    let out = reorder(SPEC_FILE, "t_77be02", Some("t_3f9a1c")).unwrap();
    let ids: Vec<_> = rules(&out).into_iter().map(|r| r.id).collect();
    assert_eq!(ids, ["t_77be02", "t_3f9a1c"]);
    assert!(remove(SPEC_FILE, "nope").is_err());
}

#[test]
fn derived_ids_are_written_explicitly_on_save() {
    let src = "[[trigger]]\nname = \"x\"\n[trigger.when]\nkind = \"text\"\nregex = \"a\"\n[[trigger.actions]]\nkind = \"bell\"\n";
    let mut r = rules(src).remove(0);
    assert!(r.id.starts_with("t_"));
    r.name = "y".into();
    let out = upsert(src, &r).unwrap();
    assert!(out.contains(&format!("id = \"{}\"", r.id)));
    assert_eq!(rules(&out)[0].id, r.id);
}

#[test]
fn files_that_do_not_load_are_never_written() {
    assert!(upsert("[[trigger]\n", &Rule::new_text("a", "b")).is_err());
}

#[test]
fn save_detects_stale_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("triggers.toml");
    std::fs::write(&path, SPEC_FILE).unwrap();
    let base = base_of(SPEC_FILE, Some("t_3f9a1c"));
    // An unrelated edit elsewhere in the file does not make this rule stale.
    let other = SPEC_FILE.replace("Long command finished", "Long command done");
    std::fs::write(&path, &other).unwrap();
    let mut r = rules(SPEC_FILE).remove(0);
    r.cooldown_ms = 5;
    save_rule(&path, &base, &r).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("Long command done") && text.contains("cooldown_ms = 5"));
    // A change to the same rule is stale.
    let edited = text.replace("Build failed\"\nenabled", "Build failed!\"\nenabled");
    std::fs::write(&path, edited).unwrap();
    assert!(matches!(save_rule(&path, &base, &r), Err(EditError::Stale)));
    // New rules start from an empty base.
    let nb = base_of("", None);
    let mut n = Rule::new_text("N", "n");
    n.id = "t_new".into();
    std::fs::write(&path, "").unwrap();
    save_rule(&path, &nb, &n).unwrap();
    assert_eq!(rules(&std::fs::read_to_string(&path).unwrap()).len(), 1);
    remove_rule(
        &path,
        &base_of(&std::fs::read_to_string(&path).unwrap(), Some("t_new")),
        "t_new",
    )
    .unwrap();
    assert!(rules(&std::fs::read_to_string(&path).unwrap()).is_empty());
}

#[test]
fn scope_round_trips_and_is_omitted_when_it_is_the_default() {
    let mut r = Rule::new_text("S", "x");
    r.id = "t_s".into();
    let out = upsert("", &r).unwrap();
    assert!(!out.contains("scope"), "{out}");
    r.scope = crate::MatchIn::Any;
    let out = upsert(&out, &r).unwrap();
    assert!(out.contains("scope = \"any\""));
    assert_eq!(rules(&out)[0].scope, crate::MatchIn::Any);
    r.scope = crate::MatchIn::Output;
    let back = upsert(&out, &r).unwrap();
    assert!(!back.contains("scope"));
}
