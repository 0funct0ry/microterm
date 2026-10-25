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
    /// Context menus: resolved at the next open; an open menu closes (SPEC §10.9.2).
    Menus,
    /// `snippets.enabled` / `snippets.templates` changed: register or tear down the panel,
    /// actions, palette provider, per-snippet keys and the `snippets.toml` watch (SPEC §10.8.1).
    Snippets,
    /// `recording.enabled` changed: register or tear down recording mode; turning it off
    /// stops and saves every running recording (SPEC §10.10.1).
    Recording,
    /// `triggers.enabled` or a second gate changed: install or remove the line taps, register
    /// or tear down the panel, markers and actions; turning it off clears every decoration and
    /// stops trigger recordings (SPEC §10.11.1).
    Triggers,
    /// `private_osc.enabled`, a feature gate or the rate changed: push the gate into every
    /// pane; what it no longer allows is cleared and not restored (SPEC §6.4.2).
    PrivateOsc,
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
    d.mark(old.panes != new.panes, "panes", &[Repaint]);
    d.mark(old.clipboard != new.clipboard, "clipboard", &[Repaint]);
    d.mark(old.paste != new.paste, "paste", &[Repaint]);
    d.mark(old.menu_bar != new.menu_bar, "menu_bar", &[Repaint]);
    d.mark(
        old.snippets.enabled != new.snippets.enabled
            || old.snippets.templates != new.snippets.templates,
        "snippets.enabled/templates",
        &[Snippets, Keymap, Repaint, Relayout],
    );
    d.mark(
        old.snippets.panel_width != new.snippets.panel_width,
        "snippets.panel_width",
        &[Relayout],
    );
    d.mark(
        old.snippets.preview_lines != new.snippets.preview_lines
            || old.snippets.confirm_multiline != new.snippets.confirm_multiline
            || old.snippets.focus_terminal_after_insert != new.snippets.focus_terminal_after_insert
            || old.snippets.panel_open_on_start != new.snippets.panel_open_on_start,
        "snippets",
        &[Repaint],
    );
    d.mark(
        old.recording.enabled != new.recording.enabled,
        "recording.enabled",
        &[Recording, Keymap, Repaint, Relayout],
    );
    d.mark(
        old.recording.panel_width != new.recording.panel_width,
        "recording.panel_width",
        &[Relayout],
    );
    d.mark(
        old.recording.capture_input != new.recording.capture_input
            || old.recording.redact_no_echo != new.recording.redact_no_echo
            || old.recording.max_duration_minutes != new.recording.max_duration_minutes
            || old.recording.max_size_mb != new.recording.max_size_mb
            || old.recording.panel_open_on_start != new.recording.panel_open_on_start
            || old.recording.confirm_delete != new.recording.confirm_delete
            || old.recording.export != new.recording.export,
        "recording",
        &[Repaint],
    );
    d.mark(
        old.triggers.enabled != new.triggers.enabled
            || old.triggers.allow_clipboard != new.triggers.allow_clipboard
            || old.triggers.allow_capture != new.triggers.allow_capture
            || old.triggers.scan_alt_screen != new.triggers.scan_alt_screen
            || old.triggers.scan_partial_after_ms != new.triggers.scan_partial_after_ms
            || old.triggers.max_rules != new.triggers.max_rules
            || old.triggers.max_line_chars != new.triggers.max_line_chars,
        "triggers.enabled",
        &[Triggers, Keymap, Repaint, Relayout],
    );
    d.mark(
        old.triggers.panel_width != new.triggers.panel_width,
        "triggers.panel_width",
        &[Relayout],
    );
    d.mark(
        old.triggers.highlights_max != new.triggers.highlights_max
            || old.triggers.markers_max != new.triggers.markers_max
            || old.triggers.findings_max != new.triggers.findings_max
            || old.triggers.capture_dir != new.triggers.capture_dir
            || old.triggers.capture_max_kb != new.triggers.capture_max_kb
            || old.triggers.capture_max_files != new.triggers.capture_max_files
            || old.triggers.capture_max_total_mb != new.triggers.capture_max_total_mb
            || old.triggers.capture_redact != new.triggers.capture_redact
            || old.triggers.panel_open_on_start != new.triggers.panel_open_on_start
            || old.triggers.confirm_delete != new.triggers.confirm_delete,
        "triggers",
        &[Repaint],
    );
    d.mark(
        old.private_osc.enabled != new.private_osc.enabled
            || old.private_osc.rate_per_second != new.private_osc.rate_per_second
            || old.private_osc.features != new.private_osc.features,
        "private_osc.enabled",
        &[PrivateOsc, Repaint],
    );
    d.mark(
        old.private_osc.fold_commands != new.private_osc.fold_commands,
        "private_osc",
        &[Repaint],
    );
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
        old.context_menu != new.context_menu
            || old.menu_groups != new.menu_groups
            || old.menu_security != new.menu_security,
        "context_menu",
        &[Menus],
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
        old.serial_ports != new.serial_ports,
        "serial_ports",
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
    // Read when a request arrives; nothing to rebuild.
    d.mark(old.ctl != new.ctl, "ctl", &[Repaint]);
    // Pushed to live panes; affects sequences received afterwards.
    d.mark(old.terminal != new.terminal, "terminal", &[Repaint]);
    d.mark(
        old.notifications != new.notifications,
        "notifications",
        &[Repaint],
    );
    // Read when a `microterm sh` session starts; nothing to rebuild in the GUI.
    d.mark(old.sh != new.sh, "sh", &[Repaint]);
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;

    #[test]
    fn every_snippets_key_is_classified() {
        let old = Config::default();
        type Edit = (&'static str, fn(&mut Snippets), Effect);
        let edits: [Edit; 7] = [
            ("enabled", |s| s.enabled = true, Effect::Snippets),
            ("templates", |s| s.templates = true, Effect::Snippets),
            ("panel_width", |s| s.panel_width = 400, Effect::Relayout),
            (
                "panel_open_on_start",
                |s| s.panel_open_on_start = true,
                Effect::Repaint,
            ),
            (
                "confirm_multiline",
                |s| s.confirm_multiline = true,
                Effect::Repaint,
            ),
            (
                "focus_terminal_after_insert",
                |s| s.focus_terminal_after_insert = false,
                Effect::Repaint,
            ),
            ("preview_lines", |s| s.preview_lines = 4, Effect::Repaint),
        ];
        for (key, edit, effect) in edits {
            let mut n = old.clone();
            edit(&mut n.snippets);
            let d = diff(&old, &n);
            assert!(!d.is_empty(), "{key} is not classified");
            assert!(d.has(effect), "{key}: {:?}", d.effects);
        }
    }

    #[test]
    fn every_recording_key_is_classified() {
        let old = Config::default();
        type Edit = (&'static str, fn(&mut Recording), Effect);
        let edits: [Edit; 10] = [
            ("enabled", |r| r.enabled = true, Effect::Recording),
            ("capture_input", |r| r.capture_input = true, Effect::Repaint),
            (
                "redact_no_echo",
                |r| r.redact_no_echo = false,
                Effect::Repaint,
            ),
            (
                "max_duration_minutes",
                |r| r.max_duration_minutes = 5,
                Effect::Repaint,
            ),
            ("max_size_mb", |r| r.max_size_mb = 5, Effect::Repaint),
            ("panel_width", |r| r.panel_width = 400, Effect::Relayout),
            (
                "panel_open_on_start",
                |r| r.panel_open_on_start = true,
                Effect::Repaint,
            ),
            (
                "confirm_delete",
                |r| r.confirm_delete = false,
                Effect::Repaint,
            ),
            ("export.gif_fps", |r| r.export.gif_fps = 5, Effect::Repaint),
            (
                "export.default_format",
                |r| r.export.default_format = "vhs".into(),
                Effect::Repaint,
            ),
        ];
        for (key, edit, effect) in edits {
            let mut n = old.clone();
            edit(&mut n.recording);
            let d = diff(&old, &n);
            assert!(!d.is_empty(), "{key} is not classified");
            assert!(d.has(effect), "{key}: {:?}", d.effects);
        }
    }

    #[test]
    fn every_triggers_key_is_classified() {
        let old = Config::default();
        type Edit = (&'static str, fn(&mut Triggers), Effect);
        let edits: [Edit; 18] = [
            ("enabled", |t| t.enabled = true, Effect::Triggers),
            (
                "allow_clipboard",
                |t| t.allow_clipboard = true,
                Effect::Triggers,
            ),
            (
                "allow_capture",
                |t| t.allow_capture = true,
                Effect::Triggers,
            ),
            (
                "scan_partial_after_ms",
                |t| t.scan_partial_after_ms = 0,
                Effect::Triggers,
            ),
            (
                "scan_alt_screen",
                |t| t.scan_alt_screen = true,
                Effect::Triggers,
            ),
            ("max_rules", |t| t.max_rules = 8, Effect::Triggers),
            (
                "max_line_chars",
                |t| t.max_line_chars = 100,
                Effect::Triggers,
            ),
            ("highlights_max", |t| t.highlights_max = 5, Effect::Repaint),
            ("markers_max", |t| t.markers_max = 5, Effect::Repaint),
            ("findings_max", |t| t.findings_max = 5, Effect::Repaint),
            (
                "capture_dir",
                |t| t.capture_dir = "/x".into(),
                Effect::Repaint,
            ),
            ("capture_max_kb", |t| t.capture_max_kb = 5, Effect::Repaint),
            (
                "capture_max_files",
                |t| t.capture_max_files = 5,
                Effect::Repaint,
            ),
            (
                "capture_max_total_mb",
                |t| t.capture_max_total_mb = 5,
                Effect::Repaint,
            ),
            (
                "capture_redact",
                |t| t.capture_redact = false,
                Effect::Repaint,
            ),
            ("panel_width", |t| t.panel_width = 400, Effect::Relayout),
            (
                "panel_open_on_start",
                |t| t.panel_open_on_start = true,
                Effect::Repaint,
            ),
            (
                "confirm_delete",
                |t| t.confirm_delete = false,
                Effect::Repaint,
            ),
        ];
        for (key, edit, effect) in edits {
            let mut n = old.clone();
            edit(&mut n.triggers);
            let d = diff(&old, &n);
            assert!(!d.is_empty(), "{key} is not classified");
            assert!(d.has(effect), "{key}: {:?}", d.effects);
        }
    }

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
