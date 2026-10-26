//! The `microterm` command-line definition (SPEC §15.5), shared by the app binary and the macOS
//! launcher so `--help`, `--version`, usage errors and exit codes are identical. No GPUI and no
//! terminal-emulation crate: the launcher links this crate and must stay small.

pub mod agent;
pub mod cli;
pub mod ctl;

pub use cli::*;
