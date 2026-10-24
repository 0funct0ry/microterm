//! `recording.*` IPC (SPEC §10.10.7): list, get, start, stop, marker, rename, remove. The
//! logic is shared by the GUI and the headless server through [`RecordingHost`]; the gate is
//! checked first, so a disabled feature never touches the store directory.

use std::sync::Arc;

use mt_config::Config;
use mt_mux::ctl::Rows;
use mt_mux::DomainKind;
use mt_proto::{Request, RpcError};
use mt_record::{Limits, Origin, Store, StoreError};
use mt_term::Terminal;
use serde_json::{json, Value};

use super::engine::{Finished, StartOpts, StoreRecorders};
use super::gate::RecordingGate;

/// What a host provides to the shared recording handlers.
pub trait RecordingHost {
    fn config(&self) -> Arc<Config>;
    fn rows(&mut self) -> Rows;
    fn terminal(&mut self, pane: u64) -> Option<Arc<Terminal>>;
    fn recorders(&mut self) -> &mut StoreRecorders;
    fn domain_kind(&mut self, pane: u64) -> Option<DomainKind>;
    /// `(theme name, font family)` at start; empty where the host has no GUI.
    fn theme_font(&mut self) -> (String, String);
    /// Bundled plus user themes for the `gif` exporter; `None` = bundled only.
    fn themes(&self) -> Option<Arc<mt_theme::Registry>> {
        None
    }
}

pub fn store_error(e: StoreError) -> RpcError {
    match e {
        StoreError::NotFound(id) => RpcError::not_found(format!("no recording with id \"{id}\"")),
        StoreError::BadId(id) => RpcError::bad_request(format!("invalid recording id \"{id}\"")),
        other => RpcError::internal(other.to_string()),
    }
}

pub fn disabled() -> RpcError {
    RpcError::disabled("recording is disabled: set `recording.enabled = true` in the config")
}

fn store() -> Result<Store, RpcError> {
    Store::from_env()
        .ok_or_else(|| RpcError::internal("cannot locate the recordings store: HOME is not set"))
}

fn sidecar_json(st: &Store, s: &mt_record::Sidecar) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
    v["path"] = json!(st.cast_path(&s.id).to_string_lossy());
    v
}

fn finished_json(f: Finished) -> Result<Value, RpcError> {
    f.result
        .map(|s| serde_json::to_value(&s).unwrap_or(Value::Null))
        .map_err(RpcError::internal)
}

fn running_ids(h: &mut dyn RecordingHost) -> Vec<String> {
    h.recorders().running().into_iter().map(|r| r.1).collect()
}

/// Stops every running recording (gate turned off, app quit) and returns what was saved.
pub fn stop_all(h: &mut dyn RecordingHost) -> Vec<Finished> {
    let mut terms = std::collections::HashMap::new();
    for p in h.recorders().panes() {
        if let Some(t) = h.terminal(p) {
            terms.insert(p, t);
        }
    }
    h.recorders().stop_all(|p| terms.get(&p).cloned())
}

