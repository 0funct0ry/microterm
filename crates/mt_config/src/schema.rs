//! The v1 configuration schema (SPEC §8.3). Every struct has serde defaults equal to the
//! documented defaults, so a missing file or a partial file is always a complete `Config`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub shell: String,
    pub term: String,
    pub scrollback_lines: u32,
    pub new_tab_cwd: String,
    pub confirm_close: ConfirmClose,
    pub startup_workspace: String,
    pub theme: ThemeSetting,
    pub env: BTreeMap<String, String>,
    pub font: Font,
    pub window: Window,
    pub tab_bar: TabBar,
    pub cursor: Cursor,
    pub text: Text,
    pub scrollbar: Scrollbar,
    pub selection: Selection,
    pub mouse: Mouse,
    pub panes: Panes,
    pub clipboard: Clipboard,
    pub paste: Paste,
    pub bell: Bell,
    pub quick_select: QuickSelect,
    pub hyperlinks: Hyperlinks,
    pub copy_mode: CopyMode,
    pub gpu: Gpu,
    pub ctl: Ctl,
    pub sh: Sh,
    pub notifications: Notifications,
    pub terminal: Terminal,
    pub hyperlink_rules: Vec<HyperlinkRule>,
    pub keys: Vec<KeyEntry>,
    pub launch: Vec<LaunchProfile>,
    pub workspaces: Vec<WorkspaceDef>,
    pub ssh_domains: Vec<SshDomain>,
    /// `[context_menu.<id>]`, keyed by registry id (SPEC §10.9).
    pub context_menu: BTreeMap<String, crate::menu::MenuConfig>,
    pub menu_groups: BTreeMap<String, crate::menu::MenuGroup>,
    pub menu_security: crate::menu::MenuSecurity,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: String::new(),
            term: "xterm-256color".into(),
            scrollback_lines: 10_000,
            new_tab_cwd: "inherit".into(),
            confirm_close: ConfirmClose::Process,
            startup_workspace: "default".into(),
            theme: ThemeSetting::default(),
            env: BTreeMap::new(),
            font: Font::default(),
            window: Window::default(),
            tab_bar: TabBar::default(),
            cursor: Cursor::default(),
            text: Text::default(),
            scrollbar: Scrollbar::default(),
            selection: Selection::default(),
            mouse: Mouse::default(),
            panes: Panes::default(),
            clipboard: Clipboard::default(),
            paste: Paste::default(),
            bell: Bell::default(),
            quick_select: QuickSelect::default(),
            hyperlinks: Hyperlinks::default(),
            copy_mode: CopyMode::default(),
            gpu: Gpu::default(),
            ctl: Ctl::default(),
            sh: Sh::default(),
            notifications: Notifications::default(),
            terminal: Terminal::default(),
            hyperlink_rules: Vec::new(),
            keys: Vec::new(),
            launch: Vec::new(),
            workspaces: Vec::new(),
            ssh_domains: Vec::new(),
            context_menu: BTreeMap::new(),
            menu_groups: BTreeMap::new(),
            menu_security: crate::menu::MenuSecurity::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmClose {
    Always,
    Process,
    Never,
}

/// `theme = "One Dark"` or `theme = { dark = "...", light = "..." }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ThemeSetting {
    Name(String),
    Pair { dark: String, light: String },
}

impl Default for ThemeSetting {
    fn default() -> Self {
        Self::Name("One Dark".into())
    }
}

impl ThemeSetting {
    /// The theme name to use for the given OS appearance.
    pub fn name_for(&self, dark: bool) -> &str {
        match self {
            Self::Name(n) => n,
            Self::Pair { dark: d, light: l } => {
                if dark {
                    d
                } else {
                    l
                }
            }
        }
    }

    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::Name(n) => vec![n],
            Self::Pair { dark, light } => vec![dark, light],
        }
    }
}

/// A font weight: a name (`"regular"`, `"bold"`, ...) or a number 100..=900.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FontWeight {
    Name(String),
    Number(u16),
}

