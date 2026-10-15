//! Parsing with precise `line:column` diagnostics: syntax errors, type errors, unknown-key
//! warnings with "did you mean" suggestions and semantic validation (SPEC §8.2).

use std::ops::Range;

use toml_edit::{ImDocument, Item, TableLike};

use crate::keys;
use crate::schema::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// A problem in the config source. `line` and `col` are 1-based; `line == 0` means the problem
/// has no source position (for example an unreadable file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl Diagnostic {
    fn at(src: &str, severity: Severity, span: Option<Range<usize>>, message: String) -> Self {
        let (line, col) = span.map_or((0, 0), |s| line_col(src, s.start));
        Self {
            severity,
            message,
            line,
            col,
        }
    }

    pub fn error_without_position(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            line: 0,
            col: 0,
        }
    }

    /// `microterm.toml:12:5 — unknown key "fnt" (did you mean "font"?)`.
    pub fn render(&self, file: &str) -> String {
        if self.line == 0 {
            format!("{file} — {}", self.message)
        } else {
            format!("{file}:{}:{} — {}", self.line, self.col, self.message)
        }
    }
}

/// A successfully parsed config plus non-fatal warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<Diagnostic>,
}

pub(crate) fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(src.len());
    let before = &src[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// Parse and validate config source. Errors are returned in source order; any error means
/// the config must not be applied.
pub fn parse_str(src: &str) -> Result<Loaded, Vec<Diagnostic>> {
    let doc = match ImDocument::parse(src) {
        Ok(d) => d,
        Err(e) => {
            return Err(vec![Diagnostic::at(
                src,
                Severity::Error,
                e.span(),
                e.message().to_string(),
            )])
        }
    };

    let mut warnings = Vec::new();
    check_unknown(src, doc.as_table(), ROOT, "", &mut warnings);

    let config: Config = match toml::from_str(src) {
        Ok(c) => c,
        Err(e) => {
            return Err(vec![Diagnostic::at(
                src,
                Severity::Error,
                e.span(),
                e.message().to_string(),
            )])
        }
    };

    let mut errors = Vec::new();
    validate(src, &doc, &config, &mut errors);
    if errors.is_empty() {
        warnings.sort_by_key(|d| (d.line, d.col));
        Ok(Loaded { config, warnings })
    } else {
        errors.sort_by_key(|d| (d.line, d.col));
        Err(errors)
    }
}

// ---------------------------------------------------------------------------------------------
// Known keys

enum K {
    Leaf,
    /// Any keys allowed (`[env]`).
    Free,
    Table(&'static [(&'static str, K)]),
    /// Array of tables (`[[keys]]`) or array of inline tables.
    Array(&'static [(&'static str, K)]),
}

const KEY_ENTRY: &[(&str, K)] = &[("key", K::Leaf), ("action", K::Leaf)];

const ROOT: &[(&str, K)] = &[
    ("shell", K::Leaf),
    ("term", K::Leaf),
    ("scrollback_lines", K::Leaf),
    ("new_tab_cwd", K::Leaf),
    ("confirm_close", K::Leaf),
    ("startup_workspace", K::Leaf),
    ("theme", K::Table(&[("dark", K::Leaf), ("light", K::Leaf)])),
    ("env", K::Free),
    (
        "font",
        K::Table(&[
            ("family", K::Leaf),
            ("size", K::Leaf),
            ("line_height", K::Leaf),
            ("weight", K::Leaf),
            ("bold_weight", K::Leaf),
            ("ligatures", K::Leaf),
            ("features", K::Leaf),
            ("fallback", K::Leaf),
        ]),
    ),
    (
        "window",
        K::Table(&[
            ("padding", K::Table(&[("x", K::Leaf), ("y", K::Leaf)])),
            ("opacity", K::Leaf),
            ("blur", K::Leaf),
            ("decorations", K::Leaf),
            (
                "initial_size",
                K::Table(&[("cols", K::Leaf), ("rows", K::Leaf)]),
            ),
            ("remember_size", K::Leaf),
        ]),
    ),
    (
        "tab_bar",
        K::Table(&[
            ("position", K::Leaf),
            ("hide_if_single", K::Leaf),
            ("title_format", K::Leaf),
            ("show_index", K::Leaf),
        ]),
    ),
    (
        "cursor",
        K::Table(&[
            ("style", K::Leaf),
            ("blink", K::Leaf),
            ("blink_interval_ms", K::Leaf),
        ]),
    ),
    (
        "text",
        K::Table(&[
            ("blink", K::Leaf),
            ("min_contrast", K::Leaf),
            ("wide_regional_indicators", K::Leaf),
        ]),
    ),
    ("scrollbar", K::Table(&[("mode", K::Leaf)])),
    (
        "selection",
        K::Table(&[
            ("word_chars", K::Leaf),
            ("copy_on_select", K::Leaf),
            ("trim_trailing_whitespace", K::Leaf),
        ]),
    ),
    (
        "mouse",
        K::Table(&[
            ("right_click", K::Leaf),
            ("hide_while_typing", K::Leaf),
            ("scroll_multiplier", K::Leaf),
            ("focus_follows_mouse", K::Leaf),
        ]),
    ),
    ("clipboard", K::Table(&[("osc52", K::Leaf)])),
    ("paste", K::Table(&[("confirm_multiline", K::Leaf)])),
    ("bell", K::Table(&[("mode", K::Leaf)])),
    (
        "quick_select",
        K::Table(&[
            ("alphabet", K::Leaf),
            ("patterns", K::Leaf),
            ("disable_builtin", K::Leaf),
        ]),
    ),
    (
        "hyperlinks",
        K::Table(&[("schemes", K::Leaf), ("modifier", K::Leaf)]),
    ),
    (
        "hyperlink_rules",
        K::Array(&[("regex", K::Leaf), ("format", K::Leaf)]),
    ),
    ("keys", K::Array(KEY_ENTRY)),
    ("copy_mode", K::Table(&[("keys", K::Array(KEY_ENTRY))])),
    (
        "launch",
        K::Array(&[("name", K::Leaf), ("command", K::Leaf), ("cwd", K::Leaf)]),
    ),
    (
        "workspaces",
        K::Array(&[
            ("name", K::Leaf),
            (
                "tabs",
                K::Array(&[("title", K::Leaf), ("cwd", K::Leaf), ("command", K::Leaf)]),
            ),
        ]),
    ),
    (
        "ssh_domains",
        K::Array(&[
            ("name", K::Leaf),
            ("host", K::Leaf),
            ("user", K::Leaf),
            ("port", K::Leaf),
            ("identity_file", K::Leaf),
            ("multiplexing", K::Leaf),
            ("remote_dir", K::Leaf),
            ("connect_timeout_s", K::Leaf),
        ]),
    ),
    ("gpu", K::Table(&[("prefer", K::Leaf)])),
];

