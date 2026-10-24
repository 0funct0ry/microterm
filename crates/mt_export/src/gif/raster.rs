//! Software rasteriser: draws a cell snapshot into an RGB canvas with a theme and the bundled
//! fonts. Box-drawing and block elements are drawn geometrically so they join across cells.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use mt_term::{Cell, CellColor, CellFlags, CursorShape, RenderSnapshot, Underline};
use mt_theme::{Rgba, Theme};

use super::emoji::{self, Emoji};
use super::font::{self, Fonts};

pub type Rgb = [u8; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chrome {
    None,
    Plain,
    Macos,
}

impl Chrome {
    pub fn parse(s: &str) -> Self {
        match s {
            "none" => Chrome::None,
            "macos" => Chrome::Macos,
            _ => Chrome::Plain,
        }
    }
}

pub struct Layout {
    pub width: usize,
    pub height: usize,
    /// Grid origin.
    pub ox: usize,
    pub oy: usize,
    /// The window rectangle inside the margin.
    wx: usize,
    wy: usize,
    ww: usize,
    wh: usize,
    bar: usize,
    radius: f32,
    scale: f32,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Row-major RGB.
    pub rgb: Vec<u8>,
}

/// sRGB byte to linear light, 12-bit.
fn to_linear() -> &'static [u16; 256] {
    static LUT: OnceLock<[u16; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            let l = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
            (l * 4095.0).round() as u16
        })
    })
}

/// 12-bit linear light back to an sRGB byte.
fn to_srgb() -> &'static [u8; 4096] {
    static LUT: OnceLock<[u8; 4096]> = OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let l = i as f32 / 4095.0;
            let c = if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            (c * 255.0).round().clamp(0.0, 255.0) as u8
        })
    })
}

/// Blends `b` over `a` with coverage `t` in linear light, so antialiased text keeps its weight.
fn mix_linear(a: u8, b: u8, t: u8) -> u8 {
    let (lin, srgb) = (to_linear(), to_srgb());
    let (la, lb, t) = (
        u32::from(lin[usize::from(a)]),
        u32::from(lin[usize::from(b)]),
        u32::from(t),
    );
    srgb[((la * (255 - t) + lb * t + 127) / 255) as usize]
}

impl Canvas {
    fn fill(&mut self, x: i64, y: i64, w: i64, h: i64, c: Rgb) {
        self.blend(x, y, w, h, c, 255);
    }

    fn blend(&mut self, x: i64, y: i64, w: i64, h: i64, c: Rgb, a: u8) {
        let x0 = x.clamp(0, self.width as i64) as usize;
        let y0 = y.clamp(0, self.height as i64) as usize;
        let x1 = (x + w).clamp(0, self.width as i64) as usize;
        let y1 = (y + h).clamp(0, self.height as i64) as usize;
        for py in y0..y1 {
            for px in x0..x1 {
                self.put(px, py, c, a);
            }
        }
    }

    fn put(&mut self, x: usize, y: usize, c: Rgb, a: u8) {
        let i = (y * self.width + x) * 3;
        if a == 255 {
            self.rgb[i..i + 3].copy_from_slice(&c);
        } else if a > 0 {
            for (dst, src) in self.rgb[i..i + 3].iter_mut().zip(c) {
                *dst = mix_linear(*dst, src, a);
            }
        }
    }

    /// An antialiased filled circle.
    fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgb) {
        let (x0, x1) = ((cx - r).floor() as i64 - 1, (cx + r).ceil() as i64 + 1);
        let (y0, y1) = ((cy - r).floor() as i64 - 1, (cy + r).ceil() as i64 + 1);
        for y in y0.max(0)..y1.min(self.height as i64) {
            for x in x0.max(0)..x1.min(self.width as i64) {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let cov = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                self.put(x as usize, y as usize, c, (cov * 255.0).round() as u8);
            }
        }
    }
}

fn mix(a: u8, b: u8, t: u8) -> u8 {
    ((u16::from(a) * (255 - u16::from(t)) + u16::from(b) * u16::from(t) + 127) / 255) as u8
}

fn mix_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = (t.clamp(0.0, 1.0) * 255.0).round() as u8;
    [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
}

