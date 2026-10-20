//! Turns an [`Output`] into text: the plain forms (TSV, `key<TAB>value`, raw JSON) used when piped,
//! in scripts and with `--json`, and the rich terminal forms (tables with a footer, key/value
//! cards, a box-drawn tree, colour) of SPEC §12.7. A pure function of the output and
//! [`RenderOpts`], shared by `ctl` and `sh`.

use mt_proto::{MuxSnapshot, PaneInfo};

use crate::output::{Body, ColKind, CtlError, Output, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOpts {
    /// Terminal width in columns.
    pub width: usize,
    /// Use the 16 ANSI colours on stdout.
    pub color: bool,
    /// stdout is a terminal that gets the rich forms.
    pub tty: bool,
    /// Box-drawing and symbols; false falls back to ASCII.
    pub unicode: bool,
    /// Colour and symbols for errors on stderr.
    pub err_color: bool,
}

impl RenderOpts {
    /// The plain forms: no colour, no layout.
    #[cfg(test)]
    pub const PLAIN: RenderOpts = RenderOpts {
        width: 80,
        color: false,
        tty: false,
        unicode: true,
        err_color: false,
    };

    /// Options for the real stdout/stderr of this process. `no_color` is `--no-color` (or the
    /// `[sh] color = "never"` setting); `NO_COLOR` is always honoured.
    pub fn detect(no_color: bool, force_plain: bool) -> RenderOpts {
        use std::io::IsTerminal;
        let no_color = no_color || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let tty = !force_plain && std::io::stdout().is_terminal();
        let utf8 = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty())
            .is_none_or(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("utf-8") || v.contains("utf8")
            });
        RenderOpts {
            width: term_width().unwrap_or(80),
            color: tty && !no_color,
            tty,
            unicode: utf8,
            err_color: !no_color && std::io::stderr().is_terminal(),
        }
    }
}

/// Columns of stdout when it is a terminal.
fn term_width() -> Option<usize> {
    term_size().map(|(cols, _)| cols)
}

/// `(columns, rows)` of stdout when it is a terminal.
#[allow(unsafe_code)]
pub fn term_size() -> Option<(usize, usize)> {
    let mut ws: libc::winsize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ fills a `winsize` and touches nothing else.
    let rc = unsafe { libc::ioctl(1, libc::TIOCGWINSZ as _, &mut ws) };
    (rc == 0 && ws.ws_col > 0).then_some((ws.ws_col as usize, ws.ws_row as usize))
}

const BOLD: &str = "1";
const DIM: &str = "2";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";

