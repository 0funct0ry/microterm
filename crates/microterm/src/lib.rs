//! The parts of `microterm` that do not need a GUI: the `ctl` command tree and the agent
//! integration (skill installer, MCP server). Kept in a library so `xtask gen-docs` can render
//! the skill reference from the same clap definitions. The clap tree itself lives in `mt_cli`
//! so the macOS launcher can share it.

pub mod agent;
pub use mt_cli::ctl;
