use super::*;

const KINDS: [&str; 13] = [
    "highlight",
    "notify",
    "status",
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
];

#[test]
fn every_kind_has_params_matching_its_values_and_round_trips() {
    for k in KINDS {
        let a = default_action(k);
        assert_eq!(a.kind(), k);
        let v = values_of(&a);
        assert_eq!(v.len(), params(k).len(), "{k}");
        assert_eq!(action_from(k, &v).unwrap(), a, "{k}");
    }
    // The groups of the Add-action menu list each kind exactly once.
    let mut listed: Vec<&str> = ACTION_GROUPS
        .iter()
        .flat_map(|(_, k)| k.iter().copied())
        .collect();
    listed.sort_unstable();
    let mut all = KINDS.to_vec();
    all.sort_unstable();
    assert_eq!(listed, all);
}

#[test]
fn edited_values_become_actions_and_bad_numbers_are_errors() {
    let mut e = ActionEdit::new(&default_action("notify"));
    e.fields[1] = TextEdit::new("T");
    e.fields[2] = TextEdit::new("{msg}");
    e.cycle(0);
    e.cycle(0);
    e.cycle(3);
    assert_eq!(
        e.to_action().unwrap(),
        Action::Notify {
            level: Level::Error,
            title: "T".into(),
            message: "{msg}".into(),
            desktop: true
        }
    );
    let mut h = ActionEdit::new(&default_action("highlight"));
    h.fields[3] = TextEdit::new("soon");
    assert!(h.to_action().unwrap_err().contains("whole number"));
    h.fields[3] = TextEdit::new("");
    assert!(h.to_action().is_ok(), "empty means 0");
    let mut s = ActionEdit::new(&default_action("status"));
    s.fields[1] = TextEdit::new("42");
    assert!(matches!(
        s.to_action().unwrap(),
        Action::Status {
            progress: Some(Progress::Value(42)),
            ..
        }
    ));
    s.fields[1] = TextEdit::new("{pct}");
    assert!(matches!(
        s.to_action().unwrap(),
        Action::Status {
            progress: Some(Progress::Template(_)),
            ..
        }
    ));
    // Cycling a text parameter does nothing.
    let mut c = ActionEdit::new(&default_action("tab_color"));
    c.cycle(0);
    assert_eq!(c.fields[0].text(), "red");
    // Missing values fall back to defaults instead of failing.
    for k in KINDS {
        assert!(action_from(k, &[]).is_ok(), "{k}");
    }
    assert!(action_from("exec", &[]).is_err());
}

fn text_editor() -> Editor {
    let mut r = Rule::new_text("Build failed", r"error: (?P<msg>.+)");
    r.id = "t_1".into();
    r.actions = vec![default_action("notify"), default_action("tab_color")];
    Editor::open(r, Some("t_1".into()), Base::new("", None))
}

#[test]
fn an_untouched_editor_is_clean_and_describes_its_rule() {
    let e = text_editor();
    assert!(!e.dirty());
    assert_eq!(e.to_rule().unwrap(), e.original);
    assert!(e.diagnostics().is_empty() && e.save_blocker().is_none());
}

#[test]
fn edits_make_it_dirty_and_blank_is_invalid_until_filled() {
    let mut e = text_editor();
    e.name = TextEdit::new("Renamed");
    assert!(e.dirty());
    let b = Editor::blank();
    assert!(b.save_blocker().is_some(), "name and regex are required");
    assert!(b.issue_at(Field::Name).is_some() && b.issue_at(Field::Regex).is_some());
    let mut b = b;
    b.name = TextEdit::new("X");
    b.regex = TextEdit::new("x");
    assert!(b.save_blocker().is_none());
    assert!(b.dirty());
}

#[test]
fn inline_errors_name_the_field() {
    let mut e = text_editor();
    e.regex = TextEdit::new("(?=x)");
    assert!(e.issue_at(Field::Regex).unwrap().contains("lookahead"));
    e.regex = TextEdit::new("error");
    e.panes = TextEdit::new("nokey:x");
    assert!(e
        .issue_at(Field::Panes)
        .unwrap()
        .contains("unknown selector key"));
    e.panes = TextEdit::new("");
    e.cooldown = TextEdit::new("soon");
    assert!(e.save_blocker().unwrap().contains("cooldown"));
    e.cooldown = TextEdit::new("");
    assert_eq!(
        e.to_rule().unwrap().cooldown_ms,
        1000,
        "empty is the default"
    );
    e.actions[0].fields[2] = TextEdit::new("{nope}");
    assert!(e
        .issue_at(Field::Action(0, "message"))
        .unwrap()
        .contains("unknown placeholder"));
}

#[test]
fn the_placeholder_list_follows_the_regex_captures() {
    let mut e = text_editor();
    let p = e.placeholders();
    assert!(
        p.contains(&"msg".to_string())
            && p.contains(&"1".to_string())
            && p.contains(&"line".to_string())
    );
    assert!(!p.contains(&"event".to_string()));
    e.set_text_kind(false);
    let p = e.placeholders();
    assert!(
        p.contains(&"event".to_string())
            && p.contains(&"exit".to_string())
            && !p.contains(&"msg".to_string())
    );
}

