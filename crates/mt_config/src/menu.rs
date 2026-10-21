//! Context menus (SPEC §10.9): the `[context_menu.<id>]`, `[menu_groups.<name>]` and
//! `[menu_security]` schema, the GPUI-free menu registry (`MENUS`) shared by config validation,
//! the UI, `ctl menu` and the docs, template parsing, item resolution and per-menu validation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use mt_proto::when::{When, WhenKey};
use serde::{Deserialize, Serialize};

use crate::schema::{ActionRef, Config, RightClick};

// ---------------------------------------------------------------------------------------------
// Schema

/// `[context_menu.<id>]`: settings and items of one registered menu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuConfig {
    pub enabled: bool,
    pub mode: MenuMode,
    /// Top-level built-ins to keep, in this order. `None` keeps them all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin: Option<Vec<String>>,
    pub max_rows: u32,
    /// `None` = not set (the viewport then honours `mouse.right_click`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    pub items: Vec<MenuItem>,
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: MenuMode::Append,
            builtin: None,
            max_rows: 20,
            trigger: None,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MenuMode {
    #[default]
    Append,
    Prepend,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    RightClick,
    ShiftRightClick,
    /// Viewport only: right-click pastes.
    Paste,
    None,
}

/// `[[menu_groups.<name>.items]]`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuGroup {
    pub items: Vec<MenuItem>,
}

/// `[menu_security]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuSecurity {
    /// Substrings that force a confirmation when found in an expanded `send`/`spawn`/`ctl`.
    pub dangerous: Vec<String>,
    /// Selectors (§12.3) of panes for which `confirm = "auto"` always confirms.
    pub confirm_panes: Vec<String>,
    /// Directories whose panes may run shell-registered `spawn` items and project menus
    /// (SPEC §10.9.7). Absolute; `"/dir"` is exact, `"/dir/**"` is the subtree.
    pub trusted_dirs: Vec<String>,
    /// Honour items registered from ssh-domain panes (never `spawn`).
    pub allow_remote_items: bool,
}

impl Default for MenuSecurity {
    fn default() -> Self {
        Self {
            dangerous: ["rm -rf", "DROP ", "kubectl delete", "git push --force"]
                .map(String::from)
                .to_vec(),
            confirm_panes: ["title:*prod*", "domain:*prod*"].map(String::from).to_vec(),
            trusted_dirs: Vec::new(),
            allow_remote_items: false,
        }
    }
}

/// One `[[context_menu.<id>.items]]` entry as written. Which kind it is comes from which kind
/// key is present ([`MenuItem::kind`]); validation insists on exactly one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_false: Option<WhenFalse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Confirm>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<ActionRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<Quote>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub multiline: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn: Option<SpawnItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submenu: Option<Vec<MenuItem>>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhenFalse {
    #[default]
    Hide,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quote {
    Sh,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutKind {
    Separator,
    Header,
}

/// `confirm = false | true | "auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Confirm {
    #[default]
    Never,
    Always,
    Auto,
}

impl<'de> Deserialize<'de> for Confirm {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Bool(false) => Ok(Confirm::Never),
            Raw::Bool(true) => Ok(Confirm::Always),
            Raw::Str(s) if s == "auto" => Ok(Confirm::Auto),
            Raw::Str(s) => Err(serde::de::Error::custom(format!(
                "confirm must be true, false or \"auto\", not {s:?}"
            ))),
        }
    }
}

impl Serialize for Confirm {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Confirm::Never => s.serialize_bool(false),
            Confirm::Always => s.serialize_bool(true),
            Confirm::Auto => s.serialize_str("auto"),
        }
    }
}

/// `spawn = { command = [...], cwd, title, where, close_on_success, ctl }`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawnItem {
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "where")]
    pub place: SpawnWhere,
    pub close_on_success: bool,
    /// Give the process the ctl socket and `MICROTERM_*` ids.
    pub ctl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpawnWhere {
    #[default]
    Tab,
    SplitLeft,
    SplitRight,
    SplitUp,
    SplitDown,
    Window,
}

