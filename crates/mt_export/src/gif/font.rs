//! Bundled fonts and a glyph cache. No system font lookup, so output is reproducible.

use std::collections::HashMap;
use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

const REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/JetBrainsMono_400Regular.ttf");
const ITALIC: &[u8] =
    include_bytes!("../../../../assets/fonts/JetBrainsMono_400Regular_Italic.ttf");
const BOLD: &[u8] = include_bytes!("../../../../assets/fonts/JetBrainsMono_700Bold.ttf");
const BOLD_ITALIC: &[u8] =
    include_bytes!("../../../../assets/fonts/JetBrainsMono_700Bold_Italic.ttf");
const SYMBOLS: &[u8] = include_bytes!("../../../../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

struct Faces {
    /// regular, bold, italic, bold-italic.
    styles: [Font; 4],
    symbols: Font,
}

fn faces() -> &'static Faces {
    static FACES: OnceLock<Faces> = OnceLock::new();
    FACES.get_or_init(|| {
        let load =
            |b: &[u8]| Font::from_bytes(b, FontSettings::default()).expect("bundled font parses");
        Faces {
            styles: [load(REGULAR), load(BOLD), load(ITALIC), load(BOLD_ITALIC)],
            symbols: load(SYMBOLS),
        }
    })
}

/// Index into the style faces: bit 0 bold, bit 1 italic.
pub fn style(bold: bool, italic: bool) -> u8 {
    u8::from(bold) | (u8::from(italic) << 1)
}

pub struct Glyph {
    /// Left edge relative to the cell's left.
    pub x: i32,
    /// Top edge relative to the baseline (negative = above).
    pub y: i32,
    pub w: usize,
    pub h: usize,
    pub coverage: Vec<u8>,
}

pub struct Fonts {
    px: f32,
    pub cell_w: usize,
    pub cell_h: usize,
    /// Baseline offset from the cell top.
    pub ascent: i32,
    cache: HashMap<(char, u8), Option<Glyph>>,
}

impl Fonts {
    /// `line_height` multiplies the face's own line height (1.0 = as designed).
    pub fn new(px: f32, line_height: f32) -> Self {
        let f = &faces().styles[0];
        let lm = f
            .horizontal_line_metrics(px)
            .expect("bundled font has horizontal metrics");
        let cell_w = f.metrics('M', px).advance_width.round().max(1.0) as usize;
        let cell_h = ((lm.ascent - lm.descent + lm.line_gap) * line_height)
            .ceil()
            .max(1.0) as usize;
        let pad = (cell_h as f32 - (lm.ascent - lm.descent)) / 2.0;
        Self {
            px,
            cell_w,
            cell_h,
            ascent: (lm.ascent + pad).round() as i32,
            cache: HashMap::new(),
        }
    }

    /// The glyph for `c` in `style`, from the style face or the symbols face; `None` when no
    /// bundled face has it (the caller draws the fallback box).
    pub fn glyph(&mut self, c: char, style: u8) -> Option<&Glyph> {
        let px = self.px;
        self.cache
            .entry((c, style))
            .or_insert_with(|| {
                let faces = faces();
                let face = &faces.styles[usize::from(style & 3)];
                let font = if face.lookup_glyph_index(c) != 0 {
                    face
                } else if faces.symbols.lookup_glyph_index(c) != 0 {
                    &faces.symbols
                } else {
                    return None;
                };
                let (m, coverage) = font.rasterize(c, px);
                Some(Glyph {
                    x: m.xmin,
                    y: -(m.height as i32 + m.ymin),
                    w: m.width,
                    h: m.height,
                    coverage,
                })
            })
            .as_ref()
    }
}
