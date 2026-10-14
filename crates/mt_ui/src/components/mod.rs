//! The component library (SPEC §4.5). Components are stateless `RenderOnce` builders that read
//! colours from the global [`Theme`]; stateful behaviour lives in the owning view.

mod button;
mod context_menu;
mod icon;
mod keybinding;
mod list;
mod modal;
mod status;
mod tab;
mod text_input;
mod toast;
mod tooltip;

pub use button::{Button, ButtonStyle, IconButton};
pub use context_menu::{ContextMenu, ContextMenuItem};
pub use icon::Icon;
pub use keybinding::KeyBinding;
pub use list::{ListItem, Picker, PickerRow};
pub use modal::Modal;
pub use status::{Pill, StatusDot, StatusKind};
pub use tab::{Tab, TabBar};
pub use text_input::TextInput;
pub use toast::{Banner, Toast, ToastKind};
pub use tooltip::Tooltip;

use gpui::App;

use crate::theme::Theme;

pub(crate) fn theme(cx: &App) -> Theme {
    *cx.global::<Theme>()
}

/// Derives a stable child element id (`parent` + row index).
pub(crate) fn child_id(parent: &gpui::ElementId, index: usize) -> gpui::ElementId {
    (parent.clone(), gpui::SharedString::from(index.to_string())).into()
}
