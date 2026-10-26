//! Agent/scope choices of `ctl skill` (the installer itself lives in `microterm`).

use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Antigravity,
    Generic,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    User,
    Project,
}
