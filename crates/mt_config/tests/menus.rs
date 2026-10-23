//! `[context_menu]`, `[menu_groups]` and `[menu_security]` (SPEC §10.9): parsing, resolution and
//! golden per-menu diagnostics.

use mt_config::menu::{self, Confirm, Entry, MenuMode, SpawnWhere, Trigger};
use mt_config::*;

fn errors(src: &str) -> Vec<String> {
    parse_str(src)
        .expect_err("should fail")
        .iter()
        .map(|d| d.render("microterm.toml"))
        .collect()
}

fn ids(cfg: &Config, m: &str) -> Vec<String> {
    menu::entries(cfg, m)
        .iter()
        .map(|e| e.id().to_string())
        .collect()
}

#[test]
fn items_of_every_kind_parse() {
    let src = r#"
[context_menu.viewport]
mode = "prepend"
max_rows = 12
trigger = "shift_right_click"

[[context_menu.viewport.items]]
id = "blame"
label = "git log -S {selection}"
icon = "search"
when = "selection, local"
when_false = "disable"
confirm = "auto"
spawn = { command = ["git", "log", "-S", "{selection}"], cwd = "{cwd}", where = "split-right", close_on_success = true }

[[context_menu.viewport.items]]
label = "Status"
send = "git status\n"
quote = "none"
confirm = true

[[context_menu.viewport.items]]
type = "separator"

[[context_menu.viewport.items]]
label = "Agent"
ctl = "pane status set --state waiting review"

[[context_menu.viewport.items]]
label = "Layout"
submenu = [
  { label = "Zoom", action = "TogglePaneZoom" },
  { label = "Split left", action = { Split = "left" } },
]

[[context_menu.tab.items]]
include = "layout"

[[menu_groups.layout.items]]
label = "Rotate"
action = "RotatePanes"

[menu_security]
dangerous = ["rm -rf"]
"#;
    let l = parse_str(src).unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let c = &l.config;
    let vp = &c.context_menu["viewport"];
    assert_eq!(vp.mode, MenuMode::Prepend);
    assert_eq!(vp.trigger, Some(Trigger::ShiftRightClick));
    assert_eq!(vp.items[0].confirm, Some(Confirm::Auto));
    let sp = vp.items[0].spawn.as_ref().unwrap();
    assert_eq!(sp.place, SpawnWhere::SplitRight);
    assert!(sp.close_on_success && !sp.ctl);
    assert_eq!(vp.items[1].confirm, Some(Confirm::Always));
    assert_eq!(vp.items[3].kind(), Some(menu::Kind::Ctl));
    assert_eq!(vp.items[4].submenu.as_ref().unwrap().len(), 2);
    // Prepend puts user rows first; ids default to label slugs.
    let v = ids(c, "viewport");
    assert_eq!(&v[..5], ["blame", "status", "item_2", "agent", "layout"]);
    assert_eq!(v[5], "copy");
    // `include` expands the group in place, after the built-ins.
    assert_eq!(
        ids(c, "tab"),
        ["close", "close_others", "rename", "color", "rotate"]
    );
    assert_eq!(c.menu_security.dangerous, ["rm -rf"]);
    // The whole config round-trips.
    let back = parse_str(&toml::to_string(c).unwrap()).unwrap();
    assert_eq!(&back.config, c);
}

#[test]
fn builtins_are_on_by_default_with_the_panes_group_inlined() {
    let c = Config::default();
    assert_eq!(
        ids(&c, "viewport"),
        [
            "copy",
            "paste",
            "select_all",
            "open_link",
            "copy_link",
            "search",
            "save_as_snippet",
            "toggle_recording",
            "recordings",
            "clear_scrollback",
            "panes",
            "zoom",
            "swap",
            "rotate",
            "split_left",
            "split_right",
            "split_up",
            "split_down",
            "close_pane",
        ]
    );
    let all = menu::entries(&c, "viewport");
    let swap = menu::find_entry(&all, "swap").unwrap();
    let kids: Vec<_> = swap.children().iter().map(|e| e.id().to_string()).collect();
    assert_eq!(kids, ["swap_left", "swap_right", "swap_up", "swap_down"]);
    assert!(matches!(
        menu::find_entry(&all, "swap_up"),
        Some(Entry::Builtin(b)) if b.chip == Some("SwapPane(up)")
    ));
    assert!(menu::find_entry(&all, "nope").is_none());
}

#[test]
fn builtin_prunes_and_reorders_and_replace_drops_them() {
    let c = parse_str(
        "[context_menu.viewport]\nbuiltin = [\"paste\", \"copy\"]\n[context_menu.tab]\nmode = \"replace\"\n[[context_menu.tab.items]]\nlabel = \"Hi\"\nsend = \"hi\"\n",
    )
    .unwrap()
    .config;
    assert_eq!(ids(&c, "viewport"), ["paste", "copy"]);
    assert_eq!(ids(&c, "tab"), ["hi"]);
    let c = parse_str("[context_menu.tab]\nenabled = false\n")
        .unwrap()
        .config;
    assert!(ids(&c, "tab").is_empty());
}

