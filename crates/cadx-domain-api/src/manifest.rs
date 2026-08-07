//! Domain identity and static pack advertisement records.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DomainId {
    Mcad,
    Aec,
    Ecad,
}

impl DomainId {
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Mcad => "mcad",
            Self::Aec => "aec",
            Self::Ecad => "ecad",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainManifest {
    pub id: DomainId,
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub priority: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainTool {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub category: &'static str,
}
