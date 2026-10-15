//! Per-OS process inspection: foreground process name and working directory (SPEC §5).

use std::os::fd::RawFd;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundProcess {
    pub pid: u32,
    pub name: String,
}

/// The foreground process group of the PTY, or `None` when it is the shell itself (idle
/// prompt) or cannot be determined.
#[allow(unsafe_code)]
pub(crate) fn foreground(master_fd: RawFd, shell_pid: u32) -> Option<ForegroundProcess> {
    // SAFETY: tcgetpgrp only reads kernel state for the fd; a stale fd yields an error.
    let pgrp = unsafe { libc::tcgetpgrp(master_fd) };
    if pgrp <= 0 || pgrp as u32 == shell_pid {
        return None;
    }
    let name = process_name(pgrp as u32)?;
    Some(ForegroundProcess {
        pid: pgrp as u32,
        name,
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn process_name(pid: u32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(s.trim_end().to_string()).filter(|s| !s.is_empty())
}

#[cfg(target_os = "linux")]
pub(crate) fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) fn process_name(pid: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for `buf.len()` bytes.
    let n = unsafe { libc::proc_name(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) fn process_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    // SAFETY: `info` is valid for `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if n < size {
        return None;
    }
    // SAFETY: `vip_path` is a contiguous NUL-terminated byte array of MAXPATHLEN.
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            info.pvi_cdir.vip_path.as_ptr().cast::<u8>(),
            std::mem::size_of_val(&info.pvi_cdir.vip_path),
        )
    };
    let end = bytes.iter().position(|b| *b == 0)?;
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(
        bytes[..end].to_vec(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_has_name_and_cwd() {
        let pid = std::process::id();
        assert!(process_name(pid).is_some());
        assert_eq!(
            process_cwd(pid).map(|p| p.canonicalize().unwrap()),
            Some(std::env::current_dir().unwrap().canonicalize().unwrap())
        );
    }
}
