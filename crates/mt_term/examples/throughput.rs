//! Headless throughput benchmark (SPEC §13): how fast the PTY → parser → grid path absorbs
//! output. Run through `make bench`; GPU rendering is not part of this number.
//!
//! Two measurements, both over the same mixed ASCII + SGR payload:
//!   * `parser`: bytes fed straight to the emulator;
//!   * `pty_cat`: `cat FILE` through a real shell and PTY, until a marker appears.
//!
//! With `--triggers N` a third measurement repeats `cat` with the trigger line tap installed and
//! N typical rules scanned by a worker thread (SPEC §10.11.10: within 10 % of the run without).
//!
//! Usage: throughput [--size-mb N] [--triggers N] [--out FILE.json]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use mt_term::{Emulator, LineQueue, LineTapCfg, SpawnOptions, Terminal};
use mt_triggers::{Matcher, Rule};

/// Deterministic mixed payload: plain text, numbers, tabs and SGR colour runs.
fn payload_chunk() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..400u32 {
        match i % 5 {
            0 => out.extend(
                format!("\x1b[3{}mThe quick brown fox {} jumps over the lazy dog\x1b[0m\r\n", i % 8, i).bytes(),
            ),
            1 => out.extend(format!("{i:08} | {:>12.3} | status=ok\tpath=/usr/local/lib/{i}\r\n", i as f64 * 1.37).bytes()),
            2 => out.extend(b"\x1b[1;32mINFO\x1b[0m  request served in 12ms  \x1b[2mid=abc123\x1b[0m\r\n"),
            3 => out.extend("a".repeat(150).bytes().chain(*b"\r\n")),
            _ => out.extend(b"--------------------------------------------------------------------------------\r\n"),
        }
    }
    out
}

/// Eight typical rules: the kind a user keeps while hostile output scrolls by.
fn typical_rules(n: usize) -> Vec<Rule> {
    const RE: [&str; 8] = [
        r"error(\[E\d+\])?: (?P<msg>.+)",
        r"(?i)fail(ed|ure)?",
        r"status=ok",
        r"panicked at",
        r"Continue\? \[y/N\]",
        r"password",
        r"\bINFO\b.*request served in (\d+)ms",
        r"(?i)warning: .+",
    ];
    (0..n)
        .map(|i| {
            let mut r = Rule::new_text(&format!("r{i}"), RE[i % RE.len()]);
            r.id = format!("t_{i}");
            r
        })
        .collect()
}

/// `cat FILE` through a real shell and PTY until a marker appears. With `triggers > 0` the line
/// tap is installed and a worker thread scans every finished line (the real arrangement of
/// SPEC §10.11.3). Returns `(seconds, lines scanned, matches)`.
fn run_cat(path: &std::path::Path, triggers: usize) -> (f64, u64, u64) {
    let (tx, rx) = std::sync::mpsc::channel();
    let term = Terminal::spawn(
        SpawnOptions {
            shell: "/bin/sh".into(),
            cols: 200,
            rows: 50,
            ..Default::default()
        },
        move || {
            let _ = tx.send(());
        },
    )
    .expect("spawn shell");
    std::thread::sleep(Duration::from_millis(500)); // let the shell settle
    term.drain_events();
    let queue = LineQueue::new(4096);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let counts = std::sync::Arc::new((
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ));
    let tap_only = std::env::var_os("MT_BENCH_TAP_ONLY").is_some();
    let worker = (triggers > 0).then(|| {
        term.set_line_tap(
            Some(LineTapCfg {
                max_line_chars: 4096,
                scan_alt_screen: false,
                partial_after_ms: 250,
            }),
            queue.clone(),
            1,
        );
        let matcher = Matcher::new(&typical_rules(if tap_only { 0 } else { triggers }));
        let (q, stop, counts) = (queue.clone(), stop.clone(), counts.clone());
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::Relaxed;
            while !stop.load(Relaxed) {
                let batch = q.recv_batch(4096, Duration::from_millis(50));
                if !batch.is_empty() {
                    std::thread::sleep(Duration::from_millis(1)); // as the real worker does
                }
                for l in batch {
                    counts.0.fetch_add(1, Relaxed);
                    counts
                        .1
                        .fetch_add(matcher.scan(&l.text).len() as u64, Relaxed);
                }
            }
        })
    });
    let start = Instant::now();
    term.write(format!("cat '{}'; echo DONE-$((40+2))\n", path.display()).into_bytes());
    let deadline = start + Duration::from_secs(600);
    let secs = loop {
        // Poll like a window does (a few times a second at most): a busy loop here would
        // hammer the terminal lock and measure the contention instead of the PTY path.
        std::thread::sleep(Duration::from_millis(40));
        while rx.try_recv().is_ok() {}
        term.drain_events();
        let snap = term.snapshot();
        if (0..snap.rows).any(|r| snap.row_text(r) == "DONE-42") {
            break start.elapsed().as_secs_f64();
        }
        assert!(
            Instant::now() < deadline,
            "cat did not finish in 10 minutes"
        );
    };
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    queue.wake();
    if let Some(w) = worker {
        let _ = w.join();
    }
    use std::sync::atomic::Ordering::Relaxed;
    (secs, counts.0.load(Relaxed), counts.1.load(Relaxed))
}

