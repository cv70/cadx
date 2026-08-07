//! Kernel-neutral product structure: definitions, occurrences, and mates.

mod component;
mod mate;
mod transform;
mod validate;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::FeatureId;

use component::validate_name;

pub use component::{
    ComponentDefinition, ComponentKind, ComponentOccurrence, MAX_ASSEMBLY_MATES,
    MAX_ASSEMBLY_NAME_LENGTH, MAX_COMPONENT_DEFINITIONS, MAX_COMPONENT_OCCURRENCES,
    MAX_OCCURRENCE_FEATURES, StepEntityRef,
};
pub use mate::{AssemblyMate, AssemblyMateKind, AssemblyMateLimits};
pub use transform::AssemblyTransform;

pub type AssemblyId = u64;
pub type ComponentDefinitionId = u64;
pub type ComponentOccurrenceId = u64;
pub type AssemblyMateId = u64;

/// Stable identity of one ordered geometry body in a reusable definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssemblyDefinitionBody {
    pub assembly_id: AssemblyId,
    pub definition_id: ComponentDefinitionId,
    pub body_slot: usize,
}

/// Stable assembly ownership of one materialized feature body.
///
/// `body_slot` is the feature's ordered position within its occurrence. Equal
/// slots on repeated occurrences of one definition identify candidate reusable
/// component-local geometry without exposing kernel-native objects to core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssemblyFeatureInstance {
    pub assembly_id: AssemblyId,
    pub definition_id: ComponentDefinitionId,
    pub occurrence_id: ComponentOccurrenceId,
    pub body_slot: usize,
}

impl AssemblyFeatureInstance {
    #[must_use]
    pub const fn definition_body(self) -> AssemblyDefinitionBody {
        AssemblyDefinitionBody {
            assembly_id: self.assembly_id,
            definition_id: self.definition_id,
            body_slot: self.body_slot,
        }
    }
}

/// A validated product structure independent of kernel-native geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assembly {
    pub id: AssemblyId,
    pub name: String,
    pub definitions: Vec<ComponentDefinition>,
    pub occurrences: Vec<ComponentOccurrence>,
    #[serde(default)]
    pub mates: Vec<AssemblyMate>,
}

impl Assembly {
    #[must_use]
    pub fn occurrence(&self, id: ComponentOccurrenceId) -> Option<&ComponentOccurrence> {
        self.occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
    }