#[test]
fn right_click_alias_and_trigger() {
    let c = parse_str("[mouse]\nright_click = \"paste\"\n")
        .unwrap()
        .config;
    assert_eq!(menu::viewport_trigger(&c), Trigger::Paste);
    let l = parse_str(
        "[mouse]\nright_click = \"none\"\n[context_menu.viewport]\ntrigger = \"right_click\"\n",
    )
    .unwrap();
    assert_eq!(
        menu::viewport_trigger(&l.config),
        Trigger::RightClick,
        "the new key wins"
    );
    assert_eq!(
        l.warnings[0].render("microterm.toml"),
        "microterm.toml:2:15 — mouse.right_click is ignored because context_menu.viewport.trigger is set"
    );
    assert_eq!(
        menu::trigger(&Config::default(), "tab"),
        Trigger::RightClick
    );
}

#[test]
fn unknown_keys_inside_menus_warn() {
    let l = parse_str("[context_menu.viewport]\nmod = \"append\"\n[[context_menu.viewport.items]]\nlabel = \"x\"\nsend = \"x\"\nwhne = \"selection\"\n").unwrap();
    let w: Vec<_> = l.warnings.iter().map(|d| d.render("f")).collect();
    assert_eq!(
        w,
        [
            "f:2:1 — unknown key \"context_menu.viewport.mod\" (did you mean \"context_menu.viewport.mode\"?)",
            "f:6:1 — unknown key \"context_menu.viewport.items.whne\" (did you mean \"context_menu.viewport.items.when\"?)",
        ]
    );
}

#[test]
fn golden_unknown_menu_id() {
    assert_eq!(
        errors("[context_menu.viewprot]\nmode = \"append\"\n"),
        ["microterm.toml:1:15 — context_menu.viewprot: unknown menu \"viewprot\" (did you mean \"viewport\"?); menus: viewport, tab"]
    );
}

#[test]
fn golden_placeholder_not_in_menu() {
    assert_eq!(
        errors("[[context_menu.tab.items]]\nlabel = \"Grep\"\nspawn = { command = [\"rg\", \"{selection}\"] }\n"),
        ["microterm.toml:3:1 — context_menu.tab.items[0]: placeholder {selection} is not available in menu \"tab\"; available: {pane} {tab} {window} {title} {cwd} {domain} {host} {last_command} {last_output} {exit_code}"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x {selction}\"\nsend = \"x\"\n"),
        ["microterm.toml:2:1 — context_menu.viewport.items[0]: unknown placeholder {selction} (did you mean \"selection\"?)"]
    );
}

#[test]
fn golden_when_errors() {
    assert_eq!(
        errors("[[context_menu.tab.items]]\nlabel = \"x\"\nsend = \"x\"\nwhen = \"selection\"\n"),
        ["microterm.toml:4:1 — context_menu.tab.items[0]: when key \"selection\" is not available in menu \"tab\"; available: title cwd cmd domain status exited focused panes tabs zoomed remote local last_exit idle running"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x\"\nsend = \"x\"\nwhen = \"panes>1,nope\"\n"),
        ["microterm.toml:4:1 — context_menu.viewport.items[0]: when: column 9: unknown key \"nope\" (keys: title, cwd, cmd, domain, status, exited, focused, link, selection, word, line, on_prompt, in_output, alt_screen, mouse_reporting, panes, tabs, zoomed, remote, local, last_exit, idle, running)"]
    );
}

#[test]
fn golden_kind_errors() {
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x\"\n"),
        ["microterm.toml:2:1 — context_menu.viewport.items[0]: needs one of action, send, spawn, ctl, submenu, type or include"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x\"\nsend = \"a\"\nctl = \"pane zoom\"\n"),
        ["microterm.toml:4:1 — context_menu.viewport.items[0]: has more than one kind (send, ctl)"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nsend = \"a\"\n"),
        ["microterm.toml:2:1 — context_menu.viewport.items[0]: needs a label"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x\"\nspawn = { command = [] }\n"),
        ["microterm.toml:3:1 — context_menu.viewport.items[0]: spawn.command must be a non-empty array"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x\"\naction = \"Copy\"\nquote = \"sh\"\n"),
        ["microterm.toml:2:1 — context_menu.viewport.items[0]: quote and multiline apply to send items only"]
    );
}

