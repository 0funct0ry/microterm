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
    "icons/case-sensitive.svg",
    "icons/check.svg",
    "icons/chevron-down.svg",
    "icons/circle-x.svg",
    "icons/copy.svg",
    "icons/folder.svg",
    "icons/info.svg",
    "icons/layers.svg",
    "icons/minus.svg",
    "icons/palette.svg",
    "icons/plus.svg",
    "icons/refresh-cw.svg",
    "icons/regex.svg",
    "icons/search.svg",
    "icons/square.svg",
    "icons/terminal.svg",
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
    CaseSensitive,
    Check,
    ChevronDown,
    CircleX,
    Copy,
    Folder,
    Info,
    Layers,
    Minus,
    Palette,
    Plus,
    RefreshCw,
    Regex,
    Search,
    Square,
    Terminal,
    TriangleAlert,
    WifiOff,
    X,
}

impl IconName {
    pub fn path(self) -> &'static str {
        match self {
            Self::ArrowDown => "icons/arrow-down.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::CaseSensitive => "icons/case-sensitive.svg",
            Self::Check => "icons/check.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::CircleX => "icons/circle-x.svg",
            Self::Copy => "icons/copy.svg",
            Self::Folder => "icons/folder.svg",
            Self::Info => "icons/info.svg",
            Self::Layers => "icons/layers.svg",
            Self::Minus => "icons/minus.svg",
            Self::Palette => "icons/palette.svg",
            Self::Plus => "icons/plus.svg",
            Self::RefreshCw => "icons/refresh-cw.svg",
            Self::Regex => "icons/regex.svg",
            Self::Search => "icons/search.svg",
            Self::Square => "icons/square.svg",
            Self::Terminal => "icons/terminal.svg",
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
        let all = [
            IconName::ArrowDown,
            IconName::ArrowUp,
            IconName::CaseSensitive,
            IconName::Check,
            IconName::ChevronDown,
            IconName::CircleX,
            IconName::Copy,
            IconName::Folder,
            IconName::Info,
            IconName::Layers,
            IconName::Minus,
            IconName::Palette,
            IconName::Plus,
            IconName::RefreshCw,
            IconName::Regex,
            IconName::Search,
            IconName::Square,
            IconName::Terminal,
            IconName::TriangleAlert,
            IconName::WifiOff,
            IconName::X,
        ];
        for icon in all {
            let bytes = Assets.load(icon.path()).unwrap();
            assert!(
                bytes.is_some_and(|b| b.starts_with(b"<!--") || b.starts_with(b"<svg")),
                "{icon:?}"
            );
        }
    }

    #[test]
    fn bundled_fonts_are_ttf() {
        for font in FONTS {
            assert_eq!(&font[..4], &[0, 1, 0, 0], "not a TrueType font");
        }
    }
}
