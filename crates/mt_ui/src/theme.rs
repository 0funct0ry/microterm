//! The active theme as GPUI colours (SPEC §4.1, §8.4). Themes come from `mt_theme`; this module
//! only converts them. The theme is a GPUI global so every view repaints from it.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use gpui::{Global, Hsla};
use mt_theme::Rgba as ThemeRgba;

/// Terminal colours of a theme.
#[derive(Clone, Copy, Debug)]
pub struct TermPalette {
    pub fg: Hsla,
    pub red: Hsla,
    pub green: Hsla,
    pub yellow: Hsla,
    pub blue: Hsla,
    pub magenta: Hsla,
    pub cyan: Hsla,
    pub dim: Hsla,
    /// Full 16-colour ANSI palette (0-7 normal, 8-15 bright) for the live terminal.
    pub ansi: [Hsla; 16],
    pub cursor: Hsla,
    /// Colour of the glyph under a block cursor.
    pub cursor_text: Hsla,
    pub selection_bg: Hsla,
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Interned, so `Theme` stays `Copy` while names come from user files.
    pub name: &'static str,
    pub is_dark: bool,
    pub background: Hsla,
    pub surface: Hsla,
    pub elevated_surface: Hsla,
    pub border: Hsla,
    pub border_variant: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_placeholder: Hsla,
    pub element_hover: Hsla,
    pub element_selected: Hsla,
    pub accent: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub term: TermPalette,
    /// The resolved `mt_theme` tokens, kept as RGB for tab-color contrast math (SPEC §4.4).
    pub tab_ui: mt_theme::UiTokens,
}

impl Global for Theme {}

fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = NAMES.get_or_init(Mutex::default).lock().unwrap();
    if let Some(existing) = set.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    set.insert(leaked);
    leaked
}

/// A theme color as GPUI's `Hsla`.
pub fn hsla(color: ThemeRgba) -> Hsla {
    c(color)
}

fn c(color: ThemeRgba) -> Hsla {
    gpui::Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: color.alpha(),
    }
    .into()
}

/// The bundled themes, parsed once.
fn bundled() -> &'static mt_theme::Registry {
    static BUNDLED: OnceLock<mt_theme::Registry> = OnceLock::new();
    BUNDLED.get_or_init(mt_theme::Registry::bundled)
}

impl Theme {
    pub fn from_mt(t: &mt_theme::Theme) -> Self {
        let ui = &t.ui;
        let ansi = t.ansi.map(c);
        Self {
            name: intern(&t.name),
            is_dark: t.is_dark(),
            background: c(ui.background),
            surface: c(ui.surface),
            elevated_surface: c(ui.elevated_surface),
            border: c(ui.border),
            border_variant: c(ui.border_variant),
            text: c(ui.text),
            text_muted: c(ui.text_muted),
            text_placeholder: c(ui.text_placeholder),
            element_hover: c(ui.element_hover),
            element_selected: c(ui.element_selected),
            accent: c(ui.accent),
            success: c(ui.success),
            warning: c(ui.warning),
            error: c(ui.error),
            term: TermPalette {
                fg: c(t.foreground),
                red: ansi[1],
                green: ansi[2],
                yellow: ansi[3],
                blue: ansi[4],
                magenta: ansi[5],
                cyan: ansi[6],
                dim: c(ui.text_placeholder),
                ansi,
                cursor: c(t.cursor),
                cursor_text: c(t.cursor_text),
                selection_bg: c(t.selection_bg),
            },
            tab_ui: t.ui,
        }
    }

    fn bundled_named(name: &str) -> Self {
        Self::from_mt(bundled().get(name).expect("default themes are bundled"))
    }

    pub fn one_dark() -> Self {
        Self::bundled_named(mt_theme::DEFAULT_DARK)
    }

    pub fn one_light() -> Self {
        Self::bundled_named(mt_theme::DEFAULT_LIGHT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_distinct_modes() {
        let (d, l) = (Theme::one_dark(), Theme::one_light());
        assert!(d.is_dark && !l.is_dark);
        assert_ne!(d.background, l.background);
        assert_eq!(d.name, "One Dark");
    }

    #[test]
    fn names_are_interned() {
        let a = Theme::one_dark().name.as_ptr();
        let b = Theme::one_dark().name.as_ptr();
        assert_eq!(a, b);
    }

    #[test]
    fn one_dark_matches_spec_tokens() {
        let t = Theme::one_dark();
        assert_eq!(t.background, c(ThemeRgba::rgb(0x28, 0x2c, 0x33)));
        assert_eq!(t.accent, c(ThemeRgba::rgb(0x74, 0xad, 0xe8)));
        assert_eq!(t.surface, c(ThemeRgba::rgb(0x2f, 0x34, 0x3e)));
        assert_eq!(t.border, c(ThemeRgba::rgb(0x46, 0x4b, 0x57)));
        assert_eq!(t.element_selected, c(ThemeRgba::rgb(0x45, 0x4a, 0x56)));
        assert_eq!(t.term.ansi[1], c(ThemeRgba::rgb(0xd0, 0x72, 0x77)));
    }

    #[test]
    fn every_bundled_theme_converts() {
        for t in bundled().all() {
            let th = Theme::from_mt(t);
            assert_eq!(th.is_dark, t.is_dark());
            assert_eq!(th.term.ansi.len(), 16);
        }
    }
}
