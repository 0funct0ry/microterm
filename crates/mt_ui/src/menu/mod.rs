//! The context-menu engine (SPEC §10.9), GPUI-free so the GUI and the headless host share it:
//! facts for `when`, row resolution, built-in operations and invocation planning. Rendering and
//! execution of GUI-only effects live in `menu_view.rs`.

pub mod expand;
pub mod nav;
pub mod shell;

use std::collections::BTreeMap;

use mt_config::menu::{
    self as reg, BuiltinDef, Confirm, Entry, Kind, MenuItem, Quote, SpawnWhere, WhenFalse,
    CONTENT_PLACEHOLDERS,
};
use mt_config::{ActionRef, Config};
use mt_mux::ctl::{PaneRow, Rows};
use mt_mux::{Dir, Id};
use mt_proto::when::{FactValue, When, WhenKey};
use mt_proto::{Request, Target};
use mt_term::Terminal;
use serde_json::{json, Value};

use expand::{ExpandError, Values};

/// Runs a `ctl` item's argv against the instance at `socket` as if from `pane`; returns the
/// command's plain output or its error. `mt_ui` cannot depend on the ctl grammar, so the
/// `microterm` binary installs it. It blocks, so hosts call it off the main thread.
pub type CtlRunner =
    fn(argv: Vec<String>, pane: Id, socket: Option<std::path::PathBuf>) -> Result<String, String>;

static CTL_RUNNER: std::sync::OnceLock<CtlRunner> = std::sync::OnceLock::new();

pub fn install_ctl_runner(f: CtlRunner) {
    let _ = CTL_RUNNER.set(f);
}

pub fn ctl_runner() -> Option<CtlRunner> {
    CTL_RUNNER.get().copied()
}

/// Records an invocation with a side effect: a `menu.invoked` event and a log line.
pub fn audit(mux: &mut mt_mux::Mux, inv: &Invocation) {
    let Some(kind) = inv.kind.filter(|k| k.has_side_effect()) else {
        return;
    };
    let s = summary(inv);
    eprintln!(
        "microterm: menu {}/{} on pane {}: {s}",
        inv.menu, inv.item, inv.pane
    );
    mux.notify_menu_invoked(inv.pane, &inv.menu, &inv.item, kind.name(), &s);
}

/// What a right-click hit, beyond the pane: the click facts of the `viewport` menu.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Click {
    pub link: Option<String>,
    /// The clicked row's text and the clicked column (char index).
    pub line: Option<(String, usize)>,
    /// The clicked row is an OSC 133 prompt row / lies in a command's output.
    pub on_prompt: bool,
    pub in_output: bool,
}

/// Everything a menu needs to know about the pane (and click) it was opened for.
#[derive(Debug, Clone)]
pub struct Facts {
    pub pane: Id,
    pub tab: Id,
    pub window: Id,
    pub remote: bool,
    pub row: PaneRow,
    facts: BTreeMap<WhenKey, FactValue>,
    pub values: Values,
    /// Items registered by programs in the pane and project items (SPEC §10.9.7).
    pub shell: shell::ShellState,
    /// The tab's manual color, for the check mark of the `tab/color` swatches.
    pub manual_color: Option<mt_mux::TabColor>,
    /// `snippets.enabled`: the "Save selection as snippet…" row exists only while it is on.
    pub snippets_enabled: bool,
    /// `recording.enabled`: the "Start/Stop recording" row exists only while it is on.
    pub recording_enabled: bool,
}

