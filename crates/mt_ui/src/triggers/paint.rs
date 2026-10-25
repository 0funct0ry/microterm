//! What the viewport draws for triggers: highlight rectangles, the left-edge marker marks and
//! the scrollbar ticks (SPEC §10.11.4). Pure data from the decoration layer and the theme.

use std::collections::BTreeMap;

use gpui::{App, Hsla};
use mt_term::RenderSnapshot;
use mt_triggers::Style;

use super::deco::MAX_ALPHA;
use super::runtime::TriggerRuntime;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub struct DecoRect {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub color: Hsla,
    pub underline: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaintDeco {
    pub rects: Vec<DecoRect>,
    /// Viewport rows with a marker and its color.
    pub edges: Vec<(usize, Hsla)>,
    /// Scrollbar track fractions of the markers and their colors.
    pub ticks: Vec<(f32, Hsla)>,
}

/// A rule color (theme token, tab color name or `#rrggbb`) as a theme-resolved color. Custom
/// colors are lightness-clamped like tab colors, so contrast holds on any theme. Unknown values
/// fall back to the accent.
pub fn color_of(theme: &Theme, name: &str) -> Hsla {
    match name {
        "accent" | "" => theme.accent,
        "warning" => theme.warning,
        "error" => theme.error,
        "success" => theme.success,
        "info" => theme.accent,
        other => match mt_mux::TabColor::parse(other) {
            Ok(c) => crate::theme::hsla(crate::tab_color::resolve_rgba(
                &theme.tab_ui,
                &BTreeMap::new(),
                c,
            )),
            Err(_) => theme.accent,
        },
    }
}

/// The decorations of `pane` for the viewport in `snap`. Empty (and free) while triggers are off.
pub fn for_pane(cx: &App, pane: u64, snap: &RenderSnapshot, theme: &Theme) -> PaintDeco {
    let Some(rt) = cx.try_global::<TriggerRuntime>() else {
        return PaintDeco::default();
    };
    if !rt.deco.has_any(pane) {
        return PaintDeco::default();
    }
    let now = mt_term::now_ms();
    PaintDeco {
        rects: rt
            .deco
            .rects(pane, snap, now)
            .into_iter()
            .map(|r| {
                let base = color_of(theme, &r.color);
                // A background tint never exceeds the cap and never touches the glyph color.
                let color = if r.flash {
                    base.opacity(super::deco::FLASH_ALPHA)
                } else if r.style == Style::Background {
                    base.opacity(MAX_ALPHA)
                } else {
                    base
                };
                DecoRect {
                    row: r.row,
                    start: r.start,
                    end: r.end,
                    color,
                    underline: r.style == Style::Underline && !r.flash,
                }
            })
            .collect(),
        edges: rt
            .deco
            .edge_marks(pane, snap)
            .into_iter()
            .map(|(row, c)| (row, color_of(theme, &c)))
            .collect(),
        ticks: rt
            .deco
            .ticks(pane, snap)
            .into_iter()
            .map(|(f, c)| (f, color_of(theme, &c)))
            .collect(),
    }
}
