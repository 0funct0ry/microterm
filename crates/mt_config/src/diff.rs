//! Classify what changed between two configs (SPEC §8.2 table).

use std::collections::BTreeSet;

use crate::schema::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effect {
    /// Re-render only.
    Repaint,
    /// Recompute cell metrics and resize PTYs.
    Relayout,
    /// Rebuild the keymap.
    Keymap,
    /// Rebuild quick-select / hyperlink patterns.
    Patterns,
    /// Applies to panes created after the reload.
    NewPanesOnly,
    /// Needs an app restart; the banner says so.
    RestartRequired,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigDiff {
    pub effects: BTreeSet<Effect>,
    /// Top-level or dotted section names that changed, for messages.
    pub changed: Vec<&'static str>,
    /// Subset of `changed` that needs a restart.
    pub restart_keys: Vec<&'static str>,
}

impl ConfigDiff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }

    pub fn has(&self, e: Effect) -> bool {
        self.effects.contains(&e)
    }

    fn mark(&mut self, changed: bool, key: &'static str, effects: &[Effect]) {
        if !changed {
            return;
        }
        self.changed.push(key);
        for e in effects {
            self.effects.insert(*e);
        }
        if effects.contains(&Effect::RestartRequired) {
            self.restart_keys.push(key);
        }
    }
}

pub fn diff(old: &Config, new: &Config) -> ConfigDiff {
    use Effect::*;
    let mut d = ConfigDiff::default();
    d.mark(old.theme != new.theme, "theme", &[Repaint]);
    d.mark(old.cursor != new.cursor, "cursor", &[Repaint]);
    d.mark(old.text != new.text, "text", &[Repaint]);
    d.mark(old.scrollbar != new.scrollbar, "scrollbar", &[Repaint]);
    d.mark(
        old.window.opacity != new.window.opacity,
        "window.opacity",
        &[Repaint],
    );
    d.mark(
        old.window.blur != new.window.blur,
        "window.blur",
        &[Repaint],
    );
    d.mark(old.tab_bar != new.tab_bar, "tab_bar", &[Repaint, Relayout]);
    // Behaviour settings read live by the UI.
    d.mark(old.selection != new.selection, "selection", &[Repaint]);
    d.mark(old.mouse != new.mouse, "mouse", &[Repaint]);
    d.mark(old.clipboard != new.clipboard, "clipboard", &[Repaint]);
    d.mark(old.paste != new.paste, "paste", &[Repaint]);
    d.mark(old.bell != new.bell, "bell", &[Repaint]);
    d.mark(
        old.confirm_close != new.confirm_close,
        "confirm_close",
        &[Repaint],
    );
    d.mark(
        old.new_tab_cwd != new.new_tab_cwd,
        "new_tab_cwd",
        &[Repaint],
    );

    d.mark(old.font != new.font, "font", &[Relayout]);
    d.mark(
        old.window.padding != new.window.padding,
        "window.padding",
        &[Relayout],
    );

    d.mark(
        old.keys != new.keys || old.copy_mode != new.copy_mode,
        "keys",
        &[Keymap],
    );
    d.mark(
        old.quick_select != new.quick_select
            || old.hyperlink_rules != new.hyperlink_rules
            || old.hyperlinks != new.hyperlinks,
        "quick_select/hyperlinks",
        &[Patterns],
    );

    d.mark(old.shell != new.shell, "shell", &[NewPanesOnly]);
    d.mark(old.env != new.env, "env", &[NewPanesOnly]);
    d.mark(old.term != new.term, "term", &[NewPanesOnly]);
    d.mark(
        old.scrollback_lines != new.scrollback_lines,
        "scrollback_lines",
        &[NewPanesOnly],
    );
    d.mark(old.launch != new.launch, "launch", &[NewPanesOnly]);
    d.mark(
        old.ssh_domains != new.ssh_domains,
        "ssh_domains",
        &[NewPanesOnly],
    );
    d.mark(
        old.workspaces != new.workspaces,
        "workspaces",
        &[NewPanesOnly],
    );
    d.mark(
        old.startup_workspace != new.startup_workspace,
        "startup_workspace",
        &[NewPanesOnly],
    );
    d.mark(
        old.window.initial_size != new.window.initial_size
            || old.window.remember_size != new.window.remember_size,
        "window.initial_size",
        &[NewPanesOnly],
    );

    d.mark(
        old.window.decorations != new.window.decorations,
        "window.decorations",
        &[RestartRequired],
    );
    d.mark(old.gpu != new.gpu, "gpu", &[RestartRequired]);
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;

    #[test]
    fn identical_is_empty() {
        assert!(diff(&Config::default(), &Config::default()).is_empty());
    }

    #[test]
    fn classifies_per_table() {
        let old = Config::default();
        let mut n = old.clone();
        n.theme = ThemeSetting::Name("Nord".into());
        n.font.size = 15.0;
        n.keys.push(KeyEntry {
            key: "cmd+k".into(),
            action: ActionRef {
                name: "Disable".into(),
                arg: None,
            },
        });
        n.quick_select.alphabet = "abc".into();
        n.shell = "/bin/zsh".into();
        n.gpu.prefer = GpuPreference::LowPower;
        let d = diff(&old, &n);
        for e in [
            Effect::Repaint,
            Effect::Relayout,
            Effect::Keymap,
            Effect::Patterns,
            Effect::NewPanesOnly,
            Effect::RestartRequired,
        ] {
            assert!(d.has(e), "{e:?}");
        }
        assert_eq!(d.restart_keys, vec!["gpu"]);
    }

    #[test]
    fn padding_is_relayout_only() {
        let old = Config::default();
        let mut n = old.clone();
        n.window.padding.x = 20.0;
        let d = diff(&old, &n);
        assert_eq!(d.effects, BTreeSet::from([Effect::Relayout]));
    }
}
