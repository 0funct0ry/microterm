//! Mux wire protocol between the GUI client and `microterm-mux` (SPEC §12.2).
//!
//! Frames are `u32 length (BE) | u8 kind | postcard payload`, where `length` counts the kind
//! byte plus the payload. The first message in each direction is `Hello`; the major protocol
//! version must match.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Mux protocol major version carried in `Hello`.
pub const MUX_PROTO: u16 = 1;
/// Largest accepted frame body (kind byte + payload).
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

pub type PaneId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: PaneId,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    /// `Some(status)` when the process exited while no client was attached.
    pub exited: Option<ExitInfo>,
    pub session: String,
    /// How the pane was split off another one (SPEC §17.1), so a client can rebuild the layout.
    pub split: Option<SplitInfo>,
}

/// A pane created by splitting `parent`. `axis` is 0 for side by side (the new pane on the
/// right), 1 for stacked (below); `permille` is the share the new pane took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitInfo {
    pub parent: PaneId,
    pub axis: u8,
    pub permille: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitInfo {
    pub status: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Msg {
    Hello {
        proto: u16,
        version: String,
    },
    /// Attach to a session; `panes` lists the panes the client already has with the last
    /// sequence number it saw. Empty means "all panes of the session, full replay".
    Attach {
        session: String,
        panes: Vec<(PaneId, u64)>,
    },
    Attached {
        panes: Vec<PaneInfo>,
    },
    Spawn(SpawnReq),
    Spawned {
        req_id: u64,
        pane: PaneId,
    },
    Input {
        pane: PaneId,
        bytes: Vec<u8>,
    },
    Resize {
        pane: PaneId,
        cols: u16,
        rows: u16,
    },
    Output {
        pane: PaneId,
        seq: u64,
        bytes: Vec<u8>,
    },
    Replay {
        pane: PaneId,
        seq: u64,
        bytes: Vec<u8>,
    },
    PaneExited {
        pane: PaneId,
        status: Option<i32>,
    },
    TitleChanged {
        pane: PaneId,
        title: String,
    },
    Ping(u64),
    Pong(u64),
    Detach,
    Kill {
        pane: PaneId,
    },
    KillSession {
        session: String,
    },
    List,
    Listing {
        panes: Vec<PaneInfo>,
    },
    Error {
        message: String,
    },
    /// `pane` was split off `parent` (both already spawned). The daemon keeps this per pane and
    /// returns it in `Attached`/`Listing`; the layout itself is the client's.
    Split {
        pane: PaneId,
        parent: PaneId,
        axis: u8,
        permille: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnReq {
    pub req_id: u64,
    pub session: String,
    pub cwd: Option<String>,
    pub command: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: u32,
}

impl Msg {
    /// The frame kind byte. Stable: append new kinds, never renumber.
    pub fn kind(&self) -> u8 {
        match self {
            Msg::Hello { .. } => 1,
            Msg::Attach { .. } => 2,
            Msg::Attached { .. } => 3,
            Msg::Spawn(_) => 4,
            Msg::Spawned { .. } => 5,
            Msg::Input { .. } => 6,
            Msg::Resize { .. } => 7,
            Msg::Output { .. } => 8,
            Msg::Replay { .. } => 9,
            Msg::PaneExited { .. } => 10,
            Msg::TitleChanged { .. } => 11,
            Msg::Ping(_) => 12,
            Msg::Pong(_) => 13,
            Msg::Detach => 14,
            Msg::Kill { .. } => 15,
            Msg::KillSession { .. } => 16,
            Msg::List => 17,
            Msg::Listing { .. } => 18,
            Msg::Error { .. } => 19,
            Msg::Split { .. } => 20,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame too large ({0} bytes)")]
    TooLarge(usize),
    #[error("empty frame")]
    Empty,
    #[error("frame kind {frame} does not match payload kind {payload}")]
    KindMismatch { frame: u8, payload: u8 },
    #[error("malformed payload: {0}")]
    Payload(String),
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
}

/// Serialises one frame.
pub fn encode(msg: &Msg) -> Result<Vec<u8>, FrameError> {
    let payload = postcard::to_stdvec(msg).map_err(|e| FrameError::Payload(e.to_string()))?;
    let body = payload.len() + 1;
    if body > MAX_FRAME {
        return Err(FrameError::TooLarge(body));
    }
    let mut out = Vec::with_capacity(4 + body);
    out.extend_from_slice(&(body as u32).to_be_bytes());
    out.push(msg.kind());
    out.extend_from_slice(&payload);
    Ok(out)
}

fn decode_body(body: &[u8]) -> Result<Msg, FrameError> {
    let (&kind, payload) = body.split_first().ok_or(FrameError::Empty)?;
    let msg: Msg = postcard::from_bytes(payload).map_err(|e| FrameError::Payload(e.to_string()))?;
    if msg.kind() != kind {
        return Err(FrameError::KindMismatch {
            frame: kind,
            payload: msg.kind(),
        });
    }
    Ok(msg)
}

pub fn write_frame(w: &mut impl Write, msg: &Msg) -> Result<(), FrameError> {
    w.write_all(&encode(msg)?)?;
    Ok(())
}

/// Reads one frame; `Ok(None)` on a clean EOF at a frame boundary.
pub fn read_frame(r: &mut impl Read) -> Result<Option<Msg>, FrameError> {
    let mut len = [0u8; 4];
    match r.read(&mut len[..1])? {
        0 => return Ok(None),
        _ => r.read_exact(&mut len[1..])?,
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(FrameError::TooLarge(n));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    decode_body(&body).map(Some)
}

/// Incremental decoder for async transports: push bytes as they arrive, pull whole messages.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_msg(&mut self) -> Result<Option<Msg>, FrameError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let n = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if n > MAX_FRAME {
            return Err(FrameError::TooLarge(n));
        }
        if self.buf.len() < 4 + n {
            return Ok(None);
        }
        let msg = decode_body(&self.buf[4..4 + n]);
        self.buf.drain(..4 + n);
        msg.map(Some)
    }
}

/// Checks a peer's `Hello`. `Err` carries a human-readable reason.
pub fn check_hello(msg: &Msg) -> Result<&str, String> {
    match msg {
        Msg::Hello { proto, version } if *proto == MUX_PROTO => Ok(version),
        Msg::Hello { proto, version } => Err(format!(
            "protocol mismatch: peer speaks v{proto} (microterm-mux {version}), expected v{MUX_PROTO}"
        )),
        _ => Err("expected Hello as the first message".into()),
    }
}

pub fn hello() -> Msg {
    Msg::Hello {
        proto: MUX_PROTO,
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn samples() -> Vec<Msg> {
        let info = PaneInfo {
            id: 3,
            title: "zsh".into(),
            cols: 80,
            rows: 24,
            exited: Some(ExitInfo { status: Some(2) }),
            session: "prod".into(),
            split: Some(SplitInfo {
                parent: 1,
                axis: 0,
                permille: 400,
            }),
        };
        vec![
            hello(),
            Msg::Attach {
                session: "prod".into(),
                panes: vec![(1, 10), (2, 0)],
            },
            Msg::Attached {
                panes: vec![info.clone()],
            },
            Msg::Spawn(SpawnReq {
                req_id: 9,
                session: "prod".into(),
                cwd: Some("/tmp".into()),
                command: Some(vec!["top".into()]),
                env: vec![("A".into(), "b".into())],
                cols: 100,
                rows: 30,
                scrollback: 10_000,
            }),
            Msg::Spawned { req_id: 9, pane: 4 },
            Msg::Input {
                pane: 4,
                bytes: b"ls\r".to_vec(),
            },
            Msg::Resize {
                pane: 4,
                cols: 120,
                rows: 40,
            },
            Msg::Output {
                pane: 4,
                seq: 77,
                bytes: vec![0, 1, 2, 255],
            },
            Msg::Replay {
                pane: 4,
                seq: 77,
                bytes: b"\x1bc".to_vec(),
            },
            Msg::PaneExited {
                pane: 4,
                status: None,
            },
            Msg::TitleChanged {
                pane: 4,
                title: "t".into(),
            },
            Msg::Ping(1),
            Msg::Pong(1),
            Msg::Detach,
            Msg::Kill { pane: 4 },
            Msg::KillSession {
                session: "prod".into(),
            },
            Msg::List,
            Msg::Listing { panes: vec![info] },
            Msg::Error {
                message: "nope".into(),
            },
            Msg::Split {
                pane: 5,
                parent: 4,
                axis: 1,
                permille: 300,
            },
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for m in samples() {
            let bytes = encode(&m).unwrap();
            let got = read_frame(&mut bytes.as_slice()).unwrap().unwrap();
            assert_eq!(got, m);
        }
    }

    #[test]
    fn kinds_are_unique() {
        let mut kinds: Vec<u8> = samples().iter().map(Msg::kind).collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), samples().len());
    }

    #[test]
    fn clean_eof_is_none_and_truncation_is_error() {
        assert!(read_frame(&mut &[][..]).unwrap().is_none());
        let bytes = encode(&Msg::Ping(5)).unwrap();
        assert!(read_frame(&mut &bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn oversize_and_garbage_are_rejected() {
        let huge = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert!(matches!(
            read_frame(&mut &huge[..]),
            Err(FrameError::TooLarge(_))
        ));
        assert!(matches!(
            read_frame(&mut &[0, 0, 0, 0][..]),
            Err(FrameError::Empty)
        ));
        let mut bad = vec![0, 0, 0, 3, 1, 0xff, 0xff];
        assert!(matches!(
            read_frame(&mut bad.as_slice()),
            Err(FrameError::Payload(_))
        ));
        // Kind byte disagreeing with the payload.
        let mut f = encode(&Msg::Ping(1)).unwrap();
        f[4] = 13;
        bad = f;
        assert!(matches!(
            read_frame(&mut bad.as_slice()),
            Err(FrameError::KindMismatch { .. })
        ));
    }

    #[test]
    fn hello_version_check() {
        assert!(check_hello(&hello()).is_ok());
        let other = Msg::Hello {
            proto: MUX_PROTO + 1,
            version: "9.0.0".into(),
        };
        assert!(check_hello(&other).unwrap_err().contains("mismatch"));
        assert!(check_hello(&Msg::Detach).is_err());
    }

    fn arb_msg() -> impl Strategy<Value = Msg> {
        let bytes = prop::collection::vec(any::<u8>(), 0..600);
        prop_oneof![
            (any::<u64>(), any::<u64>(), bytes.clone())
                .prop_map(|(pane, seq, bytes)| Msg::Output { pane, seq, bytes }),
            (any::<u64>(), bytes.clone()).prop_map(|(pane, bytes)| Msg::Input { pane, bytes }),
            (any::<u64>(), any::<u64>(), bytes).prop_map(|(pane, seq, bytes)| Msg::Replay {
                pane,
                seq,
                bytes
            }),
            (any::<u64>(), any::<u16>(), any::<u16>()).prop_map(|(pane, cols, rows)| Msg::Resize {
                pane,
                cols,
                rows
            }),
            (
                ".*",
                prop::collection::vec((any::<u64>(), any::<u64>()), 0..5)
            )
                .prop_map(|(session, panes)| Msg::Attach { session, panes }),
            (any::<u64>(), prop::option::of(any::<i32>()))
                .prop_map(|(pane, status)| Msg::PaneExited { pane, status }),
            any::<u64>().prop_map(Msg::Ping),
        ]
    }

    proptest! {
        #[test]
        fn arbitrary_messages_round_trip(msgs in prop::collection::vec(arb_msg(), 1..8)) {
            let mut wire = Vec::new();
            for m in &msgs { write_frame(&mut wire, m).unwrap(); }
            let mut r = wire.as_slice();
            for m in &msgs {
                prop_assert_eq!(&read_frame(&mut r).unwrap().unwrap(), m);
            }
            prop_assert!(read_frame(&mut r).unwrap().is_none());
        }

        #[test]
        fn decoder_handles_any_chunking(msgs in prop::collection::vec(arb_msg(), 1..6), chunk in 1usize..37) {
            let mut wire = Vec::new();
            for m in &msgs { write_frame(&mut wire, m).unwrap(); }
            let mut d = Decoder::default();
            let mut got = Vec::new();
            for c in wire.chunks(chunk) {
                d.push(c);
                while let Some(m) = d.next_msg().unwrap() { got.push(m); }
            }
            prop_assert_eq!(got, msgs);
        }

        #[test]
        fn garbage_never_panics(data in prop::collection::vec(any::<u8>(), 0..300)) {
            let mut d = Decoder::default();
            d.push(&data);
            let _ = d.next_msg();
            let _ = read_frame(&mut data.as_slice());
        }
    }
}
