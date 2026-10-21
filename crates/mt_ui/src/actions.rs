//! Action definitions and the hard-coded action list (SPEC §9.2, §9.3). M6 replaces the list
//! with the full registry; the palette, keymap and key chips all read from `ACTIONS` so they
//! cannot drift apart.

use std::collections::{HashMap, HashSet};

use gpui::{actions, Action, App, KeyBinding, KeyBindingContextPredicate};
use mt_config::{
    keys::{KeyBindingSpec, Scope},
    ActionArg,
};

use crate::{keychips::format_keystroke, platform::Platform};

actions!(
    microterm,
    [
        Quit,
        SpawnTab,
        CloseTab,
        SplitHorizontal,
        SplitVertical,
        ClosePane,
        TogglePaneZoom,
        RotatePanes,
        SpawnWindow,
        CloseWindow,
        RenameTab,
        CreateWorkspace,
        RenameWorkspace,
        DesignWorkspace,
        SaveLayoutAsWorkspace,
        ToggleFullscreen,
        Copy,
        Paste,
        PastePrimary,
        SelectAll,
        SearchSelection,
        ClearScrollback,
        ScrollToTop,
        ScrollToBottom,
        ShowCommandPalette,
        ShowLauncher,
        ShowWorkspaceSwitcher,
        ActivateQuickSelect,
        ActivateCopyMode,
        Search,
        SelectTheme,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        ReloadConfig,
        OpenConfig,
        DebugShowConfigError,
        DebugShowToast,
        // Overlay-internal; bound in the `Overlay` / `Picker` key contexts, not in the palette.
        DismissOverlay,
        PickerUp,
        PickerDown,
        PickerConfirm,
        PickerConfirmInNewWindow,
        NewWorkspaceFromQuery,
    ]
);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ActivateTab(pub usize);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ActivateTabRelative(pub isize);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct SwitchWorkspaceRelative(pub isize);

/// Focus the neighbouring pane (SPEC §17.1).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ActivatePaneDirection(pub mt_mux::Dir);

/// Swap the focused pane with its neighbour.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct SwapPane(pub mt_mux::Dir);

/// Move the focused pane's border by a number of cells.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ResizePane(pub mt_mux::Dir, pub u32);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct MoveTabRelative(pub isize);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct SwitchWorkspace(pub String);

/// New tab in a named domain. Only `local` exists until SSH domains arrive (M10).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct SpawnTabInDomain(pub String);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ScrollByPage(pub isize);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ScrollByLine(pub isize);

/// Jump to the previous (`-1`) or next (`1`) shell prompt. Needs OSC 133 marks (M8).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ScrollToPrompt(pub isize);

/// Writes the string to the active pane as if typed.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct SendString(pub String);

/// Opens a tab running a command: `{ SpawnCommand = { command = ["htop"], cwd = "~" } }`.
#[derive(Clone, Debug, Default, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct SpawnCommand {
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
}

/// New tab on an SSH domain (arrives with M10).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ConnectSsh(pub String);

/// Key context of the root view; overlays add `Overlay` and `Picker`.
pub const CONTEXT_ROOT: &str = "MicroTerm";
pub const CONTEXT_OVERLAY: &str = "Overlay";
pub const CONTEXT_PICKER: &str = "Picker";

/// One palette row / keymap entry.
pub struct ActionDef {
    /// Stable machine name, as used in config (`action = "SpawnTab"`).
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// GPUI keystrokes per platform. The first one is shown as the chip.
    pub mac: &'static [&'static str],
    pub linux: &'static [&'static str],
    pub build: fn() -> Box<dyn Action>,
    /// Debug entries have no default binding.
    pub debug: bool,
    /// Listed in the command palette. Argument-taking actions (`SendString`, ...) are not: they
    /// exist for the config and the docs, and the palette reaches them through providers.
    pub palette: bool,
}

macro_rules! def {
    ($name:literal, $label:literal, $desc:literal, $mac:expr, $linux:expr, $build:expr) => {
        ActionDef {
            name: $name,
            label: $label,
            description: $desc,
            mac: $mac,
            linux: $linux,
            build: || Box::new($build),
            debug: false,
            palette: true,
        }
    };
    (args $name:literal, $label:literal, $desc:literal, $build:expr) => {
        ActionDef {
            name: $name,
            label: $label,
            description: $desc,
            mac: &[],
            linux: &[],
            build: || Box::new($build),
            debug: false,
            palette: false,
        }
    };
    (debug $name:literal, $label:literal, $desc:literal, $build:expr) => {
        ActionDef {
            name: $name,
            label: $label,
            description: $desc,
            mac: &[],
            linux: &[],
            build: || Box::new($build),
            debug: true,
            palette: true,
        }
    };
}

