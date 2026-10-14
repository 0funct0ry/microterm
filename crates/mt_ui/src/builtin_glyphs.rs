//! Procedurally drawn glyphs that must align to the cell grid at any size (SPEC §6.2):
//! box drawing U+2500–257F, block elements U+2580–259F, Braille U+2800–28FF and the
//! powerline separators U+E0B0–E0B3.

use gpui::{point, px, size, Bounds, Hsla, PathBuilder, Pixels, Window};

pub fn is_builtin(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0x2800..=0x28FF | 0xE0B0..=0xE0B3)
}

/// Arm weights `[left, right, up, down]`: 0 none, 1 light, 2 heavy, 3 double.
/// Dashed variants are drawn solid; arcs are drawn as square corners.
const BOX: [[u8; 4]; 0x80] = [
    [1, 1, 0, 0],
    [2, 2, 0, 0],
    [0, 0, 1, 1],
    [0, 0, 2, 2], // 2500
    [1, 1, 0, 0],
    [2, 2, 0, 0],
    [0, 0, 1, 1],
    [0, 0, 2, 2], // 2504
    [1, 1, 0, 0],
    [2, 2, 0, 0],
    [0, 0, 1, 1],
    [0, 0, 2, 2], // 2508
    [0, 1, 0, 1],
    [0, 2, 0, 1],
    [0, 1, 0, 2],
    [0, 2, 0, 2], // 250C
    [1, 0, 0, 1],
    [2, 0, 0, 1],
    [1, 0, 0, 2],
    [2, 0, 0, 2], // 2510
    [0, 1, 1, 0],
    [0, 2, 1, 0],
    [0, 1, 2, 0],
    [0, 2, 2, 0], // 2514
    [1, 0, 1, 0],
    [2, 0, 1, 0],
    [1, 0, 2, 0],
    [2, 0, 2, 0], // 2518
    [0, 1, 1, 1],
    [0, 2, 1, 1],
    [0, 1, 2, 1],
    [0, 1, 1, 2], // 251C
    [0, 1, 2, 2],
    [0, 2, 2, 1],
    [0, 2, 1, 2],
    [0, 2, 2, 2], // 2520
    [1, 0, 1, 1],
    [2, 0, 1, 1],
    [1, 0, 2, 1],
    [1, 0, 1, 2], // 2524
    [1, 0, 2, 2],
    [2, 0, 2, 1],
    [2, 0, 1, 2],
    [2, 0, 2, 2], // 2528
    [1, 1, 0, 1],
    [2, 1, 0, 1],
    [1, 2, 0, 1],
    [2, 2, 0, 1], // 252C
    [1, 1, 0, 2],
    [2, 1, 0, 2],
    [1, 2, 0, 2],
    [2, 2, 0, 2], // 2530
    [1, 1, 1, 0],
    [2, 1, 1, 0],
    [1, 2, 1, 0],
    [2, 2, 1, 0], // 2534
    [1, 1, 2, 0],
    [2, 1, 2, 0],
    [1, 2, 2, 0],
    [2, 2, 2, 0], // 2538
    [1, 1, 1, 1],
    [2, 1, 1, 1],
    [1, 2, 1, 1],
    [2, 2, 1, 1], // 253C
    [1, 1, 2, 1],
    [1, 1, 1, 2],
    [1, 1, 2, 2],
    [2, 1, 2, 1], // 2540
    [1, 2, 2, 1],
    [2, 1, 1, 2],
    [1, 2, 1, 2],
    [2, 2, 2, 1], // 2544
    [2, 2, 1, 2],
    [2, 1, 2, 2],
    [1, 2, 2, 2],
    [2, 2, 2, 2], // 2548
    [1, 1, 0, 0],
    [2, 2, 0, 0],
    [0, 0, 1, 1],
    [0, 0, 2, 2], // 254C
    [3, 3, 0, 0],
    [0, 0, 3, 3],
    [0, 3, 0, 1],
    [0, 1, 0, 3], // 2550
    [0, 3, 0, 3],
    [3, 0, 0, 1],
    [1, 0, 0, 3],
    [3, 0, 0, 3], // 2554
    [0, 3, 1, 0],
    [0, 1, 3, 0],
    [0, 3, 3, 0],
    [3, 0, 1, 0], // 2558
    [1, 0, 3, 0],
    [3, 0, 3, 0],
    [0, 3, 1, 1],
    [0, 1, 3, 3], // 255C
    [0, 3, 3, 3],
    [3, 0, 1, 1],
    [1, 0, 3, 3],
    [3, 0, 3, 3], // 2560
    [3, 3, 0, 1],
    [1, 1, 0, 3],
    [3, 3, 0, 3],
    [3, 3, 1, 0], // 2564
    [1, 1, 3, 0],
    [3, 3, 3, 0],
    [3, 3, 1, 1],
    [1, 1, 3, 3], // 2568
    [3, 3, 3, 3],
    [0, 1, 0, 1],
    [1, 0, 0, 1],
    [1, 0, 1, 0], // 256C
    [0, 1, 1, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0], // 2570 (2571-2573 diagonals)
    [1, 0, 0, 0],
    [0, 0, 1, 0],
    [0, 1, 0, 0],
    [0, 0, 0, 1], // 2574
    [2, 0, 0, 0],
    [0, 0, 2, 0],
    [0, 2, 0, 0],
    [0, 0, 0, 2], // 2578
    [1, 2, 0, 0],
    [0, 0, 1, 2],
    [2, 1, 0, 0],
    [0, 0, 2, 1], // 257C
];

