//! Parsing with precise `line:column` diagnostics: syntax errors, type errors, unknown-key
//! warnings with "did you mean" suggestions and semantic validation (SPEC §8.2).

use std::ops::Range;

use toml_edit::{ImDocument, Item, TableLike};

use crate::keys;
use crate::schema::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Informational: listed with the warnings but never raises a banner.
    Info,
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
    if config.snippets.templates && !config.snippets.enabled {
        warnings.push(Diagnostic::at(
            src,
            Severity::Info,
            item_at(&doc, &["snippets", "templates"]).and_then(|i| i.span()),
            "snippets.templates has no effect while snippets.enabled = false".into(),
        ));
    }
    for issue in crate::menu::validate(&config) {
        let d = Diagnostic::at(
            src,
            if issue.warning {
                Severity::Warning
            } else {
                Severity::Error
            },
            menu_span(&doc, &issue.loc),
            issue.message,
        );
        if issue.warning {
            warnings.push(d);
        } else {
            errors.push(d);
        }
    }
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
    /// A table whose keys are names (`[context_menu.<id>]`), each a table of these keys.
    /// The names themselves are checked by semantic validation.
    Map(&'static [(&'static str, K)]),
}

const SPAWN_ITEM: &[(&str, K)] = &[
    ("command", K::Leaf),
    ("cwd", K::Leaf),
    ("title", K::Leaf),
    ("where", K::Leaf),
    ("close_on_success", K::Leaf),
    ("ctl", K::Leaf),
];

const MENU_SUBITEM: &[(&str, K)] = &[
    ("id", K::Leaf),
    ("label", K::Leaf),
    ("icon", K::Leaf),
    ("when", K::Leaf),
    ("when_false", K::Leaf),
    ("confirm", K::Leaf),
    ("action", K::Leaf),
    ("send", K::Leaf),
    ("quote", K::Leaf),
    ("multiline", K::Leaf),
    ("spawn", K::Table(SPAWN_ITEM)),
    ("ctl", K::Leaf),
    ("submenu", K::Leaf),
    ("type", K::Leaf),
    ("include", K::Leaf),
];

const MENU_ITEM: &[(&str, K)] = &[
    ("id", K::Leaf),
    ("label", K::Leaf),
    ("icon", K::Leaf),
    ("when", K::Leaf),
    ("when_false", K::Leaf),
    ("confirm", K::Leaf),
    ("action", K::Leaf),
    ("send", K::Leaf),
    ("quote", K::Leaf),
    ("multiline", K::Leaf),
    ("spawn", K::Table(SPAWN_ITEM)),
    ("ctl", K::Leaf),
    ("submenu", K::Array(MENU_SUBITEM)),
    ("type", K::Leaf),
    ("include", K::Leaf),
];

const MENU_CONFIG: &[(&str, K)] = &[
    ("enabled", K::Leaf),
    ("mode", K::Leaf),
    ("builtin", K::Leaf),
    ("max_rows", K::Leaf),
    ("trigger", K::Leaf),
    ("items", K::Array(MENU_ITEM)),
];

