use super::*;

pub const SPEC_FILE: &str = r#"version = 1

[[trigger]]
id = "t_3f9a1c"                    # generated, stable, never reused
name = "Build failed"
enabled = true
panes = "domain:local"
cooldown_ms = 1000

[trigger.when]
kind = "text"
regex = 'error(\[E\d+\])?: (?P<msg>.+)'
case_insensitive = false

[[trigger.actions]]
kind = "highlight"
scope = "line"
color = "red"

[[trigger.actions]]
kind = "notify"
level = "error"
title = "Build failed"
message = "{msg}"

[[trigger]]
id = "t_77be02"
name = "Long command finished"
panes = "!focused:true"
[trigger.when]
kind = "event"
event = "command_finished"
exit = "any"
[[trigger.actions]]
kind = "notify"
message = "{title} exited with {exit}"
desktop = true
"#;

fn err(src: &str) -> Vec<Diagnostic> {
    load_str(src, LoadOpts::default()).expect_err("should fail")
}

fn one_rule(when: &str, actions: &str) -> String {
    format!("[[trigger]]\nname = \"r\"\n[trigger.when]\n{when}\n{actions}\n")
}

const TEXT: &str = "kind = \"text\"\nregex = 'a(b)(?P<n>c)'";
const HL: &str = "[[trigger.actions]]\nkind = \"highlight\"\ncolor = \"red\"";

#[test]
fn spec_example_loads() {
    let l = load_str(SPEC_FILE, LoadOpts::default()).unwrap();
    assert_eq!(l.rules.len(), 2);
    assert_eq!(l.rules[0].id, "t_3f9a1c");
    assert_eq!(l.rules[0].actions.len(), 2);
    assert!(matches!(
        l.rules[1].when,
        When::Event {
            event: EventKind::CommandFinished,
            ..
        }
    ));
}

#[test]
fn empty_and_missing_files_are_no_rules() {
    assert!(load_str("", LoadOpts::default()).unwrap().rules.is_empty());
    let dir = tempfile::tempdir().unwrap();
    assert!(load(&dir.path().join("none.toml"))
        .unwrap()
        .rules
        .is_empty());
}

#[test]
fn missing_ids_are_derived_and_unique() {
    let src = format!("{}\n{}", one_rule(TEXT, HL), one_rule(TEXT, HL));
    let l = load_str(&src, LoadOpts::default()).unwrap();
    assert!(l.rules[0].id.starts_with("t_") && l.rules[0].id != l.rules[1].id);
}

#[test]
fn syntax_error_has_line_and_col() {
    let d = err("[[trigger]\nname = 1");
    assert_eq!(d[0].line, 1);
    assert!(d[0].render("triggers.toml").starts_with("triggers.toml:1:"));
}

#[test]
fn unknown_keys_and_kinds_are_errors() {
    assert!(!err("bogus = 1").is_empty());
    let src = one_rule(
        TEXT,
        "[[trigger.actions]]\nkind = \"exec\"\ncommand = \"rm\"",
    );
    assert!(!err(&src).is_empty());
    let src = one_rule(TEXT, "[[trigger.actions]]\nkind = \"bell\"\nvolume = 3");
    assert!(!err(&src).is_empty());
}

#[test]
fn version_must_be_one() {
    let d = err(&format!("version = 2\n{}", one_rule(TEXT, HL)));
    assert_eq!((d[0].line, d[0].col), (1, 11));
    assert!(d[0].message.contains("unsupported version"));
}

fn at_line(d: &[Diagnostic], needle: &str) -> usize {
    d.iter()
        .find(|x| x.message.contains(needle))
        .unwrap_or_else(|| panic!("{needle}: {d:?}"))
        .line
}

#[test]
fn regex_errors_have_positions_and_clear_messages() {
    let src = one_rule("kind = \"text\"\nregex = '(?=x)'", HL);
    let d = err(&src);
    assert_eq!(at_line(&d, "lookahead"), 5);
    let src = one_rule("kind = \"text\"\nregex = '(a)\\1'", HL);
    assert_eq!(at_line(&err(&src), "backreferences"), 5);
    let src = one_rule("kind = \"text\"\nregex = '(unclosed'", HL);
    assert_eq!(at_line(&err(&src), "invalid regex"), 5);
    let src = one_rule("kind = \"text\"\nregex = ''", HL);
    assert_eq!(at_line(&err(&src), "must not be empty"), 5);
}

#[test]
fn regex_size_limit_is_enforced() {
    let src = one_rule("kind = \"text\"\nregex = '(\\w{100}){100}'", HL);
    assert!(err(&src).iter().any(|d| d.message.contains("too complex")));
    let long = "a".repeat(1025);
    let src = one_rule(&format!("kind = \"text\"\nregex = '{long}'"), HL);
    assert!(err(&src)
        .iter()
        .any(|d| d.message.contains("longer than 1024")));
}

