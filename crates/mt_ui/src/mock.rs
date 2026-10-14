//! Mock terminal content for the M1 shell. M2 replaces the active tab with a real terminal.

use crate::theme::{TermPalette, Theme};
use gpui::Hsla;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermColor {
    Fg,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Dim,
}

impl TermColor {
    pub fn resolve(self, p: &TermPalette) -> Hsla {
        match self {
            Self::Fg => p.fg,
            Self::Red => p.red,
            Self::Green => p.green,
            Self::Yellow => p.yellow,
            Self::Blue => p.blue,
            Self::Magenta => p.magenta,
            Self::Cyan => p.cyan,
            Self::Dim => p.dim,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    None,
    Single,
    Curly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanStyle {
    pub color: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    pub strike: bool,
}

impl SpanStyle {
    pub const PLAIN: Self = Self {
        color: TermColor::Fg,
        bold: false,
        italic: false,
        underline: Underline::None,
        strike: false,
    };
}

#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plain(self, text: &str) -> Self {
        self.styled(text, SpanStyle::PLAIN)
    }

    pub fn color(self, text: &str, color: TermColor) -> Self {
        self.styled(
            text,
            SpanStyle {
                color,
                ..SpanStyle::PLAIN
            },
        )
    }

    pub fn bold(self, text: &str, color: TermColor) -> Self {
        self.styled(
            text,
            SpanStyle {
                color,
                bold: true,
                ..SpanStyle::PLAIN
            },
        )
    }

    pub fn styled(mut self, text: &str, style: SpanStyle) -> Self {
        self.spans.push(Span {
            text: text.to_string(),
            style,
        });
        self
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn cols(&self) -> usize {
        self.spans.iter().map(|s| s.text.chars().count()).sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockKind {
    Zsh,
    Cargo,
    Ssh,
}

/// Where a tab runs, shown in the title bar's domain indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Local,
    Prod,
}

impl MockKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Cargo => "cargo watch",
            Self::Ssh => "ssh: prod",
        }
    }

    pub fn window_title(self) -> &'static str {
        match self {
            Self::Zsh => "~/src/payments-api — zsh",
            Self::Cargo => "~/src/payments-api — cargo watch",
            Self::Ssh => "deploy@prod-1 — ssh",
        }
    }

    pub fn domain(self) -> Domain {
        match self {
            Self::Ssh => Domain::Prod,
            _ => Domain::Local,
        }
    }

    /// A foreground process other than the shell is running.
    pub fn dirty(self) -> bool {
        matches!(self, Self::Cargo)
    }

    pub fn lines(self) -> Vec<Line> {
        use TermColor::*;
        let prompt = || {
            Line::new()
                .bold("~/src/payments-api", Blue)
                .plain(" ")
                .color("main", Magenta)
                .plain(" ")
                .color("❯", Green)
                .plain(" ")
        };
        match self {
            Self::Zsh => vec![
                prompt().plain("git log --oneline -6"),
                Line::new()
                    .color("3f2a9c1", Yellow)
                    .plain(" (")
                    .bold("HEAD -> main", Cyan)
                    .plain(") fix: retry settlement on 503"),
                Line::new().color("b81e04d", Yellow).plain(" feat(bbps): add biller fetch timeout"),
                Line::new().color("9c0d7e2", Yellow).plain(" refactor: split ledger writer"),
                Line::new().color("e4a1f33", Yellow).plain(" test: cover duplicate refund path"),
                Line::new().color("71bd0a8", Yellow).plain(" chore: bump dependencies"),
                Line::new().color("a0c9f5e", Yellow).plain(" docs: settlement runbook (PAY-212)"),
                prompt().plain("printf 'styles'"),
                Line::new()
                    .styled("bold", SpanStyle { bold: true, ..SpanStyle::PLAIN })
                    .plain("  ")
                    .styled("italic", SpanStyle { italic: true, ..SpanStyle::PLAIN })
                    .plain("  ")
                    .styled("underline", SpanStyle { underline: Underline::Single, ..SpanStyle::PLAIN })
                    .plain("  ")
                    .styled("curly", SpanStyle { underline: Underline::Curly, color: Red, ..SpanStyle::PLAIN })
                    .plain("  ")
                    .styled("strikethrough", SpanStyle { strike: true, ..SpanStyle::PLAIN })
                    .plain("  ")
                    .styled(
                        "all",
                        SpanStyle {
                            bold: true,
                            italic: true,
                            underline: Underline::Single,
                            strike: true,
                            color: Magenta,
                        },
                    ),
                prompt(),
            ],
            Self::Cargo => vec![
                Line::new().color("[Running 'cargo run']", Dim),
                Line::new().bold("   Compiling", Green).plain(" payments-api v0.4.2 (/Users/dev/src/payments-api)"),
                Line::new().bold("warning", Yellow).plain(": unused variable: `retries`"),
                Line::new().color("  --> src/settle.rs:41:9", Blue),
                Line::new().bold("    Finished", Green).plain(" `dev` profile [unoptimized + debuginfo] in 2.31s"),
                Line::new().bold("     Running", Green).plain(" `target/debug/payments-api`"),
                Line::new().color("2026-10-14T10:21:07Z", Dim).plain("  ").bold("INFO", Green).plain(" listening on 0.0.0.0:8080"),
                Line::new().color("2026-10-14T10:21:09Z", Dim).plain("  ").bold("INFO", Green).plain(" connected to db at 10.0.12.9:5432"),
                Line::new().color("[Finished running. Exit status: 0]", Dim),
            ],
            Self::Ssh => vec![
                Line::new().bold("deploy@prod-1", Green).plain(":").bold("~", Blue).plain("$ kubectl get pods -o wide"),
                Line::new().color("NAME                      READY   STATUS    RESTARTS   AGE   IP            NODE", Dim),
                Line::new().plain("api-7d9f8c6b5-x2k4p       1/1     ").color("Running", Green).plain("   0          3d    10.0.12.4     node-a"),
                Line::new().plain("api-7d9f8c6b5-q8m1z       1/1     ").color("Running", Green).plain("   0          3d    10.0.12.17    node-b"),
                Line::new().plain("worker-5c6d7f8b9-t4n2v    1/1     ").color("Running", Green).plain("   2          1d    10.0.13.22    node-b"),
                Line::new().plain("ledger-0                  0/1     ").color("Pending", Yellow).plain("   0          4m    <none>        <none>"),
                Line::new().bold("deploy@prod-1", Green).plain(":").bold("~", Blue).plain("$ git rev-parse HEAD"),
                Line::new().color("3f2a9c1d8b6e4a7f0c5d2e9b1a8f7c6d5e4b3a29", Yellow),
                Line::new().bold("deploy@prod-1", Green).plain(":").bold("~", Blue).plain("$ "),
            ],
        }
    }
}

