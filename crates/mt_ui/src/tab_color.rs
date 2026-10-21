//! Tab colors as drawn (SPEC §4.4): names resolved through the theme, the swatch list of the
//! menu, tooltip text and the status-dot ring. Pure data, asserted without rendering.

use std::collections::BTreeMap;

use mt_config::TabColorStyle;
use mt_mux::{ColorSource, NamedColor, TabColor};
use mt_theme::{fit_tab_color, tint, Rgba, UiTokens, TINT_ACTIVE, TINT_INACTIVE};

use crate::components::StatusKind;

/// A tab's resolved color, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabColorChrome {
    /// `red`, `#f59e42`: what the tooltip, palette and AT label say.
    pub name: String,
    /// The theme-resolved, contrast-clamped color.
    pub rgba: Rgba,
    pub source: ColorSource,
    /// The `match` expression of the rule that colored the tab (`source == Rule`).
    pub rule: Option<String>,
    pub style: TabColorStyle,
}

/// `Color: red`, `Color: red (declared)`, `Color: red (rule: domain:prod-*)`.
pub fn tooltip(name: &str, source: ColorSource, rule: Option<&str>) -> String {
    match (source, rule) {
        (ColorSource::Rule, Some(r)) => format!("Color: {name} (rule: {r})"),
        (ColorSource::Declared, _) => format!("Color: {name} (declared)"),
        _ => format!("Color: {name}"),
    }
}

/// The color to draw for `color`: a built-in name resolves through the theme token (or the
/// `[tab_bar.palette]` override), a custom RGB is used as given. Overrides and custom colors
/// are lightness-clamped so the bar keeps 3:1 and the title keeps 4.5:1 on the tint.
pub fn resolve_rgba(ui: &UiTokens, palette: &BTreeMap<String, String>, color: TabColor) -> Rgba {
    let fit = |c: Rgba| fit_tab_color(c, ui.background, ui.surface, ui.text);
    match color {
        TabColor::Rgb(r, g, b) => fit(Rgba::rgb(r, g, b)),
        TabColor::Named(n) => match palette.get(n.name()).and_then(|h| Rgba::parse(h).ok()) {
            Some(over) => fit(over),
            None => ui
                .tab_color(n.name())
                .expect("every named color has a token"),
        },
    }
}

/// The tab background with the color mixed in (14 % active, 7 % inactive, +3 % on hover).
pub fn tinted_background(ui: &UiTokens, color: Rgba, active: bool, hover: bool) -> Rgba {
    let (base, amount) = if active {
        (ui.background, TINT_ACTIVE)
    } else {
        (ui.surface, TINT_INACTIVE)
    };
    tint(base, color, amount + if hover { 0.03 } else { 0.0 })
}

/// Whether a status dot of `kind` would vanish into a tab of this color (a red tab with an
/// `error` dot): it then gets a 1 px ring in the tab's background color.
pub fn dot_needs_ring(ui: &UiTokens, kind: StatusKind, color: Rgba) -> bool {
    let dot = match kind {
        StatusKind::Connected | StatusKind::Success => ui.success,
        StatusKind::Reconnecting | StatusKind::Warning => ui.warning,
        StatusKind::Error => ui.error,
        StatusKind::Idle => ui.text_placeholder,
        StatusKind::Accent => ui.accent,
    };
    // Distance in OKLab: a dot that is nearly the tab's color would vanish into it.
    let (a, b) = (dot.to_oklch(), color.to_oklch());
    let lab = |c: mt_theme::Oklch| (c.l, c.c * c.h.cos(), c.c * c.h.sin());
    let (l1, a1, b1) = lab(a);
    let (l2, a2, b2) = lab(b);
    ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt() < 0.06
}

/// One swatch of the menu grid and the "Tab: Set color…" list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swatch {
    /// Stable id: `default`, `red`, or the palette name.
    pub id: String,
    /// `Default` for the hollow ring, else the color name.
    pub label: String,
    /// `None` is the hollow ring (clears the manual color).
    pub color: Option<TabColor>,
}

/// The ring, the nine names in order, then the `[tab_bar.palette]` extras (capped at 10).
pub fn swatches(palette: &BTreeMap<String, String>) -> Vec<Swatch> {
    let mut out = vec![Swatch {
        id: "default".into(),
        label: "Default".into(),
        color: None,
    }];
    for n in NamedColor::ALL {
        out.push(Swatch {
            id: n.name().into(),
            label: n.name().into(),
            color: Some(TabColor::Named(n)),
        });
    }
    for (name, hex) in palette
        .iter()
        .filter(|(n, _)| NamedColor::from_name(n).is_none())
        .take(mt_config::MAX_EXTRA_COLORS)
    {
        if let Ok(c @ TabColor::Rgb(..)) = TabColor::parse(hex) {
            out.push(Swatch {
                id: name.clone(),
                label: name.clone(),
                color: Some(c),
            });
        }
    }
    out
}