fn rgb(c: Rgba) -> Rgb {
    [c.r, c.g, c.b]
}

/// Signed distance from pixel centre `(px, py)` to a rounded rectangle (negative inside).
fn rounded_dist(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let qx = (px - (x + hw)).abs() - (hw - r);
    let qy = (py - (y + hh)).abs() - (hh - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Pixel rectangle, `x1`/`y1` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rect {
    pub fn union(self, o: Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

/// What the last render drew, to find the rows that changed.
struct Prev {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    cursor: Option<(usize, usize, CursorShape)>,
}

pub struct Renderer {
    pub layout: Layout,
    fonts: Fonts,
    emoji: Emoji,
    fg: Rgb,
    bg: Rgb,
    cursor: Rgb,
    cursor_text: Rgb,
    ansi: [Rgb; 16],
    chrome: Chrome,
    title: String,
    draw_cursor: bool,
    /// The visible grid: the top-left `vis_cols` x `vis_rows` cells of the terminal.
    vis_cols: usize,
    vis_rows: usize,
    /// Backdrop, shadow and window chrome: drawn once; rows are restored from it.
    base: Canvas,
    /// The current screen, updated in place one changed row at a time.
    canvas: Canvas,
    prev: Option<Prev>,
    /// Fallback cells per grid row of the current screen.
    row_fallback: Vec<u64>,
    /// Pixels outside the rounded window corners when there is no margin (transparent in the
    /// GIF, since GIF transparency cannot be antialiased).
    pub mask: Vec<bool>,
    /// Characters no bundled face has.
    pub missing: BTreeSet<char>,
    /// Most fallback cells seen in one frame.
    pub fallback_cells: u64,
}

pub struct Params<'a> {
    pub theme: &'a Theme,
    pub font_size: f32,
    pub line_height: f32,
    pub scale: f32,
    pub padding: f32,
    /// Space around the window, filled with `margin_fill` (`None` = derived from the theme).
    pub margin: f32,
    pub margin_fill: Option<Rgb>,
    pub shadow: bool,
    pub chrome: Chrome,
    pub title: String,
    pub cursor: bool,
    /// The largest grid the recording reaches.
    pub cols: usize,
    pub rows: usize,
}

impl Renderer {
    pub fn new(p: Params) -> Self {
        let s = p.scale;
        let fonts = Fonts::new((p.font_size * s).max(4.0), p.line_height);
        let pad = (p.padding * s).round() as usize;
        let margin = (p.margin * s).round() as usize;
        let bar = if p.chrome == Chrome::None {
            0
        } else {
            (28.0 * s).round().max(fonts.cell_h as f32 * 1.4) as usize
        };
        let ww = 2 * pad + p.cols * fonts.cell_w;
        let wh = bar + 2 * pad + p.rows * fonts.cell_h;
        let radius = if p.chrome == Chrome::None && margin == 0 {
            0.0
        } else {
            (10.0 * s).round()
        };
        let t = p.theme;
        let bg = rgb(t.background);
        let fill = p.margin_fill.unwrap_or_else(|| {
            // A backdrop a little darker than the terminal so the window reads as a window.
            mix_rgb(bg, [0, 0, 0], if t.is_dark() { 0.45 } else { 0.12 })
        });
        let layout = Layout {
            width: ww + 2 * margin,
            height: wh + 2 * margin,
            ox: margin + pad,
            oy: margin + bar + pad,
            wx: margin,
            wy: margin,
            ww,
            wh,
            bar,
            radius,
            scale: s,
        };
        let mut me = Self {
            layout,
            fonts,
            emoji: Emoji::default(),
            fg: rgb(t.foreground),
            bg,
            cursor: rgb(t.cursor),
            cursor_text: rgb(t.cursor_text),
            ansi: t.ansi.map(rgb),
            chrome: p.chrome,
            title: p.title,
            draw_cursor: p.cursor,
            vis_cols: p.cols,
            vis_rows: p.rows,
            base: Canvas::default(),
            canvas: Canvas::default(),
            prev: None,
            row_fallback: Vec::new(),
            mask: Vec::new(),
            missing: BTreeSet::new(),
            fallback_cells: 0,
        };
        me.mask = if margin == 0 {
            me.corner_mask()
        } else {
            vec![false; me.layout.width * me.layout.height]
        };
        me.base = me.draw_base(fill, p.shadow && margin > 0);
        me.canvas = me.base.clone();
        me.row_fallback = vec![0; p.rows];
        me
    }

    fn corner_mask(&self) -> Vec<bool> {
        let (w, h) = (self.layout.width, self.layout.height);
        let r = self.layout.radius;
        let (wf, hf) = (w as f32, h as f32);
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                r > 0.0 && rounded_dist(x, y, 0.0, 0.0, wf, hf, r) > 0.0
            })
            .collect()
    }

    /// The frame every screen is drawn onto: backdrop, soft shadow, rounded window, title bar.
    fn draw_base(&mut self, fill: Rgb, shadow: bool) -> Canvas {
        let l = &self.layout;
        let (w, h) = (l.width, l.height);
        let (wx, wy, ww, wh) = (l.wx as f32, l.wy as f32, l.ww as f32, l.wh as f32);
        let (r, s, bar) = (l.radius, l.scale, l.bar as f32);
        let mut c = Canvas {
            width: w,
            height: h,
            rgb: fill.repeat(w * h),
        };
        let border = mix_rgb(self.bg, self.fg, 0.16);
        let bar_bg = mix_rgb(self.bg, self.fg, 0.06);
        let (blur, drop) = (18.0 * s, 6.0 * s);
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                if shadow {
                    let d = rounded_dist(px, py - drop, wx, wy, ww, wh, r);
                    let a = 0.5 * (1.0 - (d / blur).clamp(0.0, 1.0)).powi(2);
                    c.put(x, y, [0, 0, 0], (a * 255.0).round() as u8);
                }
                let d = rounded_dist(px, py, wx, wy, ww, wh, r);
                let cov = coverage(d);
                if cov == 0.0 {
                    continue;
                }
                let inside = if py < wy + bar { bar_bg } else { self.bg };
                c.put(x, y, inside, (cov * 255.0).round() as u8);
                if self.chrome != Chrome::None {
                    // A one-pixel antialiased border inside the window edge.
                    let ring = (cov - coverage(d + 1.0)).max(0.0);
                    c.put(x, y, border, (ring * 255.0).round() as u8);
                }
            }
        }
        if self.chrome == Chrome::None {
            return c;
        }
        let (x0, y0) = (l.wx as i64, l.wy as i64);
        let (wwi, bari) = (l.ww as i64, l.bar as i64);
        c.fill(x0 + 1, y0 + bari - 1, wwi - 2, 1, border);
        let mut left = 0i64;
        if self.chrome == Chrome::Macos {
            let (rad, gap) = (6.0 * s, 8.0 * s);
            let cy = wy + bar / 2.0;
            for (i, col) in [[0xff, 0x5f, 0x57], [0xfe, 0xbc, 0x2e], [0x28, 0xc8, 0x40]]
                .into_iter()
                .enumerate()
            {
                let cx = wx + 12.0 * s + rad + i as f32 * (2.0 * rad + gap);
                c.circle(cx, cy, rad, col);
                left = (cx + rad - wx) as i64;
            }
        }
        let cw = self.fonts.cell_w as i64;
        let room = ((wwi - 2 * left.max(r as i64)) / cw).max(0) as usize;
        let mut title: Vec<char> = self.title.chars().collect();
        if title.len() > room {
            title.truncate(room.saturating_sub(1));
            title.push('…');
        }
        let tx = x0 + (wwi - title.len() as i64 * cw) / 2;
        let ty = y0 + (bari - self.fonts.cell_h as i64) / 2;
        let color = mix_rgb(self.fg, self.bg, 0.3);
        for (i, ch) in title.into_iter().enumerate() {
            self.glyph(&mut c, tx + i as i64 * cw, ty, ch, 0, color);
        }
        c
    }

    /// The current screen.
    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    /// Draws `snap` over the previous screen, redrawing only the rows whose cells (or cursor)
    /// changed. Returns the rows' pixel band, or `None` when nothing changed. The result is
    /// identical to drawing the whole screen from scratch.
    pub fn render(&mut self, snap: &RenderSnapshot) -> Option<Rect> {
        let cur = snap.cursor;
        let show_cursor = self.draw_cursor && cur.visible && snap.display_offset == 0;
        let cursor = show_cursor.then_some((cur.row, cur.col, cur.shape));
        let (cols, rows) = (snap.cols.min(self.vis_cols), snap.rows.min(self.vis_rows));
        let ch = self.fonts.cell_h;
        let (w, oy) = (self.layout.width, self.layout.oy);
        let mut c = std::mem::take(&mut self.canvas);
        let first = self.prev.is_none();

        let mut dirty = vec![true; rows];
        let mut restore: Option<(usize, usize)> = None;
        match &self.prev {
            Some(p) if p.cols == cols && p.rows == rows => {
                for (r, d) in dirty.iter_mut().enumerate() {
                    *d = snap.cells[r * snap.cols..r * snap.cols + cols]
                        != p.cells[r * cols..(r + 1) * cols];
                }
                if p.cursor != cursor {
                    for (r, _, _) in p.cursor.iter().chain(cursor.iter()) {
                        if let Some(d) = dirty.get_mut(*r) {
                            *d = true;
                        }
                    }
                }
            }
            // First screen or a resize: redraw the whole grid area, including rows the new
            // grid no longer has.
            _ => restore = Some((oy, oy + self.row_fallback.len().max(rows) * ch)),
        }
        if let Some((y0, y1)) = restore {
            let (y0, y1) = (y0.min(self.base.height), y1.min(self.base.height));
            c.rgb[y0 * w * 3..y1 * w * 3].copy_from_slice(&self.base.rgb[y0 * w * 3..y1 * w * 3]);
        }
        self.row_fallback
            .resize(self.row_fallback.len().max(rows), 0);
        let mut band: Option<(usize, usize)> = restore;
        for r in (0..rows).filter(|&r| dirty[r]) {
            let (y0, y1) = (oy + r * ch, (oy + (r + 1) * ch).min(c.height));
            if restore.is_none() {
                c.rgb[y0 * w * 3..y1 * w * 3]
                    .copy_from_slice(&self.base.rgb[y0 * w * 3..y1 * w * 3]);
            }
            self.row_fallback[r] = self.draw_row(&mut c, snap, r, cols, cursor);
            band = Some(band.map_or((y0, y1), |(a, b)| (a.min(y0), b.max(y1))));
        }
        for f in self.row_fallback.iter_mut().skip(rows) {
            *f = 0;
        }
        self.fallback_cells = self
            .fallback_cells
            .max(self.row_fallback.iter().sum::<u64>());
        self.canvas = c;
        self.prev = Some(Prev {
            cols,
            rows,
            cells: (0..rows)
                .flat_map(|r| {
                    snap.cells[r * snap.cols..r * snap.cols + cols]
                        .iter()
                        .cloned()
                })
                .collect(),
            cursor,
        });
        if first {
            // The first screen is the whole canvas: backdrop, chrome and grid.
            return Some(Rect {
                x0: 0,
                y0: 0,
                x1: w,
                y1: self.layout.height,
            });
        }
        band.map(|(y0, y1)| Rect {
            x0: 0,
            y0,
            x1: w,
            y1,
        })
    }

    fn line_px(&self) -> i64 {
        (self.fonts.cell_w as i64 / 8).max(1)
    }

    fn color(&self, cc: CellColor) -> Rgb {
        match cc {
            CellColor::DefaultFg => self.fg,
            CellColor::DefaultBg => self.bg,
            CellColor::Rgb(r, g, b) => [r, g, b],
            CellColor::Indexed(i @ 0..=15) => self.ansi[usize::from(i)],
            CellColor::Indexed(i @ 16..=231) => {
                let n = usize::from(i - 16);
                let lvl = |v: usize| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
                [lvl(n / 36), lvl((n / 6) % 6), lvl(n % 6)]
            }
            CellColor::Indexed(i) => {
                let v = (8 + 10 * (usize::from(i) - 232)) as u8;
                [v, v, v]
            }
        }
    }

    fn cell_colors(&self, cell: &Cell) -> (Rgb, Rgb) {
        let (mut fg, mut bg) = (self.color(cell.fg), self.color(cell.bg));
        if cell.flags.has(CellFlags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.has(CellFlags::DIM) {
            fg = mix_rgb(fg, bg, 0.5);
        }
        if cell.flags.has(CellFlags::HIDDEN) {
            fg = bg;
        }
        (fg, bg)
    }

    /// Draws grid row `row` onto its (already restored) band; returns its fallback cell count.
    fn draw_row(
        &mut self,
        c: &mut Canvas,
        snap: &RenderSnapshot,
        row: usize,
        cols: usize,
        cursor: Option<(usize, usize, CursorShape)>,
    ) -> u64 {
        let (cw, ch) = (self.fonts.cell_w as i64, self.fonts.cell_h as i64);
        let (ox, oy) = (self.layout.ox as i64, self.layout.oy as i64);
        let block_at = cursor
            .filter(|(_, _, shape)| *shape == CursorShape::Block)
            .map(|(r, col, _)| (r, col));
        let mut fallback = 0u64;
        let cells = &snap.cells[row * snap.cols..row * snap.cols + cols];
        let y = oy + row as i64 * ch;
        // Backgrounds first so glyph overhang (italics) is not painted over. Default-coloured
        // cells are already the window colour.
        for (col, cell) in cells.iter().enumerate() {
            if cell.flags.has(CellFlags::WIDE_SPACER) {
                continue;
            }
            let span = if cell.flags.has(CellFlags::WIDE) {
                2
            } else {
                1
            };
            let (_, mut bg) = self.cell_colors(cell);
            if block_at == Some((row, col)) {
                bg = self.cursor;
            }
            if bg != self.bg {
                c.fill(ox + col as i64 * cw, y, span * cw, ch, bg);
            }
        }
        for (col, cell) in cells.iter().enumerate() {
            if cell.flags.has(CellFlags::WIDE_SPACER) {
                continue;
            }
            let span = if cell.flags.has(CellFlags::WIDE) {
                2
            } else {
                1
            };
            let x = ox + col as i64 * cw;
            let (mut fg, _) = self.cell_colors(cell);
            if block_at == Some((row, col)) {
                fg = self.cursor_text;
            }
            let style = font::style(
                cell.flags.has(CellFlags::BOLD),
                cell.flags.has(CellFlags::ITALIC),
            );
            if cell.ch != ' ' && cell.ch != '\0' && !self.geometric(c, x, y, cell.ch, fg) {
                // Pictographs are colour emoji; other characters use the text faces first and
                // the emoji font only when no text face has them (✅ ⚡ ⭐).
                let pictograph = ('\u{1F000}'..='\u{1FAFF}').contains(&cell.ch);
                let hidden = cell.flags.has(CellFlags::HIDDEN);
                let dim = cell.flags.has(CellFlags::DIM);
                let drawn = if pictograph {
                    self.emoji(c, x, y, span, cell.ch, hidden, dim)
                } else {
                    self.glyph(c, x, y, cell.ch, style, fg)
                        || self.emoji(c, x, y, span, cell.ch, hidden, dim)
                };
                if !drawn {
                    self.missing.insert(cell.ch);
                    fallback += 1;
                    let t = self.line_px();
                    let (bw, bh) = (span * cw - 4 * t, ch / 2);
                    let (bx, by) = (x + 2 * t, y + (ch - bh) / 2);
                    c.fill(bx, by, bw, t, fg);
                    c.fill(bx, by + bh - t, bw, t, fg);
                    c.fill(bx, by, t, bh, fg);
                    c.fill(bx + bw - t, by, t, bh, fg);
                }
            }
            let t = self.line_px();
            let base = y + i64::from(self.fonts.ascent);
            let uy = (base + t).min(y + ch - t);
            match cell.underline {
                Underline::None => {}
                Underline::Double => {
                    c.fill(x, uy - t, span * cw, t, fg);
                    c.fill(x, (uy + t).min(y + ch - t), span * cw, t, fg);
                }
                _ => c.fill(x, uy, span * cw, t, fg),
            }
            if cell.flags.has(CellFlags::STRIKE) {
                c.fill(x, y + ch / 2, span * cw, t, fg);
            }
        }
        if let Some((r, col, shape)) = cursor {
            if r == row && col < cols {
                let x = ox + col as i64 * cw;
                let t = (self.line_px() * 2).max(2);
                match shape {
                    CursorShape::Block => {}
                    CursorShape::Beam => c.fill(x, y, t, ch, self.cursor),
                    CursorShape::Underline => c.fill(x, y + ch - t, cw, t, self.cursor),
                }
            }
        }
        fallback
    }

    /// Draws the colour emoji for `ch` centred in its cell span; false when the emoji font has
    /// none. `hidden` cells draw nothing but still count as drawn.
    #[allow(clippy::too_many_arguments)]
    fn emoji(
        &mut self,
        c: &mut Canvas,
        x: i64,
        y: i64,
        span: i64,
        ch: char,
        hidden: bool,
        dim: bool,
    ) -> bool {
        if !emoji::has(ch) {
            return false;
        }
        if hidden {
            return true;
        }
        let (bw, bh) = (span * self.fonts.cell_w as i64, self.fonts.cell_h as i64);
        let Some(e) = self.emoji.get(ch, bw as usize, bh as usize) else {
            return false;
        };
        let (ox, oy) = (x + (bw - e.w as i64) / 2, y + (bh - e.h as i64) / 2);
        for py in 0..e.h {
            let (dy, dx0) = (oy + py as i64, ox);
            if dy < 0 || dy >= c.height as i64 {
                continue;
            }
            for px in 0..e.w {
                let o = (py * e.w + px) * 4;
                let a = if dim {
                    e.rgba[o + 3] / 2
                } else {
                    e.rgba[o + 3]
                };
                let dx = dx0 + px as i64;
                if a > 0 && dx >= 0 && dx < c.width as i64 {
                    c.put(
                        dx as usize,
                        dy as usize,
                        [e.rgba[o], e.rgba[o + 1], e.rgba[o + 2]],
                        a,
                    );
                }
            }
        }
        true
    }

    /// Draws `ch` from the bundled faces; false when none has it.
    fn glyph(&mut self, c: &mut Canvas, x: i64, y: i64, ch: char, style: u8, fg: Rgb) -> bool {
        let base = y + i64::from(self.fonts.ascent);
        let cell_h = self.fonts.cell_h as i64;
        let Some(g) = self.fonts.glyph(ch, style) else {
            return false;
        };
        // Glyphs never leave their row, so a row can be redrawn on its own.
        let (top, bottom) = (y.max(0), (y + cell_h).min(c.height as i64));
        for gy in 0..g.h {
            let py = base + i64::from(g.y) + gy as i64;
            if py < top || py >= bottom {
                continue;
            }
            for gx in 0..g.w {
                let a = g.coverage[gy * g.w + gx];
                let px = x + i64::from(g.x) + gx as i64;
                if a > 0 && px >= 0 && px < c.width as i64 {
                    c.put(px as usize, py as usize, fg, a);
                }
            }
        }
        true
    }

    /// Box drawing (U+2500-257F) and block elements (U+2580-259F); false for other chars.
    fn geometric(&self, c: &mut Canvas, x: i64, y: i64, ch: char, fg: Rgb) -> bool {
        let (w, h) = (self.fonts.cell_w as i64, self.fonts.cell_h as i64);
        let u = ch as u32;
        if (0x2580..=0x259f).contains(&u) {
            let (hw, hh) = (w / 2, h / 2);
            let e = |n: i64, of: i64| of * n / 8;
            match u {
                0x2580 => c.fill(x, y, w, hh, fg),
                0x2581..=0x2588 => {
                    let n = e(i64::from(u - 0x2580), h);
                    c.fill(x, y + h - n, w, n, fg);
                }
                0x2589..=0x258f => c.fill(x, y, e(i64::from(0x2590 - u), w), h, fg),
                0x2590 => c.fill(x + hw, y, w - hw, h, fg),
                0x2591..=0x2593 => c.blend(x, y, w, h, fg, (64 * (u - 0x2590)) as u8),
                0x2594 => c.fill(x, y, w, e(1, h), fg),
                0x2595 => c.fill(x + w - e(1, w), y, e(1, w), h, fg),
                _ => {
                    // Quadrants: upper-left, upper-right, lower-left, lower-right.
                    let q: u8 = match u {
                        0x2596 => 0b0010,
                        0x2597 => 0b0001,
                        0x2598 => 0b1000,
                        0x2599 => 0b1011,
                        0x259a => 0b1001,
                        0x259b => 0b1110,
                        0x259c => 0b1101,
                        0x259d => 0b0100,
                        0x259e => 0b0110,
                        _ => 0b0111,
                    };
                    let quads = [(0, 0), (hw, 0), (0, hh), (hw, hh)];
                    for (i, (qx, qy)) in quads.into_iter().enumerate() {
                        if q & (0b1000 >> i) != 0 {
                            let qw = if qx == 0 { hw } else { w - hw };
                            let qh = if qy == 0 { hh } else { h - hh };
                            c.fill(x + qx, y + qy, qw, qh, fg);
                        }
                    }
                }
            }
            return true;
        }
        let Some(arms) = box_arms(u) else {
            return false;
        };
        let t = self.line_px();
        let (cx, cy) = (x + (w - t) / 2, y + (h - t) / 2);
        // [left, right, up, down]: 1 light, 2 heavy, 3 double.
        for (i, weight) in arms.into_iter().enumerate() {
            let offsets: &[(i64, i64)] = match weight {
                0 => continue,
                1 => &[(0, t)],
                2 => &[(-t / 2 - 1, 2 * t)],
                _ => &[(-t, t), (t, t)],
            };
            // Arms reach a little past the centre so perpendicular lines of any weight join.
            for &(off, th) in offsets {
                match i {
                    0 => c.fill(x, cy + off, cx - x + 2 * t, th, fg),
                    1 => c.fill(cx - t, cy + off, x + w - cx + t, th, fg),
                    2 => c.fill(cx + off, y, th, cy - y + 2 * t, fg),
                    _ => c.fill(cx + off, cy - t, th, y + h - cy + t, fg),
                }
            }
        }
        true
    }
}

