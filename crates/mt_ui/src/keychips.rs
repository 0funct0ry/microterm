//! Renders keystrokes for display: `⌘⇧P` on macOS, `Ctrl+Shift+P` on Linux (SPEC §4.5, §9.1).

use crate::platform::Platform;

#[derive(Default, Debug, PartialEq, Eq)]
struct Parsed<'a> {
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
    key: &'a str,
}

/// Parses a GPUI-style keystroke such as `cmd-shift-p` or `cmd--`.
fn parse(keystroke: &str) -> Parsed<'_> {
    let mut parsed = Parsed::default();
    let mut rest = keystroke;
    loop {
        let Some((head, tail)) = rest.split_once('-') else {
            break;
        };
        if tail.is_empty() {
            break; // the key itself is `-`
        }
        match head {
            "ctrl" | "control" => parsed.ctrl = true,
            "alt" | "opt" | "option" => parsed.alt = true,
            "shift" => parsed.shift = true,
            "cmd" | "super" | "win" => parsed.cmd = true,
            _ => break,
        }
        rest = tail;
    }
    parsed.key = rest;
    parsed
}

fn key_label(key: &str, platform: Platform) -> String {
    match (key, platform) {
        ("space", _) => "Space".into(),
        ("enter", Platform::Mac) => "↵".into(),
        ("enter", _) => "Enter".into(),
        ("escape" | "esc", Platform::Mac) => "⎋".into(),
        ("escape" | "esc", _) => "Esc".into(),
        ("tab", Platform::Mac) => "⇥".into(),
        ("tab", _) => "Tab".into(),
        ("backspace", Platform::Mac) => "⌫".into(),
        ("backspace", _) => "Backspace".into(),
        ("up", Platform::Mac) => "↑".into(),
        ("down", Platform::Mac) => "↓".into(),
        ("left", Platform::Mac) => "←".into(),
        ("right", Platform::Mac) => "→".into(),
        ("up", _) => "Up".into(),
        ("down", _) => "Down".into(),
        ("left", _) => "Left".into(),
        ("right", _) => "Right".into(),
        ("pageup", _) => "PageUp".into(),
        ("pagedown", _) => "PageDown".into(),
        ("home", _) => "Home".into(),
        ("end", _) => "End".into(),
        (k, _) => k.to_uppercase(),
    }
}

/// Formats a keystroke for the given platform. Modifier order follows each platform's
/// Spelling in SPEC §4.5/§9.3 (`⌘⇧P`, `⌘⌥W`): `⌃⌘⌥⇧` on macOS, `Ctrl+Alt+Shift+Super` elsewhere.
pub fn format_keystroke(keystroke: &str, platform: Platform) -> String {
    let p = parse(keystroke);
    let key = key_label(p.key, platform);
    match platform {
        Platform::Mac => {
            let mut out = String::new();
            for (on, glyph) in [(p.ctrl, '⌃'), (p.cmd, '⌘'), (p.alt, '⌥'), (p.shift, '⇧')] {
                if on {
                    out.push(glyph);
                }
            }
            out.push_str(&key);
            out
        }
        Platform::Linux => {
            let mut parts: Vec<&str> = Vec::new();
            for (on, name) in [
                (p.ctrl, "Ctrl"),
                (p.alt, "Alt"),
                (p.shift, "Shift"),
                (p.cmd, "Super"),
            ] {
                if on {
                    parts.push(name);
                }
            }
            parts.push(&key);
            parts.join("+")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Platform::{Linux, Mac};

    #[test]
    fn palette_chord() {
        assert_eq!(format_keystroke("cmd-shift-p", Mac), "⌘⇧P");
        assert_eq!(format_keystroke("ctrl-shift-p", Linux), "Ctrl+Shift+P");
    }

    #[test]
    fn mac_modifier_order_follows_spec() {
        assert_eq!(format_keystroke("cmd-alt-w", Mac), "⌘⌥W");
        assert_eq!(format_keystroke("ctrl-alt-shift-cmd-x", Mac), "⌃⌘⌥⇧X");
    }

    #[test]
    fn linux_modifier_order_is_ctrl_alt_shift() {
        assert_eq!(format_keystroke("ctrl-alt-w", Linux), "Ctrl+Alt+W");
        assert_eq!(
            format_keystroke("shift-alt-ctrl-w", Linux),
            "Ctrl+Alt+Shift+W"
        );
    }

    #[test]
    fn named_keys() {
        assert_eq!(format_keystroke("cmd-shift-space", Mac), "⌘⇧Space");
        assert_eq!(
            format_keystroke("ctrl-shift-space", Linux),
            "Ctrl+Shift+Space"
        );
        assert_eq!(format_keystroke("ctrl-tab", Linux), "Ctrl+Tab");
        assert_eq!(format_keystroke("escape", Mac), "⎋");
        assert_eq!(format_keystroke("escape", Linux), "Esc");
        assert_eq!(format_keystroke("enter", Mac), "↵");
    }

    #[test]
    fn punctuation_keys() {
        assert_eq!(format_keystroke("cmd-shift-]", Mac), "⌘⇧]");
        assert_eq!(format_keystroke("cmd-,", Mac), "⌘,");
        assert_eq!(format_keystroke("ctrl-=", Linux), "Ctrl+=");
    }

    #[test]
    fn minus_key_is_not_a_separator() {
        assert_eq!(format_keystroke("cmd--", Mac), "⌘-");
        assert_eq!(format_keystroke("ctrl--", Linux), "Ctrl+-");
    }

    #[test]
    fn bare_key() {
        assert_eq!(format_keystroke("f1", Linux), "F1");
        assert_eq!(format_keystroke("1", Mac), "1");
    }
}
