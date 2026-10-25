//! One frame as a PNG (SPEC §10.11.4 `screenshot`): the visible grid of a pane, drawn by the
//! deterministic software rasteriser of the GIF exporter. It is **not** a window capture, so it
//! works the same on macOS, X11 and Wayland and needs no OS permission. It draws with the
//! bundled fonts and the active theme, counts cells no bundled face can draw (ligatures,
//! graphics, unknown characters) and shows no images.

use mt_term::RenderSnapshot;
use mt_theme::Theme;

use crate::gif::raster::{Chrome, Params, Renderer};

/// Largest side of a screenshot in pixels (SPEC §10.11.4).
pub const MAX_SIDE: usize = 4096;
/// Largest PNG in bytes.
pub const MAX_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOpts {
    /// 0.5-2.0.
    pub scale: f32,
    /// A title bar with the pane's title, or nothing around the grid.
    pub plain_chrome: bool,
    pub font_size: f32,
}

impl Default for FrameOpts {
    fn default() -> Self {
        Self {
            scale: 1.0,
            plain_chrome: false,
            font_size: 16.0,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("too large")]
    TooLarge,
    #[error("png: {0}")]
    Encode(String),
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub png: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// Cells drawn with a fallback glyph or not at all.
    pub fallback_cells: u64,
}

/// Renders `snap` (the whole visible screen, cursor included) as a PNG.
pub fn png_frame(
    snap: &RenderSnapshot,
    theme: &Theme,
    title: &str,
    opts: &FrameOpts,
) -> Result<Frame, FrameError> {
    let mut r = Renderer::new(Params {
        theme,
        font_size: opts.font_size,
        line_height: 1.1,
        scale: opts.scale.clamp(0.5, 2.0),
        padding: 12.0,
        margin: 0.0,
        margin_fill: None,
        shadow: false,
        chrome: if opts.plain_chrome {
            Chrome::Plain
        } else {
            Chrome::None
        },
        title: title.to_string(),
        cursor: true,
        cols: snap.cols.max(1),
        rows: snap.rows.max(1),
    });
    let (w, h) = (r.layout.width, r.layout.height);
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err(FrameError::TooLarge);
    }
    r.render(snap);
    let canvas = r.canvas();
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, canvas.width as u32, canvas.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|e| FrameError::Encode(e.to_string()))?;
        writer
            .write_image_data(&canvas.rgb)
            .map_err(|e| FrameError::Encode(e.to_string()))?;
    }
    if png.len() > MAX_BYTES {
        return Err(FrameError::TooLarge);
    }
    Ok(Frame {
        png,
        width: canvas.width,
        height: canvas.height,
        fallback_cells: r.fallback_cells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_term::Emulator;

    fn theme() -> Theme {
        mt_theme::Registry::bundled()
            .resolve("One Dark", true)
            .clone()
    }

    fn snap(text: &str, cols: usize, rows: usize) -> RenderSnapshot {
        let mut e = Emulator::new(cols, rows, 10);
        e.advance(text.as_bytes());
        e.snapshot()
    }

    #[test]
    fn renders_a_valid_png_of_the_grid() {
        let s = snap("hello \x1b[31mred\x1b[0m\r\nline two", 40, 5);
        let f = png_frame(&s, &theme(), "zsh", &FrameOpts::default()).unwrap();
        assert!(f.png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let dec = png::Decoder::new(std::io::Cursor::new(&f.png));
        let mut rd = dec.read_info().unwrap();
        let mut buf = vec![0; rd.output_buffer_size()];
        let info = rd.next_frame(&mut buf).unwrap();
        assert_eq!(
            (info.width as usize, info.height as usize),
            (f.width, f.height)
        );
        // Something other than the background was drawn.
        let bg = &buf[..3];
        assert!(buf.chunks(3).any(|p| p != bg));
        assert_eq!(f.fallback_cells, 0);
    }

    #[test]
    fn rendering_is_deterministic() {
        let s = snap("same\r\nsame", 30, 4);
        let a = png_frame(&s, &theme(), "t", &FrameOpts::default()).unwrap();
        let b = png_frame(&s, &theme(), "t", &FrameOpts::default()).unwrap();
        assert_eq!(a.png, b.png);
    }

    #[test]
    fn scale_and_chrome_change_the_size() {
        let s = snap("x", 20, 3);
        let one = png_frame(&s, &theme(), "t", &FrameOpts::default()).unwrap();
        let two = png_frame(
            &s,
            &theme(),
            "t",
            &FrameOpts {
                scale: 2.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(two.width > one.width && two.height > one.height);
        let bar = png_frame(
            &s,
            &theme(),
            "t",
            &FrameOpts {
                plain_chrome: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(bar.height > one.height, "a title bar adds height");
    }

    #[test]
    fn huge_grids_are_refused_not_rendered() {
        let s = snap("x", 500, 200);
        let r = png_frame(
            &s,
            &theme(),
            "t",
            &FrameOpts {
                scale: 2.0,
                ..Default::default()
            },
        );
        assert_eq!(r.unwrap_err(), FrameError::TooLarge);
    }
}
