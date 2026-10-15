//! Runtime configuration: the active config and theme registry as a GPUI global, and the
//! hot-reload pipeline (SPEC §8.2): parse → validate → diff → atomic swap → apply.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Global, SharedString};
use mt_config::{
    diagnostics::Severity, keys::KeyBindingSpec, Config, ConfigDiff, Diagnostic, Effect,
};
use mt_theme::Registry;

use crate::{
    actions::{self, ActionDef},
    app::AppWindow,
    components::ToastKind,
    platform::Platform,
    theme::Theme,
};

/// A message shown in the config banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub kind: ToastKind,
    pub text: String,
}

/// The active configuration. Replaced wholesale on reload, so readers never see a mix of old
/// and new values.
pub struct Settings {
    pub config: Arc<Config>,
    pub registry: Arc<Registry>,
    pub path: PathBuf,
    /// Key chips by action name, reflecting the user's bindings.
    chips: HashMap<&'static str, String>,
    /// Bumped when the terminal font changes, so shaped-line caches are discarded.
    pub font_generation: u64,
    /// The banner every window shows (startup problems, failed reloads, warnings).
    pub banner: Option<Banner>,
}

impl Global for Settings {}

impl Settings {
    /// Defaults, with no config file (used by tests and as the fallback).
    pub fn defaults() -> Self {
        let mut me = Self {
            config: Arc::new(Config::default()),
            registry: Arc::new(Registry::bundled()),
            path: PathBuf::from(mt_config::load::FILE_NAME),
            chips: HashMap::new(),
            font_generation: 0,
            banner: None,
        };
        me.rebuild_chips(Platform::current());
        me
    }

    /// Reads the config at `path`. A broken file falls back to defaults and sets the banner.
    pub fn load(path: PathBuf) -> Self {
        let mut me = Self {
            path,
            ..Self::defaults()
        };
        let name = mt_config::load::display_name(&me.path);
        match mt_config::load::load(&me.path) {
            Ok(loaded) => {
                let (registry, theme_errors) =
                    Registry::with_user_dir(&mt_config::load::themes_dir(&me.path));
                me.registry = Arc::new(registry);
                match theme_problem(&loaded.config, &me.registry) {
                    Some(msg) => {
                        me.banner = Some(Banner {
                            kind: ToastKind::Error,
                            text: format!("{name} — {msg}. Using the default config."),
                        });
                    }
                    None => {
                        me.config = Arc::new(loaded.config);
                        let key_notes = me.rebuild_chips(Platform::current());
                        me.banner = banner_for(&name, &loaded.warnings, &theme_errors, &key_notes);
                    }
                }
            }
            Err(diags) => {
                me.banner = Some(Banner {
                    kind: ToastKind::Error,
                    text: format!("{} Using the default config.", render_first(&name, &diags)),
                });
            }
        }
        me
    }

    /// Recomputes the key chips; called after every config change.
    pub fn rebuild_chips(&mut self, platform: Platform) -> Vec<String> {
        let specs = mt_config::keys::bindings(&self.config).unwrap_or_default();
        let (chips, warnings) = actions::keymap_chips(platform, &specs);
        self.chips = chips;
        warnings
    }

    /// Chip text for an action, honouring user bindings.
    pub fn chip(&self, def: &ActionDef) -> Option<String> {
        self.chips.get(def.name).cloned()
    }

    /// The theme for the current OS appearance.
    pub fn theme(&self, os_dark: bool) -> Theme {
        let name = self.config.theme.name_for(os_dark);
        Theme::from_mt(self.registry.resolve(name, os_dark))
    }

    pub fn term_font(&self) -> SharedString {
        SharedString::from(self.config.font.family.clone())
    }
}

fn os_is_dark(cx: &App) -> bool {
    !matches!(
        cx.window_appearance(),
        gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
    )
}