#[test]
fn name_cooldown_and_panes_are_checked() {
    let src = format!("[[trigger]]\nname = \"{}\"\ncooldown_ms = 61000\npanes = \"nokey:x\"\n[trigger.when]\n{TEXT}\n{HL}\n", "n".repeat(61));
    let d = err(&src);
    assert_eq!(at_line(&d, "longer than 60"), 2);
    assert_eq!(at_line(&d, "cooldown_ms"), 3);
    assert_eq!(at_line(&d, "unknown selector key"), 4);
    // `tab:` keys do not apply to a pane selector? they do; `workspace:` is fine, `id:` is fine.
    let ok = format!("[[trigger]]\nname = \"x\"\npanes = \"cmd:ssh*,!focused:true\"\n[trigger.when]\n{TEXT}\n{HL}\n");
    assert!(load_str(&ok, LoadOpts::default()).is_ok());
}

#[test]
fn action_count_and_singletons() {
    let five = (0..5).map(|_| HL).collect::<Vec<_>>().join("\n");
    assert!(err(&one_rule(TEXT, &five))
        .iter()
        .any(|d| d.message.contains("at most 4 actions")));
    let two_titles = "[[trigger.actions]]\nkind = \"title\"\ntemplate = \"a\"\n[[trigger.actions]]\nkind = \"title\"\ntemplate = \"b\"";
    let d = err(&one_rule(TEXT, two_titles));
    assert_eq!(at_line(&d, "only one title"), 10);
    let two_colors = "[[trigger.actions]]\nkind = \"tab_color\"\ncolor = \"red\"\n[[trigger.actions]]\nkind = \"tab_color\"\ncolor = \"blue\"";
    assert!(err(&one_rule(TEXT, two_colors))
        .iter()
        .any(|d| d.message.contains("only one tab_color")));
    // Repeating a combinable kind is fine.
    assert!(load_str(&one_rule(TEXT, &format!("{HL}\n{HL}")), LoadOpts::default()).is_ok());
    assert!(
        err("[[trigger]]\nname = \"x\"\n[trigger.when]\nkind = \"text\"\nregex = \"a\"\n")
            .iter()
            .any(|d| d.message.contains("at least one action"))
    );
}

#[test]
fn placeholders_are_checked_against_the_regex() {
    let act = |m: &str| format!("[[trigger.actions]]\nkind = \"notify\"\nmessage = \"{m}\"");
    assert!(load_str(
        &one_rule(
            TEXT,
            &act("{1} {2} {n} {match} {line} {rule} {pane} {title} {cwd} {domain}")
        ),
        LoadOpts::default()
    )
    .is_ok());
    let d = err(&one_rule(TEXT, &act("{3}")));
    assert_eq!(at_line(&d, "unknown placeholder {3}"), 8);
    assert!(err(&one_rule(TEXT, &act("{nope}")))
        .iter()
        .any(|d| d.message.contains("available:")));
    assert!(err(&one_rule(TEXT, &act("{event}")))
        .iter()
        .any(|d| d.message.contains("{event}")));
    assert!(err(&one_rule(TEXT, &act("{oops")))
        .iter()
        .any(|d| d.message.contains("unterminated")));
    assert!(load_str(
        &one_rule(TEXT, &act("literal {{brace")),
        LoadOpts::default()
    )
    .is_ok());
    let ev = "kind = \"event\"\nevent = \"bell\"";
    assert!(load_str(
        &one_rule(ev, &act("{event} {exit} {title}")),
        LoadOpts::default()
    )
    .is_ok());
    assert!(err(&one_rule(ev, &act("{match}")))
        .iter()
        .any(|d| d.message.contains("{match}")));
    assert!(err(&one_rule(ev, &act("{1}")))
        .iter()
        .any(|d| d.message.contains("{1}")));
}

#[test]
fn template_length_limit() {
    let long = "x".repeat(257);
    let act = format!("[[trigger.actions]]\nkind = \"notify\"\nmessage = \"{long}\"");
    assert!(err(&one_rule(TEXT, &act))
        .iter()
        .any(|d| d.message.contains("longer than 256")));
}

#[test]
fn match_scope_is_text_only() {
    let ev = "kind = \"event\"\nevent = \"bell\"";
    let m = "[[trigger.actions]]\nkind = \"highlight\"\nscope = \"match\"\ncolor = \"red\"";
    let d = err(&one_rule(ev, m));
    assert_eq!(at_line(&d, "needs a text rule"), 8);
    let l = "[[trigger.actions]]\nkind = \"highlight\"\nscope = \"line\"\ncolor = \"red\"";
    assert!(load_str(&one_rule(ev, l), LoadOpts::default()).is_ok());
    // Default scope is `match`, so an event rule must say `line`.
    assert!(!err(&one_rule(ev, HL)).is_empty());
}

