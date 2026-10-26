//! The macOS main menu (SPEC §10.7). `build_menu_bar` is a pure model of the menus;
//! `apply_menu_bar` hands it to GPUI. Linux has no global menu bar and is unchanged.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{Action, App, Global, Menu, MenuItem, SystemMenuType, Window};
use mt_config::{ActionArg, Config};
use serde::Serialize;

use crate::actions::{self, MenuDisabled, ACTIONS};
use crate::keychips::format_keystroke;
use crate::overlay_stack::OverlayKind;
use crate::platform::Platform;
use crate::{app::AppWindow, mux_host, settings::Settings};

/// What the enabled state of rows depends on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    /// Panes in the active tab of the focused window.
    pub panes: usize,
    pub tabs: usize,
    pub has_selection: bool,
    /// A hyperlink lies under the last pointer position.
    pub link_under_pointer: bool,
    pub workspaces: usize,
    /// Workspaces detached earlier and not yet attached again.
    pub parked: Vec<String>,
    /// The focused pane is recording (the Edit row reads "Stop Recording").
    pub recording: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    pub title: String,
    /// Action key as in the config (`Split(left)`); `None` for OS-provided rows.
    pub action: Option<String>,
    pub enabled: bool,
    pub chip: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item(Item),
    Separator,
    Submenu {
        title: String,
        entries: Vec<Entry>,
    },
    /// The OS-provided Services submenu.
    Services,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuModel {
    pub title: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuBar {
    pub menus: Vec<MenuModel>,
}

/// One flattened row for `ctl menu-bar list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub path: String,
    pub title: String,
    pub action: Option<String>,
    pub enabled: bool,
    pub chip: Option<String>,
}

impl MenuBar {
    pub fn rows(&self) -> Vec<Row> {
        fn walk(path: &str, entries: &[Entry], out: &mut Vec<Row>) {
            for e in entries {
                match e {
                    Entry::Item(i) => out.push(Row {
                        path: format!("{path} > {}", i.title),
                        title: i.title.clone(),
                        action: i.action.clone(),
                        enabled: i.enabled,
                        chip: i.chip.clone(),
                    }),
                    Entry::Submenu { title, entries } => {
                        walk(&format!("{path} > {title}"), entries, out)
                    }
                    Entry::Separator | Entry::Services => {}
                }
            }
        }
        let mut out = Vec::new();
        for m in &self.menus {
            walk(&m.title, &m.entries, &mut out);
        }
        out
    }

    /// Plain text for `ctl menu-bar list`.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for r in self.rows() {
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                r.path,
                r.action.as_deref().unwrap_or("-"),
                if r.enabled { "enabled" } else { "disabled" },
                r.chip.as_deref().unwrap_or("")
            ));
        }
        s
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({ "menus": self.menus.iter().map(|m| &m.title).collect::<Vec<_>>(),
                            "items": self.rows() })
    }
}

struct Ctx<'a> {
    state: &'a MenuState,
    chips: &'a HashMap<&'static str, String>,
}

impl Ctx<'_> {
    fn chip(&self, key: &str) -> Option<String> {
        if let Some(c) = self.chips.get(key) {
            return Some(c.clone());
        }
        // ⌘1..⌘9 are installed outside the registry.
        let n = key.strip_prefix("ActivateTab(")?.strip_suffix(')')?;
        let (mac, linux) = (format!("cmd-{n}"), format!("alt-{n}"));
        let p = Platform::current();
        Some(format_keystroke(
            if p == Platform::Mac { &mac } else { &linux },
            p,
        ))
    }

    fn item(&self, title: &str, key: &str, enabled: bool) -> Entry {
        Entry::Item(Item {
            title: title.into(),
            action: Some(key.into()),
            enabled,
            chip: self.chip(key),
        })
    }

    fn on(&self, title: &str, key: &str) -> Entry {
        self.item(title, key, true)
    }
}

fn sub(title: &str, entries: Vec<Entry>) -> Entry {
    Entry::Submenu {
        title: title.into(),
        entries,
    }
}

