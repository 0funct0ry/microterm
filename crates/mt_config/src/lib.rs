//! Config schema, defaults, loading, validation, watching and keymap (SPEC §8, §9.1).

pub mod diagnostics;
pub mod diff;
pub mod edit;
pub mod editor;
pub mod keys;
pub mod load;
pub mod menu;
pub mod schema;
pub mod template;
pub mod watch;

pub use diagnostics::{
    parse_str, validate_workspaces, Diagnostic, Loaded, Severity, WorkspaceIssue,
};
pub use diff::{diff, ConfigDiff, Effect};
pub use schema::*;
