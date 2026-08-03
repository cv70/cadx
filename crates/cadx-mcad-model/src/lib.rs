//! Kernel-neutral MCAD feature-tree and assembly solving contracts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use thiserror::Error;

pub type McadFeatureId = u64;
pub type OccurrenceId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKind {
    Sketch,
    Extrude,
    Revolve,
    Loft,
    Boolean,
    Chamfer,
    Fillet,
    Pattern,
    Reference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McadFeature {
    pub id: McadFeatureId,
    pub name: String,
    pub kind: FeatureKind,
    #[serde(default)]
    pub dependencies: Vec<McadFeatureId>,
    #[serde(default)]
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FeatureTree {
    #[serde(default)]
    pub features: Vec<McadFeature>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FeatureTreeError {
    #[error("feature id {0} occurs more than once")]
    DuplicateFeature(McadFeatureId),
    #[error("feature {feature} depends on missing feature {dependency}")]
    MissingDependency {
        feature: McadFeatureId,
        dependency: McadFeatureId,
    },
    #[error("feature {0} depends on itself")]
    SelfDependency(McadFeatureId),
    #[error("feature dependency graph contains a cycle")]
    DependencyCycle,
    #[error("dirty feature {0} does not exist")]
    UnknownDirtyFeature(McadFeatureId),
}

impl FeatureTree {
    /// Validates ids and returns a stable topological order.
    ///
    /// # Errors
    ///
    /// Returns a dependency error when ids are duplicated, dependencies are
    /// missing, or the graph contains a cycle.
    pub fn topological_order(&self) -> Result<Vec<McadFeatureId>, FeatureTreeError> {
        let mut by_id = BTreeMap::new();
        for feature in &self.features {
            if by_id.insert(feature.id, feature).is_some() {
                return Err(FeatureTreeError::DuplicateFeature(feature.id));
            }
        }

        let mut indegree = BTreeMap::<McadFeatureId, usize>::new();
        let mut dependents = BTreeMap::<McadFeatureId, Vec<McadFeatureId>>::new();
        for feature in &self.features {
            indegree.entry(feature.id).or_default();
            let mut unique_dependencies = BTreeSet::new();
            for dependency in &feature.dependencies {
                if *dependency == feature.id {
                    return Err(FeatureTreeError::SelfDependency(feature.id));
                }
                if !by_id.contains_key(dependency) {
                    return Err(FeatureTreeError::MissingDependency {
                        feature: feature.id,
                        dependency: *dependency,
                    });
                }
                if unique_dependencies.insert(*dependency) {
                    *indegree.entry(feature.id).or_default() += 1;
                    dependents.entry(*dependency).or_default().push(feature.id);
                }
            }
        }

        for children in dependents.values_mut() {
            children.sort_unstable();
        }
        let mut ready = indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<BTreeSet<_>>();
        let mut order = Vec::with_capacity(self.features.len());
        while let Some(id) = ready.pop_first() {
            order.push(id);
            for dependent in dependents.get(&id).into_iter().flatten() {
                let Some(degree) = indegree.get_mut(dependent) else {
                    return Err(FeatureTreeError::DependencyCycle);
                };
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(*dependent);
                }
            }
        }
        if order.len() != self.features.len() {
            return Err(FeatureTreeError::DependencyCycle);
        }
        Ok(order)
    }

    /// Returns dirty features and all transitive dependents in rebuild order.
    /// Suppressed features remain in dependency ordering but are not rebuilt.
    ///
    /// # Errors
    ///
    /// Returns a graph validation error or an unknown dirty feature error.
    pub fn regeneration_plan(
        &self,
        dirty: impl IntoIterator<Item = McadFeatureId>,
    ) -> Result<Vec<McadFeatureId>, FeatureTreeError> {
        let order = self.topological_order()?;
        let by_id = self
            .features
            .iter()
            .map(|feature| (feature.id, feature))
            .collect::<BTreeMap<_, _>>();
        let mut dependents = BTreeMap::<McadFeatureId, Vec<McadFeatureId>>::new();
        for feature in &self.features {
            for dependency in &feature.dependencies {
                dependents.entry(*dependency).or_default().push(feature.id);
            }
        }
        let mut affected = BTreeSet::new();
        let mut queue = VecDeque::new();
        for id in dirty {
            if !by_id.contains_key(&id) {
                return Err(FeatureTreeError::UnknownDirtyFeature(id));
            }
            if affected.insert(id) {
                queue.push_back(id);
            }
        }
        while let Some(id) = queue.pop_front() {
            for dependent in dependents.get(&id).into_iter().flatten() {
                if affected.insert(*dependent) {
                    queue.push_back(*dependent);
                }
            }
        }
        Ok(order
            .into_iter()
            .filter(|id| affected.contains(id) && !by_id[id].suppressed)
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceFrame {
    pub occurrence_id: OccurrenceId,
    pub translation_mm: [f64; 3],
    pub rotation_deg: [f64; 3],
    #[serde(default)]
    pub grounded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MateKind {
    Fixed,
    Coincident,
    Concentric,
    Distance,
    Angle,
    Revolute,
    Slider,
}

impl MateKind {
    #[must_use]
    pub const fn constrained_degrees(self) -> u8 {
        match self {
            Self::Fixed => 6,
            Self::Coincident => 3,
            Self::Concentric => 4,
            Self::Distance | Self::Angle => 1,
            Self::Revolute | Self::Slider => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AssemblyMate {
    pub id: u64,
    pub first: OccurrenceId,
    pub second: OccurrenceId,
    pub kind: MateKind,
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceTransformProposal {
    pub occurrence_id: OccurrenceId,
    pub translation_mm: [f64; 3],
    pub rotation_deg: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssemblyIssue {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub mate_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AssemblySolveReport {
    pub proposals: Vec<OccurrenceTransformProposal>,
    pub remaining_degrees_of_freedom: BTreeMap<OccurrenceId, u8>,
    pub issues: Vec<AssemblyIssue>,
}

/// Produces a deterministic first-order assembly placement proposal.
///
/// The geometry kernel remains responsible for exact face/axis residuals. This
/// solver handles graph-level validity, limits, grounding, and coarse frames.
#[must_use]
pub fn solve_assembly(frames: &[OccurrenceFrame], mates: &[AssemblyMate]) -> AssemblySolveReport {
    let mut report = AssemblySolveReport::default();
    let mut working = frames
        .iter()
        .map(|frame| (frame.occurrence_id, *frame))
        .collect::<BTreeMap<_, _>>();
    let mut degrees = frames
        .iter()
        .map(|frame| {
            (
                frame.occurrence_id,
                if frame.grounded { 0_u8 } else { 6_u8 },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut mate_ids = BTreeSet::new();

    for mate in mates {
        if !mate_ids.insert(mate.id) {
            report.issues.push(AssemblyIssue {
                code: "DUPLICATE_MATE".into(),
                message: format!("Mate {} is declared more than once", mate.id),
                mate_id: Some(mate.id),
            });
            continue;
        }
        let (Some(first), Some(second)) = (working.get(&mate.first), working.get(&mate.second))
        else {
            report.issues.push(AssemblyIssue {
                code: "MISSING_OCCURRENCE".into(),
                message: format!("Mate {} references an unknown occurrence", mate.id),
                mate_id: Some(mate.id),
            });
            continue;
        };
        if mate.first == mate.second
            || !mate.offset.is_finite()
            || mate.minimum.is_some_and(|value| !value.is_finite())
            || mate.maximum.is_some_and(|value| !value.is_finite())
            || mate
                .minimum
                .zip(mate.maximum)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            report.issues.push(AssemblyIssue {
                code: "INVALID_MATE".into(),
                message: format!("Mate {} has invalid references, offset, or limits", mate.id),
                mate_id: Some(mate.id),
            });
            continue;
        }

        let mut proposal = *second;
        match mate.kind {
            MateKind::Fixed | MateKind::Coincident | MateKind::Concentric => {
                proposal.translation_mm = first.translation_mm;
                if mate.kind == MateKind::Fixed {
                    proposal.rotation_deg = first.rotation_deg;
                }
            }
            MateKind::Distance | MateKind::Slider => {
                proposal.translation_mm[0] = first.translation_mm[0] + mate.offset;
            }
            MateKind::Angle | MateKind::Revolute => {
                proposal.rotation_deg[2] = first.rotation_deg[2] + mate.offset;
            }
        }
        working.insert(mate.second, proposal);
        if let Some(remaining) = degrees.get_mut(&mate.second) {
            *remaining = (*remaining).saturating_sub(mate.kind.constrained_degrees());
        }
    }

    report.proposals = working
        .into_values()
        .map(|frame| OccurrenceTransformProposal {
            occurrence_id: frame.occurrence_id,
            translation_mm: frame.translation_mm,
            rotation_deg: frame.rotation_deg,
        })
        .collect();
    report.remaining_degrees_of_freedom = degrees;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regeneration_propagates_and_orders_dependents() {
        let tree = FeatureTree {
            features: vec![
                McadFeature {
                    id: 3,
                    name: "fillet".into(),
                    kind: FeatureKind::Fillet,
                    dependencies: vec![2],
                    suppressed: false,
                },
                McadFeature {
                    id: 1,
                    name: "sketch".into(),
                    kind: FeatureKind::Sketch,
                    dependencies: vec![],
                    suppressed: false,
                },
                McadFeature {
                    id: 2,
                    name: "pad".into(),
                    kind: FeatureKind::Extrude,
                    dependencies: vec![1],
                    suppressed: false,
                },
            ],
        };
        assert_eq!(tree.regeneration_plan([1]).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn feature_cycle_is_rejected() {
        let tree = FeatureTree {
            features: vec![
                McadFeature {
                    id: 1,
                    name: "one".into(),
                    kind: FeatureKind::Extrude,
                    dependencies: vec![2],
                    suppressed: false,
                },
                McadFeature {
                    id: 2,
                    name: "two".into(),
                    kind: FeatureKind::Fillet,
                    dependencies: vec![1],
                    suppressed: false,
                },
            ],
        };
        assert_eq!(
            tree.topological_order(),
            Err(FeatureTreeError::DependencyCycle)
        );
    }

    #[test]
    fn fixed_mate_copies_the_reference_frame() {
        let report = solve_assembly(
            &[
                OccurrenceFrame {
                    occurrence_id: 1,
                    translation_mm: [4.0, 5.0, 6.0],
                    rotation_deg: [0.0, 0.0, 30.0],
                    grounded: true,
                },
                OccurrenceFrame {
                    occurrence_id: 2,
                    translation_mm: [0.0; 3],
                    rotation_deg: [0.0; 3],
                    grounded: false,
                },
            ],
            &[AssemblyMate {
                id: 1,
                first: 1,
                second: 2,
                kind: MateKind::Fixed,
                offset: 0.0,
                minimum: None,
                maximum: None,
            }],
        );
        assert!(
            report.proposals[1]
                .translation_mm
                .iter()
                .zip([4.0, 5.0, 6.0])
                .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
        );
        assert_eq!(report.remaining_degrees_of_freedom[&2], 0);
    }
}
