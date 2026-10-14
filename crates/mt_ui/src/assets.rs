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

const FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/IBMPlexSans_400Regular.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans_500Medium.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans_600SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_400Regular.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_400Regular_Italic.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_700Bold.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono_700Bold_Italic.ttf"),
];

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

/// Registers the bundled IBM Plex Sans and JetBrains Mono faces with the text system.
pub fn load_fonts(cx: &App) -> Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|b| Cow::Borrowed(*b)).collect())
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
