//! The decoration layer (SPEC §10.11.4): highlights, markers and the 1.2 s flash of a finding,
//! anchored to a row's **stable id** (`line_base + absolute row`) and to **character offsets**
//! within its logical line, so they survive scrolling, history trimming and reflow. Everything
//! is display-only: the text, selection, copy output and the PTY are untouched.

use std::collections::{HashMap, VecDeque};

use mt_term::RenderSnapshot;
use mt_triggers::Style;

/// How long a finding flashes after a click (SPEC §10.11.4).
pub const FLASH_MS: u64 = 1200;
/// The jump flash is drawn stronger than a highlight so it is easy to spot.
pub const FLASH_ALPHA: f32 = 0.5;
/// A `background` highlight is at most this opaque, so theme contrast holds.
pub const MAX_ALPHA: f32 = 0.28;

#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    pub line: u64,
    /// Character offsets into the logical line; `None` = the whole line.
    pub span: Option<(usize, usize)>,
    /// What the highlighted cells said when the rule matched. The highlight is drawn only while
    /// they still say it (an edited prompt, a cleared line, an autosuggestion dropping out).
    pub expect: Option<String>,
    pub color: String,
    pub style: Style,
    pub expires_ms: Option<u64>,
    pub rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub line: u64,
    pub label: String,
    pub color: String,
    pub rule: String,
}

#[derive(Default)]
pub struct PaneDeco {
    highlights: VecDeque<Highlight>,
    markers: VecDeque<Marker>,
    /// `(line id, until ms, matched characters or the whole line)`.
    flash: Option<(u64, u64, Option<(usize, usize)>)>,
    /// The marker `Next`/`Previous` last jumped to.
    nav: Option<u64>,
}

#[derive(Default)]
pub struct Decorations {
    panes: HashMap<u64, PaneDeco>,
    /// The editor's transient regex preview: `(line id, start char, end char)` per pane.
    preview: HashMap<u64, Vec<(u64, usize, usize)>>,
}

/// Where a pane's viewport sits (what `step_marker` needs from a snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub line_base: u64,
    pub history: usize,
    pub offset: usize,
    pub rows: usize,
}

impl View {
    pub fn of(snap: &RenderSnapshot) -> View {
        View {
            line_base: snap.line_base,
            history: snap.history_size,
            offset: snap.display_offset,
            rows: snap.rows,
        }
    }
}

/// One rectangle to paint: viewport row, columns `start..end`.
#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub color: String,
    pub style: Style,
    /// The flash of a clicked finding (drawn stronger, whatever the rule's colors say).
    pub flash: bool,
}

impl Decorations {
    fn pane(&mut self, pane: u64) -> &mut PaneDeco {
        self.panes.entry(pane).or_default()
    }

    /// Adds a highlight; an identical one (same line, span and rule) is replaced, and the oldest
    /// is evicted past `max` (`triggers.highlights_max`).
    pub fn add_highlight(&mut self, pane: u64, h: Highlight, max: usize) {
        let d = self.pane(pane);
        d.highlights
            .retain(|x| !(x.line == h.line && x.span == h.span && x.rule == h.rule));
        d.highlights.push_back(h);
        while d.highlights.len() > max.max(1) {
            d.highlights.pop_front();
        }
    }

    /// Adds a marker (one per line and rule); the oldest is dropped past `max`.
    pub fn add_marker(&mut self, pane: u64, m: Marker, max: usize) {
        let d = self.pane(pane);
        d.markers
            .retain(|x| !(x.line == m.line && x.rule == m.rule));
        d.markers.push_back(m);
        while d.markers.len() > max.max(1) {
            d.markers.pop_front();
        }
    }

    /// Adds a marker unless the line already has one from this rule (a `findings` action must
    /// not replace the label and color of the rule's own `marker` action).
    pub fn add_marker_if_absent(&mut self, pane: u64, m: Marker, max: usize) {
        if !self
            .pane(pane)
            .markers
            .iter()
            .any(|x| x.line == m.line && x.rule == m.rule)
        {
            self.add_marker(pane, m, max);
        }
    }

    /// Shows `hits` as a transient decoration (the editor's live preview); an empty list or
    /// [`clear_preview`](Self::clear_preview) removes it.
    pub fn set_preview(&mut self, pane: u64, hits: Vec<(u64, usize, usize)>) {
        if hits.is_empty() {
            self.preview.remove(&pane);
        } else {
            self.preview.insert(pane, hits);
        }
    }

    pub fn clear_preview(&mut self) {
        self.preview.clear();
    }

