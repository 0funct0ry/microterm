//! Recording mode in the window (SPEC §10.10.2): toggle, pill state, tab indicator data and
//! the toasts. The engine and IPC dispatch live next to this file; every start and stop goes
//! through `ipc::handle`, so the gate is checked first and CLI and GUI share one path.

use std::sync::OnceLock;
use std::time::Duration;

use gpui::{Context, Window};
use mt_proto::{
    RecordingMarkerParams, RecordingStartParams, RecordingStopParams, Request, RpcError,
};
use mt_record::{LimitHit, Sidecar};

use super::engine::Finished;
use super::gate::RecordingGate;
use super::ipc::{self, GuiHost, RecordingHost};
use crate::app::{AppWindow, ToastButton};
use crate::components::ToastKind;

pub const DISABLED_TOAST: &str = "Recording is disabled — set `recording.enabled = true`";

/// `mm:ss` (or `h:mm:ss` from an hour on).
pub fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// Toast text for a saved recording: "Saved ‹title› · 00:42 · 1.2 MB".
pub fn saved_text(s: &Sidecar) -> String {
    format!(
        "Saved {} · {} · {}",
        s.title,
        format_elapsed(Duration::from_millis(s.duration_ms)),
        format_size(s.size_bytes)
    )
}

/// True when the OS asks for reduced motion (the REC dot then stays solid). Read once.
pub fn reduce_motion() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        if std::env::var_os("MICROTERM_REDUCE_MOTION").is_some_and(|v| v != "0") {
            return true;
        }
        #[cfg(target_os = "macos")]
        {
            return std::process::Command::new("defaults")
                .args(["read", "com.apple.universalaccess", "reduceMotion"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
                .unwrap_or(false);
        }
        #[allow(unreachable_code)]
        false
    })
}

/// The GUI host while one window is being updated: that window's entity is leased, so its own
/// terminals cannot be found through `mux_host::terminal` and are supplied here instead.
pub(crate) struct WindowHost<'a> {
    app: &'a mut gpui::App,
    window: u64,
    terms: std::collections::HashMap<u64, std::sync::Arc<mt_term::Terminal>>,
}

impl RecordingHost for WindowHost<'_> {
    fn config(&self) -> std::sync::Arc<mt_config::Config> {
        self.app
            .global::<crate::settings::Settings>()
            .config
            .clone()
    }
    fn themes(&self) -> Option<std::sync::Arc<mt_theme::Registry>> {
        Some(
            self.app
                .global::<crate::settings::Settings>()
                .registry
                .clone(),
        )
    }
    fn rows(&mut self) -> mt_mux::ctl::Rows {
        let terms = self.terms.clone();
        let leased = crate::mux_host::Leased {
            window: self.window,
            terms: &terms,
        };
        crate::mux_host::rows_leased(self.app, Some(&leased))
    }
    fn terminal(&mut self, pane: u64) -> Option<std::sync::Arc<mt_term::Terminal>> {
        let terms = self.terms.clone();
        let leased = crate::mux_host::Leased {
            window: self.window,
            terms: &terms,
        };
        crate::mux_host::terminal_leased(self.app, pane, Some(&leased))
    }
    fn recorders(&mut self) -> &mut super::engine::StoreRecorders {
        &mut crate::mux_host::host(self.app).store_recorders
    }
    fn domain_kind(&mut self, pane: u64) -> Option<mt_mux::DomainKind> {
        GuiHost(self.app).domain_kind(pane)
    }
    fn theme_font(&mut self) -> (String, String) {
        GuiHost(self.app).theme_font()
    }
}

/// The focused pane's recording, for the pill: `(elapsed, captures input)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PillInfo {
    pub elapsed: Duration,
    pub input: bool,
}

