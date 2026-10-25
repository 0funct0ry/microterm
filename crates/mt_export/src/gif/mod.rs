//! Animated GIF exporter (SPEC §10.10.6): replays output through a headless `mt_term`
//! emulator, rasterises sampled frames in software and encodes delta frames. Deterministic:
//! no wall clock, no system fonts, no dithering.

mod emoji;
mod encode;
mod font;
pub(crate) mod raster;
mod schedule;

use std::io::Write;

use mt_record::EventKind;
use mt_term::Emulator;
use mt_theme::Registry;

use crate::{
    ExportError, ExportReport, Exporter, OptKind, OptValue, OptionSpec, Options, Progress,
    Recording, Requires,
};
use raster::{Chrome, Params, Renderer};
use schedule::Timing;

pub struct Gif;

const MAX_SECS: f64 = 1_000_000.0;

fn float(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    min: f64,
    max: f64,
    d: f64,
) -> OptionSpec {
    OptionSpec {
        key,
        label,
        help,
        kind: OptKind::Float { min, max },
        default: OptValue::Float(d),
    }
}

fn int(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    min: i64,
    max: i64,
    d: i64,
) -> OptionSpec {
    OptionSpec {
        key,
        label,
        help,
        kind: OptKind::Int { min, max },
        default: OptValue::Int(d),
    }
}

