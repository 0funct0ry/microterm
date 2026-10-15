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
    pub clipboard: Clipboard,
    pub paste: Paste,
    pub bell: Bell,
    pub quick_select: QuickSelect,
    pub hyperlinks: Hyperlinks,
    pub copy_mode: CopyMode,
    pub gpu: Gpu,
    pub hyperlink_rules: Vec<HyperlinkRule>,
    pub keys: Vec<KeyEntry>,
    pub launch: Vec<LaunchProfile>,
    pub workspaces: Vec<WorkspaceDef>,
    pub ssh_domains: Vec<SshDomain>,
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
            clipboard: Clipboard::default(),
            paste: Paste::default(),
            bell: Bell::default(),
            quick_select: QuickSelect::default(),
            hyperlinks: Hyperlinks::default(),
            copy_mode: CopyMode::default(),
            gpu: Gpu::default(),
            hyperlink_rules: Vec::new(),
            keys: Vec::new(),
            launch: Vec::new(),
            workspaces: Vec::new(),
            ssh_domains: Vec::new(),
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
            fallback: Vec::new(),
        }
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
}

impl Default for TabBar {
    fn default() -> Self {
        Self {
            position: TabBarPosition::Top,
            hide_if_single: false,
            title_format: "{title}".into(),
            show_index: false,
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
}

impl Default for Text {
    fn default() -> Self {
        Self {
            blink: false,
            min_contrast: 1.0,
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
    /// Accepted from v1.1.0 (splits).
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
    pub alphabet: String,
    pub patterns: Vec<String>,
    pub disable_builtin: Vec<String>,
}

impl Default for QuickSelect {
    fn default() -> Self {
        Self {
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTab {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
}

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
}
