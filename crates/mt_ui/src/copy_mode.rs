//! Copy mode (SPEC §10.5): a vi-style cursor over the whole scrollback.
//!
//! Pure logic over a [`TextGrid`]; the app feeds it normalised key names and applies the
//! [`Outcome`]. Positions are `(absolute line, column)`.

use crate::search::{find_all, smart_case};
use crate::textgrid::TextGrid;

pub type Pos = (usize, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelKind {
    Char,
    Line,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyAction {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBackward,
    WordEnd,
    BigWordForward,
    BigWordBackward,
    BigWordEnd,
    LineStart,
    FirstNonBlank,
    LineEnd,
    Top,
    Bottom,
    ViewTop,
    ViewMiddle,
    ViewBottom,
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    ParagraphPrev,
    ParagraphNext,
    PromptPrev,
    PromptNext,
    SelectChar,
    SelectLine,
    SelectBlock,
    SwapEnds,
    Yank,
    YankStay,
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    /// `Esc`: clears the selection first, then exits.
    Cancel,
    Exit,
}

/// Names accepted in `[[copy_mode.keys]]` `action = "…"`.
pub const ACTION_NAMES: &[(&str, CopyAction)] = &[
    ("MoveLeft", CopyAction::Left),
    ("MoveRight", CopyAction::Right),
    ("MoveUp", CopyAction::Up),
    ("MoveDown", CopyAction::Down),
    ("WordForward", CopyAction::WordForward),
    ("WordBackward", CopyAction::WordBackward),
    ("WordEnd", CopyAction::WordEnd),
    ("BigWordForward", CopyAction::BigWordForward),
    ("BigWordBackward", CopyAction::BigWordBackward),
    ("BigWordEnd", CopyAction::BigWordEnd),
    ("LineStart", CopyAction::LineStart),
    ("FirstNonBlank", CopyAction::FirstNonBlank),
    ("LineEnd", CopyAction::LineEnd),
    ("Top", CopyAction::Top),
    ("Bottom", CopyAction::Bottom),
    ("ViewTop", CopyAction::ViewTop),
    ("ViewMiddle", CopyAction::ViewMiddle),
    ("ViewBottom", CopyAction::ViewBottom),
    ("HalfPageUp", CopyAction::HalfPageUp),
    ("HalfPageDown", CopyAction::HalfPageDown),
    ("PageUp", CopyAction::PageUp),
    ("PageDown", CopyAction::PageDown),
    ("ParagraphPrev", CopyAction::ParagraphPrev),
    ("ParagraphNext", CopyAction::ParagraphNext),
    ("PromptPrev", CopyAction::PromptPrev),
    ("PromptNext", CopyAction::PromptNext),
    ("SelectChar", CopyAction::SelectChar),
    ("SelectLine", CopyAction::SelectLine),
    ("SelectBlock", CopyAction::SelectBlock),
    ("SwapEnds", CopyAction::SwapEnds),
    ("Yank", CopyAction::Yank),
    ("YankStay", CopyAction::YankStay),
    ("SearchForward", CopyAction::SearchForward),
    ("SearchBackward", CopyAction::SearchBackward),
    ("SearchNext", CopyAction::SearchNext),
    ("SearchPrev", CopyAction::SearchPrev),
    ("Cancel", CopyAction::Cancel),
    ("Exit", CopyAction::Exit),
];

impl CopyAction {
    pub fn from_name(name: &str) -> Option<Self> {
        ACTION_NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, a)| *a)
    }

    pub fn name(self) -> &'static str {
        ACTION_NAMES
            .iter()
            .find(|(_, a)| *a == self)
            .map(|(n, _)| *n)
            .unwrap_or("")
    }
}

/// Default bindings from the §10.5 table. A multi-key entry is a sequence (`g g`).
const DEFAULT_KEYS: &[(&str, CopyAction)] = &[
    ("h", CopyAction::Left),
    ("left", CopyAction::Left),
    ("l", CopyAction::Right),
    ("right", CopyAction::Right),
    ("k", CopyAction::Up),
    ("up", CopyAction::Up),
    ("j", CopyAction::Down),
    ("down", CopyAction::Down),
    ("w", CopyAction::WordForward),
    ("b", CopyAction::WordBackward),
    ("e", CopyAction::WordEnd),
    ("W", CopyAction::BigWordForward),
    ("B", CopyAction::BigWordBackward),
    ("E", CopyAction::BigWordEnd),
    ("0", CopyAction::LineStart),
    ("home", CopyAction::LineStart),
    ("^", CopyAction::FirstNonBlank),
    ("$", CopyAction::LineEnd),
    ("end", CopyAction::LineEnd),
    ("g g", CopyAction::Top),
    ("G", CopyAction::Bottom),
    ("H", CopyAction::ViewTop),
    ("M", CopyAction::ViewMiddle),
    ("L", CopyAction::ViewBottom),
    ("ctrl-u", CopyAction::HalfPageUp),
    ("ctrl-d", CopyAction::HalfPageDown),
    ("ctrl-b", CopyAction::PageUp),
    ("pageup", CopyAction::PageUp),
    ("ctrl-f", CopyAction::PageDown),
    ("pagedown", CopyAction::PageDown),
    ("{", CopyAction::ParagraphPrev),
    ("}", CopyAction::ParagraphNext),
    ("[", CopyAction::PromptPrev),
    ("]", CopyAction::PromptNext),
    ("v", CopyAction::SelectChar),
    ("V", CopyAction::SelectLine),
    ("ctrl-v", CopyAction::SelectBlock),
    ("o", CopyAction::SwapEnds),
    ("y", CopyAction::Yank),
    ("Y", CopyAction::YankStay),
    ("/", CopyAction::SearchForward),
    ("?", CopyAction::SearchBackward),
    ("n", CopyAction::SearchNext),
    ("N", CopyAction::SearchPrev),
    ("q", CopyAction::Exit),
    ("escape", CopyAction::Cancel),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Action(CopyAction),
    /// The keys so far begin a longer sequence.
    Prefix,
    None,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    entries: Vec<(Vec<String>, CopyAction)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            entries: DEFAULT_KEYS
                .iter()
                .map(|(k, a)| (k.split(' ').map(str::to_string).collect(), *a))
                .collect(),
        }
    }
}

