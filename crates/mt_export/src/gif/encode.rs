//! Delta-frame GIF encoder: each frame is the changed rectangle with unchanged pixels
//! transparent and its own local palette. Frames stream to the writer as they are produced.

use std::collections::HashMap;
use std::io::{self, Write};

use gif::{DisposalMethod, Frame, Repeat};

use super::raster::{Canvas, Rect, Rgb};
use crate::ExportError;

/// Colours per local palette; index 255 is the transparent one.
const COLORS: usize = 255;
const TRANSPARENT: u8 = 255;

pub struct Counter<'a> {
    inner: &'a mut dyn Write,
    pub bytes: u64,
}

impl Write for Counter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The pixels of a changed rectangle, copied out of the canvas when it was drawn.
struct Patch {
    rect: Rect,
    /// Row-major RGB of `rect`.
    rgb: Vec<u8>,
    start_cs: u64,
}

pub struct Encoder<'a> {
    enc: gif::Encoder<Counter<'a>>,
    width: usize,
    height: usize,
    mask: Vec<bool>,
    max_bytes: u64,
    /// The screen after the last written frame.
    last: Option<Canvas>,
    /// The newest screen change; written once its delay is known.
    pending: Option<Patch>,
    pub frames: u64,
}

fn gif_err(e: gif::EncodingError) -> ExportError {
    match e {
        gif::EncodingError::Io(e) => ExportError::Io(e),
        other => ExportError::Io(io::Error::other(other)),
    }
}

fn copy_rect(c: &Canvas, r: Rect) -> Vec<u8> {
    let mut out = Vec::with_capacity((r.x1 - r.x0) * (r.y1 - r.y0) * 3);
    for y in r.y0..r.y1 {
        out.extend_from_slice(&c.rgb[(y * c.width + r.x0) * 3..(y * c.width + r.x1) * 3]);
    }
    out
}

impl<'a> Encoder<'a> {
    /// `plays` = 0 loops forever.
    pub fn new(
        out: &'a mut dyn Write,
        width: usize,
        height: usize,
        mask: Vec<bool>,
        plays: u16,
        max_bytes: u64,
    ) -> Result<Self, ExportError> {
        let (w, h) = (u16::try_from(width), u16::try_from(height));
        let (Ok(w), Ok(h)) = (w, h) else {
            return Err(ExportError::BadOption(format!(
                "a {width}x{height} GIF is too large: lower scale or font_size"
            )));
        };
        let mut enc = gif::Encoder::new(
            Counter {
                inner: out,
                bytes: 0,
            },
            w,
            h,
            &[],
        )
        .map_err(gif_err)?;
        let repeat = match plays {
            0 => Repeat::Infinite,
            n => Repeat::Finite(n - 1),
        };
        enc.set_repeat(repeat).map_err(gif_err)?;
        Ok(Self {
            enc,
            width,
            height,
            mask,
            max_bytes,
            last: None,
            pending: None,
            frames: 0,
        })
    }

    pub fn bytes(&mut self) -> u64 {
        self.enc.get_mut().bytes
    }

    /// Offers the screen at `start_cs`; `rect` is what changed since the previous screen.
    pub fn push(&mut self, canvas: &Canvas, rect: Rect, start_cs: u64) -> Result<(), ExportError> {
        if let Some(prev) = &self.pending {
            let delay = start_cs.saturating_sub(prev.start_cs);
            if delay == 0 {
                // Same centisecond: one frame with both changes.
                let (start, union) = (prev.start_cs, prev.rect.union(rect));
                self.pending = Some(Patch {
                    rect: union,
                    rgb: copy_rect(canvas, union),
                    start_cs: start,
                });
                return Ok(());
            }
            let prev = self.pending.take().expect("checked");
            self.write(prev, delay)?;
        }
        self.pending = Some(Patch {
            rect,
            rgb: copy_rect(canvas, rect),
            start_cs,
        });
        Ok(())
    }

    /// Writes the last frame so it lasts until `end_cs`.
    pub fn finish(mut self, end_cs: u64) -> Result<u64, ExportError> {
        if let Some(p) = self.pending.take() {
            let delay = end_cs.saturating_sub(p.start_cs);
            self.write(p, delay)?;
        }
        let frames = self.frames;
        self.enc.into_inner().map_err(gif_err)?;
        Ok(frames)
    }