pub static ACTIONS: &[ActionDef] = &[
    def!(
        "Quit",
        "Quit microterm",
        "Close every window and exit",
        &["cmd-q"],
        &["ctrl-shift-q"],
        Quit
    ),
    def!(
        "SpawnTab",
        "New tab",
        "Open a new tab",
        &["cmd-t"],
        &["ctrl-shift-t"],
        SpawnTab
    ),
    def!(
        "CloseTab",
        "Close tab",
        "Close the active tab",
        &["cmd-w"],
        &["ctrl-shift-w"],
        CloseTab
    ),
    def!(
        "ActivateTabRelative(1)",
        "Next tab",
        "Switch to the next tab",
        &["cmd-shift-]"],
        &["ctrl-tab"],
        ActivateTabRelative(1)
    ),
    def!(
        "ActivateTabRelative(-1)",
        "Previous tab",
        "Switch to the previous tab",
        &["cmd-shift-["],
        &["ctrl-shift-tab"],
        ActivateTabRelative(-1)
    ),
    def!(
        "MoveTabRelative(-1)",
        "Move tab left",
        "Move the active tab one position left",
        &[],
        &[],
        MoveTabRelative(-1)
    ),
    def!(
        "MoveTabRelative(1)",
        "Move tab right",
        "Move the active tab one position right",
        &[],
        &[],
        MoveTabRelative(1)
    ),
    def!(
        "RenameTab",
        "Rename tab",
        "Edit the active tab's title",
        &[],
        &[],
        RenameTab
    ),
    def!(
        "CloseWindow",
        "Close window",
        "Close the window and all its tabs",
        &[],
        &[],
        CloseWindow
    ),
    def!(
        "CreateWorkspace",
        "Create workspace",
        "Create a workspace and switch to it",
        &[],
        &[],
        CreateWorkspace
    ),
    def!(
        "RenameWorkspace",
        "Rename workspace",
        "Rename the active workspace",
        &[],
        &[],
        RenameWorkspace
    ),
    def!(
        "DesignWorkspace",
        "Design Workspace…",
        "Edit declared workspaces visually and save them to the config",
        &[],
        &[],
        DesignWorkspace
    ),
    def!(
        "SaveLayoutAsWorkspace",
        "Save current layout as workspace…",
        "Open the workspace designer seeded from the active tab's pane layout",
        &[],
        &[],
        SaveLayoutAsWorkspace
    ),
    def!(
        "SplitHorizontal",
        "Split pane right",
        "Split the focused pane into two side by side",
        &["cmd-d"],
        &["ctrl-shift-d"],
        SplitHorizontal
    ),
    def!(
        "SplitVertical",
        "Split pane down",
        "Split the focused pane into two, one above the other",
        &["cmd-shift-d"],
        &["ctrl-shift-e"],
        SplitVertical
    ),
    def!(
        "ClosePane",
        "Close pane",
        "Close the focused pane; the last pane of a tab closes the tab",
        &[],
        &[],
        ClosePane
    ),
    def!(
        "TogglePaneZoom",
        "Toggle pane zoom",
        "Show the focused pane full-size, or restore the layout",
        &[],
        &[],
        TogglePaneZoom
    ),
    def!(
        "RotatePanes",
        "Rotate panes",
        "Rotate the positions of the panes in the tab",
        &[],
        &[],
        RotatePanes
    ),
    def!(
        "ActivatePaneDirection(left)",
        "Focus pane left",
        "Move focus to the pane left of the focused one",
        &["cmd-alt-left"],
        &["ctrl-alt-left"],
        ActivatePaneDirection(mt_mux::Dir::Left)
    ),
    def!(
        "ActivatePaneDirection(right)",
        "Focus pane right",
        "Move focus to the pane right of the focused one",
        &["cmd-alt-right"],
        &["ctrl-alt-right"],
        ActivatePaneDirection(mt_mux::Dir::Right)
    ),
    def!(
        "ActivatePaneDirection(up)",
        "Focus pane up",
        "Move focus to the pane up of the focused one",
        &["cmd-alt-up"],
        &["ctrl-alt-up"],
        ActivatePaneDirection(mt_mux::Dir::Up)
    ),
    def!(
        "ActivatePaneDirection(down)",
        "Focus pane down",
        "Move focus to the pane down of the focused one",
        &["cmd-alt-down"],
        &["ctrl-alt-down"],
        ActivatePaneDirection(mt_mux::Dir::Down)
    ),
    def!(
        "SwapPane(left)",
        "Swap pane left",
        "Swap the focused pane with its neighbour to the left",
        &[],
        &[],
        SwapPane(mt_mux::Dir::Left)
    ),
    def!(
        "SwapPane(right)",
        "Swap pane right",
        "Swap the focused pane with its neighbour to the right",
        &[],
        &[],
        SwapPane(mt_mux::Dir::Right)
    ),
    def!(
        "SwapPane(up)",
        "Swap pane up",
        "Swap the focused pane with its neighbour to the up",
        &[],
        &[],
        SwapPane(mt_mux::Dir::Up)
    ),
    def!(
        "SwapPane(down)",
        "Swap pane down",
        "Swap the focused pane with its neighbour to the down",
        &[],
        &[],
        SwapPane(mt_mux::Dir::Down)
    ),
    def!(
        "ResizePane(left,5)",
        "Resize pane left",
        "Move the focused pane's left border by 5 cells",
        &[],
        &[],
        ResizePane(mt_mux::Dir::Left, 5)
    ),
    def!(
        "ResizePane(right,5)",
        "Resize pane right",
        "Move the focused pane's right border by 5 cells",
        &[],
        &[],
        ResizePane(mt_mux::Dir::Right, 5)
    ),
    def!(
        "ResizePane(up,5)",
        "Resize pane up",
        "Move the focused pane's up border by 5 cells",
        &[],
        &[],
        ResizePane(mt_mux::Dir::Up, 5)
    ),
    def!(
        "ResizePane(down,5)",
        "Resize pane down",
        "Move the focused pane's down border by 5 cells",
        &[],
        &[],
        ResizePane(mt_mux::Dir::Down, 5)
    ),
    def!(
        "SpawnWindow",
        "New window",
        "Open a new window",
        &["cmd-n"],
        &["ctrl-shift-n"],
        SpawnWindow
    ),
    def!(
        "ToggleFullscreen",
        "Toggle fullscreen",
        "Enter or leave fullscreen",
        &[],
        &[],
        ToggleFullscreen
    ),
    def!(
        "Copy",
        "Copy",
        "Copy the selection",
        &["cmd-c"],
        &["ctrl-shift-c"],
        Copy
    ),
    def!(
        "Paste",
        "Paste",
        "Paste from the clipboard",
        &["cmd-v"],
        &["ctrl-shift-v"],
        Paste
    ),
    def!(
        "SelectAll",
        "Select all",
        "Select the whole screen and scrollback",
        &["cmd-a"],
        &["ctrl-shift-a"],
        SelectAll
    ),
    def!(
        "SearchSelection",
        "Search for selection",
        "Open search pre-filled with the selected text",
        &[],
        &[],
        SearchSelection
    ),
    def!(
        "PastePrimary",
        "Paste primary selection",
        "Paste the Linux primary selection (middle-click buffer)",
        &[],
        &[],
        PastePrimary
    ),
    def!(
        "ShowCommandPalette",
        "Command palette",
        "Search and run any action",
        &["cmd-shift-p"],
        &["ctrl-shift-p"],
        ShowCommandPalette
    ),
    def!(
        "ShowLauncher",
        "Launcher",
        "Open a launch profile, SSH domain or workspace",
        &["cmd-shift-l"],
        &["ctrl-shift-l"],
        ShowLauncher
    ),
    def!(
        "ShowWorkspaceSwitcher",
        "Switch workspace",
        "Pick or create a workspace",
        &["cmd-alt-w"],
        &["ctrl-alt-w"],
        ShowWorkspaceSwitcher
    ),
    def!(
        "SwitchWorkspaceRelative(1)",
        "Next workspace",
        "Switch to the next workspace",
        &["cmd-alt-]"],
        &["ctrl-alt-]"],
        SwitchWorkspaceRelative(1)
    ),
    def!(
        "SwitchWorkspaceRelative(-1)",
        "Previous workspace",
        "Switch to the previous workspace",
        &["cmd-alt-["],
        &["ctrl-alt-["],
        SwitchWorkspaceRelative(-1)
    ),
    def!(
        "ActivateQuickSelect",
        "Quick select",
        "Label hashes, IPs and paths on screen to copy them",
        &["cmd-shift-space"],
        &["ctrl-shift-space"],
        ActivateQuickSelect
    ),
    def!(
        "ActivateCopyMode",
        "Copy mode",
        "Select text with the keyboard (vi-style)",
        &["cmd-shift-x"],
        &["ctrl-shift-x"],
        ActivateCopyMode
    ),
    def!(
        "Search",
        "Find",
        "Search the terminal output",
        &["cmd-f"],
        &["ctrl-shift-f"],
        Search
    ),
    def!(
        "SelectTheme",
        "Select theme",
        "Preview and switch the colour theme",
        &[],
        &[],
        SelectTheme
    ),
    def!(
        "IncreaseFontSize",
        "Increase font size",
        "Make the terminal text larger",
        &["cmd-="],
        &["ctrl-="],
        IncreaseFontSize
    ),
    def!(
        "DecreaseFontSize",
        "Decrease font size",
        "Make the terminal text smaller",
        &["cmd--"],
        &["ctrl--"],
        DecreaseFontSize
    ),
    def!(
        "ResetFontSize",
        "Reset font size",
        "Restore the configured font size",
        &["cmd-0"],
        &["ctrl-0"],
        ResetFontSize
    ),
    def!(
        "ClearScrollback",
        "Clear scrollback",
        "Erase the scrollback buffer",
        &["cmd-k"],
        &["ctrl-shift-k"],
        ClearScrollback
    ),
    def!(
        "ScrollByPage(-1)",
        "Scroll page up",
        "Scroll the terminal one page towards older output",
        &["shift-pageup"],
        &["shift-pageup"],
        ScrollByPage(-1)
    ),
    def!(
        "ScrollByPage(1)",
        "Scroll page down",
        "Scroll the terminal one page towards newer output",
        &["shift-pagedown"],
        &["shift-pagedown"],
        ScrollByPage(1)
    ),
    def!(
        "ScrollByLine(-1)",
        "Scroll line up",
        "Scroll the terminal one line towards older output",
        &[],
        &[],
        ScrollByLine(-1)
    ),
    def!(
        "ScrollByLine(1)",
        "Scroll line down",
        "Scroll the terminal one line towards newer output",
        &[],
        &[],
        ScrollByLine(1)
    ),
    def!(
        "ScrollToPrompt(-1)",
        "Scroll to previous prompt",
        "Jump to the previous shell prompt (needs OSC 133 shell integration)",
        &[],
        &[],
        ScrollToPrompt(-1)
    ),
    def!(
        "ScrollToPrompt(1)",
        "Scroll to next prompt",
        "Jump to the next shell prompt (needs OSC 133 shell integration)",
        &[],
        &[],
        ScrollToPrompt(1)
    ),
    def!(
        "ScrollToTop",
        "Scroll to top",
        "Jump to the oldest scrollback line",
        &["shift-home"],
        &["shift-home"],
        ScrollToTop
    ),
    def!(
        "ScrollToBottom",
        "Scroll to bottom",
        "Jump back to the live screen",
        &["shift-end"],
        &["shift-end"],
        ScrollToBottom
    ),
    def!(
        "ReloadConfig",
        "Reload config",
        "Re-read the config file",
        &["cmd-shift-r"],
        &["ctrl-shift-r"],
        ReloadConfig
    ),
    def!(
        "OpenConfig",
        "Open config",
        "Open the config file in your editor",
        &["cmd-,"],
        &["ctrl-,"],
        OpenConfig
    ),
    def!(
        args "ActivateTab",
        "Go to tab",
        "Activate tab N (counting from 1); bound to ⌘1–⌘9 on macOS and Alt+1–Alt+9 on Linux",
        ActivateTab(0)
    ),
    def!(
        args "SwitchWorkspace",
        "Switch to workspace",
        "Switch to the workspace with the given name",
        SwitchWorkspace(String::new())
    ),
    def!(
        args "SpawnTabInDomain",
        "New tab in domain",
        "Open a tab in the named domain (`local`, or an SSH domain once M10 lands)",
        SpawnTabInDomain(String::new())
    ),
    def!(
        args "SendString",
        "Send string",
        "Write the given text to the active pane as if typed",
        SendString(String::new())
    ),
    def!(
        args "SpawnCommand",
        "Spawn command",
        "Open a tab running `command`, with optional `cwd` and `title`",
        SpawnCommand::default()
    ),
    def!(
        args "ConnectSsh",
        "Connect SSH",
        "Open a tab on the named SSH domain (arrives with M10)",
        ConnectSsh(String::new())
    ),
    def!(debug "DebugShowConfigError", "Debug: Show config error banner", "Show the invalid-config banner", DebugShowConfigError),
    def!(debug "DebugShowToast", "Debug: Show toast", "Push a toast onto the stack", DebugShowToast),
];

