//! Friendly failure when no Vulkan loader is installed on Linux (SPEC §15.2).

/// Package to install for the distro described by `/etc/os-release` contents.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn vulkan_package_hint(os_release: &str) -> &'static str {
    let field = |key: &str| {
        os_release
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim_start_matches('=').trim_matches('"').to_lowercase())
            .unwrap_or_default()
    };
    let ids = format!("{} {}", field("ID"), field("ID_LIKE"));
    let has = |names: &[&str]| ids.split_whitespace().any(|i| names.contains(&i));
    if has(&["debian", "ubuntu", "linuxmint", "pop", "raspbian"]) {
        "libvulkan1 mesa-vulkan-drivers (sudo apt install libvulkan1 mesa-vulkan-drivers)"
    } else if has(&["fedora", "rhel", "centos", "rocky", "almalinux"]) {
        "vulkan-loader mesa-vulkan-drivers (sudo dnf install vulkan-loader mesa-vulkan-drivers)"
    } else if has(&["arch", "manjaro", "endeavouros"]) {
        "vulkan-icd-loader and your GPU's Vulkan driver (sudo pacman -S vulkan-icd-loader vulkan-radeon)"
    } else if has(&["opensuse", "suse", "sles"]) {
        "libvulkan1 Mesa-vulkan-drivers (sudo zypper install libvulkan1 Mesa-vulkan-drivers)"
    } else if has(&["alpine"]) {
        "vulkan-loader mesa-vulkan-swrast (sudo apk add vulkan-loader mesa-vulkan-swrast)"
    } else {
        "the Vulkan loader (libvulkan.so.1) and a Vulkan driver such as Mesa"
    }
}

/// Whether a Vulkan loader can be found in the usual library directories.
#[cfg(target_os = "linux")]
fn vulkan_loader_present() -> bool {
    const DIRS: &[&str] = &[
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/lib/x86_64-linux-gnu",
        "/lib/aarch64-linux-gnu",
        "/usr/local/lib",
    ];
    DIRS.iter()
        .any(|d| std::path::Path::new(d).join("libvulkan.so.1").exists())
}

/// Exits with an actionable message when Vulkan is unavailable. No-op on other platforms.
pub fn ensure_gpu_available() {
    #[cfg(target_os = "linux")]
    if !vulkan_loader_present() {
        let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        eprintln!(
            "microterm: no Vulkan loader found (libvulkan.so.1). Install {}.\n\
             Software rendering via Mesa llvmpipe also needs the Mesa Vulkan drivers.",
            vulkan_package_hint(&os)
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_package_for_each_distro_family() {
        assert!(vulkan_package_hint("ID=ubuntu\nID_LIKE=debian\n").contains("libvulkan1"));
        assert!(vulkan_package_hint("ID=debian").contains("apt install"));
        assert!(vulkan_package_hint("ID=\"fedora\"").contains("vulkan-loader"));
        assert!(vulkan_package_hint("ID=rocky\nID_LIKE=\"rhel centos fedora\"").contains("dnf"));
        assert!(vulkan_package_hint("ID=arch").contains("vulkan-icd-loader"));
        assert!(
            vulkan_package_hint("ID=opensuse-leap\nID_LIKE=\"suse opensuse\"").contains("zypper")
        );
        assert!(vulkan_package_hint("ID=alpine").contains("apk"));
    }

    #[test]
    fn unknown_distro_gets_a_generic_hint() {
        assert!(vulkan_package_hint("").contains("libvulkan.so.1"));
        assert!(vulkan_package_hint("ID=plan9").contains("Mesa"));
    }
}
