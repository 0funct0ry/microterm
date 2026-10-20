//! The interceptor must not care where reads split the stream, and a hostile payload must not
//! stall or corrupt it.

use std::sync::{Arc, Mutex};

use mt_term::{Emulator, TermEvent, MAX_OSC_PAYLOAD};

const FIXTURE: &str = "plain \u{1b}[31mred\u{1b}[0m text\r\n\
\u{1b}]4;1;?\u{1b}\\\u{1b}]10;rgb:ff/00/00\u{7}\u{1b}]10;?\u{7}\
\u{1b}]5;0;rgb:00/ff/00\u{7}\u{1b}]5;0;?\u{1b}\\\
\u{1b}]21;foreground=?;nonsense=?\u{7}\u{1b}]9;build done\u{7}\u{1b}]9;4;1;40\u{1b}\\\
\u{1b}]22;pointer\u{7}\u{1b}[>1s\u{1b}]52;c;aGk=\u{7}\u{1b}]0;title\u{7}\
after \u{1b}[2;3Hx\u{1b}]133;A\u{7}$ \u{1b}]133;B\u{7}";

fn capture(chunks: &[usize]) -> (Vec<String>, String, Vec<String>, Option<String>, bool) {
    let mut e = Emulator::new(40, 6, 100);
    let replies = Arc::new(Mutex::new(Vec::<u8>::new()));
    let r = replies.clone();
    e.set_reply_sink(move |b| r.lock().unwrap().extend(b));
    let bytes = FIXTURE.as_bytes();
    let mut at = 0;
    let mut i = 0;
    while at < bytes.len() {
        let n = chunks[i % chunks.len()].max(1).min(bytes.len() - at);
        e.advance(&bytes[at..at + n]);
        at += n;
        i += 1;
    }
    let events = e
        .drain_events()
        .iter()
        .filter(|ev| !matches!(ev, TermEvent::Wakeup))
        .map(|ev| format!("{ev:?}"))
        .collect();
    let snap = e.snapshot();
    let lines = (0..snap.rows).map(|r| snap.row_text(r)).collect();
    let reply = String::from_utf8_lossy(&replies.lock().unwrap()).into_owned();
    (
        lines,
        reply,
        events,
        snap.vt.pointer_shape,
        snap.vt.shift_capture,
    )
}

#[test]
fn any_chunking_gives_the_same_grid_replies_and_events() {
    let whole = capture(&[usize::MAX / 2]);
    assert!(!whole.1.is_empty());
    assert_eq!(capture(&[1]), whole);
    assert_eq!(capture(&[2, 3, 5, 7]), whole);
    // Pseudo-random chunk sizes.
    let mut x = 0x2545_f491u32;
    for _ in 0..40 {
        let sizes: Vec<usize> = (0..16)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x % 11) as usize + 1
            })
            .collect();
        assert_eq!(capture(&sizes), whole, "sizes {sizes:?}");
    }
}

#[test]
fn oversized_and_unterminated_payloads_do_not_stall_or_corrupt() {
    let mut e = Emulator::new(40, 4, 10);
    let replies = Arc::new(Mutex::new(Vec::<u8>::new()));
    let r = replies.clone();
    e.set_reply_sink(move |b| r.lock().unwrap().extend(b));
    // Over the cap, terminated: dropped.
    let mut big = b"\x1b]9;".to_vec();
    big.extend(std::iter::repeat_n(b'x', MAX_OSC_PAYLOAD + 100));
    big.push(7);
    e.advance(&big);
    // Unterminated, then aborted by CAN and followed by real output and a real query.
    e.advance(b"\x1b]21;foreground=\x18ok\r\n\x1b]10;?\x07");
    // A malformed CSI > sequence and a lone ESC.
    e.advance(b"\x1b[>99999999s\x1b[>x\x1b\x1b]9;fine\x07");
    let events: Vec<String> = e
        .drain_events()
        .iter()
        .filter(|ev| !matches!(ev, TermEvent::Wakeup))
        .map(|ev| format!("{ev:?}"))
        .collect();
    assert_eq!(events, vec!["Notify(\"fine\")".to_string()]);
    assert!(String::from_utf8_lossy(&replies.lock().unwrap()).starts_with("\u{1b}]10;rgb:"));
    assert_eq!(e.screen_text()[0], "ok");
}