impl FontWeight {
    /// CSS-style numeric weight; `None` for an unknown name.
    pub fn numeric(&self) -> Option<u16> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Name(n) => Some(match n.to_ascii_lowercase().as_str() {
                "thin" => 100,
                "extralight" | "extra-light" => 200,
                "light" => 300,
                "regular" | "normal" => 400,
                "medium" => 500,
                "semibold" | "semi-bold" => 600,
                "bold" => 700,
                "extrabold" | "extra-bold" => 800,
                "black" => 900,
                _ => return None,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Font {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
    pub weight: FontWeight,
    pub bold_weight: FontWeight,
    pub ligatures: bool,
    pub features: Vec<String>,
    pub fallback: Vec<String>,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: "JetBrains Mono".into(),
            size: 13.0,
            line_height: 1.2,
            weight: FontWeight::Name("regular".into()),
            bold_weight: FontWeight::Name("bold".into()),
            ligatures: true,
            features: Vec::new(),
            fallback: vec!["Symbols Nerd Font Mono".into()],
        }
    }
}

/// Parse an OpenType feature spec: `"zero"` (enabled) or `"ss02=1"` / `"calt=0"`.
/// The tag must be exactly four ASCII graphic characters.
pub fn parse_font_feature(spec: &str) -> Option<(String, u32)> {
    let (tag, value) = match spec.split_once('=') {
        Some((t, v)) => (t.trim(), v.trim().parse::<u32>().ok()?),
        None => (spec.trim(), 1),
    };
    (tag.len() == 4 && tag.bytes().all(|b| b.is_ascii_graphic())).then(|| (tag.to_string(), value))
}

#[cfg(test)]
mod font_feature_tests {
    use super::*;

    #[test]
    fn parses_tags_and_values() {
        assert_eq!(parse_font_feature("zero"), Some(("zero".into(), 1)));
        assert_eq!(parse_font_feature("ss02=0"), Some(("ss02".into(), 0)));
        assert_eq!(parse_font_feature("ss020"), None);
        assert_eq!(parse_font_feature("zer"), None);
        assert_eq!(parse_font_feature("zero=x"), None);
    }

