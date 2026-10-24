use std::path::Path;

use mt_record::{Header, Writer};

use super::*;
use crate::{default_options, export_to_path, find, NoProgress};

/// A cast of `cols`x`rows` built by `build`.
fn recording(
    dir: &Path,
    cols: u16,
    rows: u16,
    build: impl FnOnce(&mut Writer<std::fs::File>),
) -> Recording {
    let path = dir.join("r.cast");
    let mut w = Writer::new(
        std::fs::File::create(&path).unwrap(),
        &Header::new(cols, rows),
    )
    .unwrap();
    build(&mut w);
    w.flush().unwrap();
    let sidecar = mt_record::Sidecar {
        id: "r".into(),
        title: "demo".into(),
        ..Default::default()
    };
    Recording::new(sidecar, path)
}

fn opts(kv: &[(&str, &str)]) -> Options {
    let ex = find("gif").unwrap();
    let mut o = default_options(ex, &Default::default());
    let kv: Vec<_> = kv
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    o.apply(&ex.options(), &kv).unwrap();
    o
}

fn export(rec: &Recording, o: &Options) -> (Vec<u8>, ExportReport) {
    let mut out = Vec::new();
    let r = Gif.export(rec, o, &mut out, &NoProgress).unwrap();
    (out, r)
}

struct Decoded {
    width: usize,
    height: usize,
    /// Delay in centiseconds and the composed RGB screen after each frame.
    frames: Vec<(u16, Vec<u8>)>,
    repeat: gif::Repeat,
}

/// Decodes and composes delta frames the way a browser does (dispose = keep).
fn decode(bytes: &[u8]) -> Decoded {
    let mut o = gif::DecodeOptions::new();
    o.set_color_output(gif::ColorOutput::Indexed);
    let mut d = o.read_info(bytes).unwrap();
    let (w, h) = (d.width() as usize, d.height() as usize);
    let mut screen = vec![0u8; w * h * 3];
    let mut frames = Vec::new();
    while let Some(f) = d.read_next_frame().unwrap() {
        assert_eq!(f.dispose, gif::DisposalMethod::Keep);
        let pal = f.palette.as_ref().expect("local palette");
        for y in 0..f.height as usize {
            for x in 0..f.width as usize {
                let idx = f.buffer[y * f.width as usize + x];
                if Some(idx) == f.transparent {
                    continue;
                }
                let i = ((f.top as usize + y) * w + f.left as usize + x) * 3;
                let p = usize::from(idx) * 3;
                screen[i..i + 3].copy_from_slice(&pal[p..p + 3]);
            }
        }
        frames.push((f.delay, screen.clone()));
    }
    Decoded {
        width: w,
        height: h,
        frames,
        repeat: d.repeat(),
    }
}

/// What the renderer draws for `bytes` on a fresh `cols`x`rows` screen with default options.
fn render(bytes: &[u8], cols: usize, rows: usize, o: &Options) -> Canvas {
    let reg = Registry::bundled();
    let mut r = Renderer::new(Params {
        theme: reg.resolve("", true),
        font_size: o.int("font_size") as f32,
        line_height: o.float("line_height") as f32,
        scale: o.float("scale") as f32,
        padding: o.int("padding") as f32,
        margin: o.int("margin") as f32,
        margin_fill: None,
        shadow: o.bool("shadow"),
        chrome: Chrome::parse(o.text("chrome")),
        title: "demo".into(),
        cursor: o.bool("cursor"),
        cols,
        rows,
    });
    let mut emu = Emulator::new(cols, rows, 0);
    emu.advance(bytes);
    r.render(&emu.snapshot());
    r.canvas().clone()
}

use raster::Canvas;

fn theme() -> mt_theme::Theme {
    Registry::bundled().resolve("", true).clone()
}

fn has_color(screen: &[u8], c: [u8; 3]) -> bool {
    screen.chunks(3).any(|p| p == c)
}