impl AppWindow {
    pub(crate) fn window_host<'a>(&self, cx: &'a mut gpui::App) -> WindowHost<'a> {
        let terms = self
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter())
            .filter_map(|v| v.session.as_ref().map(|s| (v.pane, s.term.clone())))
            .collect();
        WindowHost {
            app: cx,
            window: self.mux_window,
            terms,
        }
    }

    /// False (with the disabled toast) while the feature is off (SPEC §10.10.1).
    pub(crate) fn recording_guard(&mut self, cx: &mut Context<Self>) -> bool {
        if RecordingGate::of(cx).enabled {
            return true;
        }
        self.toast_open_config(ToastKind::Info, DISABLED_TOAST, cx);
        false
    }

    pub(crate) fn focused_pane_id(&self) -> Option<u64> {
        self.tabs.get(self.active_tab).map(|t| t.pane)
    }

    /// Pill data for the focused pane; `None` when the gate is off or it is not recording.
    pub(crate) fn recording_pill(&self, cx: &gpui::App) -> Option<PillInfo> {
        if !RecordingGate::of_config(&self.cfg).enabled {
            return None;
        }
        let pane = self.focused_pane_id()?;
        let rec = &crate::mux_host::host_ref(cx).store_recorders;
        let (_, _, elapsed) = rec.running().into_iter().find(|r| r.0 == pane)?;
        Some(PillInfo {
            elapsed,
            input: rec.captures_input(pane),
        })
    }

    pub(crate) fn focused_is_recording(&self, cx: &gpui::App) -> bool {
        self.focused_pane_id().is_some_and(|p| {
            crate::mux_host::host_ref(cx)
                .store_recorders
                .is_recording(p)
        })
    }

    /// `ToggleRecording`: start the focused pane's recording, or stop and save it.
    pub(crate) fn toggle_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording_guard(cx) {
            return;
        }
        if self.focused_is_recording(cx) {
            self.stop_recording(window, cx);
        } else {
            self.start_recording(window, cx);
        }
    }

    pub(crate) fn start_recording(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording_guard(cx) {
            return;
        }
        let Some(pane) = self.focused_pane_id() else {
            return;
        };
        if self.focused_is_recording(cx) {
            self.toast(ToastKind::Info, "This pane is already being recorded", cx);
            return;
        }
        let req = Request::RecordingStart(RecordingStartParams {
            pane,
            input: None,
            title: None,
        });
        match ipc::handle(&mut self.window_host(cx), req) {
            Ok(_) => {
                let key = if cfg!(target_os = "macos") {
                    "⌘⌥R"
                } else {
                    "Ctrl+Alt+R"
                };
                self.toast(
                    ToastKind::Success,
                    format!("Recording started (`{key}` to stop)"),
                    cx,
                );
            }
            Err(e) => self.recording_error(e, cx),
        }
        cx.notify();
    }

    pub(crate) fn stop_recording(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording_guard(cx) {
            return;
        }
        let Some(pane) = self.focused_pane_id() else {
            return;
        };
        if !self.focused_is_recording(cx) {
            self.toast(ToastKind::Info, "This pane is not being recorded", cx);
            return;
        }
        let req = Request::RecordingStop(RecordingStopParams {
            pane: Some(pane),
            id: None,
            all: false,
        });
        match ipc::handle(&mut self.window_host(cx), req) {
            Ok(v) => {
                let saved: Option<Sidecar> = v
                    .get("stopped")
                    .and_then(|a| a.get(0))
                    .and_then(|s| serde_json::from_value(s.clone()).ok());
                if let Some(s) = saved {
                    self.toast_saved(&s, cx);
                }
            }
            Err(e) => self.recording_error(e, cx),
        }
        cx.notify();
    }

    pub(crate) fn recording_marker(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording_guard(cx) {
            return;
        }
        let Some(pane) = self.focused_pane_id() else {
            return;
        };
        if !self.focused_is_recording(cx) {
            self.toast(ToastKind::Info, "This pane is not being recorded", cx);
            return;
        }
        let req = Request::RecordingMarker(RecordingMarkerParams { pane, label: None });
        if let Err(e) = ipc::handle(&mut self.window_host(cx), req) {
            self.recording_error(e, cx);
        }
    }

    fn recording_error(&mut self, e: RpcError, cx: &mut Context<Self>) {
        self.toast(
            ToastKind::Error,
            format!("Recording failed: {}", e.message),
            cx,
        );
    }

    pub(crate) fn toast_saved(&mut self, s: &Sidecar, cx: &mut Context<Self>) {
        self.toast(ToastKind::Success, saved_text(s), cx);
        self.recordings_changed(Some(s.id.clone()), cx);
        if let Some(t) = self.toasts.last_mut() {
            t.buttons = vec![ToastButton::OpenPanel, ToastButton::Export(s.id.clone())];
        }
    }

    /// Toasts for recordings that ended by themselves or because the gate went off.
    pub(crate) fn report_finished(
        &mut self,
        done: Vec<Finished>,
        gate_off: bool,
        cx: &mut Context<Self>,
    ) {
        for f in done {
            match f.result {
                Ok(s) => {
                    if gate_off {
                        self.toast(ToastKind::Info, "Recording stopped: disabled in config", cx);
                    } else if let Some(l) = f.limit {
                        let which = match l {
                            LimitHit::Duration => "recording.max_duration_minutes",
                            LimitHit::Size => "recording.max_size_mb",
                        };
                        self.toast(
                            ToastKind::Warning,
                            format!("Recording stopped: reached {which}"),
                            cx,
                        );
                    }
                    self.toast_saved(&s, cx);
                }
                Err(e) => self.toast(ToastKind::Error, format!("Recording failed: {e}"), cx),
            }
        }
    }

    /// One second of recording UI: reap recordings that ended, flip the pulse, repaint.
    pub(crate) fn recording_tick(&mut self, cx: &mut Context<Self>) {
        self.rec_pulse = !self.rec_pulse;
        if !RecordingGate::of_config(&self.cfg).enabled {
            return;
        }
        let reaped = crate::mux_host::host(cx).store_recorders.reap();
        let any = !crate::mux_host::host(cx).store_recorders.panes().is_empty();
        if !reaped.is_empty() {
            self.report_finished(reaped, false, cx);
        }
        if any || self.rec_was_active {
            cx.notify();
        }
        self.rec_was_active = any;
    }

    pub(crate) fn start_recording_tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, |this, cx| this.recording_tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_format() {
        assert_eq!(format_elapsed(Duration::from_secs(42)), "00:42");
        assert_eq!(format_elapsed(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn size_format() {
        assert_eq!(format_size(512), "0.5 KB");
        assert_eq!(format_size(1_258_291), "1.2 MB");
    }
}
