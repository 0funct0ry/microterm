//! Snippets (SPEC §10.8): model, `snippets.toml` store and the `{{ }}` template engine.
//! No GPUI and no terminal engine (checked by `make check-snippets-pure`).

pub mod analyze;
pub mod context;
pub mod docs;
pub mod eval;
pub mod funcs;
pub mod model;
pub mod store;
pub mod system;
pub mod template;
pub mod value;

pub use analyze::{Analysis, Prompt, PromptKind};
pub use chrono;
pub use context::{Context, Fact};
pub use eval::{expand, sanitize, Expanded, Mode, RuntimeError, Session, Template};
pub use model::{Snippet, SnippetSet};
pub use value::{Secret, Val, MASK};
