//! Theme schema, UI-colour derivation and bundled themes (SPEC §4.1, §8.4).

mod bundled;
pub mod color;
mod registry;
mod theme;

pub use color::{Oklch, Rgba};
pub use registry::{Registry, DEFAULT_DARK, DEFAULT_LIGHT};
pub use theme::{
    contrast_ratio_fg_bg, fit_tab_color, parse_theme, tint, Appearance, Theme, ThemeError,
    UiTokens, MIN_CONTRAST, PERMISSIVE_LICENSES, TAB_BAR_CONTRAST, TINT_ACTIVE, TINT_INACTIVE,
};