    pub fn flash(&mut self, pane: u64, line: u64, now: u64, span: Option<(usize, usize)>) {
        self.pane(pane).flash = Some((line, now + FLASH_MS, span));
    }

    pub fn highlight_count(&self, pane: u64) -> usize {
        self.panes.get(&pane).map_or(0, |d| d.highlights.len())
    }

    pub fn markers(&self, pane: u64) -> Vec<Marker> {
        let mut v: Vec<Marker> = self
            .panes
            .get(&pane)
            .map(|d| d.markers.iter().cloned().collect())
            .unwrap_or_default();
        v.sort_by_key(|m| m.line);
        v
    }

    pub fn clear_markers(&mut self, pane: u64) {
        if let Some(d) = self.panes.get_mut(&pane) {
            d.markers.clear();
            d.nav = None;
        }
    }

    /// Drops what has expired and what left the scrollback (`line < base`). Returns whether
    /// anything changed (the window repaints).
    pub fn prune(&mut self, pane: u64, base: u64, now: u64) -> bool {
        let Some(d) = self.panes.get_mut(&pane) else {
            return false;
        };
        let before = (d.highlights.len(), d.markers.len(), d.flash);
        d.highlights
            .retain(|h| h.line >= base && h.expires_ms.is_none_or(|e| e > now));
        d.markers.retain(|m| m.line >= base);
        if d.flash.is_some_and(|(_, until, _)| until <= now) {
            d.flash = None;
        }
        (d.highlights.len(), d.markers.len(), d.flash) != before
    }

    pub fn forget_pane(&mut self, pane: u64) {
        self.panes.remove(&pane);
    }

    pub fn clear(&mut self) {
        self.panes.clear();
        self.preview.clear();
    }

    pub fn has_any(&self, pane: u64) -> bool {
        self.preview.contains_key(&pane)
            || self.panes.get(&pane).is_some_and(|d| {
                !d.highlights.is_empty() || !d.markers.is_empty() || d.flash.is_some()
            })
    }

    pub fn panes(&self) -> Vec<u64> {
        self.panes.keys().copied().collect()
    }

    /// Rectangles to paint for the viewport in `snap`. Nothing on the alternate screen.
    pub fn rects(&self, pane: u64, snap: &RenderSnapshot, now: u64) -> Vec<Rect> {
        let empty = PaneDeco::default();
        let d = self.panes.get(&pane).unwrap_or(&empty);
        let preview = self.preview.get(&pane).map(Vec::as_slice).unwrap_or(&[]);
        if snap.modes.alt_screen
            || (d.highlights.is_empty() && d.flash.is_none() && preview.is_empty())
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        for line in logical_lines(snap) {
            for &(id, a, b) in preview.iter().filter(|p| p.0 == line.id) {
                let _ = id;
                for (row, start, end) in line.cells_of(Some((a, b))) {
                    out.push(Rect {
                        row,
                        start,
                        end,
                        color: "accent".into(),
                        style: Style::Background,
                        flash: false,
                    });
                }
            }
            for h in d.highlights.iter().filter(|h| h.line == line.id) {
                if h.expires_ms.is_some_and(|e| e <= now) || !line.shows(h.span, &h.expect) {
                    continue;
                }
                for (row, start, end) in line.cells_of(h.span) {
                    out.push(Rect {
                        row,
                        start,
                        end,
                        color: h.color.clone(),
                        style: h.style,
                        flash: false,
                    });
                }
            }
            if let Some((id, until, span)) = d.flash {
                if id == line.id && until > now {
                    for (row, start, end) in line.cells_of(span) {
                        out.push(Rect {
                            row,
                            start,
                            end,
                            color: "accent".into(),
                            style: Style::Background,
                            flash: true,
                        });
                    }
                }
            }
        }
        out
    }

    /// Viewport rows that start a marked line (the 3 px edge mark), with the marker color.
    pub fn edge_marks(&self, pane: u64, snap: &RenderSnapshot) -> Vec<(usize, String)> {
        let Some(d) = self.panes.get(&pane) else {
            return Vec::new();
        };
        if snap.modes.alt_screen || d.markers.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for line in logical_lines(snap) {
            if let Some(m) = d.markers.iter().rev().find(|m| m.line == line.id) {
                for row in line.first_row..=line.last_row {
                    out.push((row, m.color.clone()));
                }
            }
        }
        out
    }

