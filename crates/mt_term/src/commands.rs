//! Per-command bookkeeping from OSC 133 shell-integration marks (SPEC §12.4).
//!
//! A shell with integration emits `A` (prompt start), `B` (input start), `C` (output start) and
//! `D;status` (finished), plus OSC 7 for the working directory. [`CommandTracker`] records one
//! [`Rec`] per command in a ring of the last [`MAX_COMMANDS`], anchored to grid lines so the
//! output of a command can be cut out of scrollback later (`get-text --last-command`, `pane run
//! --in`, `pane wait --command-finished`).
//!
//! Anchors are exact because marks are applied *between* parser calls: [`drive`] splits every
//! chunk right after each mark and records the cursor there. Anchors count in a stream-wide line
//! id (`trimmed + absolute line`), so history trimming never moves them. The engine would
//! otherwise stop growing its history at the scrollback limit and silently shift lines; we give
//! it [`HISTORY_SLACK`] spare lines and trim back to the configured limit ourselves, after
//! every [`SUB_CHUNK`] bytes, which keeps an exact count of the lines that left.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Term, TermMode};

use crate::lines::line_cells;

/// Commands kept per pane.
pub const MAX_COMMANDS: usize = 500;
const MAX_PROMPTS: usize = 5000;
/// Parser input is cut into pieces of at most this many bytes so that the history can be
/// trimmed between them (one line scrolls per byte at most).
pub(crate) const SUB_CHUNK: usize = 4096;
/// Extra history the engine may hold above the configured scrollback between two trims.
pub(crate) const HISTORY_SLACK: usize = 8192;

/// One decoded sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    /// `133;A`. `secondary` for continuation prompts (`k=s`/`k=c`), which are ignored.
    PromptStart { secondary: bool },
    /// `133;B`
    InputStart,
    /// `133;C`
    OutputStart,
    /// `133;D[;status]`
    Finished(Option<i32>),
    /// Like `Finished`, from a replay stream: restores the command without counting it as new.
    Restored(Option<i32>),
    /// OSC 7 working directory (a path, already percent-decoded).
    Cwd(String),
    /// `ESC c` (RIS): the screen and scrollback are gone, and with them every anchor.
    Reset,
    /// `CSI 3 J`: the program cleared the scrollback (what `clear` sends).
    ClearHistory,
    /// `OSC 7717;menu;…`: a shell-registered menu item (see `menu_reg.rs`).
    MenuReg(crate::menu_reg::RegCmd),
    /// A sequence the engine does not model (colours, OSC 9, OSC 22, XTSHIFTESCAPE): see `vt.rs`.
    Vt(crate::vt::VtSeq),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SniffState {
    Ground,
    Esc,
    Osc,
    OscEsc,
    /// After `ESC [`.
    Csi,
    /// After `ESC [ 3`.
    Csi3,
    /// After `ESC [ >`, collecting parameters.
    CsiGt,
    /// After `ESC [ ?`, collecting parameters (DECSET/DECRST/DECRQM).
    CsiQ,
    /// After `ESC [ ? Pm $`.
    CsiQDollar,
}

/// Finds OSC 133 and OSC 7 sequences across arbitrary chunk boundaries.
#[derive(Clone, Debug)]
pub struct OscSniffer {
    state: SniffState,
    buf: Vec<u8>,
    overflow: bool,
}

impl Default for OscSniffer {
    fn default() -> Self {
        OscSniffer {
            state: SniffState::Ground,
            buf: Vec::new(),
            overflow: false,
        }
    }
}

/// Longest payload worth buffering (a `file://` URL for OSC 7 is the largest legitimate one).
const MAX_PAYLOAD: usize = 4096;

