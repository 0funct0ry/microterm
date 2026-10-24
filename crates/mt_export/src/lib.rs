//! Recording exporters (SPEC §10.10.6, D13): small pure converters from a stored recording to
//! one output file, found through a static registry. No GPUI, no processes; `gif` replays
//! through the headless `mt_term` emulator.

mod cast;
pub mod docs;
mod gif;
mod options;
mod vhs;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mt_record::{Event, Header, Reader, Sidecar};

pub use options::{OptKind, OptValue, OptionSpec, Options};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// This recording cannot be exported in this format (for example `vhs` without input).
    #[error("{0}")]
    NotPossible(String),
    #[error("{0}")]
    BadOption(String),
    #[error("export cancelled")]
    Cancelled,
    #[error("{0} already exists (use -f to overwrite)")]
    Exists(String),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("recording: {0}")]
    Record(#[from] mt_record::Error),
}

/// What an exporter needs from the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requires(u8);

impl Requires {
    pub const NONE: Requires = Requires(0);
    pub const INPUT: Requires = Requires(1);

    pub fn contains(self, other: Requires) -> bool {
        self.0 & other.0 == other.0
    }
}

pub trait Progress {
    /// Fraction done, 0.0-1.0.
    fn report(&self, fraction: f32);
    fn cancelled(&self) -> bool;
}

pub struct NoProgress;

impl Progress for NoProgress {
    fn report(&self, _: f32) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// A cancel flag with no reporting.
#[derive(Default)]
pub struct CancelFlag(pub AtomicBool);

impl Progress for CancelFlag {
    fn report(&self, _: f32) {}
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub bytes: u64,
    /// Lines of output (`cast`: events, `vhs`: tape commands).
    pub lines: u64,
    /// Dropped byte classes and how many times each occurred.
    pub dropped: BTreeMap<String, u64>,
    /// `gif`: most cells drawn as a fallback box in one frame.
    pub fallback_cells: u64,
    pub warnings: Vec<String>,
}

/// A stored recording: sidecar plus the `.cast` it describes.
pub struct Recording {
    pub sidecar: Sidecar,
    pub path: PathBuf,
    /// Themes `gif` resolves names against (bundled plus user themes); `None` = bundled only.
    pub themes: Option<Arc<mt_theme::Registry>>,
}

impl Recording {
    pub fn new(sidecar: Sidecar, path: impl Into<PathBuf>) -> Self {
        Self {
            sidecar,
            path: path.into(),
            themes: None,
        }
    }

    pub fn with_themes(mut self, themes: Arc<mt_theme::Registry>) -> Self {
        self.themes = Some(themes);
        self
    }

    pub fn reader(&self) -> Result<Reader<BufReader<File>>, ExportError> {
        Ok(Reader::new(BufReader::new(File::open(&self.path)?))?)
    }

    pub fn header(&self) -> Result<Header, ExportError> {
        Ok(self.reader()?.header().clone())
    }

    pub fn events(&self) -> Result<Vec<Event>, ExportError> {
        let r = self.reader()?;
        r.map(|e| e.map_err(ExportError::from)).collect()
    }
}

pub trait Exporter: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn extension(&self) -> &'static str;
    fn options(&self) -> Vec<OptionSpec>;
    fn requires(&self) -> Requires;
    fn export(
        &self,
        rec: &Recording,
        opts: &Options,
        out: &mut dyn Write,
        progress: &dyn Progress,
    ) -> Result<ExportReport, ExportError>;
}

static REGISTRY: [&dyn Exporter; 3] = [&cast::Cast, &vhs::Vhs, &gif::Gif];

pub fn registry() -> &'static [&'static dyn Exporter] {
    &REGISTRY
}

pub fn find(id: &str) -> Option<&'static dyn Exporter> {
    REGISTRY.iter().copied().find(|e| e.id() == id)
}

/// Why `exporter` cannot export `sidecar`'s recording, if it cannot (shown by the dialog).
pub fn unavailable(exporter: &dyn Exporter, sidecar: &Sidecar) -> Option<String> {
    (exporter.requires().contains(Requires::INPUT) && !sidecar.has_input)
        .then(|| "Needs input: this recording has none".to_string())
}

/// Defaults for every option of `exporter`, with `[recording.export]` applied.
pub fn default_options(exporter: &dyn Exporter, cfg: &mt_config::RecordingExport) -> Options {
    let mut o = Options::defaults(&exporter.options());
    let ms = |v: u32| OptValue::Int(i64::from(v));
    match exporter.id() {
        "cast" => o.set("strip_input", OptValue::Bool(cfg.strip_input)),
        "vhs" => {
            o.set("type_gap_ms", ms(cfg.vhs_type_gap_ms));
            o.set("min_sleep_ms", ms(cfg.vhs_min_sleep_ms));
            o.set("sleep_cap_ms", ms(cfg.vhs_sleep_cap_ms));
        }
        "gif" => {
            let secs = |v: f32| OptValue::Float(f64::from(v));
            o.set("fps", ms(cfg.gif_fps));
            o.set("idle_limit", secs(cfg.gif_idle_limit));
            o.set("font_size", ms(cfg.gif_font_size));
            o.set("scale", secs(cfg.gif_scale));
            o.set("chrome", OptValue::Text(cfg.gif_chrome.clone()));
            o.set("end_pause", secs(cfg.gif_end_pause));
            o.set("max_size_mb", ms(cfg.gif_max_size_mb));
        }
        _ => {}
    }
    o
}