/// Documentation grouping of the registry (SPEC §9.2).
pub const CATEGORIES: &[&str] = &[
    "Tabs & windows",
    "Panes",
    "Workspaces",
    "Clipboard & scroll",
    "Modes",
    "Fonts & look",
    "Config",
    "Misc",
    "Debug",
];

impl ActionDef {
    pub fn category(&self) -> &'static str {
        let base = self.name.split('(').next().unwrap_or(self.name);
        match base {
            _ if self.debug => "Debug",
            "SpawnTab"
            | "SpawnTabInDomain"
            | "CloseTab"
            | "ActivateTab"
            | "ActivateTabRelative"
            | "MoveTabRelative"
            | "RenameTab"
            | "SpawnWindow"
            | "CloseWindow"
            | "ToggleFullscreen" => "Tabs & windows",
            "SplitHorizontal"
            | "SplitVertical"
            | "ClosePane"
            | "TogglePaneZoom"
            | "RotatePanes"
            | "ActivatePaneDirection"
            | "SwapPane"
            | "ResizePane" => "Panes",
            "ShowWorkspaceSwitcher"
            | "SwitchWorkspace"
            | "SwitchWorkspaceRelative"
            | "CreateWorkspace"
            | "RenameWorkspace"
            | "DesignWorkspace"
            | "SaveLayoutAsWorkspace" => "Workspaces",
            "Copy" | "Paste" | "PastePrimary" | "SelectAll" | "ClearScrollback"
            | "ScrollByPage" | "ScrollByLine" | "ScrollToTop" | "ScrollToBottom"
            | "ScrollToPrompt" => "Clipboard & scroll",
            "ShowCommandPalette"
            | "ShowLauncher"
            | "ActivateQuickSelect"
            | "ActivateCopyMode"
            | "Search"
            | "SearchSelection" => "Modes",
            "IncreaseFontSize" | "DecreaseFontSize" | "ResetFontSize" | "SelectTheme" => {
                "Fonts & look"
            }
            "ReloadConfig" | "OpenConfig" => "Config",
            _ => "Misc",
        }
    }

    pub fn bindings(&self, platform: Platform) -> &'static [&'static str] {
        match platform {
            Platform::Mac => self.mac,
            Platform::Linux => self.linux,
        }
    }

    /// The chip text for this platform, e.g. `⌘⇧P`.
    pub fn chip(&self, platform: Platform) -> Option<String> {
        self.bindings(platform)
            .first()
            .map(|k| format_keystroke(k, platform))
    }
}