impl OscSniffer {
    /// Marks in `bytes`, each with the offset just past its terminator.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Mark)> {
        let mut out = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            match self.state {
                SniffState::Ground => {
                    if b == 0x1b {
                        self.state = SniffState::Esc;
                    }
                }
                SniffState::Esc => match b {
                    b']' => self.begin(),
                    b'[' => self.state = SniffState::Csi,
                    b'c' => {
                        out.push((i + 1, Mark::Reset));
                        self.state = SniffState::Ground;
                    }
                    0x1b => {}
                    _ => self.state = SniffState::Ground,
                },
                SniffState::Csi => match b {
                    b'3' => self.state = SniffState::Csi3,
                    b'>' => {
                        self.buf.clear();
                        self.state = SniffState::CsiGt;
                    }
                    b'?' => {
                        self.buf.clear();
                        self.state = SniffState::CsiQ;
                    }
                    0x1b => self.state = SniffState::Esc,
                    _ => self.state = SniffState::Ground,
                },
                SniffState::Csi3 => match b {
                    b'J' => {
                        out.push((i + 1, Mark::ClearHistory));
                        self.state = SniffState::Ground;
                    }
                    0x1b => self.state = SniffState::Esc,
                    _ => self.state = SniffState::Ground,
                },
                SniffState::CsiGt => match b {
                    b'0'..=b'9' | b';' if self.buf.len() < 8 => self.buf.push(b),
                    b's' => {
                        self.state = SniffState::Ground;
                        let n = std::str::from_utf8(&self.buf)
                            .ok()
                            .and_then(|p| p.split(';').next().unwrap_or("").parse::<u8>().ok())
                            .or(if self.buf.is_empty() { Some(0) } else { None });
                        if let Some(n) = n {
                            out.push((i + 1, Mark::Vt(crate::vt::VtSeq::ShiftEscape(n))));
                        }
                    }
                    b'q' => {
                        self.state = SniffState::Ground;
                        // XTVERSION: `CSI > q` or `CSI > 0 q`.
                        if self.buf.is_empty() || self.buf == b"0" {
                            out.push((i + 1, Mark::Vt(crate::vt::VtSeq::Xtversion)));
                        }
                    }
                    0x1b => self.state = SniffState::Esc,
                    _ => self.state = SniffState::Ground,
                },
                SniffState::CsiQ => match b {
                    b'0'..=b'9' | b';' if self.buf.len() < 64 => self.buf.push(b),
                    b'h' | b'l' => {
                        self.state = SniffState::Ground;
                        if self.buf.split(|c| *c == b';').any(|p| p == b"1016") {
                            out.push((i + 1, Mark::Vt(crate::vt::VtSeq::PixelMouse(b == b'h'))));
                        }
                    }
                    b'$' => self.state = SniffState::CsiQDollar,
                    0x1b => self.state = SniffState::Esc,
                    _ => self.state = SniffState::Ground,
                },
                SniffState::CsiQDollar => match b {
                    b'p' => {
                        self.state = SniffState::Ground;
                        if self.buf == b"1016" {
                            out.push((i + 1, Mark::Vt(crate::vt::VtSeq::QueryPixelMouse)));
                        }
                    }
                    0x1b => self.state = SniffState::Esc,
                    _ => self.state = SniffState::Ground,
                },
                SniffState::Osc => match b {
                    0x07 => self.finish(i + 1, false, &mut out),
                    0x1b => self.state = SniffState::OscEsc,
                    0x18 | 0x1a => self.state = SniffState::Ground,
                    _ => {
                        if self.buf.len() < crate::vt::MAX_OSC_PAYLOAD {
                            self.buf.push(b);
                        } else {
                            self.overflow = true;
                        }
                    }
                },
                SniffState::OscEsc => match b {
                    b'\\' => self.finish(i + 1, true, &mut out),
                    b']' => self.begin(),
                    0x1b => {}
                    _ => self.state = SniffState::Ground,
                },
            }
        }
        out
    }

    fn begin(&mut self) {
        self.state = SniffState::Osc;
        self.buf.clear();
        self.overflow = false;
    }

    fn finish(&mut self, end: usize, st: bool, out: &mut Vec<(usize, Mark)>) {
        self.state = SniffState::Ground;
        if !self.overflow {
            if let Some(m) = parse_payload(&self.buf, st) {
                out.push((end, m));
            }
        }
        self.buf.clear();
    }
}

fn parse_payload(p: &[u8], st: bool) -> Option<Mark> {
    if let Some(rest) = p.strip_prefix(b"7717;") {
        return crate::menu_reg::parse(rest).map(Mark::MenuReg);
    }
    // Marks carry short payloads; anything longer is not one of ours.
    if p.len() > MAX_PAYLOAD && (p.starts_with(b"133;") || p.starts_with(b"7;")) {
        return None;
    }
    if !p.starts_with(b"133;") && !p.starts_with(b"7;") {
        let end = p.iter().position(|b| *b == b';').unwrap_or(p.len());
        let code = std::str::from_utf8(&p[..end]).ok()?.parse::<u16>().ok()?;
        if !crate::vt::wants_osc(code) {
            return None;
        }
        let payload = String::from_utf8_lossy(p.get(end + 1..).unwrap_or(&[])).into_owned();
        return Some(Mark::Vt(crate::vt::VtSeq::Osc { code, payload, st }));
    }
    if let Some(rest) = p.strip_prefix(b"133;") {
        let s = std::str::from_utf8(rest).ok()?;
        let mut parts = s.split(';');
        return match parts.next()? {
            "A" => Some(Mark::PromptStart {
                secondary: parts.any(|o| o == "k=s" || o == "k=c"),
            }),
            "B" => Some(Mark::InputStart),
            "C" => Some(Mark::OutputStart),
            "D" => {
                let status = parts.next().and_then(|s| s.trim().parse::<i32>().ok());
                if parts.any(|o| o == "replay") {
                    Some(Mark::Restored(status))
                } else {
                    Some(Mark::Finished(status))
                }
            }
            _ => None,
        };
    }
    if let Some(rest) = p.strip_prefix(b"7;") {
        let s = std::str::from_utf8(rest).ok()?;
        let after = s.strip_prefix("file://")?;
        // `file://host/path`: the host may be empty.
        let path = &after[after.find('/')?..];
        return Some(Mark::Cwd(percent_decode(path)));
    }
    None
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    /// `trimmed + absolute line` at the time of the mark.
    id: u64,
    col: usize,
}

