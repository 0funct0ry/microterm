//! Which overlays are open and which one owns the keyboard (SPEC §4.5: "All overlays share one
//! focus model. Esc closes the top overlay. Focus returns to the pane that opened it.").

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayKind {
    CommandPalette,
    Launcher,
    WorkspaceSwitcher,
    ThemeSelector,
    QuickSelect,
    CopyMode,
    Search,
    SshReconnect,
}

/// Overlays in the same class replace each other instead of stacking.
#[derive(PartialEq, Eq)]
enum Class {
    Picker,
    Mode,
    Search,
    Ssh,
}

impl OverlayKind {
    fn class(self) -> Class {
        match self {
            Self::CommandPalette
            | Self::Launcher
            | Self::WorkspaceSwitcher
            | Self::ThemeSelector => Class::Picker,
            Self::QuickSelect | Self::CopyMode => Class::Mode,
            Self::Search => Class::Search,
            Self::SshReconnect => Class::Ssh,
        }
    }
}

/// Where keyboard focus belongs right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    /// No overlay is open: the terminal viewport.
    Viewport,
    Overlay(OverlayKind),
}

#[derive(Default, Debug)]
pub struct OverlayStack {
    stack: Vec<OverlayKind>,
}

impl OverlayStack {
    pub fn top(&self) -> Option<OverlayKind> {
        self.stack.last().copied()
    }

    pub fn contains(&self, kind: OverlayKind) -> bool {
        self.stack.contains(&kind)
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Opens `kind` on top, replacing any open overlay of the same class.
    pub fn open(&mut self, kind: OverlayKind) {
        self.stack.retain(|k| k.class() != kind.class());
        self.stack.push(kind);
    }

    /// Opens `kind`, or closes it when it is already the top overlay (shortcut pressed twice).
    pub fn toggle(&mut self, kind: OverlayKind) {
        if self.top() == Some(kind) {
            self.stack.pop();
        } else {
            self.open(kind);
        }
    }

    /// Esc: closes only the top overlay and returns it.
    pub fn dismiss_top(&mut self) -> Option<OverlayKind> {
        self.stack.pop()
    }

    pub fn close(&mut self, kind: OverlayKind) {
        self.stack.retain(|k| *k != kind);
    }

    pub fn focus_target(&self) -> FocusTarget {
        self.top()
            .map_or(FocusTarget::Viewport, FocusTarget::Overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OverlayKind::*;

    #[test]
    fn starts_with_focus_on_the_viewport() {
        assert_eq!(
            OverlayStack::default().focus_target(),
            FocusTarget::Viewport
        );
    }

    #[test]
    fn esc_closes_only_the_top_overlay_and_focus_falls_back() {
        let mut s = OverlayStack::default();
        s.open(CopyMode);
        s.open(Search);
        assert_eq!(s.focus_target(), FocusTarget::Overlay(Search));
        assert_eq!(s.dismiss_top(), Some(Search));
        assert_eq!(s.focus_target(), FocusTarget::Overlay(CopyMode));
        assert_eq!(s.dismiss_top(), Some(CopyMode));
        assert_eq!(s.focus_target(), FocusTarget::Viewport);
        assert_eq!(s.dismiss_top(), None);
    }

    #[test]
    fn pickers_replace_each_other() {
        let mut s = OverlayStack::default();
        s.open(CommandPalette);
        s.open(ThemeSelector);
        assert_eq!(s.len(), 1);
        assert_eq!(s.top(), Some(ThemeSelector));
        assert!(!s.contains(CommandPalette));
    }

    #[test]
    fn a_picker_opens_above_a_mode_and_reveals_it_again_on_close() {
        let mut s = OverlayStack::default();
        s.open(QuickSelect);
        s.open(CommandPalette);
        assert_eq!(s.top(), Some(CommandPalette));
        s.dismiss_top();
        assert_eq!(s.top(), Some(QuickSelect));
    }

    #[test]
    fn modes_are_mutually_exclusive() {
        let mut s = OverlayStack::default();
        s.open(CopyMode);
        s.open(QuickSelect);
        assert_eq!(s.len(), 1);
        assert_eq!(s.top(), Some(QuickSelect));
    }

    #[test]
    fn toggle_closes_when_already_on_top() {
        let mut s = OverlayStack::default();
        s.toggle(CommandPalette);
        assert_eq!(s.top(), Some(CommandPalette));
        s.toggle(CommandPalette);
        assert!(s.is_empty());
    }

    #[test]
    fn reopening_moves_an_overlay_to_the_top() {
        let mut s = OverlayStack::default();
        s.open(SshReconnect);
        s.open(CopyMode);
        s.open(SshReconnect);
        assert_eq!(s.top(), Some(SshReconnect));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn close_removes_a_buried_overlay() {
        let mut s = OverlayStack::default();
        s.open(CopyMode);
        s.open(CommandPalette);
        s.close(CopyMode);
        assert_eq!(s.top(), Some(CommandPalette));
        assert!(!s.contains(CopyMode));
    }
}
