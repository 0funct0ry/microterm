//! End-to-end over a virtual serial pair (`socat` pty pair): open, exchange bytes, kill the pair
//! (the USB device vanished) and bring it back (it re-enumerated). Skipped without `socat`.
//!
//! Linux only: on macOS `serialport` cannot open a pty (its termios ioctl fails with ENOTTY).
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mt_serial::link::Schedule;
use mt_serial::{start, LinkState, SerialEvent, SerialOpts, SystemOpener};

struct Pair {
    child: Child,
}

impl Pair {
    fn spawn(a: &Path, b: &Path) -> Option<Pair> {
        let child = Command::new("socat")
            .arg(format!("pty,raw,echo=0,link={}", a.display()))
            .arg(format!("pty,raw,echo=0,link={}", b.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(a.exists() && b.exists()) {
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Give socat a moment to open both ends.
        std::thread::sleep(Duration::from_millis(100));
        Some(Pair { child })
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Quick(u32);

impl Schedule for Quick {
    fn next_delay(&mut self) -> Duration {
        self.0 += 1;
        Duration::from_millis(100)
    }
    fn reset(&mut self) {
        self.0 = 0;
    }
    fn attempt(&self) -> u32 {
        self.0
    }
}

fn wait_for(
    rx: &async_channel::Receiver<SerialEvent>,
    what: &str,
    pred: impl Fn(&SerialEvent) -> bool,
) -> SerialEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(ev) = rx.try_recv() {
            if pred(&ev) {
                return ev;
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    panic!("timed out waiting for {what}");
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

#[test]
fn survives_the_device_vanishing_and_coming_back() {
    if Command::new("socat").arg("-V").output().is_err() {
        eprintln!("socat not installed; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("mt_serial_pair_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (unique(&dir, "a"), unique(&dir, "b"));
    let Some(pair) = Pair::spawn(&a, &b) else {
        eprintln!("could not start socat; skipping");
        return;
    };

    let opts = SerialOpts::new(a.to_string_lossy());
    let (h, rx) = start(opts, Arc::new(SystemOpener), Box::new(Quick(0)));
    wait_for(&rx, "connect", |e| {
        *e == SerialEvent::State(LinkState::Connected)
    });

    // Device → terminal.
    let mut dev = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&b)
        .unwrap();
    dev.write_all(b"hello\n").unwrap();
    let mut got = Vec::new();
    while !got.ends_with(b"hello\n") {
        match wait_for(&rx, "bytes", |e| matches!(e, SerialEvent::Bytes(_))) {
            SerialEvent::Bytes(b) => got.extend(b),
            _ => unreachable!(),
        }
    }

    // Terminal → device (Enter becomes CR by default).
    h.input(b"ping\r".to_vec());
    let mut buf = [0u8; 16];
    let n = dev.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"ping\r");
    drop(dev);

    // Unplug.
    pair.kill();
    wait_for(&rx, "reconnecting", |e| {
        matches!(e, SerialEvent::State(LinkState::Reconnecting { .. }))
    });
    // Replug: the same path appears again.
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
    let pair = Pair::spawn(&a, &b).expect("socat again");
    wait_for(&rx, "reconnect", |e| {
        *e == SerialEvent::State(LinkState::Connected)
    });

    h.shutdown();
    pair.kill();
    let _ = std::fs::remove_dir_all(&dir);
}
