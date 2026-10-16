//! The IPC server (SPEC §12): JSON lines over a Unix socket, one thread per connection. Requests
//! are forwarded to the thread that owns the mux (GPUI main thread, or the headless loop) as
//! [`mt_proto::Call`]s; `subscribe` turns a connection into a `MuxEvent` stream.
//!
//! Security: the socket directory is `0700` and owned by the user; every connection's peer uid
//! must equal ours.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use mt_proto::{Call, EventHub, Request, RpcError};

/// How long a request may wait for the mux thread.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Default socket directory: `$XDG_RUNTIME_DIR/microterm`, or on macOS
/// `~/Library/Application Support/microterm/sock`.
pub fn socket_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        return Some(PathBuf::from(home).join("Library/Application Support/microterm/sock"));
    }
    match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(d) if d.is_absolute() => Some(d.join("microterm")),
        _ => Some(PathBuf::from(format!("/tmp/microterm-{}", uid()))),
    }
}

pub(crate) fn uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    #[allow(unsafe_code)]
    unsafe {
        libc::getuid()
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub(crate) fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for the call.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc == 0 {
        Ok(cred.uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(unsafe_code)]
pub(crate) fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: the out-pointers are valid for the call.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if rc == 0 {
        Ok(uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Creates the socket directory with mode 0700, or checks that an existing one is ours.
pub(crate) fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let meta = std::fs::metadata(dir)?;
    if meta.uid() != uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not owned by you", dir.display()),
        ));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub struct ServerGuard {
    path: PathBuf,
}

impl ServerGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Removes sockets in `dir` that nobody listens on (left by a crashed instance).
pub(crate) fn remove_stale_sockets(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "sock") {
            if let Err(err) = UnixStream::connect(&p) {
                if err.kind() == io::ErrorKind::ConnectionRefused {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
}

pub fn start(calls: async_channel::Sender<Call>, hub: Arc<EventHub>) -> io::Result<ServerGuard> {
    let dir = socket_dir().ok_or_else(|| io::Error::other("no socket directory"))?;
    start_in(&dir, calls, hub)
}

pub(crate) fn start_in(
    dir: &Path,
    calls: async_channel::Sender<Call>,
    hub: Arc<EventHub>,
) -> io::Result<ServerGuard> {
    ensure_private_dir(dir)?;
    remove_stale_sockets(dir);
    let path = dir.join(format!("gui-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path); // stale file from a recycled pid
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("microterm-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let (calls, hub) = (calls.clone(), hub.clone());
                // One thread per connection keeps a slow client from blocking others.
                let _ = std::thread::Builder::new()
                    .name("microterm-ipc-conn".into())
                    .spawn(move || {
                        let _ = serve(stream, &calls, &hub);
                    });
            }
        })?;
    Ok(ServerGuard { path })
}

fn serve(
    stream: UnixStream,
    calls: &async_channel::Sender<Call>,
    hub: &EventHub,
) -> io::Result<()> {
    if peer_uid(&stream)? != uid() {
        return Ok(()); // refuse other users silently
    }
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut out = stream;
    loop {
        let mut line = String::new();
        let n = (&mut reader)
            .take(mt_proto::MAX_LINE as u64 + 1)
            .read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        let reply = match mt_proto::decode_request(&line) {
            Err((id, e)) => mt_proto::encode_response(id, Err(e)),
            Ok((id, Request::Subscribe)) => {
                // Register before answering so no event after the answer can be missed.
                let events = hub.subscribe();
                out.write_all(
                    mt_proto::encode_response(Some(id), Ok(serde_json::json!({}))).as_bytes(),
                )?;
                for ev in events {
                    out.write_all(mt_proto::encode_event(&ev).as_bytes())?;
                }
                return Ok(());
            }
            Ok((id, request)) => {
                let (tx, rx) = mpsc::channel();
                let result = match calls.send_blocking(Call { request, reply: tx }) {
                    Err(_) => Err(RpcError::internal("the application is shutting down")),
                    Ok(()) => rx
                        .recv_timeout(REPLY_TIMEOUT)
                        .unwrap_or_else(|_| Err(RpcError::internal("request timed out"))),
                };
                mt_proto::encode_response(Some(id), result)
            }
        };
        out.write_all(reply.as_bytes())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;
    use mt_proto::{ErrorCode, Event, SpawnParams};
    use serde_json::json;

    pub(crate) fn temp(name: &str) -> PathBuf {
        let d = PathBuf::from(format!("/tmp/mt-ipc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A fake mux thread answering every call with its method name.
    fn echo_server(dir: &Path) -> (ServerGuard, Arc<EventHub>) {
        let (tx, rx) = async_channel::unbounded::<Call>();
        let hub = Arc::new(EventHub::default());
        let guard = start_in(dir, tx, hub.clone()).unwrap();
        std::thread::spawn(move || {
            while let Ok(c) = rx.recv_blocking() {
                let r = match &c.request {
                    Request::PaneKill { pane: 99 } => {
                        Err(RpcError::not_found("pane 99 does not exist"))
                    }
                    r => Ok(json!({ "method": r.method() })),
                };
                let _ = c.reply.send(r);
            }
        });
        (guard, hub)
    }

    #[test]
    fn requests_round_trip_with_errors_and_a_private_dir() {
        let dir = temp("rt");
        let (guard, _hub) = echo_server(&dir);
        let mut c = Client::connect_to(guard.path()).unwrap();
        assert_eq!(
            c.call(&Request::MuxList).unwrap(),
            json!({"method": "mux.list"})
        );
        let e = c.call(&Request::PaneKill { pane: 99 }).unwrap_err();
        assert_eq!(e.exit_code(), mt_proto::exit::NOT_FOUND);
        // The connection stays usable after an error.
        assert!(c.call(&Request::WindowNew(SpawnParams::default())).is_ok());
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_input_gets_error_replies_and_version_mismatch() {
        let dir = temp("bad");
        let (guard, _hub) = echo_server(&dir);
        let mut s = UnixStream::connect(guard.path()).unwrap();
        s.write_all(b"{nope\n{\"v\":2,\"id\":4,\"method\":\"mux.list\"}\n{\"v\":1,\"id\":5,\"method\":\"zzz\"}\n")
            .unwrap();
        let mut r = BufReader::new(s);
        let mut next = || {
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            mt_proto::decode_incoming(&l).unwrap()
        };
        let code = |i: mt_proto::Incoming| match i {
            mt_proto::Incoming::Response(Err(e)) => e.code,
            other => panic!("{other:?}"),
        };
        assert_eq!(code(next()), ErrorCode::BadRequest);
        assert_eq!(code(next()), ErrorCode::VersionMismatch);
        assert_eq!(code(next()), ErrorCode::BadRequest);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subscribe_streams_events() {
        let dir = temp("sub");
        let (guard, hub) = echo_server(&dir);
        let c = Client::connect_to(guard.path()).unwrap();
        let mut events = c.subscribe().unwrap();
        hub.publish(Event::PaneAdded { pane: 3 });
        hub.publish(Event::PaneRemoved { pane: 3 });
        assert_eq!(
            events.next_event().unwrap(),
            Some(Event::PaneAdded { pane: 3 })
        );
        assert_eq!(
            events.next_event().unwrap(),
            Some(Event::PaneRemoved { pane: 3 })
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn newest_live_socket_wins_and_stale_ones_are_removed() {
        let dir = temp("stale");
        ensure_private_dir(&dir).unwrap();
        let stale = dir.join("gui-1.sock");
        drop(UnixListener::bind(&stale).unwrap()); // the file stays, nobody listens
        assert!(stale.exists());
        assert!(matches!(
            Client::connect_in(&dir),
            Err(crate::client::ClientError::NoInstance)
        ));
        assert!(!stale.exists());
        let (guard, _hub) = echo_server(&dir);
        let mut c = Client::connect_in(&dir).unwrap();
        assert!(c.call(&Request::ConfigReload).is_ok());
        drop(guard);
        let _ = std::fs::remove_dir_all(dir);
    }
}