impl SpawnWhere {
    pub fn as_str(self) -> &'static str {
        match self {
            SpawnWhere::Tab => "tab",
            SpawnWhere::SplitLeft => "split-left",
            SpawnWhere::SplitRight => "split-right",
            SpawnWhere::SplitUp => "split-up",
            SpawnWhere::SplitDown => "split-down",
            SpawnWhere::Window => "window",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Registry

/// The closed set of item kinds (SPEC §10.9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Action,
    Send,
    Spawn,
    Ctl,
    Submenu,
    Separator,
    Header,
    Include,
}

impl Kind {
    pub const ALL: &'static [Kind] = &[
        Kind::Action,
        Kind::Send,
        Kind::Spawn,
        Kind::Ctl,
        Kind::Submenu,
        Kind::Separator,
        Kind::Header,
        Kind::Include,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Action => "action",
            Kind::Send => "send",
            Kind::Spawn => "spawn",
            Kind::Ctl => "ctl",
            Kind::Submenu => "submenu",
            Kind::Separator => "separator",
            Kind::Header => "header",
            Kind::Include => "include",
        }
    }

    /// Kinds with a side effect outside microterm's own chrome: audited, and `confirm` applies.
    pub fn has_side_effect(self) -> bool {
        matches!(self, Kind::Send | Kind::Spawn | Kind::Ctl)
    }
}

/// A built-in row. Its behaviour lives in `mt_ui::menu`; this is the data config and docs need.
#[derive(Debug)]
pub struct BuiltinDef {
    pub id: &'static str,
    pub label: &'static str,
    /// The action whose key binding is shown as the chip.
    pub chip: Option<&'static str>,
    /// Shown (or enabled, per `when_false`) only when this holds.
    pub when: Option<&'static str>,
    pub when_false: WhenFalse,
    pub children: &'static [BuiltinDef],
    /// A group: rendered as a header with its children inlined after it (not a submenu).
    pub group: bool,
}

const fn b(id: &'static str, label: &'static str, chip: Option<&'static str>) -> BuiltinDef {
    BuiltinDef {
        id,
        label,
        chip,
        when: None,
        when_false: WhenFalse::Hide,
        children: &[],
        group: false,
    }
}

const fn b_if(
    id: &'static str,
    label: &'static str,
    chip: Option<&'static str>,
    when: &'static str,
    when_false: WhenFalse,
) -> BuiltinDef {
    BuiltinDef {
        id,
        label,
        chip,
        when: Some(when),
        when_false,
        children: &[],
        group: false,
    }
}

/// One registered menu (SPEC §10.9.1).
#[derive(Debug)]
pub struct MenuDef {
    pub id: &'static str,
    pub description: &'static str,
    pub facts: &'static [WhenKey],
    pub placeholders: &'static [&'static str],
    pub builtin: &'static [BuiltinDef],
    pub allowed_kinds: &'static [Kind],
    pub shell_registrable: bool,
}

/// Every placeholder, in documentation order.
pub const PLACEHOLDERS: &[&str] = &[
    "selection",
    "word",
    "line",
    "link",
    "path",
    "path:line",
    "cwd",
    "pane",
    "tab",
    "window",
    "title",
    "domain",
    "host",
    "last_command",
    "last_output",
    "exit_code",
];

/// Placeholders whose value is terminal content, i.e. attacker-influenced (SPEC §10.9.5).
pub const CONTENT_PLACEHOLDERS: &[&str] = &[
    "selection",
    "word",
    "line",
    "link",
    "path",
    "path:line",
    "title",
    "last_command",
    "last_output",
];

const TAB_PLACEHOLDERS: &[&str] = &[
    "pane",
    "tab",
    "window",
    "title",
    "cwd",
    "domain",
    "host",
    "last_command",
    "last_output",
    "exit_code",
];

const TAB_FACTS: &[WhenKey] = &[
    WhenKey::Title,
    WhenKey::Cwd,
    WhenKey::Cmd,
    WhenKey::Domain,
    WhenKey::Status,
    WhenKey::Exited,
    WhenKey::Focused,
    WhenKey::Panes,
    WhenKey::Tabs,
    WhenKey::Zoomed,
    WhenKey::Remote,
    WhenKey::Local,
    WhenKey::LastExit,
    WhenKey::Idle,
    WhenKey::Running,
];

const SWAP: &[BuiltinDef] = &[
    b("swap_left", "Swap left", Some("SwapPane(left)")),
    b("swap_right", "Swap right", Some("SwapPane(right)")),
    b("swap_up", "Swap up", Some("SwapPane(up)")),
    b("swap_down", "Swap down", Some("SwapPane(down)")),
];

const PANES: &[BuiltinDef] = &[
    b("zoom", "Toggle zoom", Some("TogglePaneZoom")),
    BuiltinDef {
        id: "swap",
        label: "Swap",
        chip: None,
        when: Some("panes>1"),
        when_false: WhenFalse::Hide,
        children: SWAP,
        group: false,
    },
    b_if(
        "rotate",
        "Rotate",
        Some("RotatePanes"),
        "panes>1",
        WhenFalse::Hide,
    ),
    b("split_left", "Split left", Some("Split(left)")),
    b("split_right", "Split right", Some("SplitHorizontal")),
    b("split_up", "Split up", Some("Split(up)")),
    b("split_down", "Split down", Some("SplitVertical")),
    b("close_pane", "Close pane", Some("ClosePane")),
];