/// Config name of an action reference: `SpawnTab` or `ActivateTabRelative(1)`.
pub fn action_key(name: &str, arg: &Option<ActionArg>) -> String {
    match arg {
        None => name.to_string(),
        Some(ActionArg::Int(n)) => format!("{name}({n})"),
        Some(ActionArg::Str(s)) => format!("{name}({s})"),
        Some(ActionArg::Bool(b)) => format!("{name}({b})"),
        Some(ActionArg::Table(t)) => format!("{name}({t})"),
    }
}

/// Builds the action a config entry names. `ActivateTab` counts from 1 in the config.
pub fn resolve_action(name: &str, arg: &Option<ActionArg>) -> Result<Box<dyn Action>, String> {
    match (name, arg) {
        ("ActivateTab", Some(ActionArg::Int(n))) if (1..=9999).contains(n) => {
            Ok(Box::new(ActivateTab(*n as usize - 1)))
        }
        ("ActivateTab", _) => Err("ActivateTab takes a tab number from 1".into()),
        ("ActivateTabRelative", Some(ActionArg::Int(n))) => {
            Ok(Box::new(ActivateTabRelative(*n as isize)))
        }
        ("SwitchWorkspaceRelative", Some(ActionArg::Int(n))) => {
            Ok(Box::new(SwitchWorkspaceRelative(*n as isize)))
        }
        ("MoveTabRelative", Some(ActionArg::Int(n))) => Ok(Box::new(MoveTabRelative(*n as isize))),
        ("ActivatePaneDirection", Some(ActionArg::Str(d))) => Ok(Box::new(ActivatePaneDirection(
            mt_mux::Dir::parse(d).ok_or("direction must be left, right, up or down")?,
        ))),
        ("SwapPane", Some(ActionArg::Str(d))) => Ok(Box::new(SwapPane(
            mt_mux::Dir::parse(d).ok_or("direction must be left, right, up or down")?,
        ))),
        ("ResizePane", Some(ActionArg::Table(t))) => parse_resize_pane(t)
            .map(|a| Box::new(a) as Box<dyn Action>)
            .map_err(|e| format!("ResizePane: {e}")),
        ("ResizePane", _) => {
            Err("ResizePane takes a table: { direction = \"left\", cells = 5 }".into())
        }
        ("SwitchWorkspace", Some(ActionArg::Str(name))) => {
            Ok(Box::new(SwitchWorkspace(name.clone())))
        }
        ("SpawnTabInDomain", Some(ActionArg::Str(name))) => {
            Ok(Box::new(SpawnTabInDomain(name.clone())))
        }
        ("ScrollByPage", Some(ActionArg::Int(n))) => Ok(Box::new(ScrollByPage(*n as isize))),
        ("ScrollByLine", Some(ActionArg::Int(n))) => Ok(Box::new(ScrollByLine(*n as isize))),
        ("ScrollToPrompt", Some(ActionArg::Int(n))) => Ok(Box::new(ScrollToPrompt(*n as isize))),
        ("SendString", Some(ActionArg::Str(s))) => Ok(Box::new(SendString(s.clone()))),
        ("ConnectSsh", Some(ActionArg::Str(d))) => Ok(Box::new(ConnectSsh(d.clone()))),
        ("SpawnCommand", Some(ActionArg::Table(t))) => parse_spawn_command(t)
            .map(|c| Box::new(c) as Box<dyn Action>)
            .map_err(|e| format!("SpawnCommand: {e}")),
        ("SpawnCommand", _) => {
            Err("SpawnCommand takes a table: { command = [\"htop\"], cwd = \"~\" }".into())
        }
        (_, None) => ACTIONS
            .iter()
            .find(|d| d.name == name && d.palette && !name.contains('('))
            .map(|d| (d.build)())
            .ok_or_else(|| format!("unknown action \"{name}\"")),
        (_, Some(_)) => Err(format!(
            "unknown action \"{name}\" or it takes no such argument"
        )),
    }
}

