use super::*;
use crate::Emulator;

fn cfg() -> LineTapCfg {
    LineTapCfg {
        max_line_chars: 4096,
        scan_alt_screen: false,
        partial_after_ms: 250,
    }
}

fn emu_with_tap(cols: usize, rows: usize, scroll: usize) -> (Emulator, Arc<LineQueue>) {
    let mut e = Emulator::new(cols, rows, scroll);
    let q = LineQueue::new(4096);
    e.set_line_tap(Some(cfg()), q.clone(), 7);
    (e, q)
}

fn texts(q: &LineQueue) -> Vec<(String, bool)> {
    q.recv_batch(100_000, Duration::from_millis(0))
        .into_iter()
        .map(|l| (l.text, l.partial))
        .collect()
}

#[test]
fn finished_lines_are_emitted_and_the_cursor_line_is_not() {
    let (mut e, q) = emu_with_tap(40, 5, 100);
    e.advance(b"one\r\ntwo\r\nthr");
    assert_eq!(
        texts(&q),
        vec![("one".into(), false), ("two".into(), false)]
    );
    e.advance(b"ee\r\n");
    assert_eq!(texts(&q), vec![("three".into(), false)]);
}

#[test]
fn sgr_is_stripped_and_blanks_trimmed() {
    let (mut e, q) = emu_with_tap(40, 5, 100);
    e.advance(b"\x1b[31mred\x1b[0m  plain   \r\nx");
    assert_eq!(texts(&q), vec![("red  plain".into(), false)]);
}

#[test]
fn wrapped_rows_join_into_one_logical_line() {
    let (mut e, q) = emu_with_tap(10, 5, 100);
    e.advance(b"abcdefghijklmnopqrstuvwxy\r\nz");
    assert_eq!(texts(&q), vec![("abcdefghijklmnopqrstuvwxy".into(), false)]);
}

#[test]
fn ids_are_stable_while_history_scrolls() {
    let (mut e, q) = emu_with_tap(20, 3, 100);
    for i in 0..10 {
        e.advance(format!("line {i}\r\n").as_bytes());
    }
    e.advance(b"x");
    let all = q.recv_batch(100, Duration::from_millis(0));
    let ids: Vec<u64> = all.iter().map(|l| l.abs_line).collect();
    assert_eq!(ids, (0..10).collect::<Vec<u64>>());
    assert!(all.iter().all(|l| l.pane == 7));
}

#[test]
fn ids_survive_history_trimming() {
    let (mut e, q) = emu_with_tap(20, 3, 10);
    for i in 0..200 {
        e.advance(format!("l{i}\r\n").as_bytes());
    }
    let all = q.recv_batch(1000, Duration::from_millis(0));
    assert_eq!(all.len(), 200);
    assert!(all.windows(2).all(|w| w[1].abs_line == w[0].abs_line + 1));
    assert_eq!(all[199].text, "l199");
}

#[test]
fn the_alternate_screen_is_not_scanned_unless_asked() {
    let (mut e, q) = emu_with_tap(20, 5, 100);
    e.advance(b"main\r\n\x1b[?1049hsecret\r\nmore\r\n\x1b[?1049lafter\r\n");
    let t: Vec<_> = texts(&q).into_iter().map(|x| x.0).collect();
    assert_eq!(t, vec!["main", "after"]);

    let mut e = Emulator::new(20, 5, 100);
    let q = LineQueue::new(100);
    e.set_line_tap(
        Some(LineTapCfg {
            scan_alt_screen: true,
            ..cfg()
        }),
        q.clone(),
        1,
    );
    e.advance(b"\x1b[?1049hsecret\r\nmore\r\n");
    let all = q.recv_batch(10, Duration::from_millis(0));
    assert_eq!(
        all.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        ["secret", "more"]
    );
    assert!(all[0].abs_line & ALT_ID_BIT != 0);
}

#[test]
fn replayed_bytes_emit_nothing_and_do_not_rescan_the_prompt() {
    let (mut e, q) = emu_with_tap(40, 5, 100);
    e.set_line_replay(true);
    e.advance(b"old error: x\r\nContinue? [y/N] ");
    e.settle_line_replay();
    assert!(texts(&q).is_empty());
    // The replayed prompt is not scanned again after a silence either.
    e.flush_line_partial();
    assert!(texts(&q).is_empty());
    e.advance(b"y\r\nnew error: y\r\nz");
    // The answered prompt differs from the replayed one, so it is a new line to scan.
    let t: Vec<_> = texts(&q).into_iter().map(|x| x.0).collect();
    assert_eq!(t, vec!["Continue? [y/N] y", "new error: y"], "{t:?}");
}

