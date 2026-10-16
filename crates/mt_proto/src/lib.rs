//! Wire types and codecs for the GUI↔CLI IPC protocol (SPEC §12.2): one JSON object per line over
//! a Unix socket, with a versioned envelope. The mux binary-frame protocol arrives in M10.
//!
//! ```text
//! → {"v":1,"id":7,"method":"pane.send_text","params":{"pane":12,"text":"ls\n","paste":true}}
//! ← {"v":1,"id":7,"result":{}}
//! ← {"v":1,"id":8,"error":{"code":"not_found","message":"pane 99 does not exist"}}
//! ```
//!
//! `subscribe` turns the connection into an event stream: notifications carry no `id`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Protocol major version. A peer speaking a different major is rejected.
pub const PROTO_VERSION: u32 = 1;
/// Longest accepted line, in bytes.
pub const MAX_LINE: usize = 1024 * 1024;

/// CLI exit codes (SPEC §12.1).
pub mod exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 1;
    pub const NO_INSTANCE: i32 = 2;
    pub const NOT_FOUND: i32 = 3;
    pub const VERSION_MISMATCH: i32 = 4;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    VersionMismatch,
    Internal,
}

impl ErrorCode {
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::BadRequest | ErrorCode::Internal => exit::USAGE,
            ErrorCode::NotFound => exit::NOT_FOUND,
            ErrorCode::VersionMismatch => exit::VERSION_MISMATCH,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

// ---- request parameters -------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    #[serde(default)]
    pub new_window: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Terminal size, used by `microterm record --cols/--rows`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendTextParams {
    pub pane: u64,
    pub text: String,
    #[serde(default = "yes")]
    pub paste: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetTextParams {
    pub pane: u64,
    /// Absolute-from-screen-top line numbers: `0` is the first visible row, negative values reach
    /// into scrollback (`-1` is the row just above the screen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<i64>,
    /// Inclusive. Defaults to the last screen row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default)]
    pub escapes: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateTabParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_relative: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetTabTitleParams {
    /// `None` targets the active tab of the active window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u64>,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordStartParams {
    pub pane: u64,
    pub path: String,
    #[serde(default)]
    pub input: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayParams {
    pub path: String,
    #[serde(default = "one")]
    pub speed: f64,
    #[serde(default)]
    pub new_window: bool,
}

fn one() -> f64 {
    1.0
}

/// Every IPC request (SPEC §12.2).
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    MuxList,
    PaneSpawn(SpawnParams),
    PaneSendText(SendTextParams),
    PaneGetText(GetTextParams),
    PaneKill {
        pane: u64,
    },
    TabActivate(ActivateTabParams),
    TabSetTitle(SetTabTitleParams),
    WorkspaceList,
    WorkspaceSwitch {
        name: String,
    },
    WorkspaceCreate {
        name: String,
    },
    WorkspaceRename {
        old: String,
        new: String,
    },
    RecordStart(RecordStartParams),
    RecordStop {
        pane: u64,
    },
    ReplayStart(ReplayParams),
    ConfigReload,
    Subscribe,
    /// The M5 single-instance request: open a window in the running instance.
    WindowNew(SpawnParams),
}

impl Request {
    pub fn method(&self) -> &'static str {
        match self {
            Request::MuxList => "mux.list",
            Request::PaneSpawn(_) => "pane.spawn",
            Request::PaneSendText(_) => "pane.send_text",
            Request::PaneGetText(_) => "pane.get_text",
            Request::PaneKill { .. } => "pane.kill",
            Request::TabActivate(_) => "tab.activate",
            Request::TabSetTitle(_) => "tab.set_title",
            Request::WorkspaceList => "workspace.list",
            Request::WorkspaceSwitch { .. } => "workspace.switch",
            Request::WorkspaceCreate { .. } => "workspace.create",
            Request::WorkspaceRename { .. } => "workspace.rename",
            Request::RecordStart(_) => "record.start",
            Request::RecordStop { .. } => "record.stop",
            Request::ReplayStart(_) => "replay.start",
            Request::ConfigReload => "config.reload",
            Request::Subscribe => "subscribe",
            Request::WindowNew(_) => "window.new",
        }
    }

    pub fn params(&self) -> Value {
        let to = |v: &dyn erased::Ser| v.to_value();
        match self {
            Request::MuxList
            | Request::WorkspaceList
            | Request::ConfigReload
            | Request::Subscribe => json!({}),
            Request::PaneSpawn(p) | Request::WindowNew(p) => to(p),
            Request::PaneSendText(p) => to(p),
            Request::PaneGetText(p) => to(p),
            Request::PaneKill { pane } | Request::RecordStop { pane } => json!({ "pane": pane }),
            Request::TabActivate(p) => to(p),
            Request::TabSetTitle(p) => to(p),
            Request::WorkspaceSwitch { name } | Request::WorkspaceCreate { name } => {
                json!({ "name": name })
            }
            Request::WorkspaceRename { old, new } => json!({ "old": old, "new": new }),
            Request::RecordStart(p) => to(p),
            Request::ReplayStart(p) => to(p),
        }
    }

    pub fn from_parts(method: &str, params: Value) -> Result<Request, RpcError> {
        fn p<T: for<'de> Deserialize<'de>>(method: &str, v: Value) -> Result<T, RpcError> {
            serde_json::from_value(v)
                .map_err(|e| RpcError::bad_request(format!("invalid params for {method}: {e}")))
        }
        #[derive(Deserialize)]
        struct Pane {
            pane: u64,
        }
        #[derive(Deserialize)]
        struct Name {
            name: String,
        }
        #[derive(Deserialize)]
        struct Rename {
            old: String,
            new: String,
        }
        let params = if params.is_null() { json!({}) } else { params };
        Ok(match method {
            "mux.list" => Request::MuxList,
            "pane.spawn" => Request::PaneSpawn(p(method, params)?),
            "pane.send_text" => Request::PaneSendText(p(method, params)?),
            "pane.get_text" => Request::PaneGetText(p(method, params)?),
            "pane.kill" => Request::PaneKill {
                pane: p::<Pane>(method, params)?.pane,
            },
            "tab.activate" => Request::TabActivate(p(method, params)?),
            "tab.set_title" => Request::TabSetTitle(p(method, params)?),
            "workspace.list" => Request::WorkspaceList,
            "workspace.switch" => Request::WorkspaceSwitch {
                name: p::<Name>(method, params)?.name,
            },
            "workspace.create" => Request::WorkspaceCreate {
                name: p::<Name>(method, params)?.name,
            },
            "workspace.rename" => {
                let r: Rename = p(method, params)?;
                Request::WorkspaceRename {
                    old: r.old,
                    new: r.new,
                }
            }
            "record.start" => Request::RecordStart(p(method, params)?),
            "record.stop" => Request::RecordStop {
                pane: p::<Pane>(method, params)?.pane,
            },
            "replay.start" => Request::ReplayStart(p(method, params)?),
            "config.reload" => Request::ConfigReload,
            "subscribe" => Request::Subscribe,
            "window.new" => Request::WindowNew(p(method, params)?),
            other => return Err(RpcError::bad_request(format!("unknown method {other:?}"))),
        })
    }
}

mod erased {
    use serde::Serialize;
    use serde_json::Value;
    pub trait Ser {
        fn to_value(&self) -> Value;
    }
    impl<T: Serialize> Ser for T {
        fn to_value(&self) -> Value {
            serde_json::to_value(self).unwrap_or(Value::Null)
        }
    }
}

// ---- results ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u64,
    pub title: String,
    pub cwd: Option<String>,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: u64,
    pub title: String,
    pub active: bool,
    pub panes: Vec<PaneInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub tabs: Vec<TabInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub name: String,
    pub active: bool,
    pub windows: Vec<WindowInfo>,
}

/// Result of `mux.list` and `workspace.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuxSnapshot {
    pub workspaces: Vec<WorkspaceInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawned {
    pub pane: u64,
    pub tab: u64,
    pub window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub text: String,
}

/// A `MuxEvent` as streamed to `subscribe`rs (SPEC §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    PaneAdded {
        pane: u64,
    },
    PaneRemoved {
        pane: u64,
    },
    PaneOutput {
        pane: u64,
    },
    TitleChanged {
        pane: u64,
    },
    TabAdded {
        window: u64,
        tab: u64,
    },
    TabRemoved {
        window: u64,
        tab: u64,
    },
    TabMoved {
        window: u64,
        tab: u64,
        from: usize,
        to: usize,
    },
    WindowAdded {
        window: u64,
    },
    WindowRemoved {
        window: u64,
    },
    WorkspaceSwitched {
        from: u64,
        to: u64,
    },
}