/// The generated "Reference → Actions" docs page (`cargo run -p xtask -- gen-docs`).
pub fn reference_markdown() -> String {
    fn keys(def: &ActionDef, platform: Platform) -> String {
        let all: Vec<String> = def
            .bindings(platform)
            .iter()
            .map(|k| format!("`{}`", format_keystroke(k, platform)))
            .collect();
        if all.is_empty() {
            "—".to_string()
        } else {
            all.join(" / ")
        }
    }
    let mut out = String::from(
        "---\ntitle: Actions\ndescription: Every microterm action with its default keybinding on macOS and Linux.\n---\n\n\
<!-- Generated by `cargo run -p xtask -- gen-docs` from the action registry. Do not edit. -->\n\n\
Every action can be run from the [command palette](/docs/command-palette-and-launcher/) and bound \
under `[[keys]]` in the config. Actions that take an argument are written as a table:\n\n\
```toml\n[[keys]]\nkey = \"cmd+shift+h\"\naction = { SpawnCommand = { command = [\"htop\"], cwd = \"~\" } }\n\n\
[[keys]]\nkey = \"cmd+shift+k\"\naction = \"Disable\"   # unbind a default\n```\n",
    );
    for category in CATEGORIES {
        let rows: Vec<&ActionDef> = ACTIONS
            .iter()
            .filter(|d| d.category() == *category)
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n## {category}\n\n| Action | Description | macOS | Linux |\n|---|---|---|---|\n"
        ));
        for def in rows {
            out.push_str(&format!(
                "| `{}` | **{}** — {} | {} | {} |\n",
                def.name,
                def.label,
                def.description,
                keys(def, Platform::Mac),
                keys(def, Platform::Linux)
            ));
        }
    }
    out
}

