//! Shell resolution (SPEC §8.3 `shell`): config → `$SHELL` → passwd entry → `/bin/sh`.

use std::ffi::CStr;

/// Resolves the program to run, using the real environment and passwd database.
pub fn resolve_shell(configured: &str) -> String {
    resolve_shell_with(configured, std::env::var("SHELL").ok(), passwd_shell)
}

pub fn resolve_shell_with(
    configured: &str,
    env_shell: Option<String>,
    passwd: impl FnOnce() -> Option<String>,
) -> String {
    let nonempty = |s: &str| !s.trim().is_empty();
    if nonempty(configured) {
        return configured.trim().to_string();
    }
    if let Some(s) = env_shell.filter(|s| nonempty(s)) {
        return s;
    }
    if let Some(s) = passwd().filter(|s| nonempty(s)) {
        return s;
    }
    "/bin/sh".to_string()
}

#[allow(unsafe_code)]
fn passwd_shell() -> Option<String> {
    // SAFETY: getpwuid_r writes into our buffer and `pwd`; the returned strings point into
    // `buf`, which outlives the CStr conversion below.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        );
        if rc != 0 || out.is_null() || pwd.pw_shell.is_null() {
            return None;
        }
        Some(CStr::from_ptr(pwd.pw_shell).to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_wins() {
        assert_eq!(
            resolve_shell_with("/bin/fish", Some("/bin/zsh".into()), || Some(
                "/bin/bash".into()
            )),
            "/bin/fish"
        );
    }

    #[test]
    fn env_then_passwd_then_sh() {
        assert_eq!(
            resolve_shell_with("", Some("/bin/zsh".into()), || Some("/bin/bash".into())),
            "/bin/zsh"
        );
        assert_eq!(
            resolve_shell_with("  ", Some(String::new()), || Some("/bin/bash".into())),
            "/bin/bash"
        );
        assert_eq!(resolve_shell_with("", None, || None), "/bin/sh");
    }

    #[test]
    fn passwd_lookup_returns_a_path() {
        let s = passwd_shell().expect("current user has a passwd entry");
        assert!(s.starts_with('/'));
    }
}
