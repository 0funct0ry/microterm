//! The serial link: one thread per connection that opens the port, pumps bytes and, when the
//! device goes away (USB unplug, re-enumeration), waits with backoff and opens it again.
//!
//! Unlike SSH there is no daemon to replay from: the terminal keeps its grid, so reconnecting
//! just means opening the same path again. The device is behind [`Opener`] / [`SerialIo`] so
//! the state machine is tested without hardware.

use std::io;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mt_ssh::backoff::Backoff;

use crate::opts::SerialOpts;
use crate::translate::{echo, Translator};

pub trait SerialIo: Send {
    /// Reads available bytes; `TimedOut` / `WouldBlock` mean "nothing yet".
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn send_break(&mut self) -> io::Result<()>;
    fn set_dtr(&mut self, on: bool) -> io::Result<()>;
    fn set_rts(&mut self, on: bool) -> io::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenFailure {
    /// Worth retrying with backoff (device absent, busy, re-enumerating).
    Transient(String),
    /// Retrying cannot help until the user acts (permission denied, bad settings).
    Fatal(String),
}

pub trait Opener: Send + Sync {
    fn open(&self, opts: &SerialOpts) -> Result<Box<dyn SerialIo>, OpenFailure>;
}

/// Wait schedule between reopen attempts.
pub trait Schedule: Send {
    fn next_delay(&mut self) -> Duration;
    fn reset(&mut self);
    /// Attempts started so far.
    fn attempt(&self) -> u32;
}

impl Schedule for Backoff {
    fn next_delay(&mut self) -> Duration {
        Backoff::next_delay(self)
    }
    fn reset(&mut self) {
        Backoff::reset(self)
    }
    fn attempt(&self) -> u32 {
        Backoff::attempt(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, next_in: Duration },
    Disconnected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialEvent {
    State(LinkState),
    /// Bytes from the device, or locally echoed input.
    Bytes(Vec<u8>),
    /// Modem-line state after a toggle.
    Signals {
        dtr: bool,
        rts: bool,
    },
    /// A message for the user (toast).
    Notice(String),
}

#[derive(Debug)]
enum Cmd {
    Input(Vec<u8>),
    Break,
    ToggleDtr,
    ToggleRts,
    ReconnectNow,
    Shutdown,
}

#[derive(Clone)]
pub struct SerialHandle {
    cmds: Sender<Cmd>,
}

impl SerialHandle {
    pub fn input(&self, bytes: Vec<u8>) {
        let _ = self.cmds.send(Cmd::Input(bytes));
    }
    pub fn send_break(&self) {
        let _ = self.cmds.send(Cmd::Break);
    }
    pub fn toggle_dtr(&self) {
        let _ = self.cmds.send(Cmd::ToggleDtr);
    }
    pub fn toggle_rts(&self) {
        let _ = self.cmds.send(Cmd::ToggleRts);
    }
    /// Skip the backoff wait (or retry after a fatal failure).
    pub fn reconnect_now(&self) {
        let _ = self.cmds.send(Cmd::ReconnectNow);
    }
    pub fn shutdown(&self) {
        let _ = self.cmds.send(Cmd::Shutdown);
    }
}

/// Starts the link thread. It ends on `shutdown` or once the event receiver is dropped.
pub fn start(
    opts: SerialOpts,
    opener: Arc<dyn Opener>,
    schedule: Box<dyn Schedule>,
) -> (SerialHandle, async_channel::Receiver<SerialEvent>) {
    let (cmds, rx) = channel();
    let (tx, events) = async_channel::unbounded();
    let name = format!("serial {}", opts.port);
    let _ = std::thread::Builder::new().name(name).spawn(move || {
        Link {
            opts,
            opener,
            schedule,
            rx,
            tx,
            dtr: true,
            rts: true,
        }
        .run()
    });
    (SerialHandle { cmds }, events)
}

struct Link {
    opts: SerialOpts,
    opener: Arc<dyn Opener>,
    schedule: Box<dyn Schedule>,
    rx: Receiver<Cmd>,
    tx: async_channel::Sender<SerialEvent>,
    /// Last modem-line levels we set (ports open with both asserted).
    dtr: bool,
    rts: bool,
}

enum Exit {
    Shutdown,
    /// The port failed; reconnect.
    Lost(String),
}

impl Link {
    /// `false` once nobody listens any more.
    fn emit(&self, ev: SerialEvent) -> bool {
        self.tx.try_send(ev).is_ok()
    }

    fn run(mut self) {
        if !self.emit(SerialEvent::State(LinkState::Connecting)) {
            return;
        }
        loop {
            match self.opener.open(&self.opts) {
                Ok(io) => {
                    self.schedule.reset();
                    self.dtr = true;
                    self.rts = true;
                    if !self.emit(SerialEvent::State(LinkState::Connected)) {
                        return;
                    }
                    match self.pump(io) {
                        Exit::Shutdown => return,
                        Exit::Lost(reason) => {
                            if !self.emit(SerialEvent::Notice(format!(
                                "{} disconnected: {reason}",
                                self.opts.port
                            ))) {
                                return;
                            }
                        }
                    }
                }
                Err(OpenFailure::Fatal(reason)) => {
                    if !self.emit(SerialEvent::State(LinkState::Disconnected { reason })) {
                        return;
                    }
                    if !self.wait_for_retry() {
                        return;
                    }
                    continue;
                }
                Err(OpenFailure::Transient(_)) => {}
            }
            let delay = self.schedule.next_delay();
            if !self.emit(SerialEvent::State(LinkState::Reconnecting {
                attempt: self.schedule.attempt(),
                next_in: delay,
            })) {
                return;
            }
            if !self.wait(delay) {
                return;
            }
        }
    }

    /// Blocks until `ReconnectNow`; `false` on shutdown.
    fn wait_for_retry(&mut self) -> bool {
        loop {
            match self.rx.recv() {
                Ok(Cmd::ReconnectNow) => return true,
                Ok(Cmd::Shutdown) | Err(_) => return false,
                Ok(_) => {} // nothing to send to
            }
        }
    }

    /// Sleeps `delay`, cut short by `ReconnectNow`; `false` on shutdown.
    fn wait(&mut self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(Cmd::ReconnectNow) | Err(RecvTimeoutError::Timeout) => return true,
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return false,
                Ok(_) => {} // input while disconnected is dropped
            }
        }
    }