/// The resolved menus. Pure: no GPUI calls.
pub fn build_menu_bar(
    state: &MenuState,
    config: &Config,
    chips: &HashMap<&'static str, String>,
) -> MenuBar {
    let c = Ctx { state, chips };
    let st = c.state;
    let many_panes = st.panes >= 2;
    let app = MenuModel {
        title: "microterm".into(),
        entries: if config.menu_bar.enabled {
            vec![
                c.on("About microterm", "ShowAbout"),
                Entry::Separator,
                c.on("Settings…", "OpenConfig"),
                c.on("Reload Config", "ReloadConfig"),
                Entry::Separator,
                Entry::Services,
                Entry::Separator,
                c.on("Hide microterm", "HideApp"),
                c.on("Hide Others", "HideOthers"),
                c.on("Show All", "ShowAllApps"),
                Entry::Separator,
                c.on("Quit microterm", "Quit"),
            ]
        } else {
            vec![c.on("Quit microterm", "Quit")]
        },
    };
    if !config.menu_bar.enabled {
        return MenuBar { menus: vec![app] };
    }

    let mut domains = vec![c.on("Local", "SpawnTabInDomain(local)")];
    for d in &config.ssh_domains {
        domains.push(c.on(
            &format!("SSH: {}", d.name),
            &format!("SpawnTabInDomain({})", d.name),
        ));
    }
    for p in &config.serial_ports {
        domains.push(c.on(
            &format!("Serial: {}", p.name),
            &format!("SpawnTabInDomain({})", p.name),
        ));
    }
    let attach = if st.parked.is_empty() {
        c.item("Attach", "AttachWorkspace", false)
    } else {
        sub(
            "Attach",
            st.parked
                .iter()
                .map(|n| c.on(n, &format!("AttachWorkspace({n})")))
                .collect(),
        )
    };
    let shell = MenuModel {
        title: "Shell".into(),
        entries: vec![
            c.on("New Tab", "SpawnTab"),
            c.on("New Window", "SpawnWindow"),
            Entry::Separator,
            c.on("Split Right", "SplitHorizontal"),
            c.on("Split Down", "SplitVertical"),
            c.on("Split Left", "Split(left)"),
            c.on("Split Up", "Split(up)"),
            Entry::Separator,
            c.on("Close Tab", "CloseTab"),
            c.item("Close Pane", "ClosePane", many_panes),
            c.on("Close Window", "CloseWindow"),
            Entry::Separator,
            c.on("Rename Tab", "RenameTab"),
            c.on("Reset Terminal", "ResetTerminal"),
            c.on("Show Launcher", "ShowLauncher"),
            c.item(
                "Open Link at Mouse Cursor",
                "OpenLinkAtMouse",
                st.link_under_pointer,
            ),
            Entry::Separator,
            sub("New Tab in Domain", domains),
            Entry::Separator,
            c.item("Detach Workspace", "DetachWorkspace", st.workspaces >= 2),
            attach,
        ],
    };
    let edit = MenuModel {
        title: "Edit".into(),
        entries: {
            let mut entries = vec![
                c.item("Copy", "Copy", st.has_selection),
                c.on("Paste", "Paste"),
                c.on("Paste Selection", "PastePrimary"),
                c.on("Select All", "SelectAll"),
                Entry::Separator,
                c.on("Find…", "Search"),
                c.on("Quick Select", "ActivateQuickSelect"),
                c.on("Copy Mode", "ActivateCopyMode"),
                Entry::Separator,
                c.on("Clear Scrollback", "ClearScrollback"),
            ];
            // Hidden while the triggers gate is off (SPEC §10.7, §10.11.9).
            if crate::triggers::TriggerGate::of_config(config).enabled {
                entries.push(Entry::Separator);
                entries.push(c.item(
                    "Create Trigger from Selection…",
                    "CreateTriggerFromSelection",
                    st.has_selection,
                ));
            }
            // Hidden while the recording gate is off (SPEC §10.7, §10.10.1).
            if crate::recording::gate::RecordingGate::of_config(config).enabled {
                entries.push(Entry::Separator);
                entries.push(c.on(
                    if st.recording {
                        "Stop Recording"
                    } else {
                        "Start Recording"
                    },
                    "ToggleRecording",
                ));
            }
            entries
        },
    };
    let view = MenuModel {
        title: "View".into(),
        entries: {
            let mut entries = vec![
                c.on("Command Palette", "ShowCommandPalette"),
                Entry::Separator,
                c.on("Increase Font Size", "IncreaseFontSize"),
                c.on("Decrease Font Size", "DecreaseFontSize"),
                c.on("Reset Font Size", "ResetFontSize"),
                Entry::Separator,
                c.on("Select Theme…", "SelectTheme"),
                Entry::Separator,
                c.on("Toggle Fullscreen", "ToggleFullscreen"),
                c.item("Toggle Pane Zoom", "TogglePaneZoom", many_panes),
                Entry::Separator,
                c.on("Workspace Switcher", "ShowWorkspaceSwitcher"),
            ];
            // Hidden while the recording gate is off (SPEC §10.7, §10.10.1).
            if crate::recording::gate::RecordingGate::of_config(config).enabled {
                entries.push(c.on("Recordings", "ToggleRecordingsPanel"));
            }
            // Hidden while the triggers gate is off (SPEC §10.7, §10.11.9).
            if crate::triggers::TriggerGate::of_config(config).enabled {
                entries.push(c.on("Triggers", "ToggleTriggersPanel"));
            }
            entries
        },
    };
    let go_to = (1..=9)
        .map(|n| {
            c.item(
                &format!("Tab {n}"),
                &format!("ActivateTab({n})"),
                n <= st.tabs,
            )
        })
        .collect();
    let dir_items = |verb: &str| -> Vec<Entry> {
        [
            ("Left", "left"),
            ("Up", "up"),
            ("Right", "right"),
            ("Down", "down"),
        ]
        .iter()
        .map(|(t, d)| c.item(t, &format!("{verb}({d})"), many_panes))
        .collect()
    };
    let window = MenuModel {
        title: "Window".into(),
        entries: vec![
            c.on("Minimize", "Minimize"),
            c.on("Zoom", "ZoomWindow"),
            Entry::Separator,
            c.on("Next Tab", "ActivateTabRelative(1)"),
            c.on("Previous Tab", "ActivateTabRelative(-1)"),
            sub("Go to Tab", go_to),
            Entry::Separator,
            sub("Focus Pane", dir_items("ActivatePaneDirection")),
            sub("Swap Pane", dir_items("SwapPane")),
            c.item("Rotate Panes", "RotatePanes", many_panes),
            Entry::Separator,
            c.item(
                "Next Workspace",
                "SwitchWorkspaceRelative(1)",
                st.workspaces >= 2,
            ),
            c.item(
                "Previous Workspace",
                "SwitchWorkspaceRelative(-1)",
                st.workspaces >= 2,
            ),
            Entry::Separator,
            c.on("Bring All to Front", "BringAllToFront"),
        ],
    };
    let help = MenuModel {
        title: "Help".into(),
        entries: vec![
            c.on("Documentation", "OpenDocs"),
            c.on("Release Notes", "OpenReleaseNotes"),
            c.on("Report an Issue", "ReportIssue"),
            c.on("Show Config Folder", "ShowConfigFolder"),
        ],
    };
    MenuBar {
        menus: vec![app, shell, edit, view, window, help],
    }
}