#[derive(Clone, Debug)]
struct Rec {
    seq: u64,
    prompt: Option<Anchor>,
    input: Option<Anchor>,
    output: Option<Anchor>,
    end: Option<Anchor>,
    exit_code: Option<i32>,
    command: String,
    cwd: Option<String>,
    started_ms: u64,
    finished_ms: Option<u64>,
}

impl Rec {
    fn new(seq: u64) -> Rec {
        Rec {
            seq,
            prompt: None,
            input: None,
            output: None,
            end: None,
            exit_code: None,
            command: String::new(),
            cwd: None,
            started_ms: 0,
            finished_ms: None,
        }
    }

    fn open(&self) -> bool {
        self.end.is_none()
    }

    fn latest(&self) -> Option<u64> {
        [self.prompt, self.input, self.output, self.end]
            .into_iter()
            .flatten()
            .map(|a| a.id)
            .max()
    }

    fn earliest(&self) -> Option<u64> {
        [self.prompt, self.input, self.output, self.end]
            .into_iter()
            .flatten()
            .map(|a| a.id)
            .min()
    }
}

/// A finished command with its output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    /// Monotonic per pane, starting at 1.
    pub seq: u64,
    pub command: String,
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    /// The rendered output between `C` and `D`.
    pub output: String,
    /// The start of the output scrolled out of the history, so `output` is only its tail.
    pub truncated: bool,
    pub started_ms: u64,
    pub finished_ms: u64,
}

/// What a replay stream has to say about one command (see `replay.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplayMark {
    pub mark: Mark,
    /// Absolute line (0 = oldest history row) and column.
    pub line: usize,
    pub col: usize,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug)]
pub struct CommandTracker {
    cmds: VecDeque<Rec>,
    /// Stream ids of every prompt start, for `ScrollToPrompt` and copy mode.
    prompts: VecDeque<u64>,
    /// Lines that left the top of the history for good.
    trimmed: u64,
    /// History length after the last parser call, to notice clears.
    observed: usize,
    cap: usize,
    cols: usize,
    cwd: Option<String>,
    menus: crate::menu_reg::MenuRegistry,
    seen: bool,
    next_seq: u64,
    finished_total: u64,
}

impl CommandTracker {
    pub fn new(scrollback: usize, cols: usize) -> Self {
        CommandTracker {
            cmds: VecDeque::new(),
            prompts: VecDeque::new(),
            trimmed: 0,
            observed: 0,
            cap: scrollback,
            cols,
            cwd: None,
            menus: Default::default(),
            seen: false,
            next_seq: 1,
            finished_total: 0,
        }
    }

    /// Whether the shell has ever sent a mark (shell integration is active).
    pub fn has_marks(&self) -> bool {
        self.seen
    }

    /// Number of `D` marks that finished a command so far.
    pub fn finished_total(&self) -> u64 {
        self.finished_total
    }

    /// A command is running: `C` seen, `D` not yet.
    pub fn busy(&self) -> bool {
        self.cmds
            .back()
            .is_some_and(|r| r.open() && r.output.is_some())
    }

    /// The menu items shell programs registered for this pane (SPEC §10.9.7).
    pub fn menus(&self) -> &crate::menu_reg::MenuRegistry {
        &self.menus
    }

    /// Drops the registered menu items (the process exited). Returns whether any existed.
    pub fn clear_menus(&mut self) -> bool {
        self.menus.reset()
    }

    /// The last OSC 7 working directory.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    /// Changes the scrollback limit the tracker trims to.
    pub fn set_scrollback(&mut self, cap: usize) {
        self.cap = cap;
    }

    /// Call after the engine resized. Re-wrapped lines invalidate every anchor.
    pub fn after_resize<T>(&mut self, term: &Term<T>) {
        self.observed = term.grid().history_size();
        let cols = term.columns();
        if cols != self.cols {
            self.cols = cols;
            self.cmds.clear();
        }
    }

    /// Call after dropping scrollback by other means than the parser.
    pub fn history_cleared<T>(&mut self, term: &Term<T>) {
        self.after_segment_inner(term.grid().history_size(), None::<&mut Term<T>>);
    }

    fn evict(&mut self) {
        // Drop a command once every one of its anchors left the history.
        while self
            .cmds
            .front()
            .is_some_and(|r| !r.open() && r.latest().is_some_and(|id| id < self.trimmed))
        {
            self.cmds.pop_front();
        }
    }

