//! Values flowing through a template, with the secret taint (SPEC §10.8.4 `secret`).

use std::fmt;

/// The mask shown instead of a secret in previews, logs and debug output.
pub const MASK: &str = "••••";

/// A tainted string. `Debug` and `Display` never print the value; the only way to read it is
/// [`Secret::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(MASK)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(MASK)
    }
}

/// A template value; secrets stay secret through filters.
#[derive(Clone, PartialEq, Eq)]
pub enum Val {
    Plain(String),
    Secret(Secret),
}

impl Val {
    pub fn plain(s: impl Into<String>) -> Self {
        Val::Plain(s.into())
    }
    pub fn secret(s: impl Into<String>) -> Self {
        Val::Secret(Secret::new(s))
    }
    /// The real text. Callers must not log it when [`Val::is_secret`].
    pub fn expose(&self) -> &str {
        match self {
            Val::Plain(s) => s,
            Val::Secret(s) => s.expose(),
        }
    }
    pub fn is_secret(&self) -> bool {
        matches!(self, Val::Secret(_))
    }
    /// A value derived from this one keeps its taint.
    pub fn derive(&self, text: String) -> Val {
        if self.is_secret() {
            Val::secret(text)
        } else {
            Val::Plain(text)
        }
    }
}

impl fmt::Debug for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Plain(s) => write!(f, "{s:?}"),
            Val::Secret(s) => fmt::Debug::fmt(s, f),
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Plain(s) => f.write_str(s),
            Val::Secret(s) => fmt::Display::fmt(s, f),
        }
    }
}