/// The action a menu row runs: a registry name, or `Name(arg)` as in the config.
pub fn build_action(key: &str) -> Option<Box<dyn Action>> {
    if let Some(def) = ACTIONS.iter().find(|a| a.name == key) {
        return Some((def.build)());
    }
    let (name, rest) = key.split_once('(')?;
    let arg = rest.strip_suffix(')')?;
    let arg = match arg.parse::<i64>() {
        Ok(n) => ActionArg::Int(n),
        Err(_) => ActionArg::Str(arg.to_string()),
    };
    actions::resolve_action(name, &Some(arg)).ok()
}

fn to_gpui(entries: &[Entry]) -> Vec<MenuItem> {
    entries
        .iter()
        .filter_map(|e| match e {
            Entry::Separator => Some(MenuItem::separator()),
            Entry::Services => Some(MenuItem::os_submenu("Services", SystemMenuType::Services)),
            Entry::Submenu { title, entries } => Some(MenuItem::submenu(Menu {
                name: title.clone().into(),
                items: to_gpui(entries),
            })),
            Entry::Item(i) => {
                let action = match (&i.action, i.enabled) {
                    (Some(k), true) => build_action(k)?,
                    // No view handles it, so the OS greys the row out.
                    _ => Box::new(MenuDisabled),
                };
                Some(MenuItem::Action {
                    name: i.title.clone().into(),
                    action,
                    os_action: None,
                })
            }
        })
        .collect()
}

