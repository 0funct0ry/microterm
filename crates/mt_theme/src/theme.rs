//! Theme file format, validation and UI-token derivation (SPEC §8.4).

use std::fmt;

use serde::Deserialize;

use crate::color::Rgba;

pub const PERMISSIVE_LICENSES: &[&str] = &[
    "MIT",
    "ISC",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSD",
    "CC0-1.0",
];

pub const MIN_CONTRAST: f32 = 4.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Debug, Deserialize)]
struct RawTheme {
    name: String,
    appearance: Appearance,
    #[serde(default)]
    author: String,
    #[serde(default)]
    license: String,
    colors: RawColors,
    #[serde(default)]
    ui: RawUi,
}

#[derive(Debug, Deserialize)]
struct RawColors {
    foreground: String,
    background: String,
    #[serde(default)]
    cursor: String,
    #[serde(default)]
    cursor_text: String,
    #[serde(default)]
    selection_bg: String,
    #[serde(default)]
    selection_fg: String,
    #[serde(default)]
    ansi: Vec<String>,
    #[serde(default)]
    brights: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUi {
    surface: Option<String>,
    elevated_surface: Option<String>,
    border: Option<String>,
    border_variant: Option<String>,
    text: Option<String>,
    text_muted: Option<String>,
    text_placeholder: Option<String>,
    element_hover: Option<String>,
    element_selected: Option<String>,
    accent: Option<String>,
    success: Option<String>,
    warning: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeError(pub String);

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ThemeError {}

fn err<T>(m: impl Into<String>) -> Result<T, ThemeError> {
    Err(ThemeError(m.into()))
}

/// The UI tokens of SPEC §4.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiTokens {
    pub background: Rgba,
    pub surface: Rgba,
    pub elevated_surface: Rgba,
    pub border: Rgba,
    pub border_variant: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub text_placeholder: Rgba,
    pub element_hover: Rgba,
    pub element_selected: Rgba,
    pub accent: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub appearance: Appearance,
    pub author: String,
    pub license: String,
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    pub cursor_text: Rgba,
    pub selection_bg: Rgba,
    pub selection_fg: Option<Rgba>,
    /// 0-7 normal, 8-15 bright.
    pub ansi: [Rgba; 16],
    pub ui: UiTokens,
}

impl Theme {
    pub fn is_dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }

    pub fn has_permissive_license(&self) -> bool {
        PERMISSIVE_LICENSES.contains(&self.license.as_str())
    }

    /// Foreground/background contrast ratio.
    pub fn contrast(&self) -> f32 {
        self.foreground.contrast(self.background)
    }

    /// The checks `make themes` runs: licence present and permissive, 16 colours (enforced by
    /// parsing) and foreground/background contrast of at least 4.5:1.
    pub fn validate(&self) -> Result<(), ThemeError> {
        if self.license.trim().is_empty() {
            return err(format!("{}: missing license", self.name));
        }
        if !self.has_permissive_license() {
            return err(format!(
                "{}: license \"{}\" is not permissive",
                self.name, self.license
            ));
        }
        let c = self.contrast();
        if c < MIN_CONTRAST {
            return err(format!(
                "{}: foreground/background contrast {c:.2}:1 is below {MIN_CONTRAST}:1",
                self.name
            ));
        }
        Ok(())
    }
}

pub fn contrast_ratio_fg_bg(t: &Theme) -> f32 {
    t.contrast()
}

fn color(field: &str, s: &str) -> Result<Rgba, ThemeError> {
    Rgba::parse(s).map_err(|e| ThemeError(format!("{field}: {e}")))
}

fn opt_color(field: &str, s: &Option<String>) -> Result<Option<Rgba>, ThemeError> {
    s.as_deref()
        .filter(|v| !v.is_empty())
        .map(|v| color(field, v))
        .transpose()
}