#[test]
fn delta_frames_compose_to_full_renders() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 20, 4, |w| {
        w.output(0.0, b"$ ").unwrap();
        w.output(0.5, b"ls\r\n").unwrap();
        w.output(1.0, b"\x1b[1mbold\x1b[0m \x1b[4munder\x1b[0m\r\n$ ")
            .unwrap();
    });
    let o = opts(&[("chrome", "none"), ("crop", "false")]);
    let (bytes, report) = export(&rec, &o);
    let g = decode(&bytes);
    assert_eq!(g.frames.len(), 3);
    assert_eq!(report.lines, 3);
    assert_eq!(g.repeat, gif::Repeat::Infinite);
    let steps: [&[u8]; 3] = [
        b"$ ",
        b"$ ls\r\n",
        b"$ ls\r\n\x1b[1mbold\x1b[0m \x1b[4munder\x1b[0m\r\n$ ",
    ];
    for (i, s) in steps.iter().enumerate() {
        let want = render(s, 20, 4, &o);
        assert_eq!((g.width, g.height), (want.width, want.height));
        assert!(
            g.frames[i].1 == want.rgb,
            "frame {i} differs from a full render"
        );
    }
    // 0.5 s per screen, then the last screen for 0 s + end_pause 2 s.
    let delays: Vec<_> = g.frames.iter().map(|f| f.0).collect();
    assert_eq!(delays, [50, 50, 200]);
}

#[test]
fn colours_attributes_and_cursor() {
    let d = tempfile::tempdir().unwrap();
    let t = theme();
    let rec = recording(d.path(), 12, 2, |w| {
        w.output(
            0.0,
            b"\x1b[31mred\x1b[0m \x1b[48;2;1;2;3m  \x1b[0m \x1b[7mrv\x1b[0m\x1b[48;5;196m #\x1b[0m",
        )
        .unwrap();
    });
    let (bytes, _) = export(&rec, &opts(&[("chrome", "none")]));
    let last = &decode(&bytes).frames[0].1;
    let ansi_red = [t.ansi[1].r, t.ansi[1].g, t.ansi[1].b];
    assert!(has_color(last, ansi_red), "ANSI red");
    assert!(has_color(last, [1, 2, 3]), "truecolor background");
    assert!(has_color(last, [255, 0, 0]), "256-colour 196");
    let fg = [t.foreground.r, t.foreground.g, t.foreground.b];
    assert!(
        has_color(last, fg),
        "reverse video paints the foreground as background"
    );
    assert!(
        has_color(last, [t.cursor.r, t.cursor.g, t.cursor.b]),
        "block cursor"
    );

    let (no_cursor, _) = export(&rec, &opts(&[("chrome", "none"), ("cursor", "false")]));
    assert_ne!(bytes, no_cursor);
}

#[test]
fn bold_and_italic_use_their_faces() {
    let o = opts(&[("chrome", "none"), ("cursor", "false")]);
    let plain = render(b"abc", 3, 1, &o);
    let bold = render(b"\x1b[1mabc", 3, 1, &o);
    let italic = render(b"\x1b[3mabc", 3, 1, &o);
    let strike = render(b"\x1b[9mabc", 3, 1, &o);
    assert!(plain.rgb != bold.rgb && plain.rgb != italic.rgb && bold.rgb != italic.rgb);
    assert!(plain.rgb != strike.rgb);
}

#[test]
fn wide_chars_span_two_cells_and_count_as_fallback() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 6, 1, |w| w.output(0.0, "日x".as_bytes()).unwrap());
    let o = opts(&[
        ("chrome", "none"),
        ("cursor", "false"),
        ("padding", "0"),
        ("margin", "0"),
    ]);
    let (bytes, report) = export(&rec, &o);
    assert_eq!(report.fallback_cells, 1);
    assert_eq!(report.dropped.get("fallback glyph"), Some(&1));
    let g = decode(&bytes);
    let t = theme();
    let bg = [t.background.r, t.background.g, t.background.b];
    let cell_w = g.width / 6;
    let screen = &g.frames[0].1;
    // The fallback box reaches into the second (spacer) cell.
    let second_cell_inked = (0..g.height)
        .any(|y| (cell_w + 1..2 * cell_w - 1).any(|x| screen[(y * g.width + x) * 3..][..3] != bg));
    assert!(second_cell_inked);
}

#[test]
fn box_drawing_is_geometric_and_joins() {
    let o = opts(&[
        ("chrome", "none"),
        ("cursor", "false"),
        ("padding", "0"),
        ("margin", "0"),
    ]);
    let c = render("──".as_bytes(), 2, 1, &o);
    let t = theme();
    let fg = [t.foreground.r, t.foreground.g, t.foreground.b];
    // Some row is solid foreground across both cells.
    let solid =
        (0..c.height).any(|y| (0..c.width).all(|x| c.rgb[(y * c.width + x) * 3..][..3] == fg));
    assert!(solid);
}

