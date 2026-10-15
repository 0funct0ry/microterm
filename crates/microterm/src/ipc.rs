//! Minimal single-instance IPC (SPEC §12.1). A second `microterm` sends one JSON line to the
//! running instance, which opens a window. M9 replaces this with the full protocol; the envelope
//! (`{"v":1,"id":N,"method":..,"params":..}`) is already the final one.
//!
//! Security: the socket directory is `0700` and owned by the user; every connection's peer uid
//! must equal ours.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use mt_ui::NewWindowRequest;
use serde_json::{json, Value};

const VERSION: u64 = 1;
const MAX_LINE: u64 = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    NoInstance,
}

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

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    #[allow(unsafe_code)]
    unsafe {
        libc::getuid()
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
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
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
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
fn ensure_private_dir(dir: &Path) -> io::Result<()> {
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

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn spawn_server(tx: async_channel::Sender<NewWindowRequest>) -> io::Result<ServerGuard> {
    let dir = socket_dir().ok_or_else(|| io::Error::other("no socket directory"))?;
    spawn_server_in(&dir, tx)
}

pub(crate) fn spawn_server_in(
    dir: &Path,
    tx: async_channel::Sender<NewWindowRequest>,
) -> io::Result<ServerGuard> {
    ensure_private_dir(dir)?;
    let path = dir.join(format!("gui-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path); // stale file from a recycled pid
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("microterm-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let tx = tx.clone();
                // One short-lived thread per connection keeps a slow client from blocking others.
                let _ = std::thread::Builder::new()
                    .name("microterm-ipc-conn".into())
                    .spawn(move || {
                        let _ = serve(stream, &tx);
                    });
            }
        })?;
    Ok(ServerGuard { path })
}

fn serve(stream: UnixStream, tx: &async_channel::Sender<NewWindowRequest>) -> io::Result<()> {
    if peer_uid(&stream)? != uid() {
        return Ok(()); // refuse other users silently
    }
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line)?;
    let reply = match handle_line(&line, tx) {
        Ok(()) => json!({"v": VERSION, "id": id_of(&line), "result": {}}),
        Err((code, message)) => json!({
            "v": VERSION, "id": id_of(&line), "error": {"code": code, "message": message}
        }),
    };
    let mut out = &stream;
    writeln!(out, "{reply}")
}

fn id_of(line: &str) -> Value {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(Value::Null)
}

/// `(error code, message)` per SPEC §12.1 exit-code families: 1 usage, 4 version mismatch.
fn handle_line(
    line: &str,
    tx: &async_channel::Sender<NewWindowRequest>,
) -> Result<(), (u32, String)> {
    let v: Value = serde_json::from_str(line).map_err(|e| (1, format!("invalid JSON: {e}")))?;
    if v.get("v").and_then(Value::as_u64) != Some(VERSION) {
        return Err((4, "unsupported protocol version".into()));
    }
    match v.get("method").and_then(Value::as_str) {
        Some("window.new") => {
            let p = v.get("params").cloned().unwrap_or(Value::Null);
            let req = NewWindowRequest {
                workspace: p.get("workspace").and_then(Value::as_str).map(Into::into),
                cwd: p.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                command: p.get("command").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                }),
            };
            tx.try_send(req)
                .map_err(|_| (1, "the application is shutting down".to_string()))
        }
        other => Err((1, format!("unknown method {other:?}"))),
    }
}

/// Asks the newest live instance to open a window. Dead sockets found on the way are removed.
pub fn send_new_window(req: &NewWindowRequest) -> io::Result<Delivery> {
    if let Some(path) = std::env::var_os("MICROTERM_UNIX_SOCKET").map(PathBuf::from) {
        return Ok(match try_send(&path, req)? {
            true => Delivery::Delivered,
            false => Delivery::NoInstance,
        });
    }
    match socket_dir() {
        Some(dir) => send_new_window_in(&dir, req),
        None => Ok(Delivery::NoInstance),
    }
}

pub(crate) fn send_new_window_in(dir: &Path, req: &NewWindowRequest) -> io::Result<Delivery> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Delivery::NoInstance);
    };
    let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "sock"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    sockets.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in sockets {
        match try_send(&path, req) {
            Ok(true) => return Ok(Delivery::Delivered),
            Ok(false) => {
                let _ = std::fs::remove_file(&path); // nobody is listening: stale
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Delivery::NoInstance)
}

/// `Ok(false)` when the socket is dead, `Err` when a live instance rejected the request.
fn try_send(path: &Path, req: &NewWindowRequest) -> io::Result<bool> {
    let stream = match UnixStream::connect(path) {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            return Ok(false)
        }
        Err(e) => return Err(e),
    };
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let msg = json!({
        "v": VERSION, "id": 1, "method": "window.new",
        "params": {
            "workspace": req.workspace,
            "cwd": req.cwd.as_ref().map(|p| p.to_string_lossy()),
            "command": req.command,
        }
    });
    let mut out = &stream;
    writeln!(out, "{msg}")?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    let v: Value = serde_json::from_str(&reply)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    match v.get("error") {
        Some(err) => Err(io::Error::other(
            err.get("message")
                .and_then(Value::as_str)
                .unwrap_or("request rejected")
                .to_string(),
        )),
        None => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mt-ipc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn second_instance_request_reaches_the_first() {
        let dir = temp("deliver");
        let (tx, rx) = async_channel::unbounded();
        let guard = spawn_server_in(&dir, tx).unwrap();
        let req = NewWindowRequest {
            workspace: Some("api".into()),
            cwd: Some("/tmp".into()),
            command: Some(vec!["htop".into(), "-d".into()]),
        };
        assert_eq!(send_new_window_in(&dir, &req).unwrap(), Delivery::Delivered);
        assert_eq!(rx.recv_blocking().unwrap(), req);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        drop(guard);
        assert_eq!(
            send_new_window_in(&dir, &req).unwrap(),
            Delivery::NoInstance
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_sockets_are_cleaned_up() {
        let dir = temp("stale");
        ensure_private_dir(&dir).unwrap();
        let stale = dir.join("gui-1.sock");
        drop(UnixListener::bind(&stale).unwrap()); // the file stays, nobody listens
        assert!(stale.exists());
        let r = send_new_window_in(&dir, &NewWindowRequest::default()).unwrap();
        assert_eq!(r, Delivery::NoInstance);
        assert!(!stale.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn newest_live_socket_wins_and_bad_requests_are_rejected() {
        let (tx, _rx) = async_channel::unbounded();
        assert_eq!(handle_line("{not json", &tx).unwrap_err().0, 1);
        assert_eq!(
            handle_line(r#"{"v":2,"id":1,"method":"window.new"}"#, &tx)
                .unwrap_err()
                .0,
            4
        );
        assert_eq!(
            handle_line(r#"{"v":1,"id":1,"method":"nope"}"#, &tx)
                .unwrap_err()
                .0,
            1
        );
        assert!(handle_line(r#"{"v":1,"id":1,"method":"window.new"}"#, &tx).is_ok());
    }
}