const VIEWPORT_BUILTINS: &[BuiltinDef] = &[
    b_if(
        "copy",
        "Copy",
        Some("Copy"),
        "selection",
        WhenFalse::Disable,
    ),
    b("paste", "Paste", Some("Paste")),
    b("select_all", "Select all", Some("SelectAll")),
    b_if("open_link", "Open link", None, "link", WhenFalse::Disable),
    b_if("copy_link", "Copy link", None, "link", WhenFalse::Disable),
    b_if(
        "search",
        "Search for selection",
        None,
        "selection",
        WhenFalse::Disable,
    ),
    b(
        "clear_scrollback",
        "Clear scrollback",
        Some("ClearScrollback"),
    ),
    BuiltinDef {
        id: "panes",
        label: "Panes",
        chip: None,
        when: None,
        when_false: WhenFalse::Hide,
        children: PANES,
        group: true,
    },
];

const TAB_BUILTINS: &[BuiltinDef] = &[
    b("close", "Close tab", Some("CloseTab")),
    b("close_others", "Close other tabs", None),
    b("rename", "Rename…", None),
];

pub static MENUS: &[MenuDef] = &[
    MenuDef {
        id: "viewport",
        description: "Right-click in a pane's terminal grid. Acts on the pane under the pointer.",
        facts: WhenKey::ALL,
        placeholders: PLACEHOLDERS,
        builtin: VIEWPORT_BUILTINS,
        allowed_kinds: Kind::ALL,
        shell_registrable: true,
    },
    MenuDef {
        id: "tab",
        description: "Right-click on a tab header. Pane facts are those of the tab's active pane.",
        facts: TAB_FACTS,
        placeholders: TAB_PLACEHOLDERS,
        builtin: TAB_BUILTINS,
        allowed_kinds: Kind::ALL,
        shell_registrable: false,
    },
];

pub fn menu(id: &str) -> Option<&'static MenuDef> {
    MENUS.iter().find(|m| m.id == id)
}

/// Bundled Lucide icons usable as `icon = "…"` (the files in `assets/icons`).
pub const ICON_NAMES: &[&str] = &[
    "arrow-down",
    "arrow-up",
    "book-open",
    "bug",
    "case-sensitive",
    "check",
    "chevron-down",
    "chevron-right",
    "circle-x",
    "clipboard",
    "code",
    "columns-2",
    "copy",
    "eye",
    "file",
    "file-text",
    "folder",
    "folder-open",
    "git-branch",
    "git-commit-horizontal",
    "git-pull-request",
    "globe",
    "info",
    "layers",
    "link",
    "maximize-2",
    "minus",
    "palette",
    "pencil",
    "play",
    "plus",
    "refresh-cw",
    "regex",
    "rotate-cw",
    "rows-2",
    "search",
    "server",
    "square",
    "terminal",
    "trash-2",
    "triangle-alert",
    "wifi-off",
    "x",
];

fn closest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|c| (c, strsim::damerau_levenshtein(word, c)))
        .filter(|(c, d)| *d <= 2 && *d < word.len().max(c.len()))
        .min_by_key(|(_, d)| *d)
        .map(|(c, _)| c)
}

fn did_you_mean<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    closest(word, candidates).map_or(String::new(), |c| format!(" (did you mean \"{c}\"?)"))
}

// ---------------------------------------------------------------------------------------------
// Templates

/// A piece of a parsed template: literal text or a placeholder name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(std::borrow::Cow<'a, str>),
    Placeholder(&'a str),
}

/// Splits a template into text and `{placeholder}` pieces. `{{` and `}}` are literal braces;
/// braces that do not enclose a placeholder-shaped name (`[a-z_]+` or `path:line`) are literal,
/// so shell text such as `awk '{print $1}'` needs no escaping.
pub fn parse_template(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("{{") {
            text.push('{');
            rest = r;
            continue;
        }
        if let Some(r) = rest.strip_prefix("}}") {
            text.push('}');
            rest = r;
            continue;
        }
        if let Some(r) = rest.strip_prefix('{') {
            if let Some(end) = r.find('}') {
                let name = &r[..end];
                let shaped = !name.is_empty()
                    && name.split(':').count() <= 2
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_' || c == ':')
                    && !name.starts_with(':')
                    && !name.ends_with(':');
                if shaped {
                    if !text.is_empty() {
                        out.push(Piece::Text(std::mem::take(&mut text).into()));
                    }
                    out.push(Piece::Placeholder(name));
                    rest = &r[end + 1..];
                    continue;
                }
            }
        }
        let c = rest.chars().next().expect("non-empty");
        text.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if !text.is_empty() {
        out.push(Piece::Text(text.into()));
    }
    out
}