    /// Fractions (0..1, top to bottom) of the scrollbar track where markers sit.
    pub fn ticks(&self, pane: u64, snap: &RenderSnapshot) -> Vec<(f32, String)> {
        let Some(d) = self.panes.get(&pane) else {
            return Vec::new();
        };
        let total = (snap.history_size + snap.rows).max(1) as f32;
        d.markers
            .iter()
            .filter(|m| m.line >= snap.line_base)
            .map(|m| {
                (
                    ((m.line - snap.line_base) as f32 / total).clamp(0.0, 1.0),
                    m.color.clone(),
                )
            })
            .collect()
    }

    /// The marker `NextMarker` (`forward`) or `PreviousMarker` jumps to: the next one past the
    /// last jump while that one is still on screen, else past the viewport's middle row.
    pub fn step_marker(&mut self, pane: u64, view: View, forward: bool) -> Option<u64> {
        let d = self.panes.get_mut(&pane)?;
        let top = view.line_base + (view.history - view.offset.min(view.history)) as u64;
        let bottom = top + view.rows as u64;
        let middle = top + view.rows as u64 / 2;
        let from = match d.nav {
            Some(n) if (top..bottom).contains(&n) => n,
            _ => middle,
        };
        let mut ids: Vec<u64> = d
            .markers
            .iter()
            .map(|m| m.line)
            .filter(|l| *l >= view.line_base)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let target = if forward {
            ids.iter().copied().find(|&l| l > from)
        } else {
            ids.iter().rev().copied().find(|&l| l < from)
        };
        if target.is_some() {
            d.nav = target;
        }
        target
    }
}

/// One logical line of the viewport: its stable id and the cells of its characters.
struct Line {
    id: u64,
    first_row: usize,
    last_row: usize,
    /// Per character: `(row, first col, end col)` (a wide character spans two cells).
    chars: Vec<(usize, usize, usize)>,
    /// The characters of `chars`, same indices.
    text: Vec<char>,
    /// Columns used on the last row (trailing blanks excluded).
    last_used: usize,
    cols: usize,
}

impl Line {
    /// Whether the cells in `span` (or the whole line) still read `expect` (`None`: no check).
    fn shows(&self, span: Option<(usize, usize)>, expect: &Option<String>) -> bool {
        let Some(want) = expect else {
            return true;
        };
        let now: String = match span {
            Some((a, b)) => self.text.iter().skip(a).take(b.saturating_sub(a)).collect(),
            None => self.text.iter().collect::<String>().trim_end().to_string(),
        };
        now == *want
    }

    /// Rectangles `(row, start, end)` covering characters `span` (or the whole line).
    fn cells_of(&self, span: Option<(usize, usize)>) -> Vec<(usize, usize, usize)> {
        let mut out: Vec<(usize, usize, usize)> = Vec::new();
        match span {
            None => {
                for row in self.first_row..=self.last_row {
                    let end = if row == self.last_row {
                        self.last_used
                    } else {
                        self.cols
                    };
                    if end > 0 {
                        out.push((row, 0, end));
                    }
                }
            }
            Some((a, b)) => {
                for &(row, start, end) in self.chars.iter().skip(a).take(b.saturating_sub(a)) {
                    match out.last_mut() {
                        Some(last) if last.0 == row && last.2 == start => last.2 = end,
                        _ => out.push((row, start, end)),
                    }
                }
            }
        }
        out
    }
}

fn logical_lines(snap: &RenderSnapshot) -> Vec<Line> {
    let mut out = Vec::new();
    let mut r = 0;
    while r < snap.rows {
        let first = r;
        let mut last = r;
        while last + 1 < snap.rows && snap.wrapped.get(last).copied().unwrap_or(false) {
            last += 1;
        }
        let abs = snap.history_size - snap.display_offset.min(snap.history_size) + first;
        let mut chars = Vec::new();
        let mut text = Vec::new();
        let mut last_used = 0;
        for row in first..=last {
            let cells = snap.row(row);
            let mut c = 0;
            while c < cells.len() {
                if cells[c].flags.has(mt_term::CellFlags::WIDE_SPACER) {
                    c += 1;
                    continue;
                }
                let mut end = c + 1;
                if cells
                    .get(end)
                    .is_some_and(|n| n.flags.has(mt_term::CellFlags::WIDE_SPACER))
                {
                    end += 1;
                }
                chars.push((row, c, end));
                text.push(if cells[c].ch == '\0' {
                    ' '
                } else {
                    cells[c].ch
                });
                if row == last && cells[c].ch != ' ' && cells[c].ch != '\0' {
                    last_used = end;
                }
                c = end;
            }
        }
        out.push(Line {
            id: snap.line_base + abs as u64,
            first_row: first,
            last_row: last,
            chars,
            text,
            last_used,
            cols: snap.cols,
        });
        r = last + 1;
    }
    out
}

#[cfg(test)]
mod tests;
