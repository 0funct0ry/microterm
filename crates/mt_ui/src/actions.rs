//! Action definitions and the hard-coded action list (SPEC §9.2, §9.3). M6 replaces the list
//! with the full registry; the palette, keymap and key chips all read from `ACTIONS` so they
//! cannot drift apart.

use gpui::{actions, Action, App, KeyBinding, KeyBindingContextPredicate};

use crate::{keychips::format_keystroke, platform::Platform};

actions!(
    microterm,
    [
        Quit,
        SpawnTab,
        CloseTab,
        SpawnWindow,
        ToggleFullscreen,
        Copy,
        Paste,
        ClearScrollback,
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
        DebugShowSshOverlay,
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
    /// Hidden from the palette unless it is a debug entry.
    pub debug: bool,
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
    def!(debug "DebugShowSshOverlay", "Debug: Show SSH reconnect overlay", "Show the reconnect card with mock state", DebugShowSshOverlay),
    def!(debug "DebugShowConfigError", "Debug: Show config error banner", "Show the invalid-config banner", DebugShowConfigError),
    def!(debug "DebugShowToast", "Debug: Show toast", "Push a toast onto the stack", DebugShowToast),
];

impl ActionDef {
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

/// Every binding to install: the registry plus `⌘1..⌘9` / `Alt+1..Alt+9` and overlay keys.
pub fn key_bindings(platform: Platform, cx: &App) -> Vec<KeyBinding> {
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
        .unwrap_or_else(|e| panic!("bad built-in keystroke {keystroke:?}: {e}"))
    };
    let mut out = Vec::new();
    for def in ACTIONS {
        for keystroke in def.bindings(platform) {
            out.push(bind(keystroke, (def.build)(), CONTEXT_ROOT));
        }
    }
    let tab_mod = match platform {
        Platform::Mac => "cmd",
        Platform::Linux => "alt",
    };
    for n in 1..=9usize {
        out.push(bind(
            &format!("{tab_mod}-{n}"),
            Box::new(ActivateTab(n - 1)),
            CONTEXT_ROOT,
        ));
    }
    out.push(bind("escape", Box::new(DismissOverlay), CONTEXT_OVERLAY));
    out.push(bind("up", Box::new(PickerUp), CONTEXT_PICKER));
    out.push(bind("down", Box::new(PickerDown), CONTEXT_PICKER));
    out.push(bind("ctrl-p", Box::new(PickerUp), CONTEXT_PICKER));
    out.push(bind("ctrl-n", Box::new(PickerDown), CONTEXT_PICKER));
    out.push(bind("enter", Box::new(PickerConfirm), CONTEXT_PICKER));
    let new_window = match platform {
        Platform::Mac => "cmd-enter",
        Platform::Linux => "ctrl-enter",
    };
    out.push(bind(
        new_window,
        Box::new(PickerConfirmInNewWindow),
        CONTEXT_PICKER,
    ));
    out
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
}