/// The notation the keymap and [`CopyMode::key`] use for a config keystroke: `ctrl-u`, an
/// uppercase letter for shift+letter, otherwise the key name or character.
pub fn notation(ctrl: bool, shift: bool, key: &str) -> String {
    if ctrl {
        format!("ctrl-{key}")
    } else if shift && key.chars().count() == 1 && key.chars().all(|c| c.is_alphabetic()) {
        key.to_uppercase()
    } else {
        key.to_string()
    }
}

impl Keymap {
    /// Applies `[[copy_mode.keys]]` overrides in order: `Some(action name)` rebinds the key,
    /// `None` (`Disable`) unbinds it. Returns warnings for unknown action names.
    pub fn with_overrides(overrides: &[(String, Option<String>)]) -> (Self, Vec<String>) {
        let mut km = Self::default();
        let mut warnings = Vec::new();
        for (key, action) in overrides {
            let seq = vec![key.clone()];
            match action {
                None => km.entries.retain(|(k, _)| *k != seq),
                Some(name) => match CopyAction::from_name(name) {
                    Some(a) => {
                        km.entries.retain(|(k, _)| *k != seq);
                        km.entries.push((seq, a));
                    }
                    // A typo keeps the default binding.
                    None => warnings.push(format!(
                        "copy_mode.keys: unknown action \"{name}\" for key \"{key}\""
                    )),
                },
            }
        }
        (km, warnings)
    }

    pub fn lookup(&self, seq: &[String]) -> Lookup {
        if let Some((_, a)) = self.entries.iter().find(|(k, _)| k == seq) {
            return Lookup::Action(*a);
        }
        if self
            .entries
            .iter()
            .any(|(k, _)| k.len() > seq.len() && k[..seq.len()] == *seq)
        {
            return Lookup::Prefix;
        }
        Lookup::None
    }
}