    fn write(&mut self, p: Patch, delay_cs: u64) -> Result<(), ExportError> {
        let Patch { rect, rgb, .. } = p;
        let rw = rect.x1 - rect.x0;
        let w = self.width;
        let last = self.last.get_or_insert_with(|| Canvas {
            width: w,
            height: self.height,
            rgb: vec![0; w * self.height * 3],
        });
        let first = self.frames == 0;
        let (mask, last_ref) = (&self.mask, &*last);
        let changed = |x: usize, y: usize, px: &[u8]| {
            let i = y * w + x;
            if first {
                !mask[i]
            } else {
                last_ref.rgb[i * 3..i * 3 + 3] != *px
            }
        };
        // Tight bounding box of the pixels that really changed.
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for (k, px) in rgb.chunks_exact(3).enumerate() {
            let (x, y) = (rect.x0 + k % rw, rect.y0 + k / rw);
            if changed(x, y, px) {
                (x0, y0) = (x0.min(x), y0.min(y));
                (x1, y1) = (x1.max(x + 1), y1.max(y + 1));
            }
        }
        if x1 == 0 {
            // Nothing visible changed: a one-pixel transparent frame keeps the timing.
            (x0, y0, x1, y1) = (rect.x0, rect.y0, rect.x0 + 1, rect.y0 + 1);
        }
        let (fw, fh) = (x1 - x0, y1 - y0);
        let mut px: Vec<Option<Rgb>> = Vec::with_capacity(fw * fh);
        for y in y0..y1 {
            for x in x0..x1 {
                let k = ((y - rect.y0) * rw + (x - rect.x0)) * 3;
                let color = [rgb[k], rgb[k + 1], rgb[k + 2]];
                // An unchanged pixel repeats the previous pixel's colour when it can (longer
                // runs compress better) and is transparent otherwise.
                let keep_run = px.last() == Some(&Some(color)) && !mask[y * w + x];
                px.push((keep_run || changed(x, y, &color)).then_some(color));
            }
        }
        let (palette, buffer) = quantise(&px);
        let frame = Frame {
            left: x0 as u16,
            top: y0 as u16,
            width: fw as u16,
            height: fh as u16,
            // Browsers treat delays under 2 cs as 10 cs.
            delay: delay_cs.clamp(2, u64::from(u16::MAX)) as u16,
            dispose: DisposalMethod::Keep,
            transparent: Some(TRANSPARENT),
            palette: Some(palette),
            buffer: buffer.into(),
            ..Frame::default()
        };
        self.enc.write_frame(&frame).map_err(gif_err)?;
        self.frames += 1;
        // The screen after this frame.
        let last = self.last.as_mut().expect("set above");
        for y in rect.y0..rect.y1 {
            let src = (y - rect.y0) * rw * 3;
            let dst = (y * w + rect.x0) * 3;
            last.rgb[dst..dst + rw * 3].copy_from_slice(&rgb[src..src + rw * 3]);
        }
        let bytes = self.bytes();
        if bytes > self.max_bytes {
            return Err(ExportError::NotPossible(format!(
                "the GIF would exceed max_size_mb ({} MB): lower fps or scale, trim the \
                 recording, or raise max_size_mb",
                self.max_bytes / (1024 * 1024)
            )));
        }
        Ok(())
    }
}

/// A local palette (256 entries, the last transparent) and one index per pixel. Exact when the
/// rectangle has at most 255 colours, a popularity palette otherwise.
fn quantise(px: &[Option<Rgb>]) -> (Vec<u8>, Vec<u8>) {
    let mut exact: HashMap<Rgb, u8> = HashMap::new();
    let mut order: Vec<Rgb> = Vec::new();
    let mut run: Option<Rgb> = None;
    for c in px.iter().flatten() {
        // Most pixels repeat their neighbour: skip the hash lookup for those.
        if run == Some(*c) {
            continue;
        }
        run = Some(*c);
        if !exact.contains_key(c) {
            if order.len() == COLORS {
                return popularity(px);
            }
            exact.insert(*c, order.len() as u8);
            order.push(*c);
        }
    }
    let mut palette: Vec<u8> = order.concat();
    palette.resize(256 * 3, 0);
    let mut cache: Option<(Rgb, u8)> = None;
    let buffer = px
        .iter()
        .map(|p| match p {
            None => TRANSPARENT,
            Some(c) => match cache {
                Some((k, i)) if k == *c => i,
                _ => {
                    let i = exact[c];
                    cache = Some((*c, i));
                    i
                }
            },
        })
        .collect();
    (palette, buffer)
}

/// More than 255 colours (shadow gradients, antialiased text): keep the 255 most used colours
/// exactly, so theme colours and solid cells never shift, and map the rest to the nearest one.
fn popularity(px: &[Option<Rgb>]) -> (Vec<u8>, Vec<u8>) {
    let mut counts: HashMap<Rgb, u32> = HashMap::new();
    for c in px.iter().flatten() {
        *counts.entry(*c).or_default() += 1;
    }
    let mut by_use: Vec<(Rgb, u32)> = counts.into_iter().collect();
    by_use.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let kept: Vec<Rgb> = by_use.iter().take(COLORS).map(|(c, _)| *c).collect();
    let mut index: HashMap<Rgb, u8> = kept
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, i as u8))
        .collect();
    let nearest = |c: &Rgb| {
        let d = |k: &Rgb| -> i32 {
            (0..3)
                .map(|i| (i32::from(c[i]) - i32::from(k[i])).pow(2))
                .sum()
        };
        (0..kept.len()).min_by_key(|&i| d(&kept[i])).unwrap_or(0) as u8
    };
    let buffer = px
        .iter()
        .map(|p| match p {
            None => TRANSPARENT,
            Some(c) => *index.entry(*c).or_insert_with(|| nearest(c)),
        })
        .collect();
    let mut palette: Vec<u8> = kept.concat();
    palette.resize(256 * 3, 0);
    (palette, buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_palette_when_few_colours() {
        let px = [Some([1, 2, 3]), None, Some([4, 5, 6]), Some([1, 2, 3])];
        let (pal, buf) = quantise(&px);
        assert_eq!(buf, [0, TRANSPARENT, 1, 0]);
        assert_eq!(&pal[..6], &[1, 2, 3, 4, 5, 6]);
        assert_eq!(pal.len(), 768);
    }

    #[test]
    fn many_colours_keep_the_most_used_exactly() {
        let mut px: Vec<_> = (0..4096u32)
            .map(|i| Some([(i % 256) as u8, (i / 16) as u8, (i * 7 % 256) as u8]))
            .collect();
        px.extend(std::iter::repeat_n(Some([40, 44, 52]), 5000));
        let a = quantise(&px);
        assert_eq!(a, quantise(&px));
        assert!(a.1.iter().all(|&i| i < TRANSPARENT));
        assert_eq!(&a.0[..3], &[40, 44, 52]);
        assert_eq!(*a.1.last().unwrap(), 0);
    }
}
