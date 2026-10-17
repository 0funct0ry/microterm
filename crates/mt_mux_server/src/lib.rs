//! `microterm-mux`: the headless per-user daemon that owns PTYs and authoritative terminal
//! state on a remote host (SPEC §3.1, §11.3, D6). No GPUI dependency.
//!
//! - [`paths`]: socket location, private directory, peer-credential check.
//! - [`server`]: the daemon: sessions of panes, many clients, replay on attach.
//! - [`proxy`]: `microterm-mux proxy`, the stdio ⇄ socket bridge run over an SSH exec channel;
//!   starts the daemon on demand.
//! - [`daemon`]: `serve`, including the setsid + double-fork detach.

pub mod daemon;
pub mod paths;
pub mod proxy;
pub mod server;

/// Version string used for the versioned binary name `microterm-mux-<version>` (SPEC §11.3).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
