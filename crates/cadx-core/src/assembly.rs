use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::FeatureId;

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
    fn validate(self) -> Result<(), AssemblyError> {
        if self.entity_id == 0 {
            return Err(AssemblyError::InvalidSourceEntity);
        }
        Ok(())
    }
}

/// A right-handed rigid placement from component-local to parent coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AssemblyTransform {
    pub translation: [f64; 3],
    /// Row-major orthonormal rotation matrix.
    pub rotation: [[f64; 3]; 3],
}

impl AssemblyTransform {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    #[must_use]
    pub fn compose(self, local: Self) -> Self {
        let rotation = std::array::from_fn(|row| {
            std::array::from_fn(|column| {
                (0..3)
                    .map(|axis| self.rotation[row][axis] * local.rotation[axis][column])
                    .sum()
            })
        });
        let translation = std::array::from_fn(|row| {
            self.translation[row]
                + (0..3)
                    .map(|axis| self.rotation[row][axis] * local.translation[axis])
                    .sum::<f64>()
        });
        Self {
            translation,
            rotation,
        }
    }

    /// Returns the inverse parent-to-local rigid transform.
    #[must_use]
    pub fn inverse(self) -> Self {
        let rotation =
            std::array::from_fn(|row| std::array::from_fn(|column| self.rotation[column][row]));
        let translation = std::array::from_fn(|row| {
            -(0..3)
                .map(|axis| rotation[row][axis] * self.translation[axis])
                .sum::<f64>()
        });
        Self {
            translation,
            rotation,
        }
    }

    /// Applies this placement to a point, including translation.
    #[must_use]
    pub fn transform_point(self, point: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|row| {
            self.translation[row]
                + (0..3)
                    .map(|axis| self.rotation[row][axis] * point[axis])
                    .sum::<f64>()
        })
    }

    /// Applies only this placement's rotation to a direction vector.
    #[must_use]
    pub fn transform_vector(self, vector: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|row| {
            (0..3)
                .map(|axis| self.rotation[row][axis] * vector[axis])
                .sum()
        })
    }

    #[must_use]
    pub fn from_euler_xyz_degrees(translation: [f64; 3], rotation: [f64; 3]) -> Self {
        let [x, y, z] = rotation.map(f64::to_radians);
        let (sin_x, cos_x) = x.sin_cos();
        let (sin_y, cos_y) = y.sin_cos();
        let (sin_z, cos_z) = z.sin_cos();
        Self {
            translation,
            rotation: [
                [
                    cos_z * cos_y,
                    cos_z * sin_y * sin_x - sin_z * cos_x,
                    cos_z * sin_y * cos_x + sin_z * sin_x,
                ],
                [
                    sin_z * cos_y,
                    sin_z * sin_y * sin_x + cos_z * cos_x,
                    sin_z * sin_y * cos_x - cos_z * sin_x,
                ],
                [-sin_y, cos_y * sin_x, cos_y * cos_x],
            ],
        }
    }

    /// Converts `Rz * Ry * Rx` to the Euler convention used by solid features.
    #[must_use]
    pub fn euler_xyz_degrees(self) -> [f64; 3] {
        let sine_y = (-self.rotation[2][0]).clamp(-1.0, 1.0);
        let y = sine_y.asin();
        let cosine_y = y.cos();
        let (x, z) = if cosine_y.abs() > 1.0e-10 {
            (
                self.rotation[2][1].atan2(self.rotation[2][2]),
                self.rotation[1][0].atan2(self.rotation[0][0]),
            )
        } else {
            ((-self.rotation[1][2]).atan2(self.rotation[1][1]), 0.0)
        };
        [x.to_degrees(), y.to_degrees(), z.to_degrees()]
    }

    #[must_use]
    pub fn approximately_equals(self, other: Self, tolerance: f64) -> bool {
        self.translation
            .into_iter()
            .zip(other.translation)
            .chain(
                self.rotation
                    .into_iter()
                    .flatten()
                    .zip(other.rotation.into_iter().flatten()),
            )
            .all(|(left, right)| (left - right).abs() <= tolerance)
    }

    pub(crate) fn validate(self) -> Result<(), AssemblyError> {
        const TOLERANCE: f64 = 1.0e-9;

        if self
            .translation
            .into_iter()
            .chain(self.rotation.into_iter().flatten())
            .any(|value| !value.is_finite())
        {
            return Err(AssemblyError::NonFiniteTransform);
        }
        for row in 0..3 {
            for other in 0..3 {
                let dot = (0..3)
                    .map(|axis| self.rotation[row][axis] * self.rotation[other][axis])
                    .sum::<f64>();
                let expected = if row == other { 1.0 } else { 0.0 };
                if (dot - expected).abs() > TOLERANCE {
                    return Err(AssemblyError::NonRigidTransform);
                }
            }
        }
        let determinant = self.rotation[0][0]
            * (self.rotation[1][1] * self.rotation[2][2]
                - self.rotation[1][2] * self.rotation[2][1])
            - self.rotation[0][1]
                * (self.rotation[1][0] * self.rotation[2][2]
                    - self.rotation[1][2] * self.rotation[2][0])
            + self.rotation[0][2]
                * (self.rotation[1][0] * self.rotation[2][1]
                    - self.rotation[1][1] * self.rotation[2][0]);
        if (determinant - 1.0).abs() > TOLERANCE {
            return Err(AssemblyError::NonRigidTransform);
        }
        Ok(())
    }
}