fn paint(s: &str, code: &str, on: bool) -> String {
    if on && !s.is_empty() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn width_of(s: &str) -> usize {
    s.chars().count()
}

fn truncate(s: &str, max: usize, unicode: bool) -> String {
    if width_of(s) <= max {
        return s.to_string();
    }
    let ell = if unicode { '…' } else { '~' };
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push(ell);
    out
}

/// The text for stdout.
pub fn render(out: &Output, o: &RenderOpts) -> String {
    match &out.body {
        Body::None => String::new(),
        Body::Json(v) => format!("{}\n", serde_json::to_string(v).unwrap_or_default()),
        Body::Text(t) => format!("{t}\n"),
        Body::Raw(r) => r.clone(),
        Body::Spawned { kind, spawned } => {
            if o.tty && *kind == "pane" {
                let mark = paint(if o.unicode { "✔" } else { "ok" }, GREEN, o.color);
                format!(
                    "{mark} pane {} created (tab {}, window {})\n",
                    paint(&spawned.pane.to_string(), CYAN, o.color),
                    paint(&spawned.tab.to_string(), CYAN, o.color),
                    paint(&spawned.window.to_string(), CYAN, o.color),
                )
            } else if o.tty {
                let mark = paint(if o.unicode { "✔" } else { "ok" }, GREEN, o.color);
                format!(
                    "{mark} {kind} {} created (pane {}, window {})\n",
                    paint(&spawned.tab.to_string(), CYAN, o.color),
                    paint(&spawned.pane.to_string(), CYAN, o.color),
                    paint(&spawned.window.to_string(), CYAN, o.color),
                )
            } else {
                format!(
                    "tab={} pane={} window={}\n",
                    spawned.tab, spawned.pane, spawned.window
                )
            }
        }
        Body::Table(t) if o.tty => table_rich(t, o),
        Body::Table(t) => t
            .rows
            .iter()
            .map(|r| format!("{}\n", r.join("\t")))
            .collect(),
        Body::Card(kv) if o.tty => card_rich(kv, o),
        Body::Card(kv) => kv.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect(),
        Body::Tree(s) if o.tty => tree_rich(s, o),
        Body::Tree(s) => tree_plain(s),
    }
}

/// The text for stderr (a trailing newline included, empty for a silent error).
pub fn render_error(e: &CtlError, json: bool, o: &RenderOpts) -> String {
    let plain = e.render_plain(json);
    if plain.is_empty() {
        return String::new();
    }
    if json || !o.err_color {
        return format!("{plain}\n");
    }
    let cross = if o.unicode { "✖" } else { "x" };
    let mut s = format!("{} {}\n", paint(cross, RED, true), error_message(e));
    if let Some(rpc) = &e.rpc {
        for c in &rpc.candidates {
            s.push_str(&format!("  {}\n", paint(c, DIM, true)));
        }
    }
    s
}

fn error_message(e: &CtlError) -> &str {
    match &e.rpc {
        Some(rpc) if !rpc.candidates.is_empty() || e.is_confirm_required() => &rpc.message,
        _ => &e.message,
    }
}

/// A warning line (`⚠ …`) for stderr.
pub fn render_warning(msg: &str, o: &RenderOpts) -> String {
    let mark = if o.unicode { "⚠" } else { "!" };
    format!("{} {msg}\n", paint(mark, YELLOW, o.err_color))
}

fn cell_text(kind: ColKind, raw: &str, o: &RenderOpts) -> String {
    match kind {
        ColKind::Flag if raw == "*" => (if o.unicode { "●" } else { "*" }).to_string(),
        _ => raw.to_string(),
    }
}

fn paint_cell(kind: ColKind, text: &str, o: &RenderOpts) -> String {
    match kind {
        ColKind::Id => paint(text, CYAN, o.color),
        ColKind::Flag => paint(text, GREEN, o.color),
        ColKind::Status => {
            let code = match text.split_whitespace().next().unwrap_or("") {
                "working" => CYAN,
                "waiting" => YELLOW,
                "done" => GREEN,
                "error" => RED,
                _ => DIM,
            };
            paint(text, code, o.color)
        }
        ColKind::Text => text.to_string(),
    }
}

fn table_rich(t: &Table, o: &RenderOpts) -> String {
    let n = t.columns.len();
    let cells: Vec<Vec<String>> = t
        .rows
        .iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, c)| cell_text(t.columns[i].kind, c, o))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = (0..n)
        .map(|i| {
            cells
                .iter()
                .map(|r| width_of(&r[i]))
                .chain([width_of(t.columns[i].name)])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut shown: Vec<usize> = (0..n).collect();
    let total = |shown: &[usize], widths: &[usize]| {
        shown.iter().map(|&i| widths[i]).sum::<usize>() + 2 * shown.len().saturating_sub(1)
    };
    // Low-priority columns go before anything is truncated.
    while total(&shown, &widths) > o.width {
        let victim = shown
            .iter()
            .copied()
            .filter(|&i| t.columns[i].drop_rank > 0)
            .max_by_key(|&i| t.columns[i].drop_rank);
        match victim {
            Some(v) => shown.retain(|&i| i != v),
            None => break,
        }
    }
    if total(&shown, &widths) > o.width {
        if let Some(&last) = shown.last() {
            let others = total(&shown, &widths) - widths[last];
            widths[last] = o.width.saturating_sub(others).max(4).min(widths[last]);
        }
    }
    let last = shown.last().copied();
    let line = |texts: Vec<(usize, String, String)>| {
        let mut parts = Vec::new();
        for (i, plain, painted) in texts {
            let w = widths[i];
            let pad = w.saturating_sub(width_of(&plain));
            let right = t.columns[i].kind == ColKind::Id;
            if Some(i) == last && !right {
                parts.push(painted);
            } else if right {
                parts.push(format!("{}{painted}", " ".repeat(pad)));
            } else {
                parts.push(format!("{painted}{}", " ".repeat(pad)));
            }
        }
        parts.join("  ")
    };
    let mut out = String::new();
    let header = shown
        .iter()
        .map(|&i| {
            let name = truncate(t.columns[i].name, widths[i], o.unicode);
            let painted = paint(&name, BOLD, o.color);
            (i, name, painted)
        })
        .collect();
    out.push_str(&line(header));
    out.push('\n');
    for (r, row) in cells.iter().enumerate() {
        let dim = t.dim_rows.get(r).copied().unwrap_or(false);
        let texts = shown
            .iter()
            .map(|&i| {
                let text = truncate(&row[i], widths[i], o.unicode);
                let painted = if dim {
                    paint(&text, DIM, o.color)
                } else {
                    paint_cell(t.columns[i].kind, &text, o)
                };
                (i, text, painted)
            })
            .collect();
        out.push_str(&line(texts));
        out.push('\n');
    }
    let count = t.rows.len();
    let footer = format!("{count} {}{}", t.noun, if count == 1 { "" } else { "s" });
    out.push_str(&paint(&footer, DIM, o.color));
    out.push('\n');
    out
}

fn card_rich(kv: &[(String, String)], o: &RenderOpts) -> String {
    let w = kv.iter().map(|(k, _)| width_of(k)).max().unwrap_or(0);
    let mut out = String::new();
    for (k, v) in kv {
        let pad = " ".repeat(w - width_of(k));
        let value = match k.as_str() {
            "id" | "tab" | "window" => paint(v, CYAN, o.color),
            "exited" if v == "true" => paint(v, RED, o.color),
            "status" => paint_cell(ColKind::Status, v, o),
            _ => v.clone(),
        };
        out.push_str(&format!("{}{pad}  {value}\n", paint(k, DIM, o.color)));
    }
    out
}

/// `1 | (2 / 3)`: `|` puts panes side by side, `/` stacks them; a split that is not 50/50
/// shows the first pane's share (`1 |40 2`).
pub fn layout_text(l: &mt_proto::LayoutNode) -> String {
    fn go(l: &mt_proto::LayoutNode, top: bool) -> String {
        match l {
            mt_proto::LayoutNode::Leaf { pane } => pane.to_string(),
            mt_proto::LayoutNode::Split {
                axis,
                percent,
                a,
                b,
            } => {
                let op = match axis {
                    mt_proto::SplitAxis::Horizontal => "|",
                    mt_proto::SplitAxis::Vertical => "/",
                };
                let op = if *percent == 50 {
                    op.to_string()
                } else {
                    format!("{op}{percent}")
                };
                let s = format!("{} {op} {}", go(a, false), go(b, false));
                if top {
                    s
                } else {
                    format!("({s})")
                }
            }
        }
    }
    go(l, true)
}

fn tab_layout(t: &mt_proto::TabNode) -> Option<String> {
    (t.panes.len() > 1)
        .then(|| t.layout.as_ref().map(layout_text))
        .flatten()
}

fn tree_plain(s: &MuxSnapshot) -> String {
    let mut out = String::new();
    for ws in &s.workspaces {
        out.push_str(&format!(
            "workspace {}{}\n",
            ws.name,
            if ws.active { " (active)" } else { "" }
        ));
        for w in &ws.windows {
            out.push_str(&format!(
                "  window {}{}\n",
                w.id,
                if w.focused { " (focused)" } else { "" }
            ));
            for t in &w.tabs {
                out.push_str(&format!(
                    "    tab {} {:?}{}{}\n",
                    t.id,
                    t.title,
                    if t.active { " (active)" } else { "" },
                    tab_layout(t)
                        .map(|l| format!("  [{l}]"))
                        .unwrap_or_default()
                ));
                for p in &t.panes {
                    out.push_str(&format!(
                        "      pane {}  {}{}\n",
                        p.id,
                        p.domain,
                        p.cwd
                            .as_deref()
                            .map(|c| format!("  {c}"))
                            .unwrap_or_default()
                    ));
                }
            }
        }
    }
    out
}

struct Node {
    label: String,
    children: Vec<Node>,
}

fn emit(nodes: &[Node], prefix: &str, o: &RenderOpts, out: &mut String) {
    let (tee, elbow, bar) = if o.unicode {
        ("├─ ", "└─ ", "│  ")
    } else {
        ("|- ", "`- ", "|  ")
    };
    for (i, n) in nodes.iter().enumerate() {
        let last = i + 1 == nodes.len();
        out.push_str(&format!(
            "{prefix}{}{}\n",
            if last { elbow } else { tee },
            n.label
        ));
        let next = format!("{prefix}{}", if last { "   " } else { bar });
        emit(&n.children, &next, o, out);
    }
}

fn pane_label(p: &PaneInfo, o: &RenderOpts) -> String {
    let focus = if p.focused {
        if o.unicode {
            "● "
        } else {
            "* "
        }
    } else {
        ""
    };
    let mut s = format!(
        "{focus}pane {}  {}",
        paint(&p.id.to_string(), CYAN, o.color),
        p.domain
    );
    if !p.title.is_empty() {
        s.push_str(&format!("  {}", p.title));
    }
    if let Some(cwd) = &p.cwd {
        s.push_str(&format!("  {}", paint(cwd, DIM, o.color)));
    }
    if p.zoomed {
        s.push_str(&format!("  {}", paint("(zoomed)", DIM, o.color)));
    }
    if p.exited {
        let code = p.exit_code.map(|c| format!(" {c}")).unwrap_or_default();
        return paint(&format!("{s}  exited{code}"), DIM, o.color);
    }
    if let Some(st) = &p.status {
        s.push_str(&format!(
            "  {}",
            paint_cell(ColKind::Status, st.state.as_str(), o)
        ));
    }
    s
}

fn tree_rich(snap: &MuxSnapshot, o: &RenderOpts) -> String {
    let active = if o.unicode { "▸ " } else { "> " };
    let focus = if o.unicode { "● " } else { "* " };
    let nodes: Vec<Node> = snap
        .workspaces
        .iter()
        .map(|ws| Node {
            label: format!(
                "{}{}",
                if ws.active { active } else { "" },
                paint(&format!("workspace {}", ws.name), BOLD, o.color)
            ),
            children: ws
                .windows
                .iter()
                .map(|w| Node {
                    label: format!(
                        "{}window {}",
                        if w.focused { focus } else { "" },
                        paint(&w.id.to_string(), CYAN, o.color)
                    ),
                    children: w
                        .tabs
                        .iter()
                        .map(|t| Node {
                            label: format!(
                                "{}tab {}  {}{}",
                                if t.active { active } else { "" },
                                paint(&t.id.to_string(), CYAN, o.color),
                                t.title,
                                tab_layout(t)
                                    .map(|l| format!(
                                        "  {}",
                                        paint(&format!("[{l}]"), DIM, o.color)
                                    ))
                                    .unwrap_or_default()
                            ),
                            children: t
                                .panes
                                .iter()
                                .map(|p| Node {
                                    label: pane_label(p, o),
                                    children: vec![],
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    let mut out = String::new();
    // Roots are drawn without a branch glyph.
    for n in &nodes {
        out.push_str(&format!("{}\n", n.label));
        emit(&n.children, "", o, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Column;
    use mt_proto::Spawned;

    const RICH: RenderOpts = RenderOpts {
        width: 40,
        color: false,
        tty: true,
        unicode: true,
        err_color: false,
    };

    fn table() -> Table {
        Table {
            columns: vec![
                Column::new("FOCUSED", ColKind::Flag),
                Column::new("ID", ColKind::Id),
                Column::low("DOMAIN", ColKind::Text, 1),
                Column::new("TITLE", ColKind::Text),
            ],
            rows: vec![
                vec!["*".into(), "7".into(), "local".into(), "zsh".into()],
                vec![
                    "".into(),
                    "12".into(),
                    "ssh:prod".into(),
                    "cargo watch".into(),
                ],
            ],
            dim_rows: vec![],
            noun: "pane",
        }
    }

    fn out(body: Body) -> Output {
        Output::new(body)
    }

    #[test]
    fn plain_table_is_tsv_without_header() {
        let s = render(&out(Body::Table(table())), &RenderOpts::PLAIN);
        assert_eq!(s, "*\t7\tlocal\tzsh\n\t12\tssh:prod\tcargo watch\n");
    }

    #[test]
    fn rich_table_aligns_ids_and_counts_rows() {
        let mut o = RICH;
        o.width = 60;
        let s = render(&out(Body::Table(table())), &o);
        assert_eq!(
            s,
            "FOCUSED  ID  DOMAIN    TITLE\n●         7  local     zsh\n         12  ssh:prod  cargo watch\n2 panes\n"
        );
    }

    #[test]
    fn rich_table_drops_low_priority_columns_before_truncating() {
        let mut o = RICH;
        o.width = 30;
        let s = render(&out(Body::Table(table())), &o);
        assert!(!s.contains("DOMAIN"), "{s}");
        assert!(s.contains("cargo watch"), "{s}");
    }

    #[test]
    fn rich_table_truncates_the_last_column_as_a_last_resort() {
        let mut o = RICH;
        o.width = 20;
        let s = render(&out(Body::Table(table())), &o);
        assert!(s.contains('…'), "{s}");
        assert!(s.lines().all(|l| l.chars().count() <= 20), "{s}");
    }

    #[test]
    fn ascii_fallback_has_no_unicode() {
        let mut o = RICH;
        o.unicode = false;
        o.width = 60;
        let s = render(&out(Body::Table(table())), &o);
        assert!(s.is_ascii(), "{s}");
    }

    #[test]
    fn colour_is_only_emitted_when_asked() {
        let mut o = RICH;
        o.width = 60;
        assert!(!render(&out(Body::Table(table())), &o).contains('\x1b'));
        o.color = true;
        assert!(render(&out(Body::Table(table())), &o).contains("\x1b[36m"));
    }

    #[test]
    fn card_forms() {
        let kv = vec![
            ("id".to_string(), "7".to_string()),
            ("title".into(), "zsh".into()),
        ];
        assert_eq!(
            render(&out(Body::Card(kv.clone())), &RenderOpts::PLAIN),
            "id\t7\ntitle\tzsh\n"
        );
        assert_eq!(
            render(&out(Body::Card(kv)), &RICH),
            "id     7\ntitle  zsh\n"
        );
    }

    #[test]
    fn spawned_forms() {
        let b = Body::Spawned {
            kind: "tab",
            spawned: Spawned {
                pane: 12,
                tab: 9,
                window: 1,
            },
        };
        assert_eq!(
            render(&out(b.clone()), &RenderOpts::PLAIN),
            "tab=9 pane=12 window=1\n"
        );
        assert_eq!(
            render(&out(b), &RICH),
            "✔ tab 9 created (pane 12, window 1)\n"
        );
    }

    #[test]
    fn json_text_and_raw_are_verbatim() {
        let v = serde_json::json!({"a": 1});
        assert_eq!(render(&out(Body::Json(v)), &RICH), "{\"a\":1}\n");
        assert_eq!(render(&out(Body::Text("hi".into())), &RICH), "hi\n");
        assert_eq!(render(&out(Body::Raw("a\nb".into())), &RICH), "a\nb");
        assert_eq!(render(&out(Body::None), &RICH), "");
    }

    fn snapshot() -> MuxSnapshot {
        serde_json::from_value(serde_json::json!({
            "workspaces": [{
                "id": 1, "name": "default", "active": true,
                "windows": [{
                    "id": 1, "title": "w", "active": true, "focused": true,
                    "tabs": [{
                        "id": 3, "window": 1, "title": "zsh", "active": true, "focused": true,
                        "domain": "local", "panes": [
                            {"id": 5, "tab": 3, "window": 1, "workspace": "default",
                             "domain": "local", "title": "zsh", "cwd": "/tmp",
                             "size": {"cols": 80, "rows": 24}, "focused": true,
                             "exited": false},
                            {"id": 6, "tab": 3, "window": 1, "workspace": "default",
                             "domain": "local", "title": "", "cwd": null,
                             "size": {"cols": 80, "rows": 24}, "focused": false,
                             "exited": true, "exit_code": 1}
                        ]
                    }]
                }]
            }]
        }))
        .expect("fixture matches the protocol types")
    }

    #[test]
    fn plain_tree_is_indented() {
        let s = render(&out(Body::Tree(snapshot())), &RenderOpts::PLAIN);
        assert_eq!(
            s,
            "workspace default (active)\n  window 1 (focused)\n    tab 3 \"zsh\" (active)\n      pane 5  local  /tmp\n      pane 6  local\n"
        );
    }

    #[test]
    fn rich_tree_is_box_drawn() {
        let s = render(&out(Body::Tree(snapshot())), &RICH);
        assert_eq!(
            s,
            "▸ workspace default\n└─ ● window 1\n   └─ ▸ tab 3  zsh\n      ├─ ● pane 5  local  zsh  /tmp\n      └─ pane 6  local  exited 1\n"
        );
        let mut o = RICH;
        o.unicode = false;
        assert!(render(&out(Body::Tree(snapshot())), &o).is_ascii());
    }

    #[test]
    fn rich_errors_use_a_cross_and_plain_ones_do_not() {
        let e = CtlError::new(3, "no such pane");
        assert_eq!(render_error(&e, false, &RICH), "microterm: no such pane\n");
        let mut o = RICH;
        o.err_color = true;
        assert!(render_error(&e, false, &o).contains("✖"));
        assert!(render_error(&e, true, &o).starts_with("{\"error\""));
        assert_eq!(render_error(&CtlError::silent(2), false, &o), "");
    }
}
