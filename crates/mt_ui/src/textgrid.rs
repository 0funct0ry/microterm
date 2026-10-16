//! A plain-text copy of terminal rows with absolute line numbers.
//!
//! Quick select, copy mode and search all work on this instead of the live terminal so their
//! logic is pure and testable. Line `top + i` is `rows[i]`; the live terminal numbers lines
//! from the oldest scrollback row (`mt_term::Emulator::line_cells`).

use mt_term::LineCells;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// One char per column. Blank cells are `' '`; the spacer half of a wide char is `'\0'`.
    pub chars: Vec<char>,
    /// Soft-wrapped into the next row.
    pub wrapped: bool,
}

impl Row {
    pub fn is_blank(&self) -> bool {
        self.chars.iter().all(|c| *c == ' ' || *c == '\0')
    }

    /// Column of the last non-blank cell (its base cell if wide), if any.
    pub fn last_non_blank(&self) -> Option<usize> {
        let mut i = self.chars.iter().rposition(|c| *c != ' ' && *c != '\0')?;
        while i > 0 && self.chars[i] == '\0' {
            i -= 1;
        }
        Some(i)
    }

    /// Cells `from..=to` as text: spacers dropped, optionally trimmed at the right.
    pub fn text(&self, from: usize, to: usize, trim: bool) -> String {
        let to = to.min(self.chars.len().saturating_sub(1));
        if from > to {
            return String::new();
        }
        let s: String = self.chars[from..=to]
            .iter()
            .filter(|c| **c != '\0')
            .collect();
        if trim {
            s.trim_end().to_string()
        } else {
            s
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextGrid {
    pub rows: Vec<Row>,
    pub cols: usize,
    /// Absolute line of `rows[0]`.
    pub top: usize,
}

/// A run of text on one logical (un-wrapped) line with the cell each char came from.
#[derive(Clone, Debug)]
pub struct Logical {
    pub chars: Vec<char>,
    /// `(absolute line, column, width in cells)` per char.
    pub cells: Vec<(usize, usize, usize)>,
}

impl Logical {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Per-row `(absolute line, column, length)` segments covering the char range.
    pub fn segments(&self, range: std::ops::Range<usize>) -> Vec<(usize, usize, usize)> {
        let mut out: Vec<(usize, usize, usize)> = Vec::new();
        for &(abs, col, w) in &self.cells[range] {
            match out.last_mut() {
                Some(last) if last.0 == abs && last.1 + last.2 == col => last.2 += w,
                _ => out.push((abs, col, w)),
            }
        }
        out
    }
}

impl TextGrid {
    pub fn new(top: usize, cols: usize, rows: Vec<Row>) -> Self {
        Self { rows, cols, top }
    }

    pub fn from_cells(top: usize, cols: usize, lines: Vec<LineCells>) -> Self {
        let rows = lines
            .into_iter()
            .map(|l| Row {
                chars: l.chars,
                wrapped: l.wrapped,
            })
            .collect();
        Self { rows, cols, top }
    }

    /// Hard-wrapped rows of `text`, padded to `cols`. A `\0` in `text` is a wide-char spacer.
    pub fn from_strs(cols: usize, lines: &[&str]) -> Self {
        let rows = lines
            .iter()
            .map(|l| {
                let mut chars: Vec<char> = l.chars().collect();
                chars.resize(cols.max(chars.len()), ' ');
                Row {
                    chars,
                    wrapped: false,
                }
            })
            .collect();
        Self { rows, cols, top: 0 }
    }

    /// Marks `abs` as soft-wrapped into the next row.
    pub fn wrap(mut self, abs: usize) -> Self {
        if let Some(r) = self.rows.get_mut(abs - self.top) {
            r.wrapped = true;
        }
        self
    }

    /// One past the last absolute line.
    pub fn end(&self) -> usize {
        self.top + self.rows.len()
    }

    pub fn row(&self, abs: usize) -> Option<&Row> {
        self.rows.get(abs.checked_sub(self.top)?)
    }

    /// Rows joined across soft wraps.
    pub fn logical_lines(&self) -> Vec<Logical> {
        let mut out = Vec::new();
        let mut cur = Logical {
            chars: Vec::new(),
            cells: Vec::new(),
        };
        for (i, row) in self.rows.iter().enumerate() {
            let abs = self.top + i;
            let n = row.chars.len();
            for (col, &c) in row.chars.iter().enumerate() {
                if c == '\0' {
                    continue;
                }
                let w = if col + 1 < n && row.chars[col + 1] == '\0' {
                    2
                } else {
                    1
                };
                cur.chars.push(c);
                cur.cells.push((abs, col, w));
            }
            if !row.wrapped {
                while cur.chars.last() == Some(&' ') {
                    cur.chars.pop();
                    cur.cells.pop();
                }
                out.push(std::mem::replace(
                    &mut cur,
                    Logical {
                        chars: Vec::new(),
                        cells: Vec::new(),
                    },
                ));
            }
        }
        if !cur.chars.is_empty() {
            out.push(cur);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_lines_join_wrapped_rows_and_trim() {
        let g = TextGrid::from_strs(5, &["abcde", "fg", "hi"]).wrap(0);
        let l = g.logical_lines();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].text(), "abcdefg");
        assert_eq!(l[0].cells[5], (1, 0, 1));
        assert_eq!(l[1].text(), "hi");
    }

    #[test]
    fn wide_chars_have_width_two_and_segments_merge() {
        let g = TextGrid::from_strs(8, &["a漢\0字\0b"]);
        let l = &g.logical_lines()[0];
        assert_eq!(l.text(), "a漢字b");
        assert_eq!(l.cells[1], (0, 1, 2));
        assert_eq!(l.segments(1..3), vec![(0, 1, 4)]);
    }

    #[test]
    fn segments_split_across_rows() {
        let g = TextGrid::from_strs(3, &["abc", "def"]).wrap(0);
        let l = &g.logical_lines()[0];
        assert_eq!(l.segments(1..5), vec![(0, 1, 2), (1, 0, 2)]);
    }

    #[test]
    fn row_helpers() {
        let g = TextGrid::from_strs(6, &["ab  ", "    ", "x漢\0"]);
        assert_eq!(g.rows[0].last_non_blank(), Some(1));
        assert!(g.rows[1].is_blank());
        assert_eq!(g.rows[2].last_non_blank(), Some(1));
        assert_eq!(g.rows[2].text(0, 5, true), "x漢");
    }
}