#[test]
fn resize_grows_the_canvas_to_the_largest_grid() {
    let d = tempfile::tempdir().unwrap();
    let full = opts(&[("crop", "false")]);
    let small = recording(d.path(), 10, 3, |w| w.output(0.0, b"a").unwrap());
    let (s, _) = export(&small, &full);
    let d2 = tempfile::tempdir().unwrap();
    let grown = recording(d2.path(), 10, 3, |w| {
        w.output(0.0, b"a").unwrap();
        w.resize(0.5, 20, 6).unwrap();
        w.output(0.6, b"b").unwrap();
    });
    let (g, _) = export(&grown, &full);
    let (s, g) = (decode(&s), decode(&g));
    assert!(g.width > s.width && g.height > s.height);
    // "a", then the resize and "b" (the resize may get its own frame).
    assert!(g.frames.len() >= 2);
}

#[test]
fn alt_screen_returns_to_the_primary_screen() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 10, 3, |w| {
        w.output(0.0, b"$ ").unwrap();
        w.output(0.5, b"\x1b[?1049h\x1b[2JVIM").unwrap();
        w.output(1.0, b"\x1b[?1049l").unwrap();
    });
    // No backdrop or title bar: they push the first frame past 255 colours (lossy), which is
    // not what this test is about.
    let (bytes, _) = export(&rec, &opts(&[("margin", "0"), ("chrome", "none")]));
    let g = decode(&bytes);
    assert_eq!(g.frames.len(), 3);
    assert!(g.frames[0].1 == g.frames[2].1);
    assert!(g.frames[0].1 != g.frames[1].1);
}

#[test]
fn idle_limit_and_speed_set_delays() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 10, 2, |w| {
        w.output(0.0, b"a").unwrap();
        w.output(30.0, b"b").unwrap();
    });
    let delays = |kv: &[(&str, &str)]| -> Vec<u16> {
        decode(&export(&rec, &opts(kv)).0)
            .frames
            .iter()
            .map(|f| f.0)
            .collect()
    };
    assert_eq!(delays(&[("end_pause", "1")]), [200, 100]);
    assert_eq!(delays(&[("end_pause", "1"), ("speed", "2")]), [100, 100]);
    assert_eq!(
        delays(&[("end_pause", "0"), ("idle_limit", "0")]),
        [3000, 2]
    );
}

#[test]
fn unchanged_screens_emit_no_frame_and_loop_count_is_written() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 10, 2, |w| {
        w.output(0.0, b"a").unwrap();
        w.output(0.5, b"\x1b[0m").unwrap();
        w.output(1.0, b"b").unwrap();
    });
    let (bytes, _) = export(&rec, &opts(&[("loop", "3")]));
    let g = decode(&bytes);
    assert_eq!(g.frames.iter().map(|f| f.0).collect::<Vec<_>>(), [100, 200]);
    assert_eq!(g.repeat, gif::Repeat::Finite(2));
}

#[test]
fn smaller_fps_and_scale_make_a_smaller_file() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 40, 10, |w| {
        for i in 0..100 {
            w.output(f64::from(i) * 0.05, format!("line {i}\r\n").as_bytes())
                .unwrap();
        }
    });
    let (big, _) = export(&rec, &opts(&[]));
    let (small, _) = export(&rec, &opts(&[("fps", "5"), ("scale", "0.5")]));
    assert!(
        small.len() < big.len() / 2,
        "{} vs {}",
        small.len(),
        big.len()
    );
}

#[test]
fn output_is_deterministic() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 20, 4, |w| {
        w.output(0.0, b"\x1b[32mok\x1b[0m \xe2\x94\x8c\xe2\x94\x80\r\n")
            .unwrap();
        w.output(0.3, b"\x1b[3mdone").unwrap();
    });
    let o = opts(&[("chrome", "macos")]);
    assert_eq!(export(&rec, &o).0, export(&rec, &o).0);
}

#[test]
fn golden_hash_of_a_small_recording() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 16, 3, |w| {
        w.output(0.0, b"$ ").unwrap();
        w.output(0.4, b"echo \x1b[1mhi\x1b[0m\r\nhi\r\n$ ").unwrap();
    });
    let (bytes, _) = export(&rec, &opts(&[]));
    // FNV-1a: any change to fonts, rasteriser or encoder shows up here. Update deliberately.
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3)
    });
    assert_eq!(format!("{hash:016x}"), GOLDEN, "{} bytes", bytes.len());
}

const GOLDEN: &str = "d38e5ad647aa7427";

