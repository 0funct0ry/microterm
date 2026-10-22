use mt_config::*;

fn err(src: &str) -> Vec<Diagnostic> {
    parse_str(src).expect_err("should fail")
}

#[test]
fn defaults_serialize_and_round_trip() {
    let d = Config::default();
    let s = toml::to_string(&d).expect("serialize");
    let back = parse_str(&s).expect("reparse");
    assert_eq!(back.config, d);
    assert!(back.warnings.is_empty(), "{:?}", back.warnings);
}

#[test]
fn every_section_round_trips_with_values() {
    let src = r##"
shell = "/bin/zsh"
term = "xterm"
scrollback_lines = 500
new_tab_cwd = "home"
confirm_close = "always"
theme = { dark = "Nord", light = "Dayfox" }
[env]
A = "b"
[font]
family = "Iosevka"
size = 15.5
weight = 300
bold_weight = "black"
features = ["zero"]
fallback = ["Noto Sans CJK JP"]
[window]
padding = { x = 1, y = 2 }
opacity = 0.5
decorations = "none"
initial_size = { cols = 80, rows = 24 }
[tab_bar]
position = "bottom"
hide_if_single = true
show_index = true
[cursor]
style = "beam"
blink = false
blink_interval_ms = 700
[scrollbar]
mode = "never"
[mouse]
right_click = "paste"
scroll_multiplier = 5
[clipboard]
osc52 = "read-write"
[bell]
mode = "none"
[hyperlinks]
modifier = "ctrl"
[[hyperlink_rules]]
regex = 'PAY-\d+'
format = "https://x/$0"
[[keys]]
key = "ctrl+shift+r"
action = "ReloadConfig"
[[launch]]
name = "htop"
command = ["htop"]
[[workspaces]]
name = "api"
tabs = [ { title = "s", cwd = "~", command = ["a"] } ]
[[ssh_domains]]
name = "prod"
host = "h"
port = 2222
"##;
    let l = parse_str(src).expect("parses");
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let c = l.config;
    assert_eq!(c.theme.name_for(false), "Dayfox");
    assert_eq!(c.font.weight.numeric(), Some(300));
    assert_eq!(c.window.padding.x, 1.0);
    assert_eq!(c.workspaces[0].tabs[0].command, vec!["a"]);
    // serialize -> parse is lossless
    let again = parse_str(&toml::to_string(&c).unwrap()).unwrap().config;
    assert_eq!(again, c);
}

#[test]
fn syntax_error_has_line_and_column() {
    let e = err("scrollback_lines = 1\nfont = = 2\n");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].severity, Severity::Error);
    assert_eq!(e[0].line, 2);
    assert!(e[0].col >= 1);
}

#[test]
fn type_error_points_at_the_value() {
    let e = err("[cursor]\nblink = \"yes\"\n");
    assert_eq!((e[0].line, e[0].col), (2, 9));
}

#[test]
fn unknown_key_warns_with_suggestion() {
    let l = parse_str("\n\n[fnt]\nsize = 1\n[font]\nsiz = 14\n").unwrap();
    assert_eq!(l.warnings.len(), 2, "{:?}", l.warnings);
    assert_eq!(l.warnings[0].line, 3);
    assert!(l.warnings[0].message.contains("did you mean \"font\""));
    assert!(l.warnings[1].message.contains("font.siz"));
    assert!(l.warnings[1].message.contains("did you mean \"font.size\""));
    assert_eq!(l.warnings[0].severity, Severity::Warning);
}

#[test]
fn unknown_key_without_close_match_has_no_hint() {
    let l = parse_str("zzzzzzzz = 1\n").unwrap();
    assert!(!l.warnings[0].message.contains("did you mean"));
}

#[test]
fn unknown_keys_inside_arrays_are_found() {
    let l = parse_str("[[keys]]\nkey = \"a\"\naction = \"Copy\"\nactoin = 1\n").unwrap();
    assert!(l.warnings[0].message.contains("keys.actoin"));
    assert_eq!(l.warnings[0].line, 4);
}