fn main() {
    let mut size_mb = 100usize;
    let mut out_path: Option<String> = None;
    let mut triggers = 0usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--size-mb" => {
                size_mb = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--size-mb N")
            }
            "--triggers" => {
                triggers = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--triggers N")
            }
            "--out" => out_path = args.next(),
            other => panic!("unknown argument {other}"),
        }
    }
    let total = size_mb * 1024 * 1024;
    let chunk = payload_chunk();

    // 1. Parser only.
    let mut emu = Emulator::new(200, 50, 10_000);
    let start = Instant::now();
    let mut fed = 0;
    while fed < total {
        emu.advance(&chunk);
        fed += chunk.len();
    }
    let parser_secs = start.elapsed().as_secs_f64();
    let parser_mbs = fed as f64 / 1048576.0 / parser_secs;

    // 2. cat through a PTY.
    let path = std::env::temp_dir().join(format!("mt-throughput-{}.dat", std::process::id()));
    {
        let mut f = BufWriter::new(File::create(&path).unwrap());
        let mut written = 0;
        while written < total {
            f.write_all(&chunk).unwrap();
            written += chunk.len();
        }
    }
    let file_bytes = std::fs::metadata(&path).unwrap().len();
    // Alternate the two variants and keep each one's best run: a single run of either moves by
    // 10-15 % with whatever else the machine is doing.
    let rounds = if triggers > 0 { 3 } else { 1 };
    let mut cat_secs = f64::MAX;
    let mut best_trig: Option<(f64, u64, u64)> = None;
    for _ in 0..rounds {
        cat_secs = cat_secs.min(run_cat(&path, 0).0);
        if triggers > 0 {
            let (secs, lines, hits) = run_cat(&path, triggers);
            if best_trig.is_none_or(|b| secs < b.0) {
                best_trig = Some((secs, lines, hits));
            }
        }
    }
    let cat_mbs = file_bytes as f64 / 1048576.0 / cat_secs;
    let with_triggers = best_trig
        .map(|(secs, lines, hits)| (secs, file_bytes as f64 / 1048576.0 / secs, lines, hits));
    let _ = std::fs::remove_file(&path);

    let json = format!(
        "{{\n  \"date\": \"{}\",\n  \"os\": \"{}\",\n  \"arch\": \"{}\",\n  \"profile\": \"{}\",\n  \"size_mb\": {},\n  \"parser\": {{ \"seconds\": {:.3}, \"mb_per_s\": {:.1} }},\n  \"pty_cat\": {{ \"seconds\": {:.3}, \"mb_per_s\": {:.1} }}{}\n}}\n",
        env!("MT_BENCH_DATE"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if cfg!(debug_assertions) { "debug" } else { "release" },
        size_mb,
        parser_secs,
        parser_mbs,
        cat_secs,
        cat_mbs,
        match with_triggers {
            Some((secs, mbs, lines, hits)) => format!(
                ",\n  \"pty_cat_triggers\": {{ \"rules\": {triggers}, \"seconds\": {secs:.3}, \"mb_per_s\": {mbs:.1}, \"relative\": {:.3}, \"lines_scanned\": {lines}, \"hits\": {hits} }}",
                mbs / cat_mbs
            ),
            None => String::new(),
        }
    );
    print!("{json}");
    if let Some(p) = out_path {
        std::fs::create_dir_all(std::path::Path::new(&p).parent().unwrap()).ok();
        std::fs::write(&p, &json).unwrap();
        eprintln!("wrote {p}");
    }
}