impl Facts {
    /// Facts of `pane` from the mux snapshot and its terminal (when the host has one).
    pub fn for_pane(cfg: &Config, rows: &Rows, pane: Id, term: Option<&Terminal>) -> Option<Facts> {
        let row = rows.panes.iter().find(|p| p.id == pane)?.clone();
        let mut f = BTreeMap::new();
        let mut v = Values::default();
        let remote = row.domain != "local";
        f.insert(WhenKey::Title, FactValue::Text(row.title.clone()));
        // The mux learns a pane's cwd by polling, so a new pane may not have one yet: fall back
        // to what the terminal reports (OSC 7), then to the process's own cwd.
        let cwd = row.cwd.clone().or_else(|| {
            term.and_then(|t| {
                t.reported_cwd()
                    .or_else(|| t.cwd().map(|p| p.to_string_lossy().into_owned()))
            })
        });
        if let Some(cwd) = cwd {
            f.insert(WhenKey::Cwd, FactValue::Text(cwd.clone()));
            v.set("cwd", cwd);
        }
        if let Some(p) = &row.process {
            f.insert(WhenKey::Cmd, FactValue::Text(p.clone()));
        }
        f.insert(WhenKey::Domain, FactValue::Text(row.domain.clone()));
        f.insert(
            WhenKey::Status,
            FactValue::Text(
                row.status
                    .as_ref()
                    .map_or("idle", |s| s.state.as_str())
                    .to_string(),
            ),
        );
        f.insert(WhenKey::Exited, FactValue::Bool(row.exited));
        f.insert(WhenKey::Focused, FactValue::Bool(row.focused));
        f.insert(WhenKey::Zoomed, FactValue::Bool(row.zoomed));
        f.insert(WhenKey::Remote, FactValue::Bool(remote));
        f.insert(WhenKey::Local, FactValue::Bool(!remote));
        let panes = rows
            .tabs
            .iter()
            .find(|t| t.id == row.tab)
            .map_or(1, |t| t.panes.len());
        let tabs = rows
            .windows
            .iter()
            .find(|w| w.id == row.window)
            .map_or(1, |w| w.tabs.len());
        f.insert(WhenKey::Panes, FactValue::Number(panes as i64));
        f.insert(WhenKey::Tabs, FactValue::Number(tabs as i64));
        v.set("pane", row.id.to_string());
        v.set("tab", row.tab.to_string());
        v.set("window", row.window.to_string());
        v.set("title", row.title.clone());
        v.set("domain", row.domain.clone());
        if let Some(d) = cfg.ssh_domains.iter().find(|d| d.name == row.domain) {
            v.set("host", d.host.clone());
        } else if !remote {
            v.set("host", "localhost");
        }
        if let Some(t) = term {
            if let Some(sel) = t.selection_text(cfg.selection.trim_trailing_whitespace) {
                if !sel.is_empty() {
                    v.set("selection", sel.clone());
                }
                f.insert(WhenKey::Selection, FactValue::Text(sel));
            }
            let modes = t.modes();
            f.insert(WhenKey::AltScreen, FactValue::Bool(modes.alt_screen));
            f.insert(
                WhenKey::MouseReporting,
                FactValue::Bool(modes.mouse_reporting),
            );
            if t.has_marks() {
                let busy = t.busy();
                f.insert(WhenKey::Idle, FactValue::Bool(!busy));
                f.insert(WhenKey::Running, FactValue::Bool(busy));
                if let Some(c) = t.last_command(1) {
                    v.set("last_command", c.command.clone());
                    v.set("last_output", c.output.clone());
                    if let Some(code) = c.exit_code {
                        f.insert(WhenKey::LastExit, FactValue::Number(i64::from(code)));
                        v.set("exit_code", code.to_string());
                    }
                }
            }
        }
        let shell = shell::collect(cfg, remote, v.get("cwd"), term);
        Some(Facts {
            pane,
            tab: row.tab,
            window: row.window,
            remote,
            row,
            facts: f,
            values: v,
            shell,
            manual_color: None,
            snippets_enabled: crate::snippets::gate::SnippetsGate::of_config(cfg).enabled,
            recording_enabled: crate::recording::gate::RecordingGate::of_config(cfg).enabled,
        })
    }

    /// Adds the click facts (`viewport` only).
    pub fn with_click(mut self, click: &Click, word_chars: &str) -> Self {
        if let Some(link) = &click.link {
            self.facts
                .insert(WhenKey::Link, FactValue::Text(link.clone()));
            self.values.set("link", link.clone());
        }
        if let Some((line, col)) = &click.line {
            self.facts
                .insert(WhenKey::Line, FactValue::Text(line.clone()));
            self.values.set("line", line.trim_end().to_string());
            if let Some(w) = mt_patterns::word_at(line, *col, word_chars) {
                self.facts.insert(WhenKey::Word, FactValue::Text(w.clone()));
                self.values.set("word", w);
            }
            if let Some((path, n)) = mt_patterns::path_at(line, *col) {
                self.values.set(
                    "path:line",
                    n.map_or_else(|| path.clone(), |n| format!("{path}:{n}")),
                );
                self.values.set("path", path);
            }
        }
        self.facts
            .insert(WhenKey::OnPrompt, FactValue::Bool(click.on_prompt));
        self.facts
            .insert(WhenKey::InOutput, FactValue::Bool(click.in_output));
        self
    }

    pub fn fact(&self, k: WhenKey) -> Option<FactValue> {
        self.facts.get(&k).cloned()
    }

    fn holds(&self, when: Option<&str>) -> bool {
        when.and_then(|w| When::parse(w).ok())
            .is_none_or(|w| w.eval(&|k| self.fact(k)))
    }
}

// ---------------------------------------------------------------------------------------------
// Built-ins

/// The operation behind a built-in row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Copy,
    Paste,
    SelectAll,
    OpenLink,
    CopyLink,
    Search,
    ClearScrollback,
    Zoom,
    Swap(Dir),
    Rotate,
    Split(Dir),
    ClosePane,
    CloseTab,
    CloseOtherTabs,
    RenameTab,
    SaveAsSnippet,
    ToggleRecording,
}

