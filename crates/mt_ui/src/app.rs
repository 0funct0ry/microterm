//! The application window: all M1 state (tabs, workspaces, overlays, toasts) and its behaviour,
//! driven by mock data. Rendering lives in `view.rs`.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    prelude::*, px, App, Bounds, ClipboardItem, Context, FocusHandle, KeyDownEvent, KeyUpEvent,
    Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Subscription, Window,
};
use mt_term::{KeyEventKind, ScrollCmd, TermEvent};

use crate::{
    actions::*,
    assets::IconName,
    components::{StatusKind, ToastKind},
    metrics::*,
    mock::{assign_labels, scan_matches, Line, MockKind, QsMatch},
    overlay_stack::{OverlayKind, OverlayStack},
    picker_state::{PickerInput, PickerOutcome, PickerState},
    platform::Platform,
    session::{self, TerminalSession},
    settings::{self, Banner, Settings},
    terminal_view::{CellMetrics, TermFont},
    theme::Theme,
};

pub const QUICK_SELECT_ALPHABET: &str = "asdfghjkl";

pub struct MockTab {
    pub id: u64,
    pub kind: MockKind,
    pub title: String,
    /// A live shell replaces the mock screen when present.
    pub session: Option<TerminalSession>,
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
    /// Measured terminal cell size.
    pub(crate) cell: CellMetrics,
    /// Cursor blink phase (true = drawn).
    pub(crate) cursor_phase: bool,
    pub(crate) bell_flash: bool,
    bell_epoch: u64,
    /// Text waiting on the multi-line paste confirmation.
    pub(crate) paste_confirm: Option<String>,
    /// The config this window renders with; replaced on every reload.
    pub(crate) cfg: Arc<mt_config::Config>,
    pub(crate) font: TermFont,
    pub(crate) font_generation: u64,
    pub(crate) config_banner_kind: ToastKind,
    /// False until the first paint has sized the PTY, so start-up shows no resize HUD.
    pub(crate) hud_armed: Rc<Cell<bool>>,
    blink_started: bool,
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
                session: None,
            });
        }
        if !cx.has_global::<Settings>() {
            cx.set_global(Settings::defaults());
        }
        let (cfg, banner, font_generation) = {
            let s = cx.global::<Settings>();
            (s.config.clone(), s.banner.clone(), s.font_generation)
        };
        if !cx.has_global::<Theme>() {
            let theme = cx.global::<Settings>().theme(true);
            cx.set_global(theme);
        }
        let font = TermFont::from_config(&cfg.font);
        let cell = CellMetrics::measure(window, &font, cfg.font.size, cfg.font.line_height);
        let appearance = window.observe_window_appearance(|_, cx| settings::apply_theme(cx));
        let mut this = Self {
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
            cell,
            cursor_phase: true,
            bell_flash: false,
            bell_epoch: 0,
            paste_confirm: None,
            cfg,
            font,
            font_generation,
            config_banner_kind: ToastKind::Error,
            hud_armed: Rc::new(Cell::new(false)),
            blink_started: false,
            theme_restore: None,
            _subscriptions: vec![subscription, appearance],
        };
        if let Some(b) = banner {
            this.config_banner_kind = b.kind;
            this.config_error = Some(b.text.into());
        }
        // Unit tests build windows by the dozen; they opt in to a shell explicitly.
        if !cfg!(test) {
            let opts = this.spawn_options(cx);
            if let Err(e) = this.attach_terminal(0, opts, window, cx) {
                this.toast(
                    ToastKind::Error,
                    format!("Could not start the shell: {e}"),
                    cx,
                );
            }
        }
        this
    }

    // ---------------------------------------------------------------- live terminal

    /// Replaces tab `index`'s mock screen with a real shell.
    pub fn attach_terminal(
        &mut self,
        index: usize,
        mut opts: mt_term::SpawnOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> std::io::Result<()> {
        opts.cols = session::DEFAULT_COLS;
        opts.rows = session::DEFAULT_ROWS;
        opts.cell_width = self.cell.width as u16;
        opts.cell_height = self.cell.height as u16;
        opts.palette = palette_of(cx.global::<Theme>());
        let (tx, rx) = async_channel::bounded::<()>(1);
        let session = TerminalSession::spawn(opts, tx)?;
        let Some(tab) = self.tabs.get_mut(index) else {
            return Ok(());
        };
        tab.title = session.default_title.clone();
        tab.session = Some(session);
        let tab_id = tab.id;
        cx.spawn_in(window, async move |this, cx| {
            while rx.recv().await.is_ok() {
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        this.pump_terminal(tab_id, window, cx)
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
        if !self.blink_started {
            self.blink_started = true;
            cx.spawn_in(window, async move |this, cx| loop {
                let interval = this
                    .read_with(cx, |this, _| this.cfg.cursor.blink_interval_ms)
                    .unwrap_or(530);
                cx.background_executor()
                    .timer(Duration::from_millis(u64::from(interval.max(50))))
                    .await;
                if this.update(cx, |this, cx| this.blink(cx)).is_err() {
                    break;
                }
            })
            .detach();
        }
        Ok(())
    }

    pub(crate) fn live(&self) -> Option<&TerminalSession> {
        self.tabs.get(self.active_tab)?.session.as_ref()
    }

    fn live_mut(&mut self) -> Option<&mut TerminalSession> {
        self.tabs.get_mut(self.active_tab)?.session.as_mut()
    }

    fn blink(&mut self, cx: &mut Context<Self>) {
        if self.live().is_some_and(|s| s.snapshot.cursor.blinking) {
            self.cursor_phase = !self.cursor_phase;
            cx.notify();
        }
    }

    /// Drains terminal events for a tab and refreshes its snapshot.
    fn pump_terminal(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return;
        };
        let active = index == self.active_tab;
        let tab = &mut self.tabs[index];
        let Some(session) = tab.session.as_mut() else {
            return;
        };
        let mut bell = false;
        let mut exit = None;
        let mut new_title = None;
        for ev in session.term.drain_events() {
            match ev {
                TermEvent::Wakeup | TermEvent::CursorBlinkingChange => {}
                TermEvent::Title(t) => new_title = Some(t),
                TermEvent::ResetTitle => new_title = Some(session.default_title.clone()),
                TermEvent::Bell => bell = true,
                TermEvent::ClipboardStore(kind, text) => match kind {
                    mt_term::ClipboardKind::Clipboard => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text))
                    }
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    mt_term::ClipboardKind::Primary => {
                        cx.write_to_primary(ClipboardItem::new_string(text))
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                    mt_term::ClipboardKind::Primary => {}
                },
                TermEvent::ClipboardLoad(_, format) => {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|i| i.text())
                        .unwrap_or_default();
                    session.term.write(format(&text).into_bytes());
                }
                TermEvent::ChildExit(code) => exit = Some(code),
            }
        }
        session.refresh();
        if let Some(title) = new_title {
            if active {
                window.set_window_title(&title);
            }
            tab.title = title;
        }
        self.cursor_phase = true;
        if bell {
            self.flash_bell(cx);
        }
        if let Some(code) = exit {
            if let Some(c) = code.filter(|c| *c != 0) {
                self.toast(
                    ToastKind::Warning,
                    format!("Shell exited with status {c}"),
                    cx,
                );
            }
            self.close_tab(tab_id, cx);
        }
        cx.notify();
    }

    fn flash_bell(&mut self, cx: &mut Context<Self>) {
        match self.cfg.bell.mode {
            mt_config::BellMode::None => return,
            mt_config::BellMode::Audible => {
                audible_bell();
                return;
            }
            mt_config::BellMode::Visual => {}
        }
        self.bell_flash = true;
        self.bell_epoch += 1;
        let epoch = self.bell_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(140))
                .await;
            this.update(cx, |this, cx| {
                if this.bell_epoch == epoch {
                    this.bell_flash = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn on_viewport_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        self.send_key(&ev.keystroke, ev.is_held, KeyEventKind::Press, cx);
    }

    pub(crate) fn on_viewport_key_up(&mut self, ev: &KeyUpEvent, cx: &mut Context<Self>) {
        self.send_key(&ev.keystroke, false, KeyEventKind::Release, cx);
    }

    fn send_key(
        &mut self,
        ks: &gpui::Keystroke,
        held: bool,
        kind: KeyEventKind,
        cx: &mut Context<Self>,
    ) {
        if !self.overlays.is_empty() {
            return;
        }
        let Some(session) = self.live_mut() else {
            return;
        };
        let Some(input) = session::key_input(ks, held, kind) else {
            return;
        };
        if kind == KeyEventKind::Release && !session.snapshot.modes.kitty.report_events {
            return;
        }
        session.term.send_key(&input);
        if kind != KeyEventKind::Release && session.snapshot.display_offset > 0 {
            // Typing returns to the live screen.
            session.term.scroll_display(ScrollCmd::Bottom);
            session.refresh();
        }
        self.cursor_phase = true;
        cx.notify();
    }

    pub(crate) fn on_scroll_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let cell_height = self.cell.height;
        let multiplier = self.cfg.mouse.scroll_multiplier;
        let Some(session) = self.live_mut() else {
            self.on_scroll(cx);
            return;
        };
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y * multiplier,
            ScrollDelta::Pixels(p) => f32::from(p.y) / cell_height,
        };
        session.scroll_remainder += lines;
        let n = session.scroll_remainder.trunc();
        session.scroll_remainder -= n;
        let n = n as i32;
        if n == 0 {
            return;
        }
        let modes = session.snapshot.modes;
        if modes.alt_screen {
            // Alternate scroll: the wheel becomes arrow keys (DECSET 1007), unless the app
            // asked for mouse reports, which arrive with M7.
            if modes.alternate_scroll && !modes.mouse_reporting {
                let key = if n > 0 {
                    mt_term::Key::Up
                } else {
                    mt_term::Key::Down
                };
                let input = mt_term::KeyInput::new(key, mt_term::Mods::NONE);
                for _ in 0..n.unsigned_abs() {
                    session.term.send_key(&input);
                }
            }
            return;
        }
        session.term.scroll_display(ScrollCmd::Lines(n));
        session.refresh();
        self.on_scroll(cx);
    }

    pub(crate) fn scroll_terminal(&mut self, cmd: ScrollCmd, cx: &mut Context<Self>) {
        if let Some(session) = self.live_mut() {
            session.term.scroll_display(cmd);
            session.refresh();
            self.on_scroll(cx);
        }
    }

    pub(crate) fn clear_scrollback(&mut self, cx: &mut Context<Self>) {
        match self.live_mut() {
            Some(session) => {
                session.term.clear_scrollback();
                session.refresh();
                cx.notify();
            }
            None => self.toast(ToastKind::Info, "No terminal to clear", cx),
        }
    }

    /// Paste action: clipboard text goes to the shell, after confirmation if it is multi-line
    /// and the program has not enabled bracketed paste (SPEC §6.3).
    pub(crate) fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) else {
            return;
        };
        self.paste_text(text, window, cx);
    }

    pub(crate) fn paste_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let confirm = self.cfg.paste.confirm_multiline;
        let Some(session) = self.live() else {
            self.toast(ToastKind::Info, "Paste needs a live terminal tab", cx);
            return;
        };
        let bracketed = session.term.modes().bracketed_paste;
        if mt_term::paste_needs_confirmation(&text, bracketed, confirm) {
            self.paste_confirm = Some(text);
            self.open_overlay(OverlayKind::PasteConfirm, window, cx);
        } else {
            session.term.paste(&text);
        }
    }

    pub(crate) fn confirm_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.paste_confirm.take();
        self.close_overlay(OverlayKind::PasteConfirm, true, window, cx);
        if let (Some(text), Some(session)) = (text, self.live()) {
            session.term.paste(&text);
        }
    }

    pub(crate) fn show_hud(&mut self, cols: usize, rows: usize, cx: &mut Context<Self>) {
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

    // ---------------------------------------------------------------- screen helpers

    pub(crate) fn active_kind(&self) -> Option<MockKind> {
        self.tabs.get(self.active_tab).map(|t| t.kind)
    }

    pub(crate) fn screen(&self) -> Vec<Line> {
        if let Some(session) = self.live() {
            let snap = &session.snapshot;
            return (0..snap.rows)
                .map(|r| Line::new().plain(&snap.row_text(r)))
                .collect();
        }
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
            OverlayKind::PasteConfirm => {}
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
                        self.sync_palette(cx);
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
            OverlayKind::PasteConfirm => self.paste_confirm = None,
            OverlayKind::CopyMode | OverlayKind::Search => {}
        }
    }

    // ---------------------------------------------------------------- pickers

    fn build_picker(&mut self, kind: OverlayKind, cx: &mut Context<Self>) -> PickerData {
        match kind {
            OverlayKind::CommandPalette => {
                let items = ACTIONS
                    .iter()
                    .enumerate()
                    .map(|(i, def)| {
                        let mut e = Entry::new(Target::Action(i));
                        e.detail = Some(def.description.to_string());
                        e.chip = cx.global::<Settings>().chip(def);
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
                let registry = cx.global::<Settings>().registry.clone();
                let items: Vec<(String, Entry)> = registry
                    .all()
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let mut e = Entry::new(Target::Theme(i));
                        e.icon = Some(if t.name == current.name {
                            IconName::Check
                        } else {
                            IconName::Palette
                        });
                        e.detail = Some(if t.is_dark() { "dark" } else { "light" }.to_string());
                        (t.name.clone(), e)
                    })
                    .collect();
                let mut state = PickerState::new(items);
                if let Some(i) = registry.all().iter().position(|t| t.name == current.name) {
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
            let registry = cx.global::<Settings>().registry.clone();
            if let Some(t) = registry.all().get(*i) {
                cx.set_global(Theme::from_mt(t));
                self.sync_palette(cx);
            }
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
                let registry = cx.global::<Settings>().registry.clone();
                if let Some(t) = registry.all().get(i) {
                    let theme = Theme::from_mt(t);
                    cx.set_global(theme);
                    self.sync_palette(cx);
                    self.toast(ToastKind::Success, format!("Theme: {}", theme.name), cx);
                }
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
            session: None,
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
        if self.live().is_some() {
            // The live viewport resizes its PTY (and shows the HUD) from paint, where the
            // exact viewport bounds are known.
            cx.notify();
            return;
        }
        let size = window.viewport_size();
        let tab_bar = if self.cfg.tab_bar.hide_if_single && self.tabs.len() <= 1 {
            0.0
        } else {
            TAB_BAR_HEIGHT
        };
        let (cols, rows) = self.cell.grid_for(
            f32::from(size.width) - 2.0 * self.cfg.window.padding.x,
            f32::from(size.height) - TITLE_BAR_HEIGHT - tab_bar - 2.0 * self.cfg.window.padding.y,
        );
        self.show_hud(cols as usize, rows as usize, cx);
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
            Some(OverlayKind::PasteConfirm) => {
                if ev.keystroke.key == "enter" {
                    self.confirm_paste(window, cx);
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

    // ---------------------------------------------------------------- configuration

    /// Spawn options from the active config: shell, env, `$TERM`, scrollback, OSC 52 policy,
    /// default cursor and the theme palette. These apply to new panes only (SPEC §8.2).
    pub(crate) fn spawn_options(&self, cx: &App) -> mt_term::SpawnOptions {
        let cfg = &cx.global::<Settings>().config;
        let cwd = match cfg.new_tab_cwd.as_str() {
            "inherit" => None,
            "home" => std::env::var_os("HOME").map(std::path::PathBuf::from),
            path => Some(expand_tilde(path)),
        };
        mt_term::SpawnOptions {
            shell: cfg.shell.clone(),
            cwd,
            env: cfg
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            term: cfg.term.clone(),
            scrollback: cfg.scrollback_lines as usize,
            osc52: osc52_policy(cfg.clipboard.osc52),
            cursor: cursor_default(&cfg.cursor),
            palette: palette_of(cx.global::<Theme>()),
            ..Default::default()
        }
    }

    pub(crate) fn set_config_banner(&mut self, banner: Option<Banner>, cx: &mut Context<Self>) {
        match banner {
            Some(b) => {
                self.config_banner_kind = b.kind;
                self.config_error = Some(b.text.into());
            }
            None => self.config_error = None,
        }
        cx.notify();
    }

    /// Re-reads the theme palette into every live terminal (OSC 4/10/11 answers).
    pub(crate) fn sync_palette(&self, cx: &App) {
        let palette = palette_of(cx.global::<Theme>());
        for tab in &self.tabs {
            if let Some(s) = &tab.session {
                s.term.set_palette(palette.clone());
            }
        }
    }

    /// Applies a reloaded config to this window (called after the global swap).
    pub(crate) fn apply_config(
        &mut self,
        diff: &mt_config::ConfigDiff,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (cfg, generation) = {
            let s = cx.global::<Settings>();
            (s.config.clone(), s.font_generation)
        };
        self.cfg = cfg.clone();
        self.font = TermFont::from_config(&cfg.font);
        self.font_generation = generation;
        if diff.has(mt_config::Effect::Relayout) {
            self.cell =
                CellMetrics::measure(window, &self.font, cfg.font.size, cfg.font.line_height);
            // Paint pushes the new grid size to each PTY; that is not a user resize.
            self.hud_armed.set(false);
            for tab in &self.tabs {
                if let Some(s) = &tab.session {
                    s.size.set((0, 0));
                }
            }
        }
        let background = background_appearance(&cfg.window);
        window.set_background_appearance(background);
        let cursor = cursor_default(&cfg.cursor);
        let osc52 = osc52_policy(cfg.clipboard.osc52);
        for tab in &self.tabs {
            if let Some(s) = &tab.session {
                s.term.set_runtime(osc52, cursor);
            }
        }
        self.cursor_phase = true;
        cx.notify();
    }

    /// `ReloadConfig`: runs the same pipeline as the file watcher, then reports.
    pub(crate) fn reload_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The reload updates every window, this one included, so it must run after this
        // update has finished.
        let entity = cx.entity();
        window.defer(cx, move |_, cx| {
            let outcome = settings::reload(cx);
            entity.update(cx, |this, cx| match outcome {
                settings::ReloadOutcome::Rejected => {}
                settings::ReloadOutcome::Unchanged => {
                    this.toast(ToastKind::Info, "Config reloaded (no changes)", cx)
                }
                settings::ReloadOutcome::Applied(changed) => this.toast(
                    ToastKind::Success,
                    format!("Config reloaded: {}", changed.join(", ")),
                    cx,
                ),
            });
        });
    }

    /// `OpenConfig`: `$VISUAL`/`$EDITOR` in a new tab, else the system opener.
    pub(crate) fn open_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = cx.global::<Settings>().path.clone();
        if let Err(e) = mt_config::editor::ensure_config_file(&path) {
            self.toast(
                ToastKind::Error,
                format!("Cannot create {}: {e}", path.display()),
                cx,
            );
            return;
        }
        let Some(argv) = mt_config::editor::editor_argv_from_env(&path) else {
            cx.open_with_system(&path);
            self.toast(
                ToastKind::Info,
                "Opened the config with the system opener",
                cx,
            );
            return;
        };
        let title = std::path::Path::new(&argv[0])
            .file_name()
            .map_or_else(|| "editor".into(), |n| n.to_string_lossy().into_owned());
        self.add_tab(MockKind::Zsh, &title, cx);
        let index = self.tabs.len() - 1;
        let mut opts = self.spawn_options(cx);
        opts.command = Some(argv);
        if let Err(e) = self.attach_terminal(index, opts, window, cx) {
            let id = self.tabs[index].id;
            self.close_tab(id, cx);
            self.toast(
                ToastKind::Error,
                format!("Could not start the editor: {e}"),
                cx,
            );
        } else {
            self.tabs[index].title = title;
        }
    }

    pub(crate) fn show_config_error(&mut self, cx: &mut Context<Self>) {
        self.config_banner_kind = ToastKind::Error;
        self.config_error = Some(
            "microterm.toml:12:9 — unknown key `fontt` (did you mean `font`?). Keeping the previous config.".into(),
        );
        cx.notify();
    }
}

/// Colours the terminal needs to answer OSC 4/10/11/12 queries.
fn palette_of(t: &Theme) -> mt_term::Palette {
    let rgb = |c: gpui::Hsla| {
        let c = gpui::Rgba::from(c);
        [
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
        ]
    };
    mt_term::Palette {
        fg: rgb(t.term.fg),
        bg: rgb(t.background),
        cursor: rgb(t.term.cursor),
        ansi: t.term.ansi.map(rgb),
    }
}

fn osc52_policy(p: mt_config::Osc52) -> mt_term::Osc52Policy {
    match p {
        mt_config::Osc52::Write => mt_term::Osc52Policy::Write,
        mt_config::Osc52::ReadWrite => mt_term::Osc52Policy::ReadWrite,
        mt_config::Osc52::Disabled => mt_term::Osc52Policy::Disabled,
    }
}

fn cursor_default(c: &mt_config::Cursor) -> mt_term::CursorDefault {
    mt_term::CursorDefault {
        shape: match c.style {
            mt_config::CursorStyle::Block => mt_term::CursorShape::Block,
            mt_config::CursorStyle::Beam => mt_term::CursorShape::Beam,
            mt_config::CursorStyle::Underline => mt_term::CursorShape::Underline,
        },
        blinking: c.blink,
    }
}

fn expand_tilde(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::PathBuf::from(home).join(rest),
        _ if path == "~" => std::env::var_os("HOME").map_or_else(|| path.into(), Into::into),
        _ => path.into(),
    }
}

/// Window transparency from `window.opacity` / `window.blur` (blur is macOS only).
pub(crate) fn background_appearance(w: &mt_config::Window) -> gpui::WindowBackgroundAppearance {
    if w.opacity >= 1.0 {
        gpui::WindowBackgroundAppearance::Opaque
    } else if w.blur && cfg!(target_os = "macos") {
        gpui::WindowBackgroundAppearance::Blurred
    } else {
        gpui::WindowBackgroundAppearance::Transparent
    }
}

/// Best-effort system bell for `bell.mode = "audible"`: GPUI has no bell API.
fn audible_bell() {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("afplay", &["/System/Library/Sounds/Tink.aiff"])
    } else {
        ("canberra-gtk-play", &["-i", "bell"])
    };
    let _ = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
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
    let cfg = cx.try_global::<Settings>().map_or_else(
        || Arc::new(mt_config::Config::default()),
        |s| s.config.clone(),
    );
    // Approximate the cell size from the font size; the exact grid is fitted after first paint.
    let cell_w = cfg.font.size * 0.6;
    let cell_h = cfg.font.size * cfg.font.line_height;
    let width = cfg.window.initial_size.cols as f32 * cell_w + 2.0 * cfg.window.padding.x;
    let height = cfg.window.initial_size.rows as f32 * cell_h
        + 2.0 * cfg.window.padding.y
        + TITLE_BAR_HEIGHT
        + TAB_BAR_HEIGHT;
    let bounds = Bounds::centered(
        None,
        gpui::size(px(width.max(480.)), px(height.max(300.))),
        cx,
    );
    let mac = Platform::current() == Platform::Mac;
    let decorations = match cfg.window.decorations {
        mt_config::Decorations::Server => gpui::WindowDecorations::Server,
        _ => gpui::WindowDecorations::Client,
    };
    gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: (cfg.window.decorations != mt_config::Decorations::None).then(|| {
            gpui::TitlebarOptions {
                title: Some("microterm".into()),
                appears_transparent: true,
                traffic_light_position: mac
                    .then(|| gpui::point(px(TRAFFIC_LIGHT_INSET), px(TRAFFIC_LIGHT_INSET))),
            }
        }),
        window_min_size: Some(gpui::size(px(480.), px(300.))),
        window_background: background_appearance(&cfg.window),
        app_id: Some("microterm".into()),
        window_decorations: Some(decorations),
        ..Default::default()
    }
}