/// Where the viewport sits, for `H M L` and page motions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub top: usize,
    pub rows: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Exit,
    Yank { text: String, exit: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPrompt {
    pub forward: bool,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct CopyMode {
    pub cursor: Pos,
    pub sel: Option<(SelKind, Pos)>,
    pub prompt: Option<SearchPrompt>,
    /// Numeric prefix being typed (`5j`).
    pub count: Option<usize>,
    pending: Vec<String>,
    /// Column `j`/`k` aim for.
    want_col: usize,
    last_search: Option<(String, bool)>,
    pub trim_trailing: bool,
}

impl CopyMode {
    pub fn new(cursor: Pos, trim_trailing: bool) -> Self {
        Self {
            cursor,
            sel: None,
            prompt: None,
            count: None,
            pending: Vec::new(),
            want_col: cursor.1,
            last_search: None,
            trim_trailing,
        }
    }

    /// Keys typed so far in a count or sequence (shown in the mode pill).
    pub fn pending_text(&self) -> String {
        let mut s = self.count.map(|c| c.to_string()).unwrap_or_default();
        s.push_str(&self.pending.join(""));
        s
    }

    pub fn mode_label(&self) -> &'static str {
        match self.sel {
            Some((SelKind::Char, _)) => "VISUAL",
            Some((SelKind::Line, _)) => "V-LINE",
            Some((SelKind::Block, _)) => "V-BLOCK",
            None => "COPY",
        }
    }

    /// Handles one key (see [`notation`]; plain characters arrive as themselves).
    pub fn key(
        &mut self,
        key: &str,
        keymap: &Keymap,
        grid: &TextGrid,
        marks: &[usize],
        view: View,
    ) -> Outcome {
        if grid.rows.is_empty() {
            return Outcome::None;
        }
        self.clamp(grid);
        if self.prompt.is_some() {
            return self.prompt_key(key, grid);
        }
        if self.pending.is_empty() {
            if let Some(d) = key.chars().next().and_then(|c| c.to_digit(10)) {
                if key.len() == 1 && (d != 0 || self.count.is_some()) {
                    let c = self
                        .count
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(d as usize);
                    self.count = Some(c.min(1_000_000));
                    return Outcome::None;
                }
            }
        }
        self.pending.push(key.to_string());
        match keymap.lookup(&self.pending) {
            Lookup::Prefix => Outcome::None,
            Lookup::None => {
                self.pending.clear();
                self.count = None;
                Outcome::None
            }
            Lookup::Action(a) => {
                self.pending.clear();
                let n = self.count.take().unwrap_or(1).max(1);
                self.perform(a, n, grid, marks, view)
            }
        }
    }

    fn prompt_key(&mut self, key: &str, grid: &TextGrid) -> Outcome {
        let Some(p) = self.prompt.as_mut() else {
            return Outcome::None;
        };
        match key {
            "escape" => self.prompt = None,
            "backspace" => {
                p.text.pop();
            }
            "space" => p.text.push(' '),
            "enter" => {
                let (text, forward) = (p.text.clone(), p.forward);
                self.prompt = None;
                if !text.is_empty() {
                    self.last_search = Some((text, forward));
                    self.search_step(grid, forward);
                }
            }
            k if k.chars().count() == 1 => p.text.push_str(k),
            _ => {}
        }
        Outcome::None
    }

    fn perform(
        &mut self,
        a: CopyAction,
        n: usize,
        grid: &TextGrid,
        marks: &[usize],
        view: View,
    ) -> Outcome {
        use CopyAction::*;
        let (top, end) = (grid.top, grid.end());
        let (row, col) = self.cursor;
        let cols = grid.cols.max(1);
        match a {
            Left => {
                self.set(grid, (row, col.saturating_sub(n)), false);
                self.want_col = self.cursor.1;
            }
            Right => {
                self.set(grid, (row, (col + n).min(cols - 1)), true);
                self.want_col = self.cursor.1;
            }
            Up => {
                let r = row.saturating_sub(n).max(top);
                self.set(grid, (r, self.col_for(grid, r)), false);
            }
            Down => {
                let r = (row + n).min(end - 1);
                self.set(grid, (r, self.col_for(grid, r)), false);
            }
            WordForward => self.word(grid, n, false, Dir::Fwd, Edge::Start),
            WordBackward => self.word(grid, n, false, Dir::Back, Edge::Start),
            WordEnd => self.word(grid, n, false, Dir::Fwd, Edge::End),
            BigWordForward => self.word(grid, n, true, Dir::Fwd, Edge::Start),
            BigWordBackward => self.word(grid, n, true, Dir::Back, Edge::Start),
            BigWordEnd => self.word(grid, n, true, Dir::Fwd, Edge::End),
            LineStart => {
                self.set(grid, (row, 0), true);
                self.want_col = 0;
            }
            FirstNonBlank => {
                let c = grid
                    .row(row)
                    .and_then(|r| r.chars.iter().position(|c| *c != ' ' && *c != '\0'))
                    .unwrap_or(0);
                self.set(grid, (row, c), true);
                self.want_col = self.cursor.1;
            }
            LineEnd => {
                let c = grid.row(row).and_then(|r| r.last_non_blank()).unwrap_or(0);
                self.set(grid, (row, c), false);
                // `$` sticks to the end of every line, like vi.
                self.want_col = usize::MAX;
            }
            Top => {
                // `5gg` goes to line 5 of the buffer.
                let r = if n > 1 { top + n - 1 } else { top };
                self.set(grid, (r.min(end - 1), 0), true);
                self.want_col = 0;
            }
            Bottom => {
                let r = if n > 1 { top + n - 1 } else { end - 1 };
                self.set(grid, (r.min(end - 1), 0), true);
                self.want_col = 0;
            }
            ViewTop => {
                let r = view.top.clamp(top, end - 1);
                self.set(grid, (r, self.col_for(grid, r)), false);
            }
            ViewMiddle => {
                let r = (view.top + view.rows.saturating_sub(1) / 2).clamp(top, end - 1);
                self.set(grid, (r, self.col_for(grid, r)), false);
            }
            ViewBottom => {
                let r = (view.top + view.rows.saturating_sub(1)).clamp(top, end - 1);
                self.set(grid, (r, self.col_for(grid, r)), false);
            }
            HalfPageUp => self.page(grid, n * (view.rows / 2).max(1), false),
            HalfPageDown => self.page(grid, n * (view.rows / 2).max(1), true),
            PageUp => self.page(grid, n * view.rows.max(1), false),
            PageDown => self.page(grid, n * view.rows.max(1), true),
            ParagraphNext => {
                let mut r = row;
                for _ in 0..n {
                    while r + 1 < end && blank(grid, r) {
                        r += 1;
                    }
                    while r + 1 < end && !blank(grid, r) {
                        r += 1;
                    }
                }
                self.set(grid, (r, 0), true);
                self.want_col = 0;
            }
            ParagraphPrev => {
                let mut r = row;
                for _ in 0..n {
                    while r > top && blank(grid, r) {
                        r -= 1;
                    }
                    while r > top && !blank(grid, r) {
                        r -= 1;
                    }
                }
                self.set(grid, (r, 0), true);
                self.want_col = 0;
            }
            PromptNext => {
                let mut r = row;
                for _ in 0..n {
                    match marks.iter().find(|m| **m > r) {
                        Some(m) => r = *m,
                        None => break,
                    }
                }
                if r != row {
                    self.set(grid, (r.min(end - 1), 0), true);
                    self.want_col = 0;
                }
            }
            PromptPrev => {
                let mut r = row;
                for _ in 0..n {
                    match marks.iter().rev().find(|m| **m < r) {
                        Some(m) => r = *m,
                        None => break,
                    }
                }
                if r != row {
                    self.set(grid, (r.max(top), 0), true);
                    self.want_col = 0;
                }
            }
            SelectChar => self.toggle(SelKind::Char),
            SelectLine => self.toggle(SelKind::Line),
            SelectBlock => self.toggle(SelKind::Block),
            SwapEnds => {
                if let Some((k, anchor)) = self.sel {
                    self.sel = Some((k, self.cursor));
                    self.cursor = anchor;
                    self.want_col = anchor.1;
                }
            }
            Yank | YankStay => {
                let exit = a == Yank;
                if let Some(text) = self.selection_text(grid) {
                    if !exit {
                        self.sel = None;
                    }
                    return Outcome::Yank { text, exit };
                }
            }
            SearchForward => {
                self.prompt = Some(SearchPrompt {
                    forward: true,
                    text: String::new(),
                });
            }
            SearchBackward => {
                self.prompt = Some(SearchPrompt {
                    forward: false,
                    text: String::new(),
                });
            }
            SearchNext | SearchPrev => {
                if let Some((_, fwd)) = self.last_search.clone() {
                    let forward = if a == SearchNext { fwd } else { !fwd };
                    for _ in 0..n {
                        self.search_step(grid, forward);
                    }
                }
            }
            Cancel => {
                if self.sel.is_some() {
                    self.sel = None;
                } else {
                    return Outcome::Exit;
                }
            }
            Exit => return Outcome::Exit,
        }
        Outcome::None
    }

    fn toggle(&mut self, kind: SelKind) {
        self.sel = match self.sel {
            Some((k, _)) if k == kind => None,
            Some((_, anchor)) => Some((kind, anchor)),
            None => Some((kind, self.cursor)),
        };
    }

    fn clamp(&mut self, grid: &TextGrid) {
        let r = self
            .cursor
            .0
            .clamp(grid.top, grid.end().saturating_sub(1).max(grid.top));
        self.set(grid, (r, self.cursor.1), false);
    }

    /// Moves to `p`, clamped to the grid and off wide-char spacers (`fwd` picks the side).
    fn set(&mut self, grid: &TextGrid, p: Pos, fwd: bool) {
        let row =
            p.0.clamp(grid.top, grid.end().saturating_sub(1).max(grid.top));
        let max_col = grid.cols.saturating_sub(1);
        let mut col = p.1.min(max_col);
        if let Some(r) = grid.row(row) {
            if r.chars.get(col) == Some(&'\0') {
                col = if fwd && col < max_col {
                    col + 1
                } else {
                    col.saturating_sub(1)
                };
            }
        }
        self.cursor = (row, col);
    }

    fn page(&mut self, grid: &TextGrid, rows: usize, down: bool) {
        let r = if down {
            (self.cursor.0 + rows).min(grid.end() - 1)
        } else {
            self.cursor.0.saturating_sub(rows).max(grid.top)
        };
        self.set(grid, (r, self.col_for(grid, r)), false);
    }

    /// The column `j`/`k` aim for on `row`.
    fn col_for(&self, grid: &TextGrid, row: usize) -> usize {
        if self.want_col == usize::MAX {
            grid.row(row).and_then(|r| r.last_non_blank()).unwrap_or(0)
        } else {
            self.want_col
        }
    }

    fn word(&mut self, grid: &TextGrid, n: usize, big: bool, dir: Dir, edge: Edge) {
        let lin = Lin { g: grid, big };
        let mut i = (self.cursor.0 - grid.top) * grid.cols.max(1) + self.cursor.1;
        for _ in 0..n {
            i = match (dir, edge) {
                (Dir::Fwd, Edge::Start) => lin.next_start(i),
                (Dir::Back, Edge::Start) => lin.prev_start(i),
                (Dir::Fwd, Edge::End) => lin.next_end(i),
                // `ge` is not in the table; a backward end is unused.
                (Dir::Back, Edge::End) => lin.prev_start(i),
            };
        }
        let cols = grid.cols.max(1);
        self.set(grid, (grid.top + i / cols, i % cols), false);
        self.want_col = self.cursor.1;
    }

    /// Moves to the next match of the last search from the cursor, wrapping around.
    fn search_step(&mut self, grid: &TextGrid, forward: bool) {
        let Some((query, _)) = self.last_search.clone() else {
            return;
        };
        let res = find_all(grid, &query, smart_case(&query), false);
        let cur = self.cursor;
        let starts: Vec<Pos> = res.matches.iter().map(|m| m.start()).collect();
        let hit = if forward {
            starts.iter().find(|s| **s > cur).or(starts.first())
        } else {
            starts.iter().rev().find(|s| **s < cur).or(starts.last())
        };
        if let Some(&p) = hit {
            self.set(grid, p, true);
            self.want_col = self.cursor.1;
        }
    }

    /// `(kind, start, end)` with `start <= end` in reading order (block: by line, then column).
    pub fn selection(&self) -> Option<(SelKind, Pos, Pos)> {
        let (kind, anchor) = self.sel?;
        let (a, b) = if anchor <= self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        Some((kind, a, b))
    }

    /// The selected text, `None` without a selection.
    pub fn selection_text(&self, grid: &TextGrid) -> Option<String> {
        let (kind, a, b) = self.selection()?;
        let trim = self.trim_trailing;
        let last_col = grid.cols.saturating_sub(1);
        let mut out = String::new();
        for r in a.0..=b.0 {
            let Some(row) = grid.row(r) else { continue };
            let (from, to) = match kind {
                SelKind::Line => (0, last_col),
                SelKind::Char => (
                    if r == a.0 { a.1 } else { 0 },
                    if r == b.0 { b.1 } else { last_col },
                ),
                SelKind::Block => {
                    let (x, y) = (a.1.min(b.1), a.1.max(b.1));
                    (x, y)
                }
            };
            // A wide char's spacer belongs to the selection of its base cell.
            let to = if row.chars.get(to + 1) == Some(&'\0') {
                to + 1
            } else {
                to
            };
            let from = if row.chars.get(from) == Some(&'\0') && from > 0 {
                from - 1
            } else {
                from
            };
            let joined = row.wrapped && kind != SelKind::Block && r != b.0;
            out.push_str(&row.text(from, to, trim && !joined));
            if r != b.0 && !joined {
                out.push('\n');
            }
        }
        Some(out)
    }
}

fn blank(grid: &TextGrid, abs: usize) -> bool {
    grid.row(abs).is_none_or(|r| r.is_blank())
}

#[derive(Clone, Copy)]
enum Dir {
    Fwd,
    Back,
}

#[derive(Clone, Copy)]
enum Edge {
    Start,
    End,
}

/// The grid as one flat sequence of cells for word motions.
struct Lin<'a> {
    g: &'a TextGrid,
    big: bool,
}

