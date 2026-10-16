//! asciicast v2 writer and reader (<https://docs.asciinema.org/manual/asciicast/v2/>).
//!
//! A `.cast` file is newline-delimited JSON: a header object, then one `[time, code, data]` array
//! per event. Codes: `o` output, `i` input (opt-in: it captures keystrokes, including secrets),
//! `r` resize (`"COLSxROWS"`), `m` marker.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("not an asciicast v2 file: {0}")]
    Format(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    pub width: u16,
    pub height: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Header {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            version: 2,
            width,
            height,
            timestamp: None,
            env: BTreeMap::new(),
            title: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    Output(String),
    Input(String),
    Resize { cols: u16, rows: u16 },
    Marker(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Seconds since the recording started.
    pub time: f64,
    pub kind: EventKind,
}

pub struct Writer<W: Write> {
    out: W,
    last: f64,
}

impl<W: Write> Writer<W> {
    pub fn new(mut out: W, header: &Header) -> Result<Self, Error> {
        serde_json::to_writer(&mut out, header).map_err(io::Error::from)?;
        out.write_all(b"\n")?;
        Ok(Self { out, last: 0.0 })
    }

    /// Writes an output event from raw PTY bytes. Invalid UTF-8 is replaced, as asciinema does;
    /// a multi-byte sequence split across two chunks must be joined by the caller (see
    /// [`Utf8Joiner`]).
    pub fn output(&mut self, time: f64, bytes: &[u8]) -> Result<(), Error> {
        self.event(time, "o", &String::from_utf8_lossy(bytes))
    }

    pub fn input(&mut self, time: f64, bytes: &[u8]) -> Result<(), Error> {
        self.event(time, "i", &String::from_utf8_lossy(bytes))
    }

    pub fn resize(&mut self, time: f64, cols: u16, rows: u16) -> Result<(), Error> {
        self.event(time, "r", &format!("{cols}x{rows}"))
    }

    pub fn marker(&mut self, time: f64, label: &str) -> Result<(), Error> {
        self.event(time, "m", label)
    }

    fn event(&mut self, time: f64, code: &str, data: &str) -> Result<(), Error> {
        // Event times must be non-decreasing; round to microseconds like asciinema.
        let t = (time.max(self.last) * 1e6).round() / 1e6;
        self.last = t;
        let line = serde_json::to_string(&(t, code, data)).map_err(io::Error::from)?;
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\n")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), Error> {
        Ok(self.out.flush()?)
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

/// Holds back an incomplete trailing UTF-8 sequence so chunk boundaries don't produce `U+FFFD`.
#[derive(Default)]
pub struct Utf8Joiner {
    pending: Vec<u8>,
}

impl Utf8Joiner {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let keep = match std::str::from_utf8(&self.pending) {
            Ok(_) => 0,
            Err(e) if e.error_len().is_none() => self.pending.len() - e.valid_up_to(),
            Err(_) => 0, // genuinely invalid bytes: flush them (they become U+FFFD)
        };
        let split = self.pending.len() - keep;
        let rest = self.pending.split_off(split);
        std::mem::replace(&mut self.pending, rest)
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

pub struct Reader<R: BufRead> {
    lines: io::Lines<R>,
    header: Header,
    line_no: usize,
}

impl<R: BufRead> Reader<R> {
    pub fn new(input: R) -> Result<Self, Error> {
        let mut lines = input.lines();
        let first = lines
            .next()
            .ok_or_else(|| Error::Format("empty file".into()))??;
        let header: Header =
            serde_json::from_str(&first).map_err(|e| Error::Format(format!("header: {e}")))?;
        if header.version != 2 {
            return Err(Error::Format(format!(
                "unsupported version {}",
                header.version
            )));
        }
        Ok(Self {
            lines,
            header,
            line_no: 1,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = Result<Event, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let line = match self.lines.next()? {
                Ok(l) => l,
                Err(e) => return Some(Err(e.into())),
            };
            self.line_no += 1;
            if line.trim().is_empty() {
                continue;
            }
            return Some(
                parse_event(&line)
                    .map_err(|m| Error::Format(format!("line {}: {m}", self.line_no))),
            );
        }
    }
}

fn parse_event(line: &str) -> Result<Event, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or("expected [time, code, data]")?;
    let time = a[0].as_f64().filter(|t| *t >= 0.0).ok_or("bad time")?;
    let data = a[2].as_str().ok_or("data must be a string")?.to_string();
    let kind = match a[1].as_str().ok_or("bad code")? {
        "o" => EventKind::Output(data),
        "i" => EventKind::Input(data),
        "m" => EventKind::Marker(data),
        "r" => {
            let (c, r) = data.split_once('x').ok_or("bad resize")?;
            EventKind::Resize {
                cols: c.parse().map_err(|_| "bad resize")?,
                rows: r.parse().map_err(|_| "bad resize")?,
            }
        }
        other => return Err(format!("unknown event code {other:?}")),
    };
    Ok(Event { time, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut h = Header::new(80, 24);
        h.timestamp = Some(1_700_000_000);
        h.env.insert("TERM".into(), "xterm-256color".into());
        let mut w = Writer::new(Vec::new(), &h).unwrap();
        w.output(0.1, b"hello \x1b[1mworld\x1b[0m\r\n").unwrap();
        w.input(0.2, b"ls\r").unwrap();
        w.resize(0.3, 100, 30).unwrap();
        w.marker(0.4, "m1").unwrap();
        w.output(0.2, b"clamped").unwrap(); // time going backwards is clamped
        w.into_inner()
    }

    #[test]
    fn writer_output_matches_the_v2_layout() {
        let text = String::from_utf8(sample()).unwrap();
        let mut lines = text.lines();
        let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["version"], 2);
        assert_eq!(header["width"], 80);
        assert_eq!(header["env"]["TERM"], "xterm-256color");
        let first: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first[1], "o");
        assert_eq!(first[2], "hello \u{1b}[1mworld\u{1b}[0m\r\n");
        assert!(text.contains(r#""100x30""#));
    }

    #[test]
    fn round_trips_and_clamps_time() {
        let data = sample();
        let r = Reader::new(&data[..]).unwrap();
        assert_eq!(r.header().height, 24);
        let events: Vec<Event> = r.map(Result::unwrap).collect();
        assert_eq!(events.len(), 5);
        assert_eq!(events[1].kind, EventKind::Input("ls\r".into()));
        assert_eq!(
            events[2].kind,
            EventKind::Resize {
                cols: 100,
                rows: 30
            }
        );
        assert_eq!(events[4].time, 0.4);
        assert!(events.windows(2).all(|w| w[0].time <= w[1].time));
    }

    #[test]
    fn rejects_bad_files() {
        assert!(Reader::new(&b""[..]).is_err());
        assert!(Reader::new(&br#"{"version":1,"width":1,"height":1}"#[..]).is_err());
        let bad = b"{\"version\":2,\"width\":1,\"height\":1}\n[0.1,\"x\",\"a\"]\n";
        let e = Reader::new(&bad[..]).unwrap().next().unwrap().unwrap_err();
        assert!(e.to_string().contains("line 2"));
    }

    #[test]
    fn replaying_a_recording_reproduces_the_grid() {
        use mt_term::Emulator;
        // Chunks split in the middle of escapes and of a multi-byte character, as a PTY would.
        let stream =
            "\x1b[2J\x1b[Hplain \x1b[1;31mbold red\x1b[0m\r\n漢字 \x1b[4:3mcurly\x1b[0m\r\n\
            \x1b[38;2;10;20;30mrgb\x1b[0m \x1b[?1049h alt screen \x1b[?1049l back\r\n";
        let bytes = stream.as_bytes();
        let mut live = Emulator::new(40, 6, 100);
        let mut w = Writer::new(Vec::new(), &Header::new(40, 6)).unwrap();
        let mut join = Utf8Joiner::default();
        for (i, chunk) in bytes.chunks(7).enumerate() {
            live.advance(chunk);
            let ready = join.push(chunk);
            if !ready.is_empty() {
                w.output(i as f64 * 0.01, &ready).unwrap();
            }
        }
        let tail = join.finish();
        assert!(tail.is_empty());
        let cast = w.into_inner();

        let mut replay = Emulator::new(40, 6, 100);
        for ev in Reader::new(&cast[..]).unwrap() {
            if let EventKind::Output(s) = ev.unwrap().kind {
                replay.advance(s.as_bytes());
            }
        }
        assert_eq!(live.screen_text(), replay.screen_text());
        let (a, b) = (live.snapshot(), replay.snapshot());
        for r in 0..a.rows {
            assert_eq!(a.row(r), b.row(r), "row {r}");
        }
    }

    #[test]
    fn utf8_joiner_keeps_split_sequences_together() {
        let bytes = "é漢".as_bytes();
        let mut j = Utf8Joiner::default();
        let mut out = j.push(&bytes[..1]);
        assert!(out.is_empty());
        out.extend(j.push(&bytes[1..4]));
        out.extend(j.push(&bytes[4..]));
        out.extend(j.finish());
        assert_eq!(String::from_utf8(out).unwrap(), "é漢");
    }
}