/// The placeholder names a template uses.
pub fn template_placeholders(s: &str) -> Vec<&str> {
    parse_template(s)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Placeholder(n) => Some(n),
            Piece::Text(_) => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Items

impl MenuItem {
    /// The kinds this item declares; a valid item declares exactly one.
    pub fn kinds(&self) -> Vec<Kind> {
        let mut k = Vec::new();
        if self.action.is_some() {
            k.push(Kind::Action);
        }
        if self.send.is_some() {
            k.push(Kind::Send);
        }
        if self.spawn.is_some() {
            k.push(Kind::Spawn);
        }
        if self.ctl.is_some() {
            k.push(Kind::Ctl);
        }
        if self.submenu.is_some() {
            k.push(Kind::Submenu);
        }
        match self.layout {
            Some(LayoutKind::Separator) => k.push(Kind::Separator),
            Some(LayoutKind::Header) => k.push(Kind::Header),
            None => {}
        }
        if self.include.is_some() {
            k.push(Kind::Include);
        }
        k
    }

    /// The item's kind when exactly one is declared.
    pub fn kind(&self) -> Option<Kind> {
        match self.kinds().as_slice() {
            [k] => Some(*k),
            _ => None,
        }
    }

    /// The explicit id, or a slug of the label (`"Toggle pane zoom"` → `toggle_pane_zoom`).
    pub fn effective_id(&self, index: usize) -> String {
        if let Some(id) = &self.id {
            return id.clone();
        }
        let slug = slug(self.label.as_deref().unwrap_or_default());
        if slug.is_empty() {
            format!("item_{index}")
        } else {
            slug
        }
    }

    /// Every template string in the item, with the field it came from.
    pub fn templates(&self) -> Vec<(&'static str, &str)> {
        let mut t = Vec::new();
        if let Some(l) = &self.label {
            t.push(("label", l.as_str()));
        }
        if let Some(s) = &self.send {
            t.push(("send", s.as_str()));
        }
        if let Some(c) = &self.ctl {
            t.push(("ctl", c.as_str()));
        }
        if let Some(sp) = &self.spawn {
            for c in &sp.command {
                t.push(("spawn", c.as_str()));
            }
            if let Some(c) = &sp.cwd {
                t.push(("spawn", c.as_str()));
            }
            if let Some(c) = &sp.title {
                t.push(("spawn", c.as_str()));
            }
        }
        t
    }
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A resolved row of a menu, before `when` is evaluated.
#[derive(Debug, Clone)]
pub enum Entry<'a> {
    Builtin(&'static BuiltinDef),
    User {
        id: String,
        item: &'a MenuItem,
        children: Vec<Entry<'a>>,
    },
}

impl Entry<'_> {
    pub fn id(&self) -> &str {
        match self {
            Entry::Builtin(b) => b.id,
            Entry::User { id, .. } => id,
        }
    }

    /// Submenu rows (a group's rows are inlined by [`entries`], so a group has none here).
    pub fn children(&self) -> Vec<Entry<'_>> {
        match self {
            Entry::Builtin(b) if b.group => Vec::new(),
            Entry::Builtin(b) => b.children.iter().map(Entry::Builtin).collect(),
            Entry::User { children, .. } => children.clone(),
        }
    }
}

fn user_entries<'a>(
    items: &'a [MenuItem],
    groups: &'a BTreeMap<String, MenuGroup>,
) -> Vec<Entry<'a>> {
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(g) = &item.include {
            if let Some(group) = groups.get(g) {
                for (j, gi) in group.items.iter().enumerate() {
                    if gi.include.is_none() {
                        out.push(user_entry(gi, j));
                    }
                }
            }
            continue;
        }
        out.push(user_entry(item, i));
    }
    out
}

fn user_entry(item: &MenuItem, index: usize) -> Entry<'_> {
    let children = item
        .submenu
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(j, c)| Entry::User {
            id: c.effective_id(j),
            item: c,
            children: Vec::new(),
        })
        .collect();
    Entry::User {
        id: item.effective_id(index),
        item,
        children,
    }
}