impl Lin<'_> {
    fn cols(&self) -> usize {
        self.g.cols.max(1)
    }

    fn len(&self) -> usize {
        self.g.rows.len() * self.cols()
    }

    fn raw(&self, i: usize) -> char {
        self.g.rows[i / self.cols()]
            .chars
            .get(i % self.cols())
            .copied()
            .unwrap_or(' ')
    }

    fn spacer(&self, i: usize) -> bool {
        self.raw(i) == '\0'
    }

    /// 0 blank, 1 word (or any non-blank for WORD motions), 2 punctuation.
    fn cls(&self, i: usize) -> u8 {
        let c = if self.spacer(i) && i > 0 {
            self.raw(i - 1)
        } else {
            self.raw(i)
        };
        if c == ' ' || c == '\0' {
            0
        } else if self.big || c.is_alphanumeric() || c == '_' {
            1
        } else {
            2
        }
    }

    /// A hard line end sits between `i - 1` and `i`.
    fn brk(&self, i: usize) -> bool {
        i % self.cols() == 0 && i > 0 && !self.g.rows[i / self.cols() - 1].wrapped
    }

    fn prev_base(&self, i: usize) -> Option<usize> {
        let mut j = i.checked_sub(1)?;
        if self.spacer(j) {
            j = j.checked_sub(1)?;
        }
        Some(j)
    }

    fn next_base(&self, i: usize) -> Option<usize> {
        let mut j = i + 1;
        if j < self.len() && self.spacer(j) {
            j += 1;
        }
        (j < self.len()).then_some(j)
    }

    fn is_start(&self, i: usize) -> bool {
        if self.spacer(i) || self.cls(i) == 0 {
            return false;
        }
        match self.prev_base(i) {
            None => true,
            Some(p) => self.brk(i) || self.cls(p) != self.cls(i),
        }
    }

    fn is_end(&self, i: usize) -> bool {
        if self.spacer(i) || self.cls(i) == 0 {
            return false;
        }
        match self.next_base(i) {
            None => true,
            Some(nx) => self.brk(nx) || self.cls(nx) != self.cls(i),
        }
    }

    /// Column 0 of a blank, un-wrapped row: vi stops there for `w` and `b`.
    fn empty_line(&self, i: usize) -> bool {
        let row = &self.g.rows[i / self.cols()];
        i % self.cols() == 0 && !row.wrapped && row.is_blank()
    }

    fn next_start(&self, i: usize) -> usize {
        (i + 1..self.len())
            .find(|&j| self.is_start(j) || self.empty_line(j))
            .unwrap_or_else(|| self.len() - 1)
    }

    fn prev_start(&self, i: usize) -> usize {
        (0..i)
            .rev()
            .find(|&j| self.is_start(j) || self.empty_line(j))
            .unwrap_or(0)
    }

    fn next_end(&self, i: usize) -> usize {
        (i + 1..self.len())
            .find(|&j| self.is_end(j))
            .unwrap_or_else(|| self.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: View = View { top: 0, rows: 4 };

    fn run(cm: &mut CopyMode, g: &TextGrid, keys: &[&str]) -> Outcome {
        run_marks(cm, g, &[], keys)
    }

    fn run_marks(cm: &mut CopyMode, g: &TextGrid, marks: &[usize], keys: &[&str]) -> Outcome {
        let km = Keymap::default();
        let mut last = Outcome::None;
        for k in keys {
            last = cm.key(k, &km, g, marks, V);
        }
        last
    }

    fn cm(pos: Pos) -> CopyMode {
        CopyMode::new(pos, true)
    }

    fn grid() -> TextGrid {
        TextGrid::from_strs(
            20,
            &[
                "foo bar.baz  qux",
                "  indented line",
                "",
                "second para",
                "tail",
            ],
        )
    }

    #[test]
    fn hjkl_and_arrows_clamp_at_edges() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["h", "k"]);
        assert_eq!(c.cursor, (0, 0));
        run(&mut c, &g, &["l", "l", "j"]);
        assert_eq!(c.cursor, (1, 2));
        run(&mut c, &g, &["left", "up", "down", "right"]);
        assert_eq!(c.cursor, (1, 2));
        run(&mut c, &g, &["j", "j", "j", "j", "j"]);
        assert_eq!(c.cursor.0, 4);
    }

    #[test]
    fn vertical_motion_remembers_the_column() {
        let g = grid();
        let mut c = cm((0, 10));
        run(&mut c, &g, &["j", "j"]);
        assert_eq!(c.cursor, (2, 10), "blank rows keep the column");
        run(&mut c, &g, &["k"]);
        assert_eq!(c.cursor, (1, 10));
    }

    #[test]
    fn numeric_prefixes() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["3", "l"]);
        assert_eq!(c.cursor, (0, 3));
        run(&mut c, &g, &["1", "0", "l"]);
        assert_eq!(c.cursor, (0, 13));
        run(&mut c, &g, &["2", "j"]);
        assert_eq!(c.cursor.0, 2);
        // `0` after a count is a digit, alone it is line start.
        run(&mut c, &g, &["0"]);
        assert_eq!(c.cursor.1, 0);
        run(&mut c, &g, &["x"]);
        assert_eq!(c.count, None, "an unbound key drops the count");
    }

    #[test]
    fn word_motions_split_on_punctuation() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (0, 4)); // bar
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (0, 7)); // .
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (0, 8)); // baz
        run(&mut c, &g, &["e"]);
        assert_eq!(c.cursor, (0, 10));
        run(&mut c, &g, &["b"]);
        assert_eq!(c.cursor, (0, 8));
        run(&mut c, &g, &["2", "w"]);
        assert_eq!(c.cursor, (1, 2), "w crosses to the next line's first word");
    }

    #[test]
    fn big_word_motions_use_blanks_only() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["W"]);
        assert_eq!(c.cursor, (0, 4));
        run(&mut c, &g, &["W"]);
        assert_eq!(c.cursor, (0, 13));
        run(&mut c, &g, &["B"]);
        assert_eq!(c.cursor, (0, 4));
        run(&mut c, &g, &["E"]);
        assert_eq!(c.cursor, (0, 10));
    }

    #[test]
    fn words_stop_at_empty_lines_and_never_join_hard_lines() {
        let g = TextGrid::from_strs(4, &["abcd", "efgh", "", "ij"]);
        let mut c = cm((0, 0));
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (1, 0), "abcd/efgh are separate words");
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (2, 0), "empty line is a stop");
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (3, 0));
        run(&mut c, &g, &["b", "b"]);
        assert_eq!(c.cursor, (1, 0));
    }

    #[test]
    fn words_flow_across_soft_wraps() {
        let g = TextGrid::from_strs(4, &["abcd", "efgh", "ij"]).wrap(0);
        let mut c = cm((0, 0));
        run(&mut c, &g, &["e"]);
        assert_eq!(c.cursor, (1, 3), "abcdefgh is one word");
        run(&mut c, &g, &["b"]);
        assert_eq!(c.cursor, (0, 0));
    }

    #[test]
    fn word_motions_over_wide_chars() {
        let g = TextGrid::from_strs(10, &["a 漢\0字\0 b"]);
        let mut c = cm((0, 0));
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (0, 2));
        run(&mut c, &g, &["e"]);
        assert_eq!(
            c.cursor,
            (0, 4),
            "ends on the base cell of the last wide char"
        );
        run(&mut c, &g, &["w"]);
        assert_eq!(c.cursor, (0, 7));
    }

    #[test]
    fn horizontal_motion_skips_wide_spacers() {
        let g = TextGrid::from_strs(6, &["漢\0字\0x"]);
        let mut c = cm((0, 0));
        run(&mut c, &g, &["l"]);
        assert_eq!(c.cursor, (0, 2));
        run(&mut c, &g, &["l"]);
        assert_eq!(c.cursor, (0, 4));
        run(&mut c, &g, &["h"]);
        assert_eq!(c.cursor, (0, 2));
    }

    #[test]
    fn line_motions() {
        let g = grid();
        let mut c = cm((1, 8));
        run(&mut c, &g, &["0"]);
        assert_eq!(c.cursor, (1, 0));
        run(&mut c, &g, &["^"]);
        assert_eq!(c.cursor, (1, 2));
        run(&mut c, &g, &["$"]);
        assert_eq!(c.cursor, (1, 14));
        let mut c = cm((2, 5));
        run(&mut c, &g, &["$"]);
        assert_eq!(c.cursor, (2, 0), "blank line");
    }

    #[test]
    fn dollar_sticks_to_line_ends() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["$", "j"]);
        assert_eq!(
            c.cursor,
            (1, 14),
            "lands on the last character of the next line"
        );
    }

    #[test]
    fn top_bottom_and_view_motions() {
        let g = grid();
        let mut c = cm((2, 3));
        run(&mut c, &g, &["g"]);
        assert_eq!(c.cursor, (2, 3), "g alone waits for a second key");
        run(&mut c, &g, &["g"]);
        assert_eq!(c.cursor, (0, 0));
        run(&mut c, &g, &["G"]);
        assert_eq!(c.cursor, (4, 0));
        let km = Keymap::default();
        let v = View { top: 1, rows: 3 };
        c.key("H", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 1);
        c.key("M", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 2);
        c.key("L", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 3);
        run(&mut c, &g, &["g", "x"]);
        assert_eq!(c.cursor.0, 3, "g x is unbound and resets");
        run(&mut c, &g, &["2", "G"]);
        assert_eq!(c.cursor.0, 1, "count G goes to that line");
    }

    #[test]
    fn page_motions() {
        let lines: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let g = TextGrid::from_strs(10, &refs);
        let km = Keymap::default();
        let v = View { top: 0, rows: 10 };
        let mut c = cm((0, 0));
        c.key("ctrl-d", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 5);
        c.key("ctrl-f", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 15);
        c.key("ctrl-u", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 10);
        c.key("ctrl-b", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 0);
        c.key("ctrl-b", &km, &g, &[], v);
        assert_eq!(c.cursor.0, 0);
        for _ in 0..8 {
            c.key("ctrl-f", &km, &g, &[], v);
        }
        assert_eq!(c.cursor.0, 49);
    }

    #[test]
    fn paragraph_motions() {
        let g = TextGrid::from_strs(8, &["a", "b", "", "", "c", "d", "", "e"]);
        let mut c = cm((0, 3));
        run(&mut c, &g, &["}"]);
        assert_eq!(c.cursor, (2, 0));
        run(&mut c, &g, &["}"]);
        assert_eq!(c.cursor, (6, 0), "skips the blank run and the next block");
        run(&mut c, &g, &["}"]);
        assert_eq!(c.cursor, (7, 0), "clamps at the end");
        run(&mut c, &g, &["{"]);
        assert_eq!(c.cursor, (6, 0));
        run(&mut c, &g, &["{"]);
        assert_eq!(c.cursor, (3, 0));
        run(&mut c, &g, &["2", "{"]);
        assert_eq!(c.cursor, (0, 0));
    }

    #[test]
    fn prompt_motions_use_marks_and_are_noops_without() {
        let g = TextGrid::from_strs(8, &["p1", "out", "p2", "out", "p3"]);
        let mut c = cm((1, 1));
        run_marks(&mut c, &g, &[], &["]"]);
        assert_eq!(c.cursor, (1, 1), "no marks: no-op");
        run_marks(&mut c, &g, &[0, 2, 4], &["]"]);
        assert_eq!(c.cursor, (2, 0));
        run_marks(&mut c, &g, &[0, 2, 4], &["]"]);
        assert_eq!(c.cursor, (4, 0));
        run_marks(&mut c, &g, &[0, 2, 4], &["]"]);
        assert_eq!(c.cursor, (4, 0));
        run_marks(&mut c, &g, &[0, 2, 4], &["["]);
        assert_eq!(c.cursor, (2, 0));
        run_marks(&mut c, &g, &[0, 2, 4], &["2", "["]);
        assert_eq!(c.cursor, (0, 0));
    }

    #[test]
    fn char_selection_and_yank() {
        let g = grid();
        let mut c = cm((0, 4));
        run(&mut c, &g, &["v", "e"]);
        assert_eq!(c.mode_label(), "VISUAL");
        assert_eq!(
            run(&mut c, &g, &["y"]),
            Outcome::Yank {
                text: "bar".into(),
                exit: true
            }
        );
    }

    #[test]
    fn char_selection_across_rows_and_o_swaps_ends() {
        let g = grid();
        let mut c = cm((0, 8));
        run(&mut c, &g, &["v", "j", "j"]);
        assert_eq!(c.selection().unwrap().1, (0, 8));
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["Y"]) else {
            panic!()
        };
        assert_eq!(text, "baz  qux\n  indented line\n");
        assert!(
            c.sel.is_none(),
            "Y stays in copy mode and clears the selection"
        );
        run(&mut c, &g, &["v", "k"]);
        run(&mut c, &g, &["o"]);
        assert_eq!(c.cursor, (2, 8));
        assert_eq!(c.sel.unwrap().1, (1, 8));
    }

    #[test]
    fn line_selection_takes_whole_rows() {
        let g = grid();
        let mut c = cm((0, 6));
        run(&mut c, &g, &["V", "j"]);
        assert_eq!(c.mode_label(), "V-LINE");
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "foo bar.baz  qux\n  indented line");
    }

    #[test]
    fn block_selection_yanks_a_rectangle_in_either_direction() {
        let g = TextGrid::from_strs(10, &["abcdef", "ghijkl", "mnopqr"]);
        let mut c = cm((0, 1));
        run(&mut c, &g, &["ctrl-v", "j", "j", "l", "l"]);
        assert_eq!(c.mode_label(), "V-BLOCK");
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "bcd\nhij\nnop");
        // Anchor lower-left, cursor upper-right.
        let mut c = cm((2, 1));
        run(&mut c, &g, &["ctrl-v", "k", "k", "l", "l"]);
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "bcd\nhij\nnop");
    }

    #[test]
    fn selection_kinds_toggle_and_switch() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["v"]);
        run(&mut c, &g, &["V"]);
        assert_eq!(c.sel.unwrap().0, SelKind::Line);
        run(&mut c, &g, &["V"]);
        assert!(c.sel.is_none());
        assert_eq!(run(&mut c, &g, &["y"]), Outcome::None, "nothing to yank");
    }

    #[test]
    fn yank_joins_soft_wraps_and_trims() {
        let g = TextGrid::from_strs(5, &["abcde", "fg   ", "xy"]).wrap(0);
        let mut c = cm((0, 0));
        run(&mut c, &g, &["V", "j", "j"]);
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "abcdefg\nxy");
        let mut c = CopyMode::new((0, 0), false);
        run(&mut c, &g, &["V", "j"]);
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "abcdefg   ");
    }

    #[test]
    fn yank_wide_chars_keeps_the_char_once() {
        let g = TextGrid::from_strs(8, &["a漢\0字\0b"]);
        let mut c = cm((0, 1));
        run(&mut c, &g, &["v", "l"]);
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "漢字");
    }

    #[test]
    fn selection_extends_into_scrollback_with_absolute_lines() {
        let mut g = TextGrid::from_strs(6, &["old1", "old2", "new1"]);
        g.top = 0;
        let mut c = cm((2, 0));
        run(&mut c, &g, &["v", "k", "k"]);
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "old1\nold2\nn");
    }

    #[test]
    fn escape_clears_selection_then_exits() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["v"]);
        assert_eq!(run(&mut c, &g, &["escape"]), Outcome::None);
        assert!(c.sel.is_none());
        assert_eq!(run(&mut c, &g, &["escape"]), Outcome::Exit);
        assert_eq!(run(&mut c, &g, &["q"]), Outcome::Exit);
    }

    #[test]
    fn slash_search_moves_to_the_next_match_and_n_repeats() {
        let g = TextGrid::from_strs(12, &["one foo", "two", "foo three", "Foo end"]);
        let mut c = cm((0, 0));
        run(&mut c, &g, &["/", "f", "o", "o", "enter"]);
        assert_eq!(c.cursor, (0, 4));
        assert!(c.prompt.is_none());
        run(&mut c, &g, &["n"]);
        assert_eq!(c.cursor, (2, 0));
        run(&mut c, &g, &["n"]);
        assert_eq!(c.cursor, (3, 0), "smart case: lowercase query matches Foo");
        run(&mut c, &g, &["n"]);
        assert_eq!(c.cursor, (0, 4), "wraps");
        run(&mut c, &g, &["N"]);
        assert_eq!(c.cursor, (3, 0));
    }

    #[test]
    fn question_mark_searches_backward() {
        let g = TextGrid::from_strs(12, &["foo 1", "foo 2", "x"]);
        let mut c = cm((2, 0));
        run(&mut c, &g, &["?", "f", "o", "o", "enter"]);
        assert_eq!(c.cursor, (1, 0));
        run(&mut c, &g, &["n"]);
        assert_eq!(c.cursor, (0, 0), "n continues backward");
        run(&mut c, &g, &["N"]);
        assert_eq!(c.cursor, (1, 0));
    }

    #[test]
    fn search_prompt_editing_and_cancel() {
        let g = grid();
        let mut c = cm((0, 0));
        run(&mut c, &g, &["/", "a", "b", "backspace"]);
        assert_eq!(c.prompt.as_ref().unwrap().text, "a");
        run(&mut c, &g, &["space", "escape"]);
        assert!(c.prompt.is_none());
        assert_eq!(c.cursor, (0, 0));
        // Typing in the prompt never triggers motions.
        run(&mut c, &g, &["/", "j"]);
        assert_eq!(c.cursor, (0, 0));
    }

    #[test]
    fn search_works_in_scrollback_for_selection_extension() {
        let g = TextGrid::from_strs(10, &["alpha", "beta", "gamma"]);
        let mut c = cm((0, 0));
        run(&mut c, &g, &["v", "/", "g", "enter"]);
        assert_eq!(c.cursor, (2, 0));
        let Outcome::Yank { text, .. } = run(&mut c, &g, &["y"]) else {
            panic!()
        };
        assert_eq!(text, "alpha\nbeta\ng");
    }

    #[test]
    fn overrides_rebind_and_disable() {
        let (km, warn) = Keymap::with_overrides(&[
            ("x".into(), Some("MoveDown".into())),
            ("j".into(), None),
            ("h".into(), Some("Nonsense".into())),
        ]);
        assert_eq!(warn.len(), 1);
        let g = grid();
        let mut c = cm((0, 0));
        c.key("x", &km, &g, &[], V);
        assert_eq!(c.cursor.0, 1);
        c.key("j", &km, &g, &[], V);
        assert_eq!(c.cursor.0, 1, "disabled");
        c.cursor = (0, 3);
        c.key("h", &km, &g, &[], V);
        assert_eq!(c.cursor.1, 2, "an unknown action keeps the default binding");
    }

    #[test]
    fn notation_and_action_names_round_trip() {
        assert_eq!(notation(true, false, "u"), "ctrl-u");
        assert_eq!(notation(false, true, "g"), "G");
        assert_eq!(notation(false, false, "$"), "$");
        assert_eq!(notation(false, false, "left"), "left");
        for (name, a) in ACTION_NAMES {
            assert_eq!(CopyAction::from_name(name), Some(*a));
            assert_eq!(a.name(), *name);
        }
        // Every action is reachable from some default key.
        let km = Keymap::default();
        for (_, a) in ACTION_NAMES {
            assert!(
                km.entries.iter().any(|(_, x)| x == a),
                "{a:?} has no default key"
            );
        }
    }

    #[test]
    fn pending_text_shows_count_and_prefix() {
        let g = grid();
        let km = Keymap::default();
        let mut c = cm((0, 0));
        c.key("1", &km, &g, &[], V);
        c.key("2", &km, &g, &[], V);
        c.key("g", &km, &g, &[], V);
        assert_eq!(c.pending_text(), "12g");
    }
}