// Declared split trees nest `MAX_SPLIT_DEPTH` levels; the static key table is unrolled to match.
const PANE_L4: &[(&str, K)] = &[("cwd", K::Leaf), ("command", K::Leaf)];
const PANE_L3: &[(&str, K)] = &[
    ("cwd", K::Leaf),
    ("command", K::Leaf),
    ("split", K::Leaf),
    ("percent", K::Leaf),
    ("panes", K::Array(PANE_L4)),
];
const PANE_L2: &[(&str, K)] = &[
    ("cwd", K::Leaf),
    ("command", K::Leaf),
    ("split", K::Leaf),
    ("percent", K::Leaf),
    ("panes", K::Array(PANE_L3)),
];
const PANE_L1: &[(&str, K)] = &[
    ("cwd", K::Leaf),
    ("command", K::Leaf),
    ("split", K::Leaf),
    ("percent", K::Leaf),
    ("panes", K::Array(PANE_L2)),
];

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
            ("color_style", K::Leaf),
            ("palette", K::Free),
            (
                "color_rules",
                K::Array(&[("match", K::Leaf), ("color", K::Leaf)]),
            ),
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
    (
        "panes",
        K::Table(&[("inactive_dim", K::Leaf), ("focus_delay_ms", K::Leaf)]),
    ),
    ("clipboard", K::Table(&[("osc52", K::Leaf)])),
    ("paste", K::Table(&[("confirm_multiline", K::Leaf)])),
    ("bell", K::Table(&[("mode", K::Leaf)])),
    (
        "quick_select",
        K::Table(&[
            ("enabled", K::Leaf),
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
        K::Array(&[
            ("name", K::Leaf),
            ("command", K::Leaf),
            ("cwd", K::Leaf),
            ("color", K::Leaf),
        ]),
    ),
    (
        "workspaces",
        K::Array(&[
            ("name", K::Leaf),
            (
                "tabs",
                K::Array(&[
                    ("title", K::Leaf),
                    ("color", K::Leaf),
                    ("cwd", K::Leaf),
                    ("command", K::Leaf),
                    ("split", K::Leaf),
                    ("percent", K::Leaf),
                    ("panes", K::Array(PANE_L1)),
                ]),
            ),
        ]),
    ),
    (
        "serial_ports",
        K::Array(&[
            ("name", K::Leaf),
            ("port", K::Leaf),
            ("baud", K::Leaf),
            ("data_bits", K::Leaf),
            ("parity", K::Leaf),
            ("stop_bits", K::Leaf),
            ("flow", K::Leaf),
            ("local_echo", K::Leaf),
            ("line_ending", K::Leaf),
            ("color", K::Leaf),
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
            ("color", K::Leaf),
        ]),
    ),
    ("gpu", K::Table(&[("prefer", K::Leaf)])),
    (
        "ctl",
        K::Table(&[("capture_max_bytes", K::Leaf), ("status_max_len", K::Leaf)]),
    ),
    (
        "notifications",
        K::Table(&[("badge", K::Leaf), ("toast", K::Leaf), ("desktop", K::Leaf)]),
    ),
    (
        "terminal",
        K::Table(&[
            ("osc_color_report", K::Leaf),
            ("osc_notifications", K::Leaf),
            ("allow_shift_capture", K::Leaf),
        ]),
    ),
    (
        "sh",
        K::Table(&[
            ("color", K::Leaf),
            ("history_send_text", K::Leaf),
            ("confirm_prompts", K::Leaf),
        ]),
    ),
    (
        "snippets",
        K::Table(&[
            ("enabled", K::Leaf),
            ("templates", K::Leaf),
            ("panel_width", K::Leaf),
            ("panel_open_on_start", K::Leaf),
            ("confirm_multiline", K::Leaf),
            ("focus_terminal_after_insert", K::Leaf),
            ("preview_lines", K::Leaf),
        ]),
    ),
    (
        "recording",
        K::Table(&[
            ("enabled", K::Leaf),
            ("capture_input", K::Leaf),
            ("redact_no_echo", K::Leaf),
            ("max_duration_minutes", K::Leaf),
            ("max_size_mb", K::Leaf),
            ("panel_width", K::Leaf),
            ("panel_open_on_start", K::Leaf),
            ("confirm_delete", K::Leaf),
            (
                "export",
                K::Table(&[
                    ("default_format", K::Leaf),
                    ("output_dir", K::Leaf),
                    ("open_when_done", K::Leaf),
                    ("strip_input", K::Leaf),
                    ("gif_fps", K::Leaf),
                    ("gif_idle_limit", K::Leaf),
                    ("gif_font_size", K::Leaf),
                    ("gif_scale", K::Leaf),
                    ("gif_chrome", K::Leaf),
                    ("gif_end_pause", K::Leaf),
                    ("gif_max_size_mb", K::Leaf),
                ]),
            ),
        ]),
    ),
    ("menu_bar", K::Table(&[("enabled", K::Leaf)])),
    ("context_menu", K::Map(MENU_CONFIG)),
    ("menu_groups", K::Map(&[("items", K::Array(MENU_ITEM))])),
    (
        "menu_security",
        K::Table(&[
            ("dangerous", K::Leaf),
            ("confirm_panes", K::Leaf),
            ("trusted_dirs", K::Leaf),
            ("allow_remote_items", K::Leaf),
        ]),
    ),
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
            Some((_, K::Map(children))) => {
                if let Some(map) = item.as_table_like() {
                    for (name, entry) in map.iter() {
                        for t in tables_in(entry) {
                            check_unknown(src, t, children, &format!("{full}.{name}"), out);
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Semantic validation

/// The source span of a menu issue: the field if named, else the item's first key, else the
/// menu's own key.
fn menu_span(doc: &ImDocument<&str>, loc: &crate::menu::Loc) -> Option<Range<usize>> {
    let path: Vec<&str> = loc.table.iter().map(String::as_str).collect();
    let (parent, last) = path.split_at(path.len().saturating_sub(1));
    let key_span = |t: &dyn TableLike, key: &str| {
        t.get_key_value(key)
            .and_then(|(k, v)| k.span().or_else(|| v.span()))
    };
    let first_key = |t: &dyn TableLike| t.iter().next().and_then(|(k, _)| key_span(t, k));
    let table = item_at(doc, &path);
    let Some(i) = loc.item else {
        let own = item_at(doc, parent)
            .and_then(|p| p.as_table_like())
            .and_then(|p| key_span(p, last.first()?));
        return match (loc.field, table.and_then(|t| t.as_table_like())) {
            (Some(f), Some(t)) => t.get(f).and_then(|v| v.span()).or(own),
            _ => own,
        };
    };
    let items = table?.as_table_like()?.get("items")?;
    let mut t = *tables_in(items).get(i)?;
    if let Some(j) = loc.sub {
        t = *tables_in(t.get("submenu")?).get(j)?;
    }
    loc.field
        .and_then(|f| key_span(t, f))
        .or_else(|| first_key(t))
}

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
    validate_tab_colors(c, err);
    for issue in validate_workspaces(&c.workspaces, &c.startup_workspace) {
        let key = if issue.path == "startup_workspace" {
            "startup_workspace"
        } else {
            "workspaces"
        };
        err(&[key], format!("{}: {}", issue.path, issue.message));
    }
}

/// Whether `name` is a valid palette entry name (`[a-z][a-z0-9-]*`).
fn is_color_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn validate_tab_colors(c: &Config, err: &mut impl FnMut(&[&str], String)) {
    use mt_proto::color::{NamedColor, TabColor};
    let builtin: Vec<&str> = NamedColor::ALL.iter().map(|n| n.name()).collect();
    let mut extras: Vec<&str> = Vec::new();
    for (name, value) in &c.tab_bar.palette {
        if !is_color_name(name) {
            err(
                &["tab_bar", "palette"],
                format!("tab_bar.palette name \"{name}\" must match [a-z][a-z0-9-]*"),
            );
        }
        if !(value.starts_with('#') && TabColor::parse(value).is_ok()) {
            err(
                &["tab_bar", "palette"],
                format!("tab_bar.palette.{name} must be a #rrggbb color, got \"{value}\""),
            );
        }
        if !builtin.contains(&name.as_str()) {
            extras.push(name);
        }
    }
    if extras.len() > crate::schema::MAX_EXTRA_COLORS {
        err(
            &["tab_bar", "palette"],
            format!(
                "tab_bar.palette defines {} extra colors; at most {} are allowed",
                extras.len(),
                crate::schema::MAX_EXTRA_COLORS
            ),
        );
    }
    let known: Vec<&str> = builtin
        .iter()
        .copied()
        .chain(extras.iter().copied())
        .collect();
    let check = |what: &str, value: &str, path: &[&str], err: &mut dyn FnMut(&[&str], String)| {
        if known.contains(&value) || (value.starts_with('#') && TabColor::parse(value).is_ok()) {
            return;
        }
        let mut msg = if value.starts_with('#') {
            format!("{what}: \"{value}\" is not a #rrggbb color")
        } else {
            format!("{what}: unknown color \"{value}\"")
        };
        let best = known
            .iter()
            .map(|k| (*k, strsim::damerau_levenshtein(value, k)))
            .filter(|(_, d)| *d <= 2)
            .min_by_key(|(_, d)| *d);
        if let Some((k, _)) = best {
            msg.push_str(&format!(" (did you mean \"{k}\"?)"));
        }
        msg.push_str(&format!("; use {} or #rrggbb", known.join(", ")));
        err(path, msg);
    };
    if c.tab_bar.color_rules.len() > crate::schema::MAX_COLOR_RULES {
        err(
            &["tab_bar", "color_rules"],
            format!(
                "tab_bar.color_rules has {} entries; at most {} are allowed",
                c.tab_bar.color_rules.len(),
                crate::schema::MAX_COLOR_RULES
            ),
        );
    }
    for (i, r) in c.tab_bar.color_rules.iter().enumerate() {
        let what = format!("tab_bar.color_rules[{i}]");
        match mt_proto::Selector::parse(&r.selector).and_then(|s| s.check(mt_proto::Resource::Pane))
        {
            Ok(()) => {}
            Err(e) => err(
                &["tab_bar", "color_rules"],
                format!("{what}.match \"{}\": {}", r.selector, e.message),
            ),
        }
        check(
            &format!("{what}.color"),
            &r.color,
            &["tab_bar", "color_rules"],
            err,
        );
    }
    for (i, p) in c.launch.iter().enumerate() {
        if let Some(v) = &p.color {
            check(&format!("launch[{i}].color"), v, &["launch"], err);
        }
    }
    for (wi, w) in c.workspaces.iter().enumerate() {
        for (ti, t) in w.tabs.iter().enumerate() {
            if let Some(v) = &t.color {
                check(
                    &format!("workspaces[{wi}].tabs[{ti}].color"),
                    v,
                    &["workspaces"],
                    err,
                );
            }
        }
    }
    for (i, d) in c.ssh_domains.iter().enumerate() {
        if let Some(v) = &d.color {
            check(&format!("ssh_domains[{i}].color"), v, &["ssh_domains"], err);
        }
    }
    for (i, p) in c.serial_ports.iter().enumerate() {
        if let Some(v) = &p.color {
            check(
                &format!("serial_ports[{i}].color"),
                v,
                &["serial_ports"],
                err,
            );
        }
        validate_serial_port(i, p, err);
    }
}

fn validate_serial_port(
    i: usize,
    p: &crate::schema::SerialPort,
    err: &mut impl FnMut(&[&str], String),
) {
    let mut bad = |key: &str, what: &str, got: String| {
        err(
            &["serial_ports"],
            format!("serial_ports[{i}].{key} must be {what}, got \"{got}\""),
        )
    };
    if p.name.trim().is_empty() {
        bad("name", "non-empty", p.name.clone());
    }
    if p.port.trim().is_empty() {
        bad("port", "a device path", p.port.clone());
    }
    if p.baud == Some(0) {
        bad("baud", "a positive number", "0".into());
    }
    let enums: [(&str, &Option<String>, &[&str]); 5] = [
        ("data_bits", &p.data_bits, &["5", "6", "7", "8"]),
        ("parity", &p.parity, &["none", "odd", "even"]),
        ("stop_bits", &p.stop_bits, &["1", "2"]),
        ("flow", &p.flow, &["none", "software", "hardware"]),
        ("line_ending", &p.line_ending, &["cr", "lf", "crlf"]),
    ];
    for (key, value, allowed) in enums {
        if let Some(v) = value {
            if !allowed.contains(&v.to_ascii_lowercase().as_str()) {
                bad(key, &format!("one of {}", allowed.join(", ")), v.clone());
            }
        }
    }
}

/// One problem in a declared-workspace tree, addressed like `workspaces[0].tabs[1].panes[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIssue {
    pub path: String,
    pub message: String,
}

/// The single workspace validator, shared by config load and the workspace designer.
pub fn validate_workspaces(
    workspaces: &[crate::WorkspaceDef],
    startup: &str,
) -> Vec<WorkspaceIssue> {
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for (wi, w) in workspaces.iter().enumerate() {
        let wpath = format!("workspaces[{wi}]");
        if w.name.trim().is_empty() {
            out.push(WorkspaceIssue {
                path: wpath.clone(),
                message: "workspace name must not be empty".into(),
            });
        } else if seen.contains(&w.name.as_str()) {
            out.push(WorkspaceIssue {
                path: wpath.clone(),
                message: format!("workspace \"{}\" is defined more than once", w.name),
            });
        }
        seen.push(&w.name);
        for (ti, tab) in w.tabs.iter().enumerate() {
            let tpath = format!("{wpath}.tabs[{ti}]");
            let mut fail = |path: &str, msg: String| {
                out.push(WorkspaceIssue {
                    path: path.to_string(),
                    message: msg,
                })
            };
            if tab.split.is_none() {
                if tab.percent.is_some() || !tab.panes.is_empty() {
                    fail(&tpath, "`percent` and `panes` need `split`".into());
                }
            } else {
                validate_split(tab.split, tab.percent, &tab.panes, 1, &tpath, &mut fail);
                if !tab.command.is_empty() {
                    fail(&tpath, "a split tab takes `command` on its panes".into());
                }
            }
        }
    }
    if startup != "default" && !seen.contains(&startup) {
        out.push(WorkspaceIssue {
            path: "startup_workspace".into(),
            message: format!("startup_workspace \"{startup}\" is not defined in [[workspaces]]"),
        });
    }
    out
}

fn validate_split(
    split: Option<crate::SplitKind>,
    percent: Option<u8>,
    panes: &[crate::WorkspacePane],
    depth: usize,
    path: &str,
    fail: &mut impl FnMut(&str, String),
) {
    if split.is_none() {
        return;
    }
    if depth > crate::MAX_SPLIT_DEPTH {
        fail(
            path,
            format!("splits nest at most {} deep", crate::MAX_SPLIT_DEPTH),
        );
        return;
    }
    if panes.len() != 2 {
        fail(
            path,
            format!("a split needs exactly 2 `panes`, found {}", panes.len()),
        );
        return;
    }
    if percent.is_some_and(|p| !(1..=99).contains(&p)) {
        fail(path, "`percent` must be between 1 and 99".into());
    }
    for (i, p) in panes.iter().enumerate() {
        let ppath = format!("{path}.panes[{i}]");
        if p.split.is_none() && (p.percent.is_some() || !p.panes.is_empty()) {
            fail(&ppath, "`percent` and `panes` need `split`".into());
        }
        if p.split.is_some() && !p.command.is_empty() {
            fail(&ppath, "a split takes `command` on its panes".into());
        }
        validate_split(p.split, p.percent, &p.panes, depth + 1, &ppath, fail);
    }
}

fn validate(src: &str, doc: &ImDocument<&str>, c: &Config, out: &mut Vec<Diagnostic>) {
    let mut err = |path: &[&str], msg: String| {
        let span = item_at(doc, path).and_then(|i| i.span());
        out.push(Diagnostic::at(src, Severity::Error, span, msg));
    };

    if !(220..=600).contains(&c.snippets.panel_width) {
        err(
            &["snippets", "panel_width"],
            "snippets.panel_width must be between 220 and 600".into(),
        );
    }
    if c.snippets.preview_lines > 5 {
        err(
            &["snippets", "preview_lines"],
            "snippets.preview_lines must be between 0 and 5".into(),
        );
    }
    if !(280..=600).contains(&c.recording.panel_width) {
        err(
            &["recording", "panel_width"],
            "recording.panel_width must be between 280 and 600".into(),
        );
    }
    if !(1..=30).contains(&c.recording.export.gif_fps) {
        err(
            &["recording", "export", "gif_fps"],
            "recording.export.gif_fps must be between 1 and 30".into(),
        );
    }
    if !(0.25..=2.0).contains(&c.recording.export.gif_scale) {
        err(
            &["recording", "export", "gif_scale"],
            "recording.export.gif_scale must be between 0.25 and 2.0".into(),
        );
    }
    if c.recording.export.gif_idle_limit.is_nan() || c.recording.export.gif_idle_limit < 0.0 {
        err(
            &["recording", "export", "gif_idle_limit"],
            "recording.export.gif_idle_limit must not be negative".into(),
        );
    }
    if c.recording.export.gif_end_pause.is_nan() || c.recording.export.gif_end_pause < 0.0 {
        err(
            &["recording", "export", "gif_end_pause"],
            "recording.export.gif_end_pause must not be negative".into(),
        );
    }
    if !matches!(
        c.recording.export.gif_chrome.as_str(),
        "none" | "plain" | "macos"
    ) {
        err(
            &["recording", "export", "gif_chrome"],
            "recording.export.gif_chrome must be \"none\", \"plain\" or \"macos\"".into(),
        );
    }
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
    if !(0.0..=1.0).contains(&c.panes.inactive_dim) {
        err(
            &["panes", "inactive_dim"],
            "panes.inactive_dim must be between 0.0 and 1.0".into(),
        );
    }
    if c.panes.focus_delay_ms > 5000 {
        err(
            &["panes", "focus_delay_ms"],
            "panes.focus_delay_ms must be at most 5000".into(),
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
    if c.ctl.capture_max_bytes == 0 {
        err(
            &["ctl", "capture_max_bytes"],
            "ctl.capture_max_bytes must be greater than 0".into(),
        );
    }
    if c.ctl.status_max_len == 0 {
        err(
            &["ctl", "status_max_len"],
            "ctl.status_max_len must be greater than 0".into(),
        );
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
