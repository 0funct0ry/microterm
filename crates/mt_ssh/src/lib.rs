//! SSH client for microterm (SPEC §11): `~/.ssh/config`, host keys, daemon bootstrap, the mux
//! link and reconnection. Runs on a background Tokio runtime (SPEC §3.4).

pub mod backoff;
pub mod bootstrap;
pub mod config;
pub mod dialer;
pub mod hostkeys;
pub mod link;
pub mod plain;
pub mod prompt;
pub mod runtime;
pub mod session;
