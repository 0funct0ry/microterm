//! Engine-independent view of the visible grid.
//!
//! `mt_ui` renders from these types only; nothing here leaks `alacritty_terminal`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell as EngineCell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape as EngineShape, NamedColor};

use crate::input::{InputMode, KittyFlags};

/// A colour as stored in a cell. Palette lookup happens in the UI so themes apply live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellColor {
    DefaultFg,
    DefaultBg,
    /// 0..=15 ANSI, 16..=255 cube and greys.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellFlags(pub u16);

impl CellFlags {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 1 << 1;
    pub const DIM: u16 = 1 << 2;
    pub const INVERSE: u16 = 1 << 3;
    pub const HIDDEN: u16 = 1 << 4;
    pub const STRIKE: u16 = 1 << 5;
    pub const WIDE: u16 = 1 << 6;
    /// The right half of a wide character; draws nothing.
    pub const WIDE_SPACER: u16 = 1 << 7;
    pub const WRAPLINE: u16 = 1 << 8;

    pub fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    pub ch: char,
    pub zerowidth: Option<Box<[char]>>,
    pub fg: CellColor,
    pub bg: CellColor,
    pub underline: Underline,
    pub underline_color: Option<CellColor>,
    pub flags: CellFlags,
}

impl Cell {
    pub fn is_blank(&self) -> bool {
        (self.ch == ' ' || self.ch == '\0')
            && self.zerowidth.is_none()
            && self.bg == CellColor::DefaultBg
            && self.underline == Underline::None
            && !self.flags.has(CellFlags::INVERSE | CellFlags::STRIKE)
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            zerowidth: None,
            fg: CellColor::DefaultFg,
            bg: CellColor::DefaultBg,
            underline: Underline::None,
            underline_color: None,
            flags: CellFlags::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub visible: bool,
    pub blinking: bool,
}

/// Terminal modes the UI and input encoder care about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub alt_screen: bool,
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_reporting: bool,
    pub mouse_reporting: bool,
    pub sgr_mouse: bool,
    pub alternate_scroll: bool,
    pub kitty: KittyFlags,
}

impl Modes {
    pub fn input_mode(&self) -> InputMode {
        InputMode {
            app_cursor: self.app_cursor,
            app_keypad: self.app_keypad,
            kitty: self.kitty,
        }
    }

    pub(crate) fn from_engine(m: TermMode) -> Self {
        Modes {
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            app_cursor: m.contains(TermMode::APP_CURSOR),
            app_keypad: m.contains(TermMode::APP_KEYPAD),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            focus_reporting: m.contains(TermMode::FOCUS_IN_OUT),
            mouse_reporting: m.intersects(TermMode::MOUSE_MODE),
            sgr_mouse: m.contains(TermMode::SGR_MOUSE),
            alternate_scroll: m.contains(TermMode::ALTERNATE_SCROLL),
            kitty: KittyFlags {
                disambiguate: m.contains(TermMode::DISAMBIGUATE_ESC_CODES),
                report_events: m.contains(TermMode::REPORT_EVENT_TYPES),
                report_alternates: m.contains(TermMode::REPORT_ALTERNATE_KEYS),
                report_all_keys: m.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
                report_text: m.contains(TermMode::REPORT_ASSOCIATED_TEXT),
            },
        }
    }
}

/// Which rows changed since the previous snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    Full,
    Rows(Vec<usize>),
}

#[derive(Clone, Debug)]
pub struct RenderSnapshot {
    pub cols: usize,
    pub rows: usize,
    /// Row-major, `rows * cols` cells.
    pub cells: Vec<Cell>,
    /// Content hash per row, used as the line-layout cache key.
    pub row_hashes: Vec<u64>,
    pub cursor: Cursor,
    /// Lines scrolled back from the bottom (0 = live view).
    pub display_offset: usize,
    /// Lines of scrollback currently stored.
    pub history_size: usize,
    pub modes: Modes,
    pub damage: Damage,
}

impl RenderSnapshot {
    pub fn row(&self, r: usize) -> &[Cell] {
        &self.cells[r * self.cols..(r + 1) * self.cols]
    }

    /// Row text with trailing blanks trimmed; wide-char spacers skipped.
    pub fn row_text(&self, r: usize) -> String {
        let mut s = String::new();
        for c in self.row(r) {
            if c.flags.has(CellFlags::WIDE_SPACER) {
                continue;
            }
            s.push(if c.ch == '\0' { ' ' } else { c.ch });
            if let Some(z) = &c.zerowidth {
                s.extend(z.iter());
            }
        }
        s.trim_end().to_string()
    }
}

fn convert_color(c: Color, colors: &Colors) -> CellColor {
    let over = |i: usize, fallback: CellColor| match colors[i] {
        Some(rgb) => CellColor::Rgb(rgb.r, rgb.g, rgb.b),
        None => fallback,
    };
    match c {
        Color::Spec(rgb) => CellColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => over(i as usize, CellColor::Indexed(i)),
        Color::Named(n) => match n {
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                over(256, CellColor::DefaultFg)
            }
            NamedColor::Background => over(257, CellColor::DefaultBg),
            NamedColor::Cursor => over(258, CellColor::DefaultFg),
            other => {
                let i = other as usize;
                // Dim* variants (259..=266) map onto the plain ANSI 0..=7.
                let i = if i >= 259 { i - 259 } else { i };
                over(i, CellColor::Indexed(i as u8))
            }
        },
    }
}