/// Palette lookup used by the viewport when the active theme changes.
pub fn palette(theme: &Theme) -> TermPalette {
    theme.term
}

/// A quick-select hit: position in the mock grid plus the matched text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QsMatch {
    pub row: usize,
    pub col: usize,
    pub text: String,
}

fn is_ipv4(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.chars().all(|c| c.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

fn is_hash(token: &str) -> bool {
    (7..=64).contains(&token.len())
        && token.chars().all(|c| c.is_ascii_hexdigit())
        && token.chars().any(|c| c.is_ascii_digit())
        && token.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F'))
}

/// A deliberately tiny stand-in for the real pattern set that M8 builds on `mt_patterns`:
/// git-style hashes and IPv4 addresses.
pub fn scan_matches(lines: &[Line]) -> Vec<QsMatch> {
    let mut out = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.text().chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i].is_ascii_alphanumeric() {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                let mut token: String = chars[start..i].iter().collect();
                while token.ends_with('.') {
                    token.pop();
                }
                if is_ipv4(&token) || is_hash(&token) {
                    out.push(QsMatch {
                        row,
                        col: start,
                        text: token,
                    });
                }
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Prefix-free labels for `n` matches: single letters while they fit, otherwise the last `k`
/// letters of the alphabet become two-letter prefixes.
pub fn assign_labels(n: usize, alphabet: &str) -> Vec<String> {
    let letters: Vec<char> = alphabet.chars().collect();
    let a = letters.len();
    if a == 0 || n == 0 {
        return Vec::new();
    }
    if n <= a {
        return letters.iter().take(n).map(|c| c.to_string()).collect();
    }
    // singles + prefixes*a >= n, with singles = a - prefixes.
    let mut prefixes = 1;
    while (a - prefixes) + prefixes * a < n && prefixes < a {
        prefixes += 1;
    }
    let singles = a - prefixes;
    let mut out: Vec<String> = letters
        .iter()
        .take(singles)
        .map(|c| c.to_string())
        .collect();
    'outer: for p in &letters[singles..] {
        for s in &letters {
            if out.len() == n {
                break 'outer;
            }
            out.push(format!("{p}{s}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_hashes_and_ips_in_mock_screens() {
        let zsh = scan_matches(&MockKind::Zsh.lines());
        let texts: Vec<_> = zsh.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"3f2a9c1") && texts.contains(&"a0c9f5e"));
        assert_eq!(zsh.iter().filter(|m| m.row == 1).count(), 1);

        let ssh = scan_matches(&MockKind::Ssh.lines());
        let texts: Vec<_> = ssh.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"10.0.12.4"));
        assert!(texts.contains(&"3f2a9c1d8b6e4a7f0c5d2e9b1a8f7c6d5e4b3a29"));
    }

    #[test]
    fn ignores_versions_words_and_numbers() {
        let lines =
            vec![Line::new().plain("payments-api v0.4.2 listening on 8080 deadline 1234567 cafe")];
        assert!(scan_matches(&lines).is_empty());
    }

    #[test]
    fn match_columns_point_at_the_token() {
        let line = Line::new().plain("ip 10.0.0.1 ok");
        let m = &scan_matches(&[line])[0];
        assert_eq!((m.row, m.col, m.text.as_str()), (0, 3, "10.0.0.1"));
    }

    fn prefix_free(labels: &[String]) -> bool {
        labels
            .iter()
            .all(|a| labels.iter().all(|b| a == b || !b.starts_with(a.as_str())))
    }

    #[test]
    fn labels_are_single_letters_when_they_fit() {
        let l = assign_labels(4, "asdf");
        assert_eq!(l, ["a", "s", "d", "f"]);
    }

    #[test]
    fn labels_stay_prefix_free_and_unique_when_alphabet_runs_out() {
        for n in [5, 9, 12, 20, 25] {
            let l = assign_labels(n, "asdfg");
            assert_eq!(l.len(), n, "n={n}");
            assert!(prefix_free(&l), "n={n}: {l:?}");
            let mut sorted = l.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), n);
        }
    }

    #[test]
    fn every_mock_screen_has_content() {
        for k in [MockKind::Zsh, MockKind::Cargo, MockKind::Ssh] {
            assert!(!k.lines().is_empty());
        }
    }
}
