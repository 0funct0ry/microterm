//! `StreamWriter`: a crash-safe recorder over the asciicast [`Writer`] (SPEC §10.10.2).
//!
//! Events stream through a buffered writer, flushed at least once a second, into
//! `<id>.cast.partial`; `finish` renames it to `<id>.cast` and writes the sidecar. A crash
//! leaves the `.partial`, which [`Store::list`](crate::Store::list) recovers.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::store::{display_time, new_id, unix_now, Marker, Origin, Sidecar, Store, StoreError};
use crate::{Error, Header, Utf8Joiner, Writer};

const FLUSH_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Limits {
    /// 0 = unlimited.
    pub max_duration_secs: f64,
    /// 0 = unlimited.
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitHit {
    Duration,
    Size,
}

#[derive(Debug, Clone, Default)]
pub struct StartMeta {
    pub cols: u16,
    pub rows: u16,
    /// Defaults to `<process or domain> — <date time>`.
    pub title: Option<String>,
    pub origin: Origin,
    pub theme: String,
    pub font: String,
    pub capture_input: bool,
    /// `"none"`, `"no_echo"` or `"n/a"` (SPEC §10.10.3).
    pub input_redacted: String,
    pub env: std::collections::BTreeMap<String, String>,
    pub app_version: String,
}

/// Counts bytes handed to the buffered file so size limits need no `stat`.
struct Counting {
    inner: BufWriter<fs::File>,
    n: Arc<AtomicU64>,
}

impl Write for Counting {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let k = self.inner.write(buf)?;
        self.n.fetch_add(k as u64, Ordering::Relaxed);
        Ok(k)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub struct StreamWriter {
    store: Store,
    sidecar: Sidecar,
    partial: PathBuf,
    w: Writer<Counting>,
    bytes: Arc<AtomicU64>,
    out_join: Utf8Joiner,
    in_join: Utf8Joiner,
    limits: Limits,
    last_flush: Instant,
    last_time: f64,
    mark_n: usize,
    capture_input: bool,
    limit_hit: Option<LimitHit>,
}

impl StreamWriter {
    /// Creates `<id>.cast.partial` (mode `0600`) in the store, creating the store `0700` first.
    pub fn create(store: &Store, meta: StartMeta, limits: Limits) -> Result<Self, StoreError> {
        store.ensure_dir()?;
        let now = unix_now();
        let mut id = new_id(now);
        while store.partial_path(&id).exists() || store.cast_path(&id).exists() {
            id = new_id(now);
        }
        let partial = store.partial_path(&id);
        let file = {
            let mut o = fs::OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            o.open(&partial)?
        };
        let bytes = Arc::new(AtomicU64::new(0));
        let sink = Counting {
            inner: BufWriter::new(file),
            n: bytes.clone(),
        };
        let title = meta
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                let who = [&meta.origin.process, &meta.origin.name]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "Recording".into());
                format!("{who} — {}", display_time(now))
            });
        let mut header = Header::new(meta.cols.max(1), meta.rows.max(1));
        header.timestamp = Some(now);
        header.title = Some(title.clone());
        header.env = meta.env.clone();
        let w = Writer::new(sink, &header).map_err(|e| StoreError::Format(e.to_string()))?;
        let sidecar = Sidecar {
            id,
            title,
            created_at: now,
            cols: header.width,
            rows: header.height,
            has_input: false,
            input_redacted: if meta.input_redacted.is_empty() {
                "none".into()
            } else {
                meta.input_redacted
            },
            origin: meta.origin,
            theme: meta.theme,
            font: meta.font,
            app_version: meta.app_version,
            ..Sidecar::default()
        };
        let mut me = Self {
            store: store.clone(),
            sidecar,
            partial,
            w,
            bytes,
            out_join: Utf8Joiner::default(),
            in_join: Utf8Joiner::default(),
            limits,
            last_flush: Instant::now(),
            last_time: 0.0,
            mark_n: 0,
            capture_input: meta.capture_input,
            limit_hit: None,
        };
        // Get the header on disk now so even an instant crash leaves a recoverable file.
        me.w.flush()
            .map_err(|e| StoreError::Format(e.to_string()))?;
        Ok(me)
    }

    pub fn id(&self) -> &str {
        &self.sidecar.id
    }

    pub fn title(&self) -> &str {
        &self.sidecar.title
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub fn limit_hit(&self) -> Option<LimitHit> {
        self.limit_hit
    }

    pub fn marker_count(&self) -> usize {
        self.sidecar.markers.len()
    }

    fn after(&mut self, time: f64) -> Result<Option<LimitHit>, Error> {
        self.last_time = self.last_time.max(time);
        if self.last_flush.elapsed() >= FLUSH_EVERY {
            self.flush()?;
        }
        if self.limit_hit.is_none() {
            if self.limits.max_duration_secs > 0.0
                && self.last_time >= self.limits.max_duration_secs
            {
                self.limit_hit = Some(LimitHit::Duration);
            } else if self.limits.max_bytes > 0 && self.bytes_written() >= self.limits.max_bytes {
                self.limit_hit = Some(LimitHit::Size);
            }
        }
        Ok(self.limit_hit)
    }

    /// Records output. Returns the limit that was reached, if any; the caller then calls
    /// [`finish`](Self::finish). Events after a limit was hit are ignored.
    pub fn output(&mut self, time: f64, bytes: &[u8]) -> Result<Option<LimitHit>, Error> {
        if self.limit_hit.is_some() {
            return Ok(self.limit_hit);
        }
        let b = self.out_join.push(bytes);
        if !b.is_empty() {
            self.w.output(time, &b)?;
        }
        self.after(time)
    }

    /// Records input. A no-op unless the recording captures input.
    pub fn input(&mut self, time: f64, bytes: &[u8]) -> Result<Option<LimitHit>, Error> {
        if !self.capture_input || self.limit_hit.is_some() {
            return Ok(self.limit_hit);
        }
        let b = self.in_join.push(bytes);
        if !b.is_empty() {
            self.w.input(time, &b)?;
            self.sidecar.has_input = true;
        }
        self.after(time)
    }

    pub fn resize(&mut self, time: f64, cols: u16, rows: u16) -> Result<Option<LimitHit>, Error> {
        if self.limit_hit.is_some() {
            return Ok(self.limit_hit);
        }
        self.w.resize(time, cols, rows)?;
        self.after(time)
    }

    /// Adds a marker; `None` labels it "mark N".
    pub fn marker(&mut self, time: f64, label: Option<&str>) -> Result<Option<LimitHit>, Error> {
        if self.limit_hit.is_some() {
            return Ok(self.limit_hit);
        }
        self.mark_n += 1;
        let label = label
            .filter(|l| !l.is_empty())
            .map_or_else(|| format!("mark {}", self.mark_n), str::to_string);
        self.w.marker(time, &label)?;
        self.sidecar.markers.push(Marker {
            time: time.max(0.0),
            label,
        });
        self.after(time)
    }

    pub fn flush(&mut self) -> Result<(), Error> {
        self.last_flush = Instant::now();
        self.w.flush()
    }

    /// Flushes, renames `.cast.partial` to `.cast`, writes the sidecar and returns it.
    pub fn finish(mut self) -> Result<Sidecar, StoreError> {
        let tail = self.out_join.finish();
        if !tail.is_empty() {
            let t = self.last_time;
            self.w
                .output(t, &tail)
                .map_err(|e| StoreError::Format(e.to_string()))?;
        }
        let tail = self.in_join.finish();
        if self.capture_input && !tail.is_empty() {
            let t = self.last_time;
            self.w
                .input(t, &tail)
                .map_err(|e| StoreError::Format(e.to_string()))?;
        }
        self.flush()
            .map_err(|e| StoreError::Format(e.to_string()))?;
        let cast = self.store.cast_path(&self.sidecar.id);
        drop(self.w);
        fs::rename(&self.partial, &cast)?;
        let mut sc = self.sidecar;
        sc.duration_ms = (self.last_time * 1000.0).round() as u64;
        sc.size_bytes = fs::metadata(&cast)?.len();
        self.store.write_sidecar(&sc)?;
        Ok(sc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventKind, Reader};
    use std::io::BufReader;

    fn tmp(name: &str) -> Store {
        let d = std::env::temp_dir().join(format!("mtstream-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        Store::new(d)
    }

    fn meta(input: bool) -> StartMeta {
        StartMeta {
            cols: 90,
            rows: 25,
            capture_input: input,
            input_redacted: "none".into(),
            origin: Origin {
                kind: "local".into(),
                process: "zsh".into(),
                ..Origin::default()
            },
            ..StartMeta::default()
        }
    }

    fn events(store: &Store, id: &str) -> Vec<crate::Event> {
        let f = fs::File::open(store.cast_path(id)).unwrap();
        Reader::new(BufReader::new(f))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn streaming_round_trip_with_sidecar() {
        let s = tmp("rt");
        let mut w = StreamWriter::create(&s, meta(true), Limits::default()).unwrap();
        assert!(s.partial_path(w.id()).is_file());
        w.output(0.1, b"hello").unwrap();
        w.input(0.2, b"ls\r").unwrap();
        w.resize(0.3, 100, 30).unwrap();
        w.marker(0.4, None).unwrap();
        w.marker(0.5, Some("deploy")).unwrap();
        let sc = w.finish().unwrap();
        assert!(!s.partial_path(&sc.id).exists());
        assert!(sc.title.starts_with("zsh — "));
        assert_eq!((sc.cols, sc.rows, sc.duration_ms), (90, 25, 500));
        assert!(sc.has_input);
        assert_eq!(sc.markers[0].label, "mark 1");
        assert_eq!(sc.markers[1].label, "deploy");
        let ev = events(&s, &sc.id);
        assert_eq!(ev.len(), 5);
        assert_eq!(ev[0].kind, EventKind::Output("hello".into()));
        assert_eq!(s.load(&sc.id).unwrap(), sc);
        assert_eq!(s.list().unwrap().len(), 1);
        let _ = fs::remove_dir_all(s.dir());
    }

    #[test]
    fn input_is_dropped_unless_captured() {
        let s = tmp("noinput");
        let mut w = StreamWriter::create(&s, meta(false), Limits::default()).unwrap();
        w.output(0.1, b"x").unwrap();
        w.input(0.2, b"secret").unwrap();
        let sc = w.finish().unwrap();
        assert!(!sc.has_input);
        assert!(!fs::read_to_string(s.cast_path(&sc.id))
            .unwrap()
            .contains("secret"));
        let _ = fs::remove_dir_all(s.dir());
    }

    #[test]
    fn crash_leaves_a_recoverable_partial() {
        let s = tmp("crash");
        let mut w = StreamWriter::create(&s, meta(false), Limits::default()).unwrap();
        w.output(0.1, b"one").unwrap();
        w.flush().unwrap();
        let id = w.id().to_string();
        // Simulate a torn final write, then a kill -9 (no finish, no drop flush).
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(s.partial_path(&id))
            .unwrap();
        f.write_all(b"[0.2,\"o\",\"to").unwrap();
        std::mem::forget(w);
        let l = s.list().unwrap();
        assert_eq!(l.len(), 1);
        assert!(l[0].recovered);
        assert_eq!(events(&s, &id).len(), 1);
        let _ = fs::remove_dir_all(s.dir());
    }

    #[test]
    fn duration_and_size_limits_stop_recording() {
        let s = tmp("limits");
        let lim = Limits {
            max_duration_secs: 1.0,
            max_bytes: 0,
        };
        let mut w = StreamWriter::create(&s, meta(false), lim).unwrap();
        assert_eq!(w.output(0.5, b"a").unwrap(), None);
        assert_eq!(w.output(1.5, b"b").unwrap(), Some(LimitHit::Duration));
        // Further events are ignored once a limit is hit.
        w.output(2.0, b"c").unwrap();
        let sc = w.finish().unwrap();
        assert_eq!(events(&s, &sc.id).len(), 2);

        let lim = Limits {
            max_duration_secs: 0.0,
            max_bytes: 200,
        };
        let mut w = StreamWriter::create(&s, meta(false), lim).unwrap();
        let mut hit = None;
        for i in 0..50 {
            hit = w.output(i as f64 * 0.01, b"0123456789").unwrap();
            if hit.is_some() {
                break;
            }
        }
        assert_eq!(hit, Some(LimitHit::Size));
        w.finish().unwrap();
        let _ = fs::remove_dir_all(s.dir());
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let s = tmp("perm");
        let w = StreamWriter::create(&s, meta(false), Limits::default()).unwrap();
        let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(s.dir()), 0o700);
        assert_eq!(mode(&s.partial_path(w.id())), 0o600);
        let sc = w.finish().unwrap();
        assert_eq!(mode(&s.cast_path(&sc.id)), 0o600);
        assert_eq!(mode(&s.sidecar_path(&sc.id)), 0o600);
        let _ = fs::remove_dir_all(s.dir());
    }

    #[test]
    fn split_utf8_is_joined() {
        let s = tmp("utf8");
        let mut w = StreamWriter::create(&s, meta(false), Limits::default()).unwrap();
        let e = "é".as_bytes();
        w.output(0.1, &e[..1]).unwrap();
        w.output(0.2, &e[1..]).unwrap();
        let sc = w.finish().unwrap();
        let ev = events(&s, &sc.id);
        assert_eq!(ev[0].kind, EventKind::Output("é".into()));
        let _ = fs::remove_dir_all(s.dir());
    }
}