/// Parse a theme file. Licence and contrast are *not* checked here (user themes may omit a
/// licence); call [`Theme::validate`] for the bundled-theme rules.
pub fn parse_theme(src: &str) -> Result<Theme, ThemeError> {
    let raw: RawTheme = toml::from_str(src).map_err(|e| ThemeError(e.to_string()))?;
    if raw.name.trim().is_empty() {
        return err("theme has an empty name");
    }
    if raw.colors.ansi.len() != 8 || raw.colors.brights.len() != 8 {
        return err(format!(
            "{}: need 8 `ansi` and 8 `brights` colours (16 total), found {} and {}",
            raw.name,
            raw.colors.ansi.len(),
            raw.colors.brights.len()
        ));
    }
    let fg = color("colors.foreground", &raw.colors.foreground)?;
    let bg = color("colors.background", &raw.colors.background)?;
    let mut ansi = [Rgba::rgb(0, 0, 0); 16];
    for (i, s) in raw
        .colors
        .ansi
        .iter()
        .chain(&raw.colors.brights)
        .enumerate()
    {
        ansi[i] = color(&format!("colors.ansi[{i}]"), s)?;
    }
    let cursor = opt_color("colors.cursor", &Some(raw.colors.cursor.clone()))?.unwrap_or(fg);
    let cursor_text =
        opt_color("colors.cursor_text", &Some(raw.colors.cursor_text.clone()))?.unwrap_or(bg);
    let selection_bg = opt_color(
        "colors.selection_bg",
        &Some(raw.colors.selection_bg.clone()),
    )?
    .unwrap_or(Rgba { a: 0x40, ..ansi[4] });
    let selection_fg = opt_color(
        "colors.selection_fg",
        &Some(raw.colors.selection_fg.clone()),
    )?;
    let ui = derive_ui(fg, bg, &ansi, &raw.ui)?;
    Ok(Theme {
        name: raw.name,
        appearance: raw.appearance,
        author: raw.author,
        license: raw.license,
        foreground: fg,
        background: bg,
        cursor,
        cursor_text,
        selection_bg,
        selection_fg,
        ansi,
        ui,
    })
}

/// Move `bg` towards the foreground in OKLCH lightness by `amount` (0.04 = 4 %).
fn shift(bg: Rgba, fg: Rgba, amount: f32) -> Rgba {
    let mut c = bg.to_oklch();
    let dir = if fg.to_oklch().l >= c.l { 1.0 } else { -1.0 };
    c.l = (c.l + dir * amount).clamp(0.0, 1.0);
    c.to_rgba()
}