/// `[left, right, up, down]` line weights for the box-drawing block (diagonals excluded).
fn box_arms(u: u32) -> Option<[u8; 4]> {
    // One "LRUD" digit group per code point from U+2500; '-' = not drawn geometrically.
    const TABLE: &str = "\
1100 2200 0011 0022 1100 2200 0011 0022 1100 2200 0011 0022 \
0101 0201 0102 0202 1001 2001 1002 2002 0110 0210 0120 0220 1010 2010 1020 2020 \
0111 0211 0121 0112 0122 0221 0212 0222 1011 2011 1021 1012 1022 2021 2012 2022 \
1101 2101 1201 2201 1102 2102 1202 2202 1110 2110 1210 2210 1120 2120 1220 2220 \
1111 2111 1211 2211 1121 1112 1122 2121 1221 2112 1212 2221 2212 2122 1222 2222 \
1100 2200 0011 0022 \
3300 0033 0301 0103 0303 3001 1003 3003 0310 0130 0330 3010 1030 3030 \
0311 0133 0333 3011 1033 3033 3301 1103 3303 3310 1130 3330 3311 1133 3333 \
0101 1001 1010 0110 - - - \
1000 0010 0100 0001 2000 0020 0200 0002 1200 0012 2100 0021";
    let i = u.checked_sub(0x2500)? as usize;
    let tok = TABLE.split_whitespace().nth(i)?;
    let b = tok.as_bytes();
    (b.len() == 4).then(|| [b[0] - b'0', b[1] - b'0', b[2] - b'0', b[3] - b'0'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_table_covers_the_block() {
        assert_eq!(box_arms(0x2500), Some([1, 1, 0, 0]));
        assert_eq!(box_arms(0x250c), Some([0, 1, 0, 1]));
        assert_eq!(box_arms(0x253c), Some([1, 1, 1, 1]));
        assert_eq!(box_arms(0x254b), Some([2, 2, 2, 2]));
        assert_eq!(box_arms(0x2550), Some([3, 3, 0, 0]));
        assert_eq!(box_arms(0x256c), Some([3, 3, 3, 3]));
        assert_eq!(box_arms(0x256d), Some([0, 1, 0, 1]));
        assert_eq!(box_arms(0x2571), None);
        assert_eq!(box_arms(0x257f), Some([0, 0, 2, 1]));
        assert_eq!(box_arms(0x2580), None);
    }
}
