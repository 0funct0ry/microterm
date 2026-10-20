//! OSC 22 pointer names (SPEC §10.2): the X11 cursor-name list that xterm and Kitty use, mapped
//! to the cursors GPUI can show. A name with no GPUI equivalent is `None` and the caller shows
//! the arrow.

use gpui::CursorStyle;

pub fn cursor_style(name: &str) -> Option<CursorStyle> {
    use CursorStyle as C;
    Some(match name {
        "default" | "left_ptr" | "arrow" | "top_left_arrow" => C::Arrow,
        "text" | "xterm" | "ibeam" => C::IBeam,
        "vertical-text" => C::IBeamCursorForVerticalLayout,
        "pointer" | "hand" | "hand1" | "hand2" | "pointing_hand" => C::PointingHand,
        "crosshair" | "cross" | "tcross" => C::Crosshair,
        "grab" | "openhand" => C::OpenHand,
        "grabbing" | "closedhand" => C::ClosedHand,
        "not-allowed" | "no-drop" | "crossed_circle" | "forbidden" => C::OperationNotAllowed,
        "alias" | "dnd-link" => C::DragLink,
        "e-resize" => C::ResizeRight,
        "w-resize" => C::ResizeLeft,
        "ew-resize" | "sb_h_double_arrow" | "h_double_arrow" => C::ResizeLeftRight,
        "n-resize" => C::ResizeUp,
        "s-resize" => C::ResizeDown,
        "ns-resize" | "sb_v_double_arrow" | "v_double_arrow" => C::ResizeUpDown,
        "col-resize" => C::ResizeColumn,
        "row-resize" => C::ResizeRow,
        "nwse-resize" => C::ResizeUpLeftDownRight,
        "nesw-resize" => C::ResizeUpRightDownLeft,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_names_and_rejects_the_rest() {
        assert_eq!(cursor_style("pointer"), Some(CursorStyle::PointingHand));
        assert_eq!(cursor_style("hand2"), Some(CursorStyle::PointingHand));
        assert_eq!(cursor_style("text"), Some(CursorStyle::IBeam));
        assert_eq!(cursor_style("crosshair"), Some(CursorStyle::Crosshair));
        assert_eq!(
            cursor_style("ew-resize"),
            Some(CursorStyle::ResizeLeftRight)
        );
        assert_eq!(
            cursor_style("not-allowed"),
            Some(CursorStyle::OperationNotAllowed)
        );
        assert_eq!(cursor_style("wait"), None);
        assert_eq!(cursor_style("progress"), None);
        assert_eq!(cursor_style(""), None);
    }
}
