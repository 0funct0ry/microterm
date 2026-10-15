//! Workspace/Window/Tab/Pane model, `Domain` trait and mux events (SPEC §5).
//!
//! This crate is GPUI-free and owns no PTYs: it is the bookkeeping model. The UI layer maps
//! pane ids to live terminal sessions. IDs are monotonic `u64`s and are never reused.

mod domain;
mod model;
mod title;

pub use domain::{Domain, DomainId, DomainKind, LocalDomain, SpawnSpec, LOCAL_DOMAIN_ID};
pub use model::{
    Axis, Error, Id, Mux, MuxEvent, Pane, PaneTree, Tab, Window, Workspace, DEFAULT_WORKSPACE,
};
pub use title::{format_title, should_confirm_close, ConfirmClose, TitleParts};