pub fn to_menus(bar: &MenuBar) -> Vec<Menu> {
    bar.menus
        .iter()
        .map(|m| Menu {
            name: m.title.clone().into(),
            items: to_gpui(&m.entries),
        })
        .collect()
}

/// The last bar installed, so an unchanged one is not reinstalled.
#[derive(Default)]
struct Installed(Option<MenuBar>);
impl Global for Installed {}

/// Installs `bar` (macOS only) unless it equals the one already installed.
pub fn apply_menu_bar(bar: MenuBar, cx: &mut App) {
    if Platform::current() != Platform::Mac {
        return;
    }
    if cx.try_global::<Installed>().and_then(|i| i.0.as_ref()) == Some(&bar) {
        return;
    }
    cx.set_menus(to_menus(&bar));
    cx.set_global(Installed(Some(bar)));
}

/// Reads the live state from the host and the focused window.
pub fn collect_state(cx: &mut App) -> MenuState {
    let mut st = MenuState {
        workspaces: mux_host::workspace_count(cx),
        parked: mux_host::detached_names(cx),
        ..Default::default()
    };
    if let Some(w) = cx.active_window().and_then(|w| w.downcast::<AppWindow>()) {
        let _ = w.update(cx, |this, _, cx| {
            st.recording = this.focused_is_recording(cx);
            let (panes, tabs, sel, link) = this.menu_probe();
            st.panes = panes;
            st.tabs = tabs;
            st.has_selection = sel;
            st.link_under_pointer = link;
        });
    }
    st
}

/// The menu bar for the live state (what `ctl menu-bar list` prints).
pub fn current(cx: &mut App) -> MenuBar {
    let state = collect_state(cx);
    let s = cx.global::<Settings>();
    build_menu_bar(&state, &s.config, s.chips())
}

/// Recomputes and installs the menu bar.
pub fn refresh(cx: &mut App) {
    let bar = current(cx);
    apply_menu_bar(bar, cx);
}

/// Installs the bar and keeps it current: state changes (focus, selection, panes, workspaces)
/// have no common event, so a cheap poll compares the model and reinstalls only on change.
pub fn install(cx: &mut App) {
    if Platform::current() != Platform::Mac {
        return;
    }
    refresh(cx);
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        if cx.update(refresh).is_err() {
            break;
        }
    })
    .detach();
}

const DOCS_URL: &str = "https://microterm.dev/docs/";
const RELEASES_URL: &str = "https://github.com/0funct0ry/microterm/releases";
const ISSUES_URL: &str = "https://github.com/0funct0ry/microterm/issues/new";

use crate::components::ToastKind;
use gpui::Context;

impl AppWindow {
    /// `(panes in the active tab, tabs, has selection, link under the last pointer position)`.
    pub(crate) fn menu_probe(&mut self) -> (usize, usize, bool, bool) {
        let panes = self.tabs.get(self.active_tab).map_or(0, |t| t.panes.len());
        let sel = self.live().is_some_and(|s| s.term.has_selection());
        let link = self.link_under_pointer().is_some();
        (panes, self.tabs.len(), sel, link)
    }

    pub(crate) fn reset_terminal(&mut self, cx: &mut Context<Self>) {
        match self.live_mut() {
            Some(session) => {
                session.term.reset();
                session.refresh();
                cx.notify();
            }
            None => self.toast(ToastKind::Info, "No terminal to reset", cx),
        }
    }

    pub(crate) fn open_link_at_mouse(&mut self, cx: &mut Context<Self>) {
        match self.link_under_pointer() {
            Some(link) => self.open_link(&link.target, cx),
            None => self.toast(ToastKind::Info, "No link under the pointer", cx),
        }
    }

    pub(crate) fn show_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.about = true;
        self.open_overlay(OverlayKind::About, window, cx);
    }

    pub(crate) fn open_help(&mut self, url: &'static str, cx: &mut Context<Self>) {
        cx.open_url(url);
    }

    pub(crate) fn show_config_folder(&mut self, cx: &mut Context<Self>) {
        let path = cx.global::<Settings>().path.clone();
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(path);
        cx.reveal_path(&dir);
    }

    pub(crate) fn detach_workspace(&mut self, cx: &mut Context<Self>) {
        cx.defer(|cx| {
            if let Err(e) = mux_host::detach_active(cx) {
                eprintln!("microterm: {e}");
            }
            refresh(cx);
        });
    }

    pub(crate) fn attach_workspace(&mut self, name: String, cx: &mut Context<Self>) {
        cx.defer(move |cx| {
            if let Err(e) = mux_host::attach(cx, &name) {
                eprintln!("microterm: {e}");
            }
            refresh(cx);
        });
    }
}

