//! VT replay synthesis (SPEC D6): turns a live terminal into a byte stream that rebuilds it.
//!
//! The remote daemon is authoritative; on attach it sends this stream so the client's single
//! code path (parse bytes) reconstructs the screen, scrollback, modes and cursor.

use std::fmt::Write as _;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape as EngineShape, Rgb};

use crate::commands::{CommandTracker, Mark, ReplayMark};
use crate::sgr::Style;
use crate::snapshot::{Cell, CellFlags};

/// Writes a mark the way a shell with integration would; `D` is tagged so that the receiver
/// restores the command without counting it as newly finished.
fn push_mark(out: &mut String, m: &Mark) {
    match m {
        Mark::PromptStart { .. } => out.push_str("\x1b]133;A\x07"),
        Mark::InputStart => out.push_str("\x1b]133;B\x07"),
        Mark::OutputStart => out.push_str("\x1b]133;C\x07"),
        Mark::Finished(code) => {
            let _ = match code {
                Some(c) => write!(out, "\x1b]133;D;{c};replay\x07"),
                None => write!(out, "\x1b]133;D;;replay\x07"),
            };
        }
        Mark::Cwd(p) => push_cwd(out, p),
        Mark::Private {
            cmd:
                crate::private_osc::PrivCmd::Mark(crate::private_osc::MarkCmd::Add {
                    kind, note, ..
                }),
            ..
        } => {
            let _ = write!(
                out,
                "\x1b]7722;mark;add;{};{};replay\x07",
                kind.name(),
                crate::menu_reg::b64_encode(note.as_bytes())
            );
        }
        Mark::Reset
        | Mark::Restored(_)
        | Mark::Vt(_)
        | Mark::Private { .. }
        | Mark::ClearHistory => {}
    }
}