impl Exporter for Gif {
    fn id(&self) -> &'static str {
        "gif"
    }
    fn label(&self) -> &'static str {
        "Animated GIF (.gif)"
    }
    fn extension(&self) -> &'static str {
        "gif"
    }
    /// In the export dialog's visual and Tab order: appearance first, then text and timing.
    fn options(&self) -> Vec<OptionSpec> {
        vec![
            OptionSpec {
                key: "title",
                label: "Title",
                help: "Text in the window's title bar (empty = the recording's name)",
                kind: OptKind::Text,
                default: OptValue::Text(String::new()),
            },
            OptionSpec {
                key: "theme",
                label: "Theme",
                help:
                    "Colour scheme of the terminal (default: the one the recording was made with)",
                kind: OptKind::Text,
                default: OptValue::Text(String::new()),
            },
            OptionSpec {
                key: "chrome",
                label: "Window chrome",
                help: "none, plain (rounded border and title) or macos (plus traffic lights)",
                kind: OptKind::Enum(&["none", "plain", "macos"]),
                default: OptValue::Text("macos".into()),
            },
            OptionSpec {
                key: "margin_fill",
                label: "Margin colour",
                help: "Backdrop colour as #rrggbb (empty = a shade of the theme background)",
                kind: OptKind::Text,
                default: OptValue::Text(String::new()),
            },
            int(
                "margin",
                "Margin (px)",
                "Backdrop around the window (0 = none, corners transparent)",
                0,
                200,
                32,
            ),
            int(
                "padding",
                "Padding (px)",
                "Space around the grid",
                0,
                64,
                16,
            ),
            float(
                "scale",
                "Scale",
                "Multiplies every pixel size (2 = sharp on Retina screens)",
                0.25,
                3.0,
                2.0,
            ),
            OptionSpec {
                key: "shadow",
                label: "Shadow",
                help: "Soft drop shadow under the window (needs a margin)",
                kind: OptKind::Bool,
                default: OptValue::Bool(true),
            },
            int(
                "font_size",
                "Font size (px)",
                "Terminal font size before scaling",
                6,
                48,
                16,
            ),
            float(
                "line_height",
                "Line height",
                "Multiplies the font's line height",
                0.8,
                2.0,
                1.1,
            ),
            int(
                "fps",
                "Frames per second",
                "Sampling rate of the replay",
                1,
                30,
                10,
            ),
            float(
                "speed",
                "Speed",
                "Playback speed (2 = twice as fast)",
                0.1,
                10.0,
                1.0,
            ),
            float(
                "idle_limit",
                "Idle limit (s)",
                "Shorten pauses to this many seconds (0 = off)",
                0.0,
                MAX_SECS,
                2.0,
            ),
            float(
                "end_pause",
                "End pause (s)",
                "How long the last frame stays",
                0.0,
                60.0,
                2.0,
            ),
            float(
                "trim_start",
                "Trim start (s)",
                "Skip this many seconds from the start",
                0.0,
                MAX_SECS,
                0.0,
            ),
            float(
                "trim_end",
                "Trim end (s)",
                "Drop this many seconds from the end",
                0.0,
                MAX_SECS,
                0.0,
            ),
            int(
                "loop",
                "Plays",
                "How many times the GIF plays (0 = forever)",
                0,
                1000,
                0,
            ),
            int(
                "max_size_mb",
                "Max size (MB)",
                "Abort instead of writing a larger file",
                1,
                1000,
                50,
            ),
            OptionSpec {
                key: "cursor",
                label: "Cursor",
                help: "Draw the cursor",
                kind: OptKind::Bool,
                default: OptValue::Bool(true),
            },
            OptionSpec {
                key: "crop",
                label: "Crop to content",
                help: "Show only the part of the terminal the recording uses, so the text fills the picture (off = the whole recorded window)",
                kind: OptKind::Bool,
                default: OptValue::Bool(true),
            },
        ]
    }
    fn requires(&self) -> Requires {
        Requires::NONE
    }

    fn export(
        &self,
        rec: &Recording,
        opts: &Options,
        out: &mut dyn Write,
        progress: &dyn Progress,
    ) -> Result<ExportReport, ExportError> {
        let header = rec.header()?;
        let events = rec.events()?;
        let timing = Timing {
            fps: opts.int("fps").clamp(1, 30) as u32,
            speed: opts.float("speed").max(0.1),
            idle_limit: opts.float("idle_limit"),
            trim_start: opts.float("trim_start"),
            trim_end: opts.float("trim_end"),
        };
        let tl = schedule::timeline(&events, &timing);

        let (mut cols, mut rows) = (usize::from(header.width), usize::from(header.height));
        let (mut max_cols, mut max_rows) = (cols.max(1), rows.max(1));
        for e in &events {
            if let EventKind::Resize { cols, rows } = e.kind {
                max_cols = max_cols.max(usize::from(cols));
                max_rows = max_rows.max(usize::from(rows));
            }
        }

        let bundled;
        let registry = match &rec.themes {
            Some(r) => &**r,
            None => {
                bundled = Registry::bundled();
                &bundled
            }
        };
        let theme = match opts.text("theme") {
            "" => registry.resolve(&rec.sidecar.theme, true),
            name => registry.get(name).ok_or_else(|| {
                ExportError::BadOption(format!("option theme: no theme named \"{name}\""))
            })?,
        };

        let margin_fill = match opts.text("margin_fill").trim() {
            "" => None,
            raw => {
                let c = mt_theme::Rgba::parse(raw)
                    .map_err(|e| ExportError::BadOption(format!("option margin_fill: {e}")))?;
                Some([c.r, c.g, c.b])
            }
        };
        let fps = u64::from(timing.fps);
        cols = cols.max(1);
        rows = rows.max(1);
        let draw_cursor = opts.bool("cursor");

        // Crop to the cells the recording ever uses (a 238x56 window that only shows a prompt
        // would otherwise be mostly empty and its text tiny), found by a first replay.
        let (vis_cols, vis_rows) = if opts.bool("crop") {
            let (mut used_cols, mut used_rows) = (0, 0);
            replay(&tl, cols, rows, fps, progress, false, |snap, _| {
                let (c, r) = content_extent(snap, draw_cursor);
                used_cols = used_cols.max(c);
                used_rows = used_rows.max(r);
                Ok(())
            })?;
            (
                used_cols.max(MIN_COLS).min(max_cols),
                used_rows.max(MIN_ROWS).min(max_rows),
            )
        } else {
            (max_cols, max_rows)
        };

        let mut renderer = Renderer::new(Params {
            theme,
            font_size: opts.int("font_size") as f32,
            line_height: opts.float("line_height") as f32,
            scale: opts.float("scale") as f32,
            padding: opts.int("padding") as f32,
            margin: opts.int("margin") as f32,
            margin_fill,
            shadow: opts.bool("shadow"),
            chrome: Chrome::parse(opts.text("chrome")),
            title: match opts.text("title").trim() {
                "" => rec.sidecar.title.clone(),
                custom => custom.to_string(),
            },
            cursor: draw_cursor,
            cols: vis_cols,
            rows: vis_rows,
        });
        let max_bytes = (opts.int("max_size_mb").max(1) as u64) * 1024 * 1024;
        let mut enc = encode::Encoder::new(
            out,
            renderer.layout.width,
            renderer.layout.height,
            renderer.mask.clone(),
            opts.int("loop").clamp(0, i64::from(u16::MAX)) as u16,
            max_bytes,
        )?;

        let mut last_cs = 0;
        replay(&tl, cols, rows, fps, progress, true, |snap, cs| {
            last_cs = cs;
            if let Some(rect) = renderer.render(snap) {
                enc.push(renderer.canvas(), rect, cs)?;
            }
            Ok(())
        })?;
        let cur_cs = last_cs;
        let end_cs = Timing::cs(tl.end).max(cur_cs) + Timing::cs(opts.float("end_pause"));
        let frames = enc.finish(end_cs)?;
        progress.report(1.0);

        let mut report = ExportReport {
            lines: frames,
            fallback_cells: renderer.fallback_cells,
            ..Default::default()
        };
        if !renderer.missing.is_empty() {
            report
                .dropped
                .insert("fallback glyph".into(), renderer.missing.len() as u64);
            let list: String = renderer.missing.iter().take(12).collect();
            report.warnings.push(format!(
                "{} cell(s) per frame at most drawn as □ (no bundled glyph for: {list}); \
                 ligatures, colour emoji and images are not rendered",
                renderer.fallback_cells
            ));
        }
        Ok(report)
    }
}

