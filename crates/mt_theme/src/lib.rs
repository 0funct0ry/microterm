//! Theme schema, UI-colour derivation and bundled themes (SPEC §4.1, §8.4).

mod bundled;
pub mod color;
mod registry;
mod theme;

pub use color::{Oklch, Rgba};
pub use registry::{Registry, DEFAULT_DARK, DEFAULT_LIGHT};
pub use theme::{
    contrast_ratio_fg_bg, parse_theme, Appearance, Theme, ThemeError, UiTokens, PERMISSIVE_LICENSES,
};
