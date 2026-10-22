//! Serial port connections (SPEC §17.3): option parsing, port discovery, line-ending
//! translation and a reconnecting link that turns a serial device into a byte pipe.
//!
//! Nothing here knows about the GUI. The UI feeds `SerialEvent::Bytes` into a remote
//! terminal and sends keystrokes through `SerialHandle`.

pub mod discover;
pub mod link;
pub mod opts;
pub mod port;
pub mod translate;

pub use discover::{discover, PortInfo};
pub use link::{start, LinkState, OpenFailure, Opener, SerialEvent, SerialHandle, SerialIo};
pub use opts::{DataBits, Flow, LineEnding, OptsError, Parity, SerialOpts, StopBits};
pub use port::SystemOpener;
