//! Colour emoji from the bundled Noto Color Emoji (CBDT: one PNG strike per glyph), scaled to
//! the cell box. fontdue cannot draw bitmap glyphs, so they are decoded here. Only the base
//! code point of a cell is drawn: ZWJ sequences, skin-tone modifiers and flags show their first
//! emoji.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::OnceLock;

use ttf_parser::{Face, RasterImageFormat};

const NOTO: &[u8] = include_bytes!("../../../../assets/fonts/NotoColorEmoji.ttf");

fn face() -> Option<&'static Face<'static>> {
    static FACE: OnceLock<Option<Face<'static>>> = OnceLock::new();
    FACE.get_or_init(|| Face::parse(NOTO, 0).ok()).as_ref()
}

/// An emoji scaled to fit a box, straight (non-premultiplied) RGBA.
pub struct Scaled {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct Emoji {
    cache: HashMap<(char, usize, usize), Option<Scaled>>,
}

impl Emoji {
    /// The emoji for `c` fitted (aspect kept, never enlarged past the box) into `bw`x`bh`
    /// pixels; `None` when the font has no colour glyph for it.
    pub fn get(&mut self, c: char, bw: usize, bh: usize) -> Option<&Scaled> {
        self.cache
            .entry((c, bw, bh))
            .or_insert_with(|| render(c, bw, bh))
            .as_ref()
    }
}

/// Whether the font has a colour glyph for `c`.
pub fn has(c: char) -> bool {
    face().is_some_and(|f| {
        f.glyph_index(c)
            .and_then(|g| f.glyph_raster_image(g, u16::MAX))
            .is_some()
    })
}

fn render(c: char, bw: usize, bh: usize) -> Option<Scaled> {
    let f = face()?;
    let img = f.glyph_raster_image(f.glyph_index(c)?, u16::MAX)?;
    if img.format != RasterImageFormat::PNG || bw == 0 || bh == 0 {
        return None;
    }
    let mut dec = png::Decoder::new(Cursor::new(img.data));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (sw, sh) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        _ => return None,
    };
    let src = |x: usize, y: usize| -> [f32; 4] {
        let i = (y * sw + x) * channels;
        let a = if channels == 4 { buf[i + 3] } else { 255 };
        [
            f32::from(buf[i]),
            f32::from(buf[i + 1]),
            f32::from(buf[i + 2]),
            f32::from(a),
        ]
    };
    // Fit inside the box, keeping the aspect ratio.
    let ratio = (bw as f32 / sw as f32).min(bh as f32 / sh as f32).min(1.0);
    let (tw, th) = (
        ((sw as f32 * ratio).round() as usize).max(1),
        ((sh as f32 * ratio).round() as usize).max(1),
    );
    let (kx, ky) = (sw as f32 / tw as f32, sh as f32 / th as f32);
    let mut rgba = vec![0u8; tw * th * 4];
    for dy in 0..th {
        let (y0, y1) = (dy as f32 * ky, (dy + 1) as f32 * ky);
        for dx in 0..tw {
            let (x0, x1) = (dx as f32 * kx, (dx + 1) as f32 * kx);
            // Area average with premultiplied alpha, so transparent edges do not bleed.
            let (mut r, mut g, mut b, mut a, mut wsum) = (0f32, 0f32, 0f32, 0f32, 0f32);
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(sh) {
                let wy = (sy as f32 + 1.0).min(y1) - (sy as f32).max(y0);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(sw) {
                    let w = wy * ((sx as f32 + 1.0).min(x1) - (sx as f32).max(x0));
                    let [pr, pg, pb, pa] = src(sx, sy);
                    let pa_n = pa / 255.0;
                    r += pr * pa_n * w;
                    g += pg * pa_n * w;
                    b += pb * pa_n * w;
                    a += pa_n * w;
                    wsum += w;
                }
            }
            let o = (dy * tw + dx) * 4;
            if a > 0.0 && wsum > 0.0 {
                rgba[o] = (r / a).round().clamp(0.0, 255.0) as u8;
                rgba[o + 1] = (g / a).round().clamp(0.0, 255.0) as u8;
                rgba[o + 2] = (b / a).round().clamp(0.0, 255.0) as u8;
                rgba[o + 3] = (a / wsum * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Some(Scaled { w: tw, h: th, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_font_has_the_common_emoji_and_not_cjk() {
        for c in ['📦', '🦀', '✅', '🙂', '🚀'] {
            assert!(has(c), "{c}");
        }
        assert!(!has('日'));
    }

    #[test]
    fn scaling_fits_the_box_and_keeps_colour() {
        let mut e = Emoji::default();
        let s = e.get('🦀', 40, 36).expect("crab");
        assert!(s.w <= 40 && s.h <= 36 && (s.w == 40 || s.h == 36));
        // Opaque pixels are reddish (a crab), not black.
        let red = s
            .rgba
            .chunks(4)
            .filter(|p| p[3] > 200 && p[0] > 150 && p[1] < 120)
            .count();
        assert!(red > 50, "{red}");
        // Never enlarged past the strike.
        let big = e.get('🦀', 4000, 4000).unwrap();
        assert!(big.w <= 200 && big.h <= 200);
        assert!(e.get('日', 40, 36).is_none());
    }
}
