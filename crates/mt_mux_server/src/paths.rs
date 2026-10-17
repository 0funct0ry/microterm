//! Socket location and access control (SPEC §11.3, §12.2).

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

pub fn uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    #[allow(unsafe_code)]
    unsafe {
        libc::getuid()
    }
}

/// `$XDG_RUNTIME_DIR/microterm-mux`, else `/tmp/microterm-mux-$UID`.
pub fn socket_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(d) if d.is_absolute() => d.join("microterm-mux"),
        _ => PathBuf::from(format!("/tmp/microterm-mux-{}", uid())),
    }
}

pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join("sock")
}

pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join("lock")
}

/// Creates `dir` with mode 0700, or checks that an existing one is ours and tightens it.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
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

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
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
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dir_is_created_0700_and_tightened() {
        let base = std::env::temp_dir().join(format!("mtmux-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("a");
        ensure_private_dir(&dir).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn peer_uid_of_a_socketpair_is_ours() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), uid());
    }
}