#[test]
fn partial_lines_are_scanned_once_and_not_again_when_completed_unchanged() {
    let (mut e, q) = emu_with_tap(40, 5, 100);
    e.advance(b"Continue? [y/N] ");
    assert!(texts(&q).is_empty());
    e.flush_line_partial();
    assert_eq!(texts(&q), vec![("Continue? [y/N]".into(), true)]);
    e.flush_line_partial();
    assert!(
        texts(&q).is_empty(),
        "same (line, content) is never scanned twice"
    );
    // Typing more changes the content: scanned again.
    e.advance(b"y");
    e.flush_line_partial();
    assert_eq!(texts(&q), vec![("Continue? [y/N] y".into(), true)]);
    // Completed unchanged: not emitted a second time.
    e.advance(b"\r\n");
    assert!(texts(&q).is_empty());
    // A completed line that differs from what was scanned is emitted.
    e.advance(b"Pass");
    e.flush_line_partial();
    texts(&q);
    e.advance(b"word\r\n");
    assert_eq!(texts(&q), vec![("Password".into(), false)]);
}

#[test]
fn partial_scanning_can_be_turned_off() {
    let mut e = Emulator::new(40, 5, 100);
    let q = LineQueue::new(10);
    e.set_line_tap(
        Some(LineTapCfg {
            partial_after_ms: 0,
            ..cfg()
        }),
        q.clone(),
        1,
    );
    e.advance(b"prompt> ");
    e.flush_line_partial();
    assert!(texts(&q).is_empty());
}

#[test]
fn long_lines_are_cut_to_the_prefix() {
    let mut e = Emulator::new(50, 5, 100);
    let q = LineQueue::new(10);
    e.set_line_tap(
        Some(LineTapCfg {
            max_line_chars: 100,
            ..cfg()
        }),
        q.clone(),
        1,
    );
    e.advance(format!("{}\r\nz", "a".repeat(1000)).as_bytes());
    let l = q.recv_batch(10, Duration::from_millis(0));
    assert_eq!(l[0].text.chars().count(), 100);
}

#[test]
fn a_full_queue_drops_the_oldest_and_counts() {
    let mut e = Emulator::new(20, 5, 1000);
    let q = LineQueue::new(4);
    e.set_line_tap(Some(cfg()), q.clone(), 1);
    for i in 0..10 {
        e.advance(format!("l{i}\r\n").as_bytes());
    }
    assert_eq!(q.dropped(), 6);
    let t: Vec<_> = q
        .recv_batch(10, Duration::from_millis(0))
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(t, ["l6", "l7", "l8", "l9"]);
}

#[test]
fn removed_tap_emits_nothing_and_reinstall_skips_old_output() {
    let (mut e, q) = emu_with_tap(20, 5, 100);
    e.set_line_tap(None, q.clone(), 7);
    e.advance(b"a\r\nb\r\n");
    assert!(texts(&q).is_empty());
    e.set_line_tap(Some(cfg()), q.clone(), 7);
    e.advance(b"c\r\nd");
    assert_eq!(texts(&q), vec![("c".into(), false)]);
}

#[test]
fn resize_reflow_resets_position_without_rescanning() {
    let (mut e, q) = emu_with_tap(20, 5, 100);
    e.advance(b"one\r\ntwo\r\n");
    texts(&q);
    e.resize(10, 5);
    e.advance(b"three\r\nx");
    assert_eq!(texts(&q), vec![("three".into(), false)]);
}