#[test]
fn text_and_event_switch_keeps_both_halves_and_focus_valid() {
    let mut e = text_editor();
    e.focus = Focus::Regex;
    e.set_text_kind(false);
    assert_eq!(e.focus, Focus::Name);
    e.set_event(EventKind::CommandFinished);
    e.exit = TextEdit::new("failure");
    assert_eq!(
        e.to_rule().unwrap().when,
        When::Event {
            event: EventKind::CommandFinished,
            exit: ExitFilter::Failure
        }
    );
    e.exit = TextEdit::new("7");
    assert_eq!(
        e.to_rule().unwrap().when,
        When::Event {
            event: EventKind::CommandFinished,
            exit: ExitFilter::Code(7)
        }
    );
    e.exit = TextEdit::new("maybe");
    assert!(e.to_rule().is_err());
    e.set_event(EventKind::Bell);
    assert_eq!(
        e.exit.text(),
        "any",
        "no exit filter on events without an exit"
    );
    assert!(!e.fields().contains(&Focus::Exit));
    e.set_text_kind(true);
    assert_eq!(
        e.to_rule().unwrap().when,
        When::Text {
            regex: "error: (?P<msg>.+)".into(),
            case_insensitive: false
        }
    );
}

#[test]
fn actions_are_added_removed_and_reordered_up_to_the_limit() {
    let mut e = text_editor();
    e.add_action("bell");
    e.add_action("marker");
    assert_eq!(e.actions.len(), 4);
    e.add_action("counter");
    assert_eq!(e.actions.len(), MAX_ACTIONS, "the fifth is refused");
    e.move_action(0, false);
    assert_eq!(e.actions[0].kind, "tab_color");
    e.move_action(0, true);
    assert_eq!(e.actions[0].kind, "tab_color", "the top one cannot move up");
    e.move_action(3, false);
    assert_eq!(e.actions[3].kind, "marker");
    e.remove_action(1);
    assert_eq!(e.actions.len(), 3);
    assert_eq!(e.to_rule().unwrap().actions.len(), 3);
}

#[test]
fn tab_walks_every_text_field_and_skips_choices_and_flags() {
    let mut e = text_editor();
    let f = e.fields();
    assert_eq!(
        &f[..4],
        &[Focus::Name, Focus::Panes, Focus::Regex, Focus::Cooldown]
    );
    // notify: title, message are text (0 level, 3 desktop are not); tab_color: color, ttl_s.
    assert_eq!(
        &f[4..],
        &[
            Focus::Param(0, 1),
            Focus::Param(0, 2),
            Focus::Param(1, 0),
            Focus::Param(1, 1)
        ]
    );
    let n = f.len();
    for _ in 0..n {
        e.focus_next(false);
    }
    assert_eq!(e.focus, Focus::Name, "wraps around");
    e.focus_next(true);
    assert_eq!(e.focus, *f.last().unwrap());
    e.focus = Focus::Param(0, 2);
    e.line_edit().unwrap().insert("!");
    assert!(e.actions[0].fields[2].text().ends_with('!'));
}

#[test]
fn panes_default_is_every_pane_and_round_trips_through_the_field() {
    let mut e = text_editor();
    assert_eq!(e.panes.text(), "");
    assert_eq!(e.to_rule().unwrap().panes, "*");
    e.panes = TextEdit::new("domain:local");
    assert_eq!(e.to_rule().unwrap().panes, "domain:local");
}

#[test]
fn the_placeholder_list_belongs_to_template_fields_only() {
    let mut e = Editor::open(Rule::new_text("r", "x"), None, Base::new("", None));
    e.add_action("tab_color");
    e.add_action("notify");
    let (tab, notify) = (e.actions.len() - 2, e.actions.len() - 1);
    e.focus = Focus::Param(tab, 0); // the color
    assert!(!e.focus_is_template());
    e.focus = Focus::Param(notify, 1); // the title
    assert!(e.focus_is_template());
    e.focus = Focus::Param(notify, 0); // the level (a choice)
    assert!(!e.focus_is_template());
    e.focus = Focus::Name;
    assert!(!e.focus_is_template());
}

#[test]
fn an_event_rules_highlight_marks_the_line_and_cannot_switch_to_match() {
    let mut e = Editor::open(Rule::new_text("r", "x"), None, Base::new("", None));
    assert_eq!(e.actions[0].kind, "highlight");
    e.set_text_kind(false);
    assert_eq!(e.actions[0].fields[0].text(), "line");
    e.cycle_param(0, 0);
    assert_eq!(e.actions[0].fields[0].text(), "line");
    e.add_action("highlight");
    assert_eq!(e.actions[1].fields[0].text(), "line");
    e.set_text_kind(true);
    e.cycle_param(0, 0);
    assert_eq!(e.actions[0].fields[0].text(), "match");
}