#[test]
fn semantic_errors_have_spans() {
    let e = err("[window]\nopacity = 2.0\n");
    assert_eq!((e[0].line, e[0].col), (2, 11));
    assert!(err("scrollback_lines = 2000000").len() == 1);
    assert!(err("[font]\nsize = 0").len() == 1);
    assert!(err("[font]\nweight = \"heavyish\"").len() == 1);
    assert!(err("[quick_select]\nalphabet = \"aab\"").len() == 1);
}

#[test]
fn bad_hyperlink_regex_is_an_error() {
    let e = err("[[hyperlink_rules]]\nregex = \"(\"\nformat = \"$0\"\n");
    assert_eq!(e.len(), 1);
    assert!(e[0].message.contains("hyperlink_rules[0]"));
}

#[test]
fn bad_keybinding_reports_value_span() {
    let e = err("[[keys]]\nkey = \"hyper+x\"\naction = \"Copy\"\n");
    assert_eq!(e[0].line, 2);
    assert!(e[0].message.contains("hyper"));
}

#[test]
fn render_format() {
    let d = Diagnostic {
        severity: Severity::Warning,
        message: "unknown key \"fnt\"".into(),
        line: 12,
        col: 5,
    };
    assert_eq!(
        d.render("microterm.toml"),
        "microterm.toml:12:5 — unknown key \"fnt\""
    );
}

#[test]
fn tab_and_workspace_validation() {
    let msg = |src: &str| {
        err(src)
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
            .join("|")
    };
    assert!(msg("[tab_bar]\ntitle_format = \"{nope}\"\n").contains("unknown placeholder {nope}"));
    assert!(parse_str("[tab_bar]\ntitle_format = \"{index}:{process} {cwd_basename}\"\n").is_ok());
    assert!(msg("new_tab_cwd = \"relative/dir\"\n").contains("new_tab_cwd"));
    assert!(parse_str("new_tab_cwd = \"~/work\"\n").is_ok());
    assert!(
        msg("[[workspaces]]\nname = \"a\"\n[[workspaces]]\nname = \"a\"\n")
            .contains("more than once")
    );
    assert!(msg("[[workspaces]]\nname = \" \"\n").contains("must not be empty"));
    assert!(msg("startup_workspace = \"ghost\"\n").contains("not defined"));
    assert!(parse_str("startup_workspace = \"api\"\n[[workspaces]]\nname = \"api\"\n").is_ok());
}

#[test]
fn quick_select_can_be_switched_off() {
    assert!(Config::default().quick_select.enabled);
    let cfg = parse_str("[quick_select]\nenabled = false\n")
        .unwrap()
        .config;
    assert!(!cfg.quick_select.enabled);
    assert_eq!(
        cfg.quick_select.alphabet,
        Config::default().quick_select.alphabet
    );
}

#[test]
fn ctl_capture_cap_defaults_and_validates() {
    assert_eq!(Config::default().ctl.capture_max_bytes, 1_048_576);
    let cfg = parse_str("[ctl]\ncapture_max_bytes = 4096\n")
        .unwrap()
        .config;
    assert_eq!(cfg.ctl.capture_max_bytes, 4096);
    let e = parse_str("[ctl]\ncapture_max_bytes = 0\n").unwrap_err();
    assert!(format!("{e:?}").contains("ctl.capture_max_bytes"), "{e:?}");
    let l = parse_str("[ctl]\ncapture_max_byte = 1\n").unwrap();
    assert!(
        l.warnings
            .iter()
            .any(|w| format!("{w:?}").contains("capture_max_bytes")),
        "did-you-mean for the typo: {:?}",
        l.warnings
    );
}