/// The rows of menu `id` in order, honouring `mode`, `builtin` and `include` (not `when`).
/// Empty when the menu is unknown or disabled.
pub fn entries<'a>(cfg: &'a Config, id: &str) -> Vec<Entry<'a>> {
    let Some(def) = menu(id) else {
        return Vec::new();
    };
    static DEFAULT: OnceLock<MenuConfig> = OnceLock::new();
    let mc = cfg
        .context_menu
        .get(id)
        .unwrap_or_else(|| DEFAULT.get_or_init(MenuConfig::default));
    if !mc.enabled {
        return Vec::new();
    }
    let picked: Vec<&'static BuiltinDef> = match (&mc.mode, &mc.builtin) {
        (MenuMode::Replace, _) => Vec::new(),
        (_, None) => def.builtin.iter().collect(),
        (_, Some(keep)) => keep
            .iter()
            .filter_map(|k| def.builtin.iter().find(|b| b.id == k))
            .collect(),
    };
    let mut builtins = Vec::new();
    for b in picked {
        builtins.push(Entry::Builtin(b));
        if b.group {
            builtins.extend(b.children.iter().map(Entry::Builtin));
        }
    }
    let user = user_entries(&mc.items, &cfg.menu_groups);
    match mc.mode {
        MenuMode::Prepend => user.into_iter().chain(builtins).collect(),
        _ => builtins.into_iter().chain(user).collect(),
    }
}

/// Finds `item` anywhere in `entries` (submenu children included).
pub fn find_entry<'a>(entries: &[Entry<'a>], item: &str) -> Option<Entry<'a>> {
    for e in entries {
        if e.id() == item {
            return Some(e.clone());
        }
        let found = match e {
            Entry::Builtin(b) => b.children.iter().find(|c| c.id == item).map(Entry::Builtin),
            Entry::User { children, .. } => children.iter().find(|c| c.id() == item).cloned(),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The effective viewport trigger: `context_menu.viewport.trigger`, else `mouse.right_click`.
pub fn viewport_trigger(cfg: &Config) -> Trigger {
    if let Some(t) = cfg.context_menu.get("viewport").and_then(|m| m.trigger) {
        return t;
    }
    match cfg.mouse.right_click {
        RightClick::ContextMenu => Trigger::RightClick,
        RightClick::Paste => Trigger::Paste,
        RightClick::None => Trigger::None,
    }
}

/// The trigger of a non-viewport menu.
pub fn trigger(cfg: &Config, id: &str) -> Trigger {
    if id == "viewport" {
        return viewport_trigger(cfg);
    }
    cfg.context_menu
        .get(id)
        .and_then(|m| m.trigger)
        .unwrap_or(Trigger::RightClick)
}

// ---------------------------------------------------------------------------------------------
// Validator hooks

/// Checks that need crates `mt_config` cannot depend on (the action catalogue lives in `mt_ui`,
/// the ctl grammar in `microterm`). The binary installs them at start-up; without them those
/// checks are skipped (unit tests of this crate).
#[derive(Clone, Copy)]
pub struct Validators {
    pub action: fn(&ActionRef) -> Result<(), String>,
    /// Receives the raw `ctl = "…"` string.
    pub ctl: fn(&str) -> Result<(), String>,
}

static VALIDATORS: OnceLock<Validators> = OnceLock::new();

pub fn install_validators(v: Validators) {
    let _ = VALIDATORS.set(v);
}

fn validators() -> Option<&'static Validators> {
    VALIDATORS.get()
}

// ---------------------------------------------------------------------------------------------
// Validation

/// Where an issue is: a menu or group, an item (and submenu child) in it, and a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    /// `["context_menu", "viewport"]`, `["menu_groups", "layout"]` or `["mouse", "right_click"]`.
    pub table: Vec<String>,
    pub item: Option<usize>,
    pub sub: Option<usize>,
    pub field: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuIssue {
    pub loc: Loc,
    pub message: String,
    pub warning: bool,
}

/// Hard cap on rows per menu after `include` expansion (SPEC §10.9.2).
pub const MAX_ITEMS: usize = 64;

struct Checker<'c> {
    cfg: &'c Config,
    out: Vec<MenuIssue>,
}