impl Builtin {
    pub fn from_id(menu: &str, id: &str) -> Option<Builtin> {
        Some(match (menu, id) {
            ("viewport", "copy") => Builtin::Copy,
            ("viewport", "paste") => Builtin::Paste,
            ("viewport", "select_all") => Builtin::SelectAll,
            ("viewport", "open_link") => Builtin::OpenLink,
            ("viewport", "copy_link") => Builtin::CopyLink,
            ("viewport", "search") => Builtin::Search,
            ("viewport", "clear_scrollback") => Builtin::ClearScrollback,
            ("viewport", "save_as_snippet") => Builtin::SaveAsSnippet,
            ("viewport", "toggle_recording") => Builtin::ToggleRecording,
            ("viewport", "zoom") => Builtin::Zoom,
            ("viewport", "swap_left") => Builtin::Swap(Dir::Left),
            ("viewport", "swap_right") => Builtin::Swap(Dir::Right),
            ("viewport", "swap_up") => Builtin::Swap(Dir::Up),
            ("viewport", "swap_down") => Builtin::Swap(Dir::Down),
            ("viewport", "rotate") => Builtin::Rotate,
            ("viewport", "split_left") => Builtin::Split(Dir::Left),
            ("viewport", "split_right") => Builtin::Split(Dir::Right),
            ("viewport", "split_up") => Builtin::Split(Dir::Up),
            ("viewport", "split_down") => Builtin::Split(Dir::Down),
            ("viewport", "close_pane") => Builtin::ClosePane,
            ("tab", "close") => Builtin::CloseTab,
            ("tab", "close_others") => Builtin::CloseOtherTabs,
            ("tab", "rename") => Builtin::RenameTab,
            _ => return None,
        })
    }

