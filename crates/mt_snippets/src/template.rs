//! The `{{ }}` template language (SPEC §10.8.3): lexer, parser, AST with byte spans.
//! Hand-written (D12). Parsing never panics; semantic checks live in [`crate::analyze`].

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
    pub fn slice(self, src: &str) -> &str {
        src.get(self.start..self.end).unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagKind {
    EmptyTag,
    UnterminatedTag,
    NewlineInTag,
    UnterminatedString,
    BadEscape,
    Syntax,
    UnknownFunction,
    UnknownFilter,
    UnknownArg,
    Arity,
    ArgType,
    SecondCursor,
    UnboundVar,
    Rebound,
    PromptConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub kind: DiagKind,
    pub span: Span,
    pub message: String,
}

impl Diag {
    pub(crate) fn new(kind: DiagKind, span: Span, message: impl Into<String>) -> Self {
        Self {
            kind,
            span,
            message: message.into(),
        }
    }

    /// 1-based `(line, col)` of the start of the span in `src`.
    pub fn line_col(&self, src: &str) -> (usize, usize) {
        let off = self.span.start.min(src.len());
        let off = (0..=off)
            .rev()
            .find(|i| src.is_char_boundary(*i))
            .unwrap_or(0);
        let before = &src[..off];
        (
            before.matches('\n').count() + 1,
            before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1,
        )
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lit {
    Str(String),
    /// Integer literal with its source text (`007` keeps its zeros as a string argument).
    Int(i64, String),
    Bool(bool),
    /// A bareword such as `v7` (a string).
    Bare(String),
}

impl Lit {
    /// The literal as a string argument.
    pub fn text(&self) -> String {
        match self {
            Lit::Str(s) | Lit::Bare(s) => s.clone(),
            Lit::Int(_, raw) => raw.clone(),
            Lit::Bool(b) => b.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Lit(Lit, Span),
    Sub(Box<Pipeline>, Span),
}

impl Arg {
    pub fn span(&self) -> Span {
        match self {
            Arg::Lit(_, s) | Arg::Sub(_, s) => *s,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    pub name_span: Span,
    pub args: Vec<Arg>,
    pub named: Vec<(String, Span, Lit, Span)>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Call(Call),
    Var(String, Span),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub first: Term,
    pub filters: Vec<Call>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub span: Span,
    pub bind: Option<(String, Span)>,
    pub pipeline: Pipeline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Literal text with `\{{` already unescaped.
    Text(String, Span),
    Tag(Tag),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ast {
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub ast: Ast,
    pub diags: Vec<Diag>,
}

// ---------------------------------------------------------------------------------------------
// Lexer (inside one tag)

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Word(String),
    Str(String),
    Var(String),
    Pipe,
    Assign,
    Eq,
    LParen,
    RParen,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    span: Span,
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-')
}

/// Finds the `}}` that closes the tag whose body starts at `from`. Quoted strings may contain
/// `}}`. Returns the index of the closing `}}`.
fn find_tag_end(src: &str, from: usize) -> Result<usize, (DiagKind, Span)> {
    let b = src.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'\n' | b'\r' => {
                return Err((DiagKind::NewlineInTag, Span::new(i, i + 1)));
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    if i < b.len() && (b[i] == b'\n' || b[i] == b'\r') {
                        return Err((DiagKind::NewlineInTag, Span::new(i, i + 1)));
                    }
                    i += 1;
                }
                i += 1;
            }
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    if b[i] == b'\n' || b[i] == b'\r' {
                        return Err((DiagKind::NewlineInTag, Span::new(i, i + 1)));
                    }
                    i += 1;
                }
                i += 1;
            }
            b'}' if b.get(i + 1) == Some(&b'}') => return Ok(i),
            _ => i += 1,
        }
    }
    Err((
        DiagKind::UnterminatedTag,
        Span::new(from.saturating_sub(2), src.len()),
    ))
}

fn lex(src: &str, start: usize, end: usize, diags: &mut Vec<Diag>) -> Vec<Token> {
    let mut out = Vec::new();
    let mut it = src[start..end].char_indices().peekable();
    let abs = |i: usize| start + i;
    while let Some(&(i, c)) = it.peek() {
        if c.is_whitespace() {
            it.next();
        } else if c == '|' {
            it.next();
            out.push(Token {
                tok: Tok::Pipe,
                span: Span::new(abs(i), abs(i) + 1),
            });
        } else if c == '(' || c == ')' {
            it.next();
            out.push(Token {
                tok: if c == '(' { Tok::LParen } else { Tok::RParen },
                span: Span::new(abs(i), abs(i) + 1),
            });
        } else if c == ':' && src[abs(i)..end].starts_with(":=") {
            it.next();
            it.next();
            out.push(Token {
                tok: Tok::Assign,
                span: Span::new(abs(i), abs(i) + 2),
            });
        } else if c == '=' {
            it.next();
            out.push(Token {
                tok: Tok::Eq,
                span: Span::new(abs(i), abs(i) + 1),
            });
        } else if c == '$' {
            it.next();
            let mut name = String::new();
            while let Some(&(_, d)) = it.peek() {
                if d.is_ascii_lowercase() || d.is_ascii_digit() || d == '_' {
                    name.push(d);
                    it.next();
                } else {
                    break;
                }
            }
            let sp = Span::new(abs(i), abs(i) + 1 + name.len());
            if is_ident(&name) {
                out.push(Token {
                    tok: Tok::Var(name),
                    span: sp,
                });
            } else {
                diags.push(Diag::new(DiagKind::Syntax, sp, "expected a name after `$`"));
            }
        } else if c == '"' {
            it.next();
            let mut s = String::new();
            let mut closed = false;
            let mut last = abs(i) + 1;
            while let Some((j, d)) = it.next() {
                last = abs(j) + d.len_utf8();
                match d {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => match it.next() {
                        Some((k, e)) => {
                            last = abs(k) + e.len_utf8();
                            match e {
                                '"' => s.push('"'),
                                '\\' => s.push('\\'),
                                'n' => s.push('\n'),
                                't' => s.push('\t'),
                                'u' => {
                                    let mut hex = String::new();
                                    let ok = matches!(it.peek(), Some(&(_, '{')));
                                    if ok {
                                        it.next();
                                        for (m, h) in it.by_ref() {
                                            last = abs(m) + h.len_utf8();
                                            if h == '}' {
                                                break;
                                            }
                                            hex.push(h);
                                        }
                                    }
                                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                                        Some(ch) if ok && !hex.is_empty() && hex.len() <= 6 => s.push(ch),
                                        _ => diags.push(Diag::new(
                                            DiagKind::BadEscape,
                                            Span::new(abs(j), last),
                                            "bad \\u{HEX} escape",
                                        )),
                                    }
                                }
                                other => diags.push(Diag::new(
                                    DiagKind::BadEscape,
                                    Span::new(abs(j), last),
                                    format!("unknown escape `\\{other}` (use \\\" \\\\ \\n \\t \\u{{HEX}})"),
                                )),
                            }
                        }
                        None => break,
                    },
                    d => s.push(d),
                }
            }
            let sp = Span::new(abs(i), last);
            if closed {
                out.push(Token {
                    tok: Tok::Str(s),
                    span: sp,
                });
            } else {
                diags.push(Diag::new(
                    DiagKind::UnterminatedString,
                    sp,
                    "unterminated string",
                ));
            }
        } else if c == '\'' {
            it.next();
            let mut s = String::new();
            let mut closed = false;
            let mut last = abs(i) + 1;
            for (j, d) in it.by_ref() {
                last = abs(j) + d.len_utf8();
                if d == '\'' {
                    closed = true;
                    break;
                }
                s.push(d);
            }
            let sp = Span::new(abs(i), last);
            if closed {
                out.push(Token {
                    tok: Tok::Str(s),
                    span: sp,
                });
            } else {
                diags.push(Diag::new(
                    DiagKind::UnterminatedString,
                    sp,
                    "unterminated string",
                ));
            }
        } else if is_word_char(c) {
            let mut run = String::new();
            let mut last = abs(i);
            while let Some(&(j, d)) = it.peek() {
                if !is_word_char(d) || (d == ':' && src[abs(j)..end].starts_with(":=")) {
                    break;
                }
                run.push(d);
                last = abs(j) + d.len_utf8();
                it.next();
            }
            let sp = Span::new(abs(i), last);
            out.push(Token {
                tok: if is_ident(&run) {
                    Tok::Ident(run)
                } else {
                    Tok::Word(run)
                },
                span: sp,
            });
        } else {
            it.next();
            diags.push(Diag::new(
                DiagKind::Syntax,
                Span::new(abs(i), abs(i) + c.len_utf8()),
                format!("unexpected character `{c}`"),
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Parser (one tag)

struct P<'a> {
    toks: &'a [Token],
    i: usize,
    diags: &'a mut Vec<Diag>,
    tag_span: Span,
}

fn classify(word: &str) -> Lit {
    if let Ok(n) = word.parse::<i64>() {
        Lit::Int(n, word.to_string())
    } else if word == "true" {
        Lit::Bool(true)
    } else if word == "false" {
        Lit::Bool(false)
    } else {
        Lit::Bare(word.to_string())
    }
}

impl P<'_> {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.i)
    }

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diag::new(DiagKind::Syntax, span, msg));
    }

    fn eof_span(&self) -> Span {
        self.toks.last().map_or(
            Span::new(self.tag_span.end.saturating_sub(2), self.tag_span.end),
            |t| Span::new(t.span.end, t.span.end),
        )
    }

    fn pipeline(&mut self, in_parens: bool) -> Option<Pipeline> {
        let first = match self.peek()?.clone() {
            Token {
                tok: Tok::Var(n),
                span,
            } => {
                self.i += 1;
                Term::Var(n, span)
            }
            Token {
                tok: Tok::Ident(_), ..
            } => Term::Call(self.call(in_parens)?),
            t => {
                self.err(t.span, "expected a function name or `$name`");
                return None;
            }
        };
        let start = match &first {
            Term::Call(c) => c.span.start,
            Term::Var(_, s) => s.start,
        };
        let mut filters = Vec::new();
        while matches!(self.peek(), Some(Token { tok: Tok::Pipe, .. })) {
            self.i += 1;
            match self.peek() {
                Some(Token {
                    tok: Tok::Ident(_), ..
                }) => filters.push(self.call(in_parens)?),
                Some(t) => {
                    let sp = t.span;
                    self.err(sp, "expected a filter name after `|`");
                    return None;
                }
                None => {
                    let sp = self.eof_span();
                    self.err(sp, "expected a filter name after `|`");
                    return None;
                }
            }
        }
        let end = filters.last().map_or_else(
            || match &first {
                Term::Call(c) => c.span.end,
                Term::Var(_, s) => s.end,
            },
            |c| c.span.end,
        );
        Some(Pipeline {
            first,
            filters,
            span: Span::new(start, end),
        })
    }

    fn call(&mut self, in_parens: bool) -> Option<Call> {
        let Token {
            tok: Tok::Ident(name),
            span,
        } = self.peek()?.clone()
        else {
            return None;
        };
        self.i += 1;
        let mut call = Call {
            name,
            name_span: span,
            args: Vec::new(),
            named: Vec::new(),
            span,
        };
        while let Some(t) = self.peek().cloned() {
            match t.tok {
                Tok::Pipe => break,
                Tok::RParen if in_parens => break,
                Tok::RParen => {
                    self.err(t.span, "unmatched `)`");
                    return None;
                }
                Tok::Str(s) => {
                    self.i += 1;
                    call.span.end = t.span.end;
                    call.args.push(Arg::Lit(Lit::Str(s), t.span));
                }
                Tok::Ident(k) if matches!(self.toks.get(self.i + 1), Some(e) if e.tok == Tok::Eq && e.span.start == t.span.end) =>
                {
                    self.i += 2;
                    let Some(v) = self.peek().cloned() else {
                        let sp = self.eof_span();
                        self.err(sp, format!("expected a value after `{k}=`"));
                        return None;
                    };
                    let adjacent = v.span.start == t.span.end + 1;
                    let lit = match v.tok {
                        Tok::Str(s) => Lit::Str(s),
                        Tok::Ident(w) | Tok::Word(w) => classify(&w),
                        _ => {
                            self.err(v.span, format!("expected a value after `{k}=`"));
                            return None;
                        }
                    };
                    if !adjacent {
                        self.err(
                            v.span,
                            format!("no space allowed between `{k}=` and its value"),
                        );
                    }
                    self.i += 1;
                    call.span.end = v.span.end;
                    call.named.push((k, t.span, lit, v.span));
                }
                Tok::Ident(w) | Tok::Word(w) => {
                    self.i += 1;
                    call.span.end = t.span.end;
                    call.args.push(Arg::Lit(classify(&w), t.span));
                }
                Tok::LParen => {
                    self.i += 1;
                    let sub = self.pipeline(true)?;
                    match self.peek().cloned() {
                        Some(Token {
                            tok: Tok::RParen,
                            span: close,
                        }) => {
                            self.i += 1;
                            call.span.end = close.end;
                            call.args
                                .push(Arg::Sub(Box::new(sub), Span::new(t.span.start, close.end)));
                        }
                        _ => {
                            let sp = self.peek().map_or_else(|| self.eof_span(), |t| t.span);
                            self.err(sp, "expected `)`");
                            return None;
                        }
                    }
                }
                Tok::Var(_) => {
                    self.err(
                        t.span,
                        "`$name` can only start a pipeline; as an argument write `($name)`",
                    );
                    return None;
                }
                Tok::Assign | Tok::Eq => {
                    self.err(t.span, "unexpected `:=` or `=`");
                    return None;
                }
            }
        }
        Some(call)
    }
}

fn parse_tag(src: &str, start: usize, end: usize, diags: &mut Vec<Diag>) -> Option<Tag> {
    let tag_span = Span::new(start, end + 2);
    let inner = &src[start + 2..end];
    if inner.trim().is_empty() {
        diags.push(Diag::new(DiagKind::EmptyTag, tag_span, "empty tag `{{}}`"));
        return None;
    }
    let before = diags.len();
    let toks = lex(src, start + 2, end, diags);
    if diags.len() > before {
        return None;
    }
    let mut p = P {
        toks: &toks,
        i: 0,
        diags,
        tag_span,
    };
    let bind = match (p.toks.first(), p.toks.get(1)) {
        (
            Some(Token {
                tok: Tok::Ident(n),
                span,
            }),
            Some(Token {
                tok: Tok::Assign, ..
            }),
        ) => {
            let b = (n.clone(), *span);
            p.i = 2;
            Some(b)
        }
        _ => None,
    };
    let pipeline = p.pipeline(false)?;
    if let Some(t) = p.peek().cloned() {
        p.err(t.span, "unexpected token after the expression");
        return None;
    }
    Some(Tag {
        span: tag_span,
        bind,
        pipeline,
    })
}

/// Parses a template body. Never panics; every problem is a [`Diag`] with a byte span.
pub fn parse(src: &str) -> Parsed {
    let mut nodes = Vec::new();
    let mut diags = Vec::new();
    let mut text = String::new();
    let mut text_start = 0;
    let mut i = 0;
    let flush = |text: &mut String, nodes: &mut Vec<Node>, start: usize, end: usize| {
        if !text.is_empty() {
            nodes.push(Node::Text(std::mem::take(text), Span::new(start, end)));
        }
    };
    while i < src.len() {
        let rest = &src[i..];
        if rest.starts_with("\\{{") {
            if text.is_empty() {
                text_start = i;
            }
            text.push_str("{{");
            i += 3;
        } else if rest.starts_with("{{") {
            flush(&mut text, &mut nodes, text_start, i);
            match find_tag_end(src, i + 2) {
                Ok(end) => {
                    if let Some(tag) = parse_tag(src, i, end, &mut diags) {
                        nodes.push(Node::Tag(tag));
                    }
                    i = end + 2;
                }
                Err((kind, span)) => {
                    let msg = match kind {
                        DiagKind::NewlineInTag => "a tag cannot contain a newline",
                        _ => "unterminated `{{` (missing `}}`)",
                    };
                    diags.push(Diag::new(kind, span, msg));
                    // Resume after the offending line so one error does not hide the rest.
                    i = match kind {
                        DiagKind::NewlineInTag => span.end,
                        _ => src.len(),
                    };
                }
            }
            text_start = i;
        } else {
            if text.is_empty() {
                text_start = i;
            }
            let ch = rest.chars().next().expect("non-empty");
            text.push(ch);
            i += ch.len_utf8();
        }
    }
    flush(&mut text, &mut nodes, text_start, src.len());
    Parsed {
        ast: Ast { nodes },
        diags,
    }
}