fn theme_problem(config: &Config, registry: &Registry) -> Option<String> {
    config
        .theme
        .names()
        .into_iter()
        .find(|n| registry.get(n).is_none())
        .map(|n| format!("unknown theme \"{n}\" (see `microterm themes list`)"))
}

fn render_first(file: &str, diags: &[Diagnostic]) -> String {
    let mut out = diags.first().map_or_else(String::new, |d| d.render(file));
    if diags.len() > 1 {
        out.push_str(&format!(" (+{} more)", diags.len() - 1));
    }
    out
}

/// A warning banner for non-fatal problems, or `None` when there are none.
fn banner_for(
    file: &str,
    warnings: &[Diagnostic],
    theme_errors: &[String],
    other: &[String],
) -> Option<Banner> {
    let mut lines: Vec<String> = warnings
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| d.render(file))
        .collect();
    lines.extend(theme_errors.iter().cloned());
    lines.extend(other.iter().cloned());
    let first = lines.first()?.clone();
    let more = lines.len() - 1;
    Some(Banner {
        kind: ToastKind::Warning,
        text: if more == 0 {
            first
        } else {
            format!("{first} (+{more} more)")
        },
    })
}

/// What a reload did, for the caller's feedback.
#[derive(Debug, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// New config applied; the diff was empty.
    Unchanged,
    Applied(Vec<&'static str>),
    /// The file is invalid; the previous config stays active.
    Rejected,
}

/// Re-reads the config file and applies it to every window, or keeps the previous config and
/// shows the banner. Never call this from inside an `AppWindow` update (defer it).
pub fn reload(cx: &mut App) -> ReloadOutcome {
    let (path, old_cfg, old_registry, platform) = {
        let s = cx.global::<Settings>();
        (
            s.path.clone(),
            s.config.clone(),
            s.registry.clone(),
            Platform::current(),
        )
    };
    let name = mt_config::load::display_name(&path);

    let loaded = match mt_config::load::load(&path) {
        Ok(l) => l,
        Err(diags) => {
            let text = format!(
                "{} Using the last valid config.",
                render_first(&name, &diags)
            );
            set_banner(
                cx,
                Some(Banner {
                    kind: ToastKind::Error,
                    text,
                }),
            );
            return ReloadOutcome::Rejected;
        }
    };
    let (registry, theme_errors) = Registry::with_user_dir(&mt_config::load::themes_dir(&path));
    if let Some(msg) = theme_problem(&loaded.config, &registry) {
        set_banner(
            cx,
            Some(Banner {
                kind: ToastKind::Error,
                text: format!("{name} — {msg}. Using the last valid config."),
            }),
        );
        return ReloadOutcome::Rejected;
    }

    let config = Arc::new(loaded.config);
    let diff = mt_config::diff(&old_cfg, &config);
    let registry_changed = registry.all() != old_registry.all();

    // Atomic swap: one assignment replaces the config, the registry and the key chips.
    let key_warnings;
    {
        let mut next = Settings {
            config: config.clone(),
            registry: Arc::new(registry),
            path,
            chips: HashMap::new(),
            font_generation: cx.global::<Settings>().font_generation,
            banner: None,
        };
        if diff.has(Effect::Relayout) && old_cfg.font != config.font {
            next.font_generation += 1;
        }
        key_warnings = next.rebuild_chips(platform);
        let mut restart: Vec<String> = Vec::new();
        if !diff.restart_keys.is_empty() {
            restart.push(format!(
                "Restart microterm to apply: {}",
                diff.restart_keys.join(", ")
            ));
        }
        let mut notes = key_warnings.clone();
        notes.extend(restart);
        next.banner = banner_for(&name, &loaded.warnings, &theme_errors, &notes);
        cx.set_global(next);
    }

    if diff.has(Effect::Keymap) || !key_warnings.is_empty() {
        install_keymap(cx);
    }
    if diff.has(Effect::Repaint) || registry_changed {
        apply_theme(cx);
    }
    notify_windows(cx, &diff);
    if diff.is_empty() && !registry_changed {
        ReloadOutcome::Unchanged
    } else {
        ReloadOutcome::Applied(diff.changed)
    }
}