#[test]
fn notifications_and_status_len_default_and_validate() {
    use mt_config::{DesktopMode, ToastMode};
    let c = Config::default();
    assert_eq!(c.ctl.status_max_len, 120);
    assert!(c.notifications.badge);
    assert_eq!(c.notifications.toast, ToastMode::Background);
    assert_eq!(c.notifications.desktop, DesktopMode::Unfocused);
    let cfg = parse_str(
        "[ctl]\nstatus_max_len = 10\n[notifications]\nbadge = false\ntoast = \"always\"\ndesktop = \"never\"\n",
    )
    .unwrap();
    assert_eq!(cfg.config.ctl.status_max_len, 10);
    assert!(!cfg.config.notifications.badge);
    assert_eq!(cfg.config.notifications.toast, ToastMode::Always);
    assert_eq!(cfg.config.notifications.desktop, DesktopMode::Never);
    let e = parse_str("[ctl]\nstatus_max_len = 0\n").unwrap_err();
    assert!(format!("{e:?}").contains("ctl.status_max_len"), "{e:?}");
    let l = parse_str("[notifications]\nbadg = true\n").unwrap();
    assert!(!l.warnings.is_empty());
}

#[test]
fn sh_section_defaults_values_and_validation() {
    use mt_config::ShColor;
    let c = Config::default();
    assert_eq!(c.sh.color, ShColor::Auto);
    assert!(!c.sh.history_send_text);
    assert!(c.sh.confirm_prompts);
    let cfg =
        parse_str("[sh]\ncolor = \"never\"\nhistory_send_text = true\nconfirm_prompts = false\n")
            .unwrap();
    assert_eq!(cfg.config.sh.color, ShColor::Never);
    assert!(cfg.config.sh.history_send_text);
    assert!(!cfg.config.sh.confirm_prompts);
    let e = parse_str("[sh]\ncolor = \"rainbow\"\n").unwrap_err();
    assert!(format!("{e:?}").contains("rainbow"), "{e:?}");
    assert!(format!("{e:?}").contains("line: 2"), "{e:?}");
    let l = parse_str("[sh]\ncolour = \"auto\"\n").unwrap();
    assert!(
        l.warnings
            .iter()
            .any(|w| format!("{w:?}").contains("color")),
        "did-you-mean for the typo: {:?}",
        l.warnings
    );
}

#[test]
fn terminal_section_defaults_values_and_validation() {
    use mt_config::{diff, Effect, OscColorReport};
    let c = Config::default();
    assert_eq!(c.terminal.osc_color_report, OscColorReport::Bit16);
    assert!(c.terminal.osc_notifications);
    assert!(c.terminal.allow_shift_capture);
    for (v, want) in [
        ("16-bit", OscColorReport::Bit16),
        ("8-bit", OscColorReport::Bit8),
        ("none", OscColorReport::None),
    ] {
        let cfg = parse_str(&format!(
            "[terminal]\nosc_color_report = \"{v}\"\nosc_notifications = false\nallow_shift_capture = false\n"
        ))
        .unwrap();
        assert_eq!(cfg.config.terminal.osc_color_report, want);
        assert!(!cfg.config.terminal.osc_notifications);
        assert!(!cfg.config.terminal.allow_shift_capture);
        // Round-trips through the serializer.
        let text = toml::to_string(&cfg.config).unwrap();
        assert_eq!(
            parse_str(&text).unwrap().config.terminal,
            cfg.config.terminal
        );
    }
    assert!(parse_str("[terminal]\nosc_color_report = \"32-bit\"\n").is_err());
    let l = parse_str("[terminal]\nosc_notification = true\n").unwrap();
    assert!(!l.warnings.is_empty(), "typo warns with a suggestion");
    let mut n = c.clone();
    n.terminal.osc_color_report = OscColorReport::None;
    assert!(diff(&c, &n).effects.contains(&Effect::Repaint));
}

#[test]
fn panes_table_and_focus_follows_mouse_parse() {
    let c = parse_str(
        "[mouse]\nfocus_follows_mouse = true\n[panes]\ninactive_dim = 0.3\nfocus_delay_ms = 120\n",
    )
    .expect("valid")
    .config;
    assert!(c.mouse.focus_follows_mouse);
    assert_eq!((c.panes.inactive_dim, c.panes.focus_delay_ms), (0.3, 120));
    assert_eq!(Config::default().panes.inactive_dim, 0.15);
}

