//! GPU painting of a live terminal grid (SPEC §6.2): merged background quads, shaped text per
//! row (cached by row hash), decorations drawn as quads/paths, procedural glyphs and the cursor.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    fill, font, point, px, size, Bounds, FontStyle, FontWeight, Hsla, PathBuilder, Pixels,
    ShapedLine, SharedString, TextRun, Window,
};
use mt_term::{Cell, CellColor, CellFlags, CursorShape, RenderSnapshot, Underline};

use crate::{builtin_glyphs, metrics::*, theme::Theme};

/// Measured size of one terminal cell, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub descent: f32,
    pub font_size: f32,
}

/// The terminal font from the config: family and the regular/bold weights.
#[derive(Clone, Debug, PartialEq)]
pub struct TermFont {
    pub family: SharedString,
    pub weight: f32,
    pub bold_weight: f32,
}

impl Default for TermFont {
    fn default() -> Self {
        Self {
            family: TERM_FONT.into(),
            weight: 400.0,
            bold_weight: 700.0,
        }
    }
}

impl TermFont {
    pub fn from_config(f: &mt_config::Font) -> Self {
        Self {
            family: f.family.clone().into(),
            weight: f.weight.numeric().map_or(400.0, f32::from),
            bold_weight: f.bold_weight.numeric().map_or(700.0, f32::from),
        }
    }

    /// The GPUI font for a style run.
    pub fn font(&self, bold: bool, italic: bool) -> gpui::Font {
        let mut f = font(self.family.clone());
        f.weight = FontWeight(if bold { self.bold_weight } else { self.weight });
        if italic {
            f.style = FontStyle::Italic;
        }
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

/// Shaped rows keyed by `(row hash, theme/font generation)`.
#[derive(Default)]
pub struct LineCache {
    map: HashMap<(u64, u64), ShapedLine>,
}

impl LineCache {
    fn get_or_shape(
        &mut self,
        key: (u64, u64),
        capacity: usize,
        shape: impl FnOnce() -> ShapedLine,
    ) -> ShapedLine {
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

fn theme_key(t: &Theme, font_generation: u64) -> u64 {
    // Names are unique per theme; the dark flag guards against same-name variants.
    let mut h = font_generation ^ ((t.is_dark as u64) << 63);
    for b in t.name.bytes() {
        h = h.wrapping_mul(0x100000001b3) ^ b as u64;
    }
    h
}

fn shape_row(
    window: &Window,
    row: &[Cell],
    t: &Theme,
    m: &CellMetrics,
    font: &TermFont,
    cols: usize,
) -> ShapedLine {
    let mut text = String::with_capacity(cols);
    let mut runs: Vec<TextRun> = Vec::new();
    let mut prev: Option<(Hsla, bool, bool)> = None;
    for cell in row {
        let (fg, _) = cell_colors(cell, t);
        let blank = cell.flags.has(CellFlags::WIDE_SPACER)
            || builtin_glyphs::is_builtin(cell.ch)
            || cell.ch == '\0'
            || cell.flags.has(CellFlags::HIDDEN);
        let ch = if blank { ' ' } else { cell.ch };
        let start = text.len();
        text.push(ch);
        if !blank {
            if let Some(z) = &cell.zerowidth {
                text.extend(z.iter());
            }
        }
        let len = text.len() - start;
        let style = (
            fg,
            cell.flags.has(CellFlags::BOLD),
            cell.flags.has(CellFlags::ITALIC),
        );
        if prev == Some(style) {
            runs.last_mut().expect("run exists").len += len;
            continue;
        }
        prev = Some(style);
        runs.push(TextRun {
            len,
            font: font.font(style.1, style.2),
            color: fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    window.text_system().shape_line(
        SharedString::from(text),
        px(m.font_size),
        &runs,
        Some(px(m.width)),
    )
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

        // 2. Text.
        let line = p
            .cache
            .borrow_mut()
            .get_or_shape((snap.row_hashes[r], key), capacity, || {
                shape_row(window, row, t, m, &p.font, snap.cols)
            });
        let _ = line.paint(point(px(ox), px(y)), px(m.height), window, cx);

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
                let mut text = String::from(cell.ch);
                if let Some(z) = &cell.zerowidth {
                    text.extend(z.iter());
                }
                let f = p.font.font(
                    cell.flags.has(CellFlags::BOLD),
                    cell.flags.has(CellFlags::ITALIC),
                );
                let len = text.len();
                let line = window.text_system().shape_line(
                    SharedString::from(text),
                    px(m.font_size),
                    &[TextRun {
                        len,
                        font: f,
                        color: t.term.cursor_text,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    Some(px(m.width)),
                );
                let _ = line.paint(point(px(x), px(y)), px(m.height), window, cx);
            }
        }
        CursorShape::Beam => quad(window, x, y, (m.width / 8.0).max(1.5), h, color),
        CursorShape::Underline => quad(window, x, y + h - thin * 1.5, w, thin * 1.5, color),
    }
}

#[cfg(test)]
mod tests {
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
