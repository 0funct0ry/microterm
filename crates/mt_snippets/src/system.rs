//! Real answers for the system facts (SPEC §10.8.4 "System"). They always describe the local
//! machine, even for an SSH pane.

use std::ffi::CStr;

use crate::context::Fact;

fn passwd_field(field: fn(&libc::passwd) -> *const libc::c_char) -> Option<String> {
    // SAFETY: getpwuid_r writes into `buf`/`pwd`; the strings it points to live in `buf`.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let mut buf = vec![0u8; 4096];
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        );
        if rc != 0 || result.is_null() {
            return None;
        }
        let p = field(&pwd);
        (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}

pub fn user() -> Result<String, String> {
    passwd_field(|p| p.pw_name).ok_or_else(|| "no passwd entry for the current user".into())
}

pub fn home() -> Result<String, String> {
    passwd_field(|p| p.pw_dir)
        .or_else(|| std::env::var("HOME").ok())
        .ok_or_else(|| "home directory unknown".into())
}

pub fn hostname() -> Result<String, String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` bytes; gethostname NUL-terminates within it.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    if rc != 0 {
        return Err("gethostname failed".into());
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
}

pub fn tmpdir() -> String {
    std::env::var("TMPDIR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/tmp".into())
}

pub fn cpus() -> String {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .to_string()
}

/// First non-loopback address of an up interface (`getifaddrs`; no traffic is sent). IPv6
/// link-local addresses are skipped.
pub fn local_ip(v6: bool) -> Result<String, String> {
    // SAFETY: the list returned by getifaddrs is only read and freed with freeifaddrs.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return Err("getifaddrs failed".into());
        }
        let mut found = None;
        let mut cur = head;
        while !cur.is_null() && found.is_none() {
            let ifa = &*cur;
            cur = ifa.ifa_next;
            let flags = ifa.ifa_flags as libc::c_int;
            if flags & libc::IFF_UP == 0
                || flags & libc::IFF_LOOPBACK != 0
                || ifa.ifa_addr.is_null()
            {
                continue;
            }
            let family = (*ifa.ifa_addr).sa_family as libc::c_int;
            if !v6 && family == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                found =
                    Some(std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)).to_string());
            } else if v6 && family == libc::AF_INET6 {
                let sin6 = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                let ip = std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                if ip.segments()[0] & 0xffc0 != 0xfe80 {
                    found = Some(ip.to_string());
                }
            }
        }
        libc::freeifaddrs(head);
        found.ok_or_else(|| {
            format!(
                "no {} address on an up interface",
                if v6 { "IPv6" } else { "IPv4" }
            )
        })
    }
}

/// Answers the system facts a host does not own (`Shell` and `Version` come from the host).
pub fn system_fact(f: &Fact) -> Option<Result<String, String>> {
    Some(match f {
        Fact::User => user(),
        Fact::Home => home(),
        Fact::Hostname => hostname(),
        Fact::Tmpdir => Ok(tmpdir()),
        Fact::Cpus => Ok(cpus()),
        Fact::LocalIp { v6 } => local_ip(*v6),
        _ => return None,
    })
}

/// The current local time with its UTC offset.
pub fn now() -> chrono::DateTime<chrono::FixedOffset> {
    chrono::Local::now().fixed_offset()
}

/// Fills `buf` from the OS CSPRNG.
pub fn os_random(buf: &mut [u8]) {
    use rand::RngCore as _;
    rand::rngs::OsRng.fill_bytes(buf);
}
