//! GPU painting of a live terminal grid (SPEC §6.2): merged background quads, shaped text per
//! row (cached by row hash), decorations drawn as quads/paths, procedural glyphs and the cursor.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    fill, font, point, px, size, Bounds, FontFallbacks, FontFeatures, FontStyle, FontWeight, Hsla,
    PathBuilder, Pixels, ShapedLine, SharedString, TextRun, Window,
};
use mt_term::{Cell, CellColor, CellFlags, CursorShape, RenderSnapshot, Underline};

use crate::{builtin_glyphs, metrics::*, theme::Theme, typography};

/// Measured size of one terminal cell, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub descent: f32,
    pub font_size: f32,
}

/// The terminal font from the config (SPEC §7.1): family, weights, OpenType features and the
/// fallback chain handed to the text system.
#[derive(Clone, Debug, PartialEq)]
pub struct TermFont {
    pub family: SharedString,
    pub weight: f32,
    pub bold_weight: f32,
    pub ligatures: bool,
    features_on: Vec<(String, u32)>,
    features_off: Vec<(String, u32)>,
    fallbacks: Vec<String>,
}

impl Default for TermFont {
    fn default() -> Self {
        Self::from_config(&mt_config::Font::default())
    }
}

impl TermFont {
    pub fn from_config(f: &mt_config::Font) -> Self {
        Self {
            family: f.family.clone().into(),
            weight: f.weight.numeric().map_or(400.0, f32::from),
            bold_weight: f.bold_weight.numeric().map_or(700.0, f32::from),
            ligatures: f.ligatures,
            features_on: typography::font_features(f.ligatures, &f.features),
            features_off: typography::font_features(false, &f.features),
            fallbacks: typography::fallback_families(&f.family, &f.fallback),
        }
    }

    /// The GPUI font for a style run, with the configured ligature setting.
    pub fn font(&self, bold: bool, italic: bool) -> gpui::Font {
        self.font_with(bold, italic, self.ligatures)
    }

    /// Like [`font`](Self::font) but with ligatures forced off when `ligatures` is false. Used to
    /// reshape a run whose cursor or selection boundary sits inside a ligature.
    pub fn font_with(&self, bold: bool, italic: bool, ligatures: bool) -> gpui::Font {
        let mut f = font(self.family.clone());
        f.weight = FontWeight(if bold { self.bold_weight } else { self.weight });
        if italic {
            f.style = FontStyle::Italic;
        }
        let features = if ligatures {
            &self.features_on
        } else {
            &self.features_off
        };
        f.features = FontFeatures(Arc::new(features.clone()));
        f.fallbacks = Some(FontFallbacks::from_fonts(self.fallbacks.clone()));
        f
    }
}

impl Default for CellMetrics {
    fn default() -> Self {
        CellMetrics {
            width: CELL_WIDTH,
            height: LINE_HEIGHT,
            ascent: TERM_TEXT_SIZE * 0.8,
            descent: TERM_TEXT_SIZE * 0.2,
            font_size: TERM_TEXT_SIZE,
        }
    }
}

impl CellMetrics {
    /// Measures the terminal font; falls back to the defaults if it is unavailable.
    pub fn measure(window: &Window, family: &TermFont, font_size: f32, line_height: f32) -> Self {
        let ts = window.text_system();
        let id = ts.resolve_font(&family.font(false, false));
        let scale = window.scale_factor().max(1.0);
        let snap = |v: f32| (v * scale).round() / scale;
        let fs = px(font_size);
        let Ok(adv) = ts.advance(id, fs, 'M') else {
            return Self::default();
        };
        let width = snap(f32::from(adv.width)).max(1.0);
        let height = snap(font_size * line_height).max(1.0);
        CellMetrics {
            width,
            height,
            ascent: f32::from(ts.ascent(id, fs)),
            descent: f32::from(ts.descent(id, fs)).abs(),
            font_size,
        }
    }

    /// Grid size that fits `bounds`.
    pub fn grid_for(&self, width: f32, height: f32) -> (u16, u16) {
        let cols = (width / self.width).floor().clamp(1.0, 1000.0) as u16;
        let rows = (height / self.height).floor().clamp(1.0, 1000.0) as u16;
        (cols, rows)
    }
}