#[test]
fn panes_values_are_range_checked() {
    let e = err("[panes]\ninactive_dim = 1.5\n");
    assert!(e[0].message.contains("panes.inactive_dim"), "{e:?}");
    let e = err("[panes]\nfocus_delay_ms = 9000\n");
    assert!(e[0].message.contains("focus_delay_ms"), "{e:?}");
    let w = parse_str("[panes]\ninactive_dimm = 0.1\n").expect("unknown keys only warn");
    assert!(w.warnings[0]
        .message
        .contains("did you mean \"panes.inactive_dim\""));
}

#[test]
fn workspaces_accept_nested_splits() {
    let src = r#"
[[workspaces]]
name = "api"
tabs = [
  { title = "dev", split = "horizontal", percent = 60, panes = [
      { cwd = "~/src/api", command = ["nvim"] },
      { split = "vertical", panes = [ { command = ["cargo", "watch"] }, { command = ["htop"] } ] },
  ] },
  { title = "shell" },
]
"#;
    let c = parse_str(src).expect("valid").config;
    let tab = &c.workspaces[0].tabs[0];
    assert_eq!(tab.split, Some(SplitKind::Horizontal));
    assert_eq!(tab.percent, Some(60));
    assert_eq!(tab.panes[0].command, vec!["nvim"]);
    assert_eq!(tab.panes[1].split, Some(SplitKind::Vertical));
    assert_eq!(tab.panes[1].panes.len(), 2);
    assert!(c.workspaces[0].tabs[1].panes.is_empty());
}

#[test]
fn declared_splits_are_validated() {
    let one = "[[workspaces]]\nname = \"a\"\ntabs = [ { split = \"horizontal\", panes = [ { command = [\"x\"] } ] } ]\n";
    assert!(err(one)[0].message.contains("exactly 2"));
    let pct = "[[workspaces]]\nname = \"a\"\ntabs = [ { split = \"vertical\", percent = 100, panes = [ {}, {} ] } ]\n";
    assert!(err(pct)[0].message.contains("percent"));
    let orphan = "[[workspaces]]\nname = \"a\"\ntabs = [ { percent = 40 } ]\n";
    assert!(err(orphan)[0].message.contains("need `split`"));
    let both = "[[workspaces]]\nname = \"a\"\ntabs = [ { split = \"vertical\", command = [\"x\"], panes = [ {}, {} ] } ]\n";
    assert!(err(both)[0].message.contains("on its panes"));
    let leaf = |d: usize| -> String {
        let mut s = String::from("{}");
        for _ in 0..d {
            s = format!("{{ split = \"vertical\", panes = [ {s}, {{}} ] }}");
        }
        s
    };
    let ok = format!(
        "[[workspaces]]\nname = \"a\"\ntabs = [ {} ]\n",
        leaf(1).replace("{ split", "{ title = \"t\", split")
    );
    parse_str(&ok).expect("depth 1 is fine");
    let deep = format!("[[workspaces]]\nname = \"a\"\ntabs = [ {} ]\n", leaf(5));
    assert!(err(&deep)[0].message.contains("nest at most"));
}

#[test]
fn nested_pane_keys_are_checked_for_typos() {
    let src = "[[workspaces]]\nname = \"a\"\ntabs = [ { split = \"horizontal\", panes = [ { cmd = [\"x\"] }, {} ] } ]\n";
    let l = parse_str(src).expect("typos only warn");
    assert!(
        l.warnings.iter().any(|w| w.message.contains("cmd")),
        "{:?}",
        l.warnings
    );
}

#[test]
fn command_accepts_string_or_array_and_cwd_inherits_via_split_nodes() {
    let c = parse_str(
        "[[workspaces]]\nname = \"a\"\n[[workspaces.tabs]]\ncwd = \"~/x\"\nsplit = \"vertical\"\n\
         [[workspaces.tabs.panes]]\ncommand = \"cargo watch -x 'test all'\"\n\
         [[workspaces.tabs.panes]]\ncommand = [\"htop\"]\n",
    )
    .expect("valid")
    .config;
    let t = &c.workspaces[0].tabs[0];
    assert_eq!(t.panes[0].command, vec!["cargo", "watch", "-x", "test all"]);
    assert_eq!(t.panes[1].command, vec!["htop"]);
}