/// Where the generated page lives, relative to the repository root.
pub const REFERENCE_DOC_PATH: &str = "site/src/content/docs/docs/reference/actions.md";

/// Parses the table text of a `SpawnCommand = { ... }` config argument.
pub fn parse_resize_pane(table: &str) -> Result<ResizePane, String> {
    let t: toml::Table = table
        .parse()
        .map_err(|e: toml::de::Error| e.message().to_string())?;
    let dir = t
        .get("direction")
        .and_then(|v| v.as_str())
        .and_then(mt_mux::Dir::parse)
        .ok_or("`direction` must be \"left\", \"right\", \"up\" or \"down\"")?;
    let cells = match t.get("cells") {
        None => 1,
        Some(toml::Value::Integer(n)) if (1..=1000).contains(n) => *n as u32,
        Some(_) => return Err("`cells` must be a number from 1 to 1000".into()),
    };
    Ok(ResizePane(dir, cells))
}

pub fn parse_spawn_command(table: &str) -> Result<SpawnCommand, String> {
    let t: toml::Table = table
        .parse()
        .map_err(|e: toml::de::Error| e.message().to_string())?;
    let command: Vec<String> = match t.get("command") {
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or("`command` must be a list of strings")?,
        _ => return Err("missing `command = [\"program\", \"args\"...]`".into()),
    };
    if command.is_empty() {
        return Err("`command` must not be empty".into());
    }
    let opt = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Ok(SpawnCommand {
        command,
        cwd: opt("cwd"),
        title: opt("title"),
    })
}