/// Runs `exporter` into `path`: writes a sibling temp file and renames it on success; a
/// cancelled or failed export leaves nothing behind. An existing `path` needs `force`.
pub fn export_to_path(
    exporter: &dyn Exporter,
    rec: &Recording,
    opts: &Options,
    path: &Path,
    force: bool,
    progress: &dyn Progress,
) -> Result<ExportReport, ExportError> {
    if let Some(why) = unavailable(exporter, &rec.sidecar) {
        return Err(ExportError::NotPossible(format!(
            "{} export is not possible: {why}",
            exporter.id()
        )));
    }
    if !force && path.exists() {
        return Err(ExportError::Exists(path.display().to_string()));
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".partial");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut f = File::create(&tmp)?;
        let report = exporter.export(rec, opts, &mut f, progress)?;
        f.flush()?;
        if progress.cancelled() {
            return Err(ExportError::Cancelled);
        }
        Ok(report)
    })();
    match result {
        Ok(mut report) => {
            fs::rename(&tmp, path)?;
            report.bytes = fs::metadata(path)?.len();
            Ok(report)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// The registry as JSON: id, label, extension, `requires_input` and every option with its
/// type, range and default (`recording.formats`, `--list-formats`, generated docs).
pub fn formats_json() -> serde_json::Value {
    use serde_json::json;
    let opt = |s: &OptionSpec| {
        let (ty, extra) = match &s.kind {
            OptKind::Bool => ("bool", json!({})),
            OptKind::Int { min, max } => ("int", json!({"min": min, "max": max})),
            OptKind::Float { min, max } => ("float", json!({"min": min, "max": max})),
            OptKind::Enum(v) => ("enum", json!({"values": v})),
            OptKind::Text => ("text", json!({})),
        };
        let mut o = json!({"key": s.key, "label": s.label, "help": s.help, "type": ty,
            "default": s.default.to_string()});
        if let (Some(o), Some(e)) = (o.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        o
    };
    let formats: Vec<_> = registry()
        .iter()
        .map(|e| {
            json!({
                "id": e.id(), "label": e.label(), "extension": e.extension(),
                "requires_input": e.requires().contains(Requires::INPUT),
                "options": e.options().iter().map(opt).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "formats": formats })
}

/// `<title>` as a file-name stem: lowercase ASCII words joined by `-`.
pub fn slug(title: &str) -> String {
    let mut s = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() {
        "recording".into()
    } else {
        s
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use mt_record::Writer;

    /// Writes a cast with `events` and returns a `Recording` for it.
    pub fn recording(
        dir: &Path,
        has_input: bool,
        build: impl FnOnce(&mut Writer<File>),
    ) -> Recording {
        let path = dir.join("r_test.cast");
        let mut h = Header::new(80, 24);
        h.title = Some("t".into());
        let mut w = Writer::new(File::create(&path).unwrap(), &h).unwrap();
        build(&mut w);
        w.flush().unwrap();
        let sidecar = Sidecar {
            id: "r_test".into(),
            title: "Test run".into(),
            has_input,
            ..Default::default()
        };
        Recording::new(sidecar, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_cast_vhs_and_gif() {
        let ids: Vec<_> = registry().iter().map(|e| e.id()).collect();
        assert_eq!(ids, ["cast", "vhs", "gif"]);
        assert!(find("vhs").is_some());
        assert!(find("nope").is_none());
        assert_eq!(find("vhs").unwrap().requires(), Requires::INPUT);
    }

    #[test]
    fn slug_is_a_file_stem() {
        assert_eq!(slug("zsh — 2026-01-02 10:00"), "zsh-2026-01-02-10-00");
        assert_eq!(slug("!!!"), "recording");
    }

    #[test]
    fn vhs_without_input_is_not_possible_and_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let rec = testutil::recording(d.path(), false, |w| w.output(0.1, b"x").unwrap());
        let out = d.path().join("a.tape");
        let e = export_to_path(
            find("vhs").unwrap(),
            &rec,
            &default_options(find("vhs").unwrap(), &Default::default()),
            &out,
            false,
            &NoProgress,
        )
        .unwrap_err();
        assert!(matches!(e, ExportError::NotPossible(_)), "{e}");
        assert!(!out.exists());
        assert!(!d.path().join("a.tape.partial").exists());
    }

    #[test]
    fn refuses_overwrite_without_force_and_cancel_deletes_partial() {
        let d = tempfile::tempdir().unwrap();
        let rec = testutil::recording(d.path(), true, |w| w.output(0.1, b"x").unwrap());
        let cast = find("cast").unwrap();
        let opts = default_options(cast, &Default::default());
        let out = d.path().join("o.cast");
        export_to_path(cast, &rec, &opts, &out, false, &NoProgress).unwrap();
        let e = export_to_path(cast, &rec, &opts, &out, false, &NoProgress).unwrap_err();
        assert!(matches!(e, ExportError::Exists(_)));
        export_to_path(cast, &rec, &opts, &out, true, &NoProgress).unwrap();

        let cancel = CancelFlag(AtomicBool::new(true));
        let out2 = d.path().join("c.cast");
        let e = export_to_path(cast, &rec, &opts, &out2, false, &cancel).unwrap_err();
        assert!(matches!(e, ExportError::Cancelled));
        assert!(!out2.exists() && !d.path().join("c.cast.partial").exists());
    }
}