/// One shaped piece of a row, painted at `col` cells from the left plus `dx` pixels. Plain ASCII
/// stretches are one segment (so ligatures can form); every other cell is its own segment so
/// fallback, wide and combining glyphs stay locked to the grid.
pub struct Segment {
    pub col: usize,
    pub dx: f32,
    pub line: ShapedLine,
}

pub type ShapedRow = Rc<Vec<Segment>>;

/// Shaped rows keyed by `(row hash + cursor/ligature salt, theme/font generation)`.
#[derive(Default)]
pub struct LineCache {
    map: HashMap<(u64, u64), ShapedRow>,
}

impl LineCache {
    fn get_or_shape(
        &mut self,
        key: (u64, u64),
        capacity: usize,
        shape: impl FnOnce() -> ShapedRow,
    ) -> ShapedRow {
        if let Some(line) = self.map.get(&key) {
            return line.clone();
        }
        if self.map.len() >= capacity {
            self.map.clear();
        }
        let line = shape();
        self.map.insert(key, line.clone());
        line
    }
}

/// Everything the painter needs for one frame.
pub struct TerminalPaint {
    pub snapshot: Arc<RenderSnapshot>,
    pub theme: Theme,
    pub metrics: CellMetrics,
    /// Whether the viewport has keyboard focus and the window is active.
    pub focused: bool,
    /// Blink phase: false hides a blinking cursor.
    pub cursor_on: bool,
    pub cache: Rc<RefCell<LineCache>>,
    /// Bumped when the font changes so cached lines are discarded.
    pub font_generation: u64,
    pub font: TermFont,
    /// Hyperlink under the pointer: `(row, start col, end col exclusive)`; drawn with a
    /// dotted accent underline.
    pub hover_link: Option<(usize, usize, usize)>,
    /// Trigger highlights and marker marks (SPEC §10.11.4); empty while triggers are off.
    pub deco: crate::triggers::paint::PaintDeco,
}

/// Selected columns `start..end` (end exclusive) on viewport row `r`.
pub fn selection_cols(
    span: &mt_term::SelectionSpan,
    r: usize,
    cols: usize,
) -> Option<(usize, usize)> {
    let r = r as i32;
    if r < span.start.0 || r > span.end.0 || cols == 0 {
        return None;
    }
    let (s, e) = if span.block {
        (span.start.1.min(span.end.1), span.start.1.max(span.end.1))
    } else {
        (
            if r == span.start.0 { span.start.1 } else { 0 },
            if r == span.end.0 {
                span.end.1
            } else {
                cols - 1
            },
        )
    };
    (s <= e).then(|| (s.min(cols - 1), (e + 1).min(cols)))
}

fn rgb_hsla(r: u8, g: u8, b: u8) -> Hsla {
    gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into()
}

pub fn resolve_color(c: CellColor, t: &Theme) -> Hsla {
    match c {
        CellColor::DefaultFg => t.term.fg,
        CellColor::DefaultBg => t.background,
        CellColor::Rgb(r, g, b) => rgb_hsla(r, g, b),
        CellColor::Indexed(i) => match i {
            0..=15 => t.term.ansi[i as usize],
            16..=231 => {
                let n = (i - 16) as usize;
                let lvl = |v: usize| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
                rgb_hsla(lvl(n / 36), lvl((n / 6) % 6), lvl(n % 6))
            }
            _ => {
                let v = (8 + 10 * (i as usize - 232)) as u8;
                rgb_hsla(v, v, v)
            }
        },
    }
}

