//! `microterm-mux proxy`: finds or starts the daemon, then bridges stdio to its socket
//! (SPEC §11.3 step 3). Frames pass through untouched.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::{daemon, paths};

/// Connects to the daemon in `dir`, starting `daemon_cmd serve` if nothing listens.
pub fn connect_or_start(dir: &Path, exe: &Path) -> io::Result<UnixStream> {
    paths::ensure_private_dir(dir)?;
    let socket = paths::socket_path(dir);
    if let Ok(s) = UnixStream::connect(&socket) {
        return Ok(s);
    }
    // `serve` detaches itself (setsid + double fork) once the socket is bound.
    let status = std::process::Command::new(exe)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("`serve` failed: {status}")));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(&socket) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Copies `a → b` until EOF or error, then shuts the destination's write side.
fn pump(mut from: impl Read, mut to: impl Write) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).and_then(|_| to.flush()).is_err() {
                    break;
                }
            }
        }
    }
}

pub fn run(dir: &Path, exe: &Path) -> io::Result<()> {
    let sock = connect_or_start(dir, exe)?;
    let up = sock.try_clone()?;
    std::thread::spawn(move || {
        pump(io::stdin().lock(), &up);
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    pump(&sock, io::stdout().lock());
    let _ = daemon::is_alive; // keep the module link explicit for docs
    Ok(())
}
