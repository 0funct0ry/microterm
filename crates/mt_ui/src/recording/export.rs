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

pub struct ExportDialog {
    pub sidecar: Sidecar,
    pub cast_path: PathBuf,
    /// Index into [`mt_export::registry`].
    pub format: usize,
    /// `[format][option]` as the text shown in each field.
    pub fields: Vec<Vec<TextEdit>>,
    pub path: TextEdit,
    pub path_dirty: bool,
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
                    .map(|s| TextEdit::new(shown(s, &opts)))
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

    fn default_path(&self) -> String {
        let name = format!(
            "{}.{}",
            mt_export::slug(&self.sidecar.title),
            self.exporter().extension()
        );
        self.dir.join(name).to_string_lossy().into_owned()
    }

    /// Re-derives the output path for the current format unless the user typed one.
    fn refresh_path(&mut self) {
        if !self.path_dirty {
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
        let (path_focus, f) = (self.path_focus(), self.focus);
        if f == path_focus {
            return Some(&mut self.path);
        }
        if f >= 1 && f < path_focus {
            let kind = self.specs()[f - 1].kind.clone();
            if matches!(kind, OptKind::Bool | OptKind::Enum(_)) {
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
                && self
                    .specs()
                    .get(f - 1)
                    .is_some_and(|s| matches!(s.kind, OptKind::Bool | OptKind::Enum(_))))
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
            let rec = mt_export::Recording::new(sidecar, cast_path);
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
    fn default_format_falls_back_when_unregistered() {
        // The shipped default is "gif", which is not registered until M12.5.
        let d = ExportDialog::new(sidecar(true), "x.cast".into(), &cfg());
        assert!(mt_export::registry()
            .iter()
            .any(|e| e.id() == d.exporter().id()));
        let mut c = cfg();
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
        let again = ExportJob::spawn(2, ex, sidecar(false), cast, opts, out, false, false);
        let r = loop {
            if let Some(r) = again.take_result() {
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(matches!(r, Err(ExportError::Exists(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
