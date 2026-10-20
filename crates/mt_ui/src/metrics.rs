//! Layout metrics from SPEC §4.3, in logical pixels.

pub const TITLE_BAR_HEIGHT: f32 = 34.0;
/// Space reserved for the macOS traffic lights.
pub const TRAFFIC_LIGHT_INSET: f32 = 12.0;
pub const TRAFFIC_LIGHTS_WIDTH: f32 = 84.0;
pub const TAB_BAR_HEIGHT: f32 = 32.0;
pub const TAB_MIN_WIDTH: f32 = 120.0;
pub const TAB_MAX_WIDTH: f32 = 240.0;
pub const VIEWPORT_PADDING_X: f32 = 8.0;
pub const VIEWPORT_PADDING_Y: f32 = 6.0;
pub const PALETTE_WIDTH: f32 = 560.0;
pub const PALETTE_MAX_ROWS: usize = 10;
pub const PALETTE_TOP_OFFSET: f32 = 64.0;
pub const LIST_ROW_HEIGHT: f32 = 28.0;
pub const RADIUS_CONTROL: f32 = 4.0;
pub const RADIUS_POPOVER: f32 = 6.0;
pub const SPACING: f32 = 4.0;

pub const UI_FONT: &str = "IBM Plex Sans";
pub const UI_TEXT_SIZE: f32 = 13.0;
pub const UI_TEXT_SIZE_SMALL: f32 = 12.0;
pub const TERM_FONT: &str = "JetBrains Mono";
pub const TERM_TEXT_SIZE: f32 = 13.0;
pub const TERM_LINE_HEIGHT: f32 = 1.2;

/// Advance of JetBrains Mono at 13 px (600/1000 em) and the resulting row height.
pub const CELL_WIDTH: f32 = TERM_TEXT_SIZE * 0.6;
pub const LINE_HEIGHT: f32 = TERM_TEXT_SIZE * TERM_LINE_HEIGHT;

/// Pane dividers (SPEC §4.3): the visible line and the area that grabs the pointer.
pub const DIVIDER_WIDTH: f32 = 1.0;
pub const DIVIDER_HIT_AREA: f32 = 6.0;
