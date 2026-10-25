//! The right-hand side slot holds one panel at a time (SPEC §10.10.5). Each panel keeps its
//! own width and open state (`ui.json`); opening one closes the other.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightPanel {
    Snippets,
    Recordings,
    Triggers,
}
