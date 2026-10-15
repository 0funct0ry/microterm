//! Pure typography helpers (SPEC §7): OpenType feature lists, the font fallback chain and the
//! ligature cell-span / reshape rules. Nothing here touches GPUI, so it is unit-tested directly.

use std::ops::Range;

/// OpenType features for the terminal font: the ligature toggle (`calt`/`liga`/`dlig` together)
/// followed by the user's extra `features`. Later entries win, so a user entry such as
/// `"calt=0"` overrides the toggle. Entries that do not parse are skipped (the config
/// validator already reports them).
pub fn font_features(ligatures: bool, extra: &[String]) -> Vec<(String, u32)> {
    let on = u32::from(ligatures);
    let mut out: Vec<(String, u32)> = ["calt", "liga", "dlig"]
        .iter()
        .map(|t| (t.to_string(), on))
        .collect();
    for spec in extra {
        if let Some((tag, value)) = mt_config::schema::parse_font_feature(spec) {
            out.retain(|(t, _)| *t != tag);
            out.push((tag, value));
        }
    }
    out
}

/// Why a font sits where it does in the fallback chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainStep {
    Family(String),
    Fallback(usize, String),
    PlatformEmoji(&'static str),
    LastResort,
}

impl ChainStep {
    pub fn font_name(&self) -> Option<&str> {
        match self {
            Self::Family(n) | Self::Fallback(_, n) => Some(n),
            Self::PlatformEmoji(n) => Some(n),
            Self::LastResort => None,
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Self::Family(_) => "font.family".into(),
            Self::Fallback(i, _) => format!("font.fallback[{i}]"),
            Self::PlatformEmoji(_) => "platform emoji font".into(),
            Self::LastResort => "platform last resort".into(),
        }
    }
}

#[cfg(target_os = "macos")]
pub const PLATFORM_EMOJI: &str = "Apple Color Emoji";
#[cfg(not(target_os = "macos"))]
pub const PLATFORM_EMOJI: &str = "Noto Color Emoji";

/// SPEC §7.2 order: `family` → `fallback[]` in order → platform emoji → platform last resort.
/// Empty and duplicate fallback entries are dropped, keeping the first occurrence.
pub fn fallback_chain(family: &str, fallback: &[String]) -> Vec<ChainStep> {
    let mut seen = vec![family.to_string()];
    let mut chain = vec![ChainStep::Family(family.to_string())];
    for (i, f) in fallback.iter().enumerate() {
        let f = f.trim();
        if f.is_empty() || seen.iter().any(|s| s == f) {
            continue;
        }
        seen.push(f.to_string());
        chain.push(ChainStep::Fallback(i, f.to_string()));
    }
    if !seen.iter().any(|s| s == PLATFORM_EMOJI) {
        chain.push(ChainStep::PlatformEmoji(PLATFORM_EMOJI));
    }
    chain.push(ChainStep::LastResort);
    chain
}

/// The family names handed to the text system as `FontFallbacks`: the user's list, then the
/// platform emoji font. (The last resort is the platform's own.)
pub fn fallback_families(family: &str, fallback: &[String]) -> Vec<String> {
    fallback_chain(family, fallback)
        .into_iter()
        .skip(1)
        .filter_map(|s| s.font_name().map(str::to_string))
        .collect()
}

/// The first step of `chain` whose font covers `ch` (SPEC §7.2). `covers(font name, ch)` says
/// whether an installed font has the glyph; the last resort always answers.
pub fn resolve_char(
    chain: &[ChainStep],
    ch: char,
    covers: impl Fn(&str, char) -> bool,
) -> &ChainStep {
    chain
        .iter()
        .find(|step| step.font_name().is_some_and(|n| covers(n, ch)))
        .unwrap_or_else(|| chain.last().expect("chain ends in the last resort"))
}

/// A glyph's identity in a shaped line: `(start byte, glyph id)`. Used to compare a ligated and
/// a non-ligated shaping of the same text.
pub type GlyphKey<G> = (usize, G);

/// For each cell of an ASCII segment (one byte per cell), the glyph that covers it. A glyph
/// covers the cells from its start byte up to the next glyph's start, so a merged N-cell
/// ligature glyph covers N cells and has the same key in each of them.
pub fn cell_glyphs<G: Copy>(glyphs: &[GlyphKey<G>], cells: usize) -> Vec<Option<GlyphKey<G>>> {
    let mut out = vec![None; cells];
    let mut sorted: Vec<GlyphKey<G>> = glyphs.to_vec();
    sorted.sort_by_key(|g| g.0);
    for (i, g) in sorted.iter().enumerate() {
        let end = sorted.get(i + 1).map_or(cells, |n| n.0).min(cells);
        for slot in out.iter_mut().take(end).skip(g.0.min(cells)) {
            *slot = Some(*g);
        }
    }
    out
}

