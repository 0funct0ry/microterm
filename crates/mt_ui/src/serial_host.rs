//! Serial ports in the GUI (SPEC §17.3): one [`mt_serial`] link per port domain, remote-style
//! terminals fed from the device, connection state mirrored into the mux, and the modem-line
//! actions. Mirrors [`crate::ssh_host`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use gpui::App;
use mt_mux::{DomainId, DomainState};
use mt_proto::RpcError;
use mt_serial::opts::DOMAIN_PREFIX;
use mt_serial::{LinkState, SerialEvent, SerialHandle, SerialOpts, SystemOpener};
use mt_ssh::backoff::{Backoff, Jitter};
use mt_term::{RemoteIo, SpawnOptions, Terminal};

use crate::mux_host;
use crate::settings::Settings;

/// Output that arrives before the first terminal attaches is kept (up to this many bytes) so
/// a board's boot banner is not lost.
const PENDING_CAP: usize = 64 * 1024;

/// What it takes to reach a serial domain.
#[derive(Clone, Debug)]
pub struct SerialDef {
    /// The mux domain name: the configured name, or the device path for ad-hoc ports.
    pub name: String,
    pub opts: SerialOpts,
}

pub fn opts_of(p: &mt_config::SerialPort) -> Result<SerialOpts, String> {
    let e = |e: mt_serial::OptsError| e.to_string();
    let mut o = SerialOpts::new(&p.port);
    if let Some(b) = p.baud {
        o.baud = mt_serial::opts::parse_baud(&b.to_string()).map_err(e)?;
    }
    if let Some(v) = &p.data_bits {
        o.data_bits = v.parse().map_err(e)?;
    }
    if let Some(v) = &p.parity {
        o.parity = v.parse().map_err(e)?;
    }
    if let Some(v) = &p.stop_bits {
        o.stop_bits = v.parse().map_err(e)?;
    }
    if let Some(v) = &p.flow {
        o.flow = v.parse().map_err(e)?;
    }
    if let Some(v) = &p.line_ending {
        o.line_ending = v.parse().map_err(e)?;
    }
    o.local_echo = p.local_echo;
    Ok(o)
}

/// Looks `name` up among the configured ports, then as an ad-hoc `serial:PORT?...` reference.
/// `None` when it is neither (it may be an SSH domain); `Some(Err)` for bad options.
pub fn def_for(cx: &App, name: &str) -> Option<Result<SerialDef, String>> {
    let cfg = &cx.global::<Settings>().config;
    if cfg.ssh_domains.iter().any(|d| d.name == name) {
        return None;
    }
    if let Some(p) = cfg.serial_ports.iter().find(|p| p.name == name) {
        return Some(opts_of(p).map(|opts| SerialDef {
            name: p.name.clone(),
            opts,
        }));
    }
    if name.starts_with(DOMAIN_PREFIX) {
        return Some(
            SerialOpts::from_domain_ref(name)
                .map(|opts| SerialDef {
                    name: opts.port.clone(),
                    opts,
                })
                .map_err(|e| e.to_string()),
        );
    }
    None
}

/// Names of the configured serial ports, in config order.
pub fn configured_names(cx: &App) -> Vec<String> {
    cx.global::<Settings>()
        .config
        .serial_ports
        .iter()
        .map(|p| p.name.clone())
        .collect()
}

/// Closing the last terminal of a port closes the device.
struct ConnGuard {
    handle: SerialHandle,
    alive: Arc<AtomicBool>,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        self.handle.shutdown();
    }
}

pub(crate) struct Conn {
    /// `/dev/ttyUSB0 · 115200 8N1`, for the overlay.
    pub label: String,
    /// `ttyUSB0 @ 115200`, for the tab.
    pub tab_label: String,
    handle: SerialHandle,
    alive: Arc<AtomicBool>,
    /// Shared by every terminal of the port; the last one to go closes the device.
    guard: Mutex<Weak<ConnGuard>>,
    terminals: Arc<Mutex<Vec<Weak<Terminal>>>>,
    pending: Arc<Mutex<Vec<u8>>>,
    connected_once: bool,
}

#[derive(Default)]
pub struct SerialHub {
    pub(crate) conns: HashMap<DomainId, Conn>,
}

fn state_of(s: &LinkState) -> DomainState {
    match s {
        LinkState::Connecting => DomainState::Connecting,
        LinkState::Connected => DomainState::Connected,
        LinkState::Reconnecting { attempt, next_in } => DomainState::Reconnecting {
            attempt: *attempt,
            next_in_secs: next_in.as_secs_f64().ceil() as u32,
        },
        LinkState::Disconnected { reason } => DomainState::Disconnected {
            reason: reason.clone(),
        },
    }
}

/// Registers the serial domain for `def` and starts its link the first time (or again after
/// its last tab was closed).
pub fn resolve(cx: &mut App, def: SerialDef) -> Result<DomainId, RpcError> {
    let id = mux_host::host(cx).mux.register_serial_domain(&def.name);
    let live = mux_host::host(cx)
        .serial
        .conns
        .get(&id)
        .is_some_and(|c| c.alive.load(Ordering::SeqCst));
    if !live && !cfg!(test) {
        start_conn(cx, id, def);
    }
    Ok(id)
}