#[test]
fn max_size_aborts_and_leaves_no_file() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 80, 24, |w| {
        let mut seed = 1u32;
        for f in 0..300 {
            let mut s = String::from("\x1b[H");
            for _ in 0..80 * 24 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let (r, g, b) = (seed >> 24, (seed >> 16) & 255, (seed >> 8) & 255);
                s.push_str(&format!("\x1b[48;2;{r};{g};{b}m "));
            }
            w.output(f64::from(f) * 0.1, s.as_bytes()).unwrap();
        }
    });
    let ex = find("gif").unwrap();
    let out = d.path().join("big.gif");
    let e = export_to_path(
        ex,
        &rec,
        &opts(&[("max_size_mb", "1")]),
        &out,
        false,
        &NoProgress,
    )
    .unwrap_err();
    assert!(
        matches!(&e, ExportError::NotPossible(m) if m.contains("max_size_mb")),
        "{e}"
    );
    assert!(!out.exists() && !d.path().join("big.gif.partial").exists());
}

#[test]
fn unknown_theme_is_a_bad_option() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 4, 1, |w| w.output(0.0, b"a").unwrap());
    let mut out = Vec::new();
    let e = Gif
        .export(
            &rec,
            &opts(&[("theme", "No Such Theme")]),
            &mut out,
            &NoProgress,
        )
        .unwrap_err();
    assert!(matches!(e, ExportError::BadOption(_)));
}

/// `make bench`: a 60 s 80x24 shell session at 10 fps (SPEC budget: < 10 s on an M1,
/// < 5 MB). Run with `cargo test --release -p mt_export gif_bench -- --ignored --nocapture`;
/// `MT_GIF_BENCH_OUT=path` keeps the GIF.
#[test]
#[ignore]
fn gif_bench() {
    let d = tempfile::tempdir().unwrap();
    // Twenty commands: typed key by key, then a burst of output that scrolls the screen.
    let rec = recording(d.path(), 80, 24, |w| {
        let mut t = 0.0;
        for i in 0..20 {
            w.output(t, b"\x1b[32m~/src\x1b[0m $ ").unwrap();
            for ch in "cargo build".chars() {
                t += 0.1;
                w.output(t, ch.to_string().as_bytes()).unwrap();
            }
            t += 0.3;
            let mut burst = String::from("\r\n");
            for j in 0..8 {
                burst.push_str(&format!(
                    "   \x1b[1;32mCompiling\x1b[0m crate{i}_{j} v0.1.0 (\x1b[34m/src/{j}\x1b[0m)\r\n"
                ));
            }
            w.output(t, burst.as_bytes()).unwrap();
            t += 1.6;
        }
    });
    let o = opts(&[]);
    let start = std::time::Instant::now();
    let (bytes, report) = export(&rec, &o);
    let secs = start.elapsed().as_secs_f64();
    println!(
        "gif_bench: 60 s 80x24 @10 fps: {secs:.2} s, {:.2} MB, {} frames",
        bytes.len() as f64 / 1_048_576.0,
        report.lines
    );
    if let Ok(p) = std::env::var("MT_GIF_BENCH_OUT") {
        std::fs::write(p, &bytes).unwrap();
    }
    assert!(bytes.len() < 5 * 1024 * 1024);
}

/// Exports a real cast: `MT_GIF_CAST=path cargo test --release -p mt_export gif_cast -- --ignored --nocapture`
/// (`MT_GIF_OPTS=scale=1,fps=5` overrides options).
#[test]
#[ignore]
fn gif_cast() {
    let Ok(path) = std::env::var("MT_GIF_CAST") else {
        return;
    };
    let sidecar = mt_record::Sidecar {
        id: "r".into(),
        title: "real".into(),
        ..Default::default()
    };
    let rec = Recording::new(sidecar, path);
    let kv: Vec<(&str, &str)> = std::env::var("MT_GIF_OPTS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                Box::leak(k.to_string().into_boxed_str()) as &str,
                Box::leak(v.to_string().into_boxed_str()) as &str,
            )
        })
        .collect();
    let start = std::time::Instant::now();
    let (bytes, report) = export(&rec, &opts(&kv));
    let secs = start.elapsed().as_secs_f64();
    if let Ok(out) = std::env::var("MT_GIF_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let img = decode(&bytes);
    println!(
        "gif_cast: {secs:.2} s, {:.2} MB, {} frames, {}x{} px",
        bytes.len() as f64 / 1_048_576.0,
        report.lines,
        img.width,
        img.height
    );
}

