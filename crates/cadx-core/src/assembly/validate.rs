//! Load-time proofs for hierarchy, ownership, drivers, and world placement.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::FeatureId;

use super::{
    Assembly, AssemblyError, AssemblyMate, AssemblyTransform, ComponentKind, ComponentOccurrenceId,
    MAX_ASSEMBLY_MATES, MAX_COMPONENT_DEFINITIONS, MAX_COMPONENT_OCCURRENCES,
    MAX_OCCURRENCE_FEATURES, validate_name,
};

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

#[cfg(test)]
mod tests {
    use crate::assembly::{
        Assembly, AssemblyError, AssemblyMate, AssemblyMateKind, AssemblyMateLimits,
        AssemblyTransform, ComponentDefinition, ComponentDefinitionId, ComponentKind,
        ComponentOccurrence, ComponentOccurrenceId,
    };
    use std::collections::BTreeSet;

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
