//! The client side of the IPC protocol, used by every `microterm ctl …` command and by a second
//! `microterm` that hands its window request to the running instance.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mt_proto::{ErrorCode, Event, Incoming, Request, RpcError};
use serde_json::Value;

#[derive(Debug)]
pub enum ClientError {
    /// No live socket: exit code 2.
    NoInstance,
    Rpc(RpcError),
    Io(io::Error),
}

impl ClientError {
    pub fn exit_code(&self) -> i32 {
        match self {
            ClientError::NoInstance => mt_proto::exit::NO_INSTANCE,
            ClientError::Rpc(e) => e.code.exit_code(),
            ClientError::Io(_) => mt_proto::exit::USAGE,
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NoInstance => write!(f, "no running microterm instance"),
            ClientError::Rpc(e) if e.code == ErrorCode::VersionMismatch => {
                write!(f, "protocol version mismatch: {}", e.message)
            }
            ClientError::Rpc(e) => write!(f, "{}", e.message),
            ClientError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        ClientError::Io(e)
    }
}

pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

fn dead(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}

impl Client {
    /// `$MICROTERM_UNIX_SOCKET`, else the newest live socket in the socket directory.
    pub fn connect() -> Result<Client, ClientError> {
        if let Some(path) = std::env::var_os("MICROTERM_UNIX_SOCKET").map(PathBuf::from) {
            return Client::connect_to(&path);
        }
        match crate::ipc::socket_dir() {
            Some(dir) => Client::connect_in(&dir),
            None => Err(ClientError::NoInstance),
        }
    }

    pub fn connect_to(path: &Path) -> Result<Client, ClientError> {
        let stream = match UnixStream::connect(path) {
            Ok(s) => s,
            Err(e) if dead(&e) => return Err(ClientError::NoInstance),
            Err(e) => return Err(e.into()),
        };
        Ok(Client {
            reader: BufReader::new(stream.try_clone()?),
            stream,
            next_id: 1,
        })
    }

    pub(crate) fn connect_in(dir: &Path) -> Result<Client, ClientError> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Err(ClientError::NoInstance);
        };
        let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "sock"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        sockets.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in sockets {
            match UnixStream::connect(&path) {
                Ok(stream) => {
                    return Ok(Client {
                        reader: BufReader::new(stream.try_clone()?),
                        stream,
                        next_id: 1,
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                    let _ = std::fs::remove_file(&path); // nobody is listening: stale
                }
                Err(e) if dead(&e) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(ClientError::NoInstance)
    }

    pub fn call(&mut self, req: &Request) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.stream
            .write_all(mt_proto::encode_request(id, req).as_bytes())?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed").into());
        }
        match mt_proto::decode_incoming(&line).map_err(ClientError::Rpc)? {
            Incoming::Response(r) => r.map_err(ClientError::Rpc),
            Incoming::Event(_) => Err(ClientError::Rpc(RpcError::internal("unexpected event"))),
        }
    }

    /// Turns this connection into an event stream.
    pub fn subscribe(mut self) -> Result<Events, ClientError> {
        self.call(&Request::Subscribe)?;
        Ok(Events {
            client: self,
            pending: String::new(),
        })
    }
}

pub struct Events {
    client: Client,
    /// A partly received line, kept across timeouts.
    pending: String,
}

impl Events {
    /// Waits at most 200 ms; `Ok(None)` on timeout so callers can poll a stop flag.
    pub fn next_event(&mut self) -> Result<Option<Event>, ClientError> {
        self.client
            .reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(200)))?;
        match self.client.reader.read_line(&mut self.pending) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed").into()),
            Ok(_) if self.pending.ends_with('\n') => {
                let line = std::mem::take(&mut self.pending);
                match mt_proto::decode_incoming(&line).map_err(ClientError::Rpc)? {
                    Incoming::Event(e) => Ok(Some(e)),
                    Incoming::Response(_) => Ok(None),
                }
            }
            Ok(_) => Ok(None),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
}
