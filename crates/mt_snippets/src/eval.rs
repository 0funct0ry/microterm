//! Compiling and expanding templates (SPEC §10.8.3-§10.8.5).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset};

use crate::analyze::{analyze, Analysis, PromptKind};
use crate::context::Context;
use crate::funcs::{self, Args, Def, Env, Run};
use crate::template::{self, Arg, Ast, Call, Diag, Lit, Node, Pipeline, Span, Term};
use crate::value::{Val, MASK};

/// A parsed and checked template body.
#[derive(Debug, Clone)]
pub struct Template {
    pub src: String,
    pub ast: Ast,
    pub parse_diags: Vec<Diag>,
    pub analysis: Analysis,
}

impl Template {
    pub fn compile(src: &str) -> Template {
        let parsed = template::parse(src);
        let analysis = if parsed.diags.is_empty() {
            analyze(&parsed.ast)
        } else {
            Analysis::default()
        };
        Template {
            src: src.to_string(),
            ast: parsed.ast,
            parse_diags: parsed.diags,
            analysis,
        }
    }

    /// Cached by body hash (parsing is cheap, but the panel asks for every row on every frame).
    pub fn cached(src: &str) -> Arc<Template> {
        static CACHE: Mutex<Option<HashMap<u64, Arc<Template>>>> = Mutex::new(None);
        let key = mt_config::edit::content_hash(src);
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(t) = map.get(&key).filter(|t| t.src == src) {
            return t.clone();
        }
        if map.len() >= 512 {
            map.clear();
        }
        let t = Arc::new(Template::compile(src));
        map.insert(key, t.clone());
        t
    }

    /// Every parse and analysis diagnostic, in source order.
    pub fn diagnostics(&self) -> Vec<Diag> {
        let mut d: Vec<Diag> = self
            .parse_diags
            .iter()
            .chain(&self.analysis.diags)
            .cloned()
            .collect();
        d.sort_by_key(|d| (d.span.start, d.span.end));
        d
    }

    pub fn is_valid(&self) -> bool {
        self.parse_diags.is_empty() && self.analysis.diags.is_empty()
    }

    pub fn has_secret_prompt(&self) -> bool {
        self.analysis
            .prompts
            .iter()
            .any(|p| p.kind == PromptKind::Secret)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The real expansion. Missing prompt values are errors (an `input` default fills in).
    Insert,
    /// The fill dialog preview: secrets show as `••••`, missing prompts as `‹Label›`.
    Masked,
    /// The editor preview: every prompt shows as `‹Label›` (secrets as `‹secret›`).
    Placeholder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// Byte offset of `{{cursor}}` in `text`.
    pub cursor: Option<usize>,
    pub has_secret: bool,
}

impl Expanded {
    pub fn literal(body: &str) -> Self {
        Self {
            text: body.to_string(),
            cursor: None,
            has_secret: false,
        }
    }

    /// Unicode scalars after the cursor marker (what the insert path moves left over).
    pub fn cursor_tail_chars(&self) -> Option<usize> {
        self.cursor.map(|c| self.text[c..].chars().count())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub span: Span,
    /// Source text of the failing tag part, e.g. `git_branch`.
    pub tag: String,
    pub reason: String,
    /// Set when the failure is a prompt value that was not supplied.
    pub missing: Option<String>,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{{{ {} }}}}: {}", self.tag, self.reason)
    }
}

/// State shared by every evaluation of one insert: the clock, prompt values, per-call-site
/// results (so the preview is exactly what gets inserted) and counters.
pub struct Session {
    now: DateTime<FixedOffset>,
    prompts: HashMap<String, Val>,
    memo: HashMap<(usize, String), String>,
    /// Next value of each counter touched in this session.
    counters: HashMap<String, i64>,
}

impl Session {
    pub fn new(ctx: &dyn Context) -> Self {
        Self {
            now: ctx.now(),
            prompts: HashMap::new(),
            memo: HashMap::new(),
            counters: HashMap::new(),
        }
    }

    pub fn set_prompt(&mut self, label: &str, value: Val) {
        self.prompts.insert(label.to_string(), value);
    }

