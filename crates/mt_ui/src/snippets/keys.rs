//! Per-snippet key bindings (SPEC §10.8.2 "Key precedence"): registry defaults < per-snippet
//! `keys` < user `[[keys]]`. Bindings exist only while the feature is on, because the library is
//! empty otherwise.

use mt_config::keys::{KeyBindingSpec, Keystroke, Scope};
use mt_config::{ActionArg, ActionRef};
use mt_snippets::SnippetSet;

use crate::{actions::ACTIONS, platform::Platform};

/// `InsertSnippet{id}` bindings for every snippet with a valid `keys`, in file order.
pub fn specs(set: &SnippetSet) -> Vec<KeyBindingSpec> {
    set.snippets
        .iter()
        .filter(|s| !s.keys.is_empty())
        .filter_map(|s| {
            let keystroke: Keystroke = s.keys.parse().ok()?;
            Some(KeyBindingSpec {
                keystroke,
                action: Some(ActionRef {
                    name: "InsertSnippet".into(),
                    arg: Some(ActionArg::Table(format!("id = {:?}", s.id))),
                }),
                scope: Scope::Terminal,
            })
        })
        .collect()
}

/// `(cmd, ctrl, alt, shift, key)` of a GPUI-style stroke such as `cmd-shift-s` or `cmd-,`.
fn gpui_stroke(s: &str) -> (bool, bool, bool, bool, String) {
    let mut m = (false, false, false, false);
    let mut rest = s;
    loop {
        let (head, tail) = match rest.split_once('-') {
            Some((h, t)) if !t.is_empty() => (h, t),
            _ => break,
        };
        match head {
            "cmd" | "super" | "platform" => m.0 = true,
            "ctrl" | "control" => m.1 = true,
            "alt" | "opt" | "option" => m.2 = true,
            "shift" => m.3 = true,
            _ => break,
        }
        rest = tail;
    }
    (m.0, m.1, m.2, m.3, rest.to_string())
}

/// Whether a config-syntax stroke (`cmd+shift+s`) and a registry stroke (`cmd-shift-s`) match.
fn same(a: &str, b: &str) -> bool {
    let norm = |s: &str| match s.parse::<Keystroke>() {
        Ok(k) => (k.cmd, k.ctrl, k.alt, k.shift, k.key),
        Err(_) => gpui_stroke(s),
    };
    norm(a) == norm(b)
}

/// Warnings for the snippet keys: unparsable ones, duplicates, and the default each one replaces.
pub fn problems(set: &SnippetSet, platform: Platform) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for s in set.snippets.iter().filter(|s| !s.keys.is_empty()) {
        if let Err(e) = s.keys.parse::<Keystroke>() {
            out.push(format!(
                "snippet \"{}\": key \"{}\": {e}; binding skipped",
                s.id, s.keys
            ));
            continue;
        }
        if let Some((other, _)) = seen.iter().find(|(_, k)| same(k, &s.keys)) {
            out.push(format!(
                "snippet \"{}\": key \"{}\" is also bound to snippet \"{other}\"; the later one wins",
                s.id, s.keys
            ));
        }
        seen.push((&s.id, &s.keys));
        for def in ACTIONS {
            if def.bindings(platform).iter().any(|d| same(d, &s.keys)) {
                out.push(format!(
                    "snippet \"{}\": key \"{}\" replaces the default binding of {}",
                    s.id, s.keys, def.name
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_snippets::Snippet;

    fn snip(id: &str, keys: &str) -> Snippet {
        Snippet {
            id: id.into(),
            keys: keys.into(),
            ..Default::default()
        }
    }

    #[test]
    fn valid_keys_become_insert_bindings() {
        let set = SnippetSet {
            snippets: vec![
                snip("a", "cmd+alt+1"),
                snip("b", ""),
                snip("c", "nonsense+"),
            ],
        };
        let s = specs(&set);
        assert_eq!(s.len(), 1);
        let a = s[0].action.as_ref().unwrap();
        assert_eq!(a.name, "InsertSnippet");
        let ActionArg::Table(t) = a.arg.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(crate::actions::parse_insert_snippet(t).unwrap().id, "a");
    }

    #[test]
    fn problems_name_the_default_and_the_duplicate() {
        let set = SnippetSet {
            snippets: vec![
                snip("a", "cmd+shift+s"),
                snip("b", "cmd+alt+9"),
                snip("c", "cmd+alt+9"),
                snip("d", "bad bad"),
            ],
        };
        let p = problems(&set, Platform::Mac);
        assert!(
            p.iter()
                .any(|m| m.contains("replaces the default binding of ToggleSnippetsPanel")),
            "{p:?}"
        );
        assert!(
            p.iter().any(|m| m.contains("also bound to snippet \"b\"")),
            "{p:?}"
        );
        assert!(
            p.iter()
                .any(|m| m.contains("snippet \"d\"") && m.contains("skipped")),
            "{p:?}"
        );
    }

    #[test]
    fn no_problems_for_free_keys() {
        let set = SnippetSet {
            snippets: vec![snip("a", "cmd+alt+7")],
        };
        assert!(problems(&set, Platform::Mac).is_empty());
    }
}
