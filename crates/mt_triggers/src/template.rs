//! Templates and the sanitiser (§10.11.2). Expansion is pure and local: no functions, no
//! environment, no process or network access.

/// Longest substituted value, in characters, including the trailing `…`.
pub const VALUE_MAX: usize = 120;

/// One piece of a parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Lit(String),
    Var(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("unterminated placeholder in template (write a literal `{{` as `{{{{`)")]
    Unterminated,
    #[error("empty placeholder `{{}}` (write a literal `{{` as `{{{{`)")]
    Empty,
    #[error("invalid placeholder name {0:?}")]
    BadName(String),
}

/// Parses `src`. `{{` is a literal `{`; a lone `}` is a literal `}`.
pub fn parse(src: &str) -> Result<Vec<Part>, TemplateError> {
    let mut out: Vec<Part> = Vec::new();
    let mut lit = String::new();
    let mut it = src.chars().peekable();
    while let Some(c) = it.next() {
        if c != '{' {
            lit.push(c);
            continue;
        }
        if it.peek() == Some(&'{') {
            it.next();
            lit.push('{');
            continue;
        }
        let mut name = String::new();
        loop {
            match it.next() {
                Some('}') => break,
                Some(ch) => name.push(ch),
                None => return Err(TemplateError::Unterminated),
            }
        }
        if name.is_empty() {
            return Err(TemplateError::Empty);
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(TemplateError::BadName(name));
        }
        if !lit.is_empty() {
            out.push(Part::Lit(std::mem::take(&mut lit)));
        }
        out.push(Part::Var(name));
    }
    if !lit.is_empty() {
        out.push(Part::Lit(lit));
    }
    Ok(out)
}

/// The placeholder names used by `src` (empty when it does not parse).
pub fn placeholders(src: &str) -> Vec<String> {
    parse(src)
        .map(|parts| {
            parts
                .into_iter()
                .filter_map(|p| match p {
                    Part::Var(v) => Some(v),
                    Part::Lit(_) => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Sanitises one substituted value: whitespace runs collapse to one space, control characters
/// (C0, C1, DEL) and bidi overrides are removed, and the result is cut to `max` characters with
/// a trailing `…`.
pub fn sanitize_with(value: &str, max: usize) -> String {
    let mut out = String::with_capacity(value.len().min(max + 4));
    let mut pending_space = false;
    for c in value.chars() {
        if c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
            continue;
        }
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if is_bidi(c) {
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(c);
    }
    if out.chars().count() > max {
        let cut: String = out.chars().take(max.saturating_sub(1)).collect();
        return format!("{}…", cut.trim_end());
    }
    out
}

/// [`sanitize_with`] at the standard 120-character cap.
pub fn sanitize(value: &str) -> String {
    sanitize_with(value, VALUE_MAX)
}

/// Expands `src`, looking placeholders up with `lookup` (unknown or missing names expand to an
/// empty string) and sanitising every substituted value. Literal text is kept as written.
pub fn expand(src: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let Ok(parts) = parse(src) else {
        return src.to_string();
    };
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Lit(l) => out.push_str(&l),
            Part::Var(v) => {
                if let Some(val) = lookup(&v) {
                    out.push_str(&sanitize(&val));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_literals_vars_and_escapes() {
        assert_eq!(
            parse("a {x} b {{y}} {1}").unwrap(),
            vec![
                Part::Lit("a ".into()),
                Part::Var("x".into()),
                Part::Lit(" b {y}} ".into()),
                Part::Var("1".into()),
            ]
        );
        assert_eq!(parse("{{").unwrap(), vec![Part::Lit("{".into())]);
        assert_eq!(parse("a } b").unwrap(), vec![Part::Lit("a } b".into())]);
    }

    #[test]
    fn rejects_malformed_placeholders() {
        assert_eq!(parse("{abc"), Err(TemplateError::Unterminated));
        assert_eq!(parse("{}"), Err(TemplateError::Empty));
        assert!(matches!(parse("{a b}"), Err(TemplateError::BadName(_))));
        assert!(matches!(parse("{a-b}"), Err(TemplateError::BadName(_))));
    }

    #[test]
    fn sanitiser_table() {
        let cases: &[(&str, &str)] = &[
            ("plain", "plain"),
            ("  a \t\n b  ", "a b"),
            ("a\u{1b}[31mb", "a[31mb"),
            ("x\u{0085}y\u{009f}z", "xyz"),
            ("a\u{202e}b\u{2066}c", "abc"),
            ("del\u{7f}ete", "delete"),
            ("", ""),
            ("a\u{0}b", "ab"),
        ];
        for (input, want) in cases {
            assert_eq!(sanitize(input), *want, "input {input:?}");
        }
    }

    #[test]
    fn sanitiser_truncates_with_an_ellipsis() {
        let long = "x".repeat(500);
        let s = sanitize(&long);
        assert_eq!(s.chars().count(), VALUE_MAX);
        assert!(s.ends_with('…'));
        assert_eq!(sanitize(&"y".repeat(VALUE_MAX)).chars().count(), VALUE_MAX);
        assert!(!sanitize(&"y".repeat(VALUE_MAX)).ends_with('…'));
    }

    #[test]
    fn expand_sanitises_values_not_literals() {
        let out = expand("hi {n}!\n{{x}", |k| {
            (k == "n").then(|| "a\u{7}\n b".to_string())
        });
        assert_eq!(out, "hi a b!\n{x}");
        assert_eq!(expand("{missing}.", |_| None), ".");
    }

    #[test]
    fn expansion_is_not_recursive() {
        let out = expand("{a}", |k| (k == "a").then(|| "{b}".to_string()));
        assert_eq!(out, "{b}");
    }

    proptest! {
        #[test]
        fn parse_never_panics(s in ".{0,200}") {
            let _ = parse(&s);
        }

        #[test]
        fn sanitised_values_are_clean_and_bounded(s in ".{0,400}") {
            let out = sanitize(&s);
            prop_assert!(out.chars().count() <= VALUE_MAX);
            prop_assert!(!out.chars().any(|c| c.is_control() || is_bidi(c)));
            prop_assert!(!out.contains("  "));
            prop_assert_eq!(out.trim(), out.as_str());
        }

        #[test]
        fn expand_never_panics(s in ".{0,200}", v in ".{0,200}") {
            let _ = expand(&s, |_| Some(v.clone()));
        }
    }
}