impl Checker<'_> {
    fn push(&mut self, loc: Loc, message: String) {
        self.out.push(MenuIssue {
            loc,
            message,
            warning: false,
        });
    }

    /// Checks one item against `menu`. `prefix` names it in messages; `loc` locates it.
    fn item(&mut self, def: &MenuDef, item: &MenuItem, prefix: &str, loc: &Loc, nested: bool) {
        let at = |field: Option<&'static str>| Loc {
            field,
            ..loc.clone()
        };
        let kinds = item.kinds();
        let kind = match kinds.as_slice() {
            [k] => *k,
            [] => {
                return self.push(
                    at(None),
                    format!(
                        "{prefix}: needs one of action, send, spawn, ctl, submenu, type or include"
                    ),
                )
            }
            many => {
                let names: Vec<_> = many.iter().map(|k| k.name()).collect();
                return self.push(
                    at(Some(field_of(many[1]))),
                    format!("{prefix}: has more than one kind ({})", names.join(", ")),
                );
            }
        };
        if !def.allowed_kinds.contains(&kind) {
            self.push(
                at(Some(field_of(kind))),
                format!(
                    "{prefix}: kind {} is not allowed in menu \"{}\"",
                    kind.name(),
                    def.id
                ),
            );
        }
        if kind == Kind::Include {
            if nested {
                self.push(
                    at(Some("include")),
                    format!("{prefix}: include is not allowed inside a submenu"),
                );
            }
            if item.label.is_some() || item.when.is_some() || item.id.is_some() {
                self.push(
                    at(Some("include")),
                    format!("{prefix}: an include item takes no other keys"),
                );
            }
            return;
        }
        if kind == Kind::Submenu && nested {
            return self.push(
                at(Some("submenu")),
                format!("{prefix}: submenus nest one level only"),
            );
        }
        let needs_label = !matches!(kind, Kind::Separator);
        if needs_label && item.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
            self.push(at(None), format!("{prefix}: needs a label"));
        }
        if let Some(id) = &item.id {
            if !valid_id(id) {
                self.push(
                    at(Some("id")),
                    format!("{prefix}: id {id:?} must be lowercase letters, digits, '_' or '-'"),
                );
            }
        }
        if let Some(icon) = &item.icon {
            if !ICON_NAMES.contains(&icon.as_str()) {
                self.push(
                    at(Some("icon")),
                    format!(
                        "{prefix}: unknown icon \"{icon}\"{}; run `microterm ctl menu icons` for the list",
                        did_you_mean(icon, ICON_NAMES.iter().copied())
                    ),
                );
            }
        }
        if let Some(w) = &item.when {
            match When::parse(w) {
                Err(e) => self.push(at(Some("when")), format!("{prefix}: when: {e}")),
                Ok(w) => {
                    for k in w.keys() {
                        if !def.facts.contains(&k) {
                            let avail: Vec<_> = def.facts.iter().map(|f| f.name()).collect();
                            self.push(
                                at(Some("when")),
                                format!(
                                    "{prefix}: when key \"{}\" is not available in menu \"{}\"; available: {}",
                                    k.name(),
                                    def.id,
                                    avail.join(" ")
                                ),
                            );
                        }
                    }
                }
            }
        }
        for (field, t) in item.templates() {
            for name in template_placeholders(t) {
                let known = PLACEHOLDERS.contains(&name);
                if !known || !def.placeholders.contains(&name) {
                    let avail: Vec<_> = def
                        .placeholders
                        .iter()
                        .map(|p| format!("{{{p}}}"))
                        .collect();
                    let msg = if known {
                        format!(
                            "{prefix}: placeholder {{{name}}} is not available in menu \"{}\"; available: {}",
                            def.id,
                            avail.join(" ")
                        )
                    } else {
                        format!(
                            "{prefix}: unknown placeholder {{{name}}}{}",
                            did_you_mean(name, PLACEHOLDERS.iter().copied())
                        )
                    };
                    self.push(at(Some(field)), msg);
                }
            }
        }
        match kind {
            Kind::Action => {
                if let (Some(a), Some(v)) = (&item.action, validators()) {
                    if let Err(e) = (v.action)(a) {
                        self.push(at(Some("action")), format!("{prefix}: {e}"));
                    }
                }
            }
            Kind::Ctl => {
                let c = item.ctl.as_deref().unwrap_or_default();
                if c.trim().is_empty() {
                    self.push(at(Some("ctl")), format!("{prefix}: ctl must not be empty"));
                } else if let Some(v) = validators() {
                    if let Err(e) = (v.ctl)(c) {
                        self.push(at(Some("ctl")), format!("{prefix}: ctl {c:?}: {e}"));
                    }
                }
            }
            Kind::Spawn => {
                let sp = item.spawn.as_ref().expect("kind is spawn");
                if sp.command.is_empty() || sp.command[0].trim().is_empty() {
                    self.push(
                        at(Some("spawn")),
                        format!("{prefix}: spawn.command must be a non-empty array"),
                    );
                }
            }
            Kind::Submenu => {
                let children = item.submenu.as_deref().unwrap_or_default();
                if children.is_empty() {
                    self.push(at(Some("submenu")), format!("{prefix}: submenu is empty"));
                }
                if !nested {
                    for (j, c) in children.iter().enumerate() {
                        let loc = Loc {
                            sub: Some(j),
                            ..loc.clone()
                        };
                        self.item(def, c, &format!("{prefix}.submenu[{j}]"), &loc, true);
                    }
                }
            }
            _ => {}
        }
        if (item.quote.is_some() || item.multiline) && kind != Kind::Send {
            self.push(
                at(None),
                format!("{prefix}: quote and multiline apply to send items only"),
            );
        }
    }
}

