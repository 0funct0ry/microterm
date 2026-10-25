//! Best-effort secret redaction for trigger `snapshot` captures (SPEC §10.11.7). A safety net,
//! not a guarantee: obvious shapes only. Screenshots cannot be redacted at all.

use std::sync::OnceLock;

use regex::Regex;

/// What replaces a secret.
pub const REDACTED: &str = "‹redacted›";

struct Rule {
    re: Regex,
    /// Replacement template (`$1` keeps a captured prefix such as the key name).
    with: &'static str,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let r = |p: &str, with: &'static str| Rule {
            re: Regex::new(p).expect("built-in redaction pattern"),
            with,
        };
        vec![
            // PEM private keys, whole block (the text may span lines).
            r(
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
                REDACTED,
            ),
            // `Authorization: <anything to the end of the line>`.
            r(r"(?im)^(\s*authorization\s*:\s*).*$", "${1}‹redacted›"),
            // `Bearer <token>` anywhere.
            r(r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]{8,}", "${1}‹redacted›"),
            // AWS access key ids.
            r(r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA)[0-9A-Z]{16}\b", REDACTED),
            // `user:password@` in a URL.
            r(r"(?i)\b([a-z][a-z0-9+.-]*://[^\s/:@]+:)[^\s/@]+@", "${1}‹redacted›@"),
            // `password=…`, `token: …`, `api_key = …`, `secret=…`.
            r(
                r#"(?i)\b((?:password|passwd|secret|token|api[_-]?key)\s*[=:]\s*)(?:"[^"]*"|'[^']*'|\S+)"#,
                "${1}‹redacted›",
            ),
        ]
    })
}

/// `text` with obvious secrets replaced by [`REDACTED`].
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    for rule in rules() {
        out = rule.re.replace_all(&out, rule.with).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_shapes_are_replaced() {
        let cases: &[(&str, &str)] = &[
            (
                "Authorization: Basic dXNlcjpwYXNz",
                "Authorization: ‹redacted›",
            ),
            (
                "  authorization:Bearer abc.def.ghi",
                "  authorization:‹redacted›",
            ),
            (
                "curl -H 'x: Bearer eyJhbGciOiJIUzI1NiJ9.e30.sig'",
                "curl -H 'x: Bearer ‹redacted›'",
            ),
            ("key AKIAIOSFODNN7EXAMPLE here", "key ‹redacted› here"),
            ("password=hunter2 next", "password=‹redacted› next"),
            ("PASSWD: s3cret!", "PASSWD: ‹redacted›"),
            ("api_key = \"abc def\"", "api_key = ‹redacted›"),
            ("API-KEY: abcdef", "API-KEY: ‹redacted›"),
            ("token='x y z' ok", "token=‹redacted› ok"),
            (
                "https://bob:pa55@example.com/x",
                "https://bob:‹redacted›@example.com/x",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(redact(input), *want, "input {input:?}");
        }
    }

    #[test]
    fn private_key_blocks_go_whole_even_across_lines_and_truncated() {
        let t = "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3Blbn\nAAAA\n-----END OPENSSH PRIVATE KEY-----\nafter";
        assert_eq!(redact(t), "before\n‹redacted›\nafter");
        let cut = "x\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\nstill secret";
        assert_eq!(redact(cut), "x\n‹redacted›");
    }

    #[test]
    fn ordinary_text_is_untouched() {
        for t in [
            "error[E0308]: mismatched types",
            "the token count is 5",
            "see https://example.com/path?x=1",
            "user@host:~$ ls",
            "Bearer",
            "AKIA short",
        ] {
            assert_eq!(redact(t), t, "{t:?}");
        }
    }

    #[test]
    fn redaction_is_idempotent() {
        let once = redact("password=abc Authorization: x AKIAIOSFODNN7EXAMPLE");
        assert_eq!(redact(&once), once);
    }
}