#[test]
fn golden_submenu_include_and_icon_errors() {
    let e = errors(
        "[[context_menu.viewport.items]]\nlabel = \"S\"\nsubmenu = [ { label = \"T\", submenu = [] } ]\n",
    );
    assert_eq!(
        e,
        ["microterm.toml:3:28 — context_menu.viewport.items[0].submenu[0]: submenus nest one level only"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\ninclude = \"layuot\"\n[[menu_groups.layout.items]]\nlabel = \"Z\"\naction = \"TogglePaneZoom\"\n"),
        ["microterm.toml:2:1 — context_menu.viewport.items[0]: unknown menu group \"layuot\" (did you mean \"layout\"?)"]
    );
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x\"\nsend = \"x\"\nicon = \"serach\"\n"),
        ["microterm.toml:4:1 — context_menu.viewport.items[0]: unknown icon \"serach\" (did you mean \"search\"?); run `microterm ctl menu icons` for the list"]
    );
}

#[test]
fn unknown_icons_without_a_close_match_point_at_the_list() {
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"x\"\nsend = \"x\"\nicon = \"rocket\"\n"),
        ["microterm.toml:4:1 — context_menu.viewport.items[0]: unknown icon \"rocket\"; run `microterm ctl menu icons` for the list"]
    );
}

#[test]
fn group_items_are_checked_against_the_including_menu() {
    let src = "[[menu_groups.g.items]]\nlabel = \"x\"\nsend = \"{selection}\"\n[[context_menu.viewport.items]]\ninclude = \"g\"\n";
    assert!(parse_str(src).is_ok(), "fine in viewport");
    let src = "[[menu_groups.g.items]]\nlabel = \"x\"\nsend = \"{selection}\"\n[[context_menu.tab.items]]\ninclude = \"g\"\n";
    assert_eq!(
        errors(src),
        ["microterm.toml:3:1 — menu_groups.g.items[0] (included in context_menu.tab): placeholder {selection} is not available in menu \"tab\"; available: {pane} {tab} {window} {title} {cwd} {domain} {host} {last_command} {last_output} {exit_code}"]
    );
}

#[test]
fn golden_ids_limits_and_settings() {
    assert_eq!(
        errors("[[context_menu.viewport.items]]\nlabel = \"Copy\"\nsend = \"x\"\n"),
        ["microterm.toml:2:1 — context_menu.viewport: duplicate item id \"copy\" (set `id` to tell items apart)"]
    );
    let mut src = String::new();
    for i in 0..60 {
        src.push_str(&format!(
            "[[context_menu.viewport.items]]\nlabel = \"i{i}\"\nsend = \"x\"\n"
        ));
    }
    let e = errors(&src);
    assert_eq!(e.len(), 1);
    assert!(
        e[0].ends_with("context_menu.viewport: 83 items after include; the limit is 64"),
        "{}",
        e[0]
    );
    assert_eq!(
        errors("[context_menu.tab]\ntrigger = \"paste\"\n"),
        ["microterm.toml:2:11 — context_menu.tab.trigger = \"paste\" is only valid for the viewport menu"]
    );
    assert_eq!(
        errors("[context_menu.viewport]\nbuiltin = [\"cpoy\"]\n"),
        ["microterm.toml:2:11 — context_menu.viewport.builtin: unknown built-in \"cpoy\" (did you mean \"copy\"?); built-ins: copy, paste, select_all, open_link, copy_link, search, save_as_snippet, toggle_recording, recordings, clear_scrollback, panes"]
    );
    assert_eq!(
        errors("[context_menu.viewport]\nmax_rows = 2\n"),
        ["microterm.toml:2:12 — context_menu.viewport.max_rows must be between 4 and 64"]
    );
    assert_eq!(
        errors("[menu_security]\nconfirm_panes = [\"tilte:x\"]\n"),
        ["microterm.toml:2:17 — menu_security.confirm_panes[0]: unknown selector key \"tilte\" (keys: id, title, cwd, cmd, domain, workspace, window, tab, focused, active, status, exited, color)"]
    );
}

#[test]
fn type_errors_in_items_are_errors() {
    let e = errors(
        "[[context_menu.viewport.items]]\nlabel = \"x\"\nsend = \"x\"\nconfirm = \"maybe\"\n",
    );
    assert!(
        e[0].contains("confirm must be true, false or \"auto\""),
        "{e:?}"
    );
    let e = errors("[[context_menu.viewport.items]]\nlabel = \"x\"\nspawn = { command = [\"a\"], where = \"left\" }\n");
    assert!(e[0].contains("unknown variant `left`"), "{e:?}");
}

#[test]
fn trusted_dirs_must_be_absolute_without_wildcards() {
    let cfg: mt_config::Config = toml::from_str(
        "[menu_security]\ntrusted_dirs = [\"/ok/dir\", \"rel\", \"/a/*/b\", \"/p/**\"]\nallow_remote_items = true\n",
    )
    .unwrap();
    assert!(cfg.menu_security.allow_remote_items);
    let msgs: Vec<String> = mt_config::menu::validate(&cfg)
        .into_iter()
        .map(|i| i.message)
        .collect();
    assert_eq!(msgs.len(), 2, "{msgs:?}");
    assert!(msgs[0].contains("trusted_dirs[1]") && msgs[0].contains("absolute"));
    assert!(msgs[1].contains("trusted_dirs[2]"));
}