pub fn handle(h: &mut dyn RecordingHost, request: Request) -> Result<Value, RpcError> {
    let cfg = h.config();
    if !RecordingGate::of_config(&cfg).enabled {
        return Err(disabled());
    }
    // Recordings that ended by themselves (limit, pane exit) are already saved.
    let _ = h.recorders().reap();
    match request {
        Request::RecordingList => {
            let running = running_ids(h);
            let st = store()?;
            let rows = st
                .list_skipping(&running)
                .map_err(store_error)?
                .iter()
                .map(|s| sidecar_json(&st, s))
                .collect::<Vec<_>>();
            let live: Vec<Value> = h
                .recorders()
                .running()
                .into_iter()
                .map(|(pane, id, el)| {
                    json!({"pane": pane, "id": id, "elapsed_ms": el.as_millis() as u64})
                })
                .collect();
            Ok(json!({"recordings": rows, "running": live}))
        }
        Request::RecordingGet(p) => {
            let st = store()?;
            let s = st.load(&p.id).map_err(store_error)?;
            Ok(sidecar_json(&st, &s))
        }
        Request::RecordingStart(p) => {
            let term = h
                .terminal(p.pane)
                .ok_or_else(|| RpcError::not_found(format!("pane {} does not exist", p.pane)))?;
            let rows = h.rows();
            let row = rows.panes.iter().find(|r| r.id == p.pane);
            let mut origin = Origin {
                kind: "local".into(),
                name: String::new(),
                cwd: row.and_then(|r| r.cwd.clone()).unwrap_or_default(),
                process: row.and_then(|r| r.process.clone()).unwrap_or_default(),
            };
            match h.domain_kind(p.pane) {
                Some(DomainKind::Ssh { name }) => {
                    origin.kind = "ssh".into();
                    origin.name = name;
                }
                Some(DomainKind::Serial { name }) => {
                    origin.kind = "serial".into();
                    origin.name = name;
                }
                _ => {}
            }
            let (theme, font) = h.theme_font();
            let rec = &cfg.recording;
            let opts = StartOpts {
                input: p.input.unwrap_or(rec.capture_input),
                redact_no_echo: rec.redact_no_echo,
                title: p.title,
                origin,
                theme,
                font,
                limits: Limits {
                    max_duration_secs: f64::from(rec.max_duration_minutes) * 60.0,
                    max_bytes: u64::from(rec.max_size_mb) * 1024 * 1024,
                },
            };
            let info = h.recorders().start(&store()?, p.pane, &term, opts)?;
            Ok(json!({"id": info.id, "title": info.title, "pane": p.pane}))
        }
        Request::RecordingStop(p) => {
            if p.all {
                let done = stop_all(h);
                let saved = done
                    .into_iter()
                    .map(finished_json)
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(json!({"stopped": saved}));
            }
            let pane = match (p.pane, &p.id) {
                (Some(pane), _) => pane,
                (None, Some(id)) => h
                    .recorders()
                    .running()
                    .into_iter()
                    .find(|r| &r.1 == id)
                    .map(|r| r.0)
                    .ok_or_else(|| {
                        RpcError::not_found(format!("recording \"{id}\" is not running"))
                    })?,
                (None, None) => {
                    return Err(RpcError::bad_request(
                        "name the recording: --pane ID, a recording id, or --all",
                    ))
                }
            };
            let term = h.terminal(pane);
            let s = h.recorders().stop(pane, term.as_deref())?;
            Ok(json!({"stopped": [serde_json::to_value(&s).unwrap_or(Value::Null)]}))
        }
        Request::RecordingMarker(p) => {
            let term = h
                .terminal(p.pane)
                .ok_or_else(|| RpcError::not_found(format!("pane {} does not exist", p.pane)))?;
            h.recorders().marker(p.pane, &term, p.label.as_deref())?;
            Ok(json!({}))
        }
        Request::RecordingRename(p) => {
            let st = store()?;
            let s = st.rename(&p.id, &p.title).map_err(store_error)?;
            Ok(sidecar_json(&st, &s))
        }
        Request::RecordingRemove(p) => {
            if !p.force {
                return Err(RpcError::confirm_required(format!(
                    "deleting {} recording{} cannot be undone; pass --force",
                    p.ids.len(),
                    if p.ids.len() == 1 { "" } else { "s" }
                )));
            }
            let st = store()?;
            // Check everything first so a typo does not leave half the ids deleted.
            for id in &p.ids {
                st.load(id).map_err(store_error)?;
            }
            for id in &p.ids {
                st.remove(id).map_err(store_error)?;
            }
            Ok(json!({"removed": p.ids}))
        }
        Request::RecordingFormats => Ok(mt_export::formats_json()),
        Request::RecordingExport(p) => export(&cfg, h.themes(), p),
        _ => Err(RpcError::bad_request("not a recording request")),
    }
}