/// `(gpui keystroke, action)` pairs plus key chips, from the defaults and the user's bindings.
struct KeymapPlan {
    binds: Vec<(String, Box<dyn Action>, &'static str)>,
    chips: HashMap<&'static str, String>,
    warnings: Vec<String>,
}

fn same_stroke(a: &str, b: &str) -> bool {
    match (gpui::Keystroke::parse(a), gpui::Keystroke::parse(b)) {
        (Ok(x), Ok(y)) => x.modifiers == y.modifiers && x.key == y.key,
        _ => a == b,
    }
}

fn plan_keymap(platform: Platform, user: &[KeyBindingSpec]) -> KeymapPlan {
    let mut plan = KeymapPlan {
        binds: Vec::new(),
        chips: HashMap::new(),
        warnings: Vec::new(),
    };
    let disabled: Vec<String> = user
        .iter()
        .filter(|s| s.scope == Scope::Terminal && s.action.is_none())
        .map(|s| s.keystroke.to_string())
        .collect();
    let is_disabled = |ks: &str| disabled.iter().any(|d| same_stroke(d, ks));

    for def in ACTIONS {
        for ks in def.bindings(platform) {
            if is_disabled(ks) {
                continue;
            }
            plan.binds
                .push((ks.to_string(), (def.build)(), CONTEXT_ROOT));
            plan.chips
                .entry(def.name)
                .or_insert_with(|| format_keystroke(ks, platform));
        }
    }
    let tab_mod = match platform {
        Platform::Mac => "cmd",
        Platform::Linux => "alt",
    };
    for n in 1..=9usize {
        let ks = format!("{tab_mod}-{n}");
        if !is_disabled(&ks) {
            plan.binds
                .push((ks, Box::new(ActivateTab(n - 1)), CONTEXT_ROOT));
        }
    }

    // User bindings come last, so they win over defaults on the same keystroke.
    let mut user_chips: HashSet<&'static str> = HashSet::new();
    for spec in user.iter().filter(|s| s.scope == Scope::Terminal) {
        let Some(action) = &spec.action else { continue };
        let ks = spec.keystroke.to_string();
        match resolve_action(&action.name, &action.arg) {
            Ok(a) => {
                plan.binds.push((ks.clone(), a, CONTEXT_ROOT));
                let key = action_key(&action.name, &action.arg);
                if let Some(def) = ACTIONS.iter().find(|d| d.name == key) {
                    // The user's first binding replaces the default chip.
                    if user_chips.insert(def.name) {
                        plan.chips.insert(def.name, format_keystroke(&ks, platform));
                    }
                }
            }
            Err(e) => plan
                .warnings
                .push(format!("key \"{}\": {e}; binding skipped", spec.keystroke)),
        }
    }
    plan
}

/// Chips and warnings for a set of user bindings, without touching GPUI.
pub fn keymap_chips(
    platform: Platform,
    user: &[KeyBindingSpec],
) -> (HashMap<&'static str, String>, Vec<String>) {
    let plan = plan_keymap(platform, user);
    (plan.chips, plan.warnings)
}

/// Every binding to install: the registry, `⌘1..⌘9` / `Alt+1..Alt+9`, the user's `[[keys]]`
/// and the overlay keys.
pub fn build_keymap(
    platform: Platform,
    cx: &App,
    user: &[KeyBindingSpec],
) -> (Vec<KeyBinding>, Vec<String>) {
    let mapper = cx.keyboard_mapper().clone();
    let bind = |keystroke: &str, action: Box<dyn Action>, context: &str| {
        let predicate = KeyBindingContextPredicate::parse(context)
            .expect("static context predicate")
            .into();
        KeyBinding::load(
            keystroke,
            action,
            Some(predicate),
            false,
            None,
            mapper.as_ref(),
        )
    };
    let plan = plan_keymap(platform, user);
    let mut warnings = plan.warnings;
    let mut out = Vec::new();
    for (ks, action, ctx) in plan.binds {
        match bind(&ks, action, ctx) {
            Ok(b) => out.push(b),
            Err(e) => warnings.push(format!("key \"{ks}\": {e}; binding skipped")),
        }
    }
    let fixed = |ks: &str, a: Box<dyn Action>, ctx: &str| {
        bind(ks, a, ctx).unwrap_or_else(|e| panic!("bad built-in keystroke {ks:?}: {e}"))
    };
    out.push(fixed("escape", Box::new(DismissOverlay), CONTEXT_OVERLAY));
    out.push(fixed("up", Box::new(PickerUp), CONTEXT_PICKER));
    out.push(fixed("down", Box::new(PickerDown), CONTEXT_PICKER));
    out.push(fixed("ctrl-p", Box::new(PickerUp), CONTEXT_PICKER));
    out.push(fixed("ctrl-n", Box::new(PickerDown), CONTEXT_PICKER));
    out.push(fixed("enter", Box::new(PickerConfirm), CONTEXT_PICKER));
    let new_window = match platform {
        Platform::Mac => "cmd-enter",
        Platform::Linux => "ctrl-enter",
    };
    out.push(fixed(
        new_window,
        Box::new(PickerConfirmInNewWindow),
        CONTEXT_PICKER,
    ));
    (out, warnings)
}

/// The default bindings only (tests and first start-up before the config is read).
pub fn key_bindings(platform: Platform, cx: &App) -> Vec<KeyBinding> {
    build_keymap(platform, cx, &[]).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_name_is_unique() {
        let mut names: Vec<_> = ACTIONS.iter().map(|a| a.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
    }

    #[test]
    fn default_chips_match_the_spec_table() {
        let get = |n: &str| ACTIONS.iter().find(|a| a.name == n).unwrap();
        assert_eq!(
            get("ShowCommandPalette").chip(Platform::Mac).unwrap(),
            "⌘⇧P"
        );
        assert_eq!(
            get("ShowCommandPalette").chip(Platform::Linux).unwrap(),
            "Ctrl+Shift+P"
        );
        assert_eq!(
            get("ShowWorkspaceSwitcher").chip(Platform::Mac).unwrap(),
            "⌘⌥W"
        );
        assert_eq!(
            get("ShowWorkspaceSwitcher").chip(Platform::Linux).unwrap(),
            "Ctrl+Alt+W"
        );
        assert_eq!(get("Search").chip(Platform::Mac).unwrap(), "⌘F");
        assert_eq!(get("Search").chip(Platform::Linux).unwrap(), "Ctrl+Shift+F");
        assert_eq!(
            get("ActivateQuickSelect").chip(Platform::Linux).unwrap(),
            "Ctrl+Shift+Space"
        );
        assert_eq!(get("ActivateCopyMode").chip(Platform::Mac).unwrap(), "⌘⇧X");
    }

    #[test]
    fn linux_defaults_never_steal_plain_ctrl_letters() {
        for def in ACTIONS {
            for ks in def.linux {
                let plain_ctrl_letter = ks.starts_with("ctrl-")
                    && ks.len() == "ctrl-".len() + 1
                    && ks.chars().last().is_some_and(|c| c.is_ascii_alphabetic());
                assert!(!plain_ctrl_letter, "{} binds {ks}", def.name);
            }
        }
    }

    #[test]
    fn debug_actions_have_no_default_binding() {
        for def in ACTIONS.iter().filter(|a| a.debug) {
            assert!(def.mac.is_empty() && def.linux.is_empty());
        }
    }

    fn specs(src: &str) -> Vec<KeyBindingSpec> {
        let cfg: mt_config::Config = toml::from_str(src).unwrap();
        mt_config::keys::bindings(&cfg).unwrap()
    }

    #[test]
    fn user_binding_replaces_chip_and_disable_removes_default() {
        let user = specs(
            r#"
            [[keys]]
            key = "cmd+shift+r"
            action = "Disable"
            [[keys]]
            key = "ctrl+alt+r"
            action = "ReloadConfig"
            [[keys]]
            key = "cmd+t"
            action = "Disable"
            "#,
        );
        let (chips, warnings) = keymap_chips(Platform::Mac, &user);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(chips["ReloadConfig"], "⌃⌥R");
        assert!(
            !chips.contains_key("SpawnTab"),
            "disabled default has no chip"
        );
        assert_eq!(chips["CloseTab"], "⌘W");
    }

    #[test]
    fn unknown_actions_warn_and_are_skipped() {
        let user = specs(
            r#"
            [[keys]]
            key = "ctrl+alt+x"
            action = "Frobnicate"
            [[keys]]
            key = "ctrl+alt+y"
            action = { ActivateTab = 0 }
            "#,
        );
        let (_, warnings) = keymap_chips(Platform::Linux, &user);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("Frobnicate"));
        assert!(warnings[1].contains("ActivateTab"));
    }

    #[test]
    fn parametrised_actions_resolve() {
        assert!(resolve_action("ActivateTab", &Some(ActionArg::Int(3))).is_ok());
        assert!(resolve_action("ScrollByLine", &Some(ActionArg::Int(-1))).is_ok());
        assert!(resolve_action("SpawnTab", &None).is_ok());
        assert!(resolve_action("SpawnTab", &Some(ActionArg::Int(1))).is_err());
        assert!(resolve_action("ActivateTab", &None).is_err());
        assert_eq!(
            action_key("ActivateTabRelative", &Some(ActionArg::Int(1))),
            "ActivateTabRelative(1)"
        );
    }

    #[test]
    fn defaults_have_a_chip_for_every_bound_action() {
        let (chips, warnings) = keymap_chips(Platform::Linux, &[]);
        assert!(warnings.is_empty());
        for def in ACTIONS.iter().filter(|d| !d.linux.is_empty()) {
            assert!(chips.contains_key(def.name), "{}", def.name);
        }
    }

    #[test]
    fn registry_covers_every_spec_action() {
        // SPEC §9.2 (Disable is a config keyword, not an action).
        for name in [
            "SpawnTab",
            "SpawnTabInDomain",
            "CloseTab",
            "SplitHorizontal",
            "SplitVertical",
            "ClosePane",
            "ActivatePaneDirection(left)",
            "ResizePane(left,5)",
            "TogglePaneZoom",
            "SwapPane(left)",
            "RotatePanes",
            "ActivateTab",
            "ActivateTabRelative(1)",
            "MoveTabRelative(1)",
            "RenameTab",
            "SpawnWindow",
            "CloseWindow",
            "ToggleFullscreen",
            "ShowWorkspaceSwitcher",
            "SwitchWorkspace",
            "SwitchWorkspaceRelative(1)",
            "CreateWorkspace",
            "RenameWorkspace",
            "DesignWorkspace",
            "SaveLayoutAsWorkspace",
            "Copy",
            "Paste",
            "PastePrimary",
            "ClearScrollback",
            "ScrollByPage(1)",
            "ScrollByLine(1)",
            "ScrollToTop",
            "ScrollToBottom",
            "ScrollToPrompt(1)",
            "ShowCommandPalette",
            "ShowLauncher",
            "ActivateQuickSelect",
            "ActivateCopyMode",
            "Search",
            "IncreaseFontSize",
            "DecreaseFontSize",
            "ResetFontSize",
            "SelectTheme",
            "ReloadConfig",
            "OpenConfig",
            "SendString",
            "SpawnCommand",
            "ConnectSsh",
        ] {
            assert!(ACTIONS.iter().any(|a| a.name == name), "{name} missing");
        }
    }

    #[test]
    fn every_action_has_label_description_and_category() {
        for def in ACTIONS {
            assert!(
                !def.label.is_empty() && !def.description.is_empty(),
                "{}",
                def.name
            );
            assert!(CATEGORIES.contains(&def.category()), "{}", def.name);
            // Building must not panic.
            let _ = (def.build)();
        }
    }

    #[test]
    fn spawn_command_resolves_from_a_table() {
        let a = resolve_action(
            "SpawnCommand",
            &Some(ActionArg::Table("command = [\"htop\"]\ncwd = \"~\"".into())),
        )
        .unwrap();
        let c = a.as_any().downcast_ref::<SpawnCommand>().unwrap();
        assert_eq!(c.command, ["htop"]);
        assert_eq!(c.cwd.as_deref(), Some("~"));
        assert!(resolve_action("SpawnCommand", &None).is_err());
        assert!(resolve_action(
            "SpawnCommand",
            &Some(ActionArg::Table("cwd = \"~\"".into()))
        )
        .is_err());
        assert!(resolve_action("SendString", &Some(ActionArg::Str("x".into()))).is_ok());
        assert!(resolve_action("SendString", &None).is_err());
    }

    #[test]
    fn generated_actions_doc_matches_the_registry() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(REFERENCE_DOC_PATH);
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            on_disk == reference_markdown(),
            "{REFERENCE_DOC_PATH} is stale: run `cargo run -p xtask -- gen-docs`"
        );
    }

    #[test]
    fn reference_lists_every_action_once() {
        let md = reference_markdown();
        for def in ACTIONS {
            let row = format!("| `{}` |", def.name);
            assert_eq!(md.matches(&row).count(), 1, "{}", def.name);
        }
    }
}