#[test]
fn incremental_rendering_equals_drawing_from_scratch() {
    let o = opts(&[]);
    let reg = Registry::bundled();
    let params = || Params {
        theme: reg.resolve("", true),
        font_size: 12.0,
        line_height: 1.1,
        scale: 1.0,
        padding: 8.0,
        margin: 10.0,
        margin_fill: None,
        shadow: true,
        chrome: Chrome::Macos,
        title: "inc".into(),
        cursor: o.bool("cursor"),
        cols: 20,
        rows: 6,
    };
    type Step = (&'static [u8], Option<(usize, usize)>);
    let steps: [Step; 7] = [
        (b"$ ls\r\n\x1b[1;31mred\x1b[0m \x1b[3mital\x1b[0m", None),
        (b"\r\nmore\r\n", None),
        (b"\x1b[2;3H\x1b[4 q", None),
        (b"\x1b[6;1Hbottom\r\nscroll\r\nscroll2", None),
        (b"x", Some((10, 3))),
        (b"\x1b[?25l", None),
        (
            b"\x1b[?25h\x1b[H\x1b[2J\xe2\x94\x8c\xe2\x94\x80\xe2\x94\x90",
            Some((20, 6)),
        ),
    ];
    let mut inc = Renderer::new(params());
    let mut emu = Emulator::new(20, 6, 0);
    let mut bytes_so_far: Vec<u8> = Vec::new();
    let mut dims = (20usize, 6usize);
    for (bytes, resize) in steps {
        emu.advance(bytes);
        bytes_so_far.extend_from_slice(bytes);
        if let Some((c, r)) = resize {
            emu.resize(c, r);
            dims = (c, r);
        }
        let snap = emu.snapshot();
        let rect = inc.render(&snap);
        // A fresh renderer given the same snapshot draws the identical screen.
        let mut fresh = Renderer::new(params());
        fresh.render(&snap);
        assert!(
            inc.canvas() == fresh.canvas(),
            "after {bytes:?} at {dims:?}"
        );
        // Rendering the same snapshot again changes nothing.
        assert_eq!(inc.render(&snap), None);
        if let Some(r) = rect {
            assert!(r.y1 <= inc.canvas().height && r.x1 <= inc.canvas().width);
        }
    }
}

#[test]
fn a_changed_row_dirties_only_its_band() {
    let reg = Registry::bundled();
    let mut r = Renderer::new(Params {
        theme: reg.resolve("", true),
        font_size: 12.0,
        line_height: 1.1,
        scale: 1.0,
        padding: 8.0,
        margin: 0.0,
        margin_fill: None,
        shadow: false,
        chrome: Chrome::None,
        title: String::new(),
        cursor: false,
        cols: 10,
        rows: 5,
    });
    let mut emu = Emulator::new(10, 5, 0);
    let full = r.render(&emu.snapshot()).unwrap();
    assert_eq!((full.x0, full.y0), (0, 0));
    assert_eq!((full.x1, full.y1), (r.canvas().width, r.canvas().height));
    emu.advance(b"\x1b[3;1Hhi");
    let rect = r.render(&emu.snapshot()).unwrap();
    let ch = (r.canvas().height - 16) / 5;
    assert_eq!((rect.y0, rect.y1), (8 + 2 * ch, 8 + 3 * ch));
}

#[test]
fn title_option_replaces_the_recording_name_in_the_title_bar() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 20, 2, |w| w.output(0.0, b"x").unwrap());
    let default = export(&rec, &opts(&[])).0;
    // An explicit title equal to the recording's name is the same as the default.
    assert_eq!(default, export(&rec, &opts(&[("title", "demo")])).0);
    assert_ne!(default, export(&rec, &opts(&[("title", "Release 1.0")])).0);
    // Blank means the recording's name; chrome=none has no title bar to show it in.
    assert_eq!(default, export(&rec, &opts(&[("title", "  ")])).0);
    let none = |t: &str| export(&rec, &opts(&[("chrome", "none"), ("title", t)])).0;
    assert_eq!(none("a"), none("b"));
}