// ---- envelopes and line codec -------------------------------------------------------------

/// Parses one request line. On failure returns the request `id` (if it could be read) with the
/// error to send back.
pub fn decode_request(line: &str) -> Result<(u64, Request), (Option<u64>, RpcError)> {
    if line.len() > MAX_LINE {
        return Err((None, RpcError::bad_request("request too large")));
    }
    let v: Value = serde_json::from_str(line)
        .map_err(|e| (None, RpcError::bad_request(format!("invalid JSON: {e}"))))?;
    let id = v.get("id").and_then(Value::as_u64);
    if v.get("v").and_then(Value::as_u64) != Some(u64::from(PROTO_VERSION)) {
        return Err((
            id,
            RpcError::new(ErrorCode::VersionMismatch, "unsupported protocol version"),
        ));
    }
    let Some(id) = id else {
        return Err((None, RpcError::bad_request("missing request id")));
    };
    let method = v
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| (Some(id), RpcError::bad_request("missing method")))?;
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    Request::from_parts(method, params)
        .map(|r| (id, r))
        .map_err(|e| (Some(id), e))
}

pub fn encode_request(id: u64, req: &Request) -> String {
    let mut s = json!({
        "v": PROTO_VERSION, "id": id, "method": req.method(), "params": req.params()
    })
    .to_string();
    s.push('\n');
    s
}