    pub fn prompt(&self, label: &str) -> Option<&Val> {
        self.prompts.get(label)
    }

    /// A session for another pane of a broadcast: same clock and prompt values, fresh results.
    pub fn fork(&self) -> Session {
        Session {
            now: self.now,
            prompts: self.prompts.clone(),
            memo: HashMap::new(),
            counters: HashMap::new(),
        }
    }

    /// Advances the counters this session used. Call once, after the insert succeeded.
    pub fn commit(&self, ctx: &mut dyn Context) {
        for (name, next) in &self.counters {
            ctx.counter_commit(name, *next);
        }
    }
}

struct Ev<'a> {
    tpl: &'a Template,
    ctx: &'a mut dyn Context,
    sess: &'a mut Session,
    mode: Mode,
    binds: HashMap<String, Val>,
    cursor_pending: bool,
}

pub fn expand(
    tpl: &Template,
    ctx: &mut dyn Context,
    sess: &mut Session,
    mode: Mode,
) -> Result<Expanded, RuntimeError> {
    if let Some(d) = tpl.diagnostics().first() {
        return Err(RuntimeError {
            span: d.span,
            tag: d.span.slice(&tpl.src).to_string(),
            reason: format!("template error: {}", d.message),
            missing: None,
        });
    }
    let mut ev = Ev {
        tpl,
        ctx,
        sess,
        mode,
        binds: HashMap::new(),
        cursor_pending: false,
    };
    let mut out = String::new();
    let mut cursor = None;
    for node in &tpl.ast.nodes {
        match node {
            Node::Text(t, _) => out.push_str(t),
            Node::Tag(tag) => {
                let (val, emit) = ev.pipeline(&tag.pipeline)?;
                if let Some((name, _)) = &tag.bind {
                    ev.binds.insert(name.clone(), val.clone());
                }
                if std::mem::take(&mut ev.cursor_pending) {
                    cursor = Some(out.len());
                }
                if emit {
                    out.push_str(val.expose());
                }
            }
        }
    }
    Ok(Expanded {
        text: out,
        cursor,
        has_secret: tpl.has_secret_prompt(),
    })
}