    #[must_use]
    pub fn definition(&self, id: ComponentDefinitionId) -> Option<&ComponentDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.id == id)
    }

    #[must_use]
    pub fn mate(&self, id: AssemblyMateId) -> Option<&AssemblyMate> {
        self.mates.iter().find(|mate| mate.id == id)
    }

    #[must_use]
    pub fn mate_for_child(&self, child: ComponentOccurrenceId) -> Option<&AssemblyMate> {
        self.mates
            .iter()
            .find(|mate| mate.child_occurrence_id == child)
    }

    pub fn roots(&self) -> impl Iterator<Item = &ComponentOccurrence> {
        self.occurrences
            .iter()
            .filter(|occurrence| occurrence.parent_id.is_none())
    }

    pub fn children(
        &self,
        parent: ComponentOccurrenceId,
    ) -> impl Iterator<Item = &ComponentOccurrence> {
        self.occurrences
            .iter()
            .filter(move |occurrence| occurrence.parent_id == Some(parent))
    }

    /// Resolves direct and inherited suppression for the complete hierarchy.
    ///
    /// # Errors
    ///
    /// Returns [`AssemblyError::UnresolvableOccurrenceHierarchy`] if the
    /// hierarchy is cyclic or references a missing parent.
    pub fn effective_suppression(
        &self,
    ) -> Result<BTreeMap<ComponentOccurrenceId, bool>, AssemblyError> {
        let mut effective = BTreeMap::new();
        let mut unresolved = self.occurrences.iter().collect::<Vec<_>>();
        while !unresolved.is_empty() {
            let previous_len = unresolved.len();
            unresolved.retain(|occurrence| {
                let inherited = match occurrence.parent_id {
                    None => Some(false),
                    Some(parent) => effective.get(&parent).copied(),
                };
                let Some(inherited) = inherited else {
                    return true;
                };
                effective.insert(occurrence.id, occurrence.suppressed || inherited);
                false
            });
            if unresolved.len() == previous_len {
                return Err(AssemblyError::UnresolvableOccurrenceHierarchy);
            }
        }
        Ok(effective)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AssemblyError {
    #[error("assembly id {0} must be non-zero and unique")]
    InvalidAssemblyId(AssemblyId),
    #[error(
        "assembly, component, occurrence, and mate names must contain 1 to {MAX_ASSEMBLY_NAME_LENGTH} characters"
    )]
    InvalidName,
    #[error(
        "assembly must contain 1 to {MAX_COMPONENT_DEFINITIONS} component definitions, got {0}"
    )]
    InvalidDefinitionCount(usize),
    #[error(
        "assembly must contain 1 to {MAX_COMPONENT_OCCURRENCES} component occurrences, got {0}"
    )]
    InvalidOccurrenceCount(usize),
    #[error("component definition id {0} must be non-zero and unique within its assembly")]
    InvalidDefinitionId(ComponentDefinitionId),
    #[error("component occurrence id {0} must be non-zero and unique within its assembly")]
    InvalidOccurrenceId(ComponentOccurrenceId),
    #[error("assembly mate id {0} must be non-zero and unique within its assembly")]
    InvalidMateId(AssemblyMateId),
    #[error("STEP source entity ids must be non-zero")]
    InvalidSourceEntity,
    #[error("assembly transform contains non-finite values")]
    NonFiniteTransform,
    #[error("assembly transform must contain a right-handed orthonormal rotation")]
    NonRigidTransform,
    #[error("occurrence {occurrence} references missing component definition {definition}")]
    MissingDefinition {
        occurrence: ComponentOccurrenceId,
        definition: ComponentDefinitionId,
    },
    #[error("occurrence {occurrence} references missing parent occurrence {parent}")]
    MissingParent {
        occurrence: ComponentOccurrenceId,
        parent: ComponentOccurrenceId,
    },
    #[error("part occurrence {parent} cannot contain child occurrence {child}")]
    PartCannotContainOccurrence {
        parent: ComponentOccurrenceId,
        child: ComponentOccurrenceId,
    },
    #[error("assembly has no root occurrence")]
    MissingRootOccurrence,
    #[error("component definition {0} is not used by any occurrence")]
    UnusedDefinition(ComponentDefinitionId),
    #[error("occurrence hierarchy contains a cycle reachable from {occurrence}")]
    OccurrenceCycle { occurrence: ComponentOccurrenceId },
    #[error("occurrence hierarchy is cyclic or references a missing parent")]
    UnresolvableOccurrenceHierarchy,
    #[error("assembly contains too many mates: {0}")]
    TooManyMates(usize),
    #[error("mate {mate} references missing occurrence {occurrence}")]
    MateOccurrenceNotFound {
        mate: AssemblyMateId,
        occurrence: ComponentOccurrenceId,
    },
    #[error("mate {mate} cannot drive root occurrence {occurrence}")]
    MateDrivesRoot {
        mate: AssemblyMateId,
        occurrence: ComponentOccurrenceId,
    },
    #[error(
        "mate {mate} parent {actual} does not match child {child}'s hierarchy parent {expected}"
    )]
    MateParentMismatch {
        mate: AssemblyMateId,
        child: ComponentOccurrenceId,
        expected: ComponentOccurrenceId,
        actual: ComponentOccurrenceId,
    },
    #[error("occurrence {occurrence} is driven by mates {first} and {second}")]
    OccurrenceDrivenMultipleTimes {
        occurrence: ComponentOccurrenceId,
        first: AssemblyMateId,
        second: AssemblyMateId,
    },
    #[error("mate {mate} axis must be finite and normalized")]
    InvalidMateAxis { mate: AssemblyMateId },
    #[error("mate {mate} limits must be finite and ordered")]
    InvalidMateLimits { mate: AssemblyMateId },
    #[error("mate {mate} state must be finite")]
    NonFiniteMateState { mate: AssemblyMateId },
    #[error("fixed mate {mate} state must be zero")]
    FixedMateState { mate: AssemblyMateId },
    #[error("mate {mate} state is outside its limits")]
    MateStateOutsideLimits { mate: AssemblyMateId },
    #[error("mate {mate} solved pose does not match driven occurrence {occurrence}")]
    MateTransformMismatch {
        mate: AssemblyMateId,
        occurrence: ComponentOccurrenceId,
    },
    #[error("occurrence {occurrence} contains too many feature bodies: {count}")]
    TooManyOccurrenceFeatures {
        occurrence: ComponentOccurrenceId,
        count: usize,
    },
    #[error("occurrence {occurrence} contains invalid or duplicate feature {feature}")]
    InvalidOccurrenceFeature {
        occurrence: ComponentOccurrenceId,
        feature: FeatureId,
    },
    #[error("occurrence {occurrence} references missing feature {feature}")]
    MissingFeature {
        occurrence: ComponentOccurrenceId,
        feature: FeatureId,
    },
    #[error("occurrence {occurrence} references non-solid feature {feature}")]
    NonSolidFeature {
        occurrence: ComponentOccurrenceId,
        feature: FeatureId,
    },
    #[error("occurrence {occurrence} placement does not match feature {feature} transform")]
    FeatureTransformMismatch {
        occurrence: ComponentOccurrenceId,
        feature: FeatureId,
    },
    #[error("occurrence {occurrence} placement cannot be represented by feature Euler rotation")]
    UnrepresentableFeatureTransform { occurrence: ComponentOccurrenceId },
    #[error("feature {feature} is owned by occurrences {first} and {second}")]
    FeatureOwnedMultipleTimes {
        feature: FeatureId,
        first: ComponentOccurrenceId,
        second: ComponentOccurrenceId,
    },
}
