use super::*;
use mt_term::Emulator;

fn hl(line: u64, span: Option<(usize, usize)>) -> Highlight {
    Highlight {
        line,
        span,
        expect: None,
        color: "red".into(),
        style: Style::Background,
        expires_ms: None,
        rule: "r".into(),
    }
}

fn snap(e: &mut Emulator) -> RenderSnapshot {
    e.snapshot()
}

#[test]
fn match_and_line_rects_on_a_plain_row() {
    let mut e = Emulator::new(20, 4, 100);
    e.advance(b"say error: boom now\r\n");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    d.add_highlight(1, hl(s.line_base, Some((4, 15))), 10);
    d.add_highlight(1, hl(s.line_base, None), 10);
    let r = d.rects(1, &s, 0);
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].row, r[0].start, r[0].end), (0, 4, 15));
    assert_eq!(
        (r[1].row, r[1].start, r[1].end),
        (0, 0, 19),
        "the line, up to its last glyph"
    );
}

#[test]
fn a_wrapped_line_maps_offsets_across_rows() {
    let mut e = Emulator::new(10, 5, 100);
    e.advance(b"abcdefghijKLMNOPQRSTuvw\r\n");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    d.add_highlight(1, hl(s.line_base, Some((10, 20))), 10);
    let r = d.rects(1, &s, 0);
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].row, r[0].start, r[0].end), (1, 0, 10));
    d.add_highlight(1, hl(s.line_base, Some((8, 13))), 10);
    let r = d.rects(1, &s, 0);
    let spans: Vec<_> = r.iter().skip(1).map(|x| (x.row, x.start, x.end)).collect();
    assert_eq!(spans, vec![(0, 8, 10), (1, 0, 3)]);
}

#[test]
fn wide_characters_take_two_cells_and_count_once() {
    let mut e = Emulator::new(20, 3, 100);
    e.advance("a漢字b\r\n".as_bytes());
    let s = snap(&mut e);
    let mut d = Decorations::default();
    // Characters 1..3 are the two wide glyphs: cells 1..5.
    d.add_highlight(1, hl(s.line_base, Some((1, 3))), 10);
    let r = d.rects(1, &s, 0);
    assert_eq!((r[0].start, r[0].end), (1, 5));
}

#[test]
fn highlights_follow_their_line_when_it_scrolls_and_leave_with_the_scrollback() {
    let mut e = Emulator::new(20, 3, 6);
    e.advance(b"target error\r\n");
    let id = snap(&mut e).line_base;
    let mut d = Decorations::default();
    d.add_highlight(1, hl(id, None), 10);
    d.add_marker(
        1,
        Marker {
            line: id,
            label: "m".into(),
            color: "red".into(),
            rule: "r".into(),
        },
        10,
    );
    for i in 0..3 {
        e.advance(format!("filler {i}\r\n").as_bytes());
    }
    let s = snap(&mut e);
    assert_eq!(s.history_size + s.rows - s.rows, s.history_size);
    // The line scrolled into history: not in the viewport, but kept.
    assert!(d.rects(1, &s, 0).is_empty());
    assert!(!d.prune(1, s.line_base, 0));
    assert_eq!(d.highlight_count(1), 1);
    // Far more output trims it out of the scrollback: the decoration goes with it.
    for i in 0..40 {
        e.advance(format!("more {i}\r\n").as_bytes());
    }
    let s = snap(&mut e);
    assert!(s.line_base > id);
    assert!(d.prune(1, s.line_base, 0));
    assert_eq!((d.highlight_count(1), d.markers(1).len()), (0, 0));
}

#[test]
fn scrolled_back_viewport_still_finds_the_line() {
    let mut e = Emulator::new(20, 3, 100);
    e.advance(b"first\r\nneedle\r\nthird\r\nfourth\r\nfifth\r\n");
    let id = {
        let s = snap(&mut e);
        s.line_base + 1
    };
    let mut d = Decorations::default();
    d.add_highlight(1, hl(id, Some((0, 6))), 10);
    e.scroll_display(mt_term::ScrollCmd::Top);
    let s = snap(&mut e);
    let r = d.rects(1, &s, 0);
    assert_eq!(
        r.len(),
        1,
        "history {} offset {}",
        s.history_size,
        s.display_offset
    );
    assert_eq!(s.row_text(r[0].row), "needle");
}