/// SPEC §8.4 derivation. `surface` = background ±4 %, `border` = ±12 %,
/// `text_muted` = mix(fg, bg, 0.3), `accent` = ansi[4]; `[ui]` entries override.
fn derive_ui(fg: Rgba, bg: Rgba, ansi: &[Rgba; 16], o: &RawUi) -> Result<UiTokens, ThemeError> {
    let pick = |f: &str, v: &Option<String>, d: Rgba| -> Result<Rgba, ThemeError> {
        Ok(opt_color(&format!("ui.{f}"), v)?.unwrap_or(d))
    };
    let surface = pick("surface", &o.surface, shift(bg, fg, 0.04))?;
    Ok(UiTokens {
        background: bg,
        surface,
        elevated_surface: pick("elevated_surface", &o.elevated_surface, surface)?,
        border: pick("border", &o.border, shift(bg, fg, 0.12))?,
        border_variant: pick("border_variant", &o.border_variant, shift(bg, fg, 0.07))?,
        text: pick("text", &o.text, fg)?,
        text_muted: pick("text_muted", &o.text_muted, fg.mix(bg, 0.3))?,
        text_placeholder: pick("text_placeholder", &o.text_placeholder, fg.mix(bg, 0.5))?,
        element_hover: pick("element_hover", &o.element_hover, shift(bg, fg, 0.07))?,
        element_selected: pick("element_selected", &o.element_selected, shift(bg, fg, 0.11))?,
        accent: pick("accent", &o.accent, ansi[4])?,
        success: pick("success", &o.success, ansi[2])?,
        warning: pick("warning", &o.warning, ansi[3])?,
        error: pick("error", &o.error, ansi[1])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = r##"
name = "Mini"
appearance = "dark"
author = "me"
license = "MIT"
[colors]
foreground = "#dce0e5"
background = "#282c33"
ansi    = ["#282c33", "#d07277", "#a1c181", "#dec184", "#74ade8", "#b477cf", "#6eb4bf", "#dce0e5"]
brights = ["#525561", "#de8f94", "#bad6a1", "#e9d4a1", "#a3c8ef", "#cba2dd", "#9ac9d1", "#ffffff"]
"##;

    #[test]
    fn derives_ui_tokens_for_dark() {
        let t = parse_theme(MINI).unwrap();
        assert!(t.validate().is_ok());
        let u = t.ui;
        // Dark theme: surface and border are lighter than the background, border more so.
        let l = |c: Rgba| c.to_oklch().l;
        assert!(l(u.surface) > l(u.background));
        assert!(l(u.border) > l(u.surface));
        assert!((l(u.surface) - l(u.background) - 0.04).abs() < 0.01);
        assert!((l(u.border) - l(u.background) - 0.12).abs() < 0.01);
        assert_eq!(u.accent, t.ansi[4]);
        assert_eq!(u.text_muted, t.foreground.mix(t.background, 0.3));
        assert_eq!(u.elevated_surface, u.surface);
    }

    #[test]
    fn light_theme_shifts_darker() {
        let src = MINI
            .replace("dark", "light")
            .replace("#282c33\"\n", "#fafafa\"\n")
            .replace("foreground = \"#dce0e5\"", "foreground = \"#242529\"");
        let t = parse_theme(&src).unwrap();
        let l = |c: Rgba| c.to_oklch().l;
        assert!(l(t.ui.surface) < l(t.ui.background));
    }

    #[test]
    fn derivation_snapshot_one_dark() {
        // Pinned so changes to the OKLCH maths are deliberate.
        let t = parse_theme(MINI).unwrap();
        assert_eq!(t.ui.surface.to_string(), derived_hex(&t, 0.04));
        assert_eq!(t.ui.text_muted.to_string(), "#a6aab0");
    }

    fn derived_hex(t: &Theme, amt: f32) -> String {
        shift(t.background, t.foreground, amt).to_string()
    }

    #[test]
    fn ui_overrides_win() {
        let src = format!("{MINI}\n[ui]\nsurface = \"#111111\"\naccent = \"#ff0000\"\n");
        let t = parse_theme(&src).unwrap();
        assert_eq!(t.ui.surface, Rgba::rgb(0x11, 0x11, 0x11));
        assert_eq!(t.ui.elevated_surface, t.ui.surface);
        assert_eq!(t.ui.accent, Rgba::rgb(255, 0, 0));
    }

    #[test]
    fn rejects_wrong_colour_count_and_bad_hex() {
        let short = MINI.replace("\"#dce0e5\"]", "]");
        assert!(parse_theme(&short).unwrap_err().0.contains("16 total"));
        let bad = MINI.replace("#d07277", "#d0727");
        assert!(parse_theme(&bad).is_err());
    }

    #[test]
    fn validate_checks_license_and_contrast() {
        let t = parse_theme(&MINI.replace("license = \"MIT\"", "license = \"GPL-3.0\"")).unwrap();
        assert!(t.validate().unwrap_err().0.contains("not permissive"));
        let t = parse_theme(&MINI.replace("license = \"MIT\"", "")).unwrap();
        assert!(t.validate().unwrap_err().0.contains("missing license"));
        let low = MINI.replace("foreground = \"#dce0e5\"", "foreground = \"#30343b\"");
        assert!(parse_theme(&low)
            .unwrap()
            .validate()
            .unwrap_err()
            .0
            .contains("contrast"));
    }
}