/// Smallest cropped window: enough room for the window bar and a short prompt.
const MIN_COLS: usize = 40;
const MIN_ROWS: usize = 3;

/// Columns and rows (counted from the top-left) that hold anything visible: text, a coloured
/// background, an underline or the cursor.
fn content_extent(snap: &mt_term::RenderSnapshot, cursor: bool) -> (usize, usize) {
    let (mut cols, mut rows) = (0, 0);
    for r in 0..snap.rows {
        let row = &snap.cells[r * snap.cols..(r + 1) * snap.cols];
        if let Some(c) = row.iter().rposition(|c| !c.is_blank()) {
            let wide = row[c].flags.has(mt_term::CellFlags::WIDE);
            cols = cols.max(c + 1 + usize::from(wide));
            rows = rows.max(r + 1);
        }
    }
    if cursor && snap.cursor.visible && snap.display_offset == 0 {
        cols = cols.max(snap.cursor.col + 1);
        rows = rows.max(snap.cursor.row + 1);
    }
    (cols.min(snap.cols), rows.min(snap.rows))
}

/// Feeds the timeline to a headless terminal and calls `on_frame` with the screen at every
/// frame boundary (and once at the end). `report` drives progress; the extent pass stays quiet.
fn replay(
    tl: &schedule::Timeline,
    cols: usize,
    rows: usize,
    fps: u64,
    progress: &dyn Progress,
    report: bool,
    mut on_frame: impl FnMut(&mt_term::RenderSnapshot, u64) -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    let cs_of = |tick: u64| (tick * 100 + fps / 2) / fps;
    let mut emu = Emulator::new(cols, rows, 0);
    let mut cur = 0u64;
    let total = tl.steps.len().max(1);
    for (i, step) in tl.steps.iter().enumerate() {
        if i % 64 == 0 {
            if progress.cancelled() {
                return Err(ExportError::Cancelled);
            }
            if report {
                progress.report(i as f32 / total as f32);
            }
            emu.drain_events();
        }
        if let Some(t) = step.tick {
            if t > cur {
                // A frame is the expensive part: report and honour Cancel for each one.
                if progress.cancelled() {
                    return Err(ExportError::Cancelled);
                }
                if report {
                    progress.report(i as f32 / total as f32);
                }
                on_frame(&emu.snapshot(), cs_of(cur))?;
                cur = t;
            }
        }
        match step.kind {
            EventKind::Output(s) => emu.advance(s.as_bytes()),
            EventKind::Resize { cols, rows } => {
                emu.resize(usize::from(*cols).max(1), usize::from(*rows).max(1))
            }
            _ => {}
        }
    }
    on_frame(&emu.snapshot(), cs_of(cur))
}

#[cfg(test)]
mod tests;
