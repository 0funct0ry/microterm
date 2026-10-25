//! A PTY-less terminal: feed bytes, read the grid. Used by tests and (in M10) VT replay.

use std::sync::Arc;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term, TermDamage};
use alacritty_terminal::vte::ansi::Processor;

use crate::events::{Listener, Osc52Policy, Palette, Shared, TermEvent};
use crate::snapshot::{self, CursorShape, Damage, RenderSnapshot};
use alacritty_terminal::vte::ansi::{CursorShape as EngineShape, CursorStyle};

pub(crate) struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// How the UI scrolls the viewport through scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollCmd {
    /// Positive scrolls towards older output.
    Lines(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

impl ScrollCmd {
    pub(crate) fn engine(self) -> Scroll {
        match self {
            Self::Lines(n) => Scroll::Delta(n),
            Self::PageUp => Scroll::PageUp,
            Self::PageDown => Scroll::PageDown,
            Self::Top => Scroll::Top,
            Self::Bottom => Scroll::Bottom,
        }
    }
}

/// The cursor a terminal shows until the program sets one with DECSCUSR (`cursor.*` config).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorDefault {
    pub shape: CursorShape,
    pub blinking: bool,
}

impl Default for CursorDefault {
    fn default() -> Self {
        Self {
            shape: CursorShape::Block,
            blinking: false,
        }
    }
}

impl CursorDefault {
    fn engine(self) -> CursorStyle {
        CursorStyle {
            shape: match self.shape {
                CursorShape::Block => EngineShape::Block,
                CursorShape::Underline => EngineShape::Underline,
                CursorShape::Beam => EngineShape::Beam,
            },
            blinking: self.blinking,
        }
    }
}

pub(crate) fn term_config(scrollback: usize, policy: Osc52Policy, cursor: CursorDefault) -> Config {
    Config {
        // The command tracker trims back to `scrollback` itself (see `commands.rs`).
        scrolling_history: scrollback + crate::commands::HISTORY_SLACK,
        kitty_keyboard: true,
        osc52: policy.engine(),
        default_cursor_style: cursor.engine(),
        ..Config::default()
    }
}

pub(crate) fn take_damage<T>(term: &mut Term<T>) -> Damage {
    let offset = term.grid().display_offset();
    let damage = match term.damage() {
        TermDamage::Full => Damage::Full,
        TermDamage::Partial(it) => Damage::Rows(it.map(|b| b.line).collect()),
    };
    term.reset_damage();
    if offset > 0 {
        Damage::Full
    } else {
        damage
    }
}

pub struct Emulator {
    term: Term<Listener>,
    parser: Processor,
    shared: Arc<Shared>,
    /// `text.wide_regional_indicators`: filters output and widens indicators in snapshots.
    wide_ri: bool,
    width_filter: crate::width_filter::WidthFilter,
    sniffer: crate::mouse::PixelModeSniffer,
    osc: crate::commands::OscSniffer,
    tracker: crate::commands::CommandTracker,
}

