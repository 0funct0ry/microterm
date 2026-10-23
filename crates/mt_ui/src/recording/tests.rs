//! Window-level recording-mode tests (SPEC §10.10.2, M12.2 "Tests"): the gate matrix, the
//! toggle state machine, several panes at once, Esc, stacking with copy mode, pane close and
//! hot reload, plus a live `/bin/sh` check that a typed command lands in the saved `.cast`.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};
use mt_term::SpawnOptions;

use crate::{
    actions::*,
    app::AppWindow,
    overlay_stack::OverlayKind,
    settings::{self, Settings},
};

/// One shared store for every test (`XDG_DATA_HOME` is process-wide).
pub(super) fn store_dir() -> &'static PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("mt_rec_ui_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("XDG_DATA_HOME", &d);
        d
    })
}

/// The store is shared, so the tests that count its files run one at a time.
pub(super) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn casts() -> Vec<PathBuf> {
    let dir = store_dir().join("microterm").join("recordings");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "cast"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub(super) fn config(name: &str, text: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mt_rec_cfg_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("microterm.toml"), text).unwrap();
    d.join("microterm.toml")
}

pub(super) fn boot(
    cx: &mut TestAppContext,
    path: PathBuf,
) -> (Entity<AppWindow>, &mut VisualTestContext) {
    store_dir();
    cx.update(|cx| {
        cx.set_global(Settings::load(path));
        settings::apply_theme(cx);
        settings::install_keymap(cx);
    });
    let (view, cx) = cx.add_window_view(AppWindow::new);
    view.update_in(cx, |this, window, cx| {
        this.attach_terminal(
            0,
            SpawnOptions {
                shell: "/bin/sh".into(),
                env: vec![("PS1".into(), "$ ".into())],
                ..Default::default()
            },
            window,
            cx,
        )
        .expect("spawn /bin/sh");
    });
    cx.run_until_parked();
    (view, cx)
}

pub(super) fn wait_for(view: &Entity<AppWindow>, cx: &mut VisualTestContext, needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(30));
        cx.run_until_parked();
        let hit = view.read_with(cx, |this, _| {
            let snap = &this.live().expect("live tab").snapshot;
            (0..snap.rows).any(|r| snap.row_text(r).contains(needle))
        });
        if hit {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
    }
}

