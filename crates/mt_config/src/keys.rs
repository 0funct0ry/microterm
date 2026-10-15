//! Keybinding syntax (SPEC §9.1): `cmd+shift+p`, action references and `Disable`.

use std::fmt;
use std::str::FromStr;

use crate::schema::{ActionRef, Config, KeyEntry};

/// A parsed keystroke. `cmd` is the platform key (⌘ on macOS, Super elsewhere).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Keystroke {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

const NAMED_KEYS: &[&str] = &[
    "enter",
    "tab",
    "space",
    "escape",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
];

fn valid_key(k: &str) -> bool {
    if k.chars().count() == 1 {
        return true;
    }
    if NAMED_KEYS.contains(&k) {
        return true;
    }
    k.strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

impl FromStr for Keystroke {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key".into());
        }
        // "ctrl++" and "+" bind the plus key.
        let (mods, key) = if s == "+" {
            ("", "+")
        } else if let Some(m) = s.strip_suffix("++") {
            (m, "+")
        } else {
            match s.rsplit_once('+') {
                Some((m, k)) => (m, k),
                None => ("", s),
            }
        };
        let mut out = Keystroke {
            cmd: false,
            ctrl: false,
            alt: false,
            shift: false,
            key: String::new(),
        };
        if !mods.is_empty() {
            for m in mods.split('+') {
                let flag = match m.trim().to_ascii_lowercase().as_str() {
                    "cmd" | "super" => &mut out.cmd,
                    "ctrl" | "control" => &mut out.ctrl,
                    "alt" | "opt" | "option" => &mut out.alt,
                    "shift" => &mut out.shift,
                    other => return Err(format!("unknown modifier \"{other}\" in \"{s}\"")),
                };
                if *flag {
                    return Err(format!("duplicate modifier \"{m}\" in \"{s}\""));
                }
                *flag = true;
            }
        }
        let key = if key.chars().count() == 1 {
            key.to_string()
        } else {
            key.to_ascii_lowercase()
        };
        if !valid_key(&key) {
            return Err(format!("unknown key name \"{key}\" in \"{s}\""));
        }
        out.key = key;
        Ok(out)
    }
}

impl fmt::Display for Keystroke {
    /// Canonical GPUI keystroke syntax: `cmd-shift-p`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("ctrl-")?;
        }
        if self.alt {
            f.write_str("alt-")?;
        }
        if self.shift {
            f.write_str("shift-")?;
        }
        if self.cmd {
            f.write_str("cmd-")?;
        }
        f.write_str(&self.key)
    }
}

/// Which keymap a binding belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Terminal,
    CopyMode,
}

/// A user binding. `action == None` means `Disable`: remove the default for this keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBindingSpec {
    pub keystroke: Keystroke,
    pub action: Option<ActionRef>,
    pub scope: Scope,
}

pub fn parse_entry(e: &KeyEntry) -> Result<(Keystroke, Option<ActionRef>), String> {
    let ks: Keystroke = e.key.parse()?;
    if e.action.name.is_empty() {
        return Err("empty action name".into());
    }
    if e.action.name == "Disable" && e.action.arg.is_none() {
        return Ok((ks, None));
    }
    Ok((ks, Some(e.action.clone())))
}

/// All user bindings in config order (later entries win). Errors name the offending entry.
pub fn bindings(cfg: &Config) -> Result<Vec<KeyBindingSpec>, String> {
    let mut out = Vec::new();
    for (scope, entries) in [
        (Scope::Terminal, &cfg.keys),
        (Scope::CopyMode, &cfg.copy_mode.keys),
    ] {
        for e in entries {
            let (keystroke, action) = parse_entry(e)?;
            out.push(KeyBindingSpec {
                keystroke,
                action,
                scope,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ActionArg;

    #[test]
    fn parses_modifiers_and_aliases() {
        let k: Keystroke = "super+opt+shift+p".parse().unwrap();
        assert!(k.cmd && k.alt && k.shift && !k.ctrl);
        assert_eq!(k.to_string(), "alt-shift-cmd-p");
    }

    #[test]
    fn plus_key_and_function_keys() {
        assert_eq!("ctrl++".parse::<Keystroke>().unwrap().key, "+");
        assert_eq!("f12".parse::<Keystroke>().unwrap().key, "f12");
        assert_eq!("cmd+[".parse::<Keystroke>().unwrap().key, "[");
        assert_eq!("ctrl+PageUp".parse::<Keystroke>().unwrap().key, "pageup");
    }

    #[test]
    fn rejects_bad_input() {
        assert!("hyper+p".parse::<Keystroke>().is_err());
        assert!("ctrl+ctrl+p".parse::<Keystroke>().is_err());
        assert!("ctrl+nonsense".parse::<Keystroke>().is_err());
        assert!("".parse::<Keystroke>().is_err());
    }

    #[test]
    fn disable_and_arguments() {
        let cfg: Config = toml::from_str(
            r#"
            [[keys]]
            key = "cmd+t"
            action = "Disable"
            [[keys]]
            key = "alt+2"
            action = { ActivateTab = 3 }
            [[copy_mode.keys]]
            key = "x"
            action = "Foo"
            "#,
        )
        .unwrap();
        let b = bindings(&cfg).unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].action, None);
        assert_eq!(b[1].action.as_ref().unwrap().arg, Some(ActionArg::Int(3)));
        assert_eq!(b[2].scope, Scope::CopyMode);
    }
}