/// Quadrant bit set per block element 2596..=259F: UL=1, UR=2, LL=4, LR=8.
const QUADRANTS: [u8; 10] = [
    4,
    8,
    1,
    1 | 4 | 8,
    1 | 8,
    1 | 2 | 4,
    1 | 2 | 8,
    2,
    2 | 4,
    2 | 4 | 8,
];

fn rect(window: &mut Window, x: f32, y: f32, w: f32, h: f32, color: Hsla) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    window.paint_quad(gpui::fill(
        Bounds::new(point(px(x), px(y)), size(px(w), px(h))),
        color,
    ));
}

/// Draws `c` into `cell`. Returns false when `c` is not a built-in glyph.
pub fn paint(c: char, cell: Bounds<Pixels>, color: Hsla, window: &mut Window) -> bool {
    let (x, y) = (f32::from(cell.origin.x), f32::from(cell.origin.y));
    let (w, h) = (f32::from(cell.size.width), f32::from(cell.size.height));
    match c as u32 {
        0x2571..=0x2573 => diagonals(c, x, y, w, h, color, window),
        0x2500..=0x257F => box_drawing(c as u32 - 0x2500, x, y, w, h, color, window),
        0x2580..=0x259F => block(c as u32, x, y, w, h, color, window),
        0x2800..=0x28FF => braille(c as u32 - 0x2800, x, y, w, h, color, window),
        0xE0B0..=0xE0B3 => powerline(c, x, y, w, h, color, window),
        _ => return false,
    }
    true
}

fn box_drawing(i: u32, x: f32, y: f32, w: f32, h: f32, color: Hsla, window: &mut Window) {
    let [l, r, u, d] = BOX[i as usize];
    let light = (h / 14.0).max(1.0).round();
    let thick = |weight: u8| if weight == 2 { light * 2.0 } else { light };
    // Half of the gap between double lines' centres.
    let gap = light;
    let (cx, cy) = ((x + w / 2.0).round(), (y + h / 2.0).round());

    // Extent a perpendicular arm group occupies around the centre line.
    let ext = |a: u8, b: u8| -> f32 {
        [a, b]
            .into_iter()
            .map(|wt| match wt {
                0 => 0.0,
                3 => gap + light / 2.0,
                wt => thick(wt) / 2.0,
            })
            .fold(0.0, f32::max)
    };
    let v_ext = ext(u, d);
    let h_ext = ext(l, r);

    // Horizontal lines of an arm: (centre offset, thickness).
    let lines = |weight: u8| -> Vec<(f32, f32)> {
        match weight {
            3 => vec![(-gap, light), (gap, light)],
            wt => vec![(0.0, thick(wt))],
        }
    };

    let is_corner = (l > 0) as u8 + (r > 0) as u8 == 1 && (u > 0) as u8 + (d > 0) as u8 == 1;
    let (hw, vw) = (l.max(r), u.max(d));
    if is_corner && hw == 3 && vw == 3 {
        // Double corner: outer lines meet at the outer corner, inner lines at the inner one.
        let hs = if r > 0 { 1.0 } else { -1.0 }; // direction of the horizontal arm
        let vs = if d > 0 { 1.0 } else { -1.0 }; // direction of the vertical arm
        for (off, t) in lines(3) {
            // Horizontal line at cy+off; it starts at the vertical line on the side it
            // turns away from: the outer one when it is on the opposite side to the arm.
            let outer = off * vs < 0.0;
            let v_off = if outer { -gap * hs } else { gap * hs };
            let (x0, x1) = if hs > 0.0 {
                (cx + v_off - light / 2.0, x + w)
            } else {
                (x, cx + v_off + light / 2.0)
            };
            rect(window, x0, cy + off - t / 2.0, x1 - x0, t, color);
        }
        for (off, t) in lines(3) {
            let outer = off * hs < 0.0;
            let h_off = if outer { -gap * vs } else { gap * vs };
            let (y0, y1) = if vs > 0.0 {
                (cy + h_off - light / 2.0, y + h)
            } else {
                (y, cy + h_off + light / 2.0)
            };
            rect(window, cx + off - t / 2.0, y0, t, y1 - y0, color);
        }
        return;
    }

    if r > 0 {
        for (off, t) in lines(r) {
            let start = if l > 0 { cx } else { cx - v_ext };
            rect(window, start, cy + off - t / 2.0, x + w - start, t, color);
        }
    }
    if l > 0 {
        for (off, t) in lines(l) {
            let end = if r > 0 { cx } else { cx + v_ext };
            rect(window, x, cy + off - t / 2.0, end - x, t, color);
        }
    }
    if d > 0 {
        for (off, t) in lines(d) {
            let start = if u > 0 { cy } else { cy - h_ext };
            rect(window, cx + off - t / 2.0, start, t, y + h - start, color);
        }
    }
    if u > 0 {
        for (off, t) in lines(u) {
            let end = if d > 0 { cy } else { cy + h_ext };
            rect(window, cx + off - t / 2.0, y, t, end - y, color);
        }
    }
}

