//! `snippet.*` IPC (SPEC §10.8, §12.1): list, get, expand, insert, add, remove. The logic is
//! shared by the GUI and the headless server through [`SnippetHost`]; the gate is checked first.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use mt_config::edit::EditError;
use mt_config::Config;
use mt_mux::ctl::{Rows, Verb};
use mt_proto::{Request, RpcError, SnippetAddParams, SnippetListParams, SnippetUseParams};
use mt_snippets::model::{slugify, unique_id, valid_id};
use mt_snippets::{
    expand, store, Expanded, Mode, Prompt, PromptKind, Session, Snippet, SnippetSet, Template, Val,
};
use mt_term::Terminal;
use serde_json::{json, Value};

use super::{
    context::PaneContext,
    gate::SnippetsGate,
    insert::{self, Delivery},
    panel_state,
};

/// What a host provides to the shared snippet handlers.
pub trait SnippetHost {
    fn config(&self) -> Arc<Config>;
    /// The loaded set (empty while the feature is off).
    fn set(&self) -> Arc<SnippetSet>;
    fn path(&self) -> PathBuf;
    fn rows(&mut self) -> Rows;
    fn terminal(&mut self, pane: u64) -> Option<Arc<Terminal>>;
    /// `(clipboard, font size, theme)`; `None` where the host has no GUI.
    fn gui(&mut self, needs_clipboard: bool) -> (Option<String>, Option<f32>, Option<String>);
    /// `snippets.toml` was written: refresh the set.
    fn reloaded(&mut self);
}

fn prompt_json(p: &Prompt) -> Value {
    let mut v = json!({"label": p.label, "kind": p.kind.name()});
    if p.kind == PromptKind::Choice {
        v["options"] = json!(p.options);
    }
    if let Some(d) = &p.default {
        v["default"] = json!(d);
    }
    v
}

fn snippet_json(s: &Snippet, templates_on: bool, full: bool) -> Value {
    let mut v = json!({
        "id": s.id,
        "title": s.title,
        "folder": s.folder,
        "tags": s.tags,
        "description": s.description,
        "run": s.run,
        "key": s.keys,
    });
    if templates_on {
        let t = Template::cached(&s.body);
        v["prompts"] = Value::Array(t.analysis.prompts.iter().map(prompt_json).collect());
        let diags: Vec<String> = t
            .diagnostics()
            .iter()
            .map(|d| {
                let (l, c) = d.line_col(&s.body);
                format!("{l}:{c}: {}", d.message)
            })
            .collect();
        if !diags.is_empty() {
            v["errors"] = json!(diags);
        }
    }
    if full {
        v["body"] = json!(s.body);
    }
    v
}

/// The snippet (or inline body) a request names, ready to expand.
struct Prepared {
    title: String,
    run: bool,
    body: String,
    template: Option<Arc<Template>>,
    warnings: Vec<String>,
}

fn prepare(
    set: &SnippetSet,
    templates_on: bool,
    p: &SnippetUseParams,
) -> Result<Prepared, RpcError> {
    let (title, run, body) = match (&p.id, &p.body) {
        (Some(id), None) => {
            let s = set
                .get(id)
                .ok_or_else(|| RpcError::not_found(format!("no snippet with id \"{id}\"")))?;
            (s.title.clone(), s.run, s.body.clone())
        }
        (None, Some(b)) => ("inline snippet".to_string(), false, b.replace("\r\n", "\n")),
        (Some(_), Some(_)) => return Err(RpcError::bad_request("give an id or --body, not both")),
        (None, None) => return Err(RpcError::bad_request("give a snippet id or --body TEXT")),
    };
    let mut warnings = Vec::new();
    let template = if templates_on {
        let t = Template::cached(&body);
        if let Some(d) = t.diagnostics().first() {
            let (l, c) = d.line_col(&body);
            return Err(RpcError::template_error(format!(
                "template error at {l}:{c}: {}",
                d.message
            )));
        }
        if p.agent && t.has_secret_prompt() {
            return Err(RpcError::template_error(
                "this snippet asks for a secret, and secrets are never passed through an agent",
            ));
        }
        Some(t)
    } else {
        if !p.vars.is_empty() {
            warnings.push(
                "--var ignored: snippets.templates is off, the body is used literally".into(),
            );
        }
        None
    };
    Ok(Prepared {
        title,
        run,
        body,
        template,
        warnings,
    })
}

/// Checks every prompt has a value; lists all missing ones (`Label (kind)`).
fn check_vars(t: &Template, vars: &BTreeMap<String, String>) -> Result<(), RpcError> {
    let missing: Vec<String> = t
        .analysis
        .prompts
        .iter()
        .filter(|p| {
            let has_default = p.kind == PromptKind::Input && p.default.is_some();
            !vars.contains_key(&p.label) && !has_default
        })
        .map(|p| format!("{} ({})", p.label, p.kind.name()))
        .collect();
    if !missing.is_empty() {
        return Err(RpcError::missing_vars(
            format!("missing values for: {}", missing.join(", ")),
            missing,
        ));
    }
    for p in &t.analysis.prompts {
        if let (PromptKind::Choice, Some(v)) = (p.kind, vars.get(&p.label)) {
            if !p.options.contains(v) {
                return Err(RpcError::bad_request(format!(
                    "\"{v}\" is not an option of \"{}\" (choose one of: {})",
                    p.label,
                    p.options.join(", ")
                )));
            }
        }
    }
    Ok(())
}

