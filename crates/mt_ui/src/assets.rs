//! Embedded fonts and Lucide icons, served to GPUI as an `AssetSource`.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};

macro_rules! embed {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../../../assets/", $path)).as_slice())),*]
    };
}

const FILES: &[(&str, &[u8])] = embed![
    "icons/arrow-down.svg",
    "icons/arrow-up.svg",
    "icons/book-open.svg",
    "icons/bug.svg",
    "icons/case-sensitive.svg",
    "icons/check.svg",
    "icons/chevron-down.svg",
    "icons/chevron-right.svg",
    "icons/circle-x.svg",
    "icons/clipboard.svg",
    "icons/code.svg",
    "icons/columns-2.svg",
    "icons/copy.svg",
    "icons/eye.svg",
    "icons/file-text.svg",
    "icons/file.svg",
    "icons/folder-open.svg",
    "icons/folder.svg",
    "icons/git-branch.svg",
    "icons/git-commit-horizontal.svg",
    "icons/git-pull-request.svg",
    "icons/globe.svg",
    "icons/house.svg",
    "icons/info.svg",
    "icons/layers.svg",
    "icons/link.svg",
    "icons/lock.svg",
    "icons/maximize-2.svg",
    "icons/minus.svg",
    "icons/palette.svg",
    "icons/pencil.svg",
    "icons/play.svg",
    "icons/plus.svg",
    "icons/refresh-cw.svg",
    "icons/regex.svg",
    "icons/rotate-cw.svg",
    "icons/rows-2.svg",
    "icons/search.svg",
    "icons/server.svg",
    "icons/square.svg",
    "icons/terminal.svg",
    "icons/trash-2.svg",
    "icons/triangle-alert.svg",
    "icons/wifi-off.svg",
    "icons/x.svg",
];

const SYMBOLS_NERD_FONT: &[u8] =
    include_bytes!("../../../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

const FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/IBMPlexSans_400Regular.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans_500Medium.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans_600SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_400Regular.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_400Regular_Italic.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_700Bold.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_700Bold_Italic.ttf"),
    SYMBOLS_NERD_FONT,
];

/// Raw bytes of every bundled font face (for tools that inspect fonts without a window).
pub fn bundled_fonts() -> &'static [&'static [u8]] {
    FONTS
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// Registers the bundled IBM Plex Sans, JetBrains Mono and Symbols Nerd Font Mono faces with the text system.
pub fn load_fonts(cx: &App) -> Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|b| Cow::Borrowed(*b)).collect())?;
    #[cfg(target_os = "macos")]
    register_fallback_fonts_with_coretext();
    Ok(())
}

/// GPUI keeps bundled fonts in its own in-memory source, but macOS resolves `font.fallback`
/// names through CoreText's cascade list, which cannot see them. Symbols Nerd Font Mono also has
/// no `m` glyph, so GPUI will not select it directly. Registering it process-wide makes the
/// cascade find it by name, so powerline and devicon glyphs fall back to it.
#[cfg(target_os = "macos")]
fn register_fallback_fonts_with_coretext() {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGDataProviderCreateWithData(
            info: *mut c_void,
            data: *const c_void,
            size: usize,
            release: *const c_void,
        ) -> *mut c_void;
        fn CGFontCreateWithDataProvider(provider: *mut c_void) -> *mut c_void;
        fn CGDataProviderRelease(provider: *mut c_void);
        fn CGFontRelease(font: *mut c_void);
    }
    #[link(name = "CoreText", kind = "framework")]
    extern "C" {
        fn CTFontManagerRegisterGraphicsFont(font: *mut c_void, error: *mut *mut c_void) -> bool;
    }

    let bytes = SYMBOLS_NERD_FONT;
    // SAFETY: `bytes` is `'static`, so the provider needs no release callback; the provider and
    // font references are released after registration, which retains what it needs.
    unsafe {
        let provider = CGDataProviderCreateWithData(
            null_mut(),
            bytes.as_ptr().cast(),
            bytes.len(),
            std::ptr::null(),
        );
        if provider.is_null() {
            return;
        }
        let font = CGFontCreateWithDataProvider(provider);
        CGDataProviderRelease(provider);
        if font.is_null() {
            return;
        }
        // An error here (for example the font is already registered) only means the cascade
        // may miss it; text still renders with the system fallbacks.
        let _ = CTFontManagerRegisterGraphicsFont(font, null_mut());
        CGFontRelease(font);
    }
}

/// Lucide icons bundled in `assets/icons`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconName {
    ArrowDown,
    ArrowUp,
    BookOpen,
    Bug,
    CaseSensitive,
    Check,
    ChevronDown,
    ChevronRight,
    CircleX,
    Clipboard,
    Code,
    Columns2,
    Copy,
    Eye,
    File,
    FileText,
    Folder,
    FolderOpen,
    GitBranch,
    GitCommitHorizontal,
    GitPullRequest,
    Globe,
    House,
    Info,
    Layers,
    Link,
    Lock,
    Maximize,
    Minus,
    Palette,
    Pencil,
    Play,
    Plus,
    RefreshCw,
    Regex,
    RotateCw,
    Rows2,
    Search,
    Server,
    Square,
    Terminal,
    Trash2,
    TriangleAlert,
    WifiOff,
    X,
}