pub fn encode_response(id: Option<u64>, res: Result<Value, RpcError>) -> String {
    let mut v = json!({ "v": PROTO_VERSION, "id": id });
    match res {
        Ok(result) => v["result"] = result,
        Err(e) => v["error"] = json!({ "code": e.code, "message": e.message }),
    }
    let mut s = v.to_string();
    s.push('\n');
    s
}

pub fn encode_event(event: &Event) -> String {
    let mut s = json!({ "v": PROTO_VERSION, "event": event }).to_string();
    s.push('\n');
    s
}

/// A line read by a client.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response(Result<Value, RpcError>),
    Event(Event),
}

pub fn decode_incoming(line: &str) -> Result<Incoming, RpcError> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| RpcError::bad_request(format!("invalid JSON from server: {e}")))?;
    if v.get("v").and_then(Value::as_u64) != Some(u64::from(PROTO_VERSION)) {
        return Err(RpcError::new(
            ErrorCode::VersionMismatch,
            "server speaks a different protocol version",
        ));
    }
    if let Some(ev) = v.get("event") {
        return serde_json::from_value(ev.clone())
            .map(Incoming::Event)
            .map_err(|e| RpcError::bad_request(format!("bad event: {e}")));
    }
    if let Some(err) = v.get("error") {
        return serde_json::from_value(err.clone())
            .map(|e| Incoming::Response(Err(e)))
            .map_err(|e| RpcError::bad_request(format!("bad error: {e}")));
    }
    Ok(Incoming::Response(Ok(v
        .get("result")
        .cloned()
        .unwrap_or(Value::Null))))
}

// ---- in-process plumbing shared by the GUI and headless hosts ------------------------------

pub use hub::{Call, EventHub};

mod hub {
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Mutex;

    use serde_json::Value;

    use super::{Event, Request, RpcError};

    /// One request travelling from an IPC connection thread to the thread that owns the mux
    /// (the GPUI main thread, or the headless loop). The owner sends the answer on `reply`.
    pub struct Call {
        pub request: Request,
        pub reply: Sender<Result<Value, RpcError>>,
    }

    /// Fan-out of [`Event`]s to `subscribe` connections. Cheap to clone behind an `Arc`;
    /// `publish` never blocks, and dead subscribers are dropped.
    #[derive(Default)]
    pub struct EventHub {
        subscribers: Mutex<Vec<Sender<Event>>>,
    }

    impl EventHub {
        pub fn subscribe(&self) -> Receiver<Event> {
            let (tx, rx) = channel();
            self.subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(tx);
            rx
        }