#[test]
fn alternate_screen_draws_nothing() {
    let mut e = Emulator::new(20, 3, 100);
    e.advance(b"x\r\n\x1b[?1049h");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    d.add_highlight(1, hl(s.line_base, None), 10);
    assert!(d.rects(1, &s, 0).is_empty());
}

#[test]
fn eviction_dedupe_and_ttl() {
    let mut d = Decorations::default();
    for i in 0..5 {
        d.add_highlight(1, hl(i, None), 3);
    }
    assert_eq!(d.highlight_count(1), 3);
    // The same line, span and rule replaces instead of piling up.
    d.add_highlight(1, hl(4, None), 3);
    assert_eq!(d.highlight_count(1), 3);
    let mut t = hl(9, None);
    t.expires_ms = Some(100);
    d.add_highlight(2, t, 3);
    assert!(!d.prune(2, 0, 99));
    assert!(d.prune(2, 0, 100));
    assert_eq!(d.highlight_count(2), 0);
}

#[test]
fn flash_lasts_1200_ms() {
    let mut e = Emulator::new(20, 3, 100);
    e.advance(b"found\r\n");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    d.flash(1, s.line_base, 1000, None);
    let r = d.rects(1, &s, 1599);
    assert_eq!(r.len(), 1);
    assert!(r[0].flash);
    assert!(d.rects(1, &s, 2300).is_empty());
    assert!(d.prune(1, 0, 2300));
}

#[test]
fn markers_ticks_edge_marks_and_stepping() {
    let mut e = Emulator::new(20, 4, 100);
    for i in 0..30 {
        e.advance(format!("row {i}\r\n").as_bytes());
    }
    let s = snap(&mut e);
    let base = s.line_base;
    let mut d = Decorations::default();
    for line in [3u64, 12, 20] {
        d.add_marker(
            1,
            Marker {
                line: base + line,
                label: format!("m{line}"),
                color: "red".into(),
                rule: "r".into(),
            },
            100,
        );
    }
    let ticks = d.ticks(1, &s);
    assert_eq!(ticks.len(), 3);
    let total = (s.history_size + s.rows) as f32;
    assert!((ticks[1].0 - 12.0 / total).abs() < 1e-6);
    // The viewport shows the last 4 rows; none of the markers is on screen.
    assert!(d.edge_marks(1, &s).is_empty());
    // Previous from the bottom goes to the nearest marker above, then further up.
    assert_eq!(d.step_marker(1, View::of(&s), false), Some(base + 20));
    let mut e2 = e;
    e2.scroll_display(mt_term::ScrollCmd::Top);
    let s2 = e2.snapshot();
    // From the top, Next walks the markers in order and stops at the last one. The jump target
    // is the reference for the next press while it is still on screen; otherwise the viewport's
    // middle row is.
    assert_eq!(d.step_marker(1, View::of(&s2), true), Some(base + 3));
    assert_eq!(
        d.step_marker(1, View::of(&s2), true),
        Some(base + 12),
        "3 is on screen, so continue past it"
    );
    // 12 is off screen at the top, so the reference falls back to the middle row again.
    assert_eq!(d.step_marker(1, View::of(&s2), true), Some(base + 3));
    assert_eq!(
        d.step_marker(1, View::of(&s2), false),
        None,
        "nothing above the top marker"
    );
    d.clear_markers(1);
    assert!(d.markers(1).is_empty());
    assert_eq!(d.step_marker(1, View::of(&s2), true), None);
}

#[test]
fn edge_marks_cover_the_rows_of_a_visible_marked_line() {
    let mut e = Emulator::new(10, 5, 100);
    e.advance(b"abcdefghijklmnopqrst\r\nx");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    d.add_marker(
        1,
        Marker {
            line: s.line_base,
            label: "m".into(),
            color: "info".into(),
            rule: "r".into(),
        },
        10,
    );
    let rows: Vec<usize> = d.edge_marks(1, &s).into_iter().map(|(r, _)| r).collect();
    assert_eq!(rows, vec![0, 1]);
}

