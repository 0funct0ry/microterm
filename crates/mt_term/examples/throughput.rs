//! Headless throughput benchmark (SPEC §13): how fast the PTY → parser → grid path absorbs
//! output. Run through `make bench`; GPU rendering is not part of this number.
//!
//! Two measurements, both over the same mixed ASCII + SGR payload:
//!   * `parser`: bytes fed straight to the emulator;
//!   * `pty_cat`: `cat FILE` through a real shell and PTY, until a marker appears.
//!
//! Usage: throughput [--size-mb N] [--out FILE.json]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use mt_term::{Emulator, SpawnOptions, Terminal};

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

fn main() {
    let mut size_mb = 100usize;
    let mut out_path: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--size-mb" => {
                size_mb = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--size-mb N")
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
    let start = Instant::now();
    term.write(format!("cat '{}'; echo DONE-$((40+2))\n", path.display()).into_bytes());
    let deadline = start + Duration::from_secs(600);
    let cat_secs = loop {
        let _ = rx.recv_timeout(Duration::from_millis(20));
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
    drop(term);
    let _ = std::fs::remove_file(&path);
    let cat_mbs = file_bytes as f64 / 1048576.0 / cat_secs;

    let json = format!(
        "{{\n  \"date\": \"{}\",\n  \"os\": \"{}\",\n  \"arch\": \"{}\",\n  \"profile\": \"{}\",\n  \"size_mb\": {},\n  \"parser\": {{ \"seconds\": {:.3}, \"mb_per_s\": {:.1} }},\n  \"pty_cat\": {{ \"seconds\": {:.3}, \"mb_per_s\": {:.1} }}\n}}\n",
        env!("MT_BENCH_DATE"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if cfg!(debug_assertions) { "debug" } else { "release" },
        size_mb,
        parser_secs,
        parser_mbs,
        cat_secs,
        cat_mbs
    );
    print!("{json}");
    if let Some(p) = out_path {
        std::fs::create_dir_all(std::path::Path::new(&p).parent().unwrap()).ok();
        std::fs::write(&p, &json).unwrap();
        eprintln!("wrote {p}");
    }
}
