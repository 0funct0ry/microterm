//! asciicast v2 exporter: a verified copy of the stored file, optionally transformed.

use std::io::Write;

use mt_record::{EventKind, Writer};

use crate::{
    ExportError, ExportReport, Exporter, OptKind, OptValue, OptionSpec, Options, Progress,
    Recording, Requires,
};

pub struct Cast;

const MAX_SECS: f64 = 1_000_000.0;

impl Exporter for Cast {
    fn id(&self) -> &'static str {
        "cast"
    }
    fn label(&self) -> &'static str {
        "asciicast (.cast)"
    }
    fn extension(&self) -> &'static str {
        "cast"
    }
    fn options(&self) -> Vec<OptionSpec> {
        vec![
            OptionSpec {
                key: "trim_start",
                label: "Trim start (s)",
                help: "Drop this many seconds from the start",
                kind: OptKind::Float {
                    min: 0.0,
                    max: MAX_SECS,
                },
                default: OptValue::Float(0.0),
            },
            OptionSpec {
                key: "trim_end",
                label: "Trim end (s)",
                help: "Drop this many seconds from the end",
                kind: OptKind::Float {
                    min: 0.0,
                    max: MAX_SECS,
                },
                default: OptValue::Float(0.0),
            },
            OptionSpec {
                key: "idle_limit",
                label: "Idle limit (s)",
                help: "Cap pauses at this many seconds (0 = off)",
                kind: OptKind::Float {
                    min: 0.0,
                    max: MAX_SECS,
                },
                default: OptValue::Float(0.0),
            },
            OptionSpec {
                key: "strip_input",
                label: "Strip input",
                help: "Drop keystroke events so a shared file does not leak them",
                kind: OptKind::Bool,
                default: OptValue::Bool(true),
            },
            OptionSpec {
                key: "title",
                label: "Title",
                help: "Replace the title in the header (empty = keep)",
                kind: OptKind::Text,
                default: OptValue::Text(String::new()),
            },
        ]
    }
    fn requires(&self) -> Requires {
        Requires::NONE
    }

    fn export(
        &self,
        rec: &Recording,
        opts: &Options,
        out: &mut dyn Write,
        progress: &dyn Progress,
    ) -> Result<ExportReport, ExportError> {
        let mut reader = rec.reader()?;
        let mut header = reader.header().clone();
        let mut events = Vec::new();
        for e in &mut reader {
            events.push(e?);
        }
        let mut report = ExportReport::default();
        let has_input = events.iter().any(|e| matches!(e.kind, EventKind::Input(_)));
        let (ts, te) = (opts.float("trim_start"), opts.float("trim_end"));
        let idle = opts.float("idle_limit");
        let title = opts.text("title");
        let strip = opts.bool("strip_input") && has_input;

        if ts == 0.0 && te == 0.0 && idle == 0.0 && title.is_empty() && !strip {
            // Identity: the validated file, byte for byte.
            let bytes = std::fs::read(&rec.path)?;
            out.write_all(&bytes)?;
            report.lines = events.len() as u64;
            return Ok(report);
        }

        if !title.is_empty() {
            header.title = Some(title.to_string());
        }
        let end = events.last().map_or(0.0, |e| e.time) - te;
        let mut w = Writer::new(&mut *out, &header)?;
        let (mut prev, mut clock) = (0.0f64, 0.0f64);
        let mut base: Option<f64> = None;
        let total = events.len().max(1);
        for (i, e) in events.into_iter().enumerate() {
            if i % 256 == 0 {
                if progress.cancelled() {
                    return Err(ExportError::Cancelled);
                }
                progress.report(i as f32 / total as f32);
            }
            let gap = e.time - prev;
            prev = e.time;
            clock += if idle > 0.0 { gap.min(idle) } else { gap };
            if e.time < ts || e.time > end {
                continue;
            }
            if strip && matches!(e.kind, EventKind::Input(_)) {
                *report.dropped.entry("input events".into()).or_default() += 1;
                continue;
            }
            // With a trimmed start the first kept event is at time zero.
            let base = *base.get_or_insert(if ts > 0.0 { clock } else { 0.0 });
            let t = clock - base;
            match &e.kind {
                EventKind::Output(s) => w.output(t, s.as_bytes())?,
                EventKind::Input(s) => w.input(t, s.as_bytes())?,
                EventKind::Resize { cols, rows } => w.resize(t, *cols, *rows)?,
                EventKind::Marker(l) => w.marker(t, l)?,
            }
            report.lines += 1;
        }
        w.flush()?;
        progress.report(1.0);
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{default_options, export_to_path, find, testutil, NoProgress};
    use mt_record::Reader;

    fn run(rec: &Recording, kv: &[(&str, &str)]) -> Vec<u8> {
        let e = find("cast").unwrap();
        let mut o = default_options(e, &Default::default());
        let kv: Vec<_> = kv
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        o.apply(&e.options(), &kv).unwrap();
        let out = rec.path.with_extension("out");
        export_to_path(e, rec, &o, &out, true, &NoProgress).unwrap();
        std::fs::read(out).unwrap()
    }

    fn events(b: &[u8]) -> Vec<(f64, String)> {
        Reader::new(b)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let tag = match e.kind {
                    EventKind::Output(s) => format!("o:{s}"),
                    EventKind::Input(s) => format!("i:{s}"),
                    EventKind::Resize { cols, rows } => format!("r:{cols}x{rows}"),
                    EventKind::Marker(s) => format!("m:{s}"),
                };
                (e.time, tag)
            })
            .collect()
    }

    fn sample(d: &std::path::Path) -> Recording {
        testutil::recording(d, true, |w| {
            w.output(1.0, b"a").unwrap();
            w.input(2.0, b"k").unwrap();
            w.output(10.0, b"b").unwrap();
            w.marker(11.0, "m").unwrap();
            w.output(20.0, b"c").unwrap();
        })
    }

    #[test]
    fn identity_is_byte_equal_when_nothing_is_stripped() {
        let d = tempfile::tempdir().unwrap();
        let rec = sample(d.path());
        let got = run(&rec, &[("strip_input", "false")]);
        assert_eq!(got, std::fs::read(&rec.path).unwrap());
    }

    #[test]
    fn strip_input_is_the_default_and_drops_only_input() {
        let d = tempfile::tempdir().unwrap();
        let got = run(&sample(d.path()), &[]);
        let ev = events(&got);
        assert!(ev.iter().all(|(_, t)| !t.starts_with("i:")));
        assert_eq!(ev.len(), 4);
    }

    #[test]
    fn idle_limit_caps_pauses() {
        let d = tempfile::tempdir().unwrap();
        let ev = events(&run(
            &sample(d.path()),
            &[("idle_limit", "1"), ("strip_input", "false")],
        ));
        let times: Vec<f64> = ev.iter().map(|e| e.0).collect();
        assert_eq!(times, [1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn trims_and_retitles() {
        let d = tempfile::tempdir().unwrap();
        let got = run(
            &sample(d.path()),
            &[("trim_start", "5"), ("trim_end", "5"), ("title", "Demo")],
        );
        let ev = events(&got);
        let tags: Vec<_> = ev.iter().map(|e| e.1.as_str()).collect();
        assert_eq!(tags, ["o:b", "m:m"]);
        assert!(String::from_utf8(got)
            .unwrap()
            .contains(r#""title":"Demo""#));
    }
}