pub(crate) fn help_urls() -> (&'static str, &'static str, &'static str) {
    (DOCS_URL, RELEASES_URL, ISSUES_URL)
}

/// App-level handlers for rows the OS or the whole application owns.
pub fn install_app_actions(cx: &mut App) {
    cx.on_action(|_: &actions::HideApp, cx| cx.hide());
    cx.on_action(|_: &actions::HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &actions::ShowAllApps, cx| cx.unhide_other_apps());
    cx.on_action(|_: &actions::BringAllToFront, cx| cx.activate(true));
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn bar(state: &MenuState, cfg: &Config) -> MenuBar {
        let mut s = Settings::defaults();
        s.config = std::sync::Arc::new(cfg.clone());
        s.rebuild_chips(Platform::Mac);
        build_menu_bar(state, cfg, s.chips())
    }

    fn row<'a>(rows: &'a [Row], title: &str) -> &'a Row {
        rows.iter()
            .find(|r| r.title == title)
            .unwrap_or_else(|| panic!("no row {title}"))
    }

    fn many() -> MenuState {
        MenuState {
            panes: 3,
            tabs: 3,
            has_selection: true,
            link_under_pointer: true,
            workspaces: 2,
            parked: vec!["infra".into()],
            recording: false,
        }
    }

    #[test]
    fn six_menus_in_order() {
        let b = bar(&many(), &Config::default());
        let titles: Vec<_> = b.menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(
            titles,
            ["microterm", "Shell", "Edit", "View", "Window", "Help"]
        );
    }

    #[test]
    fn golden_shell_menu() {
        let b = bar(&many(), &Config::default());
        let shell = b.menus[1].entries.iter().filter_map(|e| match e {
            Entry::Item(i) => Some(i.title.as_str()),
            Entry::Submenu { title, .. } => Some(title.as_str()),
            _ => None,
        });
        assert_eq!(
            shell.collect::<Vec<_>>(),
            [
                "New Tab",
                "New Window",
                "Split Right",
                "Split Down",
                "Split Left",
                "Split Up",
                "Close Tab",
                "Close Pane",
                "Close Window",
                "Rename Tab",
                "Reset Terminal",
                "Show Launcher",
                "Open Link at Mouse Cursor",
                "New Tab in Domain",
                "Detach Workspace",
                "Attach",
            ]
        );
    }

    #[test]
    fn every_item_maps_to_a_registered_action() {
        let mut cfg = Config::default();
        cfg.ssh_domains.push(mt_config::SshDomain {
            name: "github.com".into(),
            host: "github.com".into(),
            ..serde_json::from_str("{\"name\":\"x\",\"host\":\"x\"}").unwrap()
        });
        let b = bar(&many(), &cfg);
        // OS-provided rows without an action: only the Services submenu.
        fn walk(entries: &[Entry], n: &mut usize) {
            for e in entries {
                match e {
                    Entry::Item(i) => {
                        let key = i.action.as_deref().expect("items carry an action");
                        assert!(build_action(key).is_some(), "{key} does not resolve");
                        let base = key.split('(').next().unwrap();
                        assert!(
                            ACTIONS.iter().any(|a| a.name == key || a.name == base),
                            "{key} is not in the registry"
                        );
                        *n += 1;
                    }
                    Entry::Submenu { entries, .. } => walk(entries, n),
                    _ => {}
                }
            }
        }
        let mut n = 0;
        for m in &b.menus {
            walk(&m.entries, &mut n);
        }
        assert!(n > 60, "{n}");
    }

    #[test]
    fn recording_row_follows_the_gate_and_the_pane_state() {
        let off = bar(&many(), &Config::default()).rows();
        assert!(!off.iter().any(|r| r.title.ends_with("Recording")));
        let cfg = mt_config::parse_str("[recording]\nenabled = true\n")
            .unwrap()
            .config;
        let on = bar(&many(), &cfg).rows();
        let r = row(&on, "Start Recording");
        assert_eq!(r.action.as_deref(), Some("ToggleRecording"));
        let mut st = many();
        st.recording = true;
        let rec = bar(&st, &cfg).rows();
        assert!(rec.iter().any(|r| r.title == "Stop Recording"));
    }

    #[test]
    fn enable_disable_matrix() {
        let none = MenuState {
            panes: 1,
            tabs: 1,
            workspaces: 1,
            ..Default::default()
        };
        let rows = bar(&none, &Config::default()).rows();
        for t in [
            "Close Pane",
            "Rotate Panes",
            "Copy",
            "Detach Workspace",
            "Open Link at Mouse Cursor",
            "Attach",
            "Left",
            "Tab 2",
        ] {
            assert!(
                !rows.iter().filter(|r| r.title == t).any(|r| r.enabled),
                "{t}"
            );
        }
        let rows = bar(&many(), &Config::default()).rows();
        for t in [
            "Close Pane",
            "Rotate Panes",
            "Copy",
            "Detach Workspace",
            "Open Link at Mouse Cursor",
            "infra",
            "Tab 3",
        ] {
            assert!(row(&rows, t).enabled, "{t}");
        }
        assert!(!row(&rows, "Tab 4").enabled);
    }

    #[test]
    fn chips_follow_rebinds() {
        let mut cfg =
            mt_config::parse_str("[[keys]]\nkey = \"cmd+alt+d\"\naction = \"SplitHorizontal\"\n")
                .unwrap()
                .config;
        cfg.menu_bar.enabled = true;
        let rows = bar(&many(), &cfg).rows();
        assert_eq!(row(&rows, "Split Right").chip.as_deref(), Some("⌘⌥D"));
        let rows = bar(&many(), &Config::default()).rows();
        assert_eq!(row(&rows, "Split Right").chip.as_deref(), Some("⌘D"));
        assert_eq!(row(&rows, "New Tab").chip.as_deref(), Some("⌘T"));
    }

    #[test]
    fn domain_submenu_follows_config() {
        let mut cfg = Config::default();
        let rows = bar(&many(), &cfg).rows();
        assert!(rows.iter().all(|r| !r.title.starts_with("SSH: ")));
        cfg.ssh_domains.push(
            serde_json::from_str("{\"name\":\"github.com\",\"host\":\"github.com\"}").unwrap(),
        );
        let rows = bar(&many(), &cfg).rows();
        assert_eq!(
            row(&rows, "SSH: github.com").action.as_deref(),
            Some("SpawnTabInDomain(github.com)")
        );
    }

    #[test]
    fn disabled_config_leaves_only_the_app_menu() {
        let mut cfg = Config::default();
        cfg.menu_bar.enabled = false;
        let b = bar(&many(), &cfg);
        assert_eq!(b.menus.len(), 1);
        let rows = b.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action.as_deref(), Some("Quit"));
    }

    #[test]
    fn list_output_has_path_action_state_and_chip() {
        let t = bar(&many(), &Config::default()).text();
        assert!(t.contains("Shell > New Tab\tSpawnTab\tenabled\t⌘T"), "{t}");
        let j = bar(&many(), &Config::default()).json();
        assert_eq!(j["menus"][1], "Shell");
        assert!(j["items"].as_array().unwrap().len() > 60);
    }

    #[gpui::test]
    fn detach_attach_round_trip(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Settings::defaults());
            cx.set_global(crate::theme::Theme::one_dark());
            mux_host::ensure(cx);
            mux_host::show_workspace(cx, "default", true, None).unwrap();
            assert!(mux_host::detach_active(cx).is_err(), "needs two workspaces");
            mux_host::show_workspace(cx, "api", true, None).unwrap();
            assert!(mux_host::attach(cx, "default").is_err(), "not detached yet");
            mux_host::detach_active(cx).unwrap();
            assert_eq!(mux_host::host(cx).mux.active_workspace().name, "default");
            assert_eq!(mux_host::detached_names(cx), ["api"]);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            // Its shells stay parked, not closed.
            assert!(!mux_host::host(cx).parked.is_empty());
            mux_host::attach(cx, "api").unwrap();
            assert_eq!(mux_host::host(cx).mux.active_workspace().name, "api");
            assert!(mux_host::detached_names(cx).is_empty());
        });
    }
}
