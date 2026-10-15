//! Output filter behind `text.wide_regional_indicators`.
//!
//! zsh's line editor counts every regional indicator (U+1F1E6..=U+1F1FF) as two cells, so a
//! flag is four, while the engine places each one in a single cell (two per flag). After zsh
//! draws an autosuggestion containing a flag it moves the cursor back by its own count and ends
//! up in the wrong column. With the option on, this filter appends one space after every
//! regional indicator in the PTY output, so the engine's cursor advances the way zsh expects;
//! the snapshot then draws each indicator-plus-space pair as one wide cell
//! (see `snapshot::widen_regional_indicators`).
//!
//! The filter tracks escape sequences, so indicators inside an OSC title or a CSI/DCS/APC
//! payload are left untouched, and it holds back a UTF-8 sequence that is split across reads.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Csi,
    /// OSC, DCS, APC, PM or SOS payload.
    Str,
    StrEsc,
}

#[derive(Debug)]
pub struct WidthFilter {
    state: State,
    /// Start of a four-byte sequence cut off by the end of the previous read.
    carry: Vec<u8>,
}

impl Default for WidthFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// `F0 9F 87 A6..=BF`, the UTF-8 encoding of a regional indicator.
fn is_regional_indicator(seq: &[u8]) -> bool {
    matches!(seq, [0xF0, 0x9F, 0x87, 0xA6..=0xBF])
}

/// Whether `bytes` could still become a regional indicator once more bytes arrive.
fn is_indicator_prefix(bytes: &[u8]) -> bool {
    const HEAD: [u8; 3] = [0xF0, 0x9F, 0x87];
    bytes.len() < 4 && bytes == &HEAD[..bytes.len()]
}

impl WidthFilter {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            carry: Vec::new(),
        }
    }

    /// Appends the filtered form of `input` to `out`. Every regional indicator gains a
    /// following space; everything else passes through byte for byte.
    pub fn filter(&mut self, input: &[u8], out: &mut Vec<u8>) {
        let mut buf;
        let input = if self.carry.is_empty() {
            input
        } else {
            buf = std::mem::take(&mut self.carry);
            buf.extend_from_slice(input);
            &buf[..]
        };
        let mut i = 0;
        while i < input.len() {
            let b = input[i];
            match self.state {
                State::Ground => match b {
                    0x1B => {
                        self.state = State::Esc;
                        out.push(b);
                        i += 1;
                    }
                    0xF0 => {
                        let rest = &input[i..];
                        if rest.len() >= 4 {
                            out.extend_from_slice(&rest[..4]);
                            if is_regional_indicator(&rest[..4]) {
                                out.push(b' ');
                            }
                            i += 4;
                        } else if is_indicator_prefix(rest) {
                            self.carry = rest.to_vec();
                            return;
                        } else {
                            out.push(b);
                            i += 1;
                        }
                    }
                    _ => {
                        out.push(b);
                        i += 1;
                    }
                },
                State::Esc => {
                    out.push(b);
                    i += 1;
                    self.state = match b {
                        b'[' => State::Csi,
                        b']' | b'P' | b'X' | b'^' | b'_' => State::Str,
                        0x1B => State::Esc,
                        _ => State::Ground,
                    };
                }
                State::Csi => {
                    out.push(b);
                    i += 1;
                    self.state = match b {
                        0x40..=0x7E | 0x18 | 0x1A => State::Ground,
                        0x1B => State::Esc,
                        _ => State::Csi,
                    };
                }
                State::Str => {
                    out.push(b);
                    i += 1;
                    self.state = match b {
                        0x07 | 0x18 | 0x1A => State::Ground,
                        0x1B => State::StrEsc,
                        _ => State::Str,
                    };
                }
                State::StrEsc => {
                    // `ESC \` ends the string; any other byte starts a new escape sequence.
                    self.state = State::Esc;
                }
            }
        }
    }

    /// Forgets escape-sequence state (used when the option is switched off mid-stream).
    pub fn reset(&mut self) {
        self.state = State::Ground;
    }

    /// Bytes held back waiting for the rest of a UTF-8 sequence. Emit them unchanged at EOF.
    pub fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut f = WidthFilter::new();
        let mut out = Vec::new();
        for c in chunks {
            f.filter(c, &mut out);
        }
        out.extend(f.take_pending());
        out
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(run(&["héllo 🦀 漢".as_bytes()]), "héllo 🦀 漢".as_bytes());
    }

    #[test]
    fn each_regional_indicator_gets_a_space() {
        assert_eq!(run(&["🇯🇵".as_bytes()]), "🇯 🇵 ".as_bytes());
        assert_eq!(run(&["a🇫🇷b".as_bytes()]), "a🇫 🇷 b".as_bytes());
    }

    #[test]
    fn split_utf8_sequence_is_held_back() {
        let flag = "🇯".as_bytes();
        for cut in 1..4 {
            let out = run(&[&flag[..cut], &flag[cut..]]);
            assert_eq!(out, "🇯 ".as_bytes(), "cut at {cut}");
        }
    }

    #[test]
    fn other_four_byte_characters_are_not_held_or_changed() {
        let crab = "🦀".as_bytes();
        for cut in 1..4 {
            assert_eq!(run(&[&crab[..cut], &crab[cut..]]), crab, "cut at {cut}");
        }
        // Not an indicator: F0 9F 98 80 (😀).
        assert_eq!(run(&["😀".as_bytes()]), "😀".as_bytes());
    }

    #[test]
    fn escape_sequences_are_left_alone() {
        let osc = "\x1b]0;🇯🇵 title\x07🇯".as_bytes();
        assert_eq!(run(&[osc]), "\x1b]0;🇯🇵 title\x07🇯 ".as_bytes());
        let st = "\x1b]2;🇫🇷\x1b\\🇫".as_bytes();
        assert_eq!(run(&[st]), "\x1b]2;🇫🇷\x1b\\🇫 ".as_bytes());
        let csi = "\x1b[1;31m🇯".as_bytes();
        assert_eq!(run(&[csi]), "\x1b[1;31m🇯 ".as_bytes());
    }

    #[test]
    fn state_survives_chunk_boundaries() {
        // The OSC starts in one read and its payload arrives in the next.
        let out = run(&[b"\x1b]0;", "🇯🇵\x07".as_bytes(), "🇯".as_bytes()]);
        assert_eq!(out, "\x1b]0;🇯🇵\x07🇯 ".as_bytes());
    }

    #[test]
    fn pending_bytes_are_flushed_unchanged_at_eof() {
        let mut f = WidthFilter::new();
        let mut out = Vec::new();
        f.filter(&"🇯".as_bytes()[..2], &mut out);
        assert!(out.is_empty());
        assert_eq!(f.take_pending(), &"🇯".as_bytes()[..2]);
    }
}
