//! The application window: all M1 state (tabs, workspaces, overlays, toasts) and its behaviour,
//! driven by mock data. Rendering lives in `view.rs`.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    prelude::*, px, App, Bounds, ClipboardItem, Context, FocusHandle, KeyDownEvent, KeyUpEvent,
    Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Subscription, Window,
};
use mt_mux::{DomainState, SpawnSpec};
use mt_ssh::prompt::{PromptKind as SshAsk, PromptRequest, Secret};
use mt_term::{KeyEventKind, ScrollCmd, TermEvent};

use crate::{
    actions::*,
    assets::IconName,
    components::{StatusKind, ToastKind},
    metrics::*,
    mock::{Line, MockKind},
    mode_overlays::{CopyState, QuickState, SearchState},
    mux_host::{self, MuxHost},
    overlay_stack::{OverlayKind, OverlayStack},
    picker_state::{PickerInput, PickerOutcome, PickerState},
    platform::Platform,
    session::{self, TerminalSession},
    settings::{self, Banner, Settings},
    terminal_view::{CellMetrics, TermFont},
    theme::Theme,
};

/// The view-side companion of a mux tab: its live terminal and display state. The order of
/// `AppWindow::tabs` always mirrors the mux window's tabs.
pub struct TabView {
    /// Mux tab id.
    pub id: u64,
    /// Mux pane id.
    pub pane: u64,
    /// `None` until a shell is attached (and always in unit tests that do not opt in).
    pub session: Option<TerminalSession>,
    /// Display title after `tab_bar.title_format`.
    pub title: String,
    /// Foreground process name when it is not the shell.
    pub process: Option<String>,
    /// A non-shell foreground process is running (the tab's dirty dot).
    pub dirty: bool,
    wake: Option<async_channel::Receiver<()>>,
    pump: Option<gpui::Task<()>>,
}

impl TabView {
    fn new(id: u64, pane: u64) -> Self {
        TabView {
            id,
            pane,
            session: None,
            title: String::new(),
            process: None,
            dirty: false,
            wake: None,
            pump: None,
        }
    }
}

/// What a close-confirmation dialog will close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTarget {
    Tab(u64),
    Window,
}

/// What a name prompt will do with the typed name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    ConnectHost,
    CreateWorkspace,
    RenameWorkspace(u64),
}

/// What the reconnect card shows (SPEC §4.6).
pub struct SshCard {
    pub domain: mt_mux::DomainId,
    pub name: String,
    /// `user@host:port`.
    pub label: String,
    pub state: DomainState,
}

pub struct NamePrompt {
    pub kind: PromptKind,
    pub text: String,
}

/// What confirming a picker row does.
#[derive(Clone, Debug)]
pub enum Target {
    Action(usize),
    /// `New tab in <domain>`.
    NewTab(String),
    /// "Connect to host…": asks for `[user@]host[:port]`.
    ConnectHost,
    Workspace(String),
    CreateWorkspace,
    Theme(usize),
    /// Index into this window's tabs.
    Tab(usize),
    /// Index into `config.launch`.
    Launch(usize),
}

/// Provider groups of the command palette (the `u8` tag in `PickerState`).
pub mod group {
    pub const ACTIONS: u8 = 0;
    pub const WORKSPACES: u8 = 1;
    pub const TABS: u8 = 2;
    pub const THEMES: u8 = 3;
}

/// `>` actions (the default), `@` workspaces, `#` tabs, `~` themes (SPEC §10.1).
pub const PALETTE_PREFIXES: &[(char, u8)] = &[
    ('>', group::ACTIONS),
    ('@', group::WORKSPACES),
    ('#', group::TABS),
    ('~', group::THEMES),
];

#[derive(Clone, Debug)]
pub struct Entry {
    pub target: Target,
    /// Stable MRU key, e.g. `action:SpawnTab` or `workspace:api`.
    pub id: String,
    pub group: u8,
    pub detail: Option<String>,
    pub chip: Option<String>,
    pub icon: Option<IconName>,
    pub status: Option<StatusKind>,
}