    #[test]
    fn default_fallback_starts_with_nerd_symbols() {
        assert_eq!(Font::default().fallback[0], "Symbols Nerd Font Mono");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Padding {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decorations {
    Auto,
    Client,
    Server,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    pub padding: Padding,
    pub opacity: f32,
    pub blur: bool,
    pub decorations: Decorations,
    pub initial_size: InitialSize,
    pub remember_size: bool,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            padding: Padding { x: 8.0, y: 6.0 },
            opacity: 1.0,
            blur: false,
            decorations: Decorations::Auto,
            initial_size: InitialSize {
                cols: 120,
                rows: 34,
            },
            remember_size: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabBarPosition {
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabBar {
    pub position: TabBarPosition,
    pub hide_if_single: bool,
    pub title_format: String,
    pub show_index: bool,
    /// How a tab's color is drawn (SPEC §4.4).
    pub color_style: TabColorStyle,
    /// Per-color overrides of the nine theme colors (`orange = "#f59e42"`) plus up to ten
    /// extra names (`[a-z][a-z0-9-]*`) usable like the built-in ones.
    pub palette: std::collections::BTreeMap<String, String>,
    /// First match wins; a tab matches when any of its panes does.
    pub color_rules: Vec<ColorRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabColorStyle {
    #[serde(rename = "bar+tint")]
    BarTint,
    #[serde(rename = "bar")]
    Bar,
    #[serde(rename = "tint")]
    Tint,
    #[serde(rename = "dot")]
    Dot,
    /// Colors are stored and reported but not drawn.
    #[serde(rename = "none")]
    None,
}

/// `[[tab_bar.color_rules]]`: `{ match = "domain:prod-*", color = "red" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorRule {
    #[serde(rename = "match")]
    pub selector: String,
    pub color: String,
}

/// Most `[[tab_bar.color_rules]]` entries accepted.
pub const MAX_COLOR_RULES: usize = 64;
/// Most extra `[tab_bar.palette]` names beyond the nine built-in colors.
pub const MAX_EXTRA_COLORS: usize = 10;

impl Default for TabBar {
    fn default() -> Self {
        Self {
            position: TabBarPosition::Top,
            hide_if_single: false,
            title_format: "{title}".into(),
            show_index: false,
            color_style: TabColorStyle::BarTint,
            palette: Default::default(),
            color_rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    Block,
    Beam,
    Underline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cursor {
    pub style: CursorStyle,
    pub blink: bool,
    pub blink_interval_ms: u32,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            style: CursorStyle::Block,
            blink: true,
            blink_interval_ms: 530,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Text {
    pub blink: bool,
    pub min_contrast: f32,
    /// Count each regional indicator as two cells (four per flag), as zsh does. Fixes the
    /// cursor jumping after zsh-autosuggestions draws a flag emoji.
    pub wide_regional_indicators: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            blink: false,
            min_contrast: 1.0,
            wide_regional_indicators: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollbarMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scrollbar {
    pub mode: ScrollbarMode,
}

impl Default for Scrollbar {
    fn default() -> Self {
        Self {
            mode: ScrollbarMode::Auto,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub word_chars: String,
    pub copy_on_select: bool,
    pub trim_trailing_whitespace: bool,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            word_chars: "-_./~:@".into(),
            copy_on_select: false,
            trim_trailing_whitespace: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RightClick {
    ContextMenu,
    Paste,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mouse {
    pub right_click: RightClick,
    pub hide_while_typing: bool,
    pub scroll_multiplier: f32,
    /// Hovering a pane focuses it (SPEC §17.1).
    pub focus_follows_mouse: bool,
}

impl Default for Mouse {
    fn default() -> Self {
        Self {
            right_click: RightClick::ContextMenu,
            hide_while_typing: true,
            scroll_multiplier: 3.0,
            focus_follows_mouse: false,
        }
    }
}

/// `[panes]` (SPEC §17.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Panes {
    /// Dim overlay on unfocused panes, 0.0 to 1.0.
    pub inactive_dim: f32,
    /// Delay before `mouse.focus_follows_mouse` focuses the hovered pane.
    pub focus_delay_ms: u32,
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            inactive_dim: 0.15,
            focus_delay_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Osc52 {
    #[serde(rename = "write")]
    Write,
    #[serde(rename = "read-write")]
    ReadWrite,
    #[serde(rename = "disabled")]
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Clipboard {
    pub osc52: Osc52,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            osc52: Osc52::Write,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Paste {
    pub confirm_multiline: bool,
}

impl Default for Paste {
    fn default() -> Self {
        Self {
            confirm_multiline: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BellMode {
    Visual,
    Audible,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bell {
    pub mode: BellMode,
}

impl Default for Bell {
    fn default() -> Self {
        Self {
            mode: BellMode::Visual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickSelect {
    /// `false` turns quick select off: the action does nothing and leaves the palette.
    pub enabled: bool,
    pub alphabet: String,
    pub patterns: Vec<String>,
    pub disable_builtin: Vec<String>,
}

impl Default for QuickSelect {
    fn default() -> Self {
        Self {
            enabled: true,
            alphabet: "asdfqwerzxcvjklmiuopghtybn".into(),
            patterns: Vec::new(),
            disable_builtin: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkModifier {
    Cmd,
    Ctrl,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hyperlinks {
    pub schemes: Vec<String>,
    pub modifier: LinkModifier,
}

impl Default for Hyperlinks {
    fn default() -> Self {
        Self {
            schemes: ["http", "https", "mailto", "file"]
                .map(String::from)
                .to_vec(),
            modifier: LinkModifier::Cmd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HyperlinkRule {
    pub regex: String,
    pub format: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyMode {
    pub keys: Vec<KeyEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuPreference {
    HighPerformance,
    LowPower,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gpu {
    pub prefer: GpuPreference,
}

impl Default for Gpu {
    fn default() -> Self {
        Self {
            prefer: GpuPreference::HighPerformance,
        }
    }
}

/// `[ctl]`: limits for `microterm ctl` (SPEC §8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ctl {
    /// Cap on the output `pane run` and `pane get-text` return; the tail is kept.
    pub capture_max_bytes: u64,
    /// `pane status` text is truncated to this many characters.
    pub status_max_len: u32,
}

impl Default for Ctl {
    fn default() -> Self {
        Self {
            capture_max_bytes: 1_048_576,
            status_max_len: 120,
        }
    }
}

/// `[sh] color`: colour in `microterm sh` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShColor {
    /// Colour on a terminal, unless `NO_COLOR` is set.
    Auto,
    Always,
    Never,
}

/// `[sh]`: the interactive control shell (SPEC §12.7, §8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sh {
    pub color: ShColor,
    /// Keep lines that contain `send-text` in the history (they can carry secrets).
    pub history_send_text: bool,
    /// Offer `Close N panes? [y/N]` when a close needs confirmation (interactive sessions only).
    pub confirm_prompts: bool,
}

impl Default for Sh {
    fn default() -> Self {
        Self {
            color: ShColor::Auto,
            history_send_text: false,
            confirm_prompts: true,
        }
    }
}

/// When a `pane notify` raises a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastMode {
    /// Only when the pane is not the visible one.
    Background,
    Always,
    Never,
}

/// When a `pane notify` raises an OS notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopMode {
    /// Only while no microterm window is focused.
    Unfocused,
    Always,
    Never,
}

/// `[notifications]`: how `microterm ctl pane notify` surfaces (SPEC §8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    /// Attention badge on the pane's tab until it is focused.
    pub badge: bool,
    pub toast: ToastMode,
    pub desktop: DesktopMode,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            badge: true,
            toast: ToastMode::Background,
            desktop: DesktopMode::Unfocused,
        }
    }
}

/// `[terminal] osc_color_report`: reply format for OSC colour queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OscColorReport {
    /// `rgb:RRRR/GGGG/BBBB` (xterm).
    #[serde(rename = "16-bit")]
    Bit16,
    /// `rgb:RR/GG/BB`.
    #[serde(rename = "8-bit")]
    Bit8,
    /// Queries get no reply.
    #[serde(rename = "none")]
    None,
}

/// `[terminal]`: VT behaviour (SPEC §6.1, §6.3, §8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Terminal {
    pub osc_color_report: OscColorReport,
    /// OSC 9 desktop notifications from programs in a pane.
    pub osc_notifications: bool,
    /// Let a program turn on XTSHIFTESCAPE (Shift-modified mouse events reach it).
    pub allow_shift_capture: bool,
}

impl Default for Terminal {
    fn default() -> Self {
        Self {
            osc_color_report: OscColorReport::Bit16,
            osc_notifications: true,
            allow_shift_capture: true,
        }
    }
}

/// One `[[keys]]` entry: `key = "cmd+shift+r"`, `action = "ReloadConfig"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEntry {
    pub key: String,
    pub action: ActionRef,
}

/// An action reference: a bare name (`"ReloadConfig"`, `"Disable"`) or a single-entry table
/// (`{ ActivateTab = 3 }`, `{ SendString = "..." }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRef {
    pub name: String,
    pub arg: Option<ActionArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionArg {
    Int(i64),
    Str(String),
    Bool(bool),
    /// Structured argument such as `SpawnCommand = { command = [...], cwd = "..." }`.
    Table(String),
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum RawAction {
    Name(String),
    Table(BTreeMap<String, toml::Value>),
}

impl<'de> Deserialize<'de> for ActionRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match RawAction::deserialize(d)? {
            RawAction::Name(name) => Ok(Self { name, arg: None }),
            RawAction::Table(t) => {
                if t.len() != 1 {
                    return Err(D::Error::custom(
                        "an action table must have exactly one entry, e.g. { ActivateTab = 3 }",
                    ));
                }
                let (name, v) = t.into_iter().next().expect("len checked");
                let arg = match v {
                    toml::Value::Integer(i) => ActionArg::Int(i),
                    toml::Value::String(s) => ActionArg::Str(s),
                    toml::Value::Boolean(b) => ActionArg::Bool(b),
                    other => ActionArg::Table(other.to_string()),
                };
                Ok(Self {
                    name,
                    arg: Some(arg),
                })
            }
        }
    }
}

impl Serialize for ActionRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let raw = match &self.arg {
            None => RawAction::Name(self.name.clone()),
            Some(a) => {
                let v = match a {
                    ActionArg::Int(i) => toml::Value::Integer(*i),
                    ActionArg::Str(x) => toml::Value::String(x.clone()),
                    ActionArg::Bool(b) => toml::Value::Boolean(*b),
                    ActionArg::Table(t) => t
                        .parse::<toml::Value>()
                        .unwrap_or_else(|_| toml::Value::String(t.clone())),
                };
                RawAction::Table(BTreeMap::from([(self.name.clone(), v)]))
            }
        };
        raw.serialize(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchProfile {
    pub name: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Color of tabs opened from this profile (name or `#rrggbb`).
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTab {
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default, deserialize_with = "command_string_or_array")]
    pub command: Vec<String>,
    /// Makes the tab a split of the two `panes` instead of a single pane (SPEC §17.1).
    #[serde(default)]
    pub split: Option<SplitKind>,
    /// Share of the first pane, 1 to 99 (default 50).
    #[serde(default)]
    pub percent: Option<u8>,
    #[serde(default)]
    pub panes: Vec<WorkspacePane>,
}

/// How a declared split divides its area: `horizontal` puts the two panes side by side,
/// `vertical` stacks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitKind {
    Horizontal,
    Vertical,
}

/// One node of a declared split tree: a pane (`cwd`, `command`) or, with `split`, exactly two
/// child `panes`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePane {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default, deserialize_with = "command_string_or_array")]
    pub command: Vec<String>,
    #[serde(default)]
    pub split: Option<SplitKind>,
    #[serde(default)]
    pub percent: Option<u8>,
    #[serde(default)]
    pub panes: Vec<WorkspacePane>,
}

/// Splits nest at most this deep (a tab with `split` is depth 1).
pub const MAX_SPLIT_DEPTH: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDef {
    pub name: String,
    #[serde(default)]
    pub tabs: Vec<WorkspaceTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshDomain {
    pub name: String,
    pub host: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub identity_file: Option<String>,
    #[serde(default)]
    pub multiplexing: Option<String>,
    #[serde(default)]
    pub remote_dir: Option<String>,
    #[serde(default)]
    pub connect_timeout_s: Option<u32>,
    /// Color of every tab opened in this domain (name or `#rrggbb`).
    #[serde(default)]
    pub color: Option<String>,
}

/// Splits a command string into argv with POSIX-ish quoting (single, double, backslash).
pub fn split_command(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            '\'' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => cur.push(c),
                            Some(c) => {
                                cur.push('\\');
                                cur.push(c);
                            }
                            None => return Err("unterminated double quote".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                has = true;
                match chars.next() {
                    Some(c) => cur.push(c),
                    None => cur.push('\\'),
                }
            }
            c => {
                has = true;
                cur.push(c);
            }
        }
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// `command` accepts a string (shell-split) or an array of strings; the array is canonical.
fn command_string_or_array<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Cmd {
        Str(String),
        Arr(Vec<String>),
    }
    match Cmd::deserialize(d)? {
        Cmd::Arr(v) => Ok(v),
        Cmd::Str(s) => split_command(&s).map_err(serde::de::Error::custom),
    }
}