    /// The IPC request that performs this built-in, for those a headless host can run too.
    pub fn request(self, f: &Facts, force: bool) -> Option<Request> {
        let target = Target {
            force,
            ..Target::pane(f.pane)
        };
        let dir = |d: Dir| match d {
            Dir::Left => mt_proto::Direction::Left,
            Dir::Right => mt_proto::Direction::Right,
            Dir::Up => mt_proto::Direction::Up,
            Dir::Down => mt_proto::Direction::Down,
        };
        Some(match self {
            Builtin::Zoom => Request::PaneZoom(target),
            Builtin::Rotate => Request::PaneRotate(target),
            Builtin::Swap(d) => Request::PaneSwap(mt_proto::PaneDirParams {
                target,
                direction: dir(d),
            }),
            Builtin::Split(d) => Request::PaneSplit(mt_proto::PaneSplitParams {
                target,
                axis: match d.axis() {
                    mt_mux::Axis::Horizontal => mt_proto::SplitAxis::Horizontal,
                    mt_mux::Axis::Vertical => mt_proto::SplitAxis::Vertical,
                },
                direction: Some(dir(d)),
                percent: None,
                cwd: None,
                command: None,
            }),
            Builtin::ClosePane => Request::PaneClose(target),
            Builtin::CloseTab => Request::TabClose(Target {
                force,
                ..Target::tab(f.tab)
            }),
            Builtin::ClearScrollback => return None,
            Builtin::Copy
            | Builtin::Paste
            | Builtin::SelectAll
            | Builtin::OpenLink
            | Builtin::CopyLink
            | Builtin::Search
            | Builtin::SaveAsSnippet
            | Builtin::ToggleRecording
            | Builtin::CloseOtherTabs
            | Builtin::RenameTab => return None,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Rows

#[derive(Debug, Clone, PartialEq)]
pub enum RowKind {
    Item,
    Submenu(Vec<MenuRow>),
    Separator,
    Header,
    /// One row of the tab-color swatch grid (5 columns); `selected` is the swatch id that
    /// shows the check mark.
    Swatches {
        items: Vec<crate::tab_color::Swatch>,
        selected: String,
    },
}

/// Swatches per grid row.
pub const SWATCH_COLUMNS: usize = 5;

/// One displayed row, keyed by item id.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuRow {
    pub id: String,
    pub label: String,
    /// Action key (`"Copy"`, `"SwapPane(left)"`) whose binding is shown as the chip.
    pub chip_action: Option<String>,
    pub icon: Option<String>,
    pub enabled: bool,
    pub kind: RowKind,
}

impl MenuRow {
    fn separator(id: String) -> Self {
        MenuRow {
            id,
            label: String::new(),
            chip_action: None,
            icon: None,
            enabled: false,
            kind: RowKind::Separator,
        }
    }
}

fn builtin_row(b: &'static BuiltinDef, f: &Facts) -> Option<MenuRow> {
    if b.id == "save_as_snippet" && !f.snippets_enabled {
        return None;
    }
    if b.id == "toggle_recording" && !f.recording_enabled {
        return None;
    }
    let holds = f.holds(b.when);
    if !holds && b.when_false == WhenFalse::Hide {
        return None;
    }
    let kind = if b.group {
        RowKind::Header
    } else if b.children.is_empty() {
        RowKind::Item
    } else {
        RowKind::Submenu(
            b.children
                .iter()
                .filter_map(|c| builtin_row(c, f))
                .collect(),
        )
    };
    Some(MenuRow {
        id: b.id.to_string(),
        label: b.label.to_string(),
        chip_action: b.chip.map(str::to_string),
        icon: None,
        enabled: holds,
        kind,
    })
}

/// Templates an item would expand, used to disable rows whose values are missing.
fn item_ready(item: &MenuItem, f: &Facts) -> bool {
    item.templates().iter().all(|(_, t)| {
        reg::template_placeholders(t)
            .iter()
            .all(|p| f.values.get(p).is_some())
    })
}

fn user_row(id: &str, item: &MenuItem, children: &[Entry<'_>], f: &Facts) -> Option<MenuRow> {
    let holds = f.holds(item.when.as_deref());
    if !holds && item.when_false.unwrap_or_default() == WhenFalse::Hide {
        return None;
    }
    let label_template = item.label.clone().unwrap_or_default();
    let (label, label_ok) = match expand::expand_label(&label_template, &f.values) {
        Ok(l) => (l, true),
        Err(_) => (label_template.clone(), false),
    };
    let kind = match item.kind() {
        Some(Kind::Separator) => return Some(MenuRow::separator(id.to_string())),
        Some(Kind::Header) => RowKind::Header,
        Some(Kind::Submenu) => RowKind::Submenu(
            children
                .iter()
                .filter_map(|c| match c {
                    Entry::User { id, item, .. } => user_row(id, item, &[], f),
                    Entry::Builtin(b) => builtin_row(b, f),
                })
                .collect(),
        ),
        _ => RowKind::Item,
    };
    Some(MenuRow {
        id: id.to_string(),
        label,
        chip_action: item
            .action
            .as_ref()
            .map(|a| crate::actions::action_key(&a.name, &a.arg)),
        icon: item.icon.clone(),
        enabled: holds && label_ok && item_ready(item, f),
        kind,
    })
}

/// The `tab/color` built-in: a "Tab Color" heading and the swatch grid, 5 per row.
fn color_rows(cfg: &Config, f: &Facts, item: MenuRow) -> Vec<MenuRow> {
    let palette = &cfg.tab_bar.palette;
    let selected = crate::tab_color::selected_swatch(palette, f.manual_color);
    let mut out = vec![MenuRow {
        kind: RowKind::Header,
        enabled: false,
        ..item.clone()
    }];
    for (i, chunk) in crate::tab_color::swatches(palette)
        .chunks(SWATCH_COLUMNS)
        .enumerate()
    {
        out.push(MenuRow {
            id: format!("color-row-{i}"),
            label: String::new(),
            chip_action: None,
            icon: None,
            enabled: item.enabled,
            kind: RowKind::Swatches {
                items: chunk.to_vec(),
                selected: selected.clone(),
            },
        });
    }
    out
}

/// Drops leading, trailing and doubled separators, and puts one before each header.
fn tidy(rows: Vec<MenuRow>) -> Vec<MenuRow> {
    let mut out: Vec<MenuRow> = Vec::new();
    for r in rows {
        let last_sep = out.last().is_none_or(|l| l.kind == RowKind::Separator);
        match r.kind {
            RowKind::Separator if last_sep => {}
            RowKind::Header if !last_sep => {
                out.push(MenuRow::separator(format!("{}-sep", r.id)));
                out.push(r);
            }
            _ => out.push(r),
        }
    }
    while out.last().is_some_and(|l| l.kind == RowKind::Separator) {
        out.pop();
    }
    out
}

/// The rows of `menu` for `facts`, with `when` applied.
pub fn resolve(cfg: &Config, menu: &str, f: &Facts) -> Vec<MenuRow> {
    let mut rows = Vec::new();
    // In `viewport`, a separator divides the built-ins from the config items (SPEC §10.9.1).
    let divide = menu == "viewport";
    let mut last_user = None;
    for e in reg::entries(cfg, menu).iter() {
        let (row, user) = match e {
            Entry::Builtin(b) => (builtin_row(b, f), false),
            Entry::User { id, item, children } => (user_row(id, item, children, f), true),
        };
        let Some(row) = row.filter(|r| !matches!(&r.kind, RowKind::Submenu(c) if c.is_empty()))
        else {
            continue;
        };
        if divide && last_user.is_some_and(|u| u != user) {
            rows.push(MenuRow::separator(USER_SEPARATOR.into()));
        }
        last_user = Some(user);
        rows.push(row);
    }
    if reg::menu(menu).is_some_and(|m| m.shell_registrable) {
        rows.extend(shell::rows(cfg, f));
    }
    if menu == "tab" {
        rows = rows
            .into_iter()
            .flat_map(|r| {
                if r.id == "color" && r.kind == RowKind::Item {
                    color_rows(cfg, f, r)
                } else {
                    vec![r]
                }
            })
            .collect();
    }
    tidy(rows)
}

/// Id of the separator between built-in and config rows in `viewport`.
pub const USER_SEPARATOR: &str = "config-sep";

// ---------------------------------------------------------------------------------------------
// Invocation

/// What running an item does, fully expanded.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Builtin(Builtin),
    Action(ActionRef),
    Send {
        text: String,
    },
    Spawn {
        argv: Vec<String>,
        cwd: Option<String>,
        title: Option<String>,
        place: SpawnWhere,
        close_on_success: bool,
        ctl: bool,
        env: Vec<(String, String)>,
    },
    Ctl {
        argv: Vec<String>,
    },
}

/// A planned invocation of `menu/item` on a pane.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub menu: String,
    pub item: String,
    pub pane: Id,
    pub kind: Option<Kind>,
    pub plan: Plan,
    /// Why a confirmation is needed, when it is.
    pub confirm: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    UnknownMenu(String),
    UnknownItem(String),
    /// Hidden or disabled by `when`, or a layout row.
    Unavailable(String),
    Expand(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::UnknownMenu(m) => write!(f, "unknown menu \"{m}\""),
            PlanError::UnknownItem(i) => write!(f, "unknown menu item \"{i}\""),
            PlanError::Unavailable(m) | PlanError::Expand(m) => f.write_str(m),
        }
    }
}

impl PlanError {
    pub fn rpc(&self) -> mt_proto::RpcError {
        match self {
            PlanError::UnknownMenu(_) | PlanError::UnknownItem(_) => {
                mt_proto::RpcError::not_found(self.to_string())
            }
            _ => mt_proto::RpcError::bad_request(self.to_string()),
        }
    }
}

/// Splits `menu/item`.
pub fn parse_address(addr: &str) -> Result<(&str, &str), PlanError> {
    match addr.split_once('/') {
        Some((m, i)) if !m.is_empty() && !i.is_empty() => Ok((m, i)),
        _ => Err(PlanError::Unavailable(format!(
            "menu item {addr:?} must be MENU/ITEM, e.g. viewport/zoom"
        ))),
    }
}

/// Resolves, checks and expands `menu/item` for `facts` (SPEC §10.9.5).
pub fn plan(cfg: &Config, menu: &str, item: &str, f: &Facts) -> Result<Invocation, PlanError> {
    if reg::menu(menu).is_none() {
        return Err(PlanError::UnknownMenu(menu.to_string()));
    }
    if reg::menu(menu).is_some_and(|m| m.shell_registrable) && shell::origin_of(item).is_some() {
        return shell::plan(cfg, menu, item, f);
    }
    let entries = reg::entries(cfg, menu);
    let entry = reg::find_entry(&entries, item)
        .ok_or_else(|| PlanError::UnknownItem(format!("{menu}/{item}")))?;
    let unavailable = || PlanError::Unavailable(format!("{menu}/{item} is not available here"));
    let inv = |kind, plan, confirm| Invocation {
        menu: menu.to_string(),
        item: item.to_string(),
        pane: f.pane,
        kind,
        plan,
        confirm,
    };
    let item_def = match entry {
        Entry::Builtin(b) => {
            if !f.holds(b.when) {
                return Err(unavailable());
            }
            let op = Builtin::from_id(menu, b.id).ok_or_else(unavailable)?;
            return Ok(inv(None, Plan::Builtin(op), None));
        }
        Entry::User { item, .. } => item,
    };
    if !f.holds(item_def.when.as_deref()) {
        return Err(unavailable());
    }
    let kind = item_def.kind().ok_or_else(unavailable)?;
    let ex = |e: ExpandError| PlanError::Expand(format!("{menu}/{item}: {e}"));
    let uses_content = item_def.templates().iter().any(|(field, t)| {
        *field != "label"
            && reg::template_placeholders(t)
                .iter()
                .any(|p| CONTENT_PLACEHOLDERS.contains(p))
    });
    let plan = match kind {
        Kind::Action => Plan::Action(item_def.action.clone().expect("kind")),
        Kind::Send => {
            let quote = item_def.quote.unwrap_or(Quote::Sh) == Quote::Sh;
            let text = expand::expand(
                item_def.send.as_deref().unwrap_or_default(),
                &f.values,
                quote,
                item_def.multiline,
            )
            .map_err(ex)?;
            Plan::Send { text }
        }
        Kind::Spawn => {
            let sp = item_def.spawn.as_ref().expect("kind");
            let opt = |s: &Option<String>| {
                s.as_deref()
                    .map(|t| expand::expand(t, &f.values, false, false))
                    .transpose()
            };
            Plan::Spawn {
                argv: expand::expand_argv(&sp.command, &f.values).map_err(ex)?,
                cwd: opt(&sp.cwd).map_err(ex)?,
                title: opt(&sp.title).map_err(ex)?,
                place: sp.place,
                close_on_success: sp.close_on_success,
                ctl: sp.ctl,
                env: expand::env_for(&f.values, menu, item),
            }
        }
        Kind::Ctl => {
            let raw = item_def.ctl.as_deref().unwrap_or_default();
            let words = mt_config::split_command(raw)
                .map_err(|e| PlanError::Expand(format!("{menu}/{item}: ctl: {e}")))?;
            Plan::Ctl {
                argv: expand::expand_argv(&words, &f.values).map_err(ex)?,
            }
        }
        Kind::Submenu | Kind::Separator | Kind::Header | Kind::Include => return Err(unavailable()),
    };
    let confirm = confirm_reason(
        cfg,
        item_def.confirm.unwrap_or_default(),
        kind,
        &plan,
        f,
        uses_content,
    );
    Ok(inv(Some(kind), plan, confirm))
}

/// The text a confirmation dialog shows and `dangerous` is matched against.
pub fn plan_text(p: &Plan) -> String {
    match p {
        Plan::Builtin(b) => format!("{b:?}"),
        Plan::Action(a) => crate::actions::action_key(&a.name, &a.arg),
        Plan::Send { text } => text.clone(),
        Plan::Spawn { argv, .. } | Plan::Ctl { argv } => argv
            .iter()
            .map(|a| {
                if a.is_empty()
                    || a.contains(|c: char| c.is_whitespace() || "'\"\\$`;&|<>(){}*?".contains(c))
                {
                    expand::sh_quote(a)
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn confirm_reason(
    cfg: &Config,
    confirm: Confirm,
    kind: Kind,
    plan: &Plan,
    f: &Facts,
    uses_content: bool,
) -> Option<String> {
    if !kind.has_side_effect() {
        return (confirm == Confirm::Always).then(|| "confirm = true".into());
    }
    let text = plan_text(plan);
    if let Some(p) = expand::dangerous(&text, &cfg.menu_security.dangerous) {
        return Some(format!("matches the dangerous pattern {p:?}"));
    }
    match confirm {
        Confirm::Always => Some("confirm = true".into()),
        Confirm::Never => None,
        Confirm::Auto => {
            if f.remote {
                return Some(format!("pane {} is remote ({})", f.pane, f.row.domain));
            }
            let sensitive = cfg.menu_security.confirm_panes.iter().find(|s| {
                mt_proto::Selector::parse(s).is_ok_and(|sel| mt_mux::ctl::matches(&sel, &f.row))
            });
            if let Some(s) = sensitive {
                return Some(format!("pane {} matches {s:?}", f.pane));
            }
            (uses_content && matches!(kind, Kind::Send | Kind::Spawn))
                .then(|| "it uses terminal content".into())
        }
    }
}

/// `--dry-run` / `menu.invoke` result body.
pub fn describe(inv: &Invocation) -> Value {
    let mut v = json!({
        "menu": inv.menu,
        "item": inv.item,
        "pane": inv.pane,
        "kind": inv.kind.map_or("builtin", Kind::name),
        "confirm": inv.confirm,
    });
    match &inv.plan {
        Plan::Builtin(b) => v["builtin"] = json!(format!("{b:?}")),
        Plan::Action(a) => v["action"] = json!(crate::actions::action_key(&a.name, &a.arg)),
        Plan::Send { text } => v["text"] = json!(text),
        Plan::Spawn {
            argv,
            cwd,
            title,
            place,
            close_on_success,
            ctl,
            ..
        } => {
            v["argv"] = json!(argv);
            v["cwd"] = json!(cwd);
            v["title"] = json!(title);
            v["where"] = json!(place.as_str());
            v["close_on_success"] = json!(close_on_success);
            v["ctl"] = json!(ctl);
        }
        Plan::Ctl { argv } => v["argv"] = json!(argv),
    }
    v
}

/// One-line audit summary (truncated to 256 characters).
pub fn summary(inv: &Invocation) -> String {
    let s = format!(
        "{} {}",
        inv.kind.map_or("builtin", Kind::name),
        plan_text(&inv.plan)
    );
    let s = expand::sanitize(&s, false);
    if s.chars().count() > 256 {
        s.chars().take(255).collect::<String>() + "…"
    } else {
        s
    }
}

// ---------------------------------------------------------------------------------------------
// IPC (`menu.show`, `menu.invoke`), shared by the GUI and headless hosts

/// What `menu.invoke` should do after the checks.
pub enum Prepared {
    /// `--dry-run`: the description is the reply.
    DryRun(Value),
    Run(Box<(Invocation, Facts)>),
}

/// Resolves the target pane (never the focused one: `invoke` is an input verb), plans the item
/// and enforces the confirmation contract (exit 8 unless `--force`).
pub fn prepare_invoke(
    cfg: &Config,
    rows: &Rows,
    p: &mt_proto::MenuInvokeParams,
    term: &dyn Fn(Id) -> Option<std::sync::Arc<Terminal>>,
) -> Result<Prepared, mt_proto::RpcError> {
    let (menu, item) =
        parse_address(&p.item).map_err(|e| mt_proto::RpcError::bad_request(e.to_string()))?;
    let pane = crate::ipc_ops::resolve_panes(
        rows,
        &p.target,
        mt_mux::ctl::Verb::Acting { all_allowed: false },
    )?[0];
    let t = term(pane);
    let facts = Facts::for_pane(cfg, rows, pane, t.as_deref())
        .ok_or_else(|| mt_proto::RpcError::not_found(format!("pane {pane} does not exist")))?;
    let inv = plan(cfg, menu, item, &facts).map_err(|e| e.rpc())?;
    if p.dry_run {
        return Ok(Prepared::DryRun(describe(&inv)));
    }
    if let Some(reason) = &inv.confirm {
        if !p.target.force {
            return Err(mt_proto::RpcError::confirm_required(format!(
                "{}/{} needs confirmation ({reason}); pass --force to run it",
                inv.menu, inv.item
            )));
        }
    }
    Ok(Prepared::Run(Box::new((inv, facts))))
}

/// `menu.show` for a pane (read-only: falls back to the focused pane).
pub fn show(
    cfg: &Config,
    rows: &Rows,
    p: &mt_proto::MenuShowParams,
    term: &dyn Fn(Id) -> Option<std::sync::Arc<Terminal>>,
) -> Result<Value, mt_proto::RpcError> {
    if reg::menu(&p.menu).is_none() {
        return Err(mt_proto::RpcError::not_found(format!(
            "unknown menu \"{}\"",
            p.menu
        )));
    }
    let pane = crate::ipc_ops::resolve_panes(rows, &p.target, mt_mux::ctl::Verb::ReadOnly)?[0];
    let t = term(pane);
    let facts = Facts::for_pane(cfg, rows, pane, t.as_deref())
        .ok_or_else(|| mt_proto::RpcError::not_found(format!("pane {pane} does not exist")))?;
    Ok(show_json(cfg, &p.menu, &facts))
}

// ---------------------------------------------------------------------------------------------
// Listing and docs

fn rows_json(rows: &[MenuRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|r| {
                let mut v = json!({
                    "id": r.id,
                    "label": r.label,
                    "enabled": r.enabled,
                    "type": match r.kind {
                        RowKind::Swatches { .. } => "swatches",
                        RowKind::Item => "item",
                        RowKind::Submenu(_) => "submenu",
                        RowKind::Separator => "separator",
                        RowKind::Header => "header",
                    },
                });
                if let RowKind::Submenu(c) = &r.kind {
                    v["items"] = rows_json(c);
                }
                if let RowKind::Swatches { items, selected } = &r.kind {
                    v["swatches"] = json!(items
                        .iter()
                        .map(
                            |s| json!({"id": s.id, "label": s.label, "selected": &s.id == selected})
                        )
                        .collect::<Vec<_>>());
                }
                if let Some(o) = shell::origin_of(&r.id) {
                    v["origin"] = json!(o);
                }
                v
            })
            .collect(),
    )
}

/// `menu.show`: the rows of `menu` for a pane.
pub fn show_json(cfg: &Config, menu: &str, f: &Facts) -> Value {
    json!({ "menu": menu, "pane": f.pane, "items": rows_json(&resolve(cfg, menu, f)) })
}

fn entry_json(e: &Entry<'_>) -> Value {
    match e {
        Entry::Builtin(b) => {
            let mut v = json!({"id": b.id, "label": b.label, "source": "builtin"});
            if let Some(w) = b.when {
                v["when"] = json!(w);
            }
            if !b.children.is_empty() && !b.group {
                v["items"] = Value::Array(e.children().iter().map(entry_json).collect());
            }
            v
        }
        Entry::User { id, item, children } => {
            let mut v = json!({
                "id": id,
                "label": item.label,
                "source": "config",
                "kind": item.kind().map(Kind::name),
            });
            if let Some(w) = &item.when {
                v["when"] = json!(w);
            }
            if !children.is_empty() {
                v["items"] = Value::Array(children.iter().map(entry_json).collect());
            }
            v
        }
    }
}

/// `menu.list`: every registered menu with its facts, placeholders, kinds and items.
pub fn list_json(cfg: &Config) -> Value {
    Value::Array(
        reg::MENUS
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "description": m.description,
                    "enabled": cfg.context_menu.get(m.id).is_none_or(|c| c.enabled),
                    "trigger": serde_json::to_value(reg::trigger(cfg, m.id)).unwrap_or_default(),
                    "facts": m.facts.iter().map(|f| f.name()).collect::<Vec<_>>(),
                    "placeholders": m.placeholders,
                    "kinds": m.allowed_kinds.iter().map(|k| k.name()).collect::<Vec<_>>(),
                    "shell_registrable": m.shell_registrable,
                    "items": reg::entries(cfg, m.id).iter().map(entry_json).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

pub const REFERENCE_DOC_PATH: &str = "site/src/content/docs/docs/reference/context-menus.md";

/// The generated "Reference → Context menus" page (`cargo run -p xtask -- gen-docs`).
pub fn reference_markdown() -> String {
    let mut s = String::from(
        "---\ntitle: Context menus\ndescription: Every registered context menu with its facts, placeholders, built-in items and allowed kinds.\n---\n\n<!-- Generated by `cargo run -p xtask -- gen-docs` from `mt_config::menu::MENUS`. Do not edit. -->\n\nHow to configure menus: [Context menus](/docs/context-menus/).\n",
    );
    fn builtins(out: &mut String, defs: &[BuiltinDef], depth: usize) {
        for b in defs {
            let indent = "  ".repeat(depth);
            let mut line = format!("{indent}- `{}`: {}", b.id, b.label);
            if b.group {
                line.push_str(" (group)");
            }
            if let Some(w) = b.when {
                let how = match b.when_false {
                    WhenFalse::Hide => "shown",
                    WhenFalse::Disable => "enabled",
                };
                line.push_str(&format!(", {how} when `{w}`"));
            }
            if let Some(c) = b.chip {
                line.push_str(&format!(", chip of `{c}`"));
            }
            out.push_str(&line);
            out.push('\n');
            builtins(out, b.children, depth + 1);
        }
    }
    for m in reg::MENUS {
        s.push_str(&format!("\n## `{}`\n\n{}\n\n", m.id, m.description));
        s.push_str(&format!(
            "- **Facts** (`when`): {}\n",
            m.facts
                .iter()
                .map(|f| format!("`{}`", f.name()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        s.push_str(&format!(
            "- **Placeholders**: {}\n",
            m.placeholders
                .iter()
                .map(|p| format!("`{{{p}}}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        s.push_str(&format!(
            "- **Allowed kinds**: {}\n",
            m.allowed_kinds
                .iter()
                .map(|k| format!("`{}`", k.name()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        s.push_str(&format!(
            "- **Shell-registrable**: {}\n\n**Built-in items**\n\n",
            if m.shell_registrable {
                "yes (M11.2)"
            } else {
                "no"
            }
        ));
        builtins(&mut s, m.builtin, 0);
    }
    s.push_str(
        "\n## Icons\n\nBundled [Lucide](https://lucide.dev) icons usable as `icon = \"…\"` \
         (`microterm ctl menu icons` prints the same list):\n\n",
    );
    s.push_str(&icon_gallery());
    s
}

/// The icons as a grid of inline SVGs with their names, drawn in the page's text colour.
fn icon_gallery() -> String {
    use gpui::AssetSource as _;
    let mut out = String::from(
        "<div style=\"display:grid;grid-template-columns:repeat(auto-fill,minmax(9rem,1fr));gap:0.75rem;margin-top:1rem\">\n",
    );
    for name in reg::ICON_NAMES {
        let svg = crate::assets::IconName::from_name(name)
            .and_then(|i| crate::assets::Assets.load(i.path()).ok().flatten())
            .map(|b| inline_svg(&String::from_utf8_lossy(&b)))
            .unwrap_or_default();
        out.push_str(&format!(
            "<div style=\"display:flex;flex-direction:column;align-items:center;gap:0.4rem;padding:0.75rem 0.25rem;border:1px solid var(--sl-color-gray-5,#8884);border-radius:6px\">{svg}<code style=\"font-size:0.75rem;text-align:center;overflow-wrap:anywhere\">{name}</code></div>\n"
        ));
    }
    out.push_str("</div>\n");
    out
}

/// One line of SVG markup: the licence comment, the class and the line breaks removed.
fn inline_svg(src: &str) -> String {
    let body = match src.find("<svg") {
        Some(i) => &src[i..],
        None => src,
    };
    let one_line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line
        .split(' ')
        .filter(|w| !w.starts_with("class=") && !w.starts_with("lucide-"))
        .collect::<Vec<_>>()
        .join(" ")
        .replace(
            "width=\"24\" height=\"24\"",
            "width=\"24\" height=\"24\" aria-hidden=\"true\"",
        )
}

#[cfg(test)]
mod tests;
