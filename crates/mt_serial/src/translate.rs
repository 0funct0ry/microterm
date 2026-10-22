//! Line-ending translation for outgoing bytes and local echo (SPEC §17.3).

use crate::opts::LineEnding;

/// Rewrites every line break in outgoing bytes (`\r`, `\n`, `\r\n`) to the configured ending.
/// Stateful so a `\r` at the end of one chunk and a `\n` at the start of the next count once.
#[derive(Debug, Clone)]
pub struct Translator {
    ending: LineEnding,
    after_cr: bool,
}

impl Translator {
    pub fn new(ending: LineEnding) -> Self {
        Translator {
            ending,
            after_cr: false,
        }
    }

    pub fn translate(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len() + 2);
        for &b in input {
            match b {
                b'\r' => {
                    self.push_ending(&mut out);
                    self.after_cr = true;
                    continue;
                }
                b'\n' if self.after_cr => {} // second half of CRLF
                b'\n' => self.push_ending(&mut out),
                other => out.push(other),
            }
            self.after_cr = false;
        }
        out
    }

    fn push_ending(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(match self.ending {
            LineEnding::Cr => b"\r",
            LineEnding::Lf => b"\n",
            LineEnding::Crlf => b"\r\n",
        });
    }
}

/// What to draw locally for typed bytes when the device does not echo: line breaks start a
/// new line, Backspace/DEL erases.
pub fn echo(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 2);
    let mut after_cr = false;
    for &b in input {
        match b {
            b'\r' => {
                out.extend_from_slice(b"\r\n");
                after_cr = true;
                continue;
            }
            b'\n' if after_cr => {}
            b'\n' => out.extend_from_slice(b"\r\n"),
            0x7f | 0x08 => out.extend_from_slice(b"\x08\x1b[K"),
            other => out.push(other),
        }
        after_cr = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(e: LineEnding, s: &[u8]) -> Vec<u8> {
        Translator::new(e).translate(s)
    }

    #[test]
    fn enter_becomes_the_configured_ending() {
        assert_eq!(t(LineEnding::Cr, b"ls\r"), b"ls\r");
        assert_eq!(t(LineEnding::Lf, b"ls\r"), b"ls\n");
        assert_eq!(t(LineEnding::Crlf, b"ls\r"), b"ls\r\n");
    }

    #[test]
    fn pasted_text_endings_are_normalized() {
        assert_eq!(t(LineEnding::Crlf, b"a\nb\r\nc\rd"), b"a\r\nb\r\nc\r\nd");
        assert_eq!(t(LineEnding::Cr, b"a\nb\r\nc"), b"a\rb\rc");
        assert_eq!(t(LineEnding::Lf, b"a\r\nb"), b"a\nb");
    }

    #[test]
    fn crlf_split_across_chunks_counts_once() {
        let mut tr = Translator::new(LineEnding::Crlf);
        let mut out = tr.translate(b"a\r");
        out.extend(tr.translate(b"\nb"));
        assert_eq!(out, b"a\r\nb");
        // A lone LF after a non-CR chunk is still a break.
        assert_eq!(tr.translate(b"\n"), b"\r\n");
    }

    #[test]
    fn control_bytes_pass_through() {
        assert_eq!(t(LineEnding::Crlf, b"\x03\x1b[A"), b"\x03\x1b[A");
    }

    #[test]
    fn echo_draws_breaks_and_backspace() {
        assert_eq!(echo(b"hi\r"), b"hi\r\n");
        assert_eq!(echo(b"a\r\nb"), b"a\r\nb");
        assert_eq!(echo(b"x\x7f"), b"x\x08\x1b[K");
    }
}
