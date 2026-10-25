//! Trigger rules (SPEC §10.11, D14): the rule model, the `triggers.toml` store with surgical
//! write-back, the line matcher, templates and sanitiser, rate limiters and the pure action
//! "intents" that the UI layer applies. Nothing here touches GPUI, the PTY or the terminal engine.

pub mod capture;
pub mod limits;
pub mod matcher;
pub mod model;
pub mod plan;
pub mod rules;
pub mod store;
pub mod template;

pub use limits::{Limiter, Verdict};
pub use matcher::{Hit, Matcher};
pub use model::*;
pub use plan::{dry_run_event, dry_run_text, plan, DryFiring, FireCtx, Intent};
pub use rules::{load, load_str, Diagnostic, LoadOpts, Loaded};