fn context(h: &mut dyn SnippetHost, pane: u64, rows: &Rows, needs_clipboard: bool) -> PaneContext {
    let (clipboard, font_size, theme) = h.gui(needs_clipboard);
    PaneContext {
        cfg: h.config(),
        rows: rows.clone(),
        pane: Some(pane),
        term: h.terminal(pane),
        clipboard,
        font_size,
        theme,
    }
}

fn session_for(t: &Template, vars: &BTreeMap<String, String>, ctx: &PaneContext) -> Session {
    let mut s = Session::new(ctx);
    for p in &t.analysis.prompts {
        if let Some(v) = vars.get(&p.label) {
            s.set_prompt(
                &p.label,
                if p.kind == PromptKind::Secret {
                    Val::secret(v)
                } else {
                    Val::plain(v)
                },
            );
        }
    }
    s
}

fn template_failure(e: mt_snippets::RuntimeError) -> RpcError {
    match &e.missing {
        Some(label) => RpcError::missing_vars(e.to_string(), vec![label.clone()]),
        None => RpcError::template_error(e.to_string()),
    }
}

pub fn handle(h: &mut dyn SnippetHost, request: Request) -> Result<Value, RpcError> {
    let gate = SnippetsGate::of_config(&h.config());
    if !gate.enabled {
        return Err(RpcError::disabled(
            "snippets are disabled: set `snippets.enabled = true` in the config",
        ));
    }
    let templates_on = gate.templates_on();
    let set = h.set();
    match request {
        Request::SnippetList(p) => Ok(list(&set, templates_on, &p)),
        Request::SnippetGet(p) => {
            let s = set
                .get(&p.id)
                .ok_or_else(|| RpcError::not_found(format!("no snippet with id \"{}\"", p.id)))?;
            Ok(snippet_json(s, templates_on, true))
        }
        Request::SnippetExpand(p) => {
            let prep = prepare(&set, templates_on, &p)?;
            let rows = h.rows();
            let pane =
                mt_mux::ctl::resolve(&p.target, &rows.panes, rows.focused_pane, Verb::ReadOnly)
                    .map(|v| v[0])
                    .ok();
            let expanded = match (&prep.template, pane) {
                (Some(t), Some(pane)) => {
                    check_vars(t, &p.vars)?;
                    let mut ctx = context(h, pane, &rows, prep.body.contains("clipboard"));
                    let mut sess = session_for(t, &p.vars, &ctx);
                    expand(t, &mut ctx, &mut sess, Mode::Insert).map_err(template_failure)?
                }
                (Some(_), None) => {
                    return Err(RpcError::bad_request(
                        "expanding a template needs a pane: pass --pane or run inside one",
                    ))
                }
                (None, _) => Expanded::literal(&prep.body),
            };
            Ok(json!({
                "text": expanded.text,
                "templated": prep.template.is_some(),
                "run": prep.run,
                "cursor": expanded.cursor_tail_chars(),
                "warnings": prep.warnings,
            }))
        }
        Request::SnippetInsert(p) => {
            let prep = prepare(&set, templates_on, &p)?;
            let rows = h.rows();
            // An input verb: never falls back to the focused pane (SPEC §12.3 rule 4).
            let panes = mt_mux::ctl::resolve(
                &p.target,
                &rows.panes,
                rows.focused_pane,
                Verb::Acting { all_allowed: true },
            )?;
            if let Some(t) = &prep.template {
                check_vars(t, &p.vars)?;
            }
            let run = p.run.unwrap_or(prep.run);
            let mut deliveries: Vec<(u64, Delivery)> = Vec::new();
            let mut first: Option<(PaneContext, Session)> = None;
            for pane in &panes {
                let expanded = match &prep.template {
                    None => Expanded::literal(&prep.body),
                    Some(t) => {
                        let mut ctx = context(h, *pane, &rows, prep.body.contains("clipboard"));
                        let mut sess = match &first {
                            Some((_, s0)) => s0.fork(),
                            None => session_for(t, &p.vars, &ctx),
                        };
                        let e = expand(t, &mut ctx, &mut sess, Mode::Insert)
                            .map_err(template_failure)?;
                        if first.is_none() {
                            first = Some((ctx, sess));
                        }
                        e
                    }
                };
                deliveries.push((*pane, insert::plan(&expanded, run)));
            }
            if let Some((mut ctx, sess)) = first {
                sess.commit(&mut ctx);
            }
            for (pane, d) in &deliveries {
                let term = h
                    .terminal(*pane)
                    .ok_or_else(|| RpcError::internal(format!("pane {pane} has no terminal")))?;
                insert::deliver(&*term, d);
            }
            Ok(json!({
                "title": prep.title,
                "panes": deliveries.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
                "ran": run,
                "warnings": prep.warnings,
            }))
        }
        Request::SnippetAdd(p) => add(h, &set, &p),
        Request::SnippetRemove(p) => {
            let path = h.path();
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if set.get(&p.id).is_none() {
                return Err(RpcError::not_found(format!(
                    "no snippet with id \"{}\"",
                    p.id
                )));
            }
            let base = store::base_of(&text, Some(&p.id));
            store::remove_snippet(&path, &base, &p.id).map_err(write_error)?;
            h.reloaded();
            Ok(json!({"id": p.id}))
        }
        other => Err(RpcError::bad_request(format!(
            "{} is not a snippet method",
            other.method()
        ))),
    }
}

