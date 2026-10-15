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
        SpawnWindow,
        ToggleFullscreen,
        Copy,
        Paste,
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

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ScrollByPage(pub isize);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = microterm, no_json)]
pub struct ScrollByLine(pub isize);

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
        ("ScrollByPage", Some(ActionArg::Int(n))) => Ok(Box::new(ScrollByPage(*n as isize))),
        ("ScrollByLine", Some(ActionArg::Int(n))) => Ok(Box::new(ScrollByLine(*n as isize))),
        (_, None) => ACTIONS
            .iter()
            .find(|d| d.name == name && !name.contains('('))
            .map(|d| (d.build)())
            .ok_or_else(|| format!("unknown action \"{name}\"")),
        (_, Some(_)) => Err(format!(
            "unknown action \"{name}\" or it takes no such argument"
        )),
    }
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
}
