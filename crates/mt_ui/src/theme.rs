//! Hard-coded One Dark / One Light tokens (SPEC §4.1). M3 replaces this with the theme crate.

use gpui::{rgb, Global, Hsla};

/// ANSI palette used by the mock terminal grid.
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
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
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
}

impl Global for Theme {}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    pub fn one_dark() -> Self {
        Self {
            name: "One Dark",
            is_dark: true,
            background: c(0x282c33),
            surface: c(0x2f343e),
            elevated_surface: c(0x2f343e),
            border: c(0x464b57),
            border_variant: c(0x363c46),
            text: c(0xdce0e5),
            text_muted: c(0xa9afbc),
            text_placeholder: c(0x878a98),
            element_hover: c(0x363c46),
            element_selected: c(0x454a56),
            accent: c(0x74ade8),
            success: c(0xa1c181),
            warning: c(0xdec184),
            error: c(0xd07277),
            term: TermPalette {
                fg: c(0xdce0e5),
                red: c(0xd07277),
                green: c(0xa1c181),
                yellow: c(0xdec184),
                blue: c(0x74ade8),
                magenta: c(0xb477cf),
                cyan: c(0x6eb4bf),
                dim: c(0x878a98),
            },
        }
    }

    pub fn one_light() -> Self {
        Self {
            name: "One Light",
            is_dark: false,
            background: c(0xfafafa),
            surface: c(0xebebec),
            elevated_surface: c(0xebebec),
            border: c(0xc9c9ca),
            border_variant: c(0xdfdfe0),
            text: c(0x242529),
            text_muted: c(0x58585a),
            text_placeholder: c(0x7e8086),
            element_hover: c(0xdfdfe0),
            element_selected: c(0xd0d0d2),
            accent: c(0x5c78e2),
            success: c(0x669f59),
            warning: c(0xa48819),
            error: c(0xd36151),
            term: TermPalette {
                fg: c(0x242529),
                red: c(0xd36151),
                green: c(0x669f59),
                yellow: c(0xa48819),
                blue: c(0x5c78e2),
                magenta: c(0x994cc3),
                cyan: c(0x3b82b7),
                dim: c(0x7e8086),
            },
        }
    }

    /// Every theme the M1 selector lists.
    pub fn all() -> [Theme; 2] {
        [Self::one_dark(), Self::one_light()]
    }

    pub fn by_name(name: &str) -> Option<Theme> {
        Self::all().into_iter().find(|t| t.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_two_themes_with_distinct_modes() {
        let all = Theme::all();
        assert_eq!(all.len(), 2);
        assert!(all[0].is_dark && !all[1].is_dark);
        assert_ne!(all[0].background, all[1].background);
    }

    #[test]
    fn lookup_by_name() {
        assert!(Theme::by_name("One Light").is_some());
        assert!(Theme::by_name("Nope").is_none());
    }
}
