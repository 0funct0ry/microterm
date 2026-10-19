//! The parts of `microterm` that do not need a GUI: the `ctl` command tree and the agent
//! integration (skill installer, MCP server). Kept in a library so `xtask gen-docs` can render
//! the skill reference from the same clap definitions.

pub mod agent;
pub mod ctl;