fn list(set: &SnippetSet, templates_on: bool, p: &SnippetListParams) -> Value {
    let order: Vec<usize> = match p.query.as_deref().filter(|q| !q.trim().is_empty()) {
        Some(q) => panel_state::rank(set, q),
        None => (0..set.snippets.len()).collect(),
    };
    let items: Vec<Value> = order
        .into_iter()
        .map(|i| &set.snippets[i])
        .filter(|s| p.folder.as_ref().is_none_or(|f| &s.folder == f))
        .filter(|s| p.tag.as_ref().is_none_or(|t| s.tags.contains(t)))
        .map(|s| snippet_json(s, templates_on, false))
        .collect();
    json!(items)
}

fn write_error(e: EditError) -> RpcError {
    match e {
        EditError::Stale => {
            RpcError::busy("snippets.toml changed while it was being written; try again")
        }
        other => RpcError::bad_request(other.to_string()),
    }
}

fn add(h: &mut dyn SnippetHost, set: &SnippetSet, p: &SnippetAddParams) -> Result<Value, RpcError> {
    let title = p.title.trim();
    if title.is_empty() || title.contains('\n') {
        return Err(RpcError::bad_request(
            "--title must be a non-empty single line",
        ));
    }
    if p.body.is_empty() {
        return Err(RpcError::bad_request("the body must not be empty"));
    }
    if p.body.len() > mt_snippets::model::MAX_BODY_BYTES {
        return Err(RpcError::bad_request("the body is longer than 64 KiB"));
    }
    if let Some(k) = p.key.as_deref().filter(|k| !k.is_empty()) {
        k.parse::<mt_config::keys::Keystroke>()
            .map_err(|e| RpcError::bad_request(format!("--key: {e}")))?;
    }
    let id = match &p.id {
        Some(id) if !valid_id(id) => {
            return Err(RpcError::bad_request(format!(
                "invalid id \"{id}\" (a-z, 0-9, _ and -, at most 64)"
            )))
        }
        Some(id) => id.clone(),
        None => unique_id(&slugify(title), &|c| set.get(c).is_some()),
    };
    let exists = set.get(&id).is_some();
    if exists && !p.replace {
        return Err(RpcError::bad_request(format!(
            "a snippet with id \"{id}\" exists; pass --replace to overwrite it"
        )));
    }
    let snippet = Snippet {
        id: id.clone(),
        explicit_id: true,
        title: title.to_string(),
        body: p.body.replace("\r\n", "\n"),
        folder: p.folder.clone().unwrap_or_default().trim().to_string(),
        tags: p.tags.clone(),
        description: p.description.clone().unwrap_or_default(),
        run: p.run,
        keys: p.key.clone().unwrap_or_default(),
    };
    let path = h.path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let base = store::base_of(&text, exists.then_some(id.as_str()));
    store::save_snippet(&path, &base, &snippet).map_err(write_error)?;
    h.reloaded();
    Ok(json!({"id": id, "created": !exists}))
}

/// The GUI process as a [`SnippetHost`].
pub struct GuiHost<'a>(pub &'a mut gpui::App);

impl SnippetHost for GuiHost<'_> {
    fn config(&self) -> Arc<Config> {
        self.0.global::<crate::settings::Settings>().config.clone()
    }
    fn set(&self) -> Arc<SnippetSet> {
        super::library::set(self.0)
    }
    fn path(&self) -> PathBuf {
        super::library::path(self.0)
    }
    fn rows(&mut self) -> Rows {
        crate::mux_host::rows(self.0)
    }
    fn terminal(&mut self, pane: u64) -> Option<Arc<Terminal>> {
        crate::mux_host::terminal(self.0, pane)
    }
    fn gui(&mut self, needs_clipboard: bool) -> (Option<String>, Option<f32>, Option<String>) {
        let clipboard = needs_clipboard.then(|| {
            self.0
                .read_from_clipboard()
                .and_then(|i| i.text())
                .unwrap_or_default()
        });
        let size = self.config().font.size;
        (
            clipboard,
            Some(size),
            Some(self.0.global::<crate::theme::Theme>().name.to_string()),
        )
    }
    fn reloaded(&mut self) {
        super::library::sync_and_notify(self.0);
    }
}