fn field_of(k: Kind) -> &'static str {
    match k {
        Kind::Action => "action",
        Kind::Send => "send",
        Kind::Spawn => "spawn",
        Kind::Ctl => "ctl",
        Kind::Submenu => "submenu",
        Kind::Separator | Kind::Header => "type",
        Kind::Include => "include",
    }
}

/// Every problem with `[context_menu]`, `[menu_groups]` and `[menu_security]` (SPEC §10.9.2).
pub fn validate(cfg: &Config) -> Vec<MenuIssue> {
    let mut c = Checker {
        cfg,
        out: Vec::new(),
    };
    let menu_ids = || MENUS.iter().map(|m| m.id);
    for (id, mc) in &cfg.context_menu {
        let table = vec!["context_menu".to_string(), id.clone()];
        let loc = |item: Option<usize>, field| Loc {
            table: table.clone(),
            item,
            sub: None,
            field,
        };
        let Some(def) = menu(id) else {
            c.push(
                loc(None, None),
                format!(
                    "context_menu.{id}: unknown menu \"{id}\"{}; menus: {}",
                    did_you_mean(id, menu_ids()),
                    menu_ids().collect::<Vec<_>>().join(", ")
                ),
            );
            continue;
        };
        if !(4..=64).contains(&mc.max_rows) {
            c.push(
                loc(None, Some("max_rows")),
                format!("context_menu.{id}.max_rows must be between 4 and 64"),
            );
        }
        if mc.trigger == Some(Trigger::Paste) && id != "viewport" {
            c.push(
                loc(None, Some("trigger")),
                format!(
                    "context_menu.{id}.trigger = \"paste\" is only valid for the viewport menu"
                ),
            );
        }
        if let Some(keep) = &mc.builtin {
            let names = def.builtin.iter().map(|b| b.id);
            for k in keep {
                if !def.builtin.iter().any(|b| b.id == k) {
                    c.push(
                        loc(None, Some("builtin")),
                        format!(
                            "context_menu.{id}.builtin: unknown built-in \"{k}\"{}; built-ins: {}",
                            did_you_mean(k, names.clone()),
                            names.clone().collect::<Vec<_>>().join(", ")
                        ),
                    );
                }
            }
        }
        for (i, item) in mc.items.iter().enumerate() {
            let prefix = format!("context_menu.{id}.items[{i}]");
            c.item(def, item, &prefix, &loc(Some(i), None), false);
            if let Some(g) = &item.include {
                match cfg.menu_groups.get(g) {
                    None => c.push(
                        loc(Some(i), Some("include")),
                        format!(
                            "{prefix}: unknown menu group \"{g}\"{}",
                            did_you_mean(g, cfg.menu_groups.keys().map(String::as_str))
                        ),
                    ),
                    Some(group) => {
                        // Group items are checked against the including menu's facts.
                        for (j, gi) in group.items.iter().enumerate() {
                            let gloc = Loc {
                                table: vec!["menu_groups".into(), g.clone()],
                                item: Some(j),
                                sub: None,
                                field: None,
                            };
                            if gi.include.is_some() {
                                c.push(
                                    Loc {
                                        field: Some("include"),
                                        ..gloc
                                    },
                                    format!(
                                        "menu_groups.{g}.items[{j}]: groups cannot include groups"
                                    ),
                                );
                                continue;
                            }
                            c.item(
                                def,
                                gi,
                                &format!(
                                    "menu_groups.{g}.items[{j}] (included in context_menu.{id})"
                                ),
                                &gloc,
                                false,
                            );
                        }
                    }
                }
            }
        }
        // Ids are unique per menu, built-ins included; rows are capped.
        let all = entries(cfg, id);
        let mut seen = BTreeSet::new();
        let mut rows = 0;
        let mut ids = |e: &Entry, c: &mut Checker| {
            if !seen.insert(e.id().to_string()) {
                if let Entry::User { .. } = e {
                    let i = user_index(mc, e.id());
                    c.push(
                        loc(i, Some("id")),
                        format!(
                            "context_menu.{id}: duplicate item id \"{}\" (set `id` to tell items apart)",
                            e.id()
                        ),
                    );
                }
            }
        };
        if mc.enabled {
            for e in &all {
                rows += 1;
                ids(e, &mut c);
                for ch in e.children() {
                    rows += 1;
                    ids(&ch, &mut c);
                }
            }
        }
        if rows > MAX_ITEMS {
            c.push(
                loc(None, None),
                format!("context_menu.{id}: {rows} items after include; the limit is {MAX_ITEMS}"),
            );
        }
    }
    // Groups are checked where they are included; unused groups still get a structural check.
    for (g, group) in &cfg.menu_groups {
        let used = cfg
            .context_menu
            .values()
            .any(|m| m.items.iter().any(|i| i.include.as_deref() == Some(g)));
        if used {
            continue;
        }
        let def = &MENUS[0];
        for (j, gi) in group.items.iter().enumerate() {
            let gloc = Loc {
                table: vec!["menu_groups".into(), g.clone()],
                item: Some(j),
                sub: None,
                field: None,
            };
            c.item(
                def,
                gi,
                &format!("menu_groups.{g}.items[{j}]"),
                &gloc,
                false,
            );
        }
    }
    for (i, sel) in cfg.menu_security.confirm_panes.iter().enumerate() {
        if let Err(e) =
            mt_proto::Selector::parse(sel).and_then(|s| s.check(mt_proto::Resource::Pane))
        {
            c.push(
                Loc {
                    table: vec!["menu_security".into()],
                    item: None,
                    sub: None,
                    field: Some("confirm_panes"),
                },
                format!("menu_security.confirm_panes[{i}]: {}", e.message),
            );
        }
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    for (i, raw) in cfg.menu_security.trusted_dirs.iter().enumerate() {
        if let Err(e) = crate::trust::parse_entry(raw, home.as_deref()) {
            c.push(
                Loc {
                    table: vec!["menu_security".into()],
                    item: None,
                    sub: None,
                    field: Some("trusted_dirs"),
                },
                format!("menu_security.trusted_dirs[{i}]: {e}"),
            );
        }
    }
    if cfg
        .context_menu
        .get("viewport")
        .and_then(|m| m.trigger)
        .is_some()
        && cfg.mouse.right_click != RightClick::ContextMenu
    {
        c.out.push(MenuIssue {
            loc: Loc {
                table: vec!["mouse".into()],
                item: None,
                sub: None,
                field: Some("right_click"),
            },
            message: "mouse.right_click is ignored because context_menu.viewport.trigger is set"
                .into(),
            warning: true,
        });
    }
    let _ = c.cfg;
    c.out
}

fn user_index(mc: &MenuConfig, id: &str) -> Option<usize> {
    mc.items
        .iter()
        .enumerate()
        .rev()
        .find(|(i, it)| it.effective_id(*i) == id)
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_split_placeholders_and_keep_shell_braces() {
        let p = parse_template("git log -S {selection} {{x}} awk '{print $1}' {path:line}");
        let names: Vec<_> = p
            .iter()
            .filter_map(|p| match p {
                Piece::Placeholder(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["selection", "path:line"]);
        let text: String = p
            .iter()
            .map(|p| match p {
                Piece::Text(t) => t.to_string(),
                Piece::Placeholder(n) => format!("<{n}>"),
            })
            .collect();
        assert_eq!(
            text,
            "git log -S <selection> {x} awk '{print $1}' <path:line>"
        );
    }

    #[test]
    fn ids_default_to_label_slugs() {
        let item = MenuItem {
            label: Some("Toggle pane zoom…".into()),
            ..Default::default()
        };
        assert_eq!(item.effective_id(3), "toggle_pane_zoom");
        let sep = MenuItem::default();
        assert_eq!(sep.effective_id(3), "item_3");
    }

    #[test]
    fn registry_ids_are_unique_and_kebab_case() {
        let mut seen = BTreeSet::new();
        for m in MENUS {
            assert!(seen.insert(m.id));
            assert!(m.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
            let mut ids = BTreeSet::new();
            for b in m.builtin {
                assert!(ids.insert(b.id), "{}", b.id);
                for c in b.children {
                    assert!(ids.insert(c.id), "{}", c.id);
                    for g in c.children {
                        assert!(ids.insert(g.id), "{}", g.id);
                    }
                }
                if let Some(w) = b.when {
                    let w = When::parse(w).unwrap();
                    assert!(w.keys().all(|k| m.facts.contains(&k)));
                }
            }
            assert!(m.placeholders.iter().all(|p| PLACEHOLDERS.contains(p)));
        }
    }
}
