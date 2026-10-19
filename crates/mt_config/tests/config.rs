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