/// Cell ranges (of two or more cells) where the ligated shaping differs from the plain one.
/// Each range is one ligature drawn across exactly `range.len()` cells.
pub fn ligature_spans<G: Copy + PartialEq>(
    ligated: &[GlyphKey<G>],
    plain: &[GlyphKey<G>],
    cells: usize,
) -> Vec<Range<usize>> {
    let a = cell_glyphs(ligated, cells);
    let b = cell_glyphs(plain, cells);
    let mut spans = Vec::new();
    let mut c = 0;
    while c < cells {
        if a[c] == b[c] {
            c += 1;
            continue;
        }
        let start = c;
        while c < cells && a[c] != b[c] {
            c += 1;
        }
        if c - start >= 2 {
            spans.push(start..c);
        }
    }
    spans
}

/// SPEC §7.2: a run is reshaped without ligatures if the cursor cell lies inside a ligature, or
/// a selection boundary (an edge *between* cells `b-1` and `b`) falls strictly inside one.
pub fn needs_reshape(
    spans: &[Range<usize>],
    cursor: Option<usize>,
    selection_edges: &[usize],
) -> bool {
    spans.iter().any(|s| {
        cursor.is_some_and(|c| s.contains(&c))
            || selection_edges.iter().any(|&e| e > s.start && e < s.end)
    })
}

/// Whether a cell can be part of a plain (ASCII, single-width) segment that is shaped together
/// so ligatures can form. Everything else is shaped on its own so fallback fonts, wide glyphs
/// and combining sequences stay locked to the cell grid.
pub fn is_plain_cell(ch: char, has_zerowidth: bool, wide: bool) -> bool {
    !wide && !has_zerowidth && (' '..='~').contains(&ch)
}

/// Regional indicator symbols (U+1F1E6..=U+1F1FF); two in a row make a flag. The engine gives
/// each one cell, so the renderer shapes the pair together across those two cells.
pub fn is_regional_indicator(ch: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&ch)
}

/// Horizontal offset (px) that centres a glyph of `glyph_width` in `cells` cells, never negative.
pub fn centre_offset(cells: usize, cell_width: f32, glyph_width: f32) -> f32 {
    ((cells as f32 * cell_width - glyph_width) / 2.0).max(0.0)
}

/// Font-size scale that makes a glyph of `glyph_width` fit in `cells` cells (≤ 1: never grows).
pub fn fit_scale(cells: usize, cell_width: f32, glyph_width: f32) -> f32 {
    if glyph_width <= 0.0 {
        return 1.0;
    }
    (cells as f32 * cell_width / glyph_width).min(1.0)
}

/// Step the per-window font size; clamped to a usable range.
pub const MIN_FONT_SIZE: f32 = 6.0;
pub const MAX_FONT_SIZE: f32 = 72.0;
pub const FONT_SIZE_STEP: f32 = 1.0;