#[test]
fn capture_lines_returns_context_above_and_only_finished_lines_below() {
    let mut e = Emulator::new(10, 5, 100);
    e.advance(b"one\r\ntwo\r\nMATCHxxxxxxxxxxxxxxxxxxxx\r\nthree\r\nfour\r\nfiv");
    let base = e.snapshot().line_base;
    let id = base + 2;
    let c = e.capture_lines(id, 2, 5, 4096).unwrap();
    assert_eq!(c.before, ["one", "two"]);
    assert_eq!(c.line, "MATCHxxxxxxxxxxxxxxxxxxxx", "wrapped rows join");
    assert_eq!(
        c.after,
        ["three", "four"],
        "the line still being typed is not finished"
    );
    let c = e.capture_lines(id, 1, 1, 4096).unwrap();
    assert_eq!(
        (c.before, c.after),
        (vec!["two".to_string()], vec!["three".to_string()])
    );
    let c = e.capture_lines(base, 3, 0, 4096).unwrap();
    assert!(c.before.is_empty() && c.after.is_empty() && c.line == "one");
    assert_eq!(
        e.capture_lines(id, 0, 0, 3).unwrap().line,
        "MAT",
        "cut per line"
    );
    assert!(e.capture_lines(base + 500, 0, 0, 10).is_none());
}

#[test]
fn capture_lines_is_none_once_the_line_left_the_scrollback() {
    let mut e = Emulator::new(10, 3, 5);
    e.advance(b"old\r\n");
    let id = e.snapshot().line_base;
    for i in 0..30 {
        e.advance(format!("l{i}\r\n").as_bytes());
    }
    assert!(e.capture_lines(id, 0, 0, 10).is_none());
}

fn lazy_emu(cols: usize, rows: usize, scroll: usize) -> (Emulator, Arc<LineQueue>) {
    let mut e = Emulator::new(cols, rows, scroll);
    let q = LineQueue::new(4096);
    e.set_lazy_line_tap(cfg(), q.clone(), 7);
    (e, q)
}

#[test]
fn lazy_mode_records_rows_and_reads_them_out_on_drain_with_the_same_lines_and_ids() {
    let script: &[&[u8]] = &[
        b"one\r\ntw",
        b"o\r\n\x1b[31mthree\x1b[0m\r\n",
        b"fo",
        b"ur\r\npart",
    ];
    let (mut eager, qe) = emu_with_tap(20, 4, 100);
    let (mut lazy, ql) = lazy_emu(20, 4, 100);
    for chunk in script {
        eager.advance(chunk);
        lazy.advance(chunk);
    }
    assert!(
        ql.is_empty(),
        "the reader side reads nothing out of the grid"
    );
    assert_eq!(lazy.drain_line_tap(), 4);
    let key = |q: &LineQueue| {
        q.recv_batch(100, Duration::from_millis(0))
            .into_iter()
            .map(|l| (l.abs_line, l.text, l.partial))
            .collect::<Vec<_>>()
    };
    let (e, l) = (key(&qe), key(&ql));
    assert_eq!(
        e.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(),
        ["one", "two", "three", "four"]
    );
    assert_eq!(e, l);
    assert_eq!(lazy.drain_line_tap(), 0, "nothing new");
}

#[test]
fn lazy_mode_merges_consecutive_ranges_and_survives_scrolling_before_the_drain() {
    let (mut e, q) = lazy_emu(20, 3, 200);
    for i in 0..30 {
        e.advance(format!("line {i}\r\n").as_bytes());
    }
    e.advance(b"x");
    assert_eq!(e.drain_line_tap(), 30);
    let ids: Vec<u64> = q
        .recv_batch(100, Duration::from_millis(0))
        .into_iter()
        .map(|l| l.abs_line)
        .collect();
    assert_eq!(ids, (0..30).collect::<Vec<u64>>());
}

#[test]
fn lazy_mode_loses_only_lines_trimmed_before_the_drain() {
    let (mut e, q) = lazy_emu(20, 3, 5);
    for i in 0..50 {
        e.advance(format!("l{i}\r\n").as_bytes());
    }
    let n = e.drain_line_tap();
    let lines = q.recv_batch(100, Duration::from_millis(0));
    assert_eq!(n, lines.len());
    assert!(
        n <= 8,
        "only the rows still in the scrollback can be read: {n}"
    );
    assert_eq!(lines.last().unwrap().text, "l49");
    assert!(lines.windows(2).all(|w| w[1].abs_line == w[0].abs_line + 1));
}