fn diagonals(c: char, x: f32, y: f32, w: f32, h: f32, color: Hsla, window: &mut Window) {
    let t = (h / 14.0).max(1.0);
    let mut line = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let mut b = PathBuilder::stroke(px(t));
        b.move_to(point(px(x0), px(y0)));
        b.line_to(point(px(x1), px(y1)));
        if let Ok(p) = b.build() {
            window.paint_path(p, color);
        }
    };
    if c == '\u{2571}' || c == '\u{2573}' {
        line(x + w, y, x, y + h);
    }
    if c == '\u{2572}' || c == '\u{2573}' {
        line(x, y, x + w, y + h);
    }
}

fn block(c: u32, x: f32, y: f32, w: f32, h: f32, color: Hsla, window: &mut Window) {
    match c {
        0x2580 => rect(window, x, y, w, (h / 2.0).round(), color),
        0x2581..=0x2588 => {
            let n = (c - 0x2580) as f32;
            let bh = (h * n / 8.0).round();
            rect(window, x, y + h - bh, w, bh, color);
        }
        0x2589..=0x258F => {
            let n = (0x2590 - c) as f32;
            rect(window, x, y, (w * n / 8.0).round(), h, color);
        }
        0x2590 => {
            let half = (w / 2.0).round();
            rect(window, x + w - half, y, half, h, color);
        }
        0x2591..=0x2593 => {
            let alpha = (c - 0x2590) as f32 * 0.25;
            rect(window, x, y, w, h, color.opacity(alpha));
        }
        0x2594 => rect(window, x, y, w, (h / 8.0).round().max(1.0), color),
        0x2595 => {
            let bw = (w / 8.0).round().max(1.0);
            rect(window, x + w - bw, y, bw, h, color);
        }
        0x2596..=0x259F => {
            let q = QUADRANTS[(c - 0x2596) as usize];
            let (hw, hh) = ((w / 2.0).round(), (h / 2.0).round());
            if q & 1 != 0 {
                rect(window, x, y, hw, hh, color);
            }
            if q & 2 != 0 {
                rect(window, x + hw, y, w - hw, hh, color);
            }
            if q & 4 != 0 {
                rect(window, x, y + hh, hw, h - hh, color);
            }
            if q & 8 != 0 {
                rect(window, x + hw, y + hh, w - hw, h - hh, color);
            }
        }
        _ => {}
    }
}

fn braille(bits: u32, x: f32, y: f32, w: f32, h: f32, color: Hsla, window: &mut Window) {
    // Dot bit → (column, row).
    const DOTS: [(usize, usize); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let d = (w / 4.0).min(h / 8.0).max(1.0);
    for (bit, (col, row)) in DOTS.iter().enumerate() {
        if bits & (1 << bit) != 0 {
            let cx = x + w * (0.3 + 0.4 * *col as f32);
            let cy = y + h * (0.125 + 0.25 * *row as f32);
            rect(
                window,
                (cx - d / 2.0).round(),
                (cy - d / 2.0).round(),
                d,
                d,
                color,
            );
        }
    }
}

fn powerline(c: char, x: f32, y: f32, w: f32, h: f32, color: Hsla, window: &mut Window) {
    let solid = matches!(c, '\u{E0B0}' | '\u{E0B2}');
    let right = matches!(c, '\u{E0B0}' | '\u{E0B1}');
    // Slight overdraw keeps adjacent segments seamless.
    let (x0, x1) = if right { (x, x + w) } else { (x + w, x) };
    let mut b = if solid {
        PathBuilder::fill()
    } else {
        PathBuilder::stroke(px((h / 14.0).max(1.0)))
    };
    b.move_to(point(px(x0), px(y - 0.5)));
    b.line_to(point(px(x1), px(y + h / 2.0)));
    b.line_to(point(px(x0), px(y + h + 0.5)));
    if solid {
        b.close();
    }
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}
