//! Re-encodes styled grid rows as text with SGR escapes (`microterm cli get-text --escapes`).

use crate::snapshot::{Cell, CellColor, CellFlags, Underline};

#[derive(Clone, PartialEq)]
struct Style {
    fg: CellColor,
    bg: CellColor,
    flags: u16,
    underline: Underline,
    underline_color: Option<CellColor>,
}

const STYLE_BITS: u16 = CellFlags::BOLD
    | CellFlags::ITALIC
    | CellFlags::DIM
    | CellFlags::INVERSE
    | CellFlags::HIDDEN
    | CellFlags::STRIKE;

impl Style {
    fn of(c: &Cell) -> Self {
        Style {
            fg: c.fg,
            bg: c.bg,
            // Wide/spacer/wrap bits are layout, not style.
            flags: c.flags.0 & STYLE_BITS,
            underline: c.underline,
            underline_color: c.underline_color,
        }
    }

    fn is_default(&self) -> bool {
        *self == Style::of(&Cell::default())
    }

    fn sgr(&self) -> String {
        let mut p: Vec<String> = vec!["0".into()];
        for (bit, code) in [
            (CellFlags::BOLD, "1"),
            (CellFlags::DIM, "2"),
            (CellFlags::ITALIC, "3"),
            (CellFlags::INVERSE, "7"),
            (CellFlags::HIDDEN, "8"),
            (CellFlags::STRIKE, "9"),
        ] {
            if self.flags & bit != 0 {
                p.push(code.into());
            }
        }
        match self.underline {
            Underline::None => {}
            Underline::Single => p.push("4".into()),
            Underline::Double => p.push("4:2".into()),
            Underline::Curly => p.push("4:3".into()),
            Underline::Dotted => p.push("4:4".into()),
            Underline::Dashed => p.push("4:5".into()),
        }
        color(&mut p, self.fg, 30, 90, "38");
        color(&mut p, self.bg, 40, 100, "48");
        if let Some(c) = self.underline_color {
            color(&mut p, c, 0, 0, "58");
        }
        format!("\x1b[{}m", p.join(";"))
    }
}

fn color(p: &mut Vec<String>, c: CellColor, base: u8, bright: u8, ext: &str) {
    match c {
        CellColor::DefaultFg | CellColor::DefaultBg => {}
        CellColor::Indexed(i) if i < 8 && base != 0 => p.push((base + i).to_string()),
        CellColor::Indexed(i) if i < 16 && base != 0 => p.push((bright + i - 8).to_string()),
        CellColor::Indexed(i) => p.push(format!("{ext};5;{i}")),
        CellColor::Rgb(r, g, b) => p.push(format!("{ext};2;{r};{g};{b}")),
    }
}

/// One row as text: trailing blanks trimmed, an SGR sequence on every style change and a final
/// reset when the row ends styled.
pub fn encode_row(row: &[Cell]) -> String {
    let end = row.iter().rposition(|c| !c.is_blank()).map_or(0, |i| i + 1);
    let mut out = String::new();
    let mut cur = Style::of(&Cell::default());
    for c in &row[..end] {
        if c.flags.has(CellFlags::WIDE_SPACER) {
            continue;
        }
        let s = Style::of(c);
        if s != cur {
            out.push_str(&if s.is_default() {
                "\x1b[0m".to_string()
            } else {
                s.sgr()
            });
            cur = s;
        }
        out.push(if c.ch == '\0' { ' ' } else { c.ch });
        if let Some(z) = &c.zerowidth {
            out.extend(z.iter());
        }
    }
    if !cur.is_default() {
        out.push_str("\x1b[0m");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    const STYLED: &str = "plain \x1b[1;31mbold red\x1b[0m \x1b[3;4:3;38;2;1;2;3mcurly\x1b[0m \
        \x1b[7;48;5;200m inv \x1b[0m \x1b[9;2;94;102mx\x1b[0m 漢字";

    fn row(text: &str) -> Vec<Cell> {
        let mut e = Emulator::new(60, 3, 0);
        e.advance(text.as_bytes());
        e.snapshot().row(0).to_vec()
    }

    #[test]
    fn encoded_row_reproduces_the_same_cells() {
        let original = row(STYLED);
        let encoded = encode_row(&original);
        assert!(encoded.contains("\x1b[0;1;31m"));
        let again = row(&encoded);
        for (a, b) in original.iter().zip(&again) {
            assert_eq!(
                (a.ch, a.fg, a.bg, a.flags, a.underline, a.underline_color),
                (b.ch, b.fg, b.bg, b.flags, b.underline, b.underline_color)
            );
        }
    }

    #[test]
    fn plain_rows_have_no_escapes_and_trim() {
        assert_eq!(encode_row(&row("hello")), "hello");
        assert_eq!(encode_row(&row("")), "");
    }

    #[test]
    fn a_styled_tail_is_reset() {
        assert!(encode_row(&row("\x1b[1mhi")).ends_with("\x1b[0m"));
    }
}
