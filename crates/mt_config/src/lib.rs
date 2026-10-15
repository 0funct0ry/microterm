//! Config schema, defaults, loading, validation, watching and keymap (SPEC §8, §9.1).

pub mod diagnostics;
pub mod diff;
pub mod editor;
pub mod keys;
pub mod load;
pub mod schema;
pub mod template;
pub mod watch;

pub use diagnostics::{parse_str, Diagnostic, Loaded, Severity};
pub use diff::{diff, ConfigDiff, Effect};
pub use schema::*;
