//! `microterm-mux serve`: lock, bind, detach, accept (SPEC §11.3).

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::paths;
use crate::server::Server;

/// A bound, locked daemon socket, ready to run.
pub struct Bound {
    pub listener: UnixListener,
    pub socket: PathBuf,
    /// Held for the daemon's lifetime: the flock ensures a single daemon per directory.
    _lock: File,
}

#[derive(Debug)]
pub enum BindError {
    /// Another daemon already serves this directory.
    AlreadyRunning,
    Io(io::Error),
}

impl From<io::Error> for BindError {
    fn from(e: io::Error) -> Self {
        BindError::Io(e)
    }
}

/// Takes the directory lock, removes a stale socket and binds a fresh one (mode 0600).
pub fn bind(dir: &Path) -> Result<Bound, BindError> {
    paths::ensure_private_dir(dir)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths::lock_path(dir))?;
    // SAFETY: flock on a descriptor we own.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let e = io::Error::last_os_error();
        return Err(if e.kind() == io::ErrorKind::WouldBlock {
            BindError::AlreadyRunning
        } else {
            e.into()
        });
    }
    // The lock file records the daemon's pid (the detached child rewrites it, see `serve`).
    write_pid(&lock);
    let socket = paths::socket_path(dir);
    // We hold the lock, so any socket left here is stale.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(Bound {
        listener,
        socket,
        _lock: lock,
    })
}

fn write_pid(mut lock: &File) {
    use std::io::{Seek, Write};
    if lock.set_len(0).is_ok() && lock.rewind().is_ok() {
        let _ = write!(lock, "{}", std::process::id());
    }
}

/// Classic detach: fork, setsid, fork again, chdir `/`, stdio to the log file (or /dev/null).
/// Returns `Ok(true)` in the daemon and `Ok(false)` in the original process, which should exit.
/// Must run before any thread is started.
#[allow(unsafe_code)]
pub fn detach(log: Option<&Path>) -> io::Result<bool> {
    // SAFETY: single-threaded at this point; the parents only call _exit-style returns.
    unsafe {
        match libc::fork() {
            -1 => return Err(io::Error::last_os_error()),
            0 => {}
            _ => return Ok(false),
        }
        if libc::setsid() < 0 {
            return Err(io::Error::last_os_error());
        }
        match libc::fork() {
            -1 => return Err(io::Error::last_os_error()),
            0 => {}
            _ => libc::_exit(0),
        }
        libc::chdir(c"/".as_ptr());
        let null = File::options().read(true).write(true).open("/dev/null")?;
        let out = match log.and_then(|p| File::options().create(true).append(true).open(p).ok()) {
            Some(f) => f,
            None => null.try_clone()?,
        };
        libc::dup2(null.as_raw_fd(), 0);
        libc::dup2(out.as_raw_fd(), 1);
        libc::dup2(out.as_raw_fd(), 2);
    }
    Ok(true)
}

/// `serve`: bind, detach unless `foreground`, then accept forever.
pub fn serve(dir: &Path, foreground: bool) -> Result<(), BindError> {
    let bound = bind(dir)?;
    if !foreground {
        match detach(Some(&dir.join("daemon.log"))) {
            Ok(true) => write_pid(&bound._lock),
            Ok(false) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    Server::new().run(bound.listener);
    Ok(())
}

/// Is a daemon accepting connections at `socket`?
pub fn is_alive(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}
