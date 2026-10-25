//! The line tap (SPEC §10.11.3): the terminal facade's view of "a line of rendered output".
//!
//! After each parsed segment the tap looks at where the cursor is. Every logical line (soft-wrapped
//! rows joined, SGR gone because only cell characters are read, trailing blanks trimmed) that
//! starts before the cursor's own logical line is finished and goes into a bounded queue. A line
//! the cursor is still on (a prompt) is scanned once after a stretch of PTY silence.
//!
//! Disabled, the cost is one `Option` check per parsed segment. Only this crate touches the
//! engine; consumers receive plain strings.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};

/// Alternate-screen line ids carry this bit so they never collide with main-screen ids.
pub const ALT_ID_BIT: u64 = 1 << 63;

/// One line of rendered output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedLine {
    pub pane: u64,
    /// Stable id of the line's first row: `trimmed + absolute row`, so it survives history
    /// trimming (§10.11.3). Alternate-screen lines carry [`ALT_ID_BIT`].
    pub abs_line: u64,
    pub text: String,
    /// The line was still being typed when it was scanned.
    pub partial: bool,
    /// The line belongs to the shell's prompt or to the command being typed (between OSC 133
    /// `A` and `C`: the prompt, what you type, an autosuggestion). Always `false` without shell
    /// integration, which cannot tell input from output.
    pub input: bool,
}

/// Whether stable id `id` falls in one of the sorted, disjoint `[start, end)` regions.
pub fn in_regions(regions: &[(u64, u64)], id: u64) -> bool {
    let i = regions.partition_point(|r| r.0 <= id);
    i > 0 && id < regions[i - 1].1
}

/// A bounded queue shared by every pane's tap and drained by one worker thread. When it is full
/// the **oldest** line is dropped and counted; a producer never blocks (§10.11.3).
pub struct LineQueue {
    q: Mutex<VecDeque<CompletedLine>>,
    cv: Condvar,
    cap: usize,
    dropped: AtomicU64,
    /// The consumer is parked on `cv`: only then does a producer pay for a wake-up.
    sleeping: AtomicBool,
}

impl LineQueue {
    pub fn new(cap: usize) -> Arc<LineQueue> {
        Arc::new(LineQueue {
            q: Mutex::new(VecDeque::with_capacity(cap.min(4096))),
            cv: Condvar::new(),
            cap: cap.max(1),
            dropped: AtomicU64::new(0),
            sleeping: AtomicBool::new(false),
        })
    }

    pub fn push(&self, line: CompletedLine) {
        self.push_many(std::iter::once(line));
    }

    /// Adds `lines` under one lock acquisition, waking the consumer only when it had nothing
    /// to do (it is otherwise already running): the PTY thread pays one lock per parsed segment,
    /// not one per line.
    pub fn push_many(&self, lines: impl IntoIterator<Item = CompletedLine>) {
        let mut q = self.q.lock().unwrap_or_else(|e| e.into_inner());
        for line in lines {
            if q.len() >= self.cap {
                q.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            q.push_back(line);
        }
        let any = !q.is_empty();
        // Holding the lock decides who sees the flag: the consumer sets it under the lock before
        // it waits, so a push that finds it clear knows the consumer is awake and will look again.
        let wake = any && self.sleeping.swap(false, Ordering::Relaxed);
        drop(q);
        if wake {
            self.cv.notify_one();
        }
    }

    /// Takes up to `max` lines, waiting up to `wait` for the first one. A consumer that loops on
    /// this should pause a millisecond after a non-empty batch: that bounds its wake-ups (and
    /// the producers' cost of waking it) however fast lines arrive.
    pub fn recv_batch(&self, max: usize, wait: Duration) -> Vec<CompletedLine> {
        let mut q = self.q.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() && !wait.is_zero() {
            self.sleeping.store(true, Ordering::Relaxed);
            q = self
                .cv
                .wait_timeout(q, wait)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            self.sleeping.store(false, Ordering::Relaxed);
        }
        let n = q.len().min(max);
        q.drain(..n).collect()
    }

    pub fn len(&self) -> usize {
        self.q.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lines dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wakes a waiting consumer (to shut it down).
    pub fn wake(&self) {
        self.cv.notify_all();
    }
}

/// Tap settings (`[triggers]`).
#[derive(Debug, Clone, Copy)]
pub struct LineTapCfg {
    pub max_line_chars: usize,
    pub scan_alt_screen: bool,
    /// PTY silence before an unfinished line is scanned; 0 = never.
    pub partial_after_ms: u32,
}

/// What the reader thread and the extraction thread share (lazy mode).
pub(crate) struct TapShared {
    /// The tap is installed; the extraction thread exits when this clears.
    pub alive: AtomicBool,
    /// Finished lines are waiting to be read out of the grid.
    pub pending: AtomicBool,
    waker: Mutex<Option<std::thread::Thread>>,
}

impl TapShared {
    pub(crate) fn new() -> Arc<TapShared> {
        Arc::new(TapShared {
            alive: AtomicBool::new(true),
            pending: AtomicBool::new(false),
            waker: Mutex::new(None),
        })
    }

    pub(crate) fn set_waker(&self, t: std::thread::Thread) {
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
    }

    /// Marks work pending; wakes the extraction thread only on the empty → non-empty edge, so
    /// the reader pays a syscall at most once per extraction cycle.
    fn signal(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            if let Some(t) = self
                .waker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                t.unpark();
            }
        }
    }
}