fn suggest<'a>(unknown: &str, known: &'a [(&'static str, K)]) -> Option<&'a str> {
    known
        .iter()
        .map(|(k, _)| (*k, strsim::damerau_levenshtein(unknown, k)))
        .filter(|(k, d)| *d <= 2 && *d < unknown.len().max(k.len()))
        .min_by_key(|(_, d)| *d)
        .map(|(k, _)| k)
}

/// Tables inside an item: a table, an inline table, or an array of either.
fn tables_in(item: &Item) -> Vec<&dyn TableLike> {
    if let Some(t) = item.as_table_like() {
        return vec![t];
    }
    if let Some(a) = item.as_array_of_tables() {
        return a.iter().map(|t| t as &dyn TableLike).collect();
    }
    if let Some(a) = item.as_array() {
        return a
            .iter()
            .filter_map(|v| v.as_inline_table().map(|t| t as &dyn TableLike))
            .collect();
    }
    Vec::new()
}

fn check_unknown(
    src: &str,
    table: &dyn TableLike,
    known: &'static [(&'static str, K)],
    path: &str,
    out: &mut Vec<Diagnostic>,
) {
    for (key, item) in table.iter() {
        let full = if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        };
        match known.iter().find(|(k, _)| *k == key) {
            None => {
                let span = table.get_key_value(key).and_then(|(k, _)| k.span());
                let mut msg = format!("unknown key \"{full}\"");
                if let Some(s) = suggest(key, known) {
                    let hint = if path.is_empty() {
                        s.to_string()
                    } else {
                        format!("{path}.{s}")
                    };
                    msg.push_str(&format!(" (did you mean \"{hint}\"?)"));
                }
                out.push(Diagnostic::at(src, Severity::Warning, span, msg));
            }
            Some((_, K::Leaf | K::Free)) => {}
            Some((_, K::Table(children) | K::Array(children))) => {
                for t in tables_in(item) {
                    check_unknown(src, t, children, &full, out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Semantic validation

fn item_at<'a>(doc: &'a ImDocument<&str>, path: &[&str]) -> Option<&'a Item> {
    let mut cur: &Item = doc.as_item();
    for p in path {
        cur = cur.as_table_like()?.get(p)?;
    }
    Some(cur)
}

/// Placeholders accepted by `tab_bar.title_format` (SPEC §8.3).
pub const TITLE_PLACEHOLDERS: [&str; 4] = ["{title}", "{process}", "{cwd_basename}", "{index}"];

fn validate_tabs_and_workspaces(c: &Config, err: &mut impl FnMut(&[&str], String)) {
    // Every `{...}` in the format must be a known placeholder.
    let mut rest = c.tab_bar.title_format.as_str();
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let token = &rest[start..=start + len];
        if !TITLE_PLACEHOLDERS.contains(&token) {
            err(
                &["tab_bar", "title_format"],
                format!(
                    "tab_bar.title_format has unknown placeholder {token} (use {})",
                    TITLE_PLACEHOLDERS.join(", ")
                ),
            );
            break;
        }
        rest = &rest[start + len + 1..];
    }
    let cwd = c.new_tab_cwd.as_str();
    if !(cwd == "inherit" || cwd == "home" || cwd.starts_with('/') || cwd.starts_with('~')) {
        err(
            &["new_tab_cwd"],
            "new_tab_cwd must be \"inherit\", \"home\" or an absolute path".into(),
        );
    }
    let mut seen: Vec<&str> = Vec::new();
    for w in &c.workspaces {
        if w.name.trim().is_empty() {
            err(&["workspaces"], "workspace name must not be empty".into());
        } else if seen.contains(&w.name.as_str()) {
            err(
                &["workspaces"],
                format!("workspace \"{}\" is defined more than once", w.name),
            );
        }
        seen.push(&w.name);
    }
    let start = c.startup_workspace.as_str();
    if start != "default" && !seen.contains(&start) {
        err(
            &["startup_workspace"],
            format!("startup_workspace \"{start}\" is not defined in [[workspaces]]"),
        );
    }
}

fn validate(src: &str, doc: &ImDocument<&str>, c: &Config, out: &mut Vec<Diagnostic>) {
    let mut err = |path: &[&str], msg: String| {
        let span = item_at(doc, path).and_then(|i| i.span());
        out.push(Diagnostic::at(src, Severity::Error, span, msg));
    };

    if !(0.0..=1.0).contains(&c.window.opacity) {
        err(
            &["window", "opacity"],
            "window.opacity must be between 0.0 and 1.0".into(),
        );
    }
    if c.scrollback_lines > 1_000_000 {
        err(
            &["scrollback_lines"],
            "scrollback_lines must be at most 1000000".into(),
        );
    }
    if !(c.font.size > 0.0 && c.font.size <= 200.0) {
        err(
            &["font", "size"],
            "font.size must be greater than 0 and at most 200".into(),
        );
    }
    if c.font.line_height <= 0.0 {
        err(
            &["font", "line_height"],
            "font.line_height must be greater than 0".into(),
        );
    }
    for (k, w) in [
        ("weight", &c.font.weight),
        ("bold_weight", &c.font.bold_weight),
    ] {
        match w.numeric() {
            Some(n) if (100..=900).contains(&n) => {}
            _ => err(
                &["font", k],
                format!("font.{k} must be a name (thin..black) or a number 100..900"),
            ),
        }
    }
    for f in &c.font.features {
        if crate::schema::parse_font_feature(f).is_none() {
            err(
                &["font", "features"],
                format!(
                    "font.features entry \"{f}\" must be a 4-character tag, optionally `tag=N`"
                ),
            );
        }
    }
    if c.font.fallback.iter().any(|f| f.trim().is_empty()) {
        err(
            &["font", "fallback"],
            "font.fallback entries must not be empty".into(),
        );
    }
    if c.cursor.blink_interval_ms == 0 {
        err(
            &["cursor", "blink_interval_ms"],
            "cursor.blink_interval_ms must be greater than 0".into(),
        );
    }
    if c.mouse.scroll_multiplier <= 0.0 {
        err(
            &["mouse", "scroll_multiplier"],
            "mouse.scroll_multiplier must be greater than 0".into(),
        );
    }
    for (i, rule) in c.hyperlink_rules.iter().enumerate() {
        if let Err(e) = mt_patterns::LinkSet::check_regex(&rule.regex) {
            err(
                &["hyperlink_rules"],
                format!("hyperlink_rules[{i}].regex is not a valid regular expression: {e}"),
            );
        }
    }
    if c.text.min_contrast < 1.0 {
        err(
            &["text", "min_contrast"],
            "text.min_contrast must be at least 1.0".into(),
        );
    }
    if c.window.padding.x < 0.0 || c.window.padding.y < 0.0 {
        err(
            &["window", "padding"],
            "window.padding values must not be negative".into(),
        );
    }
    if c.window.initial_size.cols == 0 || c.window.initial_size.rows == 0 {
        err(
            &["window", "initial_size"],
            "window.initial_size cols and rows must be greater than 0".into(),
        );
    }
    let alphabet = &c.quick_select.alphabet;
    let mut seen: Vec<char> = alphabet.chars().collect();
    seen.sort_unstable();
    seen.dedup();
    if alphabet.is_empty() || seen.len() != alphabet.chars().count() {
        err(
            &["quick_select", "alphabet"],
            "quick_select.alphabet must be non-empty with no repeated characters".into(),
        );
    }

    validate_tabs_and_workspaces(c, &mut err);

    // Keybindings: report the span of the offending `key = "..."` value.
    for (section, entries) in [("keys", &c.keys), ("copy_mode", &c.copy_mode.keys)] {
        let tables: Vec<&dyn TableLike> = if section == "keys" {
            item_at(doc, &["keys"]).map(tables_in).unwrap_or_default()
        } else {
            item_at(doc, &["copy_mode", "keys"])
                .map(tables_in)
                .unwrap_or_default()
        };
        for (i, e) in entries.iter().enumerate() {
            if let Err(m) = keys::parse_entry(e) {
                let span = tables
                    .get(i)
                    .and_then(|t| t.get("key"))
                    .and_then(|v| v.span());
                out.push(Diagnostic::at(src, Severity::Error, span, m));
            }
        }
    }
}
