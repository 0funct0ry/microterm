//! The set of available themes: bundled plus user themes from `<config_dir>/themes`.

use std::path::Path;

use crate::bundled::BUNDLED;
use crate::theme::{parse_theme, Theme};

pub const DEFAULT_DARK: &str = "One Dark";
pub const DEFAULT_LIGHT: &str = "One Light";

#[derive(Debug, Clone)]
pub struct Registry {
    themes: Vec<Theme>,
}

impl Registry {
    /// Only the bundled themes, in bundled order (the file-name order).
    pub fn bundled() -> Self {
        let themes = BUNDLED
            .iter()
            .map(|(file, src)| {
                parse_theme(src).unwrap_or_else(|e| panic!("bundled theme {file}: {e}"))
            })
            .collect();
        Self { themes }
    }

    /// Bundled themes plus every `*.toml` in `dir`. A user theme replaces a bundled theme of
    /// the same name (case-insensitive). Returns the registry and one message per broken file.
    pub fn with_user_dir(dir: &Path) -> (Self, Vec<String>) {
        let mut reg = Self::bundled();
        let mut errors = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return (reg, errors);
        };
        let mut files: Vec<_> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        for f in files {
            let shown = f.file_name().map(|n| n.to_string_lossy().into_owned());
            let shown = shown.unwrap_or_default();
            match std::fs::read_to_string(&f) {
                Err(e) => errors.push(format!("themes/{shown}: {e}")),
                Ok(src) => match parse_theme(&src) {
                    Err(e) => errors.push(format!("themes/{shown}: {e}")),
                    Ok(t) => reg.insert(t),
                },
            }
        }
        (reg, errors)
    }

    fn insert(&mut self, t: Theme) {
        match self
            .themes
            .iter_mut()
            .find(|x| x.name.eq_ignore_ascii_case(&t.name))
        {
            Some(slot) => *slot = t,
            None => self.themes.push(t),
        }
    }

    pub fn all(&self) -> &[Theme] {
        &self.themes
    }

    pub fn len(&self) -> usize {
        self.themes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.themes.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.themes
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Look up `name`, falling back to the default theme for the appearance.
    pub fn resolve(&self, name: &str, os_dark: bool) -> &Theme {
        self.get(name).unwrap_or_else(|| {
            let d = if os_dark { DEFAULT_DARK } else { DEFAULT_LIGHT };
            self.get(d).expect("default themes are bundled")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_set_is_complete_and_valid() {
        let r = Registry::bundled();
        assert_eq!(r.len(), 53, "54 per SPEC minus the dropped Zenburn");
        let mut names: Vec<_> = r.all().iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 53, "names are unique");
        for t in r.all() {
            t.validate().unwrap_or_else(|e| panic!("{e}"));
        }
        assert!(r.get("zenburn").is_none());
        for must in [
            "One Dark",
            "One Light",
            "Tokyo Night",
            "Catppuccin Mocha",
            "Gruvbox Dark Hard",
            "Poimandres",
        ] {
            assert!(r.get(must).is_some(), "{must}");
        }
    }

    #[test]
    fn lookup_is_case_insensitive_with_fallback() {
        let r = Registry::bundled();
        assert_eq!(r.get("nord").unwrap().name, "Nord");
        assert_eq!(r.resolve("Nope", true).name, "One Dark");
        assert_eq!(r.resolve("Nope", false).name, "One Light");
    }

    #[test]
    fn appearance_matches_background_lightness() {
        for t in Registry::bundled().all() {
            let light_bg = t.background.to_oklch().l > 0.5;
            assert_eq!(!t.is_dark(), light_bg, "{}", t.name);
        }
    }

    #[test]
    fn user_themes_override_and_report_errors() {
        let dir = std::env::temp_dir().join(format!("mt_theme_user_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = include_str!("../../../themes/nord.toml")
            .replace("name = \"Nord\"", "name = \"My Nord\"");
        std::fs::write(dir.join("a.toml"), src).unwrap();
        std::fs::write(dir.join("broken.toml"), "name = 1").unwrap();
        let (r, errs) = Registry::with_user_dir(&dir);
        assert_eq!(r.len(), 54);
        assert!(r.get("My Nord").is_some());
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("broken.toml"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
