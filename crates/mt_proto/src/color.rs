//! Tab colors (SPEC §4.4): semantic names plus custom RGB, with a pure precedence function.

use std::fmt;

/// The fixed set of named tab colors. Names are lowercase, stable and never renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedColor {
    Blue,
    Purple,
    Pink,
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Gray,
}

impl NamedColor {
    pub const ALL: [NamedColor; 9] = [
        NamedColor::Blue,
        NamedColor::Purple,
        NamedColor::Pink,
        NamedColor::Red,
        NamedColor::Orange,
        NamedColor::Yellow,
        NamedColor::Green,
        NamedColor::Teal,
        NamedColor::Gray,
    ];

    pub fn name(self) -> &'static str {
        match self {
            NamedColor::Blue => "blue",
            NamedColor::Purple => "purple",
            NamedColor::Pink => "pink",
            NamedColor::Red => "red",
            NamedColor::Orange => "orange",
            NamedColor::Yellow => "yellow",
            NamedColor::Green => "green",
            NamedColor::Teal => "teal",
            NamedColor::Gray => "gray",
        }
    }

    pub fn from_name(name: &str) -> Option<NamedColor> {
        NamedColor::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// A tab color: a theme-resolved name or a custom RGB value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabColor {
    Named(NamedColor),
    Rgb(u8, u8, u8),
}

/// A color string that is neither a known name nor `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseColorError(pub String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<_> = NamedColor::ALL.iter().map(|c| c.name()).collect();
        write!(
            f,
            "unknown color '{}': use one of {} or '#rrggbb'",
            self.0,
            names.join(", ")
        )
    }
}

impl std::error::Error for ParseColorError {}

impl TabColor {
    /// Parses a name (`red`) or a hex value (`#f59e42`).
    pub fn parse(s: &str) -> Result<TabColor, ParseColorError> {
        let err = || ParseColorError(s.to_string());
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(err());
            }
            let v = u32::from_str_radix(hex, 16).map_err(|_| err())?;
            return Ok(TabColor::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        NamedColor::from_name(s)
            .map(TabColor::Named)
            .ok_or_else(err)
    }

    /// The name or lowercase `#rrggbb`; round-trips through [`TabColor::parse`].
    pub fn to_string_repr(self) -> String {
        match self {
            TabColor::Named(n) => n.name().to_string(),
            TabColor::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

impl fmt::Display for TabColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_repr())
    }
}

impl TabColor {
    /// Like [`TabColor::parse`], but a name may also be an entry of `[tab_bar.palette]`
    /// (`name = "#rrggbb"`); built-in names keep their theme-resolved meaning.
    pub fn parse_with_palette(
        s: &str,
        palette: &std::collections::BTreeMap<String, String>,
    ) -> Result<TabColor, ParseColorError> {
        match TabColor::parse(s) {
            Ok(c) => Ok(c),
            Err(e) => match palette.get(s) {
                Some(hex) => TabColor::parse(hex).map_err(|_| e),
                None => Err(e),
            },
        }
    }
}

/// Where the resolved color came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorSource {
    Manual,
    /// Set by a trigger rule (SPEC §10.11.4); transient.
    Trigger,
    Declared,
    Rule,
    None,
}

impl ColorSource {
    pub fn name(self) -> &'static str {
        match self {
            ColorSource::Manual => "manual",
            ColorSource::Trigger => "trigger",
            ColorSource::Declared => "declared",
            ColorSource::Rule => "rule",
            ColorSource::None => "none",
        }
    }
}

/// Resolution precedence, highest first: manual > trigger > declared > first matching rule >
/// none (SPEC §4, §10.11.4).
pub fn resolve(
    manual: Option<TabColor>,
    trigger: Option<TabColor>,
    declared: Option<TabColor>,
    rule: Option<TabColor>,
) -> (Option<TabColor>, ColorSource) {
    if let Some(c) = manual {
        (Some(c), ColorSource::Manual)
    } else if let Some(c) = trigger {
        (Some(c), ColorSource::Trigger)
    } else if let Some(c) = declared {
        (Some(c), ColorSource::Declared)
    } else if let Some(c) = rule {
        (Some(c), ColorSource::Rule)
    } else {
        (None, ColorSource::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: TabColor = TabColor::Named(NamedColor::Red);
    const BLUE: TabColor = TabColor::Named(NamedColor::Blue);
    const TEAL: TabColor = TabColor::Named(NamedColor::Teal);

    #[test]
    fn precedence_table() {
        // manual, trigger, declared, rule -> color, source
        let cases = [
            (
                [Some(RED), Some(TEAL), Some(BLUE), Some(TEAL)],
                Some(RED),
                ColorSource::Manual,
            ),
            (
                [None, Some(TEAL), Some(BLUE), Some(TEAL)],
                Some(TEAL),
                ColorSource::Trigger,
            ),
            (
                [None, None, Some(BLUE), Some(TEAL)],
                Some(BLUE),
                ColorSource::Declared,
            ),
            (
                [None, None, None, Some(TEAL)],
                Some(TEAL),
                ColorSource::Rule,
            ),
            ([None, None, None, None], None, ColorSource::None),
            (
                [Some(RED), None, None, Some(TEAL)],
                Some(RED),
                ColorSource::Manual,
            ),
            (
                [None, Some(RED), None, Some(TEAL)],
                Some(RED),
                ColorSource::Trigger,
            ),
            (
                [None, Some(RED), Some(BLUE), None],
                Some(RED),
                ColorSource::Trigger,
            ),
        ];
        for ([m, t, d, r], color, source) in cases {
            assert_eq!(
                resolve(m, t, d, r),
                (color, source),
                "{m:?} {t:?} {d:?} {r:?}"
            );
        }
    }

    #[test]
    fn parse_names_and_hex() {
        assert_eq!(
            TabColor::parse("pink").unwrap(),
            TabColor::Named(NamedColor::Pink)
        );
        assert_eq!(
            TabColor::parse("#F59e42").unwrap(),
            TabColor::Rgb(0xf5, 0x9e, 0x42)
        );
        assert_eq!(TabColor::Rgb(0xf5, 0x9e, 0x42).to_string(), "#f59e42");
        for bad in ["", "#fff", "#gggggg", "Red", "magenta", "#1234567"] {
            assert!(TabColor::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn palette_extras_resolve_to_rgb_but_builtin_names_stay_named() {
        let mut p = std::collections::BTreeMap::new();
        p.insert("brand".to_string(), "#112233".to_string());
        p.insert("red".to_string(), "#aa0000".to_string());
        assert_eq!(
            TabColor::parse_with_palette("brand", &p).unwrap(),
            TabColor::Rgb(0x11, 0x22, 0x33)
        );
        // Overriding a built-in changes what the theme resolves, not the name.
        assert_eq!(
            TabColor::parse_with_palette("red", &p).unwrap(),
            TabColor::Named(NamedColor::Red)
        );
        assert!(TabColor::parse_with_palette("nope", &p).is_err());
    }

    #[test]
    fn names_round_trip_and_error_lists_valid_names() {
        for c in NamedColor::ALL {
            assert_eq!(NamedColor::from_name(c.name()), Some(c));
        }
        let msg = TabColor::parse("magenta").unwrap_err().to_string();
        assert!(msg.contains("blue") && msg.contains("gray") && msg.contains("magenta"));
    }
}