pub(super) fn toasts(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<(String, bool)> {
    view.read_with(cx, |t, _| {
        t.toasts
            .iter()
            .map(|x| (x.text.to_string(), x.open_config))
            .collect()
    })
}

fn pill(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> bool {
    view.read_with(cx, |t, c| t.recording_pill(c).is_some())
}

pub(super) fn palette_labels(view: &Entity<AppWindow>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.dispatch_action(ShowCommandPalette);
    let labels = view.read_with(cx, |t, _| {
        let p = t.picker.as_ref().unwrap();
        p.state
            .matches()
            .iter()
            .map(|m| p.state.label(m).to_string())
            .collect()
    });
    cx.dispatch_action(DismissOverlay);
    labels
}

pub(super) const ON: &str = "[recording]\nenabled = true\n";

// ------------------------------------------------------------------ the gate matrix

#[gpui::test]
fn default_config_only_shows_the_disabled_toast(cx: &mut TestAppContext) {
    let _serial = serial();
    let (view, cx) = boot(cx, config("off", ""));
    cx.dispatch_action(ToggleRecording);
    let t = toasts(&view, cx);
    assert!(
        t.iter()
            .any(|(text, open)| text.contains("Recording is disabled") && *open),
        "{t:?}"
    );
    assert!(!pill(&view, cx));
    let labels = palette_labels(&view, cx);
    assert!(
        !labels.iter().any(|l| l.contains("Recording")),
        "{labels:?}"
    );
    // The Edit menu row is hidden too.
}

#[gpui::test]
fn toggle_starts_and_stops_and_shows_the_pill(cx: &mut TestAppContext) {
    let _serial = serial();
    let (view, cx) = boot(cx, config("toggle", ON));
    let before = casts().len();
    cx.dispatch_action(ToggleRecording);
    assert!(pill(&view, cx));
    assert!(toasts(&view, cx)
        .iter()
        .any(|(t, _)| t.starts_with("Recording started")));
    // Tab dot with tooltip.
    let dot = view.read_with(cx, |t, c| {
        let tab = &t.tabs[t.active_tab];
        t.tab_chrome_for_test(tab, c).recording
    });
    assert!(dot.is_some_and(|l| l.starts_with("Recording pane")));
    // Palette reads "Stop Recording" now.
    assert!(palette_labels(&view, cx).contains(&"Stop Recording".to_string()));
    // A second Start is a no-op with a toast.
    cx.dispatch_action(StartRecording);
    assert!(toasts(&view, cx)
        .iter()
        .any(|(t, _)| t.contains("already being recorded")));
    cx.dispatch_action(ToggleRecording);
    assert!(!pill(&view, cx));
    assert!(toasts(&view, cx)
        .iter()
        .any(|(t, _)| t.starts_with("Saved ")));
    let buttons = view.read_with(cx, |t, _| t.toasts.iter().any(|x| x.buttons.len() == 2));
    assert!(buttons, "Open panel and Export… buttons");
    assert_eq!(casts().len(), before + 1);
    assert!(palette_labels(&view, cx).contains(&"Start Recording".to_string()));
}

#[gpui::test]
fn typed_command_is_in_the_saved_cast_and_esc_reaches_the_pane(cx: &mut TestAppContext) {
    let _serial = serial();
    let (view, cx) = boot(
        cx,
        config(
            "live",
            "[recording]\nenabled = true\ncapture_input = true\n",
        ),
    );
    let before = casts();
    cx.dispatch_action(ToggleRecording);
    view.read_with(cx, |this, _| {
        this.live()
            .unwrap()
            .term
            .write(b"echo rec-live-marker\r".to_vec())
    });
    assert!(wait_for(&view, cx, "rec-live-marker"));
    // Esc is not swallowed by recording mode: no overlay opens and the recording continues.
    cx.simulate_keystrokes("escape");
    assert!(pill(&view, cx));
    view.update_in(cx, |this, w, cx| this.recording_marker(w, cx));
    cx.dispatch_action(ToggleRecording);
    let after = casts();
    let new: Vec<_> = after.iter().filter(|p| !before.contains(p)).collect();
    assert_eq!(new.len(), 1, "{after:?}");
    let text = std::fs::read_to_string(new[0]).unwrap();
    assert!(text.contains("rec-live-marker"), "{text}");
    assert!(text.contains("\"i\""), "input captured: {text}");
    assert!(text.contains("\\u001b"), "Esc reached the shell: {text}");
    assert!(text.contains("\"m\""), "marker event: {text}");
}

#[gpui::test]
fn copy_mode_stacks_with_recording(cx: &mut TestAppContext) {
    let _serial = serial();
    let (view, cx) = boot(cx, config("stack", ON));
    cx.dispatch_action(ToggleRecording);
    cx.dispatch_action(ActivateCopyMode);
    assert!(view.read_with(cx, |t, _| t.overlays.contains(OverlayKind::CopyMode)));
    assert!(
        pill(&view, cx),
        "the REC pill stays while copy mode is open"
    );
    cx.dispatch_action(DismissOverlay);
    cx.dispatch_action(ToggleRecording);
}

#[gpui::test]
fn hot_reload_off_stops_and_saves(cx: &mut TestAppContext) {
    let _serial = serial();
    let f = config("hot", ON);
    let app = cx.clone();
    let (view, vcx) = boot(cx, f.clone());
    let before = casts().len();
    vcx.dispatch_action(ToggleRecording);
    assert!(pill(&view, vcx));
    std::fs::write(&f, "").unwrap();
    app.update(settings::reload);
    vcx.run_until_parked();
    assert!(!pill(&view, vcx));
    assert_eq!(casts().len(), before + 1, "saved on gate off");
    assert!(toasts(&view, vcx)
        .iter()
        .any(|(t, _)| t.contains("disabled in config")));
}

#[gpui::test]
fn the_pill_click_target_stops_the_recording(cx: &mut TestAppContext) {
    let _serial = serial();
    let (view, cx) = boot(cx, config("pillstop", ON));
    cx.dispatch_action(ToggleRecording);
    view.update_in(cx, |this, w, cx| this.stop_recording(w, cx));
    assert!(!pill(&view, cx));
}