/// Foreground and background of a cell after inverse/dim/hidden are applied.
fn cell_colors(cell: &Cell, t: &Theme) -> (Hsla, Hsla) {
    let mut fg = resolve_color(cell.fg, t);
    let mut bg = resolve_color(cell.bg, t);
    if cell.flags.has(CellFlags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.has(CellFlags::DIM) {
        fg = fg.opacity(0.6);
    }
    if cell.flags.has(CellFlags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

fn has_visible_bg(cell: &Cell) -> bool {
    cell.bg != CellColor::DefaultBg || cell.flags.has(CellFlags::INVERSE)
}

/// Applies the program's colour overrides to one row: bold text colour (OSC 5;0), underline
/// colour (OSC 5;1) and selection foreground (OSC 19). `None` when nothing changes.
fn adjust_row(
    row: &[Cell],
    vt: &mt_term::VtView,
    sel: Option<(usize, usize)>,
) -> Option<Vec<Cell>> {
    if vt.bold_color.is_none() && vt.underline_color.is_none() && vt.selection_fg.is_none() {
        return None;
    }
    let rgb = |[r, g, b]: [u8; 3]| CellColor::Rgb(r, g, b);
    let mut out: Option<Vec<Cell>> = None;
    for (c, cell) in row.iter().enumerate() {
        let mut new = cell.clone();
        if let Some(bold) = vt.bold_color {
            if cell.flags.has(CellFlags::BOLD) {
                new.fg = rgb(bold);
            }
        }
        if let Some(ul) = vt.underline_color {
            if cell.underline != Underline::None && cell.underline_color.is_none() {
                new.underline_color = Some(rgb(ul));
            }
        }
        if let (Some(fg), Some((a, b))) = (vt.selection_fg, sel) {
            if (a..b).contains(&c) {
                new.fg = rgb(fg);
                new.flags = CellFlags(new.flags.0 & !CellFlags::INVERSE);
            }
        }
        if new != *cell {
            out.get_or_insert_with(|| row.to_vec())[c] = new;
        }
    }
    out
}

/// Cache-key part for rows drawn with overrides: the overrides themselves and the selection.
fn vt_key(vt: &mt_term::VtView, sel: Option<(usize, usize)>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |v: u64| h = (h ^ v).wrapping_mul(0x100000001b3);
    for c in [vt.bold_color, vt.underline_color, vt.selection_fg] {
        mix(c.map_or(1 << 40, |[r, g, b]| {
            u64::from(r) << 16 | u64::from(g) << 8 | u64::from(b)
        }));
    }
    if let Some((a, b)) = sel {
        mix((a as u64) << 20 | b as u64);
    }
    h
}

fn theme_key(t: &Theme, font_generation: u64) -> u64 {
    // Names are unique per theme; the dark flag guards against same-name variants.
    let mut h = font_generation ^ ((t.is_dark as u64) << 63);
    for b in t.name.bytes() {
        h = h.wrapping_mul(0x100000001b3) ^ b as u64;
    }
    h
}

/// Shapes a plain ASCII stretch. If every glyph maps to one cell the line is re-shaped with
/// `force_width` so rounding drift cannot accumulate; if a merged ligature glyph exists the
/// font's own advances are kept so the ligature spans exactly its cells.
fn shape_plain(window: &Window, text: &str, runs: &[TextRun], m: &CellMetrics) -> ShapedLine {
    let text = SharedString::from(text.to_string());
    let ts = window.text_system();
    let free = ts.shape_line(text.clone(), px(m.font_size), runs, None);
    let glyphs: usize = free.runs.iter().map(|r| r.glyphs.len()).sum();
    if glyphs == text.len() {
        ts.shape_line(text, px(m.font_size), runs, Some(px(m.width)))
    } else {
        free
    }
}

fn glyph_keys(line: &ShapedLine) -> Vec<typography::GlyphKey<gpui::GlyphId>> {
    line.runs
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| (g.index, g.id)))
        .collect()
}

type Style = (Hsla, bool, bool);

fn cell_style(cell: &Cell, t: &Theme) -> Style {
    (
        cell_colors(cell, t).0,
        cell.flags.has(CellFlags::BOLD),
        cell.flags.has(CellFlags::ITALIC),
    )
}

/// The character drawn for a cell inside a plain segment (blanks, procedural and hidden glyphs
/// become spaces: they are painted separately).
fn is_blank_cell(cell: &Cell, prev_wide: bool) -> bool {
    (cell.flags.has(CellFlags::WIDE_SPACER) && !prev_wide)
        || builtin_glyphs::is_builtin(cell.ch)
        || cell.ch == '\0'
        || cell.flags.has(CellFlags::HIDDEN)
}

/// Builds the text runs of one plain segment for the given ligature setting.
fn plain_runs(cells: &[(char, Style)], font: &TermFont, ligatures: bool) -> (String, Vec<TextRun>) {
    let mut text = String::with_capacity(cells.len());
    let mut runs: Vec<TextRun> = Vec::new();
    let mut prev: Option<Style> = None;
    for (ch, style) in cells {
        text.push(*ch);
        if prev == Some(*style) {
            runs.last_mut().expect("run exists").len += 1;
            continue;
        }
        prev = Some(*style);
        runs.push(TextRun {
            len: 1,
            font: font.font_with(style.1, style.2, ligatures),
            color: style.0,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    (text, runs)
}

/// Shapes one non-plain cell (fallback, wide, emoji or combining sequence) on its own: scaled
/// down if wider than its cells, then centred in them.
fn shape_cell(
    window: &Window,
    cell: &Cell,
    color: Hsla,
    m: &CellMetrics,
    font: &TermFont,
) -> (ShapedLine, f32) {
    let cells = if cell.flags.has(CellFlags::WIDE) {
        2
    } else {
        1
    };
    let mut text = String::from(cell.ch);
    if let Some(z) = &cell.zerowidth {
        text.extend(z.iter());
    }
    shape_glyphs(window, text, cells, cell, color, m, font)
}

/// Shapes `text` (one cluster) into `cells` cells using `cell`'s bold/italic flags.
fn shape_glyphs(
    window: &Window,
    text: String,
    cells: usize,
    cell: &Cell,
    color: Hsla,
    m: &CellMetrics,
    font: &TermFont,
) -> (ShapedLine, f32) {
    let text = SharedString::from(text);
    let f = font.font(
        cell.flags.has(CellFlags::BOLD),
        cell.flags.has(CellFlags::ITALIC),
    );
    let run = |len: usize| TextRun {
        len,
        font: f.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let ts = window.text_system();
    let mut line = ts.shape_line(text.clone(), px(m.font_size), &[run(text.len())], None);
    let w = f32::from(line.width);
    let scale = typography::fit_scale(cells, m.width, w);
    let mut width = w;
    if scale < 1.0 {
        line = ts.shape_line(
            text.clone(),
            px(m.font_size * scale),
            &[run(text.len())],
            None,
        );
        width = f32::from(line.width);
    }
    (line, typography::centre_offset(cells, m.width, width))
}

/// Shapes a row into segments (SPEC §6.2 step 4, §7.2). `cursor` is the cursor column when it is
/// on this row; with it and `selection_edges`, a plain segment whose ligature would contain the
/// cursor or a selection boundary is reshaped without ligatures.
fn shape_row(
    window: &Window,
    row: &[Cell],
    t: &Theme,
    m: &CellMetrics,
    font: &TermFont,
    cursor: Option<usize>,
    selection_edges: &[usize],
) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut plain: Vec<(char, Style)> = Vec::new();
    let mut plain_start = 0;

    let flush = |plain: &mut Vec<(char, Style)>, start: usize, out: &mut Vec<Segment>| {
        if plain.is_empty() {
            return;
        }
        if plain.iter().any(|(ch, _)| *ch != ' ') {
            let (text, runs) = plain_runs(plain, font, font.ligatures);
            let mut line = shape_plain(window, &text, &runs, m);
            let end = start + plain.len();
            let touched = cursor.is_some_and(|c| (start..end).contains(&c))
                || selection_edges.iter().any(|&e| e > start && e < end);
            if font.ligatures && touched {
                let (ptext, pruns) = plain_runs(plain, font, false);
                let flat = shape_plain(window, &ptext, &pruns, m);
                let spans =
                    typography::ligature_spans(&glyph_keys(&line), &glyph_keys(&flat), plain.len());
                let abs: Vec<_> = spans
                    .iter()
                    .map(|r| r.start + start..r.end + start)
                    .collect();
                if typography::needs_reshape(&abs, cursor, selection_edges) {
                    line = flat;
                }
            }
            out.push(Segment {
                col: start,
                dx: 0.0,
                line,
            });
        }
        plain.clear();
    };

    let mut prev_wide = false;
    let mut skip = 0usize;
    for (c, cell) in row.iter().enumerate() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if cell.flags.has(CellFlags::WIDE_SPACER) && prev_wide {
            prev_wide = false;
            continue;
        }
        let blank = is_blank_cell(cell, prev_wide);
        prev_wide = cell.flags.has(CellFlags::WIDE);
        let ch = if blank { ' ' } else { cell.ch };
        let wide = cell.flags.has(CellFlags::WIDE);
        let has_z = !blank && cell.zerowidth.is_some();
        if typography::is_plain_cell(ch, has_z, wide) {
            if plain.is_empty() {
                plain_start = c;
            }
            plain.push((ch, cell_style(cell, t)));
        } else {
            flush(&mut plain, plain_start, &mut out);
            // A flag is two regional indicators: shape the pair across both. They are one cell
            // each normally, or two each (indicator + spacer) with `wide_regional_indicators`.
            let step = if wide { 2 } else { 1 };
            let flag = typography::is_regional_indicator(cell.ch)
                .then(|| row.get(c + step))
                .flatten()
                .filter(|n| {
                    typography::is_regional_indicator(n.ch) && n.flags.has(CellFlags::WIDE) == wide
                });
            let color = cell_colors(cell, t).0;
            let (line, dx) = match flag {
                Some(next) => {
                    skip = 2 * step - 1;
                    shape_glyphs(
                        window,
                        format!("{}{}", cell.ch, next.ch),
                        2 * step,
                        cell,
                        color,
                        m,
                        font,
                    )
                }
                None => shape_cell(window, cell, color, m, font),
            };
            out.push(Segment { col: c, dx, line });
        }
    }
    flush(&mut plain, plain_start, &mut out);
    out
}

fn quad(window: &mut Window, x: f32, y: f32, w: f32, h: f32, color: Hsla) {
    if w > 0.0 && h > 0.0 {
        window.paint_quad(fill(
            Bounds::new(point(px(x), px(y)), size(px(w), px(h))),
            color,
        ));
    }
}

fn paint_underline(
    window: &mut Window,
    style: Underline,
    x0: f32,
    x1: f32,
    row_y: f32,
    m: &CellMetrics,
    color: Hsla,
) {
    let t = (m.height / 15.0).max(1.0);
    let baseline = row_y + (m.height - (m.ascent + m.descent)) / 2.0 + m.ascent;
    let y = (baseline + (m.descent * 0.45).max(1.0)).round();
    match style {
        Underline::None => {}
        Underline::Single => quad(window, x0, y, x1 - x0, t, color),
        Underline::Double => {
            quad(window, x0, y - t, x1 - x0, t, color);
            quad(window, x0, y + t, x1 - x0, t, color);
        }
        Underline::Dotted => {
            let step = t * 2.0;
            let mut x = x0;
            while x < x1 {
                quad(window, x, y, t.min(x1 - x), t, color);
                x += step;
            }
        }
        Underline::Dashed => {
            let (dash, gap) = (t * 3.0, t * 2.0);
            let mut x = x0;
            while x < x1 {
                quad(window, x, y, dash.min(x1 - x), t, color);
                x += dash + gap;
            }
        }
        Underline::Curly => {
            let amp = (m.descent * 0.35).max(1.0);
            let period = (m.width * 0.5).max(3.0);
            let mut b = PathBuilder::stroke(px(t));
            let mut x = x0;
            b.move_to(point(px(x0), px(y)));
            while x < x1 {
                let nx = (x + period / 2.0).min(x1);
                let up = (((x - x0) / (period / 2.0)).round() as i64) % 2 == 0;
                let ny = if up { y - amp } else { y + amp };
                b.line_to(point(px(nx), px(ny)));
                x = nx;
            }
            if let Ok(p) = b.build() {
                window.paint_path(p, color);
            }
        }
    }
}

/// Paints the whole grid into `bounds` (the area inside the viewport padding).
pub fn paint_terminal(
    bounds: Bounds<Pixels>,
    p: &TerminalPaint,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let snap = &p.snapshot;
    let m = &p.metrics;
    let t = &p.theme;
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let key = theme_key(t, p.font_generation);
    let capacity = snap.rows * 4 + 64;

    for r in 0..snap.rows {
        let row = snap.row(r);
        let y = oy + r as f32 * m.height;

        // 1. Backgrounds, merged into runs of equal colour.
        let mut c = 0;
        while c < row.len() {
            if !has_visible_bg(&row[c]) {
                c += 1;
                continue;
            }
            let (_, bg) = cell_colors(&row[c], t);
            let start = c;
            while c < row.len() && has_visible_bg(&row[c]) && cell_colors(&row[c], t).1 == bg {
                c += 1;
            }
            quad(
                window,
                ox + start as f32 * m.width,
                y,
                (c - start) as f32 * m.width,
                m.height,
                bg,
            );
        }

        // 1b. Selection.
        let sel = snap
            .selection
            .and_then(|sp| selection_cols(&sp, r, snap.cols));
        if let Some((a, b)) = sel {
            quad(
                window,
                ox + a as f32 * m.width,
                y,
                (b - a) as f32 * m.width,
                m.height,
                snap.vt
                    .selection_bg
                    .map_or(t.term.selection_bg, |[r, g, b]| rgb_hsla(r, g, b)),
            );
        }
        // 1c. Trigger highlights: tinted under the glyphs (never recolouring them), or an
        // underline, plus the 3 px mark at the left edge of a marked line.
        for d in p.deco.rects.iter().filter(|d| d.row == r) {
            let x = ox + d.start as f32 * m.width;
            let w = (d.end - d.start) as f32 * m.width;
            if d.underline {
                quad(window, x, y + m.height - 2.0, w, 2.0, d.color);
            } else {
                quad(window, x, y, w, m.height, d.color);
            }
        }
        for (_, c) in p.deco.edges.iter().filter(|e| e.0 == r) {
            quad(window, ox, y, 3.0, m.height, *c);
        }
        // OSC 5 / 17 / 19 overrides recolour cells on a private copy of the row.
        let adjusted = adjust_row(row, &snap.vt, sel);
        let ovr_key = adjusted.as_ref().map_or(0, |_| vt_key(&snap.vt, sel));
        let row = adjusted.as_deref().unwrap_or(row);
        let edges: Vec<usize> = sel.map(|(a, b)| vec![a, b]).unwrap_or_default();

        // 2. Text.
        // The cursor cell must stay individually addressable, so its row is keyed (and shaped)
        // with the cursor column when ligatures are on.
        let cursor_col = (p.font.ligatures && snap.cursor.visible && snap.cursor.row == r)
            .then_some(snap.cursor.col);
        let row_key = snap.row_hashes[r]
            ^ ovr_key
            ^ cursor_col.map_or(0, |c| (c as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
            ^ if p.font.ligatures {
                edges.iter().fold(0u64, |h, &e| {
                    h.wrapping_mul(31)
                        .wrapping_add((e as u64 + 1).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
                })
            } else {
                0
            };
        let segments = p
            .cache
            .borrow_mut()
            .get_or_shape((row_key, key), capacity, || {
                Rc::new(shape_row(window, row, t, m, &p.font, cursor_col, &edges))
            });
        for seg in segments.iter() {
            let x = ox + seg.col as f32 * m.width + seg.dx;
            let _ = seg
                .line
                .paint(point(px(x), px(y)), px(m.height), window, cx);
        }

        // 3. Procedural glyphs and decorations.
        for (c, cell) in row.iter().enumerate() {
            if cell.flags.has(CellFlags::WIDE_SPACER) || cell.flags.has(CellFlags::HIDDEN) {
                continue;
            }
            if builtin_glyphs::is_builtin(cell.ch) {
                let (fg, _) = cell_colors(cell, t);
                let b = Bounds::new(
                    point(px(ox + c as f32 * m.width), px(y)),
                    size(px(m.width), px(m.height)),
                );
                builtin_glyphs::paint(cell.ch, b, fg, window);
            }
        }
        let mut c = 0;
        while c < row.len() {
            let cell = &row[c];
            if cell.underline == Underline::None || cell.flags.has(CellFlags::HIDDEN) {
                c += 1;
                continue;
            }
            let color = cell
                .underline_color
                .map(|u| resolve_color(u, t))
                .unwrap_or_else(|| cell_colors(cell, t).0);
            let style = cell.underline;
            let start = c;
            while c < row.len()
                && row[c].underline == style
                && row[c]
                    .underline_color
                    .map(|u| resolve_color(u, t))
                    .unwrap_or_else(|| cell_colors(&row[c], t).0)
                    == color
            {
                c += 1;
            }
            paint_underline(
                window,
                style,
                ox + start as f32 * m.width,
                ox + c as f32 * m.width,
                y,
                m,
                color,
            );
        }
        if let Some((hr, a, b)) = p.hover_link {
            if hr == r {
                paint_underline(
                    window,
                    Underline::Dotted,
                    ox + a as f32 * m.width,
                    ox + b as f32 * m.width,
                    y,
                    m,
                    t.accent,
                );
            }
        }
        let mut c = 0;
        while c < row.len() {
            if !row[c].flags.has(CellFlags::STRIKE) || row[c].flags.has(CellFlags::HIDDEN) {
                c += 1;
                continue;
            }
            let (color, _) = cell_colors(&row[c], t);
            let start = c;
            while c < row.len()
                && row[c].flags.has(CellFlags::STRIKE)
                && cell_colors(&row[c], t).0 == color
            {
                c += 1;
            }
            let baseline = y + (m.height - (m.ascent + m.descent)) / 2.0 + m.ascent;
            quad(
                window,
                ox + start as f32 * m.width,
                (baseline - m.ascent * 0.33).round(),
                (c - start) as f32 * m.width,
                (m.height / 15.0).max(1.0),
                color,
            );
        }
    }

    paint_cursor(window, cx, p, ox, oy);
}

fn paint_cursor(window: &mut Window, cx: &mut gpui::App, p: &TerminalPaint, ox: f32, oy: f32) {
    let snap = &p.snapshot;
    let cur = snap.cursor;
    let m = &p.metrics;
    let t = &p.theme;
    if !cur.visible || cur.row >= snap.rows || cur.col >= snap.cols {
        return;
    }
    // Only a focused cursor blinks; an unfocused one is a steady hollow block.
    if p.focused && cur.blinking && !p.cursor_on {
        return;
    }
    let cell = &snap.row(cur.row)[cur.col];
    let cells = if cell.flags.has(CellFlags::WIDE) {
        2.0
    } else {
        1.0
    };
    let (x, y, w, h) = (
        ox + cur.col as f32 * m.width,
        oy + cur.row as f32 * m.height,
        m.width * cells,
        m.height,
    );
    let color = t.term.cursor;
    let thin = (m.height / 12.0).max(1.0);
    if !p.focused {
        quad(window, x, y, w, thin, color);
        quad(window, x, y + h - thin, w, thin, color);
        quad(window, x, y, thin, h, color);
        quad(window, x + w - thin, y, thin, h, color);
        return;
    }
    match cur.shape {
        CursorShape::Block => {
            quad(window, x, y, w, h, color);
            // Redraw the glyph under the cursor in the background colour.
            if !builtin_glyphs::is_builtin(cell.ch)
                && cell.ch != ' '
                && cell.ch != '\0'
                && !cell.flags.has(CellFlags::HIDDEN)
            {
                let (line, dx) = shape_cell(window, cell, t.term.cursor_text, m, &p.font);
                let _ = line.paint(point(px(x + dx), px(y)), px(m.height), window, cx);
            }
        }
        CursorShape::Beam => quad(window, x, y, (m.width / 8.0).max(1.5), h, color),
        CursorShape::Underline => quad(window, x, y + h - thin * 1.5, w, thin * 1.5, color),
    }
}

#[cfg(test)]
mod tests {
    use mt_term::SelectionSpan;

    #[test]
    fn selection_columns_per_row() {
        let sp = SelectionSpan {
            start: (1, 4),
            end: (3, 2),
            block: false,
        };
        assert_eq!(super::selection_cols(&sp, 0, 10), None);
        assert_eq!(super::selection_cols(&sp, 1, 10), Some((4, 10)));
        assert_eq!(super::selection_cols(&sp, 2, 10), Some((0, 10)));
        assert_eq!(super::selection_cols(&sp, 3, 10), Some((0, 3)));
        assert_eq!(super::selection_cols(&sp, 4, 10), None);
        let b = SelectionSpan {
            start: (0, 5),
            end: (2, 2),
            block: true,
        };
        assert_eq!(super::selection_cols(&b, 1, 10), Some((2, 6)));
    }

    use super::*;

    #[test]
    fn grid_fits_whole_cells() {
        let m = CellMetrics {
            width: 8.0,
            height: 16.0,
            ..Default::default()
        };
        assert_eq!(m.grid_for(80.0, 48.0), (10, 3));
        assert_eq!(m.grid_for(79.9, 47.9), (9, 2));
        assert_eq!(m.grid_for(1.0, 1.0), (1, 1));
    }

    #[test]
    fn indexed_colours_follow_the_xterm_cube() {
        let t = Theme::one_dark();
        assert_eq!(resolve_color(CellColor::Indexed(1), &t), t.term.ansi[1]);
        assert_eq!(resolve_color(CellColor::Indexed(16), &t), rgb_hsla(0, 0, 0));
        assert_eq!(
            resolve_color(CellColor::Indexed(231), &t),
            rgb_hsla(255, 255, 255)
        );
        assert_eq!(
            resolve_color(CellColor::Indexed(232), &t),
            rgb_hsla(8, 8, 8)
        );
        assert_eq!(
            resolve_color(CellColor::Rgb(1, 2, 3), &t),
            rgb_hsla(1, 2, 3)
        );
    }

    #[test]
    fn program_colour_overrides_recolour_bold_underline_and_selection() {
        let vt = mt_term::VtView {
            bold_color: Some([1, 2, 3]),
            underline_color: Some([4, 5, 6]),
            selection_fg: Some([7, 8, 9]),
            ..Default::default()
        };
        let mut row = vec![Cell::default(); 4];
        row[0].flags = CellFlags(CellFlags::BOLD);
        row[1].underline = Underline::Single;
        // No override, no copy.
        assert!(adjust_row(&row, &Default::default(), Some((0, 2))).is_none());
        let out = adjust_row(&row, &vt, Some((2, 4))).unwrap();
        assert_eq!(out[0].fg, CellColor::Rgb(1, 2, 3), "bold");
        assert_eq!(out[1].underline_color, Some(CellColor::Rgb(4, 5, 6)));
        assert_eq!(out[1].fg, CellColor::DefaultFg);
        assert_eq!(out[2].fg, CellColor::Rgb(7, 8, 9), "selected");
        assert_eq!(out[3].fg, CellColor::Rgb(7, 8, 9));
        // An explicit underline colour is kept.
        row[1].underline_color = Some(CellColor::Rgb(9, 9, 9));
        let out = adjust_row(&row, &vt, None).unwrap();
        assert_eq!(out[1].underline_color, Some(CellColor::Rgb(9, 9, 9)));
        // Overrides and selection change the cache key.
        assert_ne!(vt_key(&vt, None), vt_key(&vt, Some((0, 1))));
    }

    #[test]
    fn inverse_dim_and_hidden_adjust_colours() {
        let t = Theme::one_dark();
        let mut cell = Cell::default();
        let (fg, bg) = cell_colors(&cell, &t);
        assert_eq!((fg, bg), (t.term.fg, t.background));
        cell.flags = CellFlags(CellFlags::INVERSE);
        assert_eq!(cell_colors(&cell, &t), (t.background, t.term.fg));
        cell.flags = CellFlags(CellFlags::DIM);
        assert!(cell_colors(&cell, &t).0.a < 1.0);
        cell.flags = CellFlags(CellFlags::HIDDEN);
        let (fg, bg) = cell_colors(&cell, &t);
        assert_eq!(fg, bg);
    }
}
