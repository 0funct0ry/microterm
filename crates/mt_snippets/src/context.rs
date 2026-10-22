//! The only way template functions read the world (SPEC §10.8.3 "Testability").

use chrono::{DateTime, FixedOffset};

/// A fact about the machine or the target pane. The host answers each one (or fails with a
/// reason shown in the toast).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    // System: always the local machine.
    User,
    Hostname,
    Home,
    Shell,
    Tmpdir,
    Cpus,
    LocalIp {
        v6: bool,
    },
    Version,
    // Target pane.
    Cwd,
    /// `"true"` for SSH/serial panes (git functions refuse them).
    IsRemote,
    PaneId,
    TabId,
    WindowId,
    TabIndex,
    TabTitle,
    PaneTitle,
    Workspace,
    Domain,
    RemoteHost,
    Cols,
    Rows,
    FontFamily,
    FontSize,
    Theme,
    FgProcess,
    ShellPid,
    LastCommand(usize),
    LastOutput(usize),
    Selection,
    Clipboard,
}

pub trait Context {
    /// "Now" with the local UTC offset. Captured once per expansion.
    fn now(&self) -> DateTime<FixedOffset>;
    /// Fill `buf` from the OS CSPRNG (a seeded generator in tests).
    fn fill_random(&mut self, buf: &mut [u8]);
    /// microterm's own environment, not the pane shell's.
    fn env(&self, name: &str) -> Option<String>;
    fn fact(&mut self, fact: &Fact) -> Result<String, String>;
    /// Current value of a per-window counter, if it exists.
    fn counter_get(&self, name: &str) -> Option<i64>;
    /// Stores the next value of a counter (called only when an insert is committed).
    fn counter_commit(&mut self, _name: &str, _next: i64) {}
}