#[test]
fn colour_emoji_are_drawn_in_colour_and_are_not_fallback_cells() {
    let d = tempfile::tempdir().unwrap();
    let rec = recording(d.path(), 20, 2, |w| {
        w.output(0.0, "[📦 v0.1][🦀 v1.89]".as_bytes()).unwrap();
    });
    let o = opts(&[("chrome", "none"), ("cursor", "false"), ("margin", "0")]);
    let (bytes, report) = export(&rec, &o);
    assert_eq!(report.fallback_cells, 0, "{:?}", report.warnings);
    assert!(report.dropped.is_empty());
    let g = decode(&bytes);
    let t = theme();
    let known: Vec<[u8; 3]> = std::iter::once([t.background.r, t.background.g, t.background.b])
        .chain(std::iter::once([
            t.foreground.r,
            t.foreground.g,
            t.foreground.b,
        ]))
        .chain(t.ansi.iter().map(|c| [c.r, c.g, c.b]))
        .collect();
    // Many pixels use colours that are not in the theme: the emoji bitmaps.
    let foreign = g.frames[0]
        .1
        .chunks(3)
        .filter(|p| !known.iter().any(|k| k == *p) && (p[0] > 120 || p[1] > 80))
        .count();
    assert!(foreign > 200, "{foreign}");
    // The crab is red-orange and the box is brown: both present.
    assert!(g.frames[0]
        .1
        .chunks(3)
        .any(|p| p[0] > 180 && p[1] < 90 && p[2] < 90));
    // CJK still has no glyph in the bundled fonts.
    let rec = recording(d.path(), 6, 1, |w| w.output(0.0, "日".as_bytes()).unwrap());
    assert_eq!(export(&rec, &o).1.fallback_cells, 1);
}

#[test]
fn crop_to_content_fits_the_used_cells_and_makes_text_fill_the_picture() {
    let d = tempfile::tempdir().unwrap();
    // A 120x40 window that only ever shows a short prompt.
    let rec = recording(d.path(), 120, 40, |w| {
        w.output(0.0, b"user@host:~$ ls\r\nfile.txt\r\n$ ").unwrap();
    });
    let size = |kv: &[(&str, &str)]| {
        let g = decode(&export(&rec, &opts(kv)).0);
        (g.width, g.height)
    };
    let base = [("chrome", "none"), ("margin", "0"), ("padding", "0")];
    let cropped = size(&base);
    let full = size(&[base.as_slice(), &[("crop", "false")]].concat());
    // Whole window: 120 columns x 40 rows; cropped: the 40-column, 3-row minimum.
    assert!(
        cropped.0 * 2 < full.0 && cropped.1 * 5 < full.1,
        "{cropped:?} vs {full:?}"
    );
    let cell_w = full.0 / 120;
    assert_eq!(cropped.0, 40 * cell_w);
    // The font size now changes the picture, and the grid shown stays the same.
    let big = size(&[base.as_slice(), &[("font_size", "32")]].concat());
    assert!(big.0 > cropped.0 * 3 / 2 && big.1 > cropped.1 * 3 / 2);
}

#[test]
fn content_extent_counts_text_backgrounds_wide_cells_and_the_cursor() {
    let mut emu = Emulator::new(60, 10, 0);
    emu.advance(b"abc\r\n\x1b[3;20H\x1b[44m \x1b[0m");
    let (c, r) = content_extent(&emu.snapshot(), false);
    assert_eq!((c, r), (20, 3), "a coloured blank counts");
    emu.advance("\x1b[5;1H日本".as_bytes());
    assert_eq!(content_extent(&emu.snapshot(), false), (20, 5));
    emu.advance(b"\x1b[8;31H");
    assert_eq!(content_extent(&emu.snapshot(), false).1, 5);
    assert_eq!(
        content_extent(&emu.snapshot(), true),
        (31, 8),
        "the cursor counts"
    );
}

#[test]
fn cropping_never_draws_cells_outside_the_visible_region() {
    let d = tempfile::tempdir().unwrap();
    // The content is far wider than the recording's later frames need: crop covers all frames.
    let rec = recording(d.path(), 100, 6, |w| {
        w.output(0.0, b"short").unwrap();
        w.output(1.0, b"\x1b[1;80Hwide-ish").unwrap();
    });
    let g = decode(
        &export(
            &rec,
            &opts(&[("chrome", "none"), ("margin", "0"), ("padding", "0")]),
        )
        .0,
    );
    let full = decode(
        &export(
            &rec,
            &opts(&[
                ("chrome", "none"),
                ("margin", "0"),
                ("padding", "0"),
                ("crop", "false"),
            ]),
        )
        .0,
    );
    // Both frames are visible: the crop spans the widest frame (87 columns of text and the
    // cursor after it), not just the first.
    assert_eq!(g.width, full.width * 88 / 100);
}
