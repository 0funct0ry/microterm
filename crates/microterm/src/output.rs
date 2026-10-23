//! What a `ctl` verb produces, before anything is printed.
//!
//! `ctl_cmd::dispatch` returns an [`Output`] (data plus the exit code it implies) or a
//! [`CtlError`]; `ctl` and `sh` render them with [`crate::render`]. Nothing here prints or exits,
//! so both front ends share one dispatcher and tests can inspect results without a terminal.

use mt_proto::{exit, ErrorCode, MuxSnapshot, RpcError, Spawned};
use serde_json::{json, Value};

use crate::client::ClientError;

/// How a table column is presented on a terminal; plain output ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColKind {
    Text,
    /// Right-aligned, cyan.
    Id,
    /// `*` becomes a marker.
    Flag,
    /// A pane status state, coloured by meaning.
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    pub kind: ColKind,
    /// 0 = never dropped; otherwise columns with the highest rank go first when the terminal is
    /// too narrow.
    pub drop_rank: u8,
}

impl Column {
    pub const fn new(name: &'static str, kind: ColKind) -> Self {
        Column {
            name,
            kind,
            drop_rank: 0,
        }
    }

    pub const fn low(name: &'static str, kind: ColKind, drop_rank: u8) -> Self {
        Column {
            name,
            kind,
            drop_rank,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
    /// Rows to dim on a terminal (exited panes); empty or one entry per row.
    pub dim_rows: Vec<bool>,
    /// Singular noun for the footer count (`3 panes`).
    pub noun: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    None,
    /// The raw IPC result, printed as one JSON line.
    Json(Value),
    Table(Table),
    /// `key<TAB>value` lines in plain form, an aligned card on a terminal.
    Card(Vec<(String, String)>),
    Tree(MuxSnapshot),
    /// One line of text.
    Text(String),
    /// Bytes written exactly as given (`get-text`, `run` output).
    Raw(String),
    /// `tab=5 pane=6 window=2` plain, `✔ tab 5 created (…)` on a terminal.
    Spawned {
        kind: &'static str,
        spawned: Spawned,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub body: Body,
    /// Lines for stderr that do not make the command fail (`notify` drops).
    pub notes: Vec<String>,
    /// The exit code the command asks for (0 unless `run` timed out or propagates a status).
    pub code: i32,
}

impl Output {
    pub fn new(body: Body) -> Self {
        Output {
            body,
            notes: Vec::new(),
            code: exit::OK,
        }
    }

    pub fn none() -> Self {
        Output::new(Body::None)
    }

    pub fn text(line: impl Into<String>) -> Self {
        Output::new(Body::Text(line.into()))
    }

    pub fn with_code(mut self, code: i32) -> Self {
        self.code = code;
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// A failed verb: the exit code, the message and, for server errors, the structured error.
#[derive(Debug, Clone)]
pub struct CtlError {
    pub code: i32,
    pub message: String,
    pub rpc: Option<RpcError>,
    /// The connection to the instance is gone (`sh` reconnects before the next command).
    pub lost: bool,
}

impl CtlError {
    pub fn new(code: i32, message: impl std::fmt::Display) -> Self {
        CtlError {
            code,
            message: message.to_string(),
            rpc: None,
            lost: false,
        }
    }

    pub fn usage(message: impl std::fmt::Display) -> Self {
        CtlError::new(exit::USAGE, message)
    }

    /// An exit code that has already been explained (nothing is printed).
    pub fn silent(code: i32) -> Self {
        CtlError::new(code, "")
    }

    /// The server asked for confirmation before closing something.
    pub fn is_confirm_required(&self) -> bool {
        self.code == exit::CONFIRM_REQUIRED
    }

    fn json_name(&self) -> &'static str {
        match self.code {
            exit::NO_INSTANCE => "no_instance",
            exit::NOT_FOUND => "not_found",
            exit::VERSION_MISMATCH => "version_mismatch",
            exit::TIMEOUT => "timeout",
            exit::NEEDS_SHELL_INTEGRATION => "needs_shell_integration",
            exit::BUSY => "busy",
            exit::DISABLED => "disabled",
            exit::MISSING_VARS => "missing_vars",
            exit::EXPORT_IMPOSSIBLE => "export_impossible",
            _ => "bad_request",
        }
    }

    /// The plain stderr form (no trailing newline); empty for a silent error.
    ///
    /// `--json`: `{"error":{"code","message","candidates"?}}`; otherwise `microterm: MESSAGE`
    /// followed by one indented line per candidate.
    pub fn render_plain(&self, json: bool) -> String {
        if let Some(rpc) = &self.rpc {
            if json {
                return json!({ "error": rpc }).to_string();
            }
            if !rpc.candidates.is_empty() {
                let mut s = format!("microterm: {}", rpc.message);
                for c in &rpc.candidates {
                    s.push_str(&format!("\n  {c}"));
                }
                return s;
            }
            if rpc.code == ErrorCode::ConfirmRequired {
                return format!("microterm: {}", rpc.message);
            }
        }
        if self.message.is_empty() {
            return String::new();
        }
        if json {
            json!({ "error": { "code": self.json_name(), "message": self.message } }).to_string()
        } else {
            format!("microterm: {}", self.message)
        }
    }
}

impl std::fmt::Display for CtlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<ClientError> for CtlError {
    fn from(e: ClientError) -> Self {
        CtlError {
            code: e.exit_code(),
            message: e.to_string(),
            lost: matches!(e, ClientError::Io(_) | ClientError::NoInstance),
            rpc: match e {
                ClientError::Rpc(rpc) => Some(rpc),
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(code: ErrorCode, message: &str, candidates: &[&str]) -> CtlError {
        ClientError::Rpc(RpcError {
            code,
            message: message.into(),
            candidates: candidates.iter().map(|c| c.to_string()).collect(),
        })
        .into()
    }

    #[test]
    fn plain_errors_carry_the_microterm_prefix() {
        assert_eq!(
            CtlError::usage("bad flag").render_plain(false),
            "microterm: bad flag"
        );
        assert_eq!(CtlError::silent(2).render_plain(false), "");
    }

    #[test]
    fn json_errors_name_the_code() {
        let e = CtlError::new(exit::NOT_FOUND, "no such pane");
        assert_eq!(
            e.render_plain(true),
            r#"{"error":{"code":"not_found","message":"no such pane"}}"#
        );
        assert!(CtlError::usage("x")
            .render_plain(true)
            .contains("bad_request"));
    }

    #[test]
    fn candidates_are_listed_under_the_message() {
        let e = rpc(ErrorCode::Ambiguous, "3 panes match", &["pane 1", "pane 2"]);
        assert_eq!(e.code, exit::AMBIGUOUS);
        assert_eq!(
            e.render_plain(false),
            "microterm: 3 panes match\n  pane 1\n  pane 2"
        );
        assert!(e.render_plain(true).starts_with(r#"{"error":{"#));
    }

    #[test]
    fn confirm_required_is_recognised() {
        let e = rpc(ErrorCode::ConfirmRequired, "would close 2 panes", &[]);
        assert!(e.is_confirm_required());
        assert_eq!(e.render_plain(false), "microterm: would close 2 panes");
    }
}