struct Active {
    cfg: LineTapCfg,
    queue: Arc<LineQueue>,
    pane: u64,
    /// Lazy mode: the reader thread only records which rows finished (`pending`); the text is
    /// read out of the grid by [`LineTap::drain`] on another thread, so a `cat` of 100 MB pays
    /// the PTY thread almost nothing (SPEC §10.11.10).
    lazy: bool,
    /// Stable ids `[from, to)` of finished main-screen lines not yet read out.
    pending: Option<(u64, u64)>,
    /// Stable id of the first row of the next line to emit.
    next_id: Option<u64>,
    /// Alternate-screen counterpart of `next_id`.
    alt_next: Option<u64>,
    /// Replayed bytes are being applied: advance, emit nothing.
    replay: bool,
    /// The unfinished line already scanned: `(id, content hash)`.
    partial_seen: Option<(u64, u64)>,
    shared: Arc<TapShared>,
}

#[derive(Default)]
pub(crate) struct LineTap {
    active: Option<Active>,
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Appends the characters of `line` to `out` (trailing blanks left off a row that does not
/// soft-wrap); returns whether the row soft-wraps.
fn row_into<T>(term: &Term<T>, line: Line, out: &mut String) -> bool {
    let g = term.grid();
    let cols = g.columns();
    let row = &g[line];
    let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    let mut end = cols;
    if !wraps {
        while end > 0 && matches!(row[Column(end - 1)].c, ' ' | '\0') {
            end -= 1;
        }
    }
    out.reserve(end);
    for cell in &row[Column(0)..Column(end)] {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        out.push(if cell.c == '\0' { ' ' } else { cell.c });
    }
    wraps
}

/// Whether the row soft-wraps into the next one.
fn row_wraps<T>(term: &Term<T>, line: Line) -> bool {
    let g = term.grid();
    g[line][Column(g.columns() - 1)]
        .flags
        .contains(Flags::WRAPLINE)
}

fn grid_line<T>(term: &Term<T>, abs: usize) -> Line {
    Line(abs as i32 - term.grid().history_size() as i32)
}

fn total<T>(term: &Term<T>) -> usize {
    term.grid().history_size() + term.grid().screen_lines()
}

fn cursor_row<T>(term: &Term<T>) -> usize {
    let g = term.grid();
    (g.cursor.point.line.0 + g.history_size() as i32).max(0) as usize
}

/// The logical line starting at row `abs`: its text (trailing blanks trimmed, cut to `max`
/// characters) and the row after it.
fn logical_from<T>(term: &Term<T>, abs: usize, max: usize) -> (String, usize) {
    let mut text = String::new();
    let mut r = abs;
    let n = total(term);
    while r < n {
        let wrapped = row_into(term, grid_line(term, r), &mut text);
        r += 1;
        if !wrapped {
            break;
        }
    }
    // `trim_end` only looks at the tail; only a line longer than `max` characters needs the
    // (O(n)) character count.
    let kept = text.trim_end().len();
    text.truncate(kept);
    if text.len() > max {
        if let Some((i, _)) = text.char_indices().nth(max) {
            text.truncate(i);
        }
    }
    (text, r)
}

/// Absolute first row of the logical line under the cursor.
pub(crate) fn cursor_line_start<T>(term: &Term<T>) -> usize {
    logical_start(term, crate::lines::cursor_abs(term).0)
}

/// First row of the logical line that contains row `abs`.
fn logical_start<T>(term: &Term<T>, mut abs: usize) -> usize {
    while abs > 0 && row_wraps(term, grid_line(term, abs - 1)) {
        abs -= 1;
    }
    abs
}

impl std::fmt::Debug for LineTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LineTap(installed: {})", self.active.is_some())
    }
}