fn convert_cell(cell: &EngineCell, colors: &Colors) -> Cell {
    let f = cell.flags;
    let mut flags = 0u16;
    let mut set = |on: bool, bit: u16| {
        if on {
            flags |= bit;
        }
    };
    set(f.intersects(Flags::BOLD), CellFlags::BOLD);
    set(f.intersects(Flags::ITALIC), CellFlags::ITALIC);
    set(f.intersects(Flags::DIM), CellFlags::DIM);
    set(f.intersects(Flags::INVERSE), CellFlags::INVERSE);
    set(f.intersects(Flags::HIDDEN), CellFlags::HIDDEN);
    set(f.intersects(Flags::STRIKEOUT), CellFlags::STRIKE);
    set(f.intersects(Flags::WIDE_CHAR), CellFlags::WIDE);
    set(
        f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
        CellFlags::WIDE_SPACER,
    );
    set(f.intersects(Flags::WRAPLINE), CellFlags::WRAPLINE);
    let underline = if f.contains(Flags::UNDERCURL) {
        Underline::Curly
    } else if f.contains(Flags::DOUBLE_UNDERLINE) {
        Underline::Double
    } else if f.contains(Flags::DOTTED_UNDERLINE) {
        Underline::Dotted
    } else if f.contains(Flags::DASHED_UNDERLINE) {
        Underline::Dashed
    } else if f.contains(Flags::UNDERLINE) {
        Underline::Single
    } else {
        Underline::None
    };
    Cell {
        ch: cell.c,
        zerowidth: cell.zerowidth().map(|z| z.to_vec().into_boxed_slice()),
        fg: convert_color(cell.fg, colors),
        bg: convert_color(cell.bg, colors),
        underline,
        underline_color: cell.underline_color().map(|c| convert_color(c, colors)),
        flags: CellFlags(flags),
    }
}

/// Regional indicator symbols (U+1F1E6..=U+1F1FF).
fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// `text.wide_regional_indicators`: the width filter writes a space after every regional
/// indicator, so an indicator followed by a plain space is shown as one wide cell. Four cells
/// per flag is what zsh's line editor counts.
pub(crate) fn widen_regional_indicators(row: &mut [Cell]) {
    for c in 0..row.len().saturating_sub(1) {
        let (head, tail) = row.split_at_mut(c + 1);
        let (ri, next) = (&mut head[c], &mut tail[0]);
        if is_regional_indicator(ri.ch)
            && !ri.flags.has(CellFlags::WIDE)
            && !ri.flags.has(CellFlags::WIDE_SPACER)
            && ri.zerowidth.is_none()
            && next.ch == ' '
            && next.zerowidth.is_none()
            && !next.flags.has(CellFlags::WIDE)
            && !next.flags.has(CellFlags::WIDE_SPACER)
        {
            ri.flags = CellFlags(ri.flags.0 | CellFlags::WIDE);
            next.flags = CellFlags(next.flags.0 | CellFlags::WIDE_SPACER);
        }
    }
}

pub(crate) fn build<T>(term: &Term<T>, damage: Damage, widen_ri: bool) -> RenderSnapshot {
    let grid = term.grid();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    let offset = grid.display_offset();
    let colors = term.colors();

    let mut cells = Vec::with_capacity(rows * cols);
    let mut row_hashes = Vec::with_capacity(rows);
    for r in 0..rows {
        let line = Line(r as i32 - offset as i32);
        let start = cells.len();
        for c in 0..cols {
            cells.push(convert_cell(&grid[Point::new(line, Column(c))], colors));
        }
        if widen_ri {
            widen_regional_indicators(&mut cells[start..]);
        }
        let mut h = DefaultHasher::new();
        cells[start..].hash(&mut h);
        row_hashes.push(h.finish());
    }

    let style = term.cursor_style();
    let cur = grid.cursor.point;
    let cur_row = cur.line.0 + offset as i32;
    let shape = match style.shape {
        EngineShape::Underline => CursorShape::Underline,
        EngineShape::Beam => CursorShape::Beam,
        _ => CursorShape::Block,
    };
    let mode = *term.mode();
    let visible = mode.contains(TermMode::SHOW_CURSOR)
        && style.shape != EngineShape::Hidden
        && (0..rows as i32).contains(&cur_row);

    RenderSnapshot {
        cols,
        rows,
        cells,
        row_hashes,
        cursor: Cursor {
            row: cur_row.max(0) as usize,
            col: cur.column.0,
            shape,
            visible,
            blinking: style.blinking,
        },
        display_offset: offset,
        history_size: grid.history_size(),
        modes: Modes::from_engine(mode),
        damage,
    }
}
