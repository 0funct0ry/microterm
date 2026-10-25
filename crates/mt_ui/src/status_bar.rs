//! The window status bar (SPEC §4.4): `7725` status chips of the focused pane on the right and
//! the lock icon while the session override holds the private OSC master switch off.
//!
//! The bar is only drawn while it has something to show, so it costs no space otherwise. Chip
//! text is plain, sanitised text (see `mt_term::private_osc::sanitize`) and never a link.

use gpui::{div, prelude::*, px, AnyElement, Context, Hsla, Rgba, SharedString};
use mt_term::private_osc::{Level, Segment};

use crate::{
    app::AppWindow,
    assets::IconName,
    components::{Icon, Tooltip},
    private_osc::PrivateOscGate,
    theme::Theme,
};

/// Minimum contrast ratio chip text must reach against the chip background.
pub const MIN_CONTRAST: f32 = 4.5;

fn luminance(c: Hsla) -> f32 {
    let c = Rgba::from(c);
    let f = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
}

/// WCAG contrast ratio of two colors.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The text color of a chip: the theme token of the level, or the plain text color when the
/// token cannot be read on the chip background.
pub fn level_color(level: Level, t: &Theme) -> Hsla {
    let token = match level {
        Level::Info => t.accent,
        Level::Ok => t.success,
        Level::Warn => t.warning,
        Level::Error => t.error,
    };
    if contrast(token, t.elevated_surface) >= MIN_CONTRAST {
        token
    } else {
        t.text
    }
}

/// The tooltip of a chip: the origin first, then what the program said.
pub fn chip_tooltip(seg: &Segment, pane: u64) -> String {
    let origin = format!("Set by a program in pane {pane}");
    if seg.tooltip.is_empty() {
        origin
    } else {
        format!("{origin}\n{}", seg.tooltip)
    }
}

impl AppWindow {
    /// The `7725` segments of the focused pane, in slot-name order.
    pub(crate) fn focused_status_segments(&self) -> Vec<Segment> {
        self.tabs
            .get(self.active_tab)
            .and_then(|t| t.session())
            .map(|s| s.term.status_segments())
            .unwrap_or_default()
    }

    /// Pushes the current private-OSC gate into every pane of this window.
    pub(crate) fn push_private_gate(&mut self, _cx: &mut Context<Self>) {
        let vt = crate::app::vt_config(&self.cfg);
        for s in self.tabs.iter().flat_map(|t| t.sessions()) {
            s.term.set_vt_config(vt);
        }
    }

    /// `TogglePrivateOsc`: flips the session override in every window.
    pub(crate) fn toggle_private_osc(&mut self, cx: &mut Context<Self>) {
        let on = crate::private_osc::toggle(&cx.global::<crate::settings::Settings>().config);
        self.toast(
            if on {
                crate::components::ToastKind::Info
            } else {
                crate::components::ToastKind::Warning
            },
            if on {
                "Private OSC is on for this session"
            } else {
                "Private OSC is off: all registered state was cleared"
            },
            cx,
        );
        self.push_private_gate(cx);
        cx.defer(crate::private_osc::push_gate);
        cx.notify();
    }

    pub(crate) fn status_bar(&self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let locked = PrivateOscGate::of(cx).locked();
        let segs = self.focused_status_segments();
        if !locked && segs.is_empty() {
            return None;
        }
        let pane = self.focused_pane().unwrap_or(0);
        let mut chips = div().flex().flex_row().gap_1p5().items_center();
        for seg in segs {
            let tip: SharedString = chip_tooltip(&seg, pane).into();
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("status-{}", seg.slot)))
                    .px_2()
                    .rounded_full()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.elevated_surface)
                    .text_color(level_color(seg.level, t))
                    .text_size(px(11.))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tip.clone())).into())
                    .child(SharedString::from(seg.text)),
            );
        }
        let lock = locked.then(|| {
            div()
                .id("private-osc-lock")
                .tooltip(|_, cx| {
                    cx.new(|_| {
                        Tooltip::new("Private OSC is off for this session (TogglePrivateOsc)")
                    })
                    .into()
                })
                .child(Icon::new(IconName::Lock).size(px(12.)).color(t.text_muted))
        });
        Some(
            div()
                .id("status-bar")
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_2()
                .py_0p5()
                .h(px(22.))
                .border_t_1()
                .border_color(t.border_variant)
                .bg(t.surface)
                .child(div().children(lock))
                .child(chips)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(slot: &str, tip: &str) -> Segment {
        Segment {
            slot: slot.into(),
            text: "x".into(),
            level: Level::Ok,
            tooltip: tip.into(),
        }
    }

    #[test]
    fn tooltip_names_the_origin() {
        assert_eq!(chip_tooltip(&seg("a", ""), 3), "Set by a program in pane 3");
        assert!(chip_tooltip(&seg("a", "main"), 3).ends_with("\nmain"));
    }

    #[test]
    fn contrast_math() {
        let white = gpui::white();
        let black = gpui::black();
        assert!((contrast(white, black) - 21.0).abs() < 0.1);
        assert!((contrast(white, white) - 1.0).abs() < 0.01);
    }
}
