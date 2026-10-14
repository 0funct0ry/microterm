//! The application window: all M1 state (tabs, workspaces, overlays, toasts) and its behaviour,
//! driven by mock data. Rendering lives in `view.rs`.

use std::time::Duration;

use gpui::{
    prelude::*, px, App, Bounds, ClipboardItem, Context, FocusHandle, KeyDownEvent, Pixels, Point,
    SharedString, Subscription, Window,
};

use crate::{
    actions::*,
    assets::IconName,
    components::{StatusKind, ToastKind},
    metrics::*,
    mock::{assign_labels, scan_matches, Line, MockKind, QsMatch},
    overlay_stack::{OverlayKind, OverlayStack},
    picker_state::{PickerInput, PickerOutcome, PickerState},
    platform::Platform,
    theme::Theme,
};

pub const QUICK_SELECT_ALPHABET: &str = "asdfghjkl";

pub struct MockTab {
    pub id: u64,
    pub kind: MockKind,
    pub title: String,
}

pub struct MockWorkspace {
    pub name: String,
    pub windows: usize,
    pub tabs: usize,
}

/// What confirming a picker row does.
#[derive(Clone, Debug)]
pub enum Target {
    Action(usize),
    Launch { title: &'static str, kind: MockKind },
    Workspace(usize),
    CreateWorkspace,
    Theme(usize),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub target: Target,
    pub detail: Option<String>,
    pub chip: Option<String>,
    pub icon: Option<IconName>,
    pub status: Option<StatusKind>,
}

impl Entry {
    fn new(target: Target) -> Self {
        Self {
            target,
            detail: None,
            chip: None,
            icon: None,
            status: None,
        }
    }
}

pub struct PickerData {
    pub kind: OverlayKind,
    pub state: PickerState<Entry>,
    pub placeholder: &'static str,
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub case_sensitive: bool,
    pub regex: bool,
    /// Index into the match list of the highlighted hit.
    pub current: usize,
}

#[derive(Default)]
pub struct CopyState {
    pub row: usize,
    pub col: usize,
}

#[derive(Default)]
pub struct QuickState {
    pub matches: Vec<(String, QsMatch)>,
    pub typed: String,
}

pub struct ToastItem {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
}

pub struct AppWindow {
    pub(crate) viewport_focus: FocusHandle,
    pub(crate) overlay_focus: FocusHandle,
    pub(crate) platform: Platform,
    pub(crate) tabs: Vec<MockTab>,
    pub(crate) active_tab: usize,
    next_tab_id: u64,
    pub(crate) workspaces: Vec<MockWorkspace>,
    pub(crate) active_workspace: usize,
    pub(crate) overlays: OverlayStack,
    pub(crate) picker: Option<PickerData>,
    pub(crate) search: SearchState,
    pub(crate) copy: CopyState,
    pub(crate) quick: QuickState,
    pub(crate) toasts: Vec<ToastItem>,
    next_toast_id: u64,
    pub(crate) config_error: Option<SharedString>,
    pub(crate) ssh_attempt: u32,
    ssh_epoch: u64,
    pub(crate) hud: Option<(usize, usize)>,
    hud_epoch: u64,
    pub(crate) scrollbar_visible: bool,
    scrollbar_epoch: u64,
    pub(crate) tab_menu: Option<(Point<Pixels>, u64)>,
    theme_restore: Option<Theme>,
    _subscriptions: Vec<Subscription>,
}

impl AppWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let viewport_focus = cx.focus_handle();
        let overlay_focus = cx.focus_handle();
        window.focus(&viewport_focus);
        let subscription = cx.observe_window_bounds(window, |this, window, cx| {
            this.on_resize(window, cx);
        });
        let mut tabs = Vec::new();
        for (i, kind) in [MockKind::Zsh, MockKind::Cargo, MockKind::Ssh]
            .into_iter()
            .enumerate()
        {
            tabs.push(MockTab {
                id: i as u64 + 1,
                kind,
                title: kind.title().to_string(),
            });
        }
        Self {
            viewport_focus,
            overlay_focus,
            platform: Platform::current(),
            tabs,
            active_tab: 0,
            next_tab_id: 4,
            workspaces: vec![
                MockWorkspace {
                    name: "payments".into(),
                    windows: 1,
                    tabs: 3,
                },
                MockWorkspace {
                    name: "infra".into(),
                    windows: 2,
                    tabs: 5,
                },
                MockWorkspace {
                    name: "default".into(),
                    windows: 1,
                    tabs: 1,
                },
            ],
            active_workspace: 0,
            overlays: OverlayStack::default(),
            picker: None,
            search: SearchState::default(),
            copy: CopyState::default(),
            quick: QuickState::default(),
            toasts: Vec::new(),
            next_toast_id: 0,
            config_error: None,
            ssh_attempt: 1,
            ssh_epoch: 0,
            hud: None,
            hud_epoch: 0,
            scrollbar_visible: false,
            scrollbar_epoch: 0,
            tab_menu: None,
            theme_restore: None,
            _subscriptions: vec![subscription],
        }
    }

    // ---------------------------------------------------------------- screen helpers

    pub(crate) fn active_kind(&self) -> Option<MockKind> {
        self.tabs.get(self.active_tab).map(|t| t.kind)
    }

    pub(crate) fn screen(&self) -> Vec<Line> {
        self.active_kind().map(MockKind::lines).unwrap_or_default()
    }

    /// `(row, col, len, is_current)` of every search hit on the active screen.
    pub(crate) fn search_hits(&self) -> Vec<(usize, usize, usize)> {
        if self.search.query.is_empty() {
            return Vec::new();
        }
        let needle: Vec<char> = fold(&self.search.query, self.search.case_sensitive);
        let mut hits = Vec::new();
        for (row, line) in self.screen().iter().enumerate() {
            let hay: Vec<char> = fold(&line.text(), self.search.case_sensitive);
            let mut i = 0;
            while i + needle.len() <= hay.len() {
                if hay[i..i + needle.len()] == needle[..] {
                    hits.push((row, i, needle.len()));
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }
        hits
    }

    // ---------------------------------------------------------------- overlays & focus

    fn sync_focus(&self, window: &mut Window) {
        match self.overlays.focus_target() {
            crate::overlay_stack::FocusTarget::Viewport => window.focus(&self.viewport_focus),
            crate::overlay_stack::FocusTarget::Overlay(_) => window.focus(&self.overlay_focus),
        }
    }

    pub(crate) fn open_overlay(
        &mut self,
        kind: OverlayKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_menu = None;
        self.prepare(kind, cx);
        self.overlays.open(kind);
        self.sync_focus(window);
        cx.notify();
    }

    fn toggle_overlay(&mut self, kind: OverlayKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.top() == Some(kind) {
            self.dismiss_top(window, cx);
        } else {
            self.open_overlay(kind, window, cx);
        }
    }

    /// Sets up per-overlay state when it opens.
    fn prepare(&mut self, kind: OverlayKind, cx: &mut Context<Self>) {
        // A picker replacing another picker must still clean up the old one.
        if let Some(old) = self.picker.as_ref().map(|p| p.kind) {
            if old != kind {
                self.teardown(old, true, cx);
            }
        }
        match kind {
            OverlayKind::CommandPalette
            | OverlayKind::Launcher
            | OverlayKind::WorkspaceSwitcher
            | OverlayKind::ThemeSelector => self.picker = Some(self.build_picker(kind, cx)),
            OverlayKind::QuickSelect => {
                let found = scan_matches(&self.screen());
                let labels = assign_labels(found.len(), QUICK_SELECT_ALPHABET);
                self.quick = QuickState {
                    matches: labels.into_iter().zip(found).collect(),
                    typed: String::new(),
                };
            }
            OverlayKind::CopyMode => {
                let screen = self.screen();
                let row = screen.len().saturating_sub(1);
                self.copy = CopyState { row, col: 0 };
            }
            OverlayKind::Search => self.search.current = 0,
            OverlayKind::SshReconnect => self.start_ssh_attempts(cx),
        }
    }

    /// Esc: closes only the top overlay and hands focus back.
    pub(crate) fn dismiss_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_menu.take().is_some() {
            cx.notify();
            return;
        }
        if let Some(kind) = self.overlays.dismiss_top() {
            self.teardown(kind, true, cx);
        }
        self.sync_focus(window);
        cx.notify();
    }

    fn close_overlay(
        &mut self,
        kind: OverlayKind,
        keep_theme: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.close(kind);
        self.teardown(kind, !keep_theme, cx);
        self.sync_focus(window);
        cx.notify();
    }

    fn teardown(&mut self, kind: OverlayKind, revert_theme: bool, cx: &mut Context<Self>) {
        match kind {
            OverlayKind::ThemeSelector => {
                if revert_theme {
                    if let Some(theme) = self.theme_restore.take() {
                        cx.set_global(theme);
                    }
                }
                self.theme_restore = None;
                self.picker = None;
            }
            OverlayKind::CommandPalette
            | OverlayKind::Launcher
            | OverlayKind::WorkspaceSwitcher => {
                self.picker = None;
            }
            OverlayKind::QuickSelect => self.quick = QuickState::default(),
            OverlayKind::SshReconnect => self.ssh_epoch += 1,
            OverlayKind::CopyMode | OverlayKind::Search => {}
        }
    }

    // ---------------------------------------------------------------- pickers

    fn build_picker(&mut self, kind: OverlayKind, cx: &mut Context<Self>) -> PickerData {
        let platform = self.platform;
        match kind {
            OverlayKind::CommandPalette => {
                let items = ACTIONS
                    .iter()
                    .enumerate()
                    .map(|(i, def)| {
                        let mut e = Entry::new(Target::Action(i));
                        e.detail = Some(def.description.to_string());
                        e.chip = def.chip(platform);
                        (def.label.to_string(), e)
                    })
                    .collect();
                PickerData {
                    kind,
                    state: PickerState::new(items),
                    placeholder: "Type a command…",
                }
            }
            OverlayKind::Launcher => {
                let mut items: Vec<(String, Entry)> = Vec::new();
                let mut push = |label: &str, mut e: Entry, detail: &str| {
                    e.detail = Some(detail.to_string());
                    items.push((label.to_string(), e));
                };
                push(
                    "htop",
                    Entry {
                        icon: Some(IconName::Terminal),
                        ..Entry::new(Target::Launch {
                            title: "htop",
                            kind: MockKind::Cargo,
                        })
                    },
                    "launch profile",
                );
                push(
                    "cargo watch",
                    Entry {
                        icon: Some(IconName::Terminal),
                        ..Entry::new(Target::Launch {
                            title: "cargo watch",
                            kind: MockKind::Cargo,
                        })
                    },
                    "launch profile · ~/src/payments-api",
                );
                push(
                    "ssh: prod",
                    Entry {
                        status: Some(StatusKind::Connected),
                        ..Entry::new(Target::Launch {
                            title: "ssh: prod",
                            kind: MockKind::Ssh,
                        })
                    },
                    "SSH domain · prod.example.com",
                );
                push(
                    "ssh: staging",
                    Entry {
                        status: Some(StatusKind::Idle),
                        ..Entry::new(Target::Launch {
                            title: "ssh: staging",
                            kind: MockKind::Ssh,
                        })
                    },
                    "SSH domain · staging.example.com",
                );
                push(
                    "New tab in local",
                    Entry {
                        icon: Some(IconName::Plus),
                        ..Entry::new(Target::Launch {
                            title: "zsh",
                            kind: MockKind::Zsh,
                        })
                    },
                    "domain",
                );
                for (i, w) in self.workspaces.iter().enumerate() {
                    items.push((
                        format!("Workspace: {}", w.name),
                        Entry {
                            icon: Some(IconName::Layers),
                            detail: Some(format!("{} windows · {} tabs", w.windows, w.tabs)),
                            ..Entry::new(Target::Workspace(i))
                        },
                    ));
                }
                PickerData {
                    kind,
                    state: PickerState::new(items),
                    placeholder: "Launch a profile, domain or workspace…",
                }
            }
            OverlayKind::WorkspaceSwitcher => {
                let mut items: Vec<(String, Entry)> = self
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(i, w)| {
                        let active = i == self.active_workspace;
                        let mut e = Entry::new(Target::Workspace(i));
                        e.icon = Some(if active {
                            IconName::Check
                        } else {
                            IconName::Layers
                        });
                        e.detail = Some(format!(
                            "{} window{} · {} tab{}",
                            w.windows,
                            plural(w.windows),
                            w.tabs,
                            plural(w.tabs)
                        ));
                        (w.name.clone(), e)
                    })
                    .collect();
                let mut create = Entry::new(Target::CreateWorkspace);
                create.icon = Some(IconName::Plus);
                items.push(("Create workspace…".to_string(), create));
                let mut state = PickerState::new(items);
                state.select(self.active_workspace);
                PickerData {
                    kind,
                    state,
                    placeholder: "Switch workspace…",
                }
            }
            OverlayKind::ThemeSelector => {
                let current = *cx.global::<Theme>();
                self.theme_restore = Some(current);
                let items: Vec<(String, Entry)> = Theme::all()
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let mut e = Entry::new(Target::Theme(i));
                        e.icon = Some(if t.name == current.name {
                            IconName::Check
                        } else {
                            IconName::Palette
                        });
                        e.detail = Some(if t.is_dark { "dark" } else { "light" }.to_string());
                        (t.name.to_string(), e)
                    })
                    .collect();
                let mut state = PickerState::new(items);
                if let Some(i) = Theme::all().iter().position(|t| t.name == current.name) {
                    state.select(i);
                }
                PickerData {
                    kind,
                    state,
                    placeholder: "Select a theme…",
                }
            }
            _ => unreachable!("not a picker overlay"),
        }
    }

    fn preview_selected_theme(&self, cx: &mut Context<Self>) {
        let Some(picker) = &self.picker else { return };
        if let Some(Entry {
            target: Target::Theme(i),
            ..
        }) = picker.state.selected_item()
        {
            cx.set_global(Theme::all()[*i]);
        }
    }

    pub(crate) fn confirm_visible(
        &mut self,
        visible_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(picker) = &mut self.picker {
            let offset = picker.state.visible().0;
            picker.state.select(offset + visible_index);
        }
        self.confirm_picker(false, window, cx);
    }

    fn picker_input(&mut self, input: PickerInput, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.state.handle(input) {
            PickerOutcome::None => {}
            PickerOutcome::Changed => {
                if picker.kind == OverlayKind::ThemeSelector {
                    self.preview_selected_theme(cx);
                }
                cx.notify();
            }
            PickerOutcome::Confirmed(_) => self.confirm_picker(false, window, cx),
            PickerOutcome::Cancelled => self.dismiss_top(window, cx),
        }
    }

    fn confirm_picker(&mut self, new_window: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.picker else { return };
        let kind = picker.kind;
        let Some(entry) = picker.state.selected_item().cloned() else {
            return;
        };
        // Closing first returns focus to the viewport before the entry runs.
        self.close_overlay(kind, true, window, cx);
        match entry.target {
            Target::Action(i) => {
                let action = (ACTIONS[i].build)();
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            Target::Launch { title, kind } => {
                if new_window {
                    self.toast(
                        ToastKind::Info,
                        "Opening in a new window arrives with M5",
                        cx,
                    );
                }
                self.add_tab(kind, title, cx);
            }
            Target::Workspace(i) => self.switch_workspace(i, cx),
            Target::CreateWorkspace => {
                let name = format!("workspace-{}", self.workspaces.len() + 1);
                self.workspaces.push(MockWorkspace {
                    name: name.clone(),
                    windows: 1,
                    tabs: 1,
                });
                self.switch_workspace(self.workspaces.len() - 1, cx);
            }
            Target::Theme(i) => {
                let theme = Theme::all()[i];
                cx.set_global(theme);
                self.toast(ToastKind::Success, format!("Theme: {}", theme.name), cx);
            }
        }
    }

    // ---------------------------------------------------------------- tabs & workspaces

    pub(crate) fn add_tab(&mut self, kind: MockKind, title: &str, cx: &mut Context<Self>) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(MockTab {
            id,
            kind,
            title: title.to_string(),
        });
        self.active_tab = self.tabs.len() - 1;
        self.workspaces[self.active_workspace].tabs += 1;
        cx.notify();
    }

    pub(crate) fn close_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        self.tabs.remove(index);
        let ws = &mut self.workspaces[self.active_workspace];
        ws.tabs = ws.tabs.saturating_sub(1);
        if index < self.active_tab || self.active_tab >= self.tabs.len() {
            self.active_tab = self.active_tab.saturating_sub(1);
        }
        cx.notify();
    }

    pub(crate) fn activate_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() && index != self.active_tab {
            self.active_tab = index;
            cx.notify();
        }
    }

    fn switch_workspace(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active_workspace = index;
        let name = self.workspaces[index].name.clone();
        self.toast(ToastKind::Info, format!("Workspace: {name}"), cx);
    }

    // ---------------------------------------------------------------- toasts, HUD, scrollbar

    pub(crate) fn toast(
        &mut self,
        kind: ToastKind,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(ToastItem {
            id,
            kind,
            text: text.into(),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            this.update(cx, |this, cx| {
                this.toasts.retain(|t| t.id != id);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn not_yet(&mut self, what: &str, milestone: &str, cx: &mut Context<Self>) {
        self.toast(
            ToastKind::Info,
            format!("{what} arrives in {milestone}"),
            cx,
        );
    }

    fn on_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let size = window.viewport_size();
        let cols = ((f32::from(size.width) - 2.0 * VIEWPORT_PADDING_X) / CELL_WIDTH)
            .floor()
            .max(1.0) as usize;
        let chrome = TITLE_BAR_HEIGHT + TAB_BAR_HEIGHT + 2.0 * VIEWPORT_PADDING_Y;
        let rows = ((f32::from(size.height) - chrome) / LINE_HEIGHT)
            .floor()
            .max(1.0) as usize;
        self.hud = Some((cols, rows));
        self.hud_epoch += 1;
        let epoch = self.hud_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            this.update(cx, |this, cx| {
                if this.hud_epoch == epoch {
                    this.hud = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn on_scroll(&mut self, cx: &mut Context<Self>) {
        self.scrollbar_visible = true;
        self.scrollbar_epoch += 1;
        let epoch = self.scrollbar_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            this.update(cx, |this, cx| {
                if this.scrollbar_epoch == epoch {
                    this.scrollbar_visible = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn start_ssh_attempts(&mut self, cx: &mut Context<Self>) {
        self.ssh_epoch += 1;
        self.ssh_attempt = 1;
        let epoch = self.ssh_epoch;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let keep_going = this
                .update(cx, |this, cx| {
                    if this.ssh_epoch != epoch {
                        return false;
                    }
                    this.ssh_attempt += 1;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !keep_going {
                break;
            }
        })
        .detach();
    }

    pub(crate) fn reconnect_now(&mut self, cx: &mut Context<Self>) {
        let next = self.ssh_attempt + 1;
        self.start_ssh_attempts(cx);
        self.ssh_attempt = next;
        self.toast(ToastKind::Warning, "Reconnecting to prod…", cx);
    }

    pub(crate) fn close_ssh_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(OverlayKind::SshReconnect, true, window, cx);
        if let Some(id) = self
            .tabs
            .iter()
            .find(|t| t.kind == MockKind::Ssh)
            .map(|t| t.id)
        {
            self.close_tab(id, cx);
        }
    }

    // ---------------------------------------------------------------- keyboard

    pub(crate) fn on_overlay_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.overlays.top() {
            Some(
                OverlayKind::CommandPalette
                | OverlayKind::Launcher
                | OverlayKind::WorkspaceSwitcher
                | OverlayKind::ThemeSelector,
            ) => {
                if ev.keystroke.key == "backspace" {
                    self.picker_input(PickerInput::Backspace, window, cx);
                } else if let Some(text) = typed_text(ev, self.platform) {
                    for c in text.chars() {
                        self.picker_input(PickerInput::Char(c), window, cx);
                    }
                }
            }
            Some(OverlayKind::Search) => self.search_key(ev, cx),
            Some(OverlayKind::QuickSelect) => self.quick_key(ev, window, cx),
            Some(OverlayKind::CopyMode) => self.copy_key(ev, cx),
            Some(OverlayKind::SshReconnect) => {
                if ev.keystroke.key == "enter" {
                    self.reconnect_now(cx);
                }
            }
            None => {}
        }
    }

    fn search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let hits = self.search_hits().len();
        if k.key == "enter" {
            if hits > 0 {
                self.search.current = if k.modifiers.shift {
                    (self.search.current + hits - 1) % hits
                } else {
                    (self.search.current + 1) % hits
                };
            }
        } else if k.key == "backspace" {
            self.search.query.pop();
            self.search.current = 0;
        } else if k.modifiers.alt && k.key == "c" {
            self.search.case_sensitive = !self.search.case_sensitive;
            self.search.current = 0;
        } else if k.modifiers.alt && k.key == "r" {
            self.search.regex = !self.search.regex;
        } else if let Some(text) = typed_text(ev, self.platform) {
            self.search.query.push_str(text);
            self.search.current = 0;
        } else {
            return;
        }
        cx.notify();
    }

    pub(crate) fn step_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let hits = self.search_hits().len();
        if hits > 0 {
            self.search.current = if forward {
                (self.search.current + 1) % hits
            } else {
                (self.search.current + hits - 1) % hits
            };
            cx.notify();
        }
    }

    pub(crate) fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        self.search.case_sensitive = !self.search.case_sensitive;
        self.search.current = 0;
        cx.notify();
    }

    pub(crate) fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        self.search.regex = !self.search.regex;
        cx.notify();
    }

    fn quick_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = typed_text(ev, self.platform) else {
            return;
        };
        let Some(ch) = text.chars().next() else {
            return;
        };
        let mut typed = self.quick.typed.clone();
        typed.push(ch.to_ascii_lowercase());
        let upper = ch.is_ascii_uppercase();
        if let Some((_, hit)) = self.quick.matches.iter().find(|(label, _)| *label == typed) {
            let text = hit.text.clone();
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            self.close_overlay(OverlayKind::QuickSelect, true, window, cx);
            let verb = if upper { "Copied and pasted" } else { "Copied" };
            self.toast(ToastKind::Success, format!("{verb} {text}"), cx);
        } else if self
            .quick
            .matches
            .iter()
            .any(|(label, _)| label.starts_with(&typed))
        {
            self.quick.typed = typed;
            cx.notify();
        }
    }

    fn copy_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let screen = self.screen();
        if screen.is_empty() {
            return;
        }
        let last_row = screen.len() - 1;
        let width = |row: usize| screen[row].cols().max(1);
        let k = &ev.keystroke;
        let key = if k.key == "g" && k.modifiers.shift {
            "G"
        } else {
            k.key.as_str()
        };
        let c = &mut self.copy;
        match key {
            "h" | "left" => c.col = c.col.saturating_sub(1),
            "l" | "right" => c.col = (c.col + 1).min(width(c.row) - 1),
            "k" | "up" => c.row = c.row.saturating_sub(1),
            "j" | "down" => c.row = (c.row + 1).min(last_row),
            "0" => c.col = 0,
            "$" => c.col = width(c.row) - 1,
            "g" => c.row = 0,
            "G" => c.row = last_row,
            _ => return,
        }
        c.col = c.col.min(width(c.row) - 1);
        cx.notify();
    }

    // ---------------------------------------------------------------- action handlers

    pub(crate) fn on_action_spawn_tab(&mut self, cx: &mut Context<Self>) {
        self.add_tab(MockKind::Zsh, "zsh", cx);
    }

    pub(crate) fn on_action_close_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.get(self.active_tab).map(|t| t.id) {
            self.close_tab(id, cx);
        }
    }

    pub(crate) fn on_action_tab_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            let next = (self.active_tab as isize + delta).rem_euclid(n) as usize;
            self.activate_tab(next, cx);
        }
    }

    pub(crate) fn on_action_workspace_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.workspaces.len() as isize;
        let next = (self.active_workspace as isize + delta).rem_euclid(n) as usize;
        self.switch_workspace(next, cx);
    }

    pub(crate) fn on_stub(&mut self, what: &str, milestone: &str, cx: &mut Context<Self>) {
        self.not_yet(what, milestone, cx);
    }

    pub(crate) fn on_picker_confirm(
        &mut self,
        new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.picker.is_some() {
            self.confirm_picker(new_window, window, cx);
        }
    }

    pub(crate) fn on_picker_move(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_input(
            if up {
                PickerInput::Up
            } else {
                PickerInput::Down
            },
            window,
            cx,
        );
    }

    pub(crate) fn toggle(
        &mut self,
        kind: OverlayKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_overlay(kind, window, cx);
    }

    pub(crate) fn show_config_error(&mut self, cx: &mut Context<Self>) {
        self.config_error = Some(
            "microterm.toml:12:9 — unknown key `fontt` (did you mean `font`?). Keeping the previous config.".into(),
        );
        cx.notify();
    }
}

/// Printable text of a key press, ignoring shortcuts.
pub(crate) fn typed_text(ev: &KeyDownEvent, platform: Platform) -> Option<&str> {
    let m = &ev.keystroke.modifiers;
    if m.control || m.platform || m.function || (m.alt && platform == Platform::Linux) {
        return None;
    }
    ev.keystroke
        .key_char
        .as_deref()
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
}

fn fold(s: &str, case_sensitive: bool) -> Vec<char> {
    if case_sensitive {
        s.chars().collect()
    } else {
        s.chars().flat_map(char::to_lowercase).collect()
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

pub fn window_options(cx: &App) -> gpui::WindowOptions {
    let bounds = Bounds::centered(None, gpui::size(px(1000.), px(640.)), cx);
    let mac = Platform::current() == Platform::Mac;
    gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("microterm".into()),
            appears_transparent: true,
            traffic_light_position: mac
                .then(|| gpui::point(px(TRAFFIC_LIGHT_INSET), px(TRAFFIC_LIGHT_INSET))),
        }),
        window_min_size: Some(gpui::size(px(480.), px(300.))),
        app_id: Some("microterm".into()),
        window_decorations: Some(gpui::WindowDecorations::Client),
        ..Default::default()
    }
}