/// Context lines around a row, for trigger `snapshot` captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedLines {
    /// Up to `before` logical lines above the match, oldest first.
    pub before: Vec<String>,
    pub line: String,
    /// Up to `after` finished logical lines below the match (an unfinished line is not one).
    pub after: Vec<String>,
}

/// The logical line whose first row has stable id `id`, with `before` lines above and up to
/// `after` finished lines below. `None` when the row left the scrollback.
pub(crate) fn lines_around<T>(
    term: &Term<T>,
    trimmed: u64,
    id: u64,
    before: usize,
    after: usize,
    max_chars: usize,
) -> Option<CapturedLines> {
    let abs = id.checked_sub(trimmed)? as usize;
    if abs >= total(term) {
        return None;
    }
    let (line, mut next) = logical_from(term, abs, max_chars);
    let mut above = Vec::new();
    let mut cur = abs;
    while above.len() < before && cur > 0 {
        let start = logical_start(term, cur - 1);
        above.push(logical_from(term, start, max_chars).0);
        cur = start;
    }
    above.reverse();
    let cursor_start = logical_start(term, cursor_row(term));
    let mut below = Vec::new();
    while below.len() < after && next < cursor_start {
        let (text, after_row) = logical_from(term, next, max_chars);
        below.push(text);
        next = after_row.max(next + 1);
    }
    Some(CapturedLines {
        before: above,
        line,
        after: below,
    })
}

/// Reads the logical lines that start in stable ids `[from, to)` out of the grid (`base` turns an
/// id into a row: `id - base`), skipping blanks and a line already scanned as an unfinished
/// prompt whose text did not change.
fn extract_range<T>(
    a: &mut Active,
    term: &Term<T>,
    base: u64,
    bit: u64,
    from: u64,
    to: u64,
    regions: &[(u64, u64)],
) -> Vec<CompletedLine> {
    let mut out = Vec::new();
    let mut next = from;
    while next < to {
        let abs = (next - base) as usize;
        if abs >= total(term) {
            break;
        }
        let (text, after) = logical_from(term, abs, a.cfg.max_line_chars);
        if !text.is_empty() {
            let mut seen = false;
            if let Some((id, ph)) = a.partial_seen {
                if id == (next | bit) {
                    seen = ph == fnv(&text);
                    a.partial_seen = None;
                }
            }
            if !seen {
                out.push(CompletedLine {
                    pane: a.pane,
                    abs_line: next | bit,
                    text,
                    partial: false,
                    input: bit == 0 && in_regions(regions, next),
                });
            }
        }
        next += (after - abs).max(1) as u64;
    }
    out
}

