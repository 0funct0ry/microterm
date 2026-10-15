//! `microterm ls-fonts [--text STR]` (SPEC §7.2): which font renders each character and why.
//!
//! Coverage is read straight from the fonts' `cmap` tables (bundled faces plus the system's), so
//! the command works without a window or GPU. The platform last resort is the first installed
//! font that has the glyph, an approximation of the OS's own cascade.

use std::path::PathBuf;

use crate::{assets, typography};

const SAMPLE: &str = "abc λ→ ==> \u{f015} 🦀 漢字";

/// Installed fonts: the bundled faces and the system's.
pub struct FontIndex {
    db: fontdb::Database,
}

impl FontIndex {
    pub fn load() -> Self {
        let mut db = fontdb::Database::new();
        for bytes in assets::bundled_fonts() {
            db.load_font_data(bytes.to_vec());
        }
        db.load_system_fonts();
        Self { db }
    }

    fn face_has(&self, id: fontdb::ID, ch: char) -> bool {
        self.db
            .with_face_data(id, |data, index| {
                skrifa::FontRef::from_index(data, index)
                    .is_ok_and(|f| skrifa::MetadataProvider::charmap(&f).map(ch).is_some())
            })
            .unwrap_or(false)
    }

    fn faces_of<'a>(&'a self, family: &'a str) -> impl Iterator<Item = fontdb::ID> + 'a {
        self.db
            .faces()
            .filter(move |f| {
                f.families
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(family))
            })
            .map(|f| f.id)
    }

    pub fn is_installed(&self, family: &str) -> bool {
        self.faces_of(family).next().is_some()
    }

    /// Whether any face of `family` has a glyph for `ch`.
    pub fn covers(&self, family: &str, ch: char) -> bool {
        self.faces_of(family).any(|id| self.face_has(id, ch))
    }

    /// The first installed family (outside `skip`) that has a glyph for `ch`.
    pub fn any_covering(&self, ch: char, skip: &[&str]) -> Option<String> {
        self.db
            .faces()
            .filter(|f| {
                !f.families
                    .iter()
                    .any(|(n, _)| skip.iter().any(|s| s.eq_ignore_ascii_case(n)))
            })
            .find(|f| self.face_has(f.id, ch))
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
    }
}

/// One output line per character: `U+XXXX  c  font  (reason)`.
pub fn describe(index: &FontIndex, f: &mt_config::Font, text: &str) -> Vec<String> {
    let chain = typography::fallback_chain(&f.family, &f.fallback);
    let names: Vec<&str> = chain.iter().filter_map(|s| s.font_name()).collect();
    let mut out = vec!["Fallback chain:".to_string()];
    for step in &chain {
        let note = match step.font_name() {
            Some(n) if !index.is_installed(n) => "  [not installed]",
            _ => "",
        };
        out.push(format!(
            "  {}  ({}){note}",
            step.font_name().unwrap_or("(system fallback)"),
            step.reason()
        ));
    }
    out.push(String::new());
    for ch in text.chars() {
        let shown = if ch.is_control() || ch == ' ' {
            ch.escape_default().to_string()
        } else {
            ch.to_string()
        };
        let step = typography::resolve_char(&chain, ch, |n, c| index.covers(n, c));
        let (font, reason) = match step.font_name() {
            Some(n) => (n.to_string(), step.reason()),
            None => match index.any_covering(ch, &names) {
                Some(n) => (n, step.reason()),
                None => (
                    "(none installed)".to_string(),
                    "no font has this glyph".into(),
                ),
            },
        };
        out.push(format!(
            "U+{:04X}  {shown:<4} {font}  ({reason})",
            ch as u32
        ));
    }
    out
}

/// Prints the fallback chain and, per character, the font chosen. Returns the exit code.
pub fn run(config: Option<PathBuf>, text: Option<String>) -> i32 {
    let path = mt_config::load::config_path(config.as_deref());
    let cfg = crate::settings::Settings::load(path).config;
    let text = text.unwrap_or_else(|| SAMPLE.to_string());
    let index = FontIndex::load();
    for line in describe(&index, &cfg.font, &text) {
        println!("{line}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_resolve_in_chain_order() {
        // Bundled faces only: no system fonts, so the test is machine independent.
        let mut db = fontdb::Database::new();
        for b in assets::bundled_fonts() {
            db.load_font_data(b.to_vec());
        }
        let index = FontIndex { db };
        assert!(index.covers("JetBrains Mono", 'a'));
        assert!(!index.covers("JetBrains Mono", '\u{f015}'));
        assert!(index.covers("Symbols Nerd Font Mono", '\u{f015}'));
        let lines = describe(&index, &mt_config::Font::default(), "a\u{f015}");
        assert!(lines
            .iter()
            .any(|l| l.contains("U+0061") && l.contains("JetBrains Mono  (font.family)")));
        assert!(lines
            .iter()
            .any(|l| l.contains("U+F015")
                && l.contains("Symbols Nerd Font Mono  (font.fallback[0])")));
    }
}