    fn pump(&mut self, mut io: Box<dyn SerialIo>) -> Exit {
        let mut tr = Translator::new(self.opts.line_ending);
        let mut buf = [0u8; 4096];
        loop {
            loop {
                let cmd = match self.rx.try_recv() {
                    Ok(c) => c,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Exit::Shutdown,
                };
                let res = match cmd {
                    Cmd::Shutdown => return Exit::Shutdown,
                    Cmd::ReconnectNow => Ok(()),
                    Cmd::Input(bytes) => {
                        if self.opts.local_echo && !self.emit(SerialEvent::Bytes(echo(&bytes))) {
                            return Exit::Shutdown;
                        }
                        io.write_all(&tr.translate(&bytes))
                    }
                    Cmd::Break => io.send_break(),
                    Cmd::ToggleDtr => {
                        self.dtr = !self.dtr;
                        io.set_dtr(self.dtr).map(|_| self.signals())
                    }
                    Cmd::ToggleRts => {
                        self.rts = !self.rts;
                        io.set_rts(self.rts).map(|_| self.signals())
                    }
                };
                if let Err(e) = res {
                    return Exit::Lost(e.to_string());
                }
            }
            match io.read(&mut buf) {
                Ok(0) => return Exit::Lost("device closed".into()),
                Ok(n) => {
                    if !self.emit(SerialEvent::Bytes(buf[..n].to_vec())) {
                        return Exit::Shutdown;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Exit::Lost(e.to_string()),
            }
        }
    }

    fn signals(&self) {
        self.emit(SerialEvent::Signals {
            dtr: self.dtr,
            rts: self.rts,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opts::LineEnding;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A scripted device shared between the test and the link thread.
    #[derive(Default)]
    struct Dev {
        rx: VecDeque<u8>,
        written: Vec<u8>,
        calls: Vec<String>,
        /// Reads fail once this is set (the cable was pulled).
        gone: bool,
    }

    struct MockIo(Arc<Mutex<Dev>>);

    impl SerialIo for MockIo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut d = self.0.lock().unwrap();
            if d.gone {
                return Err(io::Error::other("no such device"));
            }
            let n = buf.len().min(d.rx.len());
            for slot in buf.iter_mut().take(n) {
                *slot = d.rx.pop_front().unwrap();
            }
            drop(d);
            if n == 0 {
                std::thread::sleep(Duration::from_millis(1));
                return Err(io::ErrorKind::TimedOut.into());
            }
            Ok(n)
        }
        fn write_all(&mut self, b: &[u8]) -> io::Result<()> {
            self.0.lock().unwrap().written.extend_from_slice(b);
            Ok(())
        }
        fn send_break(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().calls.push("break".into());
            Ok(())
        }
        fn set_dtr(&mut self, on: bool) -> io::Result<()> {
            self.0.lock().unwrap().calls.push(format!("dtr={on}"));
            Ok(())
        }
        fn set_rts(&mut self, on: bool) -> io::Result<()> {
            self.0.lock().unwrap().calls.push(format!("rts={on}"));
            Ok(())
        }
    }

    /// Hands out scripted open results; once empty every open fails transiently.
    struct MockOpener {
        script: Mutex<VecDeque<Result<Arc<Mutex<Dev>>, OpenFailure>>>,
        opens: Mutex<u32>,
    }

    impl MockOpener {
        fn new(script: Vec<Result<Arc<Mutex<Dev>>, OpenFailure>>) -> Arc<Self> {
            Arc::new(MockOpener {
                script: Mutex::new(script.into()),
                opens: Mutex::new(0),
            })
        }
    }

    impl Opener for MockOpener {
        fn open(&self, _: &SerialOpts) -> Result<Box<dyn SerialIo>, OpenFailure> {
            *self.opens.lock().unwrap() += 1;
            match self.script.lock().unwrap().pop_front() {
                Some(Ok(d)) => Ok(Box::new(MockIo(d))),
                Some(Err(e)) => Err(e),
                None => Err(OpenFailure::Transient("absent".into())),
            }
        }
    }

    /// Fixed short waits, but a real attempt counter.
    struct Fast(u32, Duration);

    impl Schedule for Fast {
        fn next_delay(&mut self) -> Duration {
            self.0 += 1;
            self.1
        }
        fn reset(&mut self) {
            self.0 = 0;
        }
        fn attempt(&self) -> u32 {
            self.0
        }
    }

    fn fast() -> Box<dyn Schedule> {
        Box::new(Fast(0, Duration::from_millis(5)))
    }

    fn dev() -> Arc<Mutex<Dev>> {
        Arc::default()
    }

    fn next(rx: &async_channel::Receiver<SerialEvent>) -> SerialEvent {
        rx.recv_blocking().expect("link ended")
    }

    /// Skips events until `pred` matches; fails the test if the link stalls.
    fn until(
        rx: &async_channel::Receiver<SerialEvent>,
        pred: impl Fn(&SerialEvent) -> bool,
    ) -> SerialEvent {
        for _ in 0..200 {
            let ev = next(rx);
            if pred(&ev) {
                return ev;
            }
        }
        panic!("event never arrived");
    }

    #[test]
    fn connects_and_forwards_bytes() {
        let d = dev();
        d.lock().unwrap().rx.extend(b"boot\r\n");
        let (h, rx) = start(
            SerialOpts::new("/dev/x"),
            MockOpener::new(vec![Ok(d)]),
            fast(),
        );
        assert_eq!(next(&rx), SerialEvent::State(LinkState::Connecting));
        assert_eq!(next(&rx), SerialEvent::State(LinkState::Connected));
        assert_eq!(next(&rx), SerialEvent::Bytes(b"boot\r\n".to_vec()));
        h.shutdown();
    }

    #[test]
    fn input_is_translated_and_optionally_echoed() {
        let d = dev();
        let mut o = SerialOpts::new("/dev/x");
        o.line_ending = LineEnding::Crlf;
        o.local_echo = true;
        let (h, rx) = start(o, MockOpener::new(vec![Ok(d.clone())]), fast());
        until(&rx, |e| *e == SerialEvent::State(LinkState::Connected));
        h.input(b"ls\r".to_vec());
        assert_eq!(
            until(&rx, |e| matches!(e, SerialEvent::Bytes(_))),
            SerialEvent::Bytes(b"ls\r\n".to_vec())
        );
        for _ in 0..200 {
            if d.lock().unwrap().written == b"ls\r\n" {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(d.lock().unwrap().written, b"ls\r\n");
        h.shutdown();
    }

    #[test]
    fn unplug_then_replug_reconnects_with_backoff_state() {
        let first = dev();
        let second = dev();
        second.lock().unwrap().rx.extend(b"back");
        let opener = MockOpener::new(vec![
            Ok(first.clone()),
            Err(OpenFailure::Transient("absent".into())),
            Err(OpenFailure::Transient("absent".into())),
            Ok(second),
        ]);
        let (h, rx) = start(SerialOpts::new("/dev/x"), opener.clone(), fast());
        until(&rx, |e| *e == SerialEvent::State(LinkState::Connected));
        first.lock().unwrap().gone = true; // unplug
        let ev = until(&rx, |e| matches!(e, SerialEvent::State(_)));
        assert_eq!(
            ev,
            SerialEvent::State(LinkState::Reconnecting {
                attempt: 1,
                next_in: Duration::from_millis(5)
            })
        );
        // Two failed opens → attempt 3 is announced before the device is back.
        until(&rx, |e| {
            matches!(
                e,
                SerialEvent::State(LinkState::Reconnecting { attempt: 3, .. })
            )
        });
        until(&rx, |e| *e == SerialEvent::State(LinkState::Connected));
        assert_eq!(
            until(&rx, |e| matches!(e, SerialEvent::Bytes(_))),
            SerialEvent::Bytes(b"back".to_vec())
        );
        assert_eq!(*opener.opens.lock().unwrap(), 4);
        h.shutdown();
    }

    #[test]
    fn attempt_counter_resets_after_a_successful_open() {
        let first = dev();
        let opener = MockOpener::new(vec![
            Err(OpenFailure::Transient("absent".into())),
            Ok(first.clone()),
        ]);
        let (h, rx) = start(SerialOpts::new("/dev/x"), opener, fast());
        until(&rx, |e| *e == SerialEvent::State(LinkState::Connected));
        first.lock().unwrap().gone = true;
        // A fresh outage starts again at attempt 1, not 2.
        let ev = until(&rx, |e| {
            matches!(e, SerialEvent::State(LinkState::Reconnecting { .. }))
        });
        assert_eq!(
            ev,
            SerialEvent::State(LinkState::Reconnecting {
                attempt: 1,
                next_in: Duration::from_millis(5)
            })
        );
        h.shutdown();
    }

    #[test]
    fn fatal_failure_waits_for_reconnect_now() {
        let d = dev();
        let opener = MockOpener::new(vec![
            Err(OpenFailure::Fatal("permission denied".into())),
            Ok(d),
        ]);
        let (h, rx) = start(SerialOpts::new("/dev/x"), opener.clone(), fast());
        assert_eq!(
            until(&rx, |e| matches!(
                e,
                SerialEvent::State(LinkState::Disconnected { .. })
            )),
            SerialEvent::State(LinkState::Disconnected {
                reason: "permission denied".into()
            })
        );
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(*opener.opens.lock().unwrap(), 1, "no retry on its own");
        h.reconnect_now();
        until(&rx, |e| *e == SerialEvent::State(LinkState::Connected));
        h.shutdown();
    }

    #[test]
    fn dtr_rts_and_break_reach_the_device() {
        let d = dev();
        let (h, rx) = start(
            SerialOpts::new("/dev/x"),
            MockOpener::new(vec![Ok(d.clone())]),
            fast(),
        );
        until(&rx, |e| *e == SerialEvent::State(LinkState::Connected));
        h.toggle_dtr();
        assert_eq!(
            until(&rx, |e| matches!(e, SerialEvent::Signals { .. })),
            SerialEvent::Signals {
                dtr: false,
                rts: true
            }
        );
        h.toggle_dtr();
        h.toggle_rts();
        h.send_break();
        until(&rx, |e| {
            matches!(
                e,
                SerialEvent::Signals {
                    dtr: true,
                    rts: false
                }
            )
        });
        for _ in 0..200 {
            if d.lock().unwrap().calls.len() == 4 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            d.lock().unwrap().calls,
            ["dtr=false", "dtr=true", "rts=false", "break"]
        );
        h.shutdown();
    }

    #[test]
    fn shutdown_ends_the_event_stream() {
        let (h, rx) = start(
            SerialOpts::new("/dev/x"),
            MockOpener::new(vec![Ok(dev())]),
            fast(),
        );
        until(&rx, |e| *e == SerialEvent::State(LinkState::Connected));
        h.shutdown();
        while rx.recv_blocking().is_ok() {}
    }
}
