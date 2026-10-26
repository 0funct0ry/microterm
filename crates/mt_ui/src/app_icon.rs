//! The microterm app icon (SPEC §15.5).
//!
//! An installed macOS build takes its Dock icon from `Info.plist`. A bare executable (`make run`,
//! `cargo run`, `microterm-app` run from a shell) has no bundle, so it gets the generic "exec"
//! icon; there we set it ourselves. On Linux the icon comes from the installed `.desktop` file
//! matched by `app_id`, so there is nothing to do.

/// The 1024 px master (see `scripts/gen-icons.sh`).
pub const ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/microterm-1024.png");

/// The logo as a GPUI image (About dialog).
pub fn logo_image() -> std::sync::Arc<gpui::Image> {
    use std::sync::{Arc, OnceLock};
    static IMG: OnceLock<Arc<gpui::Image>> = OnceLock::new();
    IMG.get_or_init(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            LOGO_PNG.to_vec(),
        ))
    })
    .clone()
}

/// The 256 px logo shown in the About dialog.
const LOGO_PNG: &[u8] = include_bytes!("../../../assets/app-icon/microterm-256.png");

const NOTICES_FILE: &str = "THIRD-PARTY-LICENSES.html";
const NOTICES_ONLINE: &str = "https://0funct0ry.github.io/microterm/THIRD-PARTY-LICENSES.html";

/// Where the bundled third-party notices live for an executable at `exe`: inside the `.app` on
/// macOS (`Contents/Resources`), `PREFIX/share/doc/microterm` for a Linux install.
pub fn notices_path_for(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let candidates = [
        // X.app/Contents/MacOS/microterm-app
        exe.parent()?
            .parent()
            .map(|c| c.join("Resources").join(NOTICES_FILE)),
        // PREFIX/bin/microterm
        exe.parent()?
            .parent()
            .map(|p| p.join("share/doc/microterm").join(NOTICES_FILE)),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

/// A `file://` URL for the bundled notices, else the copy on the website.
pub fn notices_url() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok())
        .and_then(|e| notices_path_for(&e))
        .map(|p| format!("file://{}", p.display()))
        .unwrap_or_else(|| NOTICES_ONLINE.to_string())
}

/// True when the running executable lives inside a `.app` bundle (which supplies its own icon).
pub fn in_app_bundle() -> bool {
    std::env::current_exe().ok().is_some_and(|p| {
        p.ancestors()
            .any(|a| a.extension().is_some_and(|e| e == "app"))
    })
}

/// Sets the Dock icon for unbundled runs. Must run on the main thread after the application
/// object exists; a no-op elsewhere.
#[cfg(target_os = "macos")]
pub fn set_dock_icon_if_unbundled() {
    use objc2::AnyThread;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};

    if in_app_bundle() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(ICON_PNG);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // SAFETY: called on the main thread with a valid image.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon_if_unbundled() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_resolve_inside_a_bundle_and_an_install_prefix() {
        let dir = std::env::temp_dir().join(format!("mt-notices-{}", std::process::id()));
        let app = dir.join("microterm.app/Contents");
        std::fs::create_dir_all(app.join("MacOS")).unwrap();
        std::fs::create_dir_all(app.join("Resources")).unwrap();
        std::fs::write(app.join("Resources").join(NOTICES_FILE), "x").unwrap();
        let exe = app.join("MacOS/microterm-app");
        assert_eq!(
            notices_path_for(&exe),
            Some(app.join("Resources").join(NOTICES_FILE))
        );

        let doc = dir.join("prefix/share/doc/microterm");
        std::fs::create_dir_all(&doc).unwrap();
        std::fs::create_dir_all(dir.join("prefix/bin")).unwrap();
        std::fs::write(doc.join(NOTICES_FILE), "x").unwrap();
        assert_eq!(
            notices_path_for(&dir.join("prefix/bin/microterm")),
            Some(doc.join(NOTICES_FILE))
        );
        assert_eq!(notices_path_for(&dir.join("nowhere/bin/microterm")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icon_assets_are_pngs() {
        assert_eq!(&ICON_PNG[..4], b"\x89PNG");
        assert_eq!(&LOGO_PNG[..4], b"\x89PNG");
    }
}