#[test]
fn workspace_issues_are_path_addressed() {
    use mt_config::{validate_workspaces, SplitKind, WorkspaceDef, WorkspacePane, WorkspaceTab};
    let tab = WorkspaceTab {
        color: None,
        title: None,
        cwd: None,
        command: vec![],
        split: Some(SplitKind::Horizontal),
        percent: Some(100),
        panes: vec![WorkspacePane::default(), WorkspacePane::default()],
    };
    let w = [WorkspaceDef {
        name: "a".into(),
        tabs: vec![tab],
    }];
    let issues = validate_workspaces(&w, "default");
    assert_eq!(issues[0].path, "workspaces[0].tabs[0]");
}

// ---- tab colors (M11.3) ----------------------------------------------------------------

fn msgs(src: &str) -> String {
    err(src)
        .iter()
        .map(|d| d.message.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tab_colors_parse_with_palette_rules_and_declared_colors() {
    let src = r##"
[tab_bar]
color_style = "dot"
[tab_bar.palette]
orange = "#f59e42"
brand = "#112233"
[[tab_bar.color_rules]]
match = "domain:prod-*"
color = "red"
[[tab_bar.color_rules]]
match = "cmd:cargo*"
color = "brand"
[[launch]]
name = "Prod"
command = ["ssh", "prod"]
color = "red"
[[workspaces]]
name = "dev"
[[workspaces.tabs]]
title = "build"
color = "#00ff00"
[[ssh_domains]]
name = "prod-db"
host = "db"
color = "red"
"##;
    let c = parse_str(src).expect("valid").config;
    assert_eq!(c.tab_bar.color_style, TabColorStyle::Dot);
    assert_eq!(c.tab_bar.palette["orange"], "#f59e42");
    assert_eq!(c.tab_bar.color_rules.len(), 2);
    assert_eq!(c.tab_bar.color_rules[0].selector, "domain:prod-*");
    assert_eq!(c.launch[0].color.as_deref(), Some("red"));
    assert_eq!(c.workspaces[0].tabs[0].color.as_deref(), Some("#00ff00"));
    assert_eq!(c.ssh_domains[0].color.as_deref(), Some("red"));
    let back = parse_str(&toml::to_string(&c).unwrap()).unwrap().config;
    assert_eq!(back, c);
}

#[test]
fn tab_color_defaults() {
    let c = Config::default();
    assert_eq!(c.tab_bar.color_style, TabColorStyle::BarTint);
    assert!(c.tab_bar.palette.is_empty() && c.tab_bar.color_rules.is_empty());
}

#[test]
fn unknown_color_name_suggests_the_closest() {
    let m = msgs("[[launch]]\nname = \"x\"\ncolor = \"gren\"\n");
    assert!(
        m.contains("unknown color \"gren\"") && m.contains("did you mean \"green\""),
        "{m}"
    );
}

#[test]
fn bad_hex_bad_selector_and_bad_palette_are_errors() {
    let m = msgs("[[ssh_domains]]\nname = \"a\"\nhost = \"h\"\ncolor = \"#12345\"\n");
    assert!(m.contains("not a #rrggbb color"), "{m}");
    let m = msgs("[[tab_bar.color_rules]]\nmatch = \"bogus:x\"\ncolor = \"red\"\n");
    assert!(m.contains("unknown selector key"), "{m}");
    let m = msgs("[tab_bar.palette]\nBad = \"#112233\"\nok = \"red\"\n");
    assert!(
        m.contains("must match [a-z][a-z0-9-]*") && m.contains("must be a #rrggbb"),
        "{m}"
    );
}

#[test]
fn too_many_rules_and_extras_are_errors() {
    let mut src = String::new();
    for _ in 0..65 {
        src.push_str("[[tab_bar.color_rules]]\nmatch = \"title:x\"\ncolor = \"red\"\n");
    }
    assert!(msgs(&src).contains("at most 64"));
    let mut src = String::from("[tab_bar.palette]\n");
    for i in 0..11 {
        src.push_str(&format!("extra{i} = \"#112233\"\n"));
    }
    assert!(msgs(&src).contains("at most 10"));
}

#[test]
fn rules_may_use_palette_extras() {
    let src = "[tab_bar.palette]\nbrand = \"#112233\"\n[[tab_bar.color_rules]]\nmatch = \"title:x\"\ncolor = \"brand\"\n";
    assert!(parse_str(src).is_ok());
}

#[test]
fn color_keys_are_known_so_no_unknown_key_warning() {
    let src = "[tab_bar]\ncolor_style = \"bar\"\n[[launch]]\nname = \"x\"\ncolor = \"red\"\n";
    let l = parse_str(src).unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
}

#[test]
fn snippets_section_defaults_values_and_validation() {
    let c = Config::default().snippets;
    assert!(!c.enabled && !c.templates && !c.panel_open_on_start && !c.confirm_multiline);
    assert_eq!((c.panel_width, c.preview_lines), (300, 2));
    assert!(c.focus_terminal_after_insert);

    let l = parse_str(
        "[snippets]\nenabled = true\ntemplates = true\npanel_width = 420\npreview_lines = 0\n",
    )
    .unwrap();
    assert!(l.config.snippets.enabled && l.config.snippets.templates);
    assert_eq!(l.config.snippets.panel_width, 420);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);

    for (src, key) in [
        ("[snippets]\npanel_width = 219\n", "panel_width"),
        ("[snippets]\npanel_width = 601\n", "panel_width"),
        ("[snippets]\npreview_lines = 6\n", "preview_lines"),
    ] {
        let e = err(src);
        assert!(e[0].message.contains(key), "{e:?}");
        assert_eq!(e[0].line, 2, "{e:?}");
    }

    let l = parse_str("[snippets]\nenable = true\n").unwrap();
    assert!(
        l.warnings.iter().any(|w| w.message.contains("enabled")),
        "{:?}",
        l.warnings
    );
}

#[test]
fn templates_without_enabled_is_an_info_diagnostic() {
    let l = parse_str("[snippets]\ntemplates = true\n").unwrap();
    assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
    assert_eq!(l.warnings[0].severity, Severity::Info);
    assert!(l.warnings[0].message.contains("no effect"));
    assert_eq!(l.warnings[0].line, 2);
    let l = parse_str("[snippets]\nenabled = true\ntemplates = true\n").unwrap();
    assert!(l.warnings.is_empty());
}

#[test]
fn serial_ports_round_trip_and_defaults() {
    let src = r##"
[[serial_ports]]
name = "uno"
port = "/dev/ttyACM0"
baud = 9600
data_bits = "7"
parity = "even"
stop_bits = "2"
flow = "hardware"
local_echo = true
line_ending = "crlf"
color = "teal"
[[serial_ports]]
name = "plain"
port = "/dev/ttyUSB0"
"##;
    let c = parse_str(src).expect("valid").config;
    assert_eq!(c.serial_ports.len(), 2);
    assert_eq!(c.serial_ports[0].baud, Some(9600));
    assert!(c.serial_ports[0].local_echo && !c.serial_ports[1].local_echo);
    assert_eq!(c.serial_ports[1].parity, None);
    let back = parse_str(&toml::to_string(&c).unwrap()).unwrap().config;
    assert_eq!(back, c);
}

#[test]
fn bad_serial_options_are_errors() {
    let m = msgs("[[serial_ports]]\nname = \"a\"\nport = \"/dev/x\"\nparity = \"mark\"\nbaud = 0\nline_ending = \"nl\"\n");
    assert!(
        m.contains("serial_ports[0].parity must be one of none, odd, even"),
        "{m}"
    );
    assert!(m.contains("serial_ports[0].baud"), "{m}");
    assert!(m.contains("serial_ports[0].line_ending"), "{m}");
    let m = msgs("[[serial_ports]]\nname = \"a\"\nport = \"/dev/x\"\ncolor = \"gren\"\n");
    assert!(m.contains("unknown color"), "{m}");
}

#[test]
fn unknown_serial_key_is_flagged() {
    let w = parse_str("[[serial_ports]]\nname = \"a\"\nport = \"/dev/x\"\nbaudrate = 1\n")
        .expect("unknown keys only warn")
        .warnings;
    assert!(w.iter().any(|d| d.message.contains("baudrate")), "{w:?}");
}