/// The swatch id that shows the check mark for a tab's manual color.
pub fn selected_swatch(palette: &BTreeMap<String, String>, manual: Option<TabColor>) -> String {
    let Some(m) = manual else {
        return "default".into();
    };
    swatches(palette)
        .into_iter()
        .find(|s| s.color == Some(m))
        .map_or_else(|| "default".into(), |s| s.id)
}

/// Check mark color on a swatch: white or black, whichever contrasts more.
pub fn check_is_white(color: Rgba) -> bool {
    color.contrast(Rgba::rgb(255, 255, 255)) >= color.contrast(Rgba::rgb(0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn ui() -> UiTokens {
        Theme::one_dark().tab_ui
    }

    #[test]
    fn swatches_are_ring_nine_names_then_capped_extras() {
        let mut p = BTreeMap::new();
        assert_eq!(swatches(&p).len(), 10);
        assert_eq!(swatches(&p)[0].color, None);
        assert_eq!(swatches(&p)[1].id, "blue");
        assert_eq!(swatches(&p)[9].id, "gray");
        p.insert("orange".into(), "#f59e42".into());
        assert_eq!(
            swatches(&p).len(),
            10,
            "overriding a built-in adds no swatch"
        );
        for i in 0..10 {
            p.insert(format!("x{i}"), "#112233".into());
        }
        let s = swatches(&p);
        assert_eq!(s.len(), 20);
        assert_eq!(s[10].id, "x0");
    }

    #[test]
    fn the_check_follows_the_manual_color() {
        let mut p = BTreeMap::new();
        p.insert("brand".into(), "#112233".into());
        assert_eq!(selected_swatch(&p, None), "default");
        assert_eq!(
            selected_swatch(&p, Some(TabColor::Named(NamedColor::Pink))),
            "pink"
        );
        assert_eq!(
            selected_swatch(&p, Some(TabColor::Rgb(0x11, 0x22, 0x33))),
            "brand"
        );
        assert_eq!(
            selected_swatch(&p, Some(TabColor::Rgb(1, 2, 3))),
            "default",
            "a custom color from ctl has no swatch"
        );
    }

    #[test]
    fn named_colors_use_tokens_and_palette_overrides_are_clamped() {
        let ui = ui();
        let p = BTreeMap::new();
        let red = resolve_rgba(&ui, &p, TabColor::Named(NamedColor::Red));
        assert_eq!(red, ui.tab_color_red);
        let mut p = BTreeMap::new();
        p.insert("red".into(), "#101010".into());
        let low = resolve_rgba(&ui, &p, TabColor::Named(NamedColor::Red));
        assert!(low.contrast(ui.background) >= 3.0 && low.contrast(ui.surface) >= 3.0);
        let custom = resolve_rgba(&ui, &BTreeMap::new(), TabColor::Rgb(0x10, 0x10, 0x10));
        assert!(custom.contrast(ui.background) >= 3.0);
    }

    #[test]
    fn tint_is_stronger_when_active_and_on_hover() {
        let ui = ui();
        let c = ui.tab_color_pink;
        let inactive = tinted_background(&ui, c, false, false);
        let hover = tinted_background(&ui, c, false, true);
        let active = tinted_background(&ui, c, true, false);
        assert_ne!(inactive, ui.surface);
        assert!(
            hover.contrast(c) < inactive.contrast(c),
            "hover is closer to the color"
        );
        assert!(active.contrast(c) < ui.background.contrast(c));
    }

    #[test]
    fn a_red_tab_rings_an_error_dot_but_not_a_success_dot() {
        let ui = ui();
        assert!(dot_needs_ring(&ui, StatusKind::Error, ui.tab_color_red));
        assert!(dot_needs_ring(&ui, StatusKind::Accent, ui.tab_color_blue));
        assert!(!dot_needs_ring(&ui, StatusKind::Success, ui.tab_color_red));
    }

    #[test]
    fn tooltips_name_the_color_and_its_source() {
        assert_eq!(tooltip("red", ColorSource::Manual, None), "Color: red");
        assert_eq!(
            tooltip("red", ColorSource::Declared, None),
            "Color: red (declared)"
        );
        assert_eq!(
            tooltip("red", ColorSource::Rule, Some("domain:prod-*")),
            "Color: red (rule: domain:prod-*)"
        );
    }

    #[test]
    fn check_mark_contrasts() {
        assert!(check_is_white(Rgba::rgb(0x20, 0x30, 0x80)));
        assert!(!check_is_white(Rgba::rgb(0xff, 0xe0, 0x60)));
    }
}