fn set_banner(cx: &mut App, banner: Option<Banner>) {
    cx.global_mut::<Settings>().banner = banner.clone();
    for handle in cx.windows() {
        if let Some(h) = handle.downcast::<AppWindow>() {
            let b = banner.clone();
            let _ = h.update(cx, |this, _, cx| this.set_config_banner(b, cx));
        }
    }
}

/// Rebinds every key from the defaults plus the user's `[[keys]]`.
pub fn install_keymap(cx: &mut App) {
    let specs: Vec<KeyBindingSpec> =
        mt_config::keys::bindings(&cx.global::<Settings>().config).unwrap_or_default();
    let (bindings, _) = actions::build_keymap(Platform::current(), cx, &specs);
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

/// Resolves the theme for the current OS appearance, installs it and updates every terminal's
/// palette so OSC colour queries answer with the new colours.
pub fn apply_theme(cx: &mut App) {
    let theme = cx.global::<Settings>().theme(os_is_dark(cx));
    cx.set_global(theme);
    for handle in cx.windows() {
        if let Some(h) = handle.downcast::<AppWindow>() {
            let _ = h.update(cx, |this, _, cx| this.sync_palette(cx));
        }
    }
    cx.refresh_windows();
}

fn notify_windows(cx: &mut App, diff: &ConfigDiff) {
    let diff = diff.clone();
    let banner = cx.global::<Settings>().banner.clone();
    for handle in cx.windows() {
        if let Some(h) = handle.downcast::<AppWindow>() {
            let (d, b) = (diff.clone(), banner.clone());
            let _ = h.update(cx, |this, window, cx| {
                this.set_config_banner(b, cx);
                this.apply_config(&d, window, cx);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mt_settings_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("microterm.toml");
        std::fs::write(&f, body).unwrap();
        f
    }

    #[test]
    fn load_valid_config() {
        let f = temp_file("ok", "[font]\nsize = 15\n");
        let s = Settings::load(f);
        assert_eq!(s.config.font.size, 15.0);
        assert!(s.banner.is_none());
    }

    #[test]
    fn unknown_keys_warn_but_apply() {
        let f = temp_file("warn", "fnt = 1\n[font]\nsize = 15\n");
        let s = Settings::load(f);
        assert_eq!(s.config.font.size, 15.0);
        let b = s.banner.unwrap();
        assert_eq!(b.kind, ToastKind::Warning);
        assert!(b.text.contains("microterm.toml:1:1"), "{}", b.text);
        assert!(b.text.contains("did you mean \"font\""));
    }

    #[test]
    fn broken_config_falls_back_with_banner() {
        let f = temp_file("bad", "[font]\nsize = \"big\"\n");
        let s = Settings::load(f);
        assert_eq!(*s.config, Config::default());
        let b = s.banner.unwrap();
        assert_eq!(b.kind, ToastKind::Error);
        assert!(b.text.contains("microterm.toml:2:8"), "{}", b.text);
        assert!(b.text.contains("default config"));
    }

    #[test]
    fn unknown_theme_is_an_error() {
        let f = temp_file("theme", "theme = \"Nope\"\n");
        let s = Settings::load(f);
        assert_eq!(*s.config, Config::default());
        assert!(s.banner.unwrap().text.contains("unknown theme \"Nope\""));
    }

    #[test]
    fn theme_pair_follows_appearance() {
        let f = temp_file("pair", "theme = { dark = \"Nord\", light = \"Dayfox\" }\n");
        let s = Settings::load(f);
        assert_eq!(s.theme(true).name, "Nord");
        assert_eq!(s.theme(false).name, "Dayfox");
    }
}