impl LineTap {
    /// Starts tapping from the cursor's current line: older output is never scanned. In `lazy`
    /// mode the reader thread only records finished rows and `shared` wakes whoever calls
    /// [`drain`](Self::drain); otherwise lines are read out at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install<T>(
        &mut self,
        cfg: LineTapCfg,
        queue: Arc<LineQueue>,
        pane: u64,
        shared: Arc<TapShared>,
        lazy: bool,
        term: &Term<T>,
        trimmed: u64,
    ) {
        self.remove();
        let alt = term.mode().contains(TermMode::ALT_SCREEN);
        let start = (!alt).then(|| trimmed + logical_start(term, cursor_row(term)) as u64);
        self.active = Some(Active {
            cfg,
            queue,
            pane,
            lazy,
            pending: None,
            next_id: start,
            alt_next: None,
            replay: false,
            partial_seen: None,
            shared,
        });
    }

    pub(crate) fn remove(&mut self) {
        if let Some(a) = self.active.take() {
            a.shared.alive.store(false, Ordering::Relaxed);
            if let Some(t) = a
                .shared
                .waker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                t.unpark();
            }
        }
    }

    pub(crate) fn installed(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn set_replay(&mut self, on: bool) {
        if let Some(a) = &mut self.active {
            a.replay = on;
        }
    }

    /// The grid was reflowed or cleared: stable row ids no longer line up.
    pub(crate) fn reset_position<T>(&mut self, term: &Term<T>, trimmed: u64) {
        if let Some(a) = &mut self.active {
            let alt = term.mode().contains(TermMode::ALT_SCREEN);
            a.next_id = (!alt).then(|| trimmed + logical_start(term, cursor_row(term)) as u64);
            a.alt_next = None;
            a.pending = None;
        }
    }

    /// Whether `scan` reads text itself (not lazy), so the caller must supply input regions.
    pub(crate) fn reads_in_scan(&self) -> bool {
        self.active.as_ref().is_some_and(|a| !a.lazy)
    }

    pub(crate) fn wants_alt(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.cfg.scan_alt_screen)
    }

    /// Emits the lines finished since the last call (lazy mode: records which ones finished).
    /// `trimmed` is the tracker's count of lines that left the history.
    pub(crate) fn scan<T>(&mut self, term: &Term<T>, trimmed: u64, regions: &[(u64, u64)]) {
        let Some(a) = &mut self.active else {
            return;
        };
        let alt = term.mode().contains(TermMode::ALT_SCREEN);
        if alt && !a.cfg.scan_alt_screen {
            a.alt_next = None;
            return;
        }
        let (base, bit) = if alt { (0, ALT_ID_BIT) } else { (trimmed, 0) };
        let cur_start = base + logical_start(term, cursor_row(term)) as u64;
        let slot = if alt { &mut a.alt_next } else { &mut a.next_id };
        // A fresh alternate screen starts empty, so its scan starts at its first row.
        let mut next = *slot.get_or_insert(if alt { 0 } else { cur_start });
        next = next.max(base);
        if next < cur_start && !a.replay && a.lazy && !alt {
            // Cheap path: remember the range; the text is read out later.
            a.pending = Some(match a.pending {
                Some((from, _)) => (from, cur_start),
                None => (next, cur_start),
            });
            a.shared.signal();
        } else if next < cur_start {
            let lines = if a.replay {
                Vec::new()
            } else {
                extract_range(a, term, base, bit, next, cur_start, regions)
            };
            if !lines.is_empty() {
                a.queue.push_many(lines);
            }
        }
        *(if alt { &mut a.alt_next } else { &mut a.next_id }) = Some(cur_start.max(next));
    }

    /// Lazy mode: reads the finished lines out of the grid and queues them. Call with the
    /// terminal and tracker locks held. Returns how many lines were queued.
    pub(crate) fn drain<T>(
        &mut self,
        term: &Term<T>,
        trimmed: u64,
        regions: &[(u64, u64)],
    ) -> usize {
        let Some(a) = &mut self.active else {
            return 0;
        };
        a.shared.pending.store(false, Ordering::Release);
        let Some((from, to)) = a.pending.take() else {
            return 0;
        };
        let lines = extract_range(a, term, trimmed, 0, from.max(trimmed), to, regions);
        let n = lines.len();
        if n > 0 {
            a.queue.push_many(lines);
        }
        n
    }

    /// Records the unfinished line as already seen (after replayed bytes), so a replayed prompt
    /// is not scanned again.
    pub(crate) fn settle_replay<T>(&mut self, term: &Term<T>, trimmed: u64) {
        let Some(a) = &mut self.active else {
            return;
        };
        a.replay = false;
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let start = logical_start(term, cursor_row(term));
        let (text, _) = logical_from(term, start, a.cfg.max_line_chars);
        if !text.is_empty() {
            a.partial_seen = Some((trimmed + start as u64, fnv(&text)));
        }
    }

    /// Scans the line the cursor is on once, if it has not been scanned in this form.
    pub(crate) fn flush_partial<T>(
        &mut self,
        term: &Term<T>,
        trimmed: u64,
        regions: &[(u64, u64)],
    ) {
        let Some(a) = &mut self.active else {
            return;
        };
        if a.cfg.partial_after_ms == 0 || a.replay {
            return;
        }
        let alt = term.mode().contains(TermMode::ALT_SCREEN);
        if alt && !a.cfg.scan_alt_screen {
            return;
        }
        let (base, bit) = if alt { (0, ALT_ID_BIT) } else { (trimmed, 0) };
        let start = logical_start(term, cursor_row(term));
        let (text, _) = logical_from(term, start, a.cfg.max_line_chars);
        if text.is_empty() {
            return;
        }
        let id = (base + start as u64) | bit;
        let key = (id, fnv(&text));
        if a.partial_seen == Some(key) {
            return;
        }
        a.partial_seen = Some(key);
        a.queue.push(CompletedLine {
            pane: a.pane,
            abs_line: id,
            text,
            partial: true,
            input: bit == 0 && in_regions(regions, id),
        });
    }
}

#[cfg(test)]
mod tests;
