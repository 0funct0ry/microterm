//! Terminal facade over `alacritty_terminal`.
//!
//! This is the only crate allowed to import the engine (SPEC §3.3 D1, enforced by
//! `make check-facade`). The UI sees `RenderSnapshot`, `TermEvent` and the input encoder only.
//!
//! Reader thread: `alacritty_terminal`'s `EventLoop` owns the PTY, reads it in chunks of at
//! most 64 KiB while the terminal lock is held (the rest of its buffer is drained between
//! lock acquisitions), and reports activity through `TermEvent::Wakeup`, coalesced to one
//! pending wakeup (SPEC §3.4).

mod color_names;
mod commands;
mod emulator;
mod events;
mod headless;
mod input;
mod lines;
mod mouse;
mod process;
mod replay;
mod selection;
mod sgr;
mod shell;
mod snapshot;
mod terminal;
mod vt;
pub mod vt_doc;
mod width_filter;

pub use commands::{now_ms, CommandInfo, CommandTracker, Mark, OscSniffer, MAX_COMMANDS};
pub use emulator::{CursorDefault, Emulator, ScrollCmd};
pub use events::{ClipboardKind, Osc52Policy, Palette, ProgressState, TermEvent, OSC52_MAX_BYTES};
pub use headless::{HeadlessOptions, HeadlessPane, PaneEvent};
pub use input::{
    encode_key, encode_paste, paste_needs_confirmation, InputMode, Key, KeyEventKind, KeyInput,
    KittyFlags, Mods,
};
pub use lines::LineCells;
pub use mouse::{
    encode_mouse, MouseButton, MouseEvent, MouseEventKind, MouseModes, MouseTracking,
    PixelModeSniffer,
};
pub use process::ForegroundProcess;
pub use selection::{SelectionKind, SelectionSpan};
pub use sgr::encode_row;
pub use shell::{resolve_shell, resolve_shell_with};
pub use snapshot::{
    Cell, CellColor, CellFlags, Cursor, CursorShape, Damage, Hyperlink, Modes, RenderSnapshot,
    Underline,
};
pub use terminal::{RemoteIo, SpawnOptions, Terminal};
pub use vt::{ColorReport, VtConfig, VtSeq, VtView, MAX_OSC_PAYLOAD};