impl Default for AssemblyTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// One scalar degree-of-freedom limit in the mate kind's declared unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AssemblyMateLimits {
    pub min: f64,
    pub max: f64,
}

/// Supported deterministic assembly motion constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssemblyMateKind {
    Fixed,
    /// Rotation in degrees about an axis expressed in the parent anchor frame.
    Revolute {
        axis: [f64; 3],
        #[serde(default)]
        limits_deg: Option<AssemblyMateLimits>,
    },
    /// Translation in millimeters along an axis expressed in the parent anchor frame.
    Slider {
        axis: [f64; 3],
        #[serde(default)]
        limits_mm: Option<AssemblyMateLimits>,
    },
}

impl AssemblyMateKind {
    fn axis_and_limits(&self) -> Option<([f64; 3], Option<AssemblyMateLimits>)> {
        match *self {
            Self::Fixed => None,
            Self::Revolute { axis, limits_deg } => Some((axis, limits_deg)),
            Self::Slider { axis, limits_mm } => Some((axis, limits_mm)),
        }
    }

    fn motion(&self, state: f64) -> AssemblyTransform {
        match *self {
            Self::Fixed => AssemblyTransform::IDENTITY,
            Self::Revolute { axis, .. } => {
                let angle = state.to_radians();
                let (sine, cosine) = angle.sin_cos();
                let one_minus_cosine = 1.0 - cosine;
                let [x, y, z] = axis;
                AssemblyTransform {
                    translation: [0.0; 3],
                    rotation: [
                        [
                            cosine + x * x * one_minus_cosine,
                            x * y * one_minus_cosine - z * sine,
                            x * z * one_minus_cosine + y * sine,
                        ],
                        [
                            y * x * one_minus_cosine + z * sine,
                            cosine + y * y * one_minus_cosine,
                            y * z * one_minus_cosine - x * sine,
                        ],
                        [
                            z * x * one_minus_cosine - y * sine,
                            z * y * one_minus_cosine + x * sine,
                            cosine + z * z * one_minus_cosine,
                        ],
                    ],
                }
            }
            Self::Slider { axis, .. } => AssemblyTransform {
                translation: axis.map(|component| component * state),
                rotation: AssemblyTransform::IDENTITY.rotation,
            },
        }
    }
}

/// A kinematic constraint that drives one occurrence from its hierarchy parent.
///
/// Anchor frames map mate-frame coordinates into their respective occurrence-local
/// coordinates. The solved child placement is
/// `parent_frame * motion(kind, state) * inverse(child_frame)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssemblyMate {
    pub id: AssemblyMateId,
    pub name: String,
    pub parent_occurrence_id: ComponentOccurrenceId,
    pub child_occurrence_id: ComponentOccurrenceId,
    pub parent_frame: AssemblyTransform,
    pub child_frame: AssemblyTransform,
    pub kind: AssemblyMateKind,
    pub state: f64,
}