pub fn step_font_size(current: f32, delta: f32) -> f32 {
    (current + delta).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn ligature_toggle_and_user_features() {
        let on = font_features(true, &s(&["zero", "ss02=1"]));
        assert!(on.contains(&("calt".into(), 1)) && on.contains(&("dlig".into(), 1)));
        assert!(on.contains(&("zero".into(), 1)) && on.contains(&("ss02".into(), 1)));
        let off = font_features(false, &s(&["calt=1", "bad"]));
        assert!(off.contains(&("liga".into(), 0)));
        assert!(
            off.contains(&("calt".into(), 1)),
            "user entry overrides toggle"
        );
        assert_eq!(off.iter().filter(|(t, _)| t == "calt").count(), 1);
    }

    #[test]
    fn chain_order_matches_spec() {
        let c = fallback_chain(
            "JetBrains Mono",
            &s(&["Symbols Nerd Font Mono", "Noto Sans CJK JP"]),
        );
        assert_eq!(c[0], ChainStep::Family("JetBrains Mono".into()));
        assert_eq!(
            c[1],
            ChainStep::Fallback(0, "Symbols Nerd Font Mono".into())
        );
        assert_eq!(c[2], ChainStep::Fallback(1, "Noto Sans CJK JP".into()));
        assert_eq!(c[3], ChainStep::PlatformEmoji(PLATFORM_EMOJI));
        assert_eq!(c[4], ChainStep::LastResort);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn chain_drops_empty_and_duplicate_entries() {
        let c = fallback_chain("A", &s(&["", "A", "B", "B"]));
        assert_eq!(c.len(), 4);
        assert_eq!(c[1], ChainStep::Fallback(2, "B".into()));
        assert_eq!(
            fallback_families("A", &s(&["B"])),
            vec!["B".to_string(), PLATFORM_EMOJI.to_string()]
        );
    }

    #[test]
    fn resolve_char_walks_the_chain_in_order() {
        let chain = fallback_chain("Mono", &s(&["Symbols", "CJK"]));
        let covers = |font: &str, ch: char| match (font, ch) {
            ("Mono", c) => c.is_ascii(),
            ("Symbols", '\u{e0b0}') => true,
            ("CJK", '漢') => true,
            (f, '🦀') => f == PLATFORM_EMOJI,
            _ => false,
        };
        assert_eq!(resolve_char(&chain, 'a', covers), &chain[0]);
        assert_eq!(resolve_char(&chain, '\u{e0b0}', covers), &chain[1]);
        assert_eq!(resolve_char(&chain, '漢', covers), &chain[2]);
        assert_eq!(resolve_char(&chain, '🦀', covers), &chain[3]);
        assert_eq!(resolve_char(&chain, 'ж', covers), &ChainStep::LastResort);
    }

    // Text "a==>b": cells 0..5. Plain shaping: one glyph per cell. Ligated: `==>` is one glyph
    // (merged) covering cells 1..4.
    fn plain() -> Vec<GlyphKey<u32>> {
        (0..5).map(|i| (i, 100 + i as u32)).collect()
    }

    #[test]
    fn merged_ligature_spans_exact_cells() {
        let lig = vec![(0, 100), (1, 900), (4, 104)];
        assert_eq!(ligature_spans(&lig, &plain(), 5), vec![1..4]);
    }

    #[test]
    fn same_count_contextual_alternates_are_spans_too() {
        // JetBrains Mono style: each of `==>` swaps to a different glyph, count unchanged.
        let lig = vec![(0, 100), (1, 801), (2, 802), (3, 803), (4, 104)];
        assert_eq!(ligature_spans(&lig, &plain(), 5), vec![1..4]);
    }

    #[test]
    fn single_cell_differences_are_not_ligatures() {
        let lig = vec![(0, 100), (1, 777), (2, 102), (3, 103), (4, 104)];
        assert!(ligature_spans(&lig, &plain(), 5).is_empty());
        assert!(ligature_spans(&plain(), &plain(), 5).is_empty());
    }

    #[test]
    fn two_separate_ligatures() {
        let p: Vec<GlyphKey<u32>> = (0..7).map(|i| (i, i as u32)).collect();
        let lig = vec![(0, 50), (2, 2), (3, 3), (4, 60), (6, 6)];
        assert_eq!(ligature_spans(&lig, &p, 7), vec![0..2, 4..6]);
    }

    #[test]
    fn cursor_or_selection_inside_a_ligature_forces_reshape() {
        #[allow(clippy::single_range_in_vec_init)]
        let spans = vec![1..4];
        assert!(!needs_reshape(&spans, None, &[]));
        assert!(!needs_reshape(&spans, Some(0), &[]));
        assert!(!needs_reshape(&spans, Some(4), &[]));
        for c in 1..4 {
            assert!(needs_reshape(&spans, Some(c), &[]), "cursor at {c}");
        }
        // Edges: 1 and 4 are the ligature's own outer boundaries; 2 and 3 cut through it.
        assert!(!needs_reshape(&spans, None, &[1, 4]));
        assert!(needs_reshape(&spans, None, &[2]));
        assert!(needs_reshape(&spans, None, &[0, 3]));
    }

    #[test]
    fn plain_cells_are_narrow_ascii_only() {
        assert!(is_plain_cell('a', false, false) && is_plain_cell(' ', false, false));
        assert!(!is_plain_cell('λ', false, false));
        assert!(!is_plain_cell('a', true, false));
        assert!(!is_plain_cell('漢', false, true));
        assert!(!is_plain_cell('\0', false, false));
    }

    #[test]
    fn regional_indicators() {
        assert!(is_regional_indicator('🇯') && is_regional_indicator('🇵'));
        assert!(!is_regional_indicator('J') && !is_regional_indicator('🦀'));
    }

    #[test]
    fn emoji_scale_and_centre() {
        assert_eq!(fit_scale(2, 8.0, 20.0), 0.8);
        assert_eq!(fit_scale(2, 8.0, 10.0), 1.0);
        assert_eq!(centre_offset(2, 8.0, 12.0), 2.0);
        assert_eq!(centre_offset(2, 8.0, 20.0), 0.0);
    }

    #[test]
    fn font_size_steps_clamp() {
        assert_eq!(step_font_size(13.0, FONT_SIZE_STEP), 14.0);
        assert_eq!(step_font_size(6.0, -1.0), MIN_FONT_SIZE);
        assert_eq!(step_font_size(72.0, 1.0), MAX_FONT_SIZE);
    }
}