fn push_cwd(out: &mut String, path: &str) {
    out.push_str("\x1b]7;file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out.push('\x07');
}

/// Prints one grid row. `marks` are the marks anchored on it, by column; each is written when
/// the cursor stands on its column.
fn push_row(
    out: &mut String,
    cur: &mut Style,
    row: &[Cell],
    keep_trailing: bool,
    marks: &[&ReplayMark],
) {
    let end = if keep_trailing {
        row.len()
    } else {
        row.iter().rposition(|c| !c.is_blank()).map_or(0, |i| i + 1)
    };
    let mut mi = 0;
    for (i, c) in row[..end].iter().enumerate() {
        while mi < marks.len() && marks[mi].col <= i {
            push_mark(out, &marks[mi].mark);
            mi += 1;
        }
        if c.flags.has(CellFlags::WIDE_SPACER) {
            continue;
        }
        let s = Style::of(c);
        if s != *cur {
            out.push_str(&if s.is_default() {
                "\x1b[0m".to_string()
            } else {
                s.sgr()
            });
            *cur = s;
        }
        out.push(if c.ch == '\0' { ' ' } else { c.ch });
        if let Some(z) = &c.zerowidth {
            out.extend(z.iter());
        }
    }
    // Marks past the printed text: move the cursor there without touching cells.
    let mut at = end;
    for m in &marks[mi..] {
        if m.col > at {
            let _ = write!(out, "\x1b[{}G", m.col + 1);
            at = m.col;
        }
        push_mark(out, &m.mark);
    }
}

fn dec(out: &mut String, mode: u16, on: bool) {
    let _ = write!(out, "\x1b[?{mode}{}", if on { 'h' } else { 'l' });
}

pub(crate) fn synthesize<T>(
    term: &Term<T>,
    title: Option<&str>,
    sgr_pixels: bool,
    max_history: usize,
    tracker: &CommandTracker,
    vt: &crate::vt::VtState,
) -> Vec<u8> {
    let mode = *term.mode();
    let alt = mode.contains(TermMode::ALT_SCREEN);
    let grid = term.grid();
    let rows = grid.screen_lines();
    let hist = if alt { 0 } else { grid.history_size() };
    let first = hist.saturating_sub(max_history);
    // Command marks of the main screen; the hidden main grid behind an alternate screen is
    // not replayed, so neither are its marks.
    let marks = if alt {
        Vec::new()
    } else {
        tracker.replay_marks(first)
    };
    let mut next_mark = 0;

    let mut out = String::new();
    out.push_str("\x1bc"); // RIS: clears screen, scrollback and modes.
    if let Some(t) = title {
        // Control characters in a title would end the OSC early.
        let t: String = t.chars().filter(|c| !c.is_control()).collect();
        let _ = write!(out, "\x1b]2;{t}\x07");
    }
    // Palette overrides (OSC 4 / 10 / 11 / 12).
    let colors = term.colors();
    for i in 0..259usize {
        if let Some(Rgb { r, g, b }) = colors[i] {
            let spec = format!("rgb:{r:02x}/{g:02x}/{b:02x}");
            match i {
                0..=255 => {
                    let _ = write!(out, "\x1b]4;{i};{spec}\x07");
                }
                256 => {
                    let _ = write!(out, "\x1b]10;{spec}\x07");
                }
                257 => {
                    let _ = write!(out, "\x1b]11;{spec}\x07");
                }
                _ => {
                    let _ = write!(out, "\x1b]12;{spec}\x07");
                }
            }
        }
    }
    // Overrides from sequences the engine does not model (OSC 5, 13–19, 21, 22, XTSHIFTESCAPE).
    vt.replay(&mut out);
    if alt {
        out.push_str("\x1b[?1049h");
    }

    // Content. History rows followed by the screen rows; each hard line break is CRLF so the
    // top rows scroll into the new terminal's scrollback.
    let cols = grid.columns();
    let colors = term.colors();
    let line_cells = |line: i32| -> Vec<Cell> {
        (0..cols)
            .map(|c| {
                crate::snapshot::convert_cell(&grid[Point::new(Line(line), Column(c))], colors)
            })
            .collect()
    };
    let mut cur = Style::of(&Cell::default());
    let last_line = rows as i32 - 1;
    // Oldest kept history row up to the last screen row.
    let mut line = -((hist - first) as i32);
    while line <= last_line {
        let cells = line_cells(line);
        let wrapped = cells[cols - 1].flags.has(CellFlags::WRAPLINE);
        let abs = (hist as i32 + line) as usize;
        let from = next_mark;
        while next_mark < marks.len() && marks[next_mark].line <= abs {
            next_mark += 1;
        }
        let row_marks: Vec<&ReplayMark> = marks[from..next_mark].iter().collect();
        push_row(&mut out, &mut cur, &cells, wrapped, &row_marks);
        if line < last_line && !wrapped {
            if !cur.is_default() {
                out.push_str("\x1b[0m");
                cur = Style::of(&Cell::default());
            }
            out.push_str("\r\n");
        }
        line += 1;
    }
    out.push_str("\x1b[0m");
    if let Some(cwd) = tracker.cwd() {
        push_cwd(&mut out, cwd);
    }
    tracker.menus().replay(&mut out);
    tracker.private_status_replay(&mut out);

    // Cursor, then modes (origin mode last: CUP must be absolute).
    let cp = grid.cursor.point;
    let _ = write!(out, "\x1b[{};{}H", cp.line.0.max(0) + 1, cp.column.0 + 1);

    // Pen: the attributes the next printed character will get.
    let t = &grid.cursor.template;
    let pen = Style::of(&crate::snapshot::convert_cell(t, term.colors()));
    if !pen.is_default() {
        out.push_str(&pen.sgr());
    }

    let style = term.cursor_style();
    let n = match (style.shape, style.blinking) {
        (EngineShape::Underline, true) => 3,
        (EngineShape::Underline, false) => 4,
        (EngineShape::Beam, true) => 5,
        (EngineShape::Beam, false) => 6,
        (_, true) => 1,
        (_, false) => 2,
    };
    let _ = write!(out, "\x1b[{n} q");

    dec(&mut out, 1, mode.contains(TermMode::APP_CURSOR));
    if mode.contains(TermMode::APP_KEYPAD) {
        out.push_str("\x1b=");
    }
    dec(&mut out, 7, mode.contains(TermMode::LINE_WRAP));
    dec(&mut out, 25, mode.contains(TermMode::SHOW_CURSOR));
    dec(&mut out, 1004, mode.contains(TermMode::FOCUS_IN_OUT));
    dec(&mut out, 1000, mode.contains(TermMode::MOUSE_REPORT_CLICK));
    dec(&mut out, 1002, mode.contains(TermMode::MOUSE_DRAG));
    dec(&mut out, 1003, mode.contains(TermMode::MOUSE_MOTION));
    dec(&mut out, 1005, mode.contains(TermMode::UTF8_MOUSE));
    dec(&mut out, 1006, mode.contains(TermMode::SGR_MOUSE));
    dec(&mut out, 1016, sgr_pixels);
    dec(&mut out, 1007, mode.contains(TermMode::ALTERNATE_SCROLL));
    dec(&mut out, 2004, mode.contains(TermMode::BRACKETED_PASTE));
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out.push_str("\x1b[20h");
    }
    if mode.contains(TermMode::INSERT) {
        out.push_str("\x1b[4h");
    }
    let mut kitty = 0u8;
    for (m, bit) in [
        (TermMode::DISAMBIGUATE_ESC_CODES, 1),
        (TermMode::REPORT_EVENT_TYPES, 2),
        (TermMode::REPORT_ALTERNATE_KEYS, 4),
        (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
        (TermMode::REPORT_ASSOCIATED_TEXT, 16),
    ] {
        if mode.contains(m) {
            kitty |= bit;
        }
    }
    if kitty != 0 {
        let _ = write!(out, "\x1b[>{kitty}u");
    }
    // Origin mode moves the cursor home, so restore the position again afterwards.
    if mode.contains(TermMode::ORIGIN) {
        out.push_str("\x1b[?6h");
        let _ = write!(out, "\x1b[{};{}H", cp.line.0.max(0) + 1, cp.column.0 + 1);
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use crate::snapshot::Cell;
    use crate::Emulator;

    fn norm(c: &Cell) -> Cell {
        if c.is_blank() {
            Cell::default()
        } else {
            let mut c = c.clone();
            c.flags = crate::CellFlags(c.flags.0 & !crate::CellFlags::WRAPLINE);
            c
        }
    }

    /// Feeds `input` to A, replays A into B and compares everything that is preserved.
    fn round_trip(input: &[u8], cols: usize, rows: usize, scrollback: usize) {
        let mut a = Emulator::new(cols, rows, scrollback);
        a.advance(input);
        let replay = a.replay_bytes(scrollback);
        let mut b = Emulator::new(cols, rows, scrollback);
        b.advance(&replay);

        let alt = a.snapshot().modes.alt_screen;
        assert_eq!(a.snapshot().modes, b.snapshot().modes, "modes");
        assert_eq!(a.snapshot().cursor, b.snapshot().cursor, "cursor");
        assert_eq!(a.cursor_abs(), b.cursor_abs(), "cursor abs");
        assert_eq!(a.title(), b.title(), "title");
        assert_eq!(a.snapshot().vt, b.snapshot().vt, "vt overrides");
        if !alt {
            assert_eq!(a.history_size(), b.history_size(), "history size");
        }
        let (ta, tb) = (a.total_lines(), b.total_lines());
        assert_eq!(ta, tb);
        for l in 0..ta {
            let (ra, rb) = (a.styled_line(l).unwrap(), b.styled_line(l).unwrap());
            let (la, lb) = (a.line_cells(l).unwrap(), b.line_cells(l).unwrap());
            assert_eq!(
                ra.iter().map(norm).collect::<Vec<_>>(),
                rb.iter().map(norm).collect::<Vec<_>>(),
                "row {l}: {:?} vs {:?}",
                la.text(),
                lb.text()
            );
            if !alt {
                assert_eq!(la.wrapped, lb.wrapped, "wrap flag row {l}");
            }
        }
        // Command records survive too (up to the history the replay carried).
        assert_eq!(a.has_marks(), b.has_marks(), "has marks");
        assert_eq!(a.busy(), b.busy(), "busy");
        assert_eq!(a.prompt_marks(), b.prompt_marks(), "prompt lines");
        assert_eq!(a.commands_len(), b.commands_len(), "retained commands");
        for n in 1..=a.commands_len() + 1 {
            let key = |c: Option<crate::CommandInfo>| {
                c.map(|c| (c.command, c.output, c.exit_code, c.cwd, c.truncated))
            };
            assert_eq!(
                key(a.last_command(n)),
                key(b.last_command(n)),
                "command {n}"
            );
        }
        assert_eq!(a.running_output(), b.running_output(), "running output");
        // Replayed commands are restored, not newly finished.
        assert_eq!(b.commands_finished(), 0);
        // Replaying twice is idempotent.
        let again = b.replay_bytes(scrollback);
        assert_eq!(replay, again, "replay of a replay differs");
    }

    fn cmd(cwd: Option<&str>, text: &str, out: &str, status: i32) -> Vec<u8> {
        let mut s = String::new();
        if let Some(c) = cwd {
            s += &format!("\x1b]7;file://{c}\x07");
        }
        s += &format!(
            "\x1b]133;A\x07$ \x1b]133;B\x07{text}\r\n\x1b]133;C\x07{out}\x1b]133;D;{status}\x07"
        );
        s.into_bytes()
    }

    #[test]
    fn marks_round_trip_with_commands_cwd_and_status() {
        let mut input = cmd(Some("/tmp/a%20b"), "echo hi", "hi\r\n", 0);
        input.extend(cmd(None, "false", "", 1));
        input.extend(cmd(Some("/srv"), "ls", "a\r\nb\r\n", 0));
        input.extend(b"\x1b]133;A\x07$ ");
        round_trip(&input, 30, 6, 100);
    }

    #[test]
    fn marks_round_trip_across_scrollback_and_wrapping() {
        let mut input = Vec::new();
        for i in 0..40 {
            let long = "w".repeat(70);
            input.extend(cmd(
                Some("/w"),
                &format!("cmd{i}"),
                &format!("{long}\r\n{i}\r\n"),
                i % 3,
            ));
        }
        round_trip(&input, 30, 6, 1000);
    }

    #[test]
    fn marks_round_trip_with_a_running_command_and_wide_text() {
        let mut input = cmd(Some("/x"), "echo 漢字", "漢字\r\n", 0);
        input.extend(
            b"\x1b]133;A\x07$ \x1b]133;B\x07sleep\r\n\x1b]133;C\x07partial output".to_vec(),
        );
        round_trip(&input, 40, 6, 100);
    }

    #[test]
    fn marks_older_than_the_replayed_history_are_left_out() {
        let mut a = Emulator::new(30, 4, 1000);
        for i in 0..20 {
            a.advance(&cmd(None, &format!("c{i}"), &format!("o{i}\r\n"), 0));
        }
        let replay = a.replay_bytes(10);
        let mut b = Emulator::new(30, 4, 1000);
        b.advance(&replay);
        assert!(b.history_size() <= 10);
        let n = b.commands_len();
        assert!((1..20).contains(&n), "{n}");
        assert_eq!(b.last_command(1).unwrap().command, "c19");
        assert_eq!(b.last_command(1).unwrap().output, "o19\n");
        assert_eq!(b.last_command(n).unwrap().output, format!("o{}\n", 20 - n));
    }

    #[test]
    fn a_reset_in_the_stream_forgets_old_commands() {
        let mut a = Emulator::new(30, 4, 100);
        a.advance(&cmd(None, "old", "x\r\n", 0));
        a.advance(b"\x1bc");
        assert!(a.last_command(1).is_none());
        a.advance(&cmd(None, "new", "y\r\n", 0));
        assert_eq!(a.last_command(1).unwrap().command, "new");
        assert_eq!(a.commands_len(), 1);
    }

    #[test]
    fn plain_and_cursor() {
        round_trip(b"hello\r\nworld", 20, 5, 100);
        round_trip(b"", 20, 5, 100);
        round_trip(b"abc\x1b[3;7Hxyz\x1b[2A", 20, 5, 100);
    }

    #[test]
    fn sgr_zoo() {
        round_trip(
            b"\x1b[1mbold\x1b[0m \x1b[3;4:3;58;2;9;8;7mcurly\x1b[0m \x1b[38;2;1;2;3;48;5;200mrgb\x1b[0m \
              \x1b[7mrev\x1b[0m \x1b[9;2mdim strike\x1b[0m \x1b[31;42mansi\r\n\x1b[1;93;100mbright",
            60,
            6,
            100,
        );
    }

    #[test]
    fn pen_survives() {
        round_trip(b"ab\x1b[1;31m", 20, 4, 10);
    }

    #[test]
    fn wide_and_emoji() {
        round_trip("漢字 🦀 e\u{301} ok\r\n👨‍👩‍👧 flags 🇯🇵".as_bytes(), 30, 4, 10);
    }

    #[test]
    fn soft_wrap_and_scrollback_overflow() {
        let mut s = String::new();
        for i in 0..60 {
            s.push_str(&format!("line {i} {}\r\n", "x".repeat(i % 40)));
        }
        round_trip(s.as_bytes(), 25, 6, 30);
        round_trip(&b"y".repeat(200), 25, 4, 30);
    }

    #[test]
    fn background_colour_erase_and_clears() {
        round_trip(b"\x1b[44m\x1b[2Jtext\x1b[K\r\nmore\x1b[0m", 20, 4, 10);
    }

    #[test]
    fn alt_screen_app() {
        round_trip(
            b"main text\r\n\x1b[?1049h\x1b[?25l\x1b[2J\x1b[1;1Hvim ~\r\n\x1b[7mstatus\x1b[0m\x1b[?1h\x1b=\x1b[?1000h\x1b[?1006h\x1b[?2004h",
            30,
            6,
            50,
        );
    }

    #[test]
    fn modes_cursor_style_title_and_colours() {
        round_trip(
            b"\x1b]2;my title\x07\x1b[5 q\x1b[?1004h\x1b[?1002h\x1b[?1007l\x1b]4;1;rgb:11/22/33\x07\x1b[31mred\x1b>",
            30,
            4,
            10,
        );
        round_trip(b"\x1b[?7l\x1b[?6hab\x1b[>5u", 30, 4, 10);
    }

    #[test]
    fn colour_overrides_shift_capture_and_pointer_shape_survive() {
        round_trip(
            b"\x1b]5;0;rgb:11/22/33\x07\x1b]5;1;rgb:44/55/66\x07\x1b]17;#102030\x07\x1b]19;rgb:ff/00/00\x07\
\x1b]13;rgb:01/02/03\x07\x1b]21;cursor_text=#fff;visual_bell=#abc\x07\x1b[>1s\x1b]22;pointer\x07\
\x1b]10;rgb:aa/bb/cc\x07\x1b]4;5;rgb:12/34/56\x07x",
            30,
            4,
            10,
        );
        // The replay really carries them (checked on a third terminal, not via `vt` equality).
        let mut a = Emulator::new(30, 4, 10);
        a.advance(b"\x1b]5;0;rgb:11/22/33\x07\x1b[>1s\x1b]22;text\x07");
        let mut b = Emulator::new(30, 4, 10);
        b.advance(&a.replay_bytes(10));
        let v = b.snapshot().vt;
        assert_eq!(v.bold_color, Some([0x11, 0x22, 0x33]));
        assert!(v.shift_capture);
        assert_eq!(v.pointer_shape.as_deref(), Some("text"));
    }

    #[test]
    fn ris_clears_overrides() {
        let mut a = Emulator::new(30, 4, 10);
        a.advance(b"\x1b]5;0;rgb:11/22/33\x07\x1b[>1s\x1b]22;text\x07\x1b]17;#102030\x07\x1bc");
        assert_eq!(a.snapshot().vt, Default::default());
    }

    #[test]
    fn history_is_capped() {
        let mut a = Emulator::new(10, 3, 1000);
        for i in 0..100 {
            a.advance(format!("r{i}\r\n").as_bytes());
        }
        let mut b = Emulator::new(10, 3, 1000);
        b.advance(&a.replay_bytes(10));
        assert_eq!(b.history_size(), 10);
        assert_eq!(b.line_cells(0).unwrap().text(), "r88");
        assert_eq!(b.screen_text()[1], "r99");
    }
}