#[test]
fn markers_are_per_line_and_rule_and_capped() {
    let mut d = Decorations::default();
    let m = |line, rule: &str| Marker {
        line,
        label: String::new(),
        color: String::new(),
        rule: rule.into(),
    };
    d.add_marker(1, m(5, "a"), 3);
    d.add_marker(1, m(5, "a"), 3);
    d.add_marker(1, m(5, "b"), 3);
    assert_eq!(d.markers(1).len(), 2);
    d.add_marker(1, m(6, "a"), 3);
    d.add_marker(1, m(7, "a"), 3);
    assert_eq!(d.markers(1).len(), 3, "the oldest is dropped");
}

#[test]
fn the_editor_preview_is_a_transient_decoration_of_its_own() {
    let mut e = Emulator::new(20, 3, 100);
    e.advance(b"say error here\r\n");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    assert!(!d.has_any(1));
    d.set_preview(1, vec![(s.line_base, 4, 9)]);
    assert!(d.has_any(1));
    let r = d.rects(1, &s, 0);
    assert_eq!(
        (r.len(), r[0].start, r[0].end, r[0].flash),
        (1, 4, 9, false)
    );
    d.clear_preview();
    assert!(!d.has_any(1) && d.rects(1, &s, 0).is_empty());
    d.set_preview(1, vec![(s.line_base, 0, 1)]);
    d.set_preview(1, vec![]);
    assert!(!d.has_any(1));
}

#[test]
fn a_highlight_is_drawn_only_while_its_text_is_still_there() {
    let mut e = Emulator::new(30, 3, 100);
    e.advance(b"echo 'error: build failed'");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    let mut h = hl(s.line_base, Some((6, 25)));
    h.expect = Some("error: build failed".to_string());
    d.add_highlight(1, h.clone(), 10);
    assert_eq!(d.rects(1, &s, 0).len(), 1, "text unchanged");
    // The line is cleared and retyped: the old highlight must not linger on the blank row.
    e.advance(b"\r\x1b[2Kecho other");
    assert!(d.rects(1, &snap(&mut e), 0).is_empty());
    // Whole-line form.
    let mut e = Emulator::new(30, 3, 100);
    e.advance(b"abc def");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    let mut h = hl(s.line_base, None);
    h.expect = Some("abc def".into());
    d.add_highlight(1, h, 10);
    assert_eq!(d.rects(1, &s, 0).len(), 1);
    e.advance(b"X");
    assert!(d.rects(1, &snap(&mut e), 0).is_empty());
}

fn program(line: u64, note: &str) -> Marker {
    Marker {
        line,
        label: note.into(),
        color: "error".into(),
        rule: PROGRAM_RULE.into(),
    }
}

#[test]
fn program_marks_share_the_marker_list_without_touching_trigger_markers() {
    let mut e = Emulator::new(20, 4, 100);
    e.advance(b"a\r\nb\r\nc\r\n");
    let s = snap(&mut e);
    let mut d = Decorations::default();
    let trig = Marker {
        line: s.line_base + 1,
        label: "t".into(),
        color: "red".into(),
        rule: "r".into(),
    };
    d.add_marker(1, trig.clone(), 10);
    d.set_program_markers(1, vec![program(s.line_base + 3, "p")]);
    let all = d.markers(1);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], trig);
    assert_eq!(
        d.step_marker(1, View::of(&s), true),
        Some(s.line_base + 3),
        "program marks are visited"
    );
    // Replacing the program marks keeps the trigger marker; an empty list removes only them.
    d.set_program_markers(1, vec![program(s.line_base, "q")]);
    assert_eq!(d.markers(1).len(), 2);
    d.set_program_markers(1, Vec::new());
    assert_eq!(d.markers(1), vec![trig]);
    assert_eq!(d.ticks(1, &s).len(), 1);
}