    /// Accounts for history changes after a parser call and trims it to the limit.
    pub(crate) fn after_segment<T>(&mut self, term: &mut Term<T>) {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return; // the alternate grid has no history
        }
        let h = term.grid().history_size();
        self.after_segment_inner(h, Some(term));
    }

    fn after_segment_inner<T>(&mut self, mut h: usize, term: Option<&mut Term<T>>) {
        if h < self.observed {
            self.trimmed += (self.observed - h) as u64; // cleared by the program
        }
        if h > self.cap {
            let extra = h - self.cap;
            if let Some(term) = term {
                let g = term.grid_mut();
                g.update_history(self.cap);
                g.update_history(self.cap + HISTORY_SLACK);
                self.trimmed += extra as u64;
                h = self.cap;
            }
        }
        self.observed = h;
        self.evict();
    }

    fn anchor<T>(&self, term: &Term<T>) -> Anchor {
        let g = term.grid();
        let abs = (g.cursor.point.line.0 + g.history_size() as i32).max(0) as u64;
        Anchor {
            id: self.trimmed + abs,
            col: g.cursor.point.column.0,
        }
    }

    fn abs(&self, a: Anchor) -> Option<usize> {
        a.id.checked_sub(self.trimmed).map(|v| v as usize)
    }

    fn push_rec(&mut self) -> &mut Rec {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.cmds.push_back(Rec::new(seq));
        while self.cmds.len() > MAX_COMMANDS {
            self.cmds.pop_front();
        }
        self.cmds.back_mut().unwrap()
    }

    fn open_rec(&mut self) -> Option<&mut Rec> {
        self.cmds.back_mut().filter(|r| r.open())
    }

    /// Applies one mark with the parser stopped right after it. Returns `Some(exit_code)` when a
    /// command finished.
    pub(crate) fn apply<T>(&mut self, term: &Term<T>, mark: Mark) -> Option<Option<i32>> {
        if let Mark::Cwd(p) = mark {
            self.cwd = Some(p);
            return None;
        }
        if matches!(mark, Mark::Vt(_) | Mark::MenuReg(_) | Mark::ClearHistory) {
            return None;
        }
        if mark == Mark::Reset {
            self.cmds.clear();
            self.prompts.clear();
            return None;
        }
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        self.seen = true;
        let restored = matches!(mark, Mark::Restored(_));
        let at = self.anchor(term);
        match mark {
            Mark::Cwd(_) | Mark::Reset | Mark::Vt(_) | Mark::MenuReg(_) | Mark::ClearHistory => {}
            Mark::PromptStart { secondary: true } => {}
            Mark::PromptStart { .. } => {
                if self.prompts.back() != Some(&at.id) {
                    self.prompts.push_back(at.id);
                    while self.prompts.len() > MAX_PROMPTS {
                        self.prompts.pop_front();
                    }
                }
                // A redrawn prompt replaces a command that never started.
                if self.open_rec().is_some_and(|r| r.output.is_none()) {
                    let r = self.cmds.back_mut().unwrap();
                    *r = Rec::new(r.seq);
                    r.prompt = Some(at);
                } else {
                    self.push_rec().prompt = Some(at);
                }
            }
            Mark::InputStart => {
                if self.open_rec().is_none() {
                    self.push_rec();
                }
                let r = self.cmds.back_mut().unwrap();
                // Only the first `B` counts: continuation prompts send their own.
                if r.output.is_none() && r.input.is_none() {
                    r.input = Some(at);
                }
            }
            Mark::OutputStart => {
                if self.open_rec().is_none() {
                    self.push_rec();
                }
                let cwd = self.cwd.clone();
                let command = {
                    let r = self.cmds.back().unwrap();
                    r.input
                        .and_then(|b| self.abs(b).map(|l| (l, b.col)))
                        .and_then(|from| {
                            let to = (self.abs(at)?, at.col);
                            Some(region_text(term, from, to).trim().to_string())
                        })
                        .unwrap_or_default()
                };
                let r = self.cmds.back_mut().unwrap();
                r.output = Some(at);
                r.cwd = cwd;
                r.command = command;
                r.started_ms = now_ms();
            }
            Mark::Finished(code) | Mark::Restored(code) => {
                // A `D` before any command (shells send one before the first prompt) is noise.
                let ok = self.open_rec().is_some_and(|r| r.output.is_some());
                if !ok {
                    return None;
                }
                let r = self.cmds.back_mut().unwrap();
                r.end = Some(at);
                r.exit_code = code;
                r.finished_ms = Some(now_ms());
                if restored {
                    return None;
                }
                self.finished_total += 1;
                return Some(code);
            }
        }
        None
    }

    /// Absolute lines of prompt starts, ascending.
    pub fn prompt_lines(&self) -> Vec<usize> {
        self.prompts
            .iter()
            .filter_map(|id| id.checked_sub(self.trimmed))
            .map(|l| l as usize)
            .collect()
    }

    fn info<T>(&self, term: &Term<T>, r: &Rec) -> Option<CommandInfo> {
        let (out, end) = (r.output?, r.end?);
        let (from, truncated) = match self.abs(out) {
            Some(l) => ((l, out.col), false),
            None => ((0, 0), true),
        };
        let to = (self.abs(end)?, end.col);
        Some(CommandInfo {
            seq: r.seq,
            command: r.command.clone(),
            exit_code: r.exit_code,
            cwd: r.cwd.clone(),
            output: region_text(term, from, to),
            truncated,
            started_ms: r.started_ms,
            finished_ms: r.finished_ms.unwrap_or(r.started_ms),
        })
    }

    /// The `n`-th most recent finished command (`1` = latest).
    pub fn finished<T>(&self, term: &Term<T>, n: usize) -> Option<CommandInfo> {
        self.cmds
            .iter()
            .rev()
            .filter(|r| r.end.is_some() && r.output.is_some())
            .nth(n.checked_sub(1)?)
            .and_then(|r| self.info(term, r))
    }

    /// The finished command with this sequence number, if still retained.
    pub fn finished_seq<T>(&self, term: &Term<T>, seq: u64) -> Option<CommandInfo> {
        self.cmds
            .iter()
            .find(|r| r.seq == seq && r.end.is_some())
            .and_then(|r| self.info(term, r))
    }

    /// Sequence number the next command will get; `run --in` remembers it before typing.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The running command's output so far (`C` to the cursor), for partial results on timeout.
    pub fn running_output<T>(&self, term: &Term<T>) -> Option<String> {
        let r = self.cmds.back().filter(|r| r.open())?;
        let out = r.output?;
        let g = term.grid();
        let cur = (
            (g.cursor.point.line.0 + g.history_size() as i32).max(0) as usize,
            g.cursor.point.column.0,
        );
        let from = self.abs(out).map_or((0, 0), |l| (l, out.col));
        Some(region_text(term, from, cur))
    }

    /// Marks to re-emit, oldest first, for lines `>= first_line` (`replay.rs`).
    pub(crate) fn replay_marks(&self, first_line: usize) -> Vec<ReplayMark> {
        let mut v = Vec::new();
        for r in &self.cmds {
            let first = r
                .earliest()
                .and_then(|id| id.checked_sub(self.trimmed))
                .map(|l| l as usize);
            if first.is_none_or(|l| l < first_line) {
                continue;
            }
            let mut parts = vec![
                (Mark::PromptStart { secondary: false }, r.prompt),
                (Mark::InputStart, r.input),
            ];
            if let Some(cwd) = &r.cwd {
                // The working directory is read when the command starts.
                parts.push((Mark::Cwd(cwd.clone()), r.output));
            }
            parts.push((Mark::OutputStart, r.output));
            parts.push((Mark::Finished(r.exit_code), r.end));
            for (mark, a) in parts {
                if let Some(a) = a {
                    if let Some(line) = self.abs(a) {
                        v.push(ReplayMark {
                            mark,
                            line,
                            col: a.col,
                        });
                    }
                }
            }
        }
        // Stable: marks at the same cell (a `D` and the next `A`) keep their order.
        v.sort_by_key(|m| (m.line, m.col));
        v
    }
}