impl Ev<'_> {
    fn err(&self, span: Span, reason: impl Into<String>) -> RuntimeError {
        RuntimeError {
            span,
            tag: span.slice(&self.tpl.src).to_string(),
            reason: reason.into(),
            missing: None,
        }
    }

    fn pipeline(&mut self, p: &Pipeline) -> Result<(Val, bool), RuntimeError> {
        let mut v = match &p.first {
            Term::Var(name, span) => self
                .binds
                .get(name)
                .cloned()
                .ok_or_else(|| self.err(*span, format!("`${name}` is not bound")))?,
            Term::Call(c) => self.call(c, None)?,
        };
        let mut emit = true;
        for f in &p.filters {
            if f.name == "silent" {
                emit = false;
            }
            v = self.call(f, Some(&v))?;
        }
        Ok((v, emit))
    }

    fn call(&mut self, c: &Call, input: Option<&Val>) -> Result<Val, RuntimeError> {
        let Some(def) = funcs::lookup(&c.name) else {
            return Err(self.err(c.name_span, format!("unknown `{}`", c.name)));
        };
        let is_prompt = matches!(def.run, Run::Input | Run::Choice | Run::Secret);
        let fallback = (!is_prompt)
            .then(|| {
                c.named
                    .iter()
                    .find(|(k, ..)| k == "default")
                    .map(|(_, _, l, _)| l.text())
            })
            .flatten();
        match (self.call_inner(def, c, input), fallback) {
            (Err(e), Some(f)) if e.missing.is_none() => Ok(Val::plain(f)),
            (r, _) => r,
        }
    }

    fn call_inner(
        &mut self,
        def: &'static Def,
        c: &Call,
        input: Option<&Val>,
    ) -> Result<Val, RuntimeError> {
        let mut pos = Vec::new();
        for a in &c.args {
            pos.push(match a {
                Arg::Lit(l, _) => Val::plain(l.text()),
                Arg::Sub(p, _) => self.pipeline(p)?.0,
            });
        }
        let named = c
            .named
            .iter()
            .map(|(k, _, l, _)| (k.clone(), Val::plain(Lit::text(l))))
            .collect();
        let args = Args { pos, named };
        let fail = |this: &Self, r: String| this.err(c.span, r);
        match def.run {
            Run::Func(f) => {
                let key = (
                    c.span.start,
                    args.pos
                        .iter()
                        .map(|v| v.expose())
                        .collect::<Vec<_>>()
                        .join("\u{1}"),
                );
                if def.memo {
                    if let Some(hit) = self.sess.memo.get(&key) {
                        return Ok(Val::plain(hit.clone()));
                    }
                }
                let mut env = Env {
                    ctx: &mut *self.ctx,
                    now: self.sess.now,
                };
                let text = f(&mut env, &args).map_err(|e| fail(self, e.0))?;
                if def.memo {
                    self.sess.memo.insert(key, text.clone());
                }
                Ok(if args.any_secret() {
                    Val::secret(text)
                } else {
                    Val::plain(text)
                })
            }
            Run::Filter(f) => {
                let inp = input.ok_or_else(|| fail(self, "a filter needs an input".into()))?;
                let mut env = Env {
                    ctx: &mut *self.ctx,
                    now: self.sess.now,
                };
                let text = f(&mut env, inp.expose(), &args).map_err(|e| fail(self, e.0))?;
                Ok(inp.derive(text))
            }
            Run::Cursor => {
                self.cursor_pending = true;
                Ok(Val::plain(""))
            }
            Run::Input | Run::Choice | Run::Secret => {
                let kind = match def.run {
                    Run::Input => PromptKind::Input,
                    Run::Choice => PromptKind::Choice,
                    _ => PromptKind::Secret,
                };
                self.prompt(kind, args.s(0), c)
            }
            Run::Counter => {
                let key = (c.span.start, args.s(0).to_string());
                if let Some(hit) = self.sess.memo.get(&key) {
                    return Ok(Val::plain(hit.clone()));
                }
                let name = args.s(0).to_string();
                let start = args.named_int("start", 1).map_err(|e| fail(self, e.0))?;
                let step = args.named_int("step", 1).map_err(|e| fail(self, e.0))?;
                let value = self
                    .sess
                    .counters
                    .get(&name)
                    .copied()
                    .or_else(|| self.ctx.counter_get(&name))
                    .unwrap_or(start);
                self.sess.counters.insert(name, value.saturating_add(step));
                self.sess.memo.insert(key, value.to_string());
                Ok(Val::plain(value.to_string()))
            }
        }
    }

    fn prompt(&mut self, kind: PromptKind, label: &str, c: &Call) -> Result<Val, RuntimeError> {
        let placeholder = || match kind {
            PromptKind::Secret => Val::plain("‹secret›"),
            _ => Val::plain(format!("‹{label}›")),
        };
        if self.mode == Mode::Placeholder {
            return Ok(placeholder());
        }
        if let Some(v) = self.sess.prompts.get(label) {
            return Ok(match kind {
                PromptKind::Secret if self.mode == Mode::Masked => Val::secret(MASK),
                PromptKind::Secret => Val::secret(v.expose()),
                _ => Val::plain(v.expose()),
            });
        }
        let default = (kind == PromptKind::Input)
            .then(|| self.tpl.analysis.prompts.iter().find(|p| p.label == label))
            .flatten()
            .and_then(|p| p.default.clone());
        match (default, self.mode) {
            (Some(d), _) => Ok(Val::plain(d)),
            (None, Mode::Masked) => Ok(placeholder()),
            _ => {
                let mut e = self.err(c.span, format!("no value for {} \"{label}\"", kind.name()));
                e.missing = Some(label.to_string());
                Err(e)
            }
        }
    }
}

/// Removes NUL and ESC and normalises `\r\n` before text goes through `Terminal::paste`
/// (SPEC §10.8.5 step 5), so clipboard, selection and output content cannot inject escapes.
pub fn sanitize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|c| *c != '\0' && *c != '\x1b')
        .collect()
}