/// `recording.export`: runs the exporter to completion and returns the report. Text formats
/// finish in milliseconds, so there is no job yet; the GUI panel (M12.4) will move this onto a
/// worker thread with `export.progress` events.
fn export(
    cfg: &Config,
    themes: Option<Arc<mt_theme::Registry>>,
    p: mt_proto::RecordingExportParams,
) -> Result<Value, RpcError> {
    let exporter = mt_export::find(&p.format).ok_or_else(|| {
        let ids: Vec<_> = mt_export::registry().iter().map(|e| e.id()).collect();
        RpcError::export_impossible(format!(
            "unknown export format \"{}\" (known: {})",
            p.format,
            ids.join(", ")
        ))
    })?;
    let st = store()?;
    let sidecar = st.load(&p.id).map_err(store_error)?;
    let mut opts = mt_export::default_options(exporter, &cfg.recording.export);
    opts.apply(&exporter.options(), &p.opts)
        .map_err(map_export)?;
    let cwd = std::path::PathBuf::from(&p.cwd);
    let path = match &p.output {
        Some(o) => cwd.join(o),
        None => {
            let dir = &cfg.recording.export.output_dir;
            let dir = if dir.is_empty() { cwd } else { cwd.join(dir) };
            dir.join(format!(
                "{}.{}",
                mt_export::slug(&sidecar.title),
                exporter.extension()
            ))
        }
    };
    if exporter.id() == "vhs" && !p.opts.iter().any(|(k, _)| k == "output") {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("demo");
        opts.set("output", mt_export::OptValue::Text(format!("{stem}.gif")));
    }
    let mut rec = mt_export::Recording::new(sidecar, st.cast_path(&p.id));
    if let Some(t) = themes {
        rec = rec.with_themes(t);
    }
    let report = mt_export::export_to_path(
        exporter,
        &rec,
        &opts,
        &path,
        p.force,
        &mt_export::NoProgress,
    )
    .map_err(map_export)?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "bytes": report.bytes,
        "format": exporter.id(),
        "report": {
            "lines": report.lines,
            "dropped": report.dropped,
            "fallback_cells": report.fallback_cells,
            "warnings": report.warnings,
        },
    }))
}

fn map_export(e: mt_export::ExportError) -> RpcError {
    use mt_export::ExportError as E;
    match e {
        E::NotPossible(m) | E::BadOption(m) => RpcError::export_impossible(m),
        E::Exists(_) => RpcError::bad_request(e.to_string()),
        other => RpcError::internal(other.to_string()),
    }
}

/// The GUI process as a [`RecordingHost`].
pub struct GuiHost<'a>(pub &'a mut gpui::App);

impl RecordingHost for GuiHost<'_> {
    fn config(&self) -> Arc<Config> {
        self.0.global::<crate::settings::Settings>().config.clone()
    }
    fn themes(&self) -> Option<Arc<mt_theme::Registry>> {
        Some(
            self.0
                .global::<crate::settings::Settings>()
                .registry
                .clone(),
        )
    }
    fn rows(&mut self) -> Rows {
        crate::mux_host::rows(self.0)
    }
    fn terminal(&mut self, pane: u64) -> Option<Arc<Terminal>> {
        crate::mux_host::terminal(self.0, pane)
    }
    fn recorders(&mut self) -> &mut StoreRecorders {
        &mut crate::mux_host::host(self.0).store_recorders
    }
    fn domain_kind(&mut self, pane: u64) -> Option<DomainKind> {
        let host = crate::mux_host::host(self.0);
        let domain = host.mux.pane_domain(pane)?;
        host.mux.domain_kind(domain)
    }
    fn theme_font(&mut self) -> (String, String) {
        let font = self.config().font.family.clone();
        (
            self.0.global::<crate::theme::Theme>().name.to_string(),
            font,
        )
    }
}

/// Quits the app after finishing every running recording (SPEC §10.10.2).
pub fn quit(cx: &mut gpui::App) {
    let _ = stop_all(&mut GuiHost(cx));
    cx.quit();
}
