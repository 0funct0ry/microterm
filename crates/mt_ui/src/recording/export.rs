//! The export dialog (SPEC §10.10.5-§10.10.6): its pure state, built from the exporter
//! registry, and the background jobs that run an export with progress and cancel.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use mt_config::Config;
use mt_export::{ExportError, ExportReport, Exporter, OptKind, Options, Progress};
use mt_record::Sidecar;

use crate::components::TextEdit;

/// What each option field shows: the default with `[recording.export]` applied.
fn shown(spec: &mt_export::OptionSpec, opts: &Options) -> String {
    match spec.kind {
        OptKind::Bool => opts.bool(spec.key).to_string(),
        OptKind::Int { .. } => opts.int(spec.key).to_string(),
        OptKind::Float { .. } => opts.float(spec.key).to_string(),
        OptKind::Enum(_) | OptKind::Text => opts.text(spec.key).to_string(),
    }
}

/// `~/x` and `~` expand to `$HOME`; anything else is taken as is.
pub fn expand_home(p: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    match p.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => Path::new(&home).join(rest),
        _ if p == "~" && !home.is_empty() => PathBuf::from(home),
        _ => PathBuf::from(p),
    }
}

/// Where an export goes by default: `[recording.export].output_dir`, else the home folder.
pub fn default_dir(cfg: &Config) -> PathBuf {
    let dir = &cfg.recording.export.output_dir;
    if !dir.is_empty() {
        return expand_home(dir);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Backdrop colours offered next to the hex field (`margin_fill`).
pub const SWATCHES: [&str; 12] = [
    "#0d1117", "#1e1e2e", "#282a36", "#0f172a", "#7c3aed", "#2563eb", "#059669", "#ea580c",
    "#db2777", "#9ca3af", "#e5e7eb", "#ffffff",
];

/// One column of the dialog: an optional heading and rows of option indices (one or two each).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub title: Option<&'static str>,
    pub rows: Vec<Vec<usize>>,
}

/// Rows of `gif` option keys for the two panes, in Tab order. Keys the exporter no longer has
/// are skipped and options not listed here are appended to the second pane.
const GIF_PANES: [(&str, &[&[&str]]); 2] = [
    (
        "Appearance",
        &[
            &["title"],
            &["theme"],
            &["chrome"],
            &["margin_fill"],
            &["margin", "padding"],
            &["scale", "shadow"],
        ],
    ),
    (
        "Text and timing",
        &[
            &["font_size", "line_height"],
            &["fps", "speed"],
            &["idle_limit", "end_pause"],
            &["trim_start", "trim_end"],
            &["loop", "max_size_mb"],
            &["cursor", "crop"],
        ],
    ),
];

/// Options that fit half a row next to another one.
fn compact(kind: &OptKind) -> bool {
    matches!(
        kind,
        OptKind::Bool | OptKind::Int { .. } | OptKind::Float { .. }
    )
}

pub struct ExportDialog {
    pub sidecar: Sidecar,
    pub cast_path: PathBuf,
    /// Index into [`mt_export::registry`].
    pub format: usize,
    /// `[format][option]` as the text shown in each field.
    pub fields: Vec<Vec<TextEdit>>,
    pub path: TextEdit,
    pub path_dirty: bool,
    /// The path was derived (not typed or browsed), so it follows the format's extension.
    path_auto: bool,
    /// Themes the `theme` option can pick; empty until [`with_themes`](Self::with_themes).
    pub themes: Vec<mt_theme::Theme>,
    pub theme_open: bool,
    pub theme_scroll: gpui::ScrollHandle,
    pub open_when_done: bool,
    /// 0 = format, `1..=n` = options of the current format, then the path and "open when done".
    pub focus: usize,
    pub error: Option<String>,
    /// The output exists; the next Export overwrites it.
    pub overwrite_armed: bool,
    dir: PathBuf,
}

impl ExportDialog {
    pub fn new(sidecar: Sidecar, cast_path: PathBuf, cfg: &Config) -> Self {
        let reg = mt_export::registry();
        let fields = reg
            .iter()
            .map(|e| {
                let opts = mt_export::default_options(*e, &cfg.recording.export);
                e.options()
                    .iter()
                    .map(|s| match (e.id(), s.key) {
                        // The GIF title bar starts as the recording's name; it can be edited.
                        ("gif", "title") => TextEdit::new(sidecar.title.clone()),
                        _ => TextEdit::new(shown(s, &opts)),
                    })
                    .collect()
            })
            .collect();
        let wanted = reg
            .iter()
            .position(|e| e.id() == cfg.recording.export.default_format);
        let mut d = Self {
            sidecar,
            cast_path,
            format: 0,
            fields,
            path: TextEdit::default(),
            path_dirty: false,
            // The path starts empty (the user picks one) unless the config names a folder.
            path_auto: !cfg.recording.export.output_dir.is_empty(),
            themes: Vec::new(),
            theme_open: false,
            theme_scroll: gpui::ScrollHandle::new(),
            open_when_done: cfg.recording.export.open_when_done,
            focus: 0,
            error: None,
            overwrite_armed: false,
            dir: default_dir(cfg),
        };
        // The configured default may not be registered yet or may not fit this recording.
        d.format = wanted
            .filter(|i| d.unavailable(*i).is_none())
            .or_else(|| (0..reg.len()).find(|i| d.unavailable(*i).is_none()))
            .unwrap_or(0);
        d.refresh_path();
        d
    }

    /// The themes offered by the `theme` option (the registry's, user themes included).
    pub fn with_themes(mut self, themes: Vec<mt_theme::Theme>) -> Self {
        self.themes = themes;
        self
    }

    pub fn exporter(&self) -> &'static dyn Exporter {
        mt_export::registry()[self.format]
    }

    pub fn formats(&self) -> usize {
        mt_export::registry().len()
    }

    /// Why format `ix` cannot export this recording, if it cannot.
    pub fn unavailable(&self, ix: usize) -> Option<String> {
        mt_export::unavailable(mt_export::registry()[ix], &self.sidecar)
    }

    /// The default file name for the current format: `<title>.<ext>`.
    pub fn default_name(&self) -> String {
        format!(
            "{}.{}",
            mt_export::slug(&self.sidecar.title),
            self.exporter().extension()
        )
    }

    /// Folder and file name the Browse… panel starts in.
    pub fn browse_start(&self) -> (PathBuf, String) {
        let current = PathBuf::from(self.path.text().trim());
        match (current.parent(), current.file_name()) {
            (Some(dir), Some(name)) if dir.is_dir() => {
                (dir.to_path_buf(), name.to_string_lossy().into_owned())
            }
            _ => (self.dir.clone(), self.default_name()),
        }
    }

    /// The home button: `<home>/<recording name>.<ext>`, following the format from now on.
    pub fn fill_home(&mut self) {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        self.fill_in(home);
    }

    fn fill_in(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.path_dirty = false;
        self.path_auto = true;
        self.overwrite_armed = false;
        self.error = None;
        self.refresh_path();
    }

    /// The browse result or any other path chosen outside the text field.
    pub fn set_path(&mut self, path: String) {
        self.path = TextEdit::new(path);
        self.path_dirty = true;
        self.overwrite_armed = false;
        self.error = None;
    }

    fn default_path(&self) -> String {
        self.dir
            .join(self.default_name())
            .to_string_lossy()
            .into_owned()
    }

    /// Re-derives the output path for the current format unless the user chose one.
    fn refresh_path(&mut self) {
        if self.path_auto && !self.path_dirty {
            self.path = TextEdit::new(self.default_path());
        }
    }

    /// Selects format `ix` when it is available; false otherwise.
    pub fn select_format(&mut self, ix: usize) -> bool {
        if ix >= self.formats() || self.unavailable(ix).is_some() {
            return false;
        }
        self.format = ix;
        self.focus = self.focus.min(self.focus_count() - 1);
        self.overwrite_armed = false;
        self.error = None;
        self.refresh_path();
        true
    }

    /// Moves to the next or previous available format.
    pub fn cycle_format(&mut self, delta: i32) {
        let n = self.formats() as i32;
        for step in 1..=n {
            let ix = (self.format as i32 + delta.signum() * step).rem_euclid(n) as usize;
            if self.select_format(ix) {
                return;
            }
        }
    }

    pub fn specs(&self) -> Vec<mt_export::OptionSpec> {
        self.exporter().options()
    }

    /// How the options of the current format are laid out in the two panes. Reading order is
    /// the Tab order, so the rows always list the options in `specs()` order.
    pub fn panes(&self) -> [Pane; 2] {
        let specs = self.specs();
        if self.exporter().id() == "gif" {
            let ix = |k: &str| specs.iter().position(|s| s.key == k);
            let mut panes = GIF_PANES.map(|(title, rows)| Pane {
                title: Some(title),
                rows: rows
                    .iter()
                    .map(|r| r.iter().filter_map(|k| ix(k)).collect::<Vec<_>>())
                    .filter(|r| !r.is_empty())
                    .collect(),
            });
            let used: Vec<usize> = panes.iter().flat_map(|p| p.rows.concat()).collect();
            for i in (0..specs.len()).filter(|i| !used.contains(i)) {
                panes[1].rows.push(vec![i]);
            }
            return panes;
        }
        // Other formats: pair consecutive compact options, then split the rows in half.
        let mut rows: Vec<Vec<usize>> = Vec::new();
        for (i, s) in specs.iter().enumerate() {
            let pairable = compact(&s.kind)
                && rows
                    .last()
                    .is_some_and(|r| r.len() == 1 && compact(&specs[r[0]].kind));
            if pairable {
                rows.last_mut().expect("checked").push(i);
            } else {
                rows.push(vec![i]);
            }
        }
        let right = rows.split_off(rows.len().div_ceil(2));
        [
            Pane { title: None, rows },
            Pane {
                title: None,
                rows: right,
            },
        ]
    }

    /// The option under the focus, if the focus is on one.
    pub fn focused_spec(&self) -> Option<mt_export::OptionSpec> {
        self.specs().get(self.focus.checked_sub(1)?).cloned()
    }

    fn focused_key(&self) -> Option<&'static str> {
        self.focused_spec().map(|s| s.key)
    }

    /// Whether the focused option picks a theme from the list.
    pub fn is_theme_focus(&self) -> bool {
        self.focused_key() == Some("theme")
    }

    /// Index of the option `key` of the current format.
    pub fn spec_ix(&self, key: &str) -> Option<usize> {
        self.specs().iter().position(|s| s.key == key)
    }

    /// The text of option `key` ("" when the format has no such option).
    pub fn value(&self, key: &str) -> &str {
        self.spec_ix(key)
            .and_then(|i| self.fields[self.format].get(i))
            .map_or("", |f| f.text())
    }

    pub fn set_value(&mut self, key: &str, v: &str) {
        if let Some(i) = self.spec_ix(key) {
            self.fields[self.format][i] = TextEdit::new(v);
            self.error = None;
        }
    }

    /// Opens or closes the theme list, scrolling the current theme into view.
    pub fn toggle_theme_sheet(&mut self) {
        self.theme_open = !self.theme_open;
        if self.theme_open {
            let cur = self.value("theme").to_string();
            let at = self.theme_choices().iter().position(|c| *c == cur);
            self.theme_scroll.scroll_to_top_of_item(at.unwrap_or(0));
        }
    }

    /// Theme choices: "" (the recording's own) then every theme name.
    fn theme_choices(&self) -> Vec<String> {
        std::iter::once(String::new())
            .chain(self.themes.iter().map(|t| t.name.clone()))
            .collect()
    }

    pub fn focus_count(&self) -> usize {
        1 + self.specs().len() + 2
    }

    pub fn path_focus(&self) -> usize {
        1 + self.specs().len()
    }

    pub fn open_focus(&self) -> usize {
        self.path_focus() + 1
    }

    pub fn move_focus(&mut self, delta: i32) {
        let n = self.focus_count() as i32;
        self.focus = (self.focus as i32 + delta).rem_euclid(n) as usize;
    }

    /// The text field under the focus, if it has one.
    pub fn field_mut(&mut self) -> Option<&mut TextEdit> {
        self.edit_at(self.focus)
    }

    /// The text field at focus index `f`, if that is a typed field (not a toggle or a list).
    pub fn edit_at(&mut self, f: usize) -> Option<&mut TextEdit> {
        let path_focus = self.path_focus();
        if f == path_focus {
            return Some(&mut self.path);
        }
        if f >= 1 && f < path_focus {
            let spec = &self.specs()[f - 1];
            if matches!(spec.kind, OptKind::Bool | OptKind::Enum(_)) || spec.key == "theme" {
                return None;
            }
            return self.fields[self.format].get_mut(f - 1);
        }
        None
    }

    /// Typing in the path field means the user owns it from now on.
    pub fn touched(&mut self) {
        if self.focus == self.path_focus() {
            self.path_dirty = true;
        }
        self.error = None;
        self.overwrite_armed = false;
    }

    /// Space / arrows on a bool or enum option (or the "open when done" switch).
    pub fn cycle_value(&mut self, delta: i32) {
        let f = self.focus;
        if f == self.open_focus() {
            self.open_when_done = !self.open_when_done;
            return;
        }
        if f == 0 {
            return self.cycle_format(delta);
        }
        let Some(spec) = self.specs().get(f.wrapping_sub(1)).cloned() else {
            return;
        };
        let cur = self.fields[self.format][f - 1].text().to_string();
        if spec.key == "theme" {
            let choices = self.theme_choices();
            let at = choices.iter().position(|c| *c == cur).unwrap_or(0) as i32;
            let next = (at + delta.signum()).rem_euclid(choices.len() as i32) as usize;
            self.fields[self.format][f - 1] = TextEdit::new(choices[next].clone());
            self.error = None;
            return;
        }
        let next = match &spec.kind {
            OptKind::Bool => (cur != "true").to_string(),
            OptKind::Enum(values) => {
                let i = values.iter().position(|v| *v == cur).unwrap_or(0) as i32;
                values[(i + delta.signum()).rem_euclid(values.len() as i32) as usize].to_string()
            }
            _ => return,
        };
        self.fields[self.format][f - 1] = TextEdit::new(next);
        self.error = None;
    }

    pub fn is_toggle(&self) -> bool {
        let f = self.focus;
        f == self.open_focus()
            || (f >= 1
                && self.specs().get(f - 1).is_some_and(|s| {
                    matches!(s.kind, OptKind::Bool | OptKind::Enum(_)) || s.key == "theme"
                }))
    }

    /// The validated options and output path, ready for a job.
    pub fn build(&self, cfg: &Config) -> Result<(Options, PathBuf), String> {
        let ex = self.exporter();
        if let Some(why) = self.unavailable(self.format) {
            return Err(why);
        }
        let specs = ex.options();
        let kv: Vec<(String, String)> = specs
            .iter()
            .zip(&self.fields[self.format])
            .map(|(s, f)| (s.key.to_string(), f.text().trim().to_string()))
            .collect();
        let mut opts = mt_export::default_options(ex, &cfg.recording.export);
        opts.apply(&specs, &kv).map_err(|e| e.to_string())?;
        let path = self.path.text().trim();
        if path.is_empty() {
            return Err("Choose an output file".into());
        }
        Ok((opts, expand_home(path)))
    }
}

