//! Recording mode below the UI (SPEC §10.10, M12.1): the gate, the tap consumer that turns a
//! pane's byte stream into a store recording, and the IPC dispatch for `recording.*`.

pub mod engine;
pub mod export;
pub mod gate;
pub mod ipc;
pub mod panel;
pub mod panel_state;
pub mod panel_view;
pub mod ui;

#[cfg(test)]
mod panel_tests;
#[cfg(test)]
pub(crate) mod tests;