/// Text from `from` up to (excluding) `to`, both `(absolute line, column)`. Soft-wrapped rows
/// are joined; each hard-wrapped row loses its trailing blanks and ends in `\n`.
pub(crate) fn region_text<T>(term: &Term<T>, from: (usize, usize), to: (usize, usize)) -> String {
    let mut out = String::new();
    if to < from {
        return out;
    }
    for abs in from.0..=to.0 {
        let Some(cells) = line_cells(term, abs) else {
            break;
        };
        let start = if abs == from.0 { from.1 } else { 0 };
        let end = if abs == to.0 { to.1 } else { cells.chars.len() };
        let end = end.min(cells.chars.len());
        let start = start.min(end);
        let seg: String = cells.chars[start..end]
            .iter()
            .filter(|c| **c != '\0')
            .collect();
        if abs == to.0 {
            out.push_str(seg.trim_end());
        } else if cells.wrapped {
            out.push_str(&seg);
        } else {
            out.push_str(seg.trim_end());
            out.push('\n');
        }
    }
    out
}

/// Feeds `bytes` to `parse`, stopping after every mark (to apply it) and every [`SUB_CHUNK`]
/// bytes (to trim history). `on_finished` runs for each `D`.
pub(crate) fn drive<T: alacritty_terminal::event::EventListener>(
    osc: &mut OscSniffer,
    tracker: &mut CommandTracker,
    shared: &crate::events::Shared,
    term: &mut Term<T>,
    bytes: &[u8],
    mut parse: impl FnMut(&mut Term<T>, &[u8]),
    mut on_finished: impl FnMut(Option<i32>),
) {
    let marks = osc.feed(bytes);
    let mut start = 0;
    let mut mi = 0;
    while start < bytes.len() {
        let mark_end = marks.get(mi).map(|(e, _)| *e);
        let end = (start + SUB_CHUNK)
            .min(bytes.len())
            .min(mark_end.unwrap_or(usize::MAX));
        parse(term, &bytes[start..end]);
        tracker.after_segment(term);
        crate::vt::after_segment(term, shared);
        if mark_end == Some(end) {
            let mark = marks[mi].1.clone();
            match &mark {
                Mark::Vt(seq) => crate::vt::apply(term, shared, seq.clone()),
                Mark::Reset => {
                    crate::vt::reset(term, shared);
                    if tracker.menus.reset() {
                        shared.notify(crate::TermEvent::MenuItemsChanged);
                    }
                }
                Mark::ClearHistory => {
                    if tracker.menus.reset() {
                        shared.notify(crate::TermEvent::MenuItemsChanged);
                    }
                }
                Mark::MenuReg(cmd) => {
                    if tracker.menus.apply(cmd.clone(), now_ms()) {
                        shared.notify(crate::TermEvent::MenuItemsChanged);
                    }
                }
                _ => {}
            }
            if let Some(done) = tracker.apply(term, mark) {
                on_finished(done);
            }
            mi += 1;
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    fn mk(payload: &str) -> Vec<u8> {
        format!("\x1b]{payload}\x07").into_bytes()
    }

    #[test]
    fn sniffer_decodes_marks_with_either_terminator_across_chunks() {
        let mut s = OscSniffer::default();
        assert!(s.feed(b"ab\x1b]13").is_empty());
        let m = s.feed(b"3;A\x07x\x1b]133;D;7\x1b\\");
        assert_eq!(
            m,
            vec![
                (4, Mark::PromptStart { secondary: false }),
                (16, Mark::Finished(Some(7)))
            ]
        );
        // Split inside the ST terminator.
        assert!(s.feed(b"\x1b]133;C\x1b").is_empty());
        assert_eq!(s.feed(b"\\"), vec![(1, Mark::OutputStart)]);
    }

    #[test]
    fn sniffer_reads_status_secondary_prompts_and_cwd() {
        let mut s = OscSniffer::default();
        let m = s.feed(b"\x1b]133;D\x07\x1b]133;D;\x07\x1b]133;D;-1\x07\x1b]133;A;k=s\x07");
        let marks: Vec<_> = m.into_iter().map(|(_, m)| m).collect();
        assert_eq!(
            marks,
            vec![
                Mark::Finished(None),
                Mark::Finished(None),
                Mark::Finished(Some(-1)),
                Mark::PromptStart { secondary: true }
            ]
        );
        let m = s.feed(b"\x1b]7;file://host/tmp/a%20b/c\x07\x1b]7;http://x\x07\x1b]0;title\x07");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1, Mark::Cwd("/tmp/a b/c".into()));
    }

    #[test]
    fn sniffer_survives_garbage_overlong_and_aborted_sequences() {
        let mut s = OscSniffer::default();
        let mut long = b"\x1b]133;A".to_vec();
        long.extend(std::iter::repeat_n(b'x', MAX_PAYLOAD + 10));
        long.push(0x07);
        assert!(s.feed(&long).is_empty());
        // An aborted OSC followed by a real one.
        let m = s.feed(b"\x1b]133;\x18\x1b]133;B\x07");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1, Mark::InputStart);
        // A new OSC started by ESC ] inside another one wins.
        let m = s.feed(b"\x1b]0;ti\x1b]133;C\x07");
        assert_eq!(m.len(), 1);
    }

    fn session(e: &mut Emulator, cmd: &str, out: &str, status: i32) {
        e.advance(&mk("133;A"));
        e.advance(b"$ ");
        e.advance(&mk("133;B"));
        e.advance(cmd.as_bytes());
        e.advance(b"\r\n");
        e.advance(&mk("133;C"));
        e.advance(out.as_bytes());
        e.advance(&mk(&format!("133;D;{status}")));
    }

    #[test]
    fn one_command_yields_text_output_status_and_event() {
        let mut e = Emulator::new(40, 6, 100);
        assert!(!e.has_marks());
        session(&mut e, "echo hi", "hi\r\n", 0);
        assert!(e.has_marks());
        let c = e.last_command(1).unwrap();
        assert_eq!(c.command, "echo hi");
        assert_eq!(c.output, "hi\n");
        assert_eq!(c.exit_code, Some(0));
        assert_eq!(e.commands_finished(), 1);
        assert!(!e.busy());
        let evs = e.drain_events();
        assert!(evs
            .iter()
            .any(|e| matches!(e, crate::TermEvent::CommandFinished(Some(0)))));
    }

    #[test]
    fn nth_most_recent_and_failure_status() {
        let mut e = Emulator::new(40, 6, 100);
        session(&mut e, "one", "1\r\n", 0);
        session(&mut e, "two", "2\r\n", 3);
        assert_eq!(e.last_command(1).unwrap().command, "two");
        assert_eq!(e.last_command(1).unwrap().exit_code, Some(3));
        assert_eq!(e.last_command(2).unwrap().command, "one");
        assert!(e.last_command(3).is_none());
        assert!(e.last_command(0).is_none());
    }

    #[test]
    fn busy_between_c_and_d_and_running_output() {
        let mut e = Emulator::new(40, 6, 100);
        e.advance(&mk("133;A"));
        e.advance(b"$ ");
        e.advance(&mk("133;B"));
        e.advance(b"sleep\r\n");
        assert!(!e.busy());
        e.advance(&mk("133;C"));
        e.advance(b"partial");
        assert!(e.busy());
        assert_eq!(e.running_output().unwrap(), "partial");
        e.advance(&mk("133;D;0"));
        assert!(!e.busy());
    }

    #[test]
    fn missing_d_leaves_an_unfinished_command_and_next_prompt_starts_fresh() {
        let mut e = Emulator::new(40, 6, 100);
        e.advance(&mk("133;A"));
        e.advance(b"$ ");
        e.advance(&mk("133;B"));
        e.advance(b"x\r\n");
        e.advance(&mk("133;C"));
        e.advance(b"out\r\n");
        // No D: the shell was killed, then a new prompt appears.
        session(&mut e, "y", "ok\r\n", 0);
        assert_eq!(e.last_command(1).unwrap().command, "y");
        assert!(
            e.last_command(2).is_none(),
            "unfinished commands do not count"
        );
    }

    #[test]
    fn redrawn_prompts_and_a_leading_d_do_not_create_commands() {
        let mut e = Emulator::new(40, 6, 100);
        e.advance(&mk("133;D;0"));
        e.advance(&mk("133;A"));
        e.advance(b"$ ");
        e.advance(&mk("133;B"));
        e.advance(&mk("133;A")); // redraw
        e.advance(b"$ ");
        e.advance(&mk("133;B"));
        assert_eq!(e.commands_len(), 1);
        assert!(e.has_marks());
        assert!(e.last_command(1).is_none());
        assert_eq!(e.commands_finished(), 0);
    }

    #[test]
    fn nested_prompts_use_secondary_marks() {
        let mut e = Emulator::new(40, 6, 100);
        e.advance(&mk("133;A"));
        e.advance(b"$ ");
        e.advance(&mk("133;B"));
        e.advance(b"echo \\\r\n");
        e.advance(&mk("133;A;k=s"));
        e.advance(b"> ");
        e.advance(&mk("133;B"));
        e.advance(b"hi\r\n");
        e.advance(&mk("133;C"));
        e.advance(b"hi\r\n");
        e.advance(&mk("133;D;0"));
        let c = e.last_command(1).unwrap();
        assert_eq!(c.output, "hi\n");
        assert_eq!(e.commands_len(), 1);
    }

    #[test]
    fn output_without_trailing_newline_and_wrapped_lines() {
        let mut e = Emulator::new(10, 8, 100);
        session(&mut e, "x", "0123456789abcdef", 0);
        assert_eq!(e.last_command(1).unwrap().output, "0123456789abcdef");
        session(&mut e, "y", "a\r\n\r\nb\r\n", 0);
        assert_eq!(e.last_command(1).unwrap().output, "a\n\nb\n");
    }

    #[test]
    fn wide_characters_in_command_and_output() {
        let mut e = Emulator::new(20, 6, 100);
        session(&mut e, "echo 漢字", "漢字\r\n", 0);
        let c = e.last_command(1).unwrap();
        assert_eq!(c.command, "echo 漢字");
        assert_eq!(c.output, "漢字\n");
    }

    #[test]
    fn cwd_is_captured_when_the_command_starts() {
        let mut e = Emulator::new(40, 6, 100);
        e.advance(&mk("7;file:///tmp/work"));
        session(&mut e, "pwd", "/tmp/work\r\n", 0);
        e.advance(&mk("7;file:///tmp/other"));
        assert_eq!(e.last_command(1).unwrap().cwd.as_deref(), Some("/tmp/work"));
    }

    #[test]
    fn anchors_survive_scrolling_into_history() {
        let mut e = Emulator::new(20, 4, 1000);
        let out: String = (0..30).map(|i| format!("line{i}\r\n")).collect();
        session(&mut e, "seq", &out, 0);
        assert!(e.history_size() > 20);
        let c = e.last_command(1).unwrap();
        assert!(c.output.starts_with("line0\nline1\n"), "{}", c.output);
        assert!(c.output.ends_with("line29\n"));
        assert_eq!(c.output.lines().count(), 30);
    }

    #[test]
    fn anchors_survive_a_full_scrollback() {
        // History fills up (cap 50) while the command prints 400 lines: older output is gone,
        // but a later short command must still be cut out exactly.
        let mut e = Emulator::new(20, 4, 50);
        let out: String = (0..400).map(|i| format!("L{i}\r\n")).collect();
        session(&mut e, "big", &out, 0);
        assert_eq!(e.history_size(), 50);
        session(&mut e, "small", "ok\r\n", 0);
        let c = e.last_command(1).unwrap();
        assert_eq!(c.command, "small");
        assert_eq!(c.output, "ok\n");
        // The big one lost its head to the scrollback limit and says so.
        let big = e.last_command(2).unwrap();
        assert_eq!(big.command, "big");
        assert!(big.truncated);
        assert!(big.output.ends_with("L399\n"), "{:?}", big.output);
    }

    #[test]
    fn open_command_spanning_a_full_scrollback_keeps_exact_anchors() {
        let mut e = Emulator::new(20, 4, 20);
        e.advance(&mk("133;A"));
        e.advance(b"$ ");
        e.advance(&mk("133;B"));
        e.advance(b"go\r\n");
        e.advance(&mk("133;C"));
        // Fill and overflow the history while C is outstanding, in odd-sized pieces.
        let out: String = (0..30).map(|i| format!("{i}\r\n")).collect();
        for chunk in out.as_bytes().chunks(7) {
            e.advance(chunk);
        }
        e.advance(&mk("133;D;0"));
        assert_eq!(e.history_size(), 20);
        let c = e.last_command(1).unwrap();
        // 30 lines of output; only history (20) + screen rows remain, so the head is gone and
        // the result says so instead of pointing at the wrong region.
        assert!(c.truncated);
        assert!(c.output.ends_with("29\n"), "{:?}", c.output);
    }

    #[test]
    fn ring_keeps_the_last_500_commands() {
        let mut e = Emulator::new(20, 4, 10_000);
        for i in 0..520 {
            session(&mut e, &format!("c{i}"), "o\r\n", 0);
        }
        assert_eq!(e.commands_len(), MAX_COMMANDS);
        assert_eq!(e.last_command(1).unwrap().command, "c519");
        assert_eq!(e.last_command(500).unwrap().command, "c20");
        assert!(e.last_command(501).is_none());
        assert_eq!(e.commands_finished(), 520);
    }

    #[test]
    fn marks_in_the_alternate_screen_are_ignored() {
        let mut e = Emulator::new(20, 4, 100);
        e.advance(b"\x1b[?1049h");
        e.advance(&mk("133;A"));
        e.advance(&mk("133;C"));
        e.advance(&mk("133;D;0"));
        e.advance(b"\x1b[?1049l");
        assert!(!e.has_marks());
        assert_eq!(e.commands_len(), 0);
    }

    #[test]
    fn entering_the_alternate_screen_does_not_look_like_a_history_clear() {
        let mut e = Emulator::new(20, 3, 100);
        session(&mut e, "a", "1\r\n2\r\n3\r\n4\r\n", 0);
        e.advance(b"\x1b[?1049hvim\x1b[?1049l");
        session(&mut e, "b", "x\r\n", 0);
        assert_eq!(e.last_command(1).unwrap().output, "x\n");
        assert_eq!(e.last_command(2).unwrap().output, "1\n2\n3\n4\n");
    }

    #[test]
    fn prompt_lines_follow_history() {
        let mut e = Emulator::new(10, 3, 100);
        e.advance(&mk("133;A"));
        e.advance(b"prompt1\r\nout\r\n");
        e.advance(&mk("133;A"));
        e.advance(b"prompt2");
        assert_eq!(e.prompt_marks(), vec![0, 2]);
        e.advance(b"\r\nmore\r\nmore2");
        assert_eq!(e.prompt_marks(), vec![0, 2]);
        assert_eq!(e.line_cells(2).unwrap().text(), "prompt2");
    }

    #[test]
    fn resize_with_new_width_drops_stale_commands_but_keeps_the_flag() {
        let mut e = Emulator::new(20, 4, 100);
        session(&mut e, "a", "1\r\n", 0);
        e.resize(20, 6);
        assert!(e.last_command(1).is_some());
        e.resize(30, 6);
        assert!(e.last_command(1).is_none());
        assert!(e.has_marks());
    }

    #[test]
    fn clearing_scrollback_evicts_commands_in_it() {
        let mut e = Emulator::new(20, 3, 100);
        session(&mut e, "a", "1\r\n2\r\n3\r\n4\r\n5\r\n", 0);
        e.advance(b"\x1b[3J");
        let a = e.last_command(1).unwrap();
        assert!(a.truncated, "its start was in the cleared history");
        session(&mut e, "b", "z\r\n", 0);
        assert_eq!(e.last_command(1).unwrap().output, "z\n");
    }

    fn reg(id: &str) -> Vec<u8> {
        use crate::menu_reg::b64_encode;
        format!(
            "\x1b]7717;menu;add;{id};{};send;{}\x07",
            b64_encode(id.as_bytes()),
            b64_encode(b"make test")
        )
        .into_bytes()
    }

    #[test]
    fn menu_items_follow_the_lifecycle_and_replay() {
        let mut e = Emulator::new(40, 5, 100);
        e.advance(&reg("tests"));
        e.advance(b"\x1b]7717;menu;add;lint;");
        e.advance(b"TGludA==;send;Y2FyZ28gY2xpcHB5\x1b\\");
        assert_eq!(e.menu_items().len(), 2);
        assert!(e
            .drain_events()
            .iter()
            .any(|ev| matches!(ev, crate::TermEvent::MenuItemsChanged)));

        // A replay stream rebuilds the same items after its leading RIS.
        let bytes = e.replay_bytes(100);
        let mut b = Emulator::new(40, 5, 100);
        b.advance(&reg("stale"));
        b.advance(&bytes);
        assert_eq!(b.menu_items(), e.menu_items());

        // `clear` (CSI 3 J), `CSI J` (not a clear) and RIS.
        e.advance(b"\x1b[J");
        assert_eq!(e.menu_items().len(), 2);
        e.advance(b"\x1b[3J");
        assert!(e.menu_items().is_empty());
        e.advance(&reg("again"));
        e.advance(b"\x1bc");
        assert!(e.menu_items().is_empty());
    }

    #[test]
    fn hostile_output_is_bounded() {
        let mut e = Emulator::new(40, 5, 100);
        // `cat` of a file full of registrations: capped, counted, never panics.
        for i in 0..50 {
            e.advance(&reg(&format!("i{i}")));
        }
        e.advance(b"\x1b]7717;menu;add;x;!!;send;!!\x07\x1b]7717;menu;nope\x07");
        let n = e.menu_items().len();
        assert!(n <= crate::menu_reg::RATE_PER_SEC as usize && n > 0, "{n}");
    }
}