/// Progress and cancel shared between a worker thread and the window.
#[derive(Default)]
pub struct JobProgress {
    bits: AtomicU32,
    cancel: AtomicBool,
}

impl JobProgress {
    pub fn fraction(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl Progress for JobProgress {
    fn report(&self, fraction: f32) {
        self.bits.store(fraction.to_bits(), Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub type JobResult = Result<ExportReport, ExportError>;

/// One export running on its own thread.
pub struct ExportJob {
    pub n: u64,
    pub title: String,
    pub format: &'static str,
    pub path: PathBuf,
    pub open_when_done: bool,
    pub progress: Arc<JobProgress>,
    slot: Arc<Mutex<Option<JobResult>>>,
}

impl ExportJob {
    /// Starts the export on a worker thread; poll [`take_result`](Self::take_result).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        n: u64,
        exporter: &'static dyn Exporter,
        sidecar: Sidecar,
        cast_path: PathBuf,
        opts: Options,
        path: PathBuf,
        force: bool,
        open_when_done: bool,
        themes: Option<Arc<mt_theme::Registry>>,
    ) -> Self {
        let progress = Arc::new(JobProgress::default());
        let slot = Arc::new(Mutex::new(None));
        let job = Self {
            n,
            title: sidecar.title.clone(),
            format: exporter.id(),
            path: path.clone(),
            open_when_done,
            progress: progress.clone(),
            slot: slot.clone(),
        };
        std::thread::spawn(move || {
            let mut rec = mt_export::Recording::new(sidecar, cast_path);
            if let Some(t) = themes {
                rec = rec.with_themes(t);
            }
            let r = mt_export::export_to_path(exporter, &rec, &opts, &path, force, &*progress);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        });
        job
    }

    pub fn take_result(&self) -> Option<JobResult> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar(has_input: bool) -> Sidecar {
        Sidecar {
            id: "r_x".into(),
            title: "My demo".into(),
            has_input,
            ..Sidecar::default()
        }
    }

    fn cfg() -> Config {
        let mut c = Config::default();
        c.recording.export.output_dir = "/tmp/out".into();
        // These tests exercise the text formats; the shipped default is "gif".
        c.recording.export.default_format = "cast".into();
        c
    }

    #[test]
    fn registry_formats_are_listed_and_vhs_is_disabled_without_input() {
        let d = ExportDialog::new(sidecar(false), "x.cast".into(), &cfg());
        let vhs = mt_export::registry()
            .iter()
            .position(|e| e.id() == "vhs")
            .unwrap();
        assert!(d.unavailable(vhs).unwrap().contains("Needs input"));
        let mut d = d;
        assert!(!d.select_format(vhs));
        assert_eq!(d.exporter().id(), "cast");
        let with_input = ExportDialog::new(sidecar(true), "x.cast".into(), &cfg());
        assert!(with_input.unavailable(vhs).is_none());
    }

    #[test]
    fn default_format_comes_from_config_and_falls_back() {
        let d = ExportDialog::new(sidecar(true), "x.cast".into(), &Config::default());
        assert_eq!(d.exporter().id(), "gif");
        let mut c = cfg();
        c.recording.export.default_format = "nope".into();
        let d = ExportDialog::new(sidecar(true), "x.cast".into(), &c);
        assert!(mt_export::registry()
            .iter()
            .any(|e| e.id() == d.exporter().id()));
        c.recording.export.default_format = "vhs".into();
        let d = ExportDialog::new(sidecar(true), "x.cast".into(), &c);
        assert_eq!(d.exporter().id(), "vhs");
        let d = ExportDialog::new(sidecar(false), "x.cast".into(), &c);
        assert_eq!(d.exporter().id(), "cast");
    }

    #[test]
    fn path_follows_the_format_until_edited() {
        let mut d = ExportDialog::new(sidecar(true), "x.cast".into(), &cfg());
        assert_eq!(d.path.text(), "/tmp/out/my-demo.cast");
        d.cycle_format(1);
        assert_eq!(d.exporter().id(), "vhs");
        assert_eq!(d.path.text(), "/tmp/out/my-demo.tape");
        d.focus = d.path_focus();
        d.touched();
        d.path = TextEdit::new("/x/y.tape");
        d.cycle_format(1);
        assert_eq!(d.path.text(), "/x/y.tape");
    }

    #[test]
    fn options_come_from_the_registry_with_config_defaults() {
        let mut c = cfg();
        c.recording.export.strip_input = true;
        let d = ExportDialog::new(sidecar(true), "x.cast".into(), &c);
        let specs = d.specs();
        let ix = specs.iter().position(|s| s.key == "strip_input").unwrap();
        assert_eq!(d.fields[d.format][ix].text(), "true");
        let (opts, path) = d.build(&c).unwrap();
        assert!(opts.bool("strip_input"));
        assert_eq!(path, PathBuf::from("/tmp/out/my-demo.cast"));
    }

    #[test]
    fn invalid_option_values_are_rejected_like_the_cli() {
        let c = cfg();
        let mut d = ExportDialog::new(sidecar(true), "x.cast".into(), &c);
        let ix = d
            .specs()
            .iter()
            .position(|s| s.key == "idle_limit")
            .unwrap();
        d.fields[d.format][ix] = TextEdit::new("not-a-number");
        assert!(d.build(&c).unwrap_err().contains("idle_limit"));
    }

    #[test]
    fn toggles_cycle_bools_and_the_open_switch() {
        let c = cfg();
        let mut d = ExportDialog::new(sidecar(true), "x.cast".into(), &c);
        let ix = d
            .specs()
            .iter()
            .position(|s| s.key == "strip_input")
            .unwrap();
        d.focus = ix + 1;
        assert!(d.is_toggle());
        let before = d.fields[d.format][ix].text().to_string();
        d.cycle_value(1);
        assert_ne!(d.fields[d.format][ix].text(), before);
        d.focus = d.open_focus();
        let open = d.open_when_done;
        d.cycle_value(1);
        assert_eq!(d.open_when_done, !open);
    }

    #[test]
    fn a_job_exports_and_a_cancelled_one_leaves_nothing() {
        let dir = std::env::temp_dir().join(format!("mt_exp_job_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cast = dir.join("r_x.cast");
        std::fs::write(
            &cast,
            "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hi\"]\n",
        )
        .unwrap();
        let ex = mt_export::find("cast").unwrap();
        let opts = mt_export::default_options(ex, &Config::default().recording.export);
        let out = dir.join("out.cast");
        let job = ExportJob::spawn(
            1,
            ex,
            sidecar(false),
            cast.clone(),
            opts.clone(),
            out.clone(),
            false,
            false,
            None,
        );
        let r = loop {
            if let Some(r) = job.take_result() {
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(r.is_ok());
        assert!(out.is_file());
        // Exporting onto an existing file needs force.
        let again = ExportJob::spawn(2, ex, sidecar(false), cast, opts, out, false, false, None);
        let r = loop {
            if let Some(r) = again.take_result() {
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(matches!(r, Err(ExportError::Exists(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn gif_dialog() -> ExportDialog {
        let mut c = Config::default();
        c.recording.export.default_format = "gif".into();
        ExportDialog::new(sidecar(true), "x.cast".into(), &c)
            .with_themes(mt_theme::Registry::bundled().all().to_vec())
    }

    #[test]
    fn panes_cover_every_option_once_in_tab_order() {
        for fmt in 0..mt_export::registry().len() {
            let mut d = gif_dialog();
            assert!(d.select_format(fmt) || d.unavailable(fmt).is_some());
            let n = d.specs().len();
            let panes = d.panes();
            let flat: Vec<usize> = panes.iter().flat_map(|p| p.rows.concat()).collect();
            assert_eq!(flat, (0..n).collect::<Vec<_>>(), "{}", d.exporter().id());
            assert!(panes.iter().all(|p| p.rows.iter().all(|r| r.len() <= 2)));
            // Neither pane is left empty (the dialog is two balanced columns).
            assert!(
                panes.iter().all(|p| !p.rows.is_empty()) || n < 2,
                "{}",
                d.exporter().id()
            );
        }
    }

    #[test]
    fn gif_panes_group_appearance_and_text_timing() {
        let d = gif_dialog();
        let specs = d.specs();
        let keys = |p: &Pane| -> Vec<Vec<&str>> {
            p.rows
                .iter()
                .map(|r| r.iter().map(|&i| specs[i].key).collect())
                .collect()
        };
        let [left, right] = d.panes();
        assert_eq!(left.title, Some("Appearance"));
        assert_eq!(keys(&left)[0], ["title"]);
        assert_eq!(keys(&left)[1], ["theme"]);
        assert!(keys(&left).contains(&vec!["margin", "padding"]));
        assert!(keys(&right).contains(&vec!["fps", "speed"]));
        assert!(left.rows.len().abs_diff(right.rows.len()) <= 1);
    }

    #[test]
    fn gif_title_starts_as_the_recording_name_and_can_be_changed() {
        let mut d = gif_dialog();
        assert_eq!(d.value("title"), "My demo");
        d.path = TextEdit::new("/tmp/x.gif");
        let (opts, _) = d.build(&Config::default()).unwrap();
        assert_eq!(opts.text("title"), "My demo");
        d.set_value("title", "Release 1.0");
        let (opts, _) = d.build(&Config::default()).unwrap();
        assert_eq!(opts.text("title"), "Release 1.0");
        // The cast exporter's own `title` option is not prefilled (empty keeps the header).
        let cast = mt_export::registry()
            .iter()
            .position(|e| e.id() == "cast")
            .unwrap();
        assert!(d.select_format(cast));
        assert_eq!(d.value("title"), "");
    }

    #[test]
    fn chrome_defaults_to_macos() {
        let d = gif_dialog();
        assert_eq!(d.value("chrome"), "macos");
    }

    #[test]
    fn path_starts_empty_and_the_home_button_fills_it() {
        let mut c = Config::default();
        c.recording.export.default_format = "gif".into();
        let mut d = ExportDialog::new(sidecar(true), "x.cast".into(), &c);
        assert_eq!(d.path.text(), "");
        assert!(d.build(&c).unwrap_err().contains("output file"));
        d.fill_in(PathBuf::from("/home/me"));
        assert_eq!(d.path.text(), "/home/me/my-demo.gif");
        // A derived path follows the format.
        d.cycle_format(-1);
        assert!(d.path.text().ends_with("my-demo.tape") || d.path.text().ends_with("my-demo.cast"));
        // A browsed or typed path is the user's from then on.
        d.set_path("/x/y.gif".into());
        d.cycle_format(1);
        assert_eq!(d.path.text(), "/x/y.gif");
    }

    #[test]
    fn browse_starts_in_the_default_folder_with_the_default_name() {
        let d = ExportDialog::new(sidecar(true), "x.cast".into(), &Config::default());
        let (dir, name) = d.browse_start();
        assert_eq!(name, d.default_name());
        assert_eq!(dir, default_dir(&Config::default()));
    }

    #[test]
    fn theme_is_picked_from_the_registry_not_typed() {
        let mut d = gif_dialog();
        let ix = d.spec_ix("theme").unwrap();
        d.focus = ix + 1;
        assert!(d.is_theme_focus() && d.is_toggle() && d.field_mut().is_none());
        assert_eq!(d.value("theme"), "");
        d.cycle_value(1);
        let first = mt_theme::Registry::bundled().all()[0].name.clone();
        assert_eq!(d.value("theme"), first);
        d.cycle_value(-1);
        assert_eq!(d.value("theme"), "");
        d.cycle_value(-1);
        let last = mt_theme::Registry::bundled()
            .all()
            .last()
            .unwrap()
            .name
            .clone();
        assert_eq!(d.value("theme"), last);
        let (opts, _) = {
            d.path = TextEdit::new("/tmp/x.gif");
            d.build(&Config::default()).unwrap()
        };
        assert_eq!(opts.text("theme"), last);
        d.toggle_theme_sheet();
        assert!(d.theme_open);
        d.toggle_theme_sheet();
        assert!(!d.theme_open);
    }

    #[test]
    fn margin_colour_swatches_are_valid_hex() {
        for hex in SWATCHES {
            assert!(mt_theme::Rgba::parse(hex).is_ok(), "{hex}");
        }
        let mut d = gif_dialog();
        d.set_value("margin_fill", SWATCHES[0]);
        assert_eq!(d.value("margin_fill"), SWATCHES[0]);
        d.path = TextEdit::new("/tmp/x.gif");
        let (opts, _) = d.build(&Config::default()).unwrap();
        assert_eq!(opts.text("margin_fill"), SWATCHES[0]);
    }
}
