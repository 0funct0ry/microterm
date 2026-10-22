//! Snippets (SPEC §10.8): the side panel, editor, fill dialog and palette/key/IPC entry points.

pub mod context;
pub mod editor;
pub mod editor_ctl;
pub mod fill;
pub mod gate;
pub mod highlight;
pub mod insert;
pub mod ipc;
pub mod keys;
pub mod library;
pub mod panel_state;
pub mod panel_view;
pub mod ui;

pub use gate::SnippetsGate;

#[cfg(test)]
mod tests;