impl Entry {
    fn new(target: Target, id: impl Into<String>) -> Self {
        Self {
            target,
            id: id.into(),
            group: group::ACTIONS,
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

pub struct ToastItem {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
}

pub struct AppWindow {
    pub(crate) viewport_focus: FocusHandle,
    pub(crate) overlay_focus: FocusHandle,
    pub(crate) platform: Platform,
    /// The mux window this OS window shows.
    pub(crate) mux_window: u64,
    /// Set while the window is being hidden by a workspace switch (tabs are parked, not closed).
    pub(crate) hiding: bool,
    pub(crate) tabs: Vec<TabView>,
    pub(crate) active_tab: usize,
    /// Inline tab rename in progress: `(tab id, text)`.
    pub(crate) renaming: Option<(u64, String)>,
    pub(crate) close_confirm: Option<CloseTarget>,
    pub(crate) prompt: Option<NamePrompt>,
    last_window_title: String,
    size_epoch: u64,
    pub(crate) overlays: OverlayStack,
    pub(crate) picker: Option<PickerData>,
    pub(crate) search: SearchState,
    pub(crate) copy: CopyState,
    pub(crate) quick: QuickState,
    pub(crate) toasts: Vec<ToastItem>,
    next_toast_id: u64,
    pub(crate) config_error: Option<SharedString>,
    /// Questions from SSH connections (host keys, passwords), oldest first; the front one is
    /// on screen. Dropping a request cancels it.
    pub(crate) ssh_prompts: VecDeque<PromptRequest>,
    /// What has been typed into the front secret prompt (never rendered, never stored).
    pub(crate) ssh_input: String,
    pub(crate) hud: Option<(usize, usize)>,
    hud_epoch: u64,
    pub(crate) scrollbar_visible: bool,
    scrollbar_epoch: u64,
    pub(crate) tab_menu: Option<(Point<Pixels>, u64)>,
    pub(crate) mouse: crate::mouse_input::MouseState,
    /// Built-in plus `[[hyperlink_rules]]` patterns; rebuilt on config reload.
    pub(crate) links: Arc<mt_patterns::LinkSet>,
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
    /// Per-window font size set by Increase/DecreaseFontSize; `None` follows the config.
    pub(crate) font_size_override: Option<f32>,
    pub(crate) config_banner_kind: ToastKind,
    /// False until the first paint has sized the PTY, so start-up shows no resize HUD.
    pub(crate) hud_armed: Rc<Cell<bool>>,
    blink_started: bool,
    /// Give the pane focus on the next frame (an overlay closed during a tab switch).
    pub(crate) refocus_viewport: bool,
    theme_restore: Option<Theme>,
    _subscriptions: Vec<Subscription>,
}

impl AppWindow {
    /// A new window in the active workspace with one tab (unit tests and the single-window
    /// fallback). Production windows are opened by `mux_host`, which attaches existing mux
    /// windows.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<Settings>() {
            cx.set_global(Settings::defaults());
        }
        let mux_window = {
            let host = mux_host::host(cx);
            let w = host.mux.spawn_window(None);
            host.spawn_tab(w, SpawnSpec::default());
            w
        };
        Self::attach(mux_window, window, cx)
    }

    /// A window showing `mux_window`. Tabs parked by a workspace switch return with their
    /// shells still running; tabs without a terminal start one.
    pub fn attach(mux_window: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let viewport_focus = cx.focus_handle();
        let overlay_focus = cx.focus_handle();
        window.focus(&viewport_focus);
        let subscription = cx.observe_window_bounds(window, |this, window, cx| {
            this.on_resize(window, cx);
        });
        // The window the user is in is the one `ctl` treats as focused.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.clear_active_attention(cx);
                let id = this.mux_window;
                let host = mux_host::host(cx);
                if host
                    .mux
                    .window(id)
                    .is_some_and(|w| w.workspace_id == host.mux.active_workspace().id)
                {
                    let _ = host.mux.focus_window(id);
                }
            }
        });
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
        let release = cx.on_release(|this, cx| this.released(cx));
        let parked = mux_host::host(cx)
            .parked
            .remove(&mux_window)
            .unwrap_or_default();
        let mut this = Self {
            viewport_focus,
            overlay_focus,
            platform: Platform::current(),
            mux_window,
            hiding: false,
            tabs: parked,
            active_tab: 0,
            renaming: None,
            close_confirm: None,
            prompt: None,
            last_window_title: String::new(),
            size_epoch: 0,
            overlays: OverlayStack::default(),
            picker: None,
            search: SearchState::default(),
            copy: CopyState::default(),
            quick: QuickState::default(),
            toasts: Vec::new(),
            next_toast_id: 0,
            config_error: None,
            ssh_prompts: VecDeque::new(),
            ssh_input: String::new(),
            hud: None,
            hud_epoch: 0,
            scrollbar_visible: false,
            scrollbar_epoch: 0,
            tab_menu: None,
            mouse: Default::default(),
            links: crate::mouse_input::build_links(&cfg),
            cell,
            cursor_phase: true,
            bell_flash: false,
            bell_epoch: 0,
            paste_confirm: None,
            cfg,
            font,
            font_generation,
            font_size_override: None,
            config_banner_kind: ToastKind::Error,
            hud_armed: Rc::new(Cell::new(false)),
            blink_started: false,
            refocus_viewport: false,
            theme_restore: None,
            _subscriptions: vec![subscription, activation, appearance, release],
        };
        if let Some(b) = banner {
            this.config_banner_kind = b.kind;
            this.config_error = Some(b.text.into());
        }
        this.reconcile(window, cx);
        // Unit tests build windows by the dozen and never poll processes.
        if !cfg!(test) {
            this.start_process_poll(cx);
        }
        this
    }

    /// Runs when the OS window goes away: a hidden window parks its tabs for the next time its
    /// workspace is shown; a closed one removes itself from the mux (dropping its shells).
    fn released(&mut self, cx: &mut App) {
        if !cx.has_global::<MuxHost>() {
            return;
        }
        let id = self.mux_window;
        let host = mux_host::host(cx);
        host.forget_open(id);
        if self.hiding {
            let mut tabs = std::mem::take(&mut self.tabs);
            for t in &mut tabs {
                t.pump = None;
            }
            host.parked.insert(id, tabs);
        } else {
            for t in &self.tabs {
                host.pending.remove(&t.pane);
            }
            let _ = host.mux.close_window(id);
        }
    }

    // ---------------------------------------------------------------- tabs mirror the mux

    /// Brings `self.tabs` in line with the mux window: new tabs get a view (and a shell),
    /// removed ones are dropped, order and the active tab are copied.
    pub(crate) fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (order, active) = {
            let host = mux_host::host(cx);
            match host.mux.window(self.mux_window) {
                Some(w) => (
                    w.tabs
                        .iter()
                        .map(|t| (t.id, t.pane().id))
                        .collect::<Vec<_>>(),
                    w.active,
                ),
                None => (Vec::new(), 0),
            }
        };
        let mut old = std::mem::take(&mut self.tabs);
        let mut fresh = Vec::new();
        for (id, pane) in order {
            match old.iter().position(|t| t.id == id) {
                Some(i) => self.tabs.push(old.swap_remove(i)),
                None => {
                    self.tabs.push(TabView::new(id, pane));
                    fresh.push(id);
                }
            }
        }
        drop(old);
        self.active_tab = active.min(self.tabs.len().saturating_sub(1));
        // `tab focus` over IPC, a closed tab or a workspace switch can bring a badged tab into
        // view without a click.
        if window.is_window_active() {
            self.clear_active_attention(cx);
        }

        let mut failed = Vec::new();
        for i in 0..self.tabs.len() {
            let id = self.tabs[i].id;
            if self.tabs[i].session.is_some() {
                if self.tabs[i].pump.is_none() {
                    self.start_pump(id, window, cx);
                }
            } else if fresh.contains(&id) && !cfg!(test) {
                let spec = mux_host::host(cx)
                    .pending
                    .remove(&self.tabs[i].pane)
                    .unwrap_or_default();
                if spec.domain != mt_mux::LOCAL_DOMAIN_ID {
                    let domain = spec.domain;
                    if let Err(e) = self.attach_remote_terminal(i, domain, &spec, window, cx) {
                        self.toast(ToastKind::Error, format!("Could not open the tab: {e}"), cx);
                        failed.push(id);
                    }
                    continue;
                }
                let mut opts = self.spawn_options(cx);
                opts.cwd = spec.cwd;
                opts.command = spec.command;
                // Local panes only: SSH-domain panes never get these (SPEC §12.6 S7).
                let socket = mux_host::host(cx).socket.clone();
                opts.env.extend(crate::ipc_ops::pane_env(
                    self.tabs[i].pane,
                    self.tabs[i].id,
                    self.mux_window,
                    socket.as_deref(),
                ));
                if let Err(e) = self.attach_terminal(i, opts, window, cx) {
                    self.toast(
                        ToastKind::Error,
                        format!("Could not start the shell: {e}"),
                        cx,
                    );
                    failed.push(id);
                }
            }
        }
        self.refresh_titles(cx);
        cx.notify();
        for id in failed {
            self.close_tab(id, window, cx);
        }
    }

    /// Recomputes every tab's display title and process state from the mux and its terminal.
    fn refresh_titles(&mut self, cx: &mut Context<Self>) {
        let fmt = self.cfg.tab_bar.title_format.clone();
        let show_index = self.cfg.tab_bar.show_index;
        let host = mux_host::host(cx);
        let Some(win) = host.mux.window(self.mux_window) else {
            return;
        };
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let Some(mt) = win.tabs.get(i) else { continue };
            let pane = mt.pane();
            let shell = tab.session.as_ref().map(|s| s.default_title.as_str());
            let parts = mt_mux::TitleParts {
                title_override: mt.title_override.as_deref(),
                pane_title: Some(pane.title.as_str()),
                process: tab.process.as_deref().or(shell),
                cwd: pane.cwd.as_deref(),
                index: i,
                show_index,
            };
            tab.title = mt_mux::format_title(&parts, &fmt);
            if tab.title.is_empty() {
                tab.title = "shell".into();
            }
        }
    }

    pub(crate) fn active_title(&self) -> &str {
        self.tabs
            .get(self.active_tab)
            .map_or("microterm", |t| t.title.as_str())
    }

    /// Foreground process, cwd and dirty-dot state change without any output, so they are
    /// polled (SPEC §5: per-OS process inspection).
    fn start_process_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(750))
                .await;
            if this.update(cx, |this, cx| this.poll_processes(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn poll_processes(&mut self, cx: &mut Context<Self>) {
        let before: Vec<(String, bool)> = self
            .tabs
            .iter()
            .map(|t| (t.title.clone(), t.dirty))
            .collect();
        let mut cwds = Vec::new();
        for tab in &mut self.tabs {
            let Some(session) = &tab.session else {
                continue;
            };
            tab.process = session.term.foreground_process().map(|p| p.name);
            tab.dirty = tab.process.is_some();
            if let Some(cwd) = session.term.cwd() {
                cwds.push((tab.pane, cwd));
            }
        }
        {
            let host = mux_host::host(cx);
            for (pane, cwd) in cwds {
                host.mux.set_pane_cwd(pane, cwd);
            }
        }
        self.refresh_titles(cx);
        let after: Vec<(String, bool)> = self
            .tabs
            .iter()
            .map(|t| (t.title.clone(), t.dirty))
            .collect();
        if before != after {
            cx.notify();
        }
    }

    // ---------------------------------------------------------------- live terminal

    /// Starts a shell for tab `index`.
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
        tab.session = Some(session);
        tab.wake = Some(rx);
        let tab_id = tab.id;
        self.start_pump(tab_id, window, cx);
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

    /// Attaches a terminal whose process lives in an SSH domain's daemon to tab `index`.
    fn attach_remote_terminal(
        &mut self,
        index: usize,
        domain: mt_mux::DomainId,
        spec: &SpawnSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut opts = self.spawn_options(cx);
        opts.cols = session::DEFAULT_COLS;
        opts.rows = session::DEFAULT_ROWS;
        opts.cell_width = self.cell.width as u16;
        opts.cell_height = self.cell.height as u16;
        opts.palette = palette_of(cx.global::<Theme>());
        if opts.scrollback == 0 {
            opts.scrollback = session::DEFAULT_SCROLLBACK;
        }
        let pane = self.tabs.get(index).ok_or("no such tab")?.pane;
        let (tx, rx) = async_channel::bounded::<()>(1);
        let term = crate::ssh_host::remote_terminal(cx, domain, pane, spec, opts, tx)?;
        let title = {
            let host = mux_host::host(cx);
            match host.mux.domain_kind(domain) {
                Some(mt_mux::DomainKind::Ssh { name }) => name,
                _ => "ssh".to_string(),
            }
        };
        let tab = self.tabs.get_mut(index).ok_or("no such tab")?;
        tab.session = Some(TerminalSession::from_terminal(term, title));
        tab.wake = Some(rx);
        let tab_id = tab.id;
        self.start_pump(tab_id, window, cx);
        Ok(())
    }

    /// Drains a tab's terminal events whenever it wakes (and once up front, for output that
    /// arrived while the window was hidden). The task is dropped when the tab is parked.
    fn start_pump(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let Some(rx) = tab.wake.clone() else { return };
        tab.pump = Some(cx.spawn_in(window, async move |this, cx| {
            if this
                .update_in(cx, |this, window, cx| {
                    this.pump_terminal(tab_id, window, cx)
                })
                .is_err()
            {
                return;
            }
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
        }));
    }

    pub(crate) fn live(&self) -> Option<&TerminalSession> {
        self.tabs.get(self.active_tab)?.session.as_ref()
    }

    pub(crate) fn live_mut(&mut self) -> Option<&mut TerminalSession> {
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
        let tab = &mut self.tabs[index];
        let pane = tab.pane;
        let Some(session) = tab.session.as_mut() else {
            return;
        };
        let mut bell = false;
        let mut exit = None;
        let mut new_title = None;
        let mut finished = Vec::new();
        let mut progress = Vec::new();
        let mut notes = Vec::new();
        let term = session.term.clone();
        for ev in session.term.drain_events() {
            match ev {
                TermEvent::Wakeup | TermEvent::CursorBlinkingChange => {}
                TermEvent::Title(t) => new_title = Some(t),
                TermEvent::ResetTitle => new_title = Some(String::new()),
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
                TermEvent::CommandFinished(code) => finished.push(code),
                TermEvent::Progress { state, value } => progress.push((state, value)),
                TermEvent::Notify(text) => notes.push(text),
            }
        }
        session.refresh();
        if index == self.active_tab {
            self.search_follow_output();
        }
        if let Some(title) = new_title {
            mux_host::host(cx).mux.set_pane_title(pane, &title);
            self.refresh_titles(cx);
        }
        self.cursor_phase = true;
        if bell {
            self.flash_bell(cx);
        }
        for code in finished {
            mux_host::on_command_finished(cx, pane, code);
        }
        for (state, value) in progress {
            mux_host::on_osc_progress(cx, pane, state, value);
        }
        for text in notes {
            mux_host::on_osc_notify(cx, pane, &text);
        }
        if let Some(code) = exit {
            // `pane run --keep` leaves the tab open, showing the exit notice.
            if !mux_host::on_pane_exit(cx, pane, &term, code) {
                if let Some(c) = code.filter(|c| *c != 0) {
                    self.toast(
                        ToastKind::Warning,
                        format!("Shell exited with status {c}"),
                        cx,
                    );
                }
                self.close_tab(tab_id, window, cx);
            }
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

    pub(crate) fn on_viewport_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The pane must never swallow keys while an overlay is open: if focus ended up here,
        // take it back and hand the key to the overlay so Esc and labels still work.
        if !self.overlays.is_empty() {
            self.sync_focus(window);
            if ev.keystroke.key == "escape" {
                self.dismiss_top(window, cx);
            } else {
                self.on_overlay_key(ev, window, cx);
            }
            return;
        }
        if self.renaming.is_some() {
            self.rename_key(ev, cx);
            return;
        }
        // Keyboard input is blocked while the domain is down (SPEC §11.4); Enter retries now.
        if self.ssh_blocks_input(cx) {
            if ev.keystroke.key == "enter" {
                self.reconnect_now(cx);
            }
            return;
        }
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
        if kind != KeyEventKind::Release {
            // Typing drops the selection and hides the pointer until it moves again.
            if session.term.has_selection() {
                session.term.selection_clear();
                session.refresh();
            }
            if self.cfg.mouse.hide_while_typing {
                self.mouse.pointer_hidden = true;
                self.mouse.hover = None;
            }
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
        if self.mouse_wheel_report(ev, n) {
            return;
        }
        let Some(session) = self.live_mut() else {
            return;
        };
        if modes.alt_screen {
            // Alternate scroll: the wheel becomes arrow keys (DECSET 1007), unless the app
            // asked for mouse reports (those were sent above unless Shift bypassed them).
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

    /// `SendString`: writes text to the active pane unmodified (no bracketed paste).
    pub(crate) fn send_string(&mut self, text: &str, cx: &mut Context<Self>) {
        match self.live() {
            Some(session) => session.term.write(text.as_bytes().to_vec()),
            None => self.toast(ToastKind::Info, "Send string needs a live terminal tab", cx),
        }
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

    /// Tabs without a shell (unit tests, failed spawns) show the mock screen.
    pub(crate) fn active_kind(&self) -> Option<MockKind> {
        self.tabs.get(self.active_tab).map(|_| MockKind::Zsh)
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
        if kind == OverlayKind::QuickSelect && !self.cfg.quick_select.enabled {
            self.toast(
                ToastKind::Info,
                "Quick select is turned off (quick_select.enabled = false)",
                cx,
            );
            return;
        }
        self.tab_menu = None;
        self.mouse.menu = None;
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
            OverlayKind::QuickSelect => self.prepare_quick_select(cx),
            OverlayKind::CopyMode => self.prepare_copy_mode(cx),
            OverlayKind::Search => {
                if !self.search.query.is_empty() {
                    self.search_refresh(true);
                }
            }
            OverlayKind::PasteConfirm
            | OverlayKind::CloseConfirm
            | OverlayKind::NamePrompt
            | OverlayKind::SshPrompt => {}
        }
    }

    /// Esc: closes only the top overlay and hands focus back.
    pub(crate) fn dismiss_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_menu.take().is_some() || self.mouse.menu.take().is_some() {
            cx.notify();
            return;
        }
        if self.overlays.top() == Some(OverlayKind::CopyMode) {
            self.copy_escape(window, cx);
            return;
        }
        if let Some(kind) = self.overlays.dismiss_top() {
            self.teardown(kind, true, cx);
        }
        self.sync_focus(window);
        cx.notify();
    }

    pub(crate) fn close_overlay(
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
            OverlayKind::ThemeSelector | OverlayKind::CommandPalette => {
                if revert_theme {
                    if let Some(theme) = self.theme_restore.take() {
                        cx.set_global(theme);
                        self.sync_palette(cx);
                    }
                }
                self.theme_restore = None;
                self.picker = None;
            }
            OverlayKind::Launcher | OverlayKind::WorkspaceSwitcher => {
                self.picker = None;
            }
            OverlayKind::QuickSelect => self.quick = QuickState::default(),
            OverlayKind::CopyMode => self.teardown_copy_mode(),
            OverlayKind::Search => {
                self.search.matches.clear();
                self.search.error = None;
            }
            OverlayKind::SshPrompt => {
                // Closing without answering cancels the request (dropped here).
                self.ssh_prompts.pop_front();
                self.ssh_input.clear();
            }
            OverlayKind::PasteConfirm => self.paste_confirm = None,
            OverlayKind::CloseConfirm => self.close_confirm = None,
            OverlayKind::NamePrompt => self.prompt = None,
        }
    }

    // ---------------------------------------------------------------- pickers

    fn build_picker(&mut self, kind: OverlayKind, cx: &mut Context<Self>) -> PickerData {
        match kind {
            OverlayKind::CommandPalette => {
                let current = *cx.global::<Theme>();
                self.theme_restore = Some(current);
                let items = self.palette_entries(cx);
                PickerData {
                    kind,
                    state: with_mru(items, cx)
                        .with_prefixes(PALETTE_PREFIXES, Some(group::ACTIONS)),
                    placeholder: "Type a command…  (@ workspaces · # tabs · ~ themes)",
                }
            }
            OverlayKind::Launcher => {
                let items = self.launcher_entries(cx);
                PickerData {
                    kind,
                    state: with_mru(items, cx),
                    placeholder: "Launch a profile, domain or workspace…",
                }
            }
            OverlayKind::WorkspaceSwitcher => {
                let rows = self.workspace_rows(cx);
                let active = rows.iter().position(|r| r.2).unwrap_or(0);
                let mut items: Vec<(String, Entry)> = rows
                    .into_iter()
                    .map(|(name, detail, is_active)| {
                        let mut e = Entry::new(
                            Target::Workspace(name.clone()),
                            format!("workspace:{name}"),
                        );
                        e.icon = Some(if is_active {
                            IconName::Check
                        } else {
                            IconName::Layers
                        });
                        e.detail = Some(detail);
                        (name, e)
                    })
                    .collect();
                let mut create = Entry::new(Target::CreateWorkspace, "workspace:create");
                create.icon = Some(IconName::Plus);
                items.push(("Create workspace…".to_string(), create));
                let mut state = PickerState::new(items);
                state.select(active);
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
                        let mut e = Entry::new(Target::Theme(i), format!("theme:{}", t.name));
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

    /// Every palette provider's rows: actions, workspaces, tabs, themes (SPEC §10.1).
    pub(crate) fn palette_entries(&self, cx: &mut Context<Self>) -> Vec<(String, Entry)> {
        let mut items: Vec<(String, Entry)> = Vec::new();
        let quick_select_on = self.cfg.quick_select.enabled;
        for (i, def) in ACTIONS
            .iter()
            .enumerate()
            .filter(|(_, d)| d.palette)
            .filter(|(_, d)| quick_select_on || d.name != "ActivateQuickSelect")
        {
            let mut e = Entry::new(Target::Action(i), format!("action:{}", def.name));
            e.detail = Some(def.description.to_string());
            e.chip = cx.global::<Settings>().chip(def);
            items.push((def.label.to_string(), e));
        }
        for (name, detail, is_active) in self.workspace_rows(cx) {
            let mut e = Entry::new(Target::Workspace(name.clone()), format!("workspace:{name}"));
            e.group = group::WORKSPACES;
            e.icon = Some(if is_active {
                IconName::Check
            } else {
                IconName::Layers
            });
            e.detail = Some(detail);
            items.push((format!("Switch to workspace: {name}"), e));
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            let mut e = Entry::new(Target::Tab(i), format!("tab:{}", tab.title));
            e.group = group::TABS;
            e.icon = Some(IconName::Terminal);
            let chrome = crate::mux_host::tab_chrome(cx, tab.pane);
            e.detail = Some(match &chrome.tooltip {
                Some(t) => format!("tab {} · {t}", i + 1),
                None => format!("tab {}", i + 1),
            });
            e.status = chrome.dot.or(chrome.badge);
            items.push((format!("Go to tab: {}", tab.title), e));
        }
        let current = *cx.global::<Theme>();
        let registry = cx.global::<Settings>().registry.clone();
        for (i, t) in registry.all().iter().enumerate() {
            let mut e = Entry::new(Target::Theme(i), format!("theme:{}", t.name));
            e.group = group::THEMES;
            e.icon = Some(if t.name == current.name {
                IconName::Check
            } else {
                IconName::Palette
            });
            e.detail = Some(if t.is_dark() { "dark" } else { "light" }.to_string());
            items.push((format!("Theme: {}", t.name), e));
        }
        items
    }

    /// Launcher providers: `[[launch]]` profiles, "New tab in <domain>", workspaces. Domains are
    /// generic so M10 can add SSH domains next to `local`.
    fn launcher_entries(&self, cx: &mut Context<Self>) -> Vec<(String, Entry)> {
        let mut items: Vec<(String, Entry)> = Vec::new();
        let profiles = cx.global::<Settings>().config.launch.clone();
        for (i, p) in profiles.iter().enumerate() {
            let mut e = Entry::new(Target::Launch(i), format!("launch:{}", p.name));
            e.icon = Some(IconName::Terminal);
            e.detail = Some(if p.command.is_empty() {
                "shell".to_string()
            } else {
                p.command.join(" ")
            });
            items.push((p.name.clone(), e));
        }
        for domain in self.domain_names(cx) {
            let mut e = Entry::new(Target::NewTab(domain.clone()), format!("domain:{domain}"));
            e.icon = Some(IconName::Plus);
            e.detail = Some("domain".into());
            e.status = Some(self.domain_status(&domain, cx));
            items.push((format!("New tab in {domain}"), e));
        }
        let mut e = Entry::new(Target::ConnectHost, "domain:connect".to_string());
        e.icon = Some(IconName::Plus);
        e.detail = Some("ssh".into());
        items.push(("Connect to host…".to_string(), e));
        for (name, detail, _) in self.workspace_rows(cx) {
            let mut e = Entry::new(Target::Workspace(name.clone()), format!("workspace:{name}"));
            e.icon = Some(IconName::Layers);
            e.detail = Some(detail);
            items.push((format!("Workspace: {name}"), e));
        }
        items
    }

    /// Names of the domains a tab can open in: `local`, then the `[[ssh_domains]]`.
    fn domain_names(&self, cx: &App) -> Vec<String> {
        let mut v = vec!["local".to_string()];
        v.extend(crate::ssh_host::configured_names(cx));
        v
    }

    /// The status dot of a domain in the launcher.
    pub(crate) fn domain_status(&self, name: &str, cx: &mut Context<Self>) -> StatusKind {
        if name == "local" {
            return StatusKind::Connected;
        }
        let host = mux_host::host(cx);
        match host
            .mux
            .domain_by_name(name)
            .and_then(|d| host.mux.domain_state(d))
        {
            Some(DomainState::Connected) => StatusKind::Connected,
            Some(DomainState::Connecting | DomainState::Reconnecting { .. }) => {
                StatusKind::Reconnecting
            }
            Some(DomainState::Disconnected { .. }) => StatusKind::Error,
            None => StatusKind::Idle,
        }
    }

    /// Rebuilds an open palette or launcher in place (config reload), keeping the query.
    fn refresh_picker(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &self.picker else { return };
        let kind = picker.kind;
        if !matches!(kind, OverlayKind::CommandPalette | OverlayKind::Launcher) {
            return;
        }
        let query = picker.state.query().to_string();
        let mut data = self.build_picker(kind, cx);
        data.state.set_query(&query);
        self.picker = Some(data);
    }

    /// `(name, "N windows · M tabs", is_active)` per workspace.
    fn workspace_rows(&self, cx: &mut Context<Self>) -> Vec<(String, String, bool)> {
        let host = mux_host::host(cx);
        let active = host.mux.active_workspace().id;
        host.mux
            .workspaces()
            .iter()
            .map(|w| {
                let (windows, tabs) = host.mux.counts(w.id);
                let detail = if windows == 0 {
                    "not started".to_string()
                } else {
                    format!(
                        "{windows} window{} · {tabs} tab{}",
                        plural(windows),
                        plural(tabs)
                    )
                };
                (w.name.clone(), detail, w.id == active)
            })
            .collect()
    }

    /// Live theme preview: a highlighted theme row previews it, any other row shows the theme
    /// the overlay opened with.
    fn preview_selected_theme(&self, cx: &mut Context<Self>) {
        let (Some(picker), Some(original)) = (&self.picker, self.theme_restore) else {
            return;
        };
        match picker.state.selected_item() {
            Some(Entry {
                target: Target::Theme(i),
                ..
            }) => {
                let registry = cx.global::<Settings>().registry.clone();
                if let Some(t) = registry.all().get(*i) {
                    cx.set_global(Theme::from_mt(t));
                    self.sync_palette(cx);
                }
            }
            _ => {
                if cx.global::<Theme>().name != original.name {
                    cx.set_global(original);
                    self.sync_palette(cx);
                }
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
                if matches!(
                    picker.kind,
                    OverlayKind::ThemeSelector | OverlayKind::CommandPalette
                ) {
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
        // Only a theme row keeps a previewed theme; anything else restores the original.
        let keep_theme = matches!(entry.target, Target::Theme(_));
        // Closing first returns focus to the viewport before the entry runs.
        self.close_overlay(kind, keep_theme, window, cx);
        crate::mru::MruStore::record(cx, &entry.id);
        match entry.target {
            Target::Action(i) => {
                let action = (ACTIONS[i].build)();
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            Target::ConnectHost => {
                self.begin_prompt(PromptKind::ConnectHost, String::new(), window, cx);
            }
            Target::NewTab(domain) => {
                if domain != "local" {
                    self.spawn_tab_in_domain(&domain, window, cx);
                } else if new_window {
                    let spec = SpawnSpec {
                        cwd: self.new_tab_cwd(cx),
                        ..Default::default()
                    };
                    cx.defer(move |cx| mux_host::open_new_window(cx, spec));
                } else {
                    self.spawn_tab(None, window, cx);
                }
            }
            Target::Launch(i) => {
                let Some(p) = cx.global::<Settings>().config.launch.get(i).cloned() else {
                    return;
                };
                let spec = spawn_spec_for(&p.command, p.cwd.as_deref(), Some(p.name));
                if new_window {
                    cx.defer(move |cx| mux_host::open_new_window(cx, spec));
                } else {
                    self.spawn_tab(Some(spec), window, cx);
                }
            }
            Target::Tab(i) => self.activate_tab(i, cx),
            // A workspace switch has no "new window" form; the modifier acts like plain Enter.
            Target::Workspace(name) => self.switch_workspace_named(name, false, cx),
            Target::CreateWorkspace => {
                self.begin_prompt(PromptKind::CreateWorkspace, String::new(), window, cx)
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

    /// Working directory for a new tab per `new_tab_cwd`: the active pane's, `$HOME`, or a path.
    pub(crate) fn new_tab_cwd(&self, _cx: &App) -> Option<std::path::PathBuf> {
        match self.cfg.new_tab_cwd.as_str() {
            "inherit" => self.live().and_then(|s| s.term.cwd()),
            "home" => std::env::var_os("HOME").map(std::path::PathBuf::from),
            path => Some(expand_tilde(path)),
        }
    }

    /// Opens a tab after the active one. `None` uses the `new_tab_cwd` policy and a shell.
    pub(crate) fn spawn_tab(
        &mut self,
        spec: Option<SpawnSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(cx);
        let spec = spec.unwrap_or_else(|| SpawnSpec {
            cwd: self.new_tab_cwd(cx),
            ..Default::default()
        });
        mux_host::host(cx).spawn_tab(self.mux_window, spec);
        self.reconcile(window, cx);
    }

    /// Closes a tab immediately; the window closes with its last tab.
    pub(crate) fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.tabs.iter().find(|t| t.id == id).map(|t| t.pane) else {
            return;
        };
        if self.renaming.as_ref().is_some_and(|(t, _)| *t == id) {
            self.renaming = None;
        }
        let empty = {
            let term = self
                .tabs
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.session.as_ref().map(|s| s.term.clone()));
            let host = mux_host::host(cx);
            // Finish a running recording while the file is still ours to complete.
            host.recorders.finish(pane, term.as_deref());
            host.pending.remove(&pane);
            host.mux.close_tab(self.mux_window, id).unwrap_or(false)
        };
        self.reconcile(window, cx);
        if empty {
            window.remove_window();
        }
    }

    fn busy_process(tab: &TabView) -> bool {
        tab.session
            .as_ref()
            .is_some_and(|s| s.term.foreground_process().is_some())
    }

    fn confirm_policy(&self) -> mt_mux::ConfirmClose {
        match self.cfg.confirm_close {
            mt_config::ConfirmClose::Always => mt_mux::ConfirmClose::Always,
            mt_config::ConfirmClose::Process => mt_mux::ConfirmClose::Process,
            mt_config::ConfirmClose::Never => mt_mux::ConfirmClose::Never,
        }
    }

    /// CloseTab: asks first when `confirm_close` says so.
    pub(crate) fn request_close_tab(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let busy = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .is_some_and(Self::busy_process);
        if mt_mux::should_confirm_close(self.confirm_policy(), busy) {
            self.close_confirm = Some(CloseTarget::Tab(id));
            self.open_overlay(OverlayKind::CloseConfirm, window, cx);
        } else {
            self.close_tab(id, window, cx);
        }
    }

    /// CloseWindow: asks first when `confirm_close` says so.
    pub(crate) fn request_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.tabs.iter().any(Self::busy_process);
        if mt_mux::should_confirm_close(self.confirm_policy(), busy) {
            self.close_confirm = Some(CloseTarget::Window);
            self.open_overlay(OverlayKind::CloseConfirm, window, cx);
        } else {
            window.remove_window();
        }
    }

    pub(crate) fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.close_confirm.take();
        self.close_overlay(OverlayKind::CloseConfirm, true, window, cx);
        match target {
            Some(CloseTarget::Tab(id)) => self.close_tab(id, window, cx),
            Some(CloseTarget::Window) => window.remove_window(),
            None => {}
        }
    }

    /// What the close dialog says: the running process, if any.
    pub(crate) fn close_confirm_text(&self) -> String {
        let running = |t: &TabView| t.process.clone().filter(|_| Self::busy_process(t));
        match self.close_confirm {
            Some(CloseTarget::Tab(id)) => {
                match self.tabs.iter().find(|t| t.id == id).and_then(running) {
                    Some(p) => format!("{p} is still running in this tab. Close it anyway?"),
                    None => "Close this tab?".into(),
                }
            }
            Some(CloseTarget::Window) => {
                let n = self.tabs.iter().filter(|t| Self::busy_process(t)).count();
                if n > 0 {
                    format!(
                        "{n} tab{} still running a process. Close the window anyway?",
                        plural(n)
                    )
                } else {
                    format!("Close this window and its {} tabs?", self.tabs.len())
                }
            }
            None => String::new(),
        }
    }

    pub(crate) fn close_other_tabs(
        &mut self,
        keep: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let others: Vec<u64> = self
            .tabs
            .iter()
            .map(|t| t.id)
            .filter(|id| *id != keep)
            .collect();
        for id in others {
            self.close_tab(id, window, cx);
        }
    }

    /// Mirrors the active tab's title onto the OS window title.
    pub(crate) fn sync_window_title(&mut self, window: &mut Window) {
        let title = self.active_title().to_string();
        if title != self.last_window_title {
            window.set_window_title(&title);
            self.last_window_title = title;
        }
    }

    pub(crate) fn activate_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|(t, _)| *t != self.tabs[index].id)
        {
            self.commit_rename(cx);
        }
        self.leave_tab(cx);
        let _ = mux_host::host(cx).mux.activate_tab(self.mux_window, index);
        self.active_tab = index;
        self.entered_tab();
        self.clear_active_attention(cx);
        cx.notify();
    }

    /// The pane the user is now looking at no longer needs to call for attention.
    fn clear_active_attention(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.tabs.get(self.active_tab).map(|t| t.pane) {
            if mux_host::host(cx).mux.clear_attention(pane) {
                cx.notify();
            }
        }
    }

    /// Before the active tab changes: copy mode and quick select belong to one pane's buffer,
    /// so they end here (copy mode clears its selection on the pane it is leaving).
    fn leave_tab(&mut self, cx: &mut Context<Self>) {
        for kind in [OverlayKind::CopyMode, OverlayKind::QuickSelect] {
            if self.overlays.contains(kind) {
                self.overlays.close(kind);
                self.teardown(kind, true, cx);
                self.refocus_viewport = true;
            }
        }
    }

    /// After the active tab changed: the find bar's matches were computed for the old pane.
    fn entered_tab(&mut self) {
        if self.overlays.contains(OverlayKind::Search) {
            self.search_refresh(true);
        }
    }

    pub(crate) fn on_action_tab_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        self.leave_tab(cx);
        let host = mux_host::host(cx);
        let _ = host.mux.activate_tab_relative(self.mux_window, delta);
        if let Some(w) = host.mux.window(self.mux_window) {
            self.active_tab = w.active;
        }
        self.entered_tab();
        cx.notify();
    }

    pub(crate) fn move_tab_relative(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = mux_host::host(cx)
            .mux
            .move_tab_relative(self.mux_window, delta);
        self.reconcile(window, cx);
    }

    /// Drag-to-reorder: moves tab `id` to where tab `onto` is.
    pub(crate) fn move_tab_onto(
        &mut self,
        id: u64,
        onto: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = self.tabs.iter().position(|t| t.id == id);
        let to = self.tabs.iter().position(|t| t.id == onto);
        if let (Some(from), Some(to)) = (from, to) {
            let _ = mux_host::host(cx).mux.move_tab(self.mux_window, from, to);
            self.reconcile(window, cx);
        }
    }

    // ---- inline rename

    pub(crate) fn begin_rename(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let id = tab.id;
        let current = mux_host::host(cx)
            .mux
            .window(self.mux_window)
            .and_then(|w| w.tabs.iter().find(|t| t.id == id))
            .and_then(|t| t.title_override.clone())
            .unwrap_or_else(|| self.tabs[self.active_tab].title.clone());
        self.renaming = Some((id, current));
        cx.notify();
    }

    pub(crate) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((id, text)) = self.renaming.take() {
            let _ = mux_host::host(cx)
                .mux
                .rename_tab(self.mux_window, id, Some(text));
            self.refresh_titles(cx);
            cx.notify();
        }
    }

    fn rename_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "enter" => self.commit_rename(cx),
            "escape" => {
                self.renaming = None;
                cx.notify();
            }
            "backspace" => {
                if let Some((_, text)) = &mut self.renaming {
                    text.pop();
                }
                cx.notify();
            }
            _ => {
                if let (Some(typed), Some((_, text))) =
                    (typed_text(ev, self.platform), self.renaming.as_mut())
                {
                    text.push_str(typed);
                    cx.notify();
                }
            }
        }
    }

    // ---- workspaces

    /// Switches workspace after the current update finishes (it closes and opens windows,
    /// this one included).
    pub(crate) fn switch_workspace_named(
        &mut self,
        name: String,
        create: bool,
        cx: &mut Context<Self>,
    ) {
        cx.defer(move |cx| {
            if let Err(e) = mux_host::show_workspace(cx, &name, create, None) {
                eprintln!("microterm: {e}");
            }
        });
    }

    pub(crate) fn on_action_workspace_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let (names, current) = {
            let host = mux_host::host(cx);
            let active = host.mux.active_workspace().id;
            let names: Vec<String> = host
                .mux
                .workspaces()
                .iter()
                .map(|w| w.name.clone())
                .collect();
            let current = host
                .mux
                .workspaces()
                .iter()
                .position(|w| w.id == active)
                .unwrap_or(0);
            (names, current)
        };
        let next = (current as isize + delta).rem_euclid(names.len() as isize) as usize;
        self.switch_workspace_named(names[next].clone(), false, cx);
    }

    pub(crate) fn begin_prompt(
        &mut self,
        kind: PromptKind,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt = Some(NamePrompt { kind, text });
        self.open_overlay(OverlayKind::NamePrompt, window, cx);
    }

    pub(crate) fn begin_rename_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (id, name) = {
            let w = mux_host::host(cx).mux.active_workspace();
            (w.id, w.name.clone())
        };
        self.begin_prompt(PromptKind::RenameWorkspace(id), name, window, cx);
    }

    pub(crate) fn confirm_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let name = prompt.text.trim().to_string();
        if name.is_empty() {
            self.prompt = Some(prompt);
            return;
        }
        match prompt.kind {
            PromptKind::ConnectHost => {
                self.close_overlay(OverlayKind::NamePrompt, true, window, cx);
                let domain = format!("{}{name}", crate::ssh_host::ADHOC_PREFIX);
                self.spawn_tab_in_domain(&domain, window, cx);
            }
            PromptKind::CreateWorkspace => {
                self.close_overlay(OverlayKind::NamePrompt, true, window, cx);
                self.switch_workspace_named(name, true, cx);
            }
            PromptKind::RenameWorkspace(id) => {
                let result = mux_host::host(cx).mux.rename_workspace(id, &name);
                match result {
                    Ok(()) => self.close_overlay(OverlayKind::NamePrompt, true, window, cx),
                    Err(e) => {
                        self.prompt = Some(prompt);
                        self.toast(ToastKind::Error, e.to_string(), cx);
                    }
                }
            }
        }
    }

    fn prompt_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "enter" => self.confirm_prompt(window, cx),
            "backspace" => {
                if let Some(p) = &mut self.prompt {
                    p.text.pop();
                }
                cx.notify();
            }
            _ => {
                if let (Some(typed), Some(p)) =
                    (typed_text(ev, self.platform), self.prompt.as_mut())
                {
                    p.text.push_str(typed);
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn active_workspace_name(&self, cx: &mut Context<Self>) -> String {
        mux_host::host(cx).mux.active_workspace().name.clone()
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

    /// Saves the window size 500 ms after the last resize (`window.remember_size`).
    fn remember_size(&mut self, window: &Window, cx: &mut Context<Self>) {
        if cfg!(test)
            || !self.cfg.window.remember_size
            || window.is_fullscreen()
            || window.is_maximized()
        {
            return;
        }
        let size = window.bounds().size;
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        self.size_epoch += 1;
        let epoch = self.size_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |this, _| {
                if this.size_epoch == epoch {
                    if let Some(dir) = crate::state::state_dir() {
                        let _ = crate::state::save_window_size(&dir, w, h);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_size(window, cx);
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

    // ---------------------------------------------------------------- SSH domains

    /// The reconnect card's data for the active tab: its domain, unless it is connected.
    pub(crate) fn ssh_card(&self, cx: &mut App) -> Option<SshCard> {
        let pane = self.tabs.get(self.active_tab)?.pane;
        let host = mux_host::host(cx);
        let domain = host.mux.pane_domain(pane)?;
        let state = host.mux.domain_state(domain)?.clone();
        let mt_mux::DomainKind::Ssh { name } = host.mux.domain_kind(domain)? else {
            return None;
        };
        let label = host
            .ssh
            .conns
            .get(&domain)
            .map_or_else(|| name.clone(), |c| c.label.clone());
        Some(SshCard {
            domain,
            name,
            label,
            state,
        })
    }

    /// The active tab's domain is connecting or disconnected: its keys go nowhere.
    pub(crate) fn ssh_blocks_input(&self, cx: &mut App) -> bool {
        self.ssh_card(cx)
            .is_some_and(|c| c.state != DomainState::Connected)
    }

    pub(crate) fn reconnect_now(&mut self, cx: &mut Context<Self>) {
        if let Some(card) = self.ssh_card(cx) {
            crate::ssh_host::reconnect_now(cx, card.domain);
        }
    }

    /// *Close tab* on the reconnect card: the remote pane keeps running on the host.
    pub(crate) fn close_ssh_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.get(self.active_tab).map(|t| t.id) {
            self.close_tab(id, window, cx);
        }
    }

    /// A domain's connection state changed (or a notice is due): repaint.
    pub(crate) fn on_domain_event(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// Queues a question from an SSH connection and shows it when nothing else is asked.
    pub(crate) fn enqueue_ssh_prompt(
        &mut self,
        req: PromptRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ssh_prompts.push_back(req);
        if self.ssh_prompts.len() == 1 {
            self.ssh_input.clear();
            self.open_overlay(OverlayKind::SshPrompt, window, cx);
        }
        cx.notify();
    }

    /// Answers the front prompt and moves on to the next one.
    fn answer_ssh_prompt(
        &mut self,
        answer: impl FnOnce(PromptRequest, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(req) = self.ssh_prompts.pop_front() {
            let typed = std::mem::take(&mut self.ssh_input);
            answer(req, typed);
        }
        if self.ssh_prompts.is_empty() {
            self.close_overlay(OverlayKind::SshPrompt, true, window, cx);
        } else {
            cx.notify();
        }
    }

    fn ssh_prompt_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(front) = self.ssh_prompts.front() else {
            return;
        };
        match &front.kind {
            SshAsk::HostKey(_) => {
                if ev.keystroke.key == "enter" {
                    self.answer_ssh_prompt(|r, _| r.confirm(true), window, cx);
                }
            }
            SshAsk::Secret(_) => match ev.keystroke.key.as_str() {
                "enter" => self.answer_ssh_prompt(
                    |r, typed| r.secret(Some(Secret::new(typed))),
                    window,
                    cx,
                ),
                "backspace" => {
                    self.ssh_input.pop();
                    cx.notify();
                }
                _ => {
                    if let Some(typed) = typed_text(ev, self.platform) {
                        self.ssh_input.push_str(typed);
                        cx.notify();
                    }
                }
            },
        }
    }

    pub(crate) fn confirm_host_key(
        &mut self,
        trust: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.answer_ssh_prompt(|r, _| r.confirm(trust), window, cx);
    }

    /// Opens a tab in a named SSH domain (or `local`).
    pub(crate) fn spawn_tab_in_domain(
        &mut self,
        domain: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match crate::ssh_host::resolve_domain(cx, domain) {
            Ok(id) => {
                let spec = SpawnSpec {
                    domain: id,
                    cwd: if id == mt_mux::LOCAL_DOMAIN_ID {
                        self.new_tab_cwd(cx)
                    } else {
                        None
                    },
                    ..Default::default()
                };
                self.spawn_tab(Some(spec), window, cx);
            }
            Err(e) => self.toast(ToastKind::Warning, e.message, cx),
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
            Some(OverlayKind::CopyMode) => self.copy_key(ev, window, cx),
            Some(OverlayKind::SshPrompt) => self.ssh_prompt_key(ev, window, cx),
            Some(OverlayKind::PasteConfirm) => {
                if ev.keystroke.key == "enter" {
                    self.confirm_paste(window, cx);
                }
            }
            Some(OverlayKind::CloseConfirm) => {
                if ev.keystroke.key == "enter" {
                    self.confirm_close(window, cx);
                }
            }
            Some(OverlayKind::NamePrompt) => self.prompt_key(ev, window, cx),
            None => {}
        }
    }

    // ---------------------------------------------------------------- action handlers

    pub(crate) fn on_action_spawn_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn_tab(None, window, cx);
    }

    pub(crate) fn on_action_close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.get(self.active_tab).map(|t| t.id) {
            self.request_close_tab(id, window, cx);
        }
    }

    /// SpawnWindow: a new window in the active workspace.
    pub(crate) fn on_action_spawn_window(&mut self, cx: &mut Context<Self>) {
        let spec = SpawnSpec {
            cwd: self.new_tab_cwd(cx),
            ..Default::default()
        };
        cx.defer(move |cx| mux_host::open_new_window(cx, spec));
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

    /// Spawn options from the active config (the caller sets `cwd`/`command`): shell, env, `$TERM`, scrollback, OSC 52 policy,
    /// default cursor and the theme palette. These apply to new panes only (SPEC §8.2).
    pub(crate) fn spawn_options(&self, cx: &App) -> mt_term::SpawnOptions {
        let cfg = &cx.global::<Settings>().config;
        mt_term::SpawnOptions {
            shell: cfg.shell.clone(),
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
            wide_regional_indicators: cfg.text.wide_regional_indicators,
            vt: vt_config(cfg),
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
        // A config edit of `font.size` takes over from the window's own zoom.
        if self.cfg.font.size != cfg.font.size {
            self.font_size_override = None;
        }
        self.cfg = cfg.clone();
        self.links = crate::mouse_input::build_links(&cfg);
        self.mouse.link_cache.clear();
        self.mouse.hover = None;
        self.font = TermFont::from_config(&cfg.font);
        self.font_generation = generation;
        if diff.has(mt_config::Effect::Relayout) {
            self.relayout(window);
        }
        let background = background_appearance(&cfg.window);
        window.set_background_appearance(background);
        let cursor = cursor_default(&cfg.cursor);
        let osc52 = osc52_policy(cfg.clipboard.osc52);
        for tab in &self.tabs {
            if let Some(s) = &tab.session {
                s.term.set_runtime(osc52, cursor);
                s.term
                    .set_wide_regional_indicators(cfg.text.wide_regional_indicators);
                s.term.set_vt_config(vt_config(&cfg));
            }
        }
        self.cursor_phase = true;
        mux_host::sync_declared(cx);
        self.refresh_titles(cx);
        self.refresh_picker(cx);
        cx.notify();
    }

    /// The font size this window renders with: its own zoom, else the config's.
    pub(crate) fn font_size(&self) -> f32 {
        self.font_size_override.unwrap_or(self.cfg.font.size)
    }

    /// Re-measures the cell and makes every PTY pick up the new grid size on the next paint.
    fn relayout(&mut self, window: &Window) {
        self.cell = CellMetrics::measure(
            window,
            &self.font,
            self.font_size(),
            self.cfg.font.line_height,
        );
        // Paint pushes the new grid size to each PTY; that is not a user resize.
        self.hud_armed.set(false);
        for tab in &self.tabs {
            if let Some(s) = &tab.session {
                s.size.set((0, 0));
            }
        }
    }

    /// `IncreaseFontSize` / `DecreaseFontSize` (`Some(step)`) and `ResetFontSize` (`None`).
    pub(crate) fn change_font_size(
        &mut self,
        step: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.font_size_override =
            step.map(|d| crate::typography::step_font_size(self.font_size(), d));
        self.relayout(window);
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
        let spec = SpawnSpec {
            cwd: self.new_tab_cwd(cx),
            command: Some(argv),
            title: Some(title),
            ..Default::default()
        };
        self.spawn_tab(Some(spec), window, cx);
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

/// `[terminal]` as the terminal facade wants it.
pub fn vt_config(cfg: &mt_config::Config) -> mt_term::VtConfig {
    mt_term::VtConfig {
        report: match cfg.terminal.osc_color_report {
            mt_config::OscColorReport::Bit16 => mt_term::ColorReport::Bit16,
            mt_config::OscColorReport::Bit8 => mt_term::ColorReport::Bit8,
            mt_config::OscColorReport::None => mt_term::ColorReport::None,
        },
        osc_notifications: cfg.terminal.osc_notifications,
        allow_shift_capture: cfg.terminal.allow_shift_capture,
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

/// Builds a picker over `items` tagged with provider groups and MRU ranks.
fn with_mru(items: Vec<(String, Entry)>, cx: &mut App) -> PickerState<Entry> {
    let ids: Vec<String> = items.iter().map(|(_, e)| e.id.clone()).collect();
    let groups = items.iter().map(|(_, e)| e.group).collect();
    let ranks = crate::mru::MruStore::ranks(cx, &ids);
    PickerState::new(items).with_meta(groups, ranks)
}

/// A spawn spec for a command with an optional `~`-expanded cwd (`SpawnCommand`, `[[launch]]`).
pub(crate) fn spawn_spec_for(
    command: &[String],
    cwd: Option<&str>,
    title: Option<String>,
) -> SpawnSpec {
    SpawnSpec {
        cwd: cwd.map(expand_tilde),
        command: (!command.is_empty()).then(|| command.to_vec()),
        title,
        ..Default::default()
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
    let mut width = cfg.window.initial_size.cols as f32 * cell_w + 2.0 * cfg.window.padding.x;
    let mut height = cfg.window.initial_size.rows as f32 * cell_h
        + 2.0 * cfg.window.padding.y
        + TITLE_BAR_HEIGHT
        + TAB_BAR_HEIGHT;
    if cfg.window.remember_size && !cfg!(test) {
        if let Some((w, h)) = crate::state::state_dir()
            .as_deref()
            .and_then(crate::state::load_window_size)
        {
            (width, height) = (w, h);
        }
    }
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