impl Emulator {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self::with_cursor(cols, rows, scrollback, CursorDefault::default())
    }

    pub fn with_cursor(cols: usize, rows: usize, scrollback: usize, cursor: CursorDefault) -> Self {
        let shared = Shared::new(
            Box::new(|| {}),
            Palette::default(),
            alacritty_terminal::event::WindowSize {
                num_lines: rows as u16,
                num_cols: cols as u16,
                cell_width: 1,
                cell_height: 1,
            },
            Osc52Policy::Write,
        );
        let term = Term::new(
            term_config(scrollback, Osc52Policy::Write, cursor),
            &Size { cols, rows },
            Listener(shared.clone()),
        );
        Emulator {
            term,
            parser: Processor::default(),
            shared,
            wide_ri: false,
            width_filter: crate::width_filter::WidthFilter::new(),
            sniffer: Default::default(),
            osc: Default::default(),
            tracker: crate::commands::CommandTracker::new(scrollback, cols),
        }
    }

    /// Counts a regional indicator as two cells (four per flag), as zsh does.
    pub fn with_wide_regional_indicators(mut self, on: bool) -> Self {
        self.wide_ri = on;
        self
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        self.sniffer.feed(bytes);
        let Emulator {
            term,
            parser,
            wide_ri,
            width_filter,
            osc,
            tracker,
            shared,
            ..
        } = self;
        let wide_ri = *wide_ri;
        crate::commands::drive(
            osc,
            tracker,
            shared,
            term,
            bytes,
            |term, seg| {
                if wide_ri {
                    let mut out = Vec::with_capacity(seg.len() + seg.len() / 4);
                    width_filter.filter(seg, &mut out);
                    parser.advance(term, &out);
                } else {
                    parser.advance(term, seg);
                }
            },
            |code| shared.notify(TermEvent::CommandFinished(code)),
        );
    }

    /// Installs (or with `None` removes) the trigger line tap; lines go to `queue` tagged `pane`.
    /// No partial-line timer runs for an emulator: call [`flush_line_partial`](Self::flush_line_partial).
    pub fn set_line_tap(
        &mut self,
        cfg: Option<crate::linetap::LineTapCfg>,
        queue: Arc<crate::linetap::LineQueue>,
        pane: u64,
    ) {
        match cfg {
            Some(cfg) => {
                let trimmed = self.tracker.trimmed();
                self.tracker.lines.install(
                    cfg,
                    queue,
                    pane,
                    crate::linetap::TapShared::new(),
                    false,
                    &self.term,
                    trimmed,
                )
            }
            None => self.tracker.lines.remove(),
        }
    }

    /// The logical line with stable id `id` plus context; see [`crate::Terminal::capture_lines`].
    pub fn capture_lines(
        &self,
        id: u64,
        before: usize,
        after: usize,
        max_chars: usize,
    ) -> Option<crate::CapturedLines> {
        crate::linetap::lines_around(
            &self.term,
            self.tracker.trimmed(),
            id,
            before,
            after,
            max_chars,
        )
    }

    /// Installs the tap the way a live terminal does: rows that finish are only recorded, and
    /// [`drain_line_tap`](Self::drain_line_tap) reads them out (tests).
    #[cfg(test)]
    pub(crate) fn set_lazy_line_tap(
        &mut self,
        cfg: crate::linetap::LineTapCfg,
        queue: Arc<crate::linetap::LineQueue>,
        pane: u64,
    ) {
        let trimmed = self.tracker.trimmed();
        self.tracker.lines.install(
            cfg,
            queue,
            pane,
            crate::linetap::TapShared::new(),
            true,
            &self.term,
            trimmed,
        );
    }

    #[cfg(test)]
    pub(crate) fn drain_line_tap(&mut self) -> usize {
        self.tracker.drain_lines(&self.term)
    }

    #[cfg(test)]
    pub(crate) fn set_line_replay(&mut self, on: bool) {
        self.tracker.lines.set_replay(on);
    }

    #[cfg(test)]
    pub(crate) fn settle_line_replay(&mut self) {
        let trimmed = self.tracker.trimmed();
        self.tracker.lines.settle_replay(&self.term, trimmed);
    }

    #[cfg(test)]
    pub(crate) fn flush_line_partial(&mut self) {
        self.tracker.flush_partial_lines(&self.term);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(Size { cols, rows });
        self.tracker.after_resize(&self.term);
    }

    pub fn scroll_display(&mut self, cmd: ScrollCmd) {
        self.term.scroll_display(cmd.engine());
    }

    pub fn snapshot(&mut self) -> RenderSnapshot {
        let damage = take_damage(&mut self.term);
        let mut snap = snapshot::build(&self.term, damage, self.wide_ri);
        snap.modes.sgr_pixels = self
            .sniffer
            .flag()
            .load(std::sync::atomic::Ordering::Relaxed);
        snap.vt = self.shared.vt.lock().unwrap().view();
        snap.line_base = self.tracker.trimmed();
        snap
    }

    pub fn selection_start(
        &mut self,
        kind: crate::SelectionKind,
        row: usize,
        col: usize,
        right: bool,
        word_chars: &str,
    ) {
        crate::selection::start(&mut self.term, kind, row, col, right, word_chars);
    }

    pub fn selection_update(&mut self, row: usize, col: usize, right: bool) {
        crate::selection::update(&mut self.term, row, col, right);
    }

    pub fn selection_clear(&mut self) {
        crate::selection::clear(&mut self.term);
    }

    pub fn select_all(&mut self) {
        crate::selection::select_all(&mut self.term);
    }

    pub fn selection_text(&self, trim_trailing: bool) -> Option<String> {
        crate::selection::text(&self.term, trim_trailing)
    }

    /// Lines of scrollback plus visible rows.
    pub fn total_lines(&self) -> usize {
        crate::lines::total_lines(&self.term)
    }

    pub fn history_size(&self) -> usize {
        crate::lines::history_size(&self.term)
    }

    /// One row by absolute line (0 = oldest scrollback row).
    pub fn line_cells(&self, abs: usize) -> Option<crate::LineCells> {
        crate::lines::line_cells(&self.term, abs)
    }

    pub fn styled_line(&self, abs: usize) -> Option<Vec<crate::Cell>> {
        crate::lines::styled_line(&self.term, abs)
    }

    /// The terminal cursor as `(absolute line, column)`.
    pub fn cursor_abs(&self) -> (usize, usize) {
        crate::lines::cursor_abs(&self.term)
    }

    pub fn select_abs(
        &mut self,
        kind: crate::SelectionKind,
        start: (usize, usize),
        end: (usize, usize),
    ) {
        crate::lines::select_abs(&mut self.term, kind, start, end);
    }

    pub fn scroll_to_abs(&mut self, abs: usize, center: bool) {
        crate::lines::scroll_to_abs(&mut self.term, abs, center);
    }

    /// Absolute lines of OSC 133 prompt starts, ascending. Empty when the shell sends none.
    pub fn prompt_marks(&mut self) -> Vec<usize> {
        self.tracker.prompt_lines()
    }

    /// Whether the shell ever sent an OSC 133 mark.
    pub fn has_marks(&self) -> bool {
        self.tracker.has_marks()
    }

    /// A command is running (`C` seen, `D` not yet).
    pub fn busy(&self) -> bool {
        self.tracker.busy()
    }

    /// Commands finished so far (count of `D` marks that ended a command).
    pub fn commands_finished(&self) -> u64 {
        self.tracker.finished_total()
    }

    /// Commands retained in the ring (at most 500), finished or not.
    pub fn commands_len(&self) -> usize {
        self.tracker.len()
    }

    /// The `n`-th most recent finished command (`1` = latest) with its rendered output.
    pub fn last_command(&self, n: usize) -> Option<crate::CommandInfo> {
        self.tracker.finished(&self.term, n)
    }

    /// Output of the running command so far.
    pub fn running_output(&self) -> Option<String> {
        self.tracker.running_output(&self.term)
    }

    /// The whole grid as logical lines (soft wraps joined, trailing blanks trimmed).
    pub fn logical_lines(&self) -> Vec<String> {
        crate::lines::logical_lines(&self.term)
    }

    pub fn drain_events(&mut self) -> Vec<TermEvent> {
        self.shared.drain()
    }

    /// Sends the terminal's replies (DA, DSR, colour queries) to `f`; tests and the daemon use
    /// it where there is no PTY.
    pub fn set_reply_sink(&self, f: impl Fn(Vec<u8>) + Send + Sync + 'static) {
        self.shared.set_sink(Box::new(f));
    }

    /// Applies the `[terminal]` settings.
    pub fn set_vt_config(&mut self, cfg: crate::vt::VtConfig) {
        let old = std::mem::replace(&mut self.shared.vt.lock().unwrap().cfg, cfg).private;
        if old != cfg.private {
            let cleared = self.tracker.private_gate_changed(&old, &cfg.private);
            self.shared.notify_cleared(cleared);
        }
    }

    /// The `7725` status segments the program set.
    pub fn status_segments(&self) -> Vec<crate::private_osc::Segment> {
        self.tracker.status_segments()
    }

    /// The `7720` progress bars the program set.
    pub fn progress_bars(&self) -> Vec<crate::private_osc::Bar> {
        self.tracker.progress_bars()
    }

    /// The `7718` badge the program set.
    pub fn badge(&self) -> Option<crate::private_osc::Badge> {
        self.tracker.badge()
    }

    /// The `7722` marks the program set, oldest first.
    pub fn program_marks(&self) -> Vec<crate::private_osc::ProgramMark> {
        self.tracker.program_marks()
    }

    /// Stable id of the oldest line still in the grid (see `cursor_line_id`).
    pub fn line_id_base(&self) -> u64 {
        self.tracker.line_id_base()
    }

    /// The `7719` color the program declared for its tab.
    pub fn declared_tab_color(&self) -> Option<String> {
        self.tracker.declared_tab_color()
    }

    /// Drops stale and finished bars at `now_ms`.
    pub fn expire_progress(&mut self, now_ms: u64) -> bool {
        self.tracker.expire_progress(now_ms)
    }

    /// Private-OSC counters.
    pub fn private_counters(&self) -> crate::private_osc::Counters {
        self.tracker.private_counters()
    }

    /// A byte stream that, fed to a fresh emulator of the same size, reproduces this one's
    /// screen, scrollback (at most `max_history` rows), modes, cursor, colours and title
    /// (SPEC D6). Not preserved: scroll region, saved cursor, charsets, tab stops, hyperlinks
    /// and the main screen hidden behind an active alternate screen.
    pub fn replay_bytes(&self, max_history: usize) -> Vec<u8> {
        crate::replay::synthesize(
            &self.term,
            self.shared.title().as_deref(),
            self.sniffer
                .flag()
                .load(std::sync::atomic::Ordering::Relaxed),
            max_history,
            &self.tracker,
            &self.shared.vt.lock().unwrap(),
        )
    }

    /// Items shell programs registered with `OSC 7717` (SPEC §10.9.7).
    pub fn menu_items(&self) -> Vec<crate::menu_reg::RegItem> {
        self.tracker.menus().items().to_vec()
    }

    /// The working directory the program reported with OSC 7.
    pub fn cwd(&self) -> Option<String> {
        self.tracker.cwd().map(str::to_string)
    }

    /// The last OSC 0/2 title the program set.
    pub fn title(&self) -> Option<String> {
        self.shared.title()
    }

    /// Visible rows as trimmed text.
    pub fn screen_text(&mut self) -> Vec<String> {
        let s = self.snapshot();
        (0..s.rows).map(|r| s.row_text(r)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{CellColor, CellFlags, CursorShape, Underline};

    fn emu(cols: usize, rows: usize) -> Emulator {
        Emulator::new(cols, rows, 100)
    }

    fn first_cell(input: &str) -> crate::snapshot::Cell {
        let mut e = emu(10, 3);
        e.advance(input.as_bytes());
        e.snapshot().row(0)[0].clone()
    }

    #[test]
    fn plain_text_and_newline() {
        let mut e = emu(10, 3);
        e.advance(b"hello\r\nworld");
        let t = e.screen_text();
        assert_eq!(t[0], "hello");
        assert_eq!(t[1], "world");
        assert_eq!(e.snapshot().cursor.col, 5);
    }

    #[test]
    fn sgr_bold_dim_italic_strike_inverse_hidden() {
        assert!(first_cell("\x1b[1mx").flags.has(CellFlags::BOLD));
        assert!(first_cell("\x1b[2mx").flags.has(CellFlags::DIM));
        assert!(first_cell("\x1b[3mx").flags.has(CellFlags::ITALIC));
        assert!(first_cell("\x1b[9mx").flags.has(CellFlags::STRIKE));
        assert!(first_cell("\x1b[7mx").flags.has(CellFlags::INVERSE));
        assert!(first_cell("\x1b[8mx").flags.has(CellFlags::HIDDEN));
        // Reset.
        let c = first_cell("\x1b[1;3;9m\x1b[0mx");
        assert_eq!(c.flags, CellFlags::default());
    }

    #[test]
    fn sgr_underline_styles() {
        assert_eq!(first_cell("\x1b[4mx").underline, Underline::Single);
        assert_eq!(first_cell("\x1b[4:1mx").underline, Underline::Single);
        // SGR 21 is "cancel bold" in alacritty_terminal's parser (a deviation from SPEC §6.1,
        // which lists it as double underline); `4:2` is the supported spelling.
        assert_eq!(first_cell("\x1b[4:2mx").underline, Underline::Double);
        assert_eq!(first_cell("\x1b[4:3mx").underline, Underline::Curly);
        assert_eq!(first_cell("\x1b[4:4mx").underline, Underline::Dotted);
        assert_eq!(first_cell("\x1b[4:5mx").underline, Underline::Dashed);
        assert_eq!(first_cell("\x1b[4m\x1b[24mx").underline, Underline::None);
    }

    #[test]
    fn sgr_underline_colour() {
        let c = first_cell("\x1b[4;58;2;255;0;128mx");
        assert_eq!(c.underline_color, Some(CellColor::Rgb(255, 0, 128)));
        let c = first_cell("\x1b[4;58;5;9mx");
        assert_eq!(c.underline_color, Some(CellColor::Indexed(9)));
        let c = first_cell("\x1b[4;58;5;9m\x1b[59mx");
        assert_eq!(c.underline_color, None);
    }

    #[test]
    fn sgr_colours() {
        let c = first_cell("\x1b[31;42mx");
        assert_eq!(c.fg, CellColor::Indexed(1));
        assert_eq!(c.bg, CellColor::Indexed(2));
        let c = first_cell("\x1b[91;102mx");
        assert_eq!(c.fg, CellColor::Indexed(9));
        assert_eq!(c.bg, CellColor::Indexed(10));
        let c = first_cell("\x1b[38;5;200;48;5;17mx");
        assert_eq!(c.fg, CellColor::Indexed(200));
        assert_eq!(c.bg, CellColor::Indexed(17));
        let c = first_cell("\x1b[38;2;1;2;3;48;2;4;5;6mx");
        assert_eq!(c.fg, CellColor::Rgb(1, 2, 3));
        assert_eq!(c.bg, CellColor::Rgb(4, 5, 6));
        let c = first_cell("\x1b[31m\x1b[39mx");
        assert_eq!(c.fg, CellColor::DefaultFg);
    }

    #[test]
    fn osc_palette_override_resolves_in_snapshot() {
        let mut e = emu(10, 3);
        e.advance(b"\x1b]4;1;rgb:ff/00/00\x07\x1b[31mx");
        assert_eq!(e.snapshot().row(0)[0].fg, CellColor::Rgb(255, 0, 0));
    }

    #[test]
    fn cursor_movement() {
        let mut e = emu(10, 5);
        e.advance(b"\x1b[3;4H");
        let c = e.snapshot().cursor;
        assert_eq!((c.row, c.col), (2, 3));
        e.advance(b"\x1b[2A\x1b[2C");
        let c = e.snapshot().cursor;
        assert_eq!((c.row, c.col), (0, 5));
        e.advance(b"\x1b[H\x1b[B\x1b[D");
        let c = e.snapshot().cursor;
        assert_eq!((c.row, c.col), (1, 0));
    }

    #[test]
    fn erase_and_clear() {
        let mut e = emu(10, 3);
        e.advance(b"abcdef\x1b[1;3H\x1b[K");
        assert_eq!(e.screen_text()[0], "ab");
        e.advance(b"\x1b[2J");
        assert!(e.screen_text().iter().all(|l| l.is_empty()));
    }

    #[test]
    fn wrapping() {
        let mut e = emu(5, 3);
        e.advance(b"abcdefgh");
        let t = e.screen_text();
        assert_eq!(t[0], "abcde");
        assert_eq!(t[1], "fgh");
        assert!(e.snapshot().row(0)[4].flags.has(CellFlags::WRAPLINE));
    }

    #[test]
    fn wide_chars_take_two_cells() {
        let mut e = emu(6, 2);
        e.advance("a漢b".as_bytes());
        let s = e.snapshot();
        assert!(s.row(0)[1].flags.has(CellFlags::WIDE));
        assert!(s.row(0)[2].flags.has(CellFlags::WIDE_SPACER));
        assert_eq!(s.row(0)[3].ch, 'b');
        assert_eq!(s.row_text(0), "a漢b");
    }

    #[test]
    fn combining_marks_attach() {
        let mut e = emu(6, 2);
        e.advance("e\u{301}".as_bytes());
        let c = e.snapshot().row(0)[0].clone();
        assert_eq!(c.ch, 'e');
        assert_eq!(c.zerowidth.as_deref(), Some(&['\u{301}'][..]));
    }

    #[test]
    fn alternate_screen_round_trip() {
        let mut e = emu(10, 3);
        e.advance(b"main");
        e.advance(b"\x1b[?1049h");
        assert!(e.snapshot().modes.alt_screen);
        assert!(e.screen_text().iter().all(|l| l.is_empty()));
        e.advance(b"\x1b[Halt");
        assert_eq!(e.screen_text()[0], "alt");
        e.advance(b"\x1b[?1049l");
        assert!(!e.snapshot().modes.alt_screen);
        assert_eq!(e.screen_text()[0], "main");
    }

    #[test]
    fn scrollback_and_display_offset() {
        let mut e = Emulator::new(10, 3, 100);
        for i in 0..10 {
            e.advance(format!("line{i}\r\n").as_bytes());
        }
        let s = e.snapshot();
        assert!(s.history_size >= 7);
        assert_eq!(s.display_offset, 0);
        e.scroll_display(ScrollCmd::Top);
        let s = e.snapshot();
        assert_eq!(s.display_offset, s.history_size);
        assert_eq!(s.row_text(0), "line0");
        e.scroll_display(ScrollCmd::Bottom);
        assert_eq!(e.snapshot().display_offset, 0);
        e.scroll_display(ScrollCmd::Lines(2));
        assert_eq!(e.snapshot().display_offset, 2);
    }

    #[test]
    fn resize_reflows_wrapped_lines() {
        let mut e = emu(10, 4);
        e.advance(b"0123456789abcdef");
        let t = e.screen_text();
        assert_eq!(t[0], "0123456789");
        assert_eq!(t[1], "abcdef");
        e.resize(20, 4);
        let t = e.screen_text();
        assert_eq!(t[0], "0123456789abcdef");
        let s = e.snapshot();
        assert_eq!((s.cols, s.rows), (20, 4));
        e.resize(5, 4);
        e.scroll_display(ScrollCmd::Top);
        assert_eq!(e.screen_text()[0], "01234");
    }

    #[test]
    fn decscusr_cursor_styles() {
        let mut e = emu(10, 3);
        e.advance(b"\x1b[6 q");
        let c = e.snapshot().cursor;
        assert_eq!(c.shape, CursorShape::Beam);
        assert!(!c.blinking);
        e.advance(b"\x1b[3 q");
        let c = e.snapshot().cursor;
        assert_eq!(c.shape, CursorShape::Underline);
        assert!(c.blinking);
        e.advance(b"\x1b[2 q");
        assert_eq!(e.snapshot().cursor.shape, CursorShape::Block);
    }

    #[test]
    fn configured_default_cursor_applies_until_decscusr_overrides_it() {
        let mut e = Emulator::with_cursor(
            10,
            3,
            100,
            CursorDefault {
                shape: CursorShape::Beam,
                blinking: true,
            },
        );
        let c = e.snapshot().cursor;
        assert_eq!((c.shape, c.blinking), (CursorShape::Beam, true));
        e.advance(b"\x1b[2 q"); // steady block
        let c = e.snapshot().cursor;
        assert_eq!((c.shape, c.blinking), (CursorShape::Block, false));
        e.advance(b"\x1b[0 q"); // back to the configured default
        let c = e.snapshot().cursor;
        assert_eq!((c.shape, c.blinking), (CursorShape::Beam, true));
    }

    #[test]
    fn cursor_visibility() {
        let mut e = emu(10, 3);
        assert!(e.snapshot().cursor.visible);
        e.advance(b"\x1b[?25l");
        assert!(!e.snapshot().cursor.visible);
        e.advance(b"\x1b[?25h");
        assert!(e.snapshot().cursor.visible);
    }

    #[test]
    fn modes_reported() {
        let mut e = emu(10, 3);
        e.advance(b"\x1b[?2004h\x1b[?1h\x1b[?1004h\x1b=");
        let m = e.snapshot().modes;
        assert!(m.bracketed_paste && m.app_cursor && m.focus_reporting && m.app_keypad);
        e.advance(b"\x1b[?1000h\x1b[?1006h");
        let m = e.snapshot().modes;
        assert!(m.mouse_reporting && m.sgr_mouse);
    }

    #[test]
    fn kitty_keyboard_flags_push_and_query() {
        let mut e = emu(10, 3);
        e.advance(b"\x1b[>1u");
        assert!(e.snapshot().modes.kitty.disambiguate);
        e.advance(b"\x1b[<u");
        assert!(!e.snapshot().modes.kitty.any());
    }

    #[test]
    fn title_and_bell_events() {
        let mut e = emu(10, 3);
        e.advance(b"\x1b]0;hello\x07\x07\x1b]2;world\x1b\\");
        let ev = e.drain_events();
        let titles: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                TermEvent::Title(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(titles, ["hello", "world"]);
        assert!(ev.iter().any(|e| matches!(e, TermEvent::Bell)));
    }

    #[test]
    fn wakeups_coalesce() {
        let mut e = emu(10, 3);
        e.advance(b"a");
        e.advance(b"b");
        let n = e
            .drain_events()
            .iter()
            .filter(|e| matches!(e, TermEvent::Wakeup))
            .count();
        assert!(n <= 1);
    }

    #[test]
    fn osc52_write_allowed_and_capped() {
        let mut e = emu(10, 3);
        // "hi" in base64.
        e.advance(b"\x1b]52;c;aGk=\x07");
        let ev = e.drain_events();
        assert!(ev
            .iter()
            .any(|e| matches!(e, TermEvent::ClipboardStore(_, t) if t == "hi")));
        // Read request is not surfaced under the write-only policy.
        e.advance(b"\x1b]52;c;?\x07");
        assert!(!e
            .drain_events()
            .iter()
            .any(|e| matches!(e, TermEvent::ClipboardLoad(..))));
        // > 100 KiB payload is dropped.
        let big = "A".repeat(200 * 1024);
        let payload = base64_encode(big.as_bytes());
        e.advance(format!("\x1b]52;c;{payload}\x07").as_bytes());
        assert!(!e
            .drain_events()
            .iter()
            .any(|e| matches!(e, TermEvent::ClipboardStore(..))));
    }

    fn base64_encode(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for ch in data.chunks(3) {
            let n = (ch[0] as u32) << 16
                | (*ch.get(1).unwrap_or(&0) as u32) << 8
                | *ch.get(2).unwrap_or(&0) as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if ch.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if ch.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    #[test]
    fn synchronized_output_defers_content() {
        let mut e = emu(10, 3);
        e.advance(b"\x1b[?2026hhidden");
        assert_eq!(e.screen_text()[0], "");
        e.advance(b"\x1b[?2026l");
        assert_eq!(e.screen_text()[0], "hidden");
    }

    #[test]
    fn row_hashes_track_content() {
        let mut e = emu(10, 3);
        e.advance(b"abc");
        let a = e.snapshot();
        e.advance(b"\x1b[2;1Hxyz");
        let b = e.snapshot();
        assert_eq!(a.row_hashes[0], b.row_hashes[0]);
        assert_ne!(a.row_hashes[1], b.row_hashes[1]);
    }

    #[test]
    fn damage_reports_changed_rows() {
        let mut e = emu(10, 4);
        e.snapshot(); // consume initial full damage
        e.advance(b"\x1b[3;1Hx");
        match e.snapshot().damage {
            // Row 0 is the old cursor row; row 2 is the edit.
            Damage::Rows(r) => {
                assert!(r.contains(&2));
                assert!(!r.contains(&1) && !r.contains(&3));
            }
            Damage::Full => panic!("expected partial damage"),
        }
    }

    /// Cells used by `text` followed by `x`, and where `x` landed. The engine has no grapheme
    /// clustering (SPEC §7.2 defers widths to its tables), so these pin down what the renderer
    /// has to cope with.
    fn x_col(text: &str) -> usize {
        let mut e = emu(16, 2);
        e.advance(text.as_bytes());
        e.advance(b"x");
        let s = e.snapshot();
        s.row(0)
            .iter()
            .position(|c| c.ch == 'x')
            .expect("x on row 0")
    }

    #[test]
    fn emoji_and_cjk_widths() {
        assert_eq!(x_col("a"), 1);
        assert_eq!(x_col("🦀"), 2, "emoji presentation is 2 cells");
        assert_eq!(x_col("漢字"), 4, "CJK is 2 cells each");
        assert_eq!(x_col("e\u{301}"), 1, "combining mark adds no cell");
        assert_eq!(x_col("❤\u{fe0f}"), 1, "VS16 does not widen in the engine");
        assert_eq!(x_col("❤\u{fe0e}"), 1);
    }

    #[test]
    fn multi_codepoint_emoji_widths_follow_the_engine() {
        // No grapheme clustering: each wide scalar takes its own two cells.
        assert_eq!(
            x_col("👍🏽"),
            4,
            "skin tone modifier is its own wide cell pair"
        );
        assert_eq!(
            x_col("👨\u{200d}👩\u{200d}👧"),
            6,
            "ZWJ keeps the joiner on the previous cell"
        );
        // Regional indicators are narrow: a flag is two single cells.
        assert_eq!(x_col("🇯🇵"), 2);
    }

    #[test]
    fn zwj_is_stored_as_a_zero_width_mark() {
        let mut e = emu(16, 2);
        e.advance("👨\u{200d}👩".as_bytes());
        let s = e.snapshot();
        assert_eq!(s.row(0)[0].zerowidth.as_deref(), Some(&['\u{200d}'][..]));
        assert!(s.row(0)[0].flags.has(CellFlags::WIDE));
        assert_eq!(s.row(0)[2].ch, '👩');
    }

    /// zsh-autosuggestions draws `ho 🇯🇵🇫🇷` after `ec`, then moves back by zsh's own count
    /// (flags are 4 cells there). With the option on the cursor must land after `ec`.
    #[test]
    fn wide_regional_indicators_keep_zsh_cursor_moves_in_step() {
        let typed = "❯ ec";
        let suggestion = "ho 🇯🇵🇫🇷";
        let zsh_width = 3 + 4 * 2; // "ho " + two flags of 4 cells each
        let input = format!("{typed}{suggestion}\x1b[{zsh_width}D");

        let mut off = emu(30, 2);
        off.advance(input.as_bytes());
        assert_eq!(
            off.snapshot().cursor.col,
            0,
            "default engine widths: cursor ends up at column 0 (the reported bug)"
        );

        let mut on = Emulator::new(30, 2, 100).with_wide_regional_indicators(true);
        on.advance(input.as_bytes());
        let s = on.snapshot();
        assert_eq!(s.cursor.col, 4, "cursor sits right after the typed `ec`");
        assert_eq!(s.row_text(0), "❯ echo 🇯🇵🇫🇷");
    }

    #[test]
    fn wide_regional_indicators_occupy_two_cells_each() {
        let mut e = Emulator::new(16, 2, 100).with_wide_regional_indicators(true);
        e.advance("🇯🇵x".as_bytes());
        let s = e.snapshot();
        assert!(s.row(0)[0].flags.has(CellFlags::WIDE));
        assert!(s.row(0)[1].flags.has(CellFlags::WIDE_SPACER));
        assert!(s.row(0)[2].flags.has(CellFlags::WIDE));
        assert!(s.row(0)[3].flags.has(CellFlags::WIDE_SPACER));
        assert_eq!(s.row(0)[4].ch, 'x');
        // Copying the row gives the flag back without the filler spaces.
        assert_eq!(s.row_text(0), "🇯🇵x");
    }

    #[test]
    fn default_widths_are_unchanged_when_the_option_is_off() {
        assert_eq!(x_col("🇯🇵"), 2);
    }

    #[test]
    fn wide_regional_indicators_split_reads_and_titles() {
        let mut e = Emulator::new(16, 2, 100).with_wide_regional_indicators(true);
        let flag = "🇯".as_bytes();
        e.advance(&flag[..2]);
        e.advance(&flag[2..]);
        e.advance(b"x");
        let s = e.snapshot();
        assert_eq!(s.row(0)[2].ch, 'x');
        // Indicators in an OSC title are not padded.
        e.advance("\x1b]0;🇯🇵\x07".as_bytes());
        let titles: Vec<_> = e
            .drain_events()
            .into_iter()
            .filter_map(|ev| match ev {
                TermEvent::Title(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(titles, ["🇯🇵"]);
    }
}

#[cfg(test)]
mod hyperlink_tests {
    use super::*;

    fn link_at(snap: &RenderSnapshot, col: usize) -> Option<(String, String)> {
        snap.row(0)[col]
            .link
            .as_ref()
            .map(|l| (l.id.clone(), l.uri.clone()))
    }

    #[test]
    fn osc8_explicit_link() {
        let mut e = Emulator::new(20, 2, 0);
        e.advance(b"a\x1b]8;;https://x.io/p\x1b\\link\x1b]8;;\x1b\\b");
        let s = e.snapshot();
        assert_eq!(link_at(&s, 0), None);
        assert_eq!(link_at(&s, 1).unwrap().1, "https://x.io/p");
        assert_eq!(link_at(&s, 4).unwrap().1, "https://x.io/p");
        assert_eq!(link_at(&s, 5), None);
    }

    #[test]
    fn osc8_id_param_and_semicolon_in_uri() {
        let mut e = Emulator::new(20, 2, 0);
        e.advance(b"\x1b]8;id=abc:foo=bar;https://x.io/a;b?c=1;2\x07hi\x1b]8;;\x07");
        let s = e.snapshot();
        let (id, uri) = link_at(&s, 0).unwrap();
        assert_eq!(id, "abc");
        assert_eq!(uri, "https://x.io/a;b?c=1;2");
    }

    #[test]
    fn link_changes_row_hash() {
        let mut a = Emulator::new(10, 1, 0);
        a.advance(b"hi");
        let mut b = Emulator::new(10, 1, 0);
        b.advance(b"\x1b]8;;http://a\x07hi\x1b]8;;\x07");
        assert_ne!(a.snapshot().row_hashes[0], b.snapshot().row_hashes[0]);
    }
}
