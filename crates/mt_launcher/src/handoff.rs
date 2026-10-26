//! Hands a window request to a running instance over its IPC socket (the same wire format as
//! the app's own client; only what the launcher needs).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use mt_proto::{Incoming, Request, SpawnParams};

/// `~/Library/Application Support/microterm/sock` (mirrors `microterm::ipc::socket_dir`).
fn socket_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/microterm/sock"))
}

fn candidates() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("MICROTERM_UNIX_SOCKET") {
        return vec![PathBuf::from(p)];
    }
    let Some(dir) = socket_dir() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut socks: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "sock"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    socks.sort_by(|a, b| b.0.cmp(&a.0));
    socks.into_iter().map(|(_, p)| p).collect()
}

fn call(path: &Path, params: &SpawnParams) -> Result<(), String> {
    let stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut stream = stream;
    let req = mt_proto::encode_request(1, &Request::WindowNew(params.clone()));
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    match mt_proto::decode_incoming(&line) {
        Ok(Incoming::Response(Ok(_))) => Ok(()),
        Ok(Incoming::Response(Err(e))) => Err(e.message),
        Ok(Incoming::Event(_)) => Err("unexpected event".into()),
        Err(e) => Err(e.message),
    }
}

/// True when a live instance accepted the request.
pub fn try_running_instance(params: &SpawnParams) -> bool {
    for path in candidates() {
        if call(&path, params).is_ok() {
            return true;
        }
    }
    false
}