impl IconName {
    /// Every icon, in file order.
    pub const ALL: &'static [IconName] = &[
        Self::ArrowDown,
        Self::ArrowUp,
        Self::BookOpen,
        Self::Bug,
        Self::CaseSensitive,
        Self::Check,
        Self::ChevronDown,
        Self::ChevronRight,
        Self::CircleX,
        Self::Clipboard,
        Self::Code,
        Self::Columns2,
        Self::Copy,
        Self::Eye,
        Self::File,
        Self::FileText,
        Self::Folder,
        Self::FolderOpen,
        Self::GitBranch,
        Self::GitCommitHorizontal,
        Self::GitPullRequest,
        Self::Globe,
        Self::House,
        Self::Info,
        Self::Layers,
        Self::Link,
        Self::Lock,
        Self::Maximize,
        Self::Minus,
        Self::Palette,
        Self::Pencil,
        Self::Play,
        Self::Plus,
        Self::RefreshCw,
        Self::Regex,
        Self::RotateCw,
        Self::Rows2,
        Self::Search,
        Self::Server,
        Self::Square,
        Self::Terminal,
        Self::Trash2,
        Self::TriangleAlert,
        Self::WifiOff,
        Self::X,
    ];

    /// The icon a config names by its Lucide file name (`icon = "maximize-2"`).
    pub fn from_name(name: &str) -> Option<IconName> {
        Self::ALL.iter().copied().find(|i| {
            i.path()
                .strip_prefix("icons/")
                .and_then(|p| p.strip_suffix(".svg"))
                == Some(name)
        })
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::ArrowDown => "icons/arrow-down.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::BookOpen => "icons/book-open.svg",
            Self::Bug => "icons/bug.svg",
            Self::CaseSensitive => "icons/case-sensitive.svg",
            Self::Check => "icons/check.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::CircleX => "icons/circle-x.svg",
            Self::Clipboard => "icons/clipboard.svg",
            Self::Code => "icons/code.svg",
            Self::Columns2 => "icons/columns-2.svg",
            Self::Copy => "icons/copy.svg",
            Self::Eye => "icons/eye.svg",
            Self::File => "icons/file.svg",
            Self::FileText => "icons/file-text.svg",
            Self::Folder => "icons/folder.svg",
            Self::FolderOpen => "icons/folder-open.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::GitCommitHorizontal => "icons/git-commit-horizontal.svg",
            Self::GitPullRequest => "icons/git-pull-request.svg",
            Self::Globe => "icons/globe.svg",
            Self::House => "icons/house.svg",
            Self::Info => "icons/info.svg",
            Self::Layers => "icons/layers.svg",
            Self::Link => "icons/link.svg",
            Self::Lock => "icons/lock.svg",
            Self::Maximize => "icons/maximize-2.svg",
            Self::Minus => "icons/minus.svg",
            Self::Palette => "icons/palette.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::Play => "icons/play.svg",
            Self::Plus => "icons/plus.svg",
            Self::RefreshCw => "icons/refresh-cw.svg",
            Self::Regex => "icons/regex.svg",
            Self::RotateCw => "icons/rotate-cw.svg",
            Self::Rows2 => "icons/rows-2.svg",
            Self::Search => "icons/search.svg",
            Self::Server => "icons/server.svg",
            Self::Square => "icons/square.svg",
            Self::Terminal => "icons/terminal.svg",
            Self::Trash2 => "icons/trash-2.svg",
            Self::TriangleAlert => "icons/triangle-alert.svg",
            Self::WifiOff => "icons/wifi-off.svg",
            Self::X => "icons/x.svg",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_embedded() {
        let all = IconName::ALL;
        for &icon in all {
            let bytes = Assets.load(icon.path()).unwrap();
            assert!(
                bytes.is_some_and(|b| b.starts_with(b"<!--") || b.starts_with(b"<svg")),
                "{icon:?}"
            );
        }
    }

    #[test]
    fn config_icon_names_match_the_bundle() {
        for name in mt_config::menu::ICON_NAMES {
            assert!(IconName::from_name(name).is_some(), "{name}");
        }
        assert_eq!(IconName::ALL.len(), mt_config::menu::ICON_NAMES.len());
        assert_eq!(IconName::from_name("maximize-2"), Some(IconName::Maximize));
    }

    #[test]
    fn bundled_fonts_are_ttf() {
        for font in FONTS {
            assert_eq!(&font[..4], &[0, 1, 0, 0], "not a TrueType font");
        }
    }
}
