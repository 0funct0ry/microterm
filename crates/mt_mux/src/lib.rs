//! Workspace/Window/Tab/Pane model, `Domain` trait and mux events (SPEC §5).
//!
//! This crate is GPUI-free and owns no PTYs: it is the bookkeeping model. The UI layer maps
//! pane ids to live terminal sessions. IDs are monotonic `u64`s and are never reused.

pub mod color_rules;
pub mod ctl;
mod domain;
mod model;
pub mod status;
mod title;
pub mod tree;
pub mod wait;

pub use color_rules::ColorRules;
pub use domain::{
    Domain, DomainId, DomainKind, DomainState, LocalDomain, SerialDomain, SpawnSpec, SplitOrigin,
    SshDomain, LOCAL_DOMAIN_ID,
};
pub use model::{
    Axis, Error, Id, Mux, MuxEvent, NotifySource, Pane, PaneClosed, PaneTree, Tab, Window,
    Workspace, DEFAULT_WORKSPACE,
};
pub use mt_proto::color::{
    resolve as resolve_color, ColorSource, NamedColor, ParseColorError, TabColor,
};
pub use status::{sanitize, NotifyLimiter, Status, StatusSource, NOTIFY_BURST};
pub use title::{format_title, should_confirm_close, ConfirmClose, TitleParts};
pub use tree::{Dir, Placement, Rect, SplitRect};
