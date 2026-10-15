//! Domains: where a pane's process lives (SPEC §5). Only `Local` exists in v1.0 M5.

use std::path::PathBuf;

pub type DomainId = u64;

/// The local domain always has this id.
pub const LOCAL_DOMAIN_ID: DomainId = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainKind {
    Local,
    Ssh { name: String },
    Serial { name: String },
}

/// What the UI should launch for a new pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnSpec {
    pub cwd: Option<PathBuf>,
    pub command: Option<Vec<String>>,
    pub title: Option<String>,
}

pub trait Domain: Send + Sync {
    fn id(&self) -> DomainId;
    fn name(&self) -> &str;
    fn kind(&self) -> DomainKind;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalDomain;

impl Domain for LocalDomain {
    fn id(&self) -> DomainId {
        LOCAL_DOMAIN_ID
    }
    fn name(&self) -> &str {
        "local"
    }
    fn kind(&self) -> DomainKind {
        DomainKind::Local
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_domain_identity() {
        let d = LocalDomain;
        assert_eq!(d.id(), LOCAL_DOMAIN_ID);
        assert_eq!(d.name(), "local");
        assert_eq!(d.kind(), DomainKind::Local);
    }
}
