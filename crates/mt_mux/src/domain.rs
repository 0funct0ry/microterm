//! Domains: where a pane's process lives (SPEC §5): `Local`, and since M10 `Ssh`.

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
    /// Where to run it; `LOCAL_DOMAIN_ID` (0) by default.
    pub domain: DomainId,
    pub cwd: Option<PathBuf>,
    pub command: Option<Vec<String>>,
    pub title: Option<String>,
    /// Set for a pane created by a split: the pane it was split from, with the axis and the
    /// new pane's share (remote domains record it on their daemon).
    pub split_from: Option<SplitOrigin>,
    /// Extra environment for the process (context-menu `spawn`: `MT_*`).
    pub env: Vec<(String, String)>,
    /// Withhold the ctl socket and `MICROTERM_*` ids (context-menu `spawn` without `ctl`).
    pub no_ctl: bool,
}

/// Where a split pane came from (see [`SpawnSpec::split_from`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitOrigin {
    pub parent: u64,
    pub axis: crate::model::Axis,
    /// The new pane's share of the space, in thousandths.
    pub permille: u16,
}

pub trait Domain: Send + Sync {
    fn id(&self) -> DomainId;
    fn name(&self) -> &str;
    fn kind(&self) -> DomainKind;
}

/// Connection state of a domain (SPEC §5 `DomainStateChanged`). The local domain is always
/// `Connected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, next_in_secs: u32 },
    Disconnected { reason: String },
}

impl DomainState {
    /// Short lowercase name used in events and the CLI.
    pub fn label(&self) -> &'static str {
        match self {
            DomainState::Connecting => "connecting",
            DomainState::Connected => "connected",
            DomainState::Reconnecting { .. } => "reconnecting",
            DomainState::Disconnected { .. } => "disconnected",
        }
    }
}

/// A named SSH destination (`[[ssh_domains]]` or an ad-hoc `microterm ssh host`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshDomain {
    id: DomainId,
    name: String,
}

impl SshDomain {
    pub fn new(id: DomainId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

impl Domain for SshDomain {
    fn id(&self) -> DomainId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> DomainKind {
        DomainKind::Ssh {
            name: self.name.clone(),
        }
    }
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
    fn ssh_domain_identity_and_state_labels() {
        let d = SshDomain::new(3, "prod");
        assert_eq!((d.id(), d.name()), (3, "prod"));
        assert_eq!(
            d.kind(),
            DomainKind::Ssh {
                name: "prod".into()
            }
        );
        assert_eq!(
            DomainState::Reconnecting {
                attempt: 2,
                next_in_secs: 4
            }
            .label(),
            "reconnecting"
        );
        assert_eq!(
            DomainState::Disconnected { reason: "x".into() }.label(),
            "disconnected"
        );
    }

    #[test]
    fn local_domain_identity() {
        let d = LocalDomain;
        assert_eq!(d.id(), LOCAL_DOMAIN_ID);
        assert_eq!(d.name(), "local");
        assert_eq!(d.kind(), DomainKind::Local);
    }
}
