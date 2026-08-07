//! Reusable component definitions, their placed occurrences, and STEP sources.

use serde::{Deserialize, Serialize};

use crate::domain::FeatureId;

use super::{AssemblyError, AssemblyTransform, ComponentDefinitionId, ComponentOccurrenceId};

pub const MAX_ASSEMBLY_NAME_LENGTH: usize = 160;
pub const MAX_COMPONENT_DEFINITIONS: usize = 65_536;
pub const MAX_COMPONENT_OCCURRENCES: usize = 262_144;
pub const MAX_OCCURRENCE_FEATURES: usize = 4_096;
pub const MAX_ASSEMBLY_MATES: usize = MAX_COMPONENT_OCCURRENCES - 1;

/// Stable identity of one source entity in an embedded STEP physical file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StepEntityRef {
    pub data_section: usize,
    pub entity_id: u64,
}

impl StepEntityRef {
    pub(super) fn validate(self) -> Result<(), AssemblyError> {
        if self.entity_id == 0 {
            return Err(AssemblyError::InvalidSourceEntity);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    Part,
    Assembly,
}

/// One reusable product definition in an assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentDefinition {
    pub id: ComponentDefinitionId,
    pub name: String,
    pub kind: ComponentKind,
    #[serde(default)]
    pub source: Option<StepEntityRef>,
}

/// One placed use of a component definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentOccurrence {
    pub id: ComponentOccurrenceId,
    pub name: String,
    pub definition_id: ComponentDefinitionId,
    #[serde(default)]
    pub parent_id: Option<ComponentOccurrenceId>,
    /// Direct suppression state. A suppressed ancestor also suppresses this
    /// occurrence without changing this stored value.
    #[serde(default)]
    pub suppressed: bool,
    #[serde(default)]
    pub transform: AssemblyTransform,
    /// Concrete feature bodies materialized for this occurrence.
    #[serde(default)]
    pub feature_ids: Vec<FeatureId>,
    #[serde(default)]
    pub source: Option<StepEntityRef>,
}

pub(super) fn validate_name(name: &str) -> Result<(), AssemblyError> {
    let count = name.trim().chars().count();
    if count == 0 || count > MAX_ASSEMBLY_NAME_LENGTH {
        return Err(AssemblyError::InvalidName);
    }
    Ok(())
}