fn start_conn(cx: &mut App, id: DomainId, def: SerialDef) {
    let (handle, events) = mt_serial::start(
        def.opts.clone(),
        Arc::new(SystemOpener),
        Box::new(Backoff::new(Jitter::from_clock())),
    );
    let conn = Conn {
        label: format!("{} · {}", def.opts.port, def.opts.framing()),
        tab_label: def.opts.label(),
        handle,
        alive: Arc::new(AtomicBool::new(true)),
        guard: Mutex::new(Weak::new()),
        terminals: Default::default(),
        pending: Default::default(),
        connected_once: false,
    };
    mux_host::host(cx).serial.conns.insert(id, conn);
    cx.spawn(async move |cx| {
        while let Ok(ev) = events.recv().await {
            if cx.update(|cx| handle_event(cx, id, ev)).is_err() {
                break;
            }
        }
    })
    .detach();
}

fn feed_all(conn: &Conn, bytes: &[u8]) {
    let live: Vec<Arc<Terminal>> = conn
        .terminals
        .lock()
        .map(|mut v| {
            v.retain(|w| w.strong_count() > 0);
            v.iter().filter_map(Weak::upgrade).collect()
        })
        .unwrap_or_default();
    if live.is_empty() {
        if let Ok(mut p) = conn.pending.lock() {
            if p.len() + bytes.len() <= PENDING_CAP {
                p.extend_from_slice(bytes);
            }
        }
        return;
    }
    for t in live {
        t.feed(bytes);
    }
}

fn handle_event(cx: &mut App, domain: DomainId, ev: SerialEvent) {
    match ev {
        SerialEvent::State(s) => {
            let host = mux_host::host(cx);
            let mut banner = None;
            if let Some(c) = host.serial.conns.get_mut(&domain) {
                match &s {
                    LinkState::Connected => {
                        if c.connected_once {
                            banner = Some("\r\n\x1b[2m[serial port reconnected]\x1b[0m\r\n");
                        }
                        c.connected_once = true;
                    }
                    LinkState::Reconnecting { attempt: 1, .. } => {
                        banner = Some("\r\n\x1b[2m[serial port disconnected]\x1b[0m\r\n");
                    }
                    _ => {}
                }
                if let Some(b) = banner {
                    feed_all(c, b.as_bytes());
                }
            }
            let _ = host.mux.set_domain_state(domain, state_of(&s));
            mux_host::notify_windows(cx);
        }
        SerialEvent::Bytes(bytes) => {
            if let Some(c) = mux_host::host(cx).serial.conns.get(&domain) {
                feed_all(c, &bytes);
            }
        }
        SerialEvent::Signals { dtr, rts } => mux_host::toast_all(
            cx,
            crate::components::ToastKind::Info,
            format!(
                "DTR {} · RTS {}",
                if dtr { "on" } else { "off" },
                if rts { "on" } else { "off" }
            ),
        ),
        SerialEvent::Notice(text) => {
            mux_host::toast_all(cx, crate::components::ToastKind::Warning, text)
        }
    }
}

/// A terminal for a pane on a serial domain.
pub(crate) fn serial_terminal(
    cx: &mut App,
    domain: DomainId,
    opts: SpawnOptions,
    wake: async_channel::Sender<()>,
) -> Result<Arc<Terminal>, String> {
    let conn = mux_host::host(cx)
        .serial
        .conns
        .get(&domain)
        .ok_or("this domain has no connection")?;
    let guard = {
        let mut slot = conn.guard.lock().map_err(|e| e.to_string())?;
        slot.upgrade().unwrap_or_else(|| {
            let g = Arc::new(ConnGuard {
                handle: conn.handle.clone(),
                alive: conn.alive.clone(),
            });
            *slot = Arc::downgrade(&g);
            g
        })
    };
    let handle = conn.handle.clone();
    let term = Arc::new(Terminal::remote(
        SpawnOptions {
            command: None,
            ..opts
        },
        RemoteIo {
            input: Box::new(move |bytes| {
                let _keep = &guard;
                handle.input(bytes);
            }),
            // A serial line has no window size.
            resize: Box::new(|_, _| {}),
        },
        move || {
            let _ = wake.try_send(());
        },
    ));
    conn.terminals
        .lock()
        .map_err(|e| e.to_string())?
        .push(Arc::downgrade(&term));
    let early = conn
        .pending
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default();
    if !early.is_empty() {
        term.feed(&early);
    }
    Ok(term)
}

fn with_conn(cx: &mut App, domain: DomainId, f: impl FnOnce(&SerialHandle)) {
    if let Some(c) = mux_host::host(cx).serial.conns.get(&domain) {
        f(&c.handle);
    }
}

pub(crate) fn reconnect_now(cx: &mut App, domain: DomainId) {
    with_conn(cx, domain, |h| h.reconnect_now());
}

pub(crate) fn send_break(cx: &mut App, domain: DomainId) {
    with_conn(cx, domain, |h| h.send_break());
}

pub(crate) fn toggle_dtr(cx: &mut App, domain: DomainId) {
    with_conn(cx, domain, |h| h.toggle_dtr());
}

pub(crate) fn toggle_rts(cx: &mut App, domain: DomainId) {
    with_conn(cx, domain, |h| h.toggle_rts());
}

/// Serial ports on this machine (tests see none, so they never touch real devices).
pub fn discovered() -> Vec<mt_serial::PortInfo> {
    if cfg!(test) {
        Vec::new()
    } else {
        mt_serial::discover()
    }
}

/// `/dev/ttyUSB0 · 115200 8N1` of a serial domain.
pub fn label(cx: &mut App, domain: DomainId) -> Option<String> {
    mux_host::host(cx)
        .serial
        .conns
        .get(&domain)
        .map(|c| c.label.clone())
}

/// `ttyUSB0 @ 115200`, the tab title.
pub(crate) fn tab_label(cx: &mut App, domain: DomainId) -> Option<String> {
    mux_host::host(cx)
        .serial
        .conns
        .get(&domain)
        .map(|c| c.tab_label.clone())
}