#[test]
fn lazy_mode_ignores_replayed_bytes_and_resizes_drop_stale_ranges() {
    let (mut e, q) = lazy_emu(20, 4, 100);
    e.set_line_replay(true);
    e.advance(b"old\r\nold two\r\nprompt> ");
    e.settle_line_replay();
    assert_eq!(e.drain_line_tap(), 0);
    e.advance(b"y\r\nnew\r\nz");
    e.advance(b"\r\n");
    e.resize(10, 4);
    assert_eq!(e.drain_line_tap(), 0, "ids are meaningless after a reflow");
    assert!(q.is_empty());
    e.advance(b"after\r\nq");
    assert_eq!(e.drain_line_tap(), 1);
    assert_eq!(q.recv_batch(10, Duration::from_millis(0))[0].text, "after");
}

#[test]
fn lazy_mode_does_not_rescan_a_prompt_that_completes_unchanged() {
    let (mut e, q) = lazy_emu(40, 5, 100);
    e.advance(b"Continue? [y/N] ");
    e.flush_line_partial();
    assert_eq!(q.recv_batch(10, Duration::from_millis(0)).len(), 1);
    e.advance(b"\r\n");
    assert_eq!(e.drain_line_tap(), 0);
    assert!(q.is_empty());
}

fn osc(m: &str) -> Vec<u8> {
    format!("\x1b]133;{m}\x07").into_bytes()
}

#[test]
fn lines_of_the_prompt_and_typed_command_are_flagged_as_input_but_output_is_not() {
    let (mut e, q) = emu_with_tap(60, 8, 100);
    e.advance(&osc("A"));
    e.advance(b"$ ");
    e.advance(&osc("B"));
    e.advance(b"echo error: boom\r\n");
    e.advance(&osc("C"));
    e.advance(b"error: boom\r\n");
    e.advance(&osc("D;0"));
    e.advance(&osc("A"));
    e.advance(b"$ ");
    e.advance(&osc("B"));
    e.advance(b"next");
    e.flush_line_partial();
    let got: Vec<(String, bool, bool)> = q
        .recv_batch(100, Duration::from_millis(0))
        .into_iter()
        .map(|l| (l.text, l.partial, l.input))
        .collect();
    assert_eq!(
        got,
        vec![
            ("$ echo error: boom".to_string(), false, true),
            ("error: boom".to_string(), false, false),
            ("$ next".to_string(), true, true),
        ]
    );
}

#[test]
fn a_program_prompt_after_the_command_started_is_output_not_input() {
    let (mut e, q) = emu_with_tap(60, 8, 100);
    e.advance(&osc("A"));
    e.advance(b"$ ");
    e.advance(&osc("B"));
    e.advance(b"rm -i x\r\n");
    e.advance(&osc("C"));
    e.advance(b"remove x? [y/N] ");
    e.flush_line_partial();
    let got = q.recv_batch(100, Duration::from_millis(0));
    let prompt = got.iter().find(|l| l.text.starts_with("remove")).unwrap();
    assert!(prompt.partial && !prompt.input);
}

#[test]
fn without_shell_integration_nothing_is_flagged_and_lazy_mode_flags_too() {
    let (mut e, q) = emu_with_tap(40, 4, 100);
    e.advance(b"$ echo hi\r\nhi\r\nx");
    assert!(q
        .recv_batch(100, Duration::from_millis(0))
        .iter()
        .all(|l| !l.input));
    let (mut e, q) = lazy_emu(60, 8, 100);
    e.advance(&osc("A"));
    e.advance(b"$ ");
    e.advance(&osc("B"));
    e.advance(b"ls\r\n");
    e.advance(&osc("C"));
    e.advance(b"out\r\n");
    e.advance(&osc("D;0"));
    e.advance(b"q");
    e.drain_line_tap();
    let flags: Vec<(String, bool)> = q
        .recv_batch(100, Duration::from_millis(0))
        .into_iter()
        .map(|l| (l.text, l.input))
        .collect();
    assert_eq!(
        flags,
        vec![("$ ls".to_string(), true), ("out".to_string(), false)]
    );
}

#[test]
fn region_lookup_is_a_binary_search_over_disjoint_ranges() {
    use crate::linetap::in_regions;
    let r = [(2, 4), (10, 11), (20, u64::MAX)];
    for (id, want) in [
        (0, false),
        (2, true),
        (3, true),
        (4, false),
        (10, true),
        (11, false),
        (19, false),
        (20, true),
        (u64::MAX - 1, true),
    ] {
        assert_eq!(in_regions(&r, id), want, "{id}");
    }
    assert!(!in_regions(&[], 5));
}
