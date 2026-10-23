//! Typed option specs shared by the dialog and `ctl recording export --opt k=v`.

use std::collections::BTreeMap;

use crate::ExportError;

#[derive(Debug, Clone, PartialEq)]
pub enum OptValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl std::fmt::Display for OptValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptValue::Bool(b) => write!(f, "{b}"),
            OptValue::Int(i) => write!(f, "{i}"),
            OptValue::Float(x) => write!(f, "{x}"),
            OptValue::Text(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OptKind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Enum(&'static [&'static str]),
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OptionSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: OptKind,
    pub default: OptValue,
}

impl OptionSpec {
    /// Parses and range-checks one `key=value` string.
    pub fn parse(&self, raw: &str) -> Result<OptValue, ExportError> {
        let bad = |why: String| ExportError::BadOption(format!("option {}: {why}", self.key));
        match &self.kind {
            OptKind::Bool => match raw {
                "true" | "on" | "yes" | "1" => Ok(OptValue::Bool(true)),
                "false" | "off" | "no" | "0" => Ok(OptValue::Bool(false)),
                _ => Err(bad(format!("\"{raw}\" is not true or false"))),
            },
            OptKind::Int { min, max } => {
                let v: i64 = raw
                    .parse()
                    .map_err(|_| bad(format!("\"{raw}\" is not an integer")))?;
                if (*min..=*max).contains(&v) {
                    Ok(OptValue::Int(v))
                } else {
                    Err(bad(format!("{v} is outside {min}..{max}")))
                }
            }
            OptKind::Float { min, max } => {
                let v: f64 = raw
                    .parse()
                    .map_err(|_| bad(format!("\"{raw}\" is not a number")))?;
                if v.is_finite() && (*min..=*max).contains(&v) {
                    Ok(OptValue::Float(v))
                } else {
                    Err(bad(format!("{raw} is outside {min}..{max}")))
                }
            }
            OptKind::Enum(values) => values
                .contains(&raw)
                .then(|| OptValue::Text(raw.to_string()))
                .ok_or_else(|| bad(format!("\"{raw}\" is not one of {}", values.join(", ")))),
            OptKind::Text => Ok(OptValue::Text(raw.to_string())),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options(BTreeMap<String, OptValue>);

impl Options {
    pub fn defaults(specs: &[OptionSpec]) -> Self {
        Self(
            specs
                .iter()
                .map(|s| (s.key.to_string(), s.default.clone()))
                .collect(),
        )
    }

    pub fn set(&mut self, key: &str, v: OptValue) {
        self.0.insert(key.to_string(), v);
    }

    /// Applies `key=value` overrides after validating them against `specs`.
    pub fn apply(
        &mut self,
        specs: &[OptionSpec],
        kv: &[(String, String)],
    ) -> Result<(), ExportError> {
        for (k, v) in kv {
            let spec = specs.iter().find(|s| s.key == k).ok_or_else(|| {
                let keys: Vec<_> = specs.iter().map(|s| s.key).collect();
                ExportError::BadOption(format!(
                    "unknown option \"{k}\" (known: {})",
                    keys.join(", ")
                ))
            })?;
            self.0.insert(k.clone(), spec.parse(v)?);
        }
        Ok(())
    }

    pub fn bool(&self, k: &str) -> bool {
        matches!(self.0.get(k), Some(OptValue::Bool(true)))
    }

    pub fn int(&self, k: &str) -> i64 {
        match self.0.get(k) {
            Some(OptValue::Int(i)) => *i,
            _ => 0,
        }
    }

    pub fn float(&self, k: &str) -> f64 {
        match self.0.get(k) {
            Some(OptValue::Float(x)) => *x,
            Some(OptValue::Int(i)) => *i as f64,
            _ => 0.0,
        }
    }

    pub fn text(&self, k: &str) -> &str {
        match self.0.get(k) {
            Some(OptValue::Text(s)) => s,
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs() -> Vec<OptionSpec> {
        vec![
            OptionSpec {
                key: "n",
                label: "N",
                help: "",
                kind: OptKind::Int { min: 1, max: 5 },
                default: OptValue::Int(2),
            },
            OptionSpec {
                key: "f",
                label: "F",
                help: "",
                kind: OptKind::Float { min: 0.0, max: 1.0 },
                default: OptValue::Float(0.5),
            },
            OptionSpec {
                key: "b",
                label: "B",
                help: "",
                kind: OptKind::Bool,
                default: OptValue::Bool(false),
            },
            OptionSpec {
                key: "e",
                label: "E",
                help: "",
                kind: OptKind::Enum(&["a", "b"]),
                default: OptValue::Text("a".into()),
            },
        ]
    }

    fn kv(k: &str, v: &str) -> Vec<(String, String)> {
        vec![(k.into(), v.into())]
    }

    #[test]
    fn validates_every_kind() {
        let s = specs();
        let mut o = Options::defaults(&s);
        assert_eq!(o.int("n"), 2);
        o.apply(&s, &kv("n", "5")).unwrap();
        o.apply(&s, &kv("f", "0.25")).unwrap();
        o.apply(&s, &kv("b", "true")).unwrap();
        o.apply(&s, &kv("e", "b")).unwrap();
        assert_eq!(
            (o.int("n"), o.float("f"), o.bool("b"), o.text("e")),
            (5, 0.25, true, "b")
        );
        for (k, v) in [
            ("n", "6"),
            ("n", "x"),
            ("f", "2"),
            ("f", "nan"),
            ("b", "maybe"),
            ("e", "c"),
            ("zzz", "1"),
        ] {
            assert!(
                matches!(o.apply(&s, &kv(k, v)), Err(ExportError::BadOption(_))),
                "{k}={v}"
            );
        }
    }
}