impl AssemblyMate {
    /// Resolves the child occurrence's local placement at the current state.
    #[must_use]
    pub fn local_transform(&self) -> AssemblyTransform {
        self.parent_frame
            .compose(self.kind.motion(self.state))
            .compose(self.child_frame.inverse())
    }

    fn validate(&self) -> Result<(), AssemblyError> {
        if self.id == 0 {
            return Err(AssemblyError::InvalidMateId(self.id));
        }
        validate_name(&self.name)?;
        self.parent_frame.validate()?;
        self.child_frame.validate()?;
        if !self.state.is_finite() {
            return Err(AssemblyError::NonFiniteMateState { mate: self.id });
        }
        let Some((axis, limits)) = self.kind.axis_and_limits() else {
            if self.state != 0.0 {
                return Err(AssemblyError::FixedMateState { mate: self.id });
            }
            return Ok(());
        };
        if axis.into_iter().any(|component| !component.is_finite()) {
            return Err(AssemblyError::InvalidMateAxis { mate: self.id });
        }
        let length_squared = axis
            .into_iter()
            .map(|component| component * component)
            .sum::<f64>();
        if !length_squared.is_finite() || (length_squared - 1.0).abs() > 1.0e-9 {
            return Err(AssemblyError::InvalidMateAxis { mate: self.id });
        }
        if let Some(limits) = limits {
            if !limits.min.is_finite() || !limits.max.is_finite() || limits.min > limits.max {
                return Err(AssemblyError::InvalidMateLimits { mate: self.id });
            }
            if self.state < limits.min || self.state > limits.max {
                return Err(AssemblyError::MateStateOutsideLimits { mate: self.id });
            }
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
    pub(crate) fn validate(
        &self,
        available_features: &BTreeSet<FeatureId>,
    ) -> Result<(), AssemblyError> {
        if self.id == 0 {
            return Err(AssemblyError::InvalidAssemblyId(self.id));
        }
        validate_name(&self.name)?;
        if self.definitions.is_empty() || self.definitions.len() > MAX_COMPONENT_DEFINITIONS {
            return Err(AssemblyError::InvalidDefinitionCount(
                self.definitions.len(),
            ));
        }
        if self.occurrences.is_empty() || self.occurrences.len() > MAX_COMPONENT_OCCURRENCES {
            return Err(AssemblyError::InvalidOccurrenceCount(
                self.occurrences.len(),
            ));
        }

        let mut definitions = BTreeMap::new();
        for definition in &self.definitions {
            if definition.id == 0 || definitions.insert(definition.id, definition).is_some() {
                return Err(AssemblyError::InvalidDefinitionId(definition.id));
            }
            validate_name(&definition.name)?;
            if let Some(source) = definition.source {
                source.validate()?;
            }
        }

        let mut occurrences = BTreeMap::new();
        for occurrence in &self.occurrences {
            if occurrence.id == 0 || occurrences.insert(occurrence.id, occurrence).is_some() {
                return Err(AssemblyError::InvalidOccurrenceId(occurrence.id));
            }
            validate_name(&occurrence.name)?;
            if !definitions.contains_key(&occurrence.definition_id) {
                return Err(AssemblyError::MissingDefinition {
                    occurrence: occurrence.id,
                    definition: occurrence.definition_id,
                });
            }
            occurrence.transform.validate()?;
            if let Some(source) = occurrence.source {
                source.validate()?;
            }
            if occurrence.feature_ids.len() > MAX_OCCURRENCE_FEATURES {
                return Err(AssemblyError::TooManyOccurrenceFeatures {
                    occurrence: occurrence.id,
                    count: occurrence.feature_ids.len(),
                });
            }
            let mut feature_ids = BTreeSet::new();
            for feature_id in &occurrence.feature_ids {
                if *feature_id == 0 || !feature_ids.insert(*feature_id) {
                    return Err(AssemblyError::InvalidOccurrenceFeature {
                        occurrence: occurrence.id,
                        feature: *feature_id,
                    });
                }
                if !available_features.contains(feature_id) {
                    return Err(AssemblyError::MissingFeature {
                        occurrence: occurrence.id,
                        feature: *feature_id,
                    });
                }
            }
        }

        let mut owned_features = BTreeMap::new();
        let mut referenced_definitions = BTreeSet::new();
        let mut has_root = false;
        for occurrence in &self.occurrences {
            referenced_definitions.insert(occurrence.definition_id);
            has_root |= occurrence.parent_id.is_none();
            if let Some(parent_id) = occurrence.parent_id {
                let parent = occurrences
                    .get(&parent_id)
                    .ok_or(AssemblyError::MissingParent {
                        occurrence: occurrence.id,
                        parent: parent_id,
                    })?;
                if definitions[&parent.definition_id].kind != ComponentKind::Assembly {
                    return Err(AssemblyError::PartCannotContainOccurrence {
                        parent: parent_id,
                        child: occurrence.id,
                    });
                }
            }
            for feature_id in &occurrence.feature_ids {
                if let Some(owner) = owned_features.insert(*feature_id, occurrence.id) {
                    return Err(AssemblyError::FeatureOwnedMultipleTimes {
                        feature: *feature_id,
                        first: owner,
                        second: occurrence.id,
                    });
                }
            }
        }
        if !has_root {
            return Err(AssemblyError::MissingRootOccurrence);
        }
        if let Some(definition) = self
            .definitions
            .iter()
            .find(|definition| !referenced_definitions.contains(&definition.id))
        {
            return Err(AssemblyError::UnusedDefinition(definition.id));
        }

        for occurrence in &self.occurrences {
            let mut chain = BTreeSet::new();
            let mut current = Some(occurrence.id);
            while let Some(id) = current {
                if !chain.insert(id) {
                    return Err(AssemblyError::OccurrenceCycle {
                        occurrence: occurrence.id,
                    });
                }
                current = occurrences.get(&id).and_then(|node| node.parent_id);
            }
        }

        if self.mates.len() > MAX_ASSEMBLY_MATES {
            return Err(AssemblyError::TooManyMates(self.mates.len()));
        }
        let mut mate_ids = BTreeSet::new();
        let mut driven_children = BTreeMap::new();
        for mate in &self.mates {
            mate.validate()?;
            if !mate_ids.insert(mate.id) {
                return Err(AssemblyError::InvalidMateId(mate.id));
            }
            let child = occurrences.get(&mate.child_occurrence_id).ok_or(
                AssemblyError::MateOccurrenceNotFound {
                    mate: mate.id,
                    occurrence: mate.child_occurrence_id,
                },
            )?;
            if !occurrences.contains_key(&mate.parent_occurrence_id) {
                return Err(AssemblyError::MateOccurrenceNotFound {
                    mate: mate.id,
                    occurrence: mate.parent_occurrence_id,
                });
            }
            let Some(actual_parent) = child.parent_id else {
                return Err(AssemblyError::MateDrivesRoot {
                    mate: mate.id,
                    occurrence: child.id,
                });
            };
            if actual_parent != mate.parent_occurrence_id {
                return Err(AssemblyError::MateParentMismatch {
                    mate: mate.id,
                    child: child.id,
                    expected: actual_parent,
                    actual: mate.parent_occurrence_id,
                });
            }
            if let Some(first) = driven_children.insert(child.id, mate.id) {
                return Err(AssemblyError::OccurrenceDrivenMultipleTimes {
                    occurrence: child.id,
                    first,
                    second: mate.id,
                });
            }
            if !child
                .transform
                .approximately_equals(mate.local_transform(), 1.0e-8)
            {
                return Err(AssemblyError::MateTransformMismatch {
                    mate: mate.id,
                    occurrence: child.id,
                });
            }
        }
        Ok(())
    }

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

    pub(crate) fn world_transforms(
        &self,
    ) -> Result<BTreeMap<ComponentOccurrenceId, AssemblyTransform>, AssemblyError> {
        let mut world = BTreeMap::new();
        let mates_by_child = self
            .mates
            .iter()
            .map(|mate| (mate.child_occurrence_id, mate))
            .collect::<BTreeMap<_, _>>();
        let mut unresolved = self.occurrences.iter().collect::<Vec<_>>();
        while !unresolved.is_empty() {
            let previous_len = unresolved.len();
            unresolved.retain(|occurrence| {
                let local = mates_by_child
                    .get(&occurrence.id)
                    .copied()
                    .map_or(occurrence.transform, AssemblyMate::local_transform);
                let transform = match occurrence.parent_id {
                    None => Some(local),
                    Some(parent) => world
                        .get(&parent)
                        .copied()
                        .map(|parent_transform: AssemblyTransform| parent_transform.compose(local)),
                };
                let Some(transform) = transform else {
                    return true;
                };
                world.insert(occurrence.id, transform);
                false
            });
            if unresolved.len() == previous_len {
                return Err(AssemblyError::UnresolvableOccurrenceHierarchy);
            }
        }
        Ok(world)
    }
}

fn validate_name(name: &str) -> Result<(), AssemblyError> {
    let count = name.trim().chars().count();
    if count == 0 || count > MAX_ASSEMBLY_NAME_LENGTH {
        return Err(AssemblyError::InvalidName);
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn occurrence(
        id: ComponentOccurrenceId,
        definition_id: ComponentDefinitionId,
        parent_id: Option<ComponentOccurrenceId>,
        transform: AssemblyTransform,
    ) -> ComponentOccurrence {
        ComponentOccurrence {
            id,
            name: format!("occurrence {id}"),
            definition_id,
            parent_id,
            suppressed: false,
            transform,
            feature_ids: Vec::new(),
            source: None,
        }
    }

    fn kinematic_assembly() -> Assembly {
        let revolute = AssemblyMate {
            id: 1,
            name: "shoulder".into(),
            parent_occurrence_id: 1,
            child_occurrence_id: 2,
            parent_frame: AssemblyTransform {
                translation: [10.0, 0.0, 0.0],
                ..AssemblyTransform::IDENTITY
            },
            child_frame: AssemblyTransform {
                translation: [2.0, 0.0, 0.0],
                ..AssemblyTransform::IDENTITY
            },
            kind: AssemblyMateKind::Revolute {
                axis: [0.0, 0.0, 1.0],
                limits_deg: Some(AssemblyMateLimits {
                    min: -180.0,
                    max: 180.0,
                }),
            },
            state: 90.0,
        };
        let slider = AssemblyMate {
            id: 2,
            name: "extension".into(),
            parent_occurrence_id: 2,
            child_occurrence_id: 3,
            parent_frame: AssemblyTransform {
                translation: [0.0, 5.0, 0.0],
                ..AssemblyTransform::IDENTITY
            },
            child_frame: AssemblyTransform::IDENTITY,
            kind: AssemblyMateKind::Slider {
                axis: [1.0, 0.0, 0.0],
                limits_mm: Some(AssemblyMateLimits {
                    min: 0.0,
                    max: 10.0,
                }),
            },
            state: 4.0,
        };
        Assembly {
            id: 1,
            name: "robot".into(),
            definitions: vec![
                ComponentDefinition {
                    id: 1,
                    name: "base".into(),
                    kind: ComponentKind::Assembly,
                    source: None,
                },
                ComponentDefinition {
                    id: 2,
                    name: "arm".into(),
                    kind: ComponentKind::Assembly,
                    source: None,
                },
                ComponentDefinition {
                    id: 3,
                    name: "tool".into(),
                    kind: ComponentKind::Part,
                    source: None,
                },
            ],
            occurrences: vec![
                occurrence(
                    1,
                    1,
                    None,
                    AssemblyTransform {
                        translation: [50.0, -10.0, 5.0],
                        rotation: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                    },
                ),
                occurrence(2, 2, Some(1), revolute.local_transform()),
                occurrence(3, 3, Some(2), slider.local_transform()),
            ],
            mates: vec![revolute, slider],
        }
    }

    #[test]
    fn rigid_transform_composes_and_converts_to_feature_euler_angles() {
        let rotation_z_90 = AssemblyTransform {
            translation: [10.0, 0.0, 0.0],
            rotation: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let local = AssemblyTransform {
            translation: [2.0, 0.0, 0.0],
            ..AssemblyTransform::IDENTITY
        };
        let world = rotation_z_90.compose(local);
        assert!(
            world
                .translation
                .into_iter()
                .zip([10.0, 2.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        assert!(
            world
                .euler_xyz_degrees()
                .into_iter()
                .zip([0.0, 0.0, 90.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        world.validate().unwrap();

        let point = [3.0, -2.0, 5.0];
        let transformed = world.transform_point(point);
        let restored = world.inverse().transform_point(transformed);
        assert!(
            restored
                .into_iter()
                .zip(point)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        let direction = [0.25, -0.5, 0.75];
        let restored_direction = world
            .inverse()
            .transform_vector(world.transform_vector(direction));
        assert!(
            restored_direction
                .into_iter()
                .zip(direction)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
    }

    #[test]
    fn reflections_and_scaled_placements_are_not_rigid() {
        for rotation in [
            [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        ] {
            assert!(matches!(
                AssemblyTransform {
                    translation: [0.0; 3],
                    rotation,
                }
                .validate(),
                Err(AssemblyError::NonRigidTransform)
            ));
        }
    }

    #[test]
    fn feature_euler_round_trip_preserves_rigid_matrix() {
        let source = AssemblyTransform::from_euler_xyz_degrees([1.0, 2.0, 3.0], [17.0, 31.0, 73.0]);
        let rebuilt = AssemblyTransform::from_euler_xyz_degrees(
            source.translation,
            source.euler_xyz_degrees(),
        );
        assert!(source.approximately_equals(rebuilt, 1.0e-12));
    }

    #[test]
    fn full_anchor_frames_remain_coincident() {
        let parent_frame =
            AssemblyTransform::from_euler_xyz_degrees([12.0, -3.0, 8.0], [20.0, 10.0, 70.0]);
        let child_frame =
            AssemblyTransform::from_euler_xyz_degrees([2.0, 4.0, -1.0], [-15.0, 35.0, 5.0]);
        let mate = AssemblyMate {
            id: 1,
            name: "fixed".into(),
            parent_occurrence_id: 1,
            child_occurrence_id: 2,
            parent_frame,
            child_frame,
            kind: AssemblyMateKind::Fixed,
            state: 0.0,
        };

        assert!(
            mate.local_transform()
                .compose(child_frame)
                .approximately_equals(parent_frame, 1.0e-10)
        );
    }

    #[test]
    fn revolute_motion_supports_an_arbitrary_unit_axis() {
        let inverse_sqrt_two = 0.5_f64.sqrt();
        let mate = AssemblyMate {
            id: 1,
            name: "diagonal hinge".into(),
            parent_occurrence_id: 1,
            child_occurrence_id: 2,
            parent_frame: AssemblyTransform::from_euler_xyz_degrees(
                [4.0, 5.0, 6.0],
                [10.0, 20.0, 30.0],
            ),
            child_frame: AssemblyTransform {
                translation: [2.0, -3.0, 7.0],
                ..AssemblyTransform::IDENTITY
            },
            kind: AssemblyMateKind::Revolute {
                axis: [inverse_sqrt_two, inverse_sqrt_two, 0.0],
                limits_deg: None,
            },
            state: 73.0,
        };
        mate.validate().unwrap();

        let solved_parent_anchor = mate.local_transform().compose(mate.child_frame);
        let expected_parent_anchor = mate.parent_frame.compose(mate.kind.motion(mate.state));
        assert!(solved_parent_anchor.approximately_equals(expected_parent_anchor, 1.0e-10));
        assert!(
            mate.local_transform()
                .transform_point(mate.child_frame.translation)
                .into_iter()
                .zip(mate.parent_frame.translation)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-10)
        );
    }

    #[test]
    fn nested_forward_kinematics_composes_revolute_and_slider_motion() {
        let assembly = kinematic_assembly();
        assembly.validate(&BTreeSet::new()).unwrap();
        let world = assembly.world_transforms().unwrap();

        let child = world[&2];
        let grandchild = world[&3];
        assert!(child.approximately_equals(
            world[&1].compose(assembly.mates[0].local_transform()),
            1.0e-10
        ));
        assert!(
            grandchild
                .approximately_equals(child.compose(assembly.mates[1].local_transform()), 1.0e-10)
        );

        let mut suppressed = assembly.clone();
        suppressed.occurrences[0].suppressed = true;
        assert_eq!(suppressed.world_transforms().unwrap(), world);
        assert!(
            suppressed
                .effective_suppression()
                .unwrap()
                .values()
                .all(|value| *value)
        );
    }

    #[test]
    fn mate_validation_rejects_invalid_axes_limits_and_duplicate_drivers() {
        let assembly = kinematic_assembly();

        let mut invalid_axis = assembly.clone();
        if let AssemblyMateKind::Revolute { axis, .. } = &mut invalid_axis.mates[0].kind {
            *axis = [2.0, 0.0, 0.0];
        }
        assert!(matches!(
            invalid_axis.validate(&BTreeSet::new()),
            Err(AssemblyError::InvalidMateAxis { mate: 1 })
        ));

        let mut outside_limits = assembly.clone();
        outside_limits.mates[1].state = 11.0;
        outside_limits.occurrences[2].transform = outside_limits.mates[1].local_transform();
        assert!(matches!(
            outside_limits.validate(&BTreeSet::new()),
            Err(AssemblyError::MateStateOutsideLimits { mate: 2 })
        ));

        let mut duplicate_driver = assembly.clone();
        let mut duplicate = duplicate_driver.mates[0].clone();
        duplicate.id = 3;
        duplicate_driver.mates.push(duplicate);
        assert!(matches!(
            duplicate_driver.validate(&BTreeSet::new()),
            Err(AssemblyError::OccurrenceDrivenMultipleTimes {
                occurrence: 2,
                first: 1,
                second: 3
            })
        ));
    }

    #[test]
    fn mate_validation_rejects_invalid_identity_hierarchy_frames_and_state() {
        let assembly = kinematic_assembly();

        let mut duplicate_id = assembly.clone();
        duplicate_id.mates[1].id = 1;
        assert!(matches!(
            duplicate_id.validate(&BTreeSet::new()),
            Err(AssemblyError::InvalidMateId(1))
        ));

        let mut root_driver = assembly.clone();
        root_driver.mates[0].child_occurrence_id = 1;
        root_driver.mates[0].parent_occurrence_id = 2;
        assert!(matches!(
            root_driver.validate(&BTreeSet::new()),
            Err(AssemblyError::MateDrivesRoot {
                mate: 1,
                occurrence: 1
            })
        ));

        let mut wrong_parent = assembly.clone();
        wrong_parent.mates[1].parent_occurrence_id = 1;
        assert!(matches!(
            wrong_parent.validate(&BTreeSet::new()),
            Err(AssemblyError::MateParentMismatch {
                mate: 2,
                child: 3,
                expected: 2,
                actual: 1
            })
        ));

        let mut invalid_frame = assembly.clone();
        invalid_frame.mates[0].parent_frame.rotation[0][0] = -1.0;
        assert!(matches!(
            invalid_frame.validate(&BTreeSet::new()),
            Err(AssemblyError::NonRigidTransform)
        ));

        let mut non_finite_state = assembly.clone();
        non_finite_state.mates[0].state = f64::NAN;
        assert!(matches!(
            non_finite_state.validate(&BTreeSet::new()),
            Err(AssemblyError::NonFiniteMateState { mate: 1 })
        ));

        let mut mismatched_pose = assembly;
        mismatched_pose.occurrences[1].transform.translation[0] += 1.0;
        assert!(matches!(
            mismatched_pose.validate(&BTreeSet::new()),
            Err(AssemblyError::MateTransformMismatch {
                mate: 1,
                occurrence: 2
            })
        ));
    }
}