        pub fn publish(&self, event: Event) {
            self.subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|tx| tx.send(event.clone()).is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_fans_out_and_drops_dead_subscribers() {
        let hub = EventHub::default();
        let a = hub.subscribe();
        let b = hub.subscribe();
        drop(b);
        hub.publish(Event::PaneAdded { pane: 1 });
        hub.publish(Event::PaneRemoved { pane: 1 });
        assert_eq!(a.try_recv().unwrap(), Event::PaneAdded { pane: 1 });
        assert_eq!(a.try_recv().unwrap(), Event::PaneRemoved { pane: 1 });
    }

    fn roundtrip(req: Request) {
        let line = encode_request(7, &req);
        assert!(line.ends_with('\n'));
        let (id, back) = decode_request(&line).unwrap();
        assert_eq!(id, 7);
        assert_eq!(back, req);
    }

    #[test]
    fn every_method_round_trips() {
        roundtrip(Request::MuxList);
        roundtrip(Request::PaneSpawn(SpawnParams {
            window: Some(2),
            workspace: Some("api".into()),
            cwd: Some("/tmp".into()),
            title: Some("t".into()),
            command: Some(vec!["sh".into(), "-c".into(), "echo hi".into()]),
            cols: Some(100),
            rows: Some(30),
            ..Default::default()
        }));
        roundtrip(Request::PaneSendText(SendTextParams {
            pane: 12,
            text: "ls\n".into(),
            paste: false,
        }));
        roundtrip(Request::PaneGetText(GetTextParams {
            pane: 1,
            start_line: Some(-5),
            end_line: Some(10),
            escapes: true,
        }));
        roundtrip(Request::PaneKill { pane: 3 });
        roundtrip(Request::TabActivate(ActivateTabParams {
            tab_relative: Some(-1),
            ..Default::default()
        }));
        roundtrip(Request::TabSetTitle(SetTabTitleParams {
            tab_id: None,
            title: "x".into(),
        }));
        roundtrip(Request::WorkspaceList);
        roundtrip(Request::WorkspaceSwitch { name: "a".into() });
        roundtrip(Request::WorkspaceCreate { name: "a".into() });
        roundtrip(Request::WorkspaceRename {
            old: "a".into(),
            new: "b".into(),
        });
        roundtrip(Request::RecordStart(RecordStartParams {
            pane: 1,
            path: "x.cast".into(),
            input: true,
        }));
        roundtrip(Request::RecordStop { pane: 1 });
        roundtrip(Request::ReplayStart(ReplayParams {
            path: "x.cast".into(),
            speed: 2.0,
            new_window: true,
        }));
        roundtrip(Request::ConfigReload);
        roundtrip(Request::Subscribe);
        roundtrip(Request::WindowNew(SpawnParams::default()));
    }

    #[test]
    fn spec_example_decodes_with_defaults() {
        let line = r#"{"v":1,"id":7,"method":"pane.send_text","params":{"pane":12,"text":"ls\n"}}"#;
        let (_, r) = decode_request(line).unwrap();
        assert_eq!(
            r,
            Request::PaneSendText(SendTextParams {
                pane: 12,
                text: "ls\n".into(),
                paste: true
            })
        );
    }

    #[test]
    fn errors_carry_codes_and_ids() {
        let (id, e) = decode_request(r#"{"v":2,"id":9,"method":"mux.list"}"#).unwrap_err();
        assert_eq!((id, e.code), (Some(9), ErrorCode::VersionMismatch));
        assert_eq!(e.code.exit_code(), exit::VERSION_MISMATCH);
        let (id, e) = decode_request("{nope").unwrap_err();
        assert_eq!((id, e.code), (None, ErrorCode::BadRequest));
        let (_, e) = decode_request(r#"{"v":1,"id":1,"method":"zzz"}"#).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
        let (_, e) =
            decode_request(r#"{"v":1,"id":1,"method":"pane.kill","params":{}}"#).unwrap_err();
        assert!(e.message.contains("pane.kill"));
        assert_eq!(ErrorCode::NotFound.exit_code(), exit::NOT_FOUND);
    }

    #[test]
    fn responses_and_events_decode() {
        let ok = encode_response(Some(1), Ok(json!({"text": "hi"})));
        assert_eq!(
            decode_incoming(&ok).unwrap(),
            Incoming::Response(Ok(json!({"text": "hi"})))
        );
        let err = encode_response(Some(2), Err(RpcError::not_found("pane 99 does not exist")));
        assert!(err.contains(r#""code":"not_found""#));
        assert_eq!(
            decode_incoming(&err).unwrap(),
            Incoming::Response(Err(RpcError::not_found("pane 99 does not exist")))
        );
        let ev = Event::TabMoved {
            window: 1,
            tab: 2,
            from: 0,
            to: 1,
        };
        let line = encode_event(&ev);
        assert!(!line.contains("\"id\""));
        assert_eq!(decode_incoming(&line).unwrap(), Incoming::Event(ev));
        assert_eq!(
            decode_incoming(r#"{"v":3,"id":1,"result":{}}"#)
                .unwrap_err()
                .code,
            ErrorCode::VersionMismatch
        );
    }
}
