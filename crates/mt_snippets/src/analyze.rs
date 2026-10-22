//! Semantic checks on a parsed template (SPEC §10.8.3): function and filter lookup, arity and
//! argument types, bindings, prompt discovery and the single `cursor`.

use std::collections::HashSet;

use crate::funcs::{self, Def, Kind, Run, Ty};
use crate::template::{Arg, Ast, Call, Diag, DiagKind, Lit, Node, Pipeline, Span, Term};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Input,
    Choice,
    Secret,
}

impl PromptKind {
    pub fn name(self) -> &'static str {
        match self {
            PromptKind::Input => "input",
            PromptKind::Choice => "choice",
            PromptKind::Secret => "secret",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub label: String,
    pub kind: PromptKind,
    /// `input`: the `default=` of the first occurrence.
    pub default: Option<String>,
    /// `choice`: the options of the first occurrence.
    pub options: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Analysis {
    /// In order of first appearance, de-duplicated by label.
    pub prompts: Vec<Prompt>,
    pub has_cursor: bool,
    pub diags: Vec<Diag>,
}

struct A {
    prompts: Vec<Prompt>,
    cursor_seen: bool,
    bound: HashSet<String>,
    diags: Vec<Diag>,
}

fn suggest(unknown: &str, known: impl Iterator<Item = &'static str>) -> String {
    known
        .map(|k| (strsim::damerau_levenshtein(unknown, k), k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map_or(String::new(), |(_, k)| format!(" (did you mean `{k}`?)"))
}

impl A {
    fn diag(&mut self, kind: DiagKind, span: Span, msg: impl Into<String>) {
        self.diags.push(Diag::new(kind, span, msg));
    }

    fn pipeline(&mut self, p: &Pipeline) {
        match &p.first {
            Term::Var(name, span) => {
                if !self.bound.contains(name) {
                    self.diag(
                        DiagKind::UnboundVar,
                        *span,
                        format!("`${name}` is used before it is bound"),
                    );
                }
            }
            Term::Call(c) => self.call(c, Kind::Func),
        }
        for f in &p.filters {
            self.call(f, Kind::Filter);
        }
    }

    fn call(&mut self, c: &Call, want: Kind) {
        // Sub-pipelines are checked whatever happens to the call itself.
        for a in &c.args {
            if let Arg::Sub(p, _) = a {
                self.pipeline(p);
            }
        }
        let Some(def) = funcs::lookup(&c.name) else {
            let (kind, what, known) = match want {
                Kind::Func => (
                    DiagKind::UnknownFunction,
                    "function",
                    funcs::names(Kind::Func),
                ),
                Kind::Filter => (
                    DiagKind::UnknownFilter,
                    "filter",
                    funcs::names(Kind::Filter),
                ),
            };
            let hint = suggest(&c.name, known);
            self.diag(
                kind,
                c.name_span,
                format!("unknown {what} `{}`{hint}", c.name),
            );
            return;
        };
        if def.kind != want {
            let (kind, msg) = match want {
                Kind::Func => (
                    DiagKind::UnknownFunction,
                    format!(
                        "`{}` is a filter; pipe a value into it (`… | {}`)",
                        c.name, c.name
                    ),
                ),
                Kind::Filter => (
                    DiagKind::UnknownFilter,
                    format!("`{}` is a function, not a filter", c.name),
                ),
            };
            self.diag(kind, c.name_span, msg);
            return;
        }
        self.check_args(def, c);
        match def.run {
            Run::Cursor => {
                if self.cursor_seen {
                    self.diag(
                        DiagKind::SecondCursor,
                        c.span,
                        "only one `cursor` is allowed per snippet",
                    );
                }
                self.cursor_seen = true;
            }
            Run::Input | Run::Choice | Run::Secret => self.prompt(def, c),
            _ => {}
        }
    }

    fn check_args(&mut self, def: &Def, c: &Call) {
        let n = c.args.len();
        let required =
            def.params.iter().filter(|p| !p.optional).count() + def.variadic.map_or(0, |v| v.1);
        let max = if def.variadic.is_some() {
            usize::MAX
        } else {
            def.params.len()
        };
        if n < required || n > max {
            let want = if max == usize::MAX {
                format!("at least {required}")
            } else if required == max {
                format!("{required}")
            } else {
                format!("{required} to {max}")
            };
            self.diag(
                DiagKind::Arity,
                c.span,
                format!(
                    "`{}` takes {want} argument{} (got {n}); usage: {}",
                    c.name,
                    if want == "1" { "" } else { "s" },
                    def.sig
                ),
            );
        }
        for (i, a) in c.args.iter().enumerate() {
            let Arg::Lit(lit, span) = a else { continue };
            let Some(p) = def.params.get(i) else { continue };
            self.check_lit(p.ty, p.name, p.choices, lit, *span);
        }
        for (k, kspan, lit, vspan) in &c.named {
            if k == "default" {
                continue;
            }
            match def.named.iter().find(|(n, _)| n == k) {
                Some((_, ty)) => self.check_lit(*ty, k, &[], lit, *vspan),
                None => {
                    let hint = suggest(
                        k,
                        def.named
                            .iter()
                            .map(|(n, _)| *n)
                            .chain(std::iter::once("default")),
                    );
                    self.diag(
                        DiagKind::UnknownArg,
                        *kspan,
                        format!("`{}` has no argument `{k}=`{hint}", c.name),
                    );
                }
            }
        }
        let mut seen = HashSet::new();
        for (k, kspan, ..) in &c.named {
            if !seen.insert(k.as_str()) {
                self.diag(DiagKind::UnknownArg, *kspan, format!("`{k}=` given twice"));
            }
        }
    }

    fn check_lit(&mut self, ty: Ty, name: &str, choices: &[&str], lit: &Lit, span: Span) {
        let ok = matches!(
            (ty, lit),
            (Ty::Str, _) | (Ty::Int, Lit::Int(..)) | (Ty::Bool, Lit::Bool(_))
        );
        if !ok {
            self.diag(
                DiagKind::ArgType,
                span,
                format!("`{name}` expects {}", ty.name()),
            );
        } else if !choices.is_empty() && !choices.contains(&lit.text().as_str()) {
            self.diag(
                DiagKind::ArgType,
                span,
                format!("`{name}` must be one of: {}", choices.join(", ")),
            );
        }
    }

    fn prompt(&mut self, def: &Def, c: &Call) {
        let kind = match def.run {
            Run::Input => PromptKind::Input,
            Run::Choice => PromptKind::Choice,
            _ => PromptKind::Secret,
        };
        let Some(Arg::Lit(label, lspan)) = c.args.first() else {
            if !c.args.is_empty() {
                self.diag(
                    DiagKind::ArgType,
                    c.args[0].span(),
                    format!("the label of `{}` must be a literal", c.name),
                );
            }
            return;
        };
        let mut options = Vec::new();
        for a in c.args.iter().skip(1) {
            match a {
                Arg::Lit(l, _) => options.push(l.text()),
                Arg::Sub(_, s) => {
                    self.diag(DiagKind::ArgType, *s, "choice options must be literals")
                }
            }
        }
        let default = c
            .named
            .iter()
            .find(|(k, ..)| k == "default")
            .map(|(_, _, l, _)| l.text());
        let label = label.text();
        match self.prompts.iter().find(|p| p.label == label) {
            Some(prev) if prev.kind != kind => {
                let msg = format!(
                    "label \"{label}\" is used as both {} and {}",
                    prev.kind.name(),
                    kind.name()
                );
                self.diag(DiagKind::PromptConflict, *lspan, msg);
            }
            Some(_) => {}
            None => self.prompts.push(Prompt {
                label,
                kind,
                default: if kind == PromptKind::Input {
                    default
                } else {
                    None
                },
                options,
                span: c.span,
            }),
        }
    }
}

pub fn analyze(ast: &Ast) -> Analysis {
    let mut a = A {
        prompts: Vec::new(),
        cursor_seen: false,
        bound: HashSet::new(),
        diags: Vec::new(),
    };
    for node in &ast.nodes {
        let Node::Tag(tag) = node else { continue };
        a.pipeline(&tag.pipeline);
        if let Some((name, span)) = &tag.bind {
            if !a.bound.insert(name.clone()) {
                a.diag(
                    DiagKind::Rebound,
                    *span,
                    format!("`{name}` is already bound"),
                );
            }
        }
    }
    // Sub-pipelines are visited before their call, so restore source order.
    a.prompts.sort_by_key(|p| p.span.start);
    a.diags.sort_by_key(|d| (d.span.start, d.span.end));
    Analysis {
        prompts: a.prompts,
        has_cursor: a.cursor_seen,
        diags: a.diags,
    }
}
