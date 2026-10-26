//! View-only folding of `7721` regions (SPEC §6.4.12). The grid, scrollback, selection text,
//! search and exports never change: a collapsed region is replaced by one summary row in the
//! snapshot only, and mouse rows are translated back to grid lines.

use std::collections::BTreeSet;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::Term;

use crate::snapshot::{grid_row, Cell, CellColor, CellFlags, RenderSnapshot, Underline};

/// A foldable region in stable line ids, `[start, end)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub end: Option<u64>,
    pub title: String,
    pub open: bool,
}

/// Where a viewport row came from: grid lines `first..=last` (a summary row spans many).
#[derive(Clone, Copy, Debug)]
pub struct RowMap {
    pub first: i32,
    pub last: i32,
    pub summary: Option<u64>,
}

#[derive(Debug, Default)]
pub struct FoldState {
    pub collapsed: BTreeSet<u64>,
    pub rows: Vec<RowMap>,
}

impl FoldState {
    /// The grid line a viewport row stands for (`end` picks the last line of a summary).
    pub fn line_of(&self, row: usize, end: bool) -> Option<i32> {
        self.rows
            .get(row)
            .map(|m| if end { m.last } else { m.first })
    }
}

/// Outermost collapsed regions as absolute `[start, end)` plus title and stable start id.
fn collapsed_abs(
    regions: &[Region],
    collapsed: &BTreeSet<u64>,
    trimmed: u64,
    total: usize,
    cursor_abs: usize,
) -> Vec<(usize, usize, String, u64)> {
    let mut v: Vec<(usize, usize, String, u64)> = regions
        .iter()
        .filter(|r| collapsed.contains(&r.start))
        .filter_map(|r| {
            let s = r.start.checked_sub(trimmed)? as usize;
            let e = match r.end {
                Some(e) => (e.checked_sub(trimmed)? as usize).min(total),
                None => total,
            };
            // The region the cursor is writing into is never hidden.
            if r.open && cursor_abs >= s {
                return None;
            }
            (e > s && s < total).then(|| (s, e, r.title.clone(), r.start))
        })
        .collect();
    v.sort_by_key(|r| (r.0, std::cmp::Reverse(r.1)));
    let mut out: Vec<(usize, usize, String, u64)> = Vec::new();
    for r in v {
        if out.last().is_some_and(|l| r.0 < l.1) {
            continue;
        }
        out.push(r);
    }
    out
}

enum Entry {
    Row(usize),
    Summary(usize),
}

/// Rebuilds `snap` (built from `term`) with the collapsed regions folded. Returns the row map.
pub fn apply<T>(
    term: &Term<T>,
    snap: &mut RenderSnapshot,
    regions: &[Region],
    state: &mut FoldState,
    trimmed: u64,
    widen_ri: bool,
) {
    state.rows.clear();
    let grid = term.grid();
    let (hist, rows, cols) = (grid.history_size(), grid.screen_lines(), grid.columns());
    let total = hist + rows;
    let cursor_abs = (hist as i32 + grid.cursor.point.line.0).max(0) as usize;
    let col = collapsed_abs(regions, &state.collapsed, trimmed, total, cursor_abs);
    if col.is_empty() {
        return;
    }
    let region_at = |abs: usize| col.iter().position(|r| abs >= r.0 && abs < r.1);
    let off = snap.display_offset;

    // Entries, top to bottom.
    let mut entries: Vec<Entry> = Vec::with_capacity(rows);
    if off == 0 {
        let mut a = total as isize - 1;
        while a >= 0 && entries.len() < rows {
            match region_at(a as usize) {
                Some(i) => {
                    entries.push(Entry::Summary(i));
                    a = col[i].0 as isize - 1;
                }
                None => {
                    entries.push(Entry::Row(a as usize));
                    a -= 1;
                }
            }
        }
        entries.reverse();
    } else {
        let mut a = hist - off.min(hist);
        while a < total && entries.len() < rows {
            match region_at(a) {
                Some(i) => {
                    entries.push(Entry::Summary(i));
                    a = col[i].1;
                }
                None => {
                    entries.push(Entry::Row(a));
                    a += 1;
                }
            }
        }
    }

    let mut cells = Vec::with_capacity(rows * cols);
    let mut hashes = Vec::with_capacity(rows);
    let mut wrapped = Vec::with_capacity(rows);
    let mut summaries = Vec::with_capacity(rows);
    let mut cursor_row = None;
    for (r, e) in entries.iter().enumerate() {
        match e {
            Entry::Row(abs) => {
                let line = *abs as i32 - hist as i32;
                let (row, wrap, h) = grid_row(term, Line(line), widen_ri);
                cells.extend(row);
                wrapped.push(wrap);
                hashes.push(h);
                summaries.push(None);
                if *abs == cursor_abs {
                    cursor_row = Some(r);
                }
                state.rows.push(RowMap {
                    first: line,
                    last: line,
                    summary: None,
                });
            }
            Entry::Summary(i) => {
                let (s, en, title, id) = &col[*i];
                let text = format!("▸ {title} · {} lines", en - s);
                let row = summary_cells(&text, cols);
                let mut h = std::collections::hash_map::DefaultHasher::new();
                std::hash::Hash::hash(&row, &mut h);
                hashes.push(std::hash::Hasher::finish(&h));
                cells.extend(row);
                wrapped.push(false);
                summaries.push(Some(*id));
                state.rows.push(RowMap {
                    first: *s as i32 - hist as i32,
                    last: *en as i32 - 1 - hist as i32,
                    summary: Some(*id),
                });
            }
        }
    }
    // Pad (top-anchored views that ran out of lines) with blank rows.
    while state.rows.len() < rows {
        cells.extend(summary_cells("", cols));
        wrapped.push(false);
        hashes.push(0);
        summaries.push(None);
        state.rows.push(RowMap {
            first: i32::MAX,
            last: i32::MAX,
            summary: None,
        });
    }

    // Selection endpoints: viewport rows are grid lines + offset; translate to the new rows.
    if let Some(sel) = snap.selection.as_mut() {
        let find = |line: i32, end: bool| -> i32 {
            let l = line - off as i32;
            let hit = state.rows.iter().position(|m| l >= m.first && l <= m.last);
            match hit {
                Some(r) => r as i32,
                None if end => rows as i32,
                None => -1,
            }
        };
        sel.start.0 = find(sel.start.0, false);
        sel.end.0 = find(sel.end.0, true);
    }

    snap.cells = cells;
    snap.row_hashes = hashes;
    snap.wrapped = wrapped;
    snap.fold_summaries = summaries;
    match cursor_row {
        Some(r) => snap.cursor.row = r,
        None => snap.cursor.visible = false,
    }
}

fn summary_cells(text: &str, cols: usize) -> Vec<Cell> {
    let blank = |ch: char, dim: bool| Cell {
        ch,
        link: None,
        zerowidth: None,
        fg: CellColor::DefaultFg,
        bg: CellColor::DefaultBg,
        underline: Underline::None,
        underline_color: None,
        flags: CellFlags(if dim { CellFlags::DIM } else { 0 }),
    };
    let mut row: Vec<Cell> = text.chars().take(cols).map(|c| blank(c, true)).collect();
    row.resize(cols, blank(' ', false));
    row
}