#[test]
fn exit_filter_forms() {
    let mk = |e: &str| {
        format!("[[trigger]]\nname = \"r\"\n[trigger.when]\nkind = \"event\"\nevent = \"command_finished\"\nexit = {e}\n[[trigger.actions]]\nkind = \"bell\"\n")
    };
    for (txt, f) in [
        ("\"any\"", ExitFilter::Any),
        ("\"success\"", ExitFilter::Success),
        ("\"failure\"", ExitFilter::Failure),
        ("7", ExitFilter::Code(7)),
    ] {
        let l = load_str(&mk(txt), LoadOpts::default()).unwrap();
        assert!(matches!(&l.rules[0].when, When::Event { exit, .. } if *exit == f));
    }
    assert!(!err(&mk("\"maybe\"")).is_empty());
    let bad = "[[trigger]]\nname = \"r\"\n[trigger.when]\nkind = \"event\"\nevent = \"bell\"\nexit = \"failure\"\n[[trigger.actions]]\nkind = \"bell\"\n";
    assert!(err(bad)
        .iter()
        .any(|d| d.message.contains("exit only applies")));
    assert!(ExitFilter::Failure.matches(Some(2)) && !ExitFilter::Failure.matches(Some(0)));
    assert!(ExitFilter::Success.matches(Some(0)) && ExitFilter::Code(3).matches(Some(3)));
}

#[test]
fn colors_and_numeric_limits() {
    let bad = "[[trigger.actions]]\nkind = \"highlight\"\nscope = \"line\"\ncolor = \"mauve\"";
    assert_eq!(at_line(&err(&one_rule(TEXT, bad)), "not a color"), 9);
    for ok in [
        "accent", "warning", "error", "success", "info", "red", "#ff0000",
    ] {
        let a = format!("[[trigger.actions]]\nkind = \"highlight\"\ncolor = \"{ok}\"");
        assert!(
            load_str(&one_rule(TEXT, &a), LoadOpts::default()).is_ok(),
            "{ok}"
        );
    }
    let snap = "[[trigger.actions]]\nkind = \"snapshot\"\nbefore = 51";
    assert!(!err(&one_rule(TEXT, snap)).is_empty());
    let shot = "[[trigger.actions]]\nkind = \"screenshot\"\nscale = 3.0";
    assert!(!err(&one_rule(TEXT, shot)).is_empty());
}

#[test]
fn clipboard_source_must_exist() {
    let ok = "[[trigger.actions]]\nkind = \"clipboard\"\nsource = \"n\"";
    assert!(load_str(&one_rule(TEXT, ok), LoadOpts::default()).is_ok());
    let ok = "[[trigger.actions]]\nkind = \"clipboard\"\nsource = \"2\"";
    assert!(load_str(&one_rule(TEXT, ok), LoadOpts::default()).is_ok());
    let bad = "[[trigger.actions]]\nkind = \"clipboard\"\nsource = \"3\"";
    assert!(!err(&one_rule(TEXT, bad)).is_empty());
}

#[test]
fn rule_count_limit_and_duplicate_ids() {
    let many: String = (0..3).map(|_| one_rule(TEXT, HL)).collect();
    let d = load_str(&many, LoadOpts { max_rules: 2 }).unwrap_err();
    assert!(d
        .iter()
        .any(|d| d.message.contains("too many rules") && d.line > 1));
    let dup = "[[trigger]]\nid = \"a\"\nname = \"x\"\n[trigger.when]\nkind = \"text\"\nregex = \"x\"\n[[trigger.actions]]\nkind = \"bell\"\n";
    let src = format!("{dup}{dup}");
    assert!(err(&src).iter().any(|d| d.message.contains("duplicate id")));
    let bad = dup.replace("\"a\"", "\"a b\"");
    assert!(err(&bad).iter().any(|d| d.message.contains("must be 1-32")));
}

#[test]
fn new_ids_avoid_used() {
    let mut used = HashSet::new();
    for _ in 0..50 {
        let id = new_id(&used);
        assert!(id.starts_with("t_") && id.len() == 8 && valid_id(&id));
        assert!(used.insert(id));
    }
}

#[test]
fn scope_defaults_to_output_accepts_any_and_is_text_only() {
    let l = load_str(&one_rule(TEXT, HL), LoadOpts::default()).unwrap();
    assert_eq!(l.rules[0].scope, MatchIn::Output);
    let any = format!("[[trigger]]\nname = \"r\"\nscope = \"any\"\n[trigger.when]\n{TEXT}\n{HL}\n");
    assert_eq!(
        load_str(&any, LoadOpts::default()).unwrap().rules[0].scope,
        MatchIn::Any
    );
    let bad = any.replace("\"any\"", "\"everywhere\"");
    assert!(!err(&bad).is_empty());
    let ev = "[[trigger]]\nname = \"r\"\nscope = \"any\"\n[trigger.when]\nkind = \"event\"\nevent = \"bell\"\n[[trigger.actions]]\nkind = \"bell\"\n";
    let d = err(ev);
    assert_eq!(d[0].line, 3);
    assert!(d[0].message.contains("scope only applies to text rules"));
}
