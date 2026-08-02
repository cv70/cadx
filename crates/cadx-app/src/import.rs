use std::collections::BTreeMap;

use cadx_core::{
    assembly::{AssemblyTransform, ComponentOccurrence},
    domain::{CadDocument, FeatureId, ModelCommand},
};
use cadx_io::{StepImport, StepImportBody};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct StepImportPlan {
    pub commands: Vec<ModelCommand>,
    pub imported_features: Vec<FeatureId>,
    pub unsupported_color_count: usize,
}

/// Builds one atomic command plan from parsed STEP bodies and occurrences.
///
/// # Errors
///
/// Returns [`StepImportPlanError`] when assembly references are inconsistent
/// or the document feature-id space cannot hold every materialized occurrence.
pub fn plan_step_import(
    document: &CadDocument,
    import: StepImport,
    fallback_name: &str,
) -> Result<StepImportPlan, StepImportPlanError> {
    let StepImport {
        source,
        bodies,
        assemblies,
        standalone_body_indices,
    } = import;
    let mut commands = Vec::new();
    let mut imported_features = Vec::new();
    let mut unsupported_color_count = 0;
    let mut next_feature_id = document.next_feature_id();

    for (position, body_index) in standalone_body_indices.into_iter().enumerate() {
        let body = bodies
            .get(body_index)
            .ok_or(StepImportPlanError::MissingBody(body_index))?;
        let name = body
            .name
            .clone()
            .unwrap_or_else(|| format!("{fallback_name} body {}", position + 1));
        materialize_body(
            &mut commands,
            &mut imported_features,
            &mut unsupported_color_count,
            &mut next_feature_id,
            &source,
            body,
            &name,
            AssemblyTransform::IDENTITY,
        )?;
    }

    for assembly in assemblies {
        let world_transforms = resolve_world_transforms(&assembly.occurrences)?;
        let mut occurrences = Vec::with_capacity(assembly.occurrences.len());
        for occurrence in assembly.occurrences {
            let world = world_transforms[&occurrence.id];
            let mut feature_ids = Vec::with_capacity(occurrence.body_indices.len());
            for (body_position, body_index) in occurrence.body_indices.iter().enumerate() {
                let body = bodies
                    .get(*body_index)
                    .ok_or(StepImportPlanError::MissingBody(*body_index))?;
                let name = if occurrence.body_indices.len() == 1 {
                    occurrence.name.clone()
                } else {
                    let body_name = body
                        .name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map_or_else(|| format!("body {}", body_position + 1), str::to_owned);
                    format!("{} / {body_name}", occurrence.name)
                };
                let feature_id = materialize_body(
                    &mut commands,
                    &mut imported_features,
                    &mut unsupported_color_count,
                    &mut next_feature_id,
                    &source,
                    body,
                    &name,
                    world,
                )?;
                feature_ids.push(feature_id);
            }
            occurrences.push(ComponentOccurrence {
                id: occurrence.id,
                name: occurrence.name,
                definition_id: occurrence.definition_id,
                parent_id: occurrence.parent_id,
                suppressed: false,
                transform: occurrence.transform,
                feature_ids,
                source: Some(occurrence.source),
            });
        }
        commands.push(ModelCommand::CreateAssembly {
            name: assembly.name,
            definitions: assembly.definitions,
            occurrences,
        });
    }

    if imported_features.is_empty() {
        return Err(StepImportPlanError::NoBodies);
    }
    Ok(StepImportPlan {
        commands,
        imported_features,
        unsupported_color_count,
    })
}

#[allow(clippy::too_many_arguments)]
fn materialize_body(
    commands: &mut Vec<ModelCommand>,
    imported_features: &mut Vec<FeatureId>,
    unsupported_color_count: &mut usize,
    next_feature_id: &mut FeatureId,
    source: &str,
    body: &StepImportBody,
    name: &str,
    transform: AssemblyTransform,
) -> Result<FeatureId, StepImportPlanError> {
    let feature_id = *next_feature_id;
    *next_feature_id = next_feature_id
        .checked_add(1)
        .ok_or(StepImportPlanError::FeatureIdOverflow)?;
    commands.push(ModelCommand::ImportStep {
        name: name.into(),
        source: source.into(),
        data_section: body.data_section,
        shell_id: body.shell_id,
        void_shells: body.void_shells.clone(),
        length_unit: body.length_unit.clone(),
        color: body.color.color(),
        position: transform.translation,
    });
    let rotation = transform.euler_xyz_degrees();
    if rotation.into_iter().any(|angle| angle.abs() > 1.0e-10) {
        commands.push(ModelCommand::Rotate {
            id: feature_id,
            rotation,
        });
    }
    *unsupported_color_count += usize::from(body.color.is_unsupported());
    imported_features.push(feature_id);
    Ok(feature_id)
}

fn resolve_world_transforms(
    occurrences: &[cadx_io::StepImportOccurrence],
) -> Result<BTreeMap<u64, AssemblyTransform>, StepImportPlanError> {
    let mut world = BTreeMap::<u64, AssemblyTransform>::new();
    let mut unresolved = occurrences.iter().collect::<Vec<_>>();
    while !unresolved.is_empty() {
        let previous_len = unresolved.len();
        unresolved.retain(|occurrence| {
            let transform = match occurrence.parent_id {
                None => Some(occurrence.transform),
                Some(parent) => world
                    .get(&parent)
                    .copied()
                    .map(|parent_transform| parent_transform.compose(occurrence.transform)),
            };
            let Some(transform) = transform else {
                return true;
            };
            if world.insert(occurrence.id, transform).is_some() {
                return true;
            }
            false
        });
        if unresolved.len() == previous_len {
            return Err(StepImportPlanError::InvalidOccurrenceHierarchy);
        }
    }
    Ok(world)
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StepImportPlanError {
    #[error("STEP import references missing body index {0}")]
    MissingBody(usize),
    #[error("STEP assembly occurrence hierarchy is cyclic, duplicated, or has a missing parent")]
    InvalidOccurrenceHierarchy,
    #[error("document feature id space is exhausted while expanding STEP occurrences")]
    FeatureIdOverflow,
    #[error("STEP document contains no materializable body occurrences")]
    NoBodies,
}

#[cfg(test)]
mod tests {
    use cadx_core::assembly::{ComponentDefinition, ComponentKind, StepEntityRef};
    use cadx_core::domain::{Primitive, StepLengthUnit};
    use cadx_io::{StepBodyColor, StepImportAssembly, StepImportOccurrence};

    use super::*;

    #[test]
    fn nested_occurrences_materialize_world_transforms_and_owned_features() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::CreateBox {
                name: "existing".into(),
                size: [1.0; 3],
                position: [0.0; 3],
            })
            .unwrap();
        let root_transform = AssemblyTransform {
            translation: [10.0, 0.0, 0.0],
            rotation: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let source_ref = |entity_id| StepEntityRef {
            data_section: 0,
            entity_id,
        };
        let import = StepImport {
            source: "embedded STEP".into(),
            bodies: vec![StepImportBody {
                data_section: 0,
                shell_id: 42,
                void_shells: Vec::new(),
                name: Some("pin".into()),
                length_unit: StepLengthUnit::millimeter(),
                color: StepBodyColor::Uniform([0.2, 0.4, 0.8, 1.0]),
            }],
            assemblies: vec![StepImportAssembly {
                name: "fixture".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "fixture".into(),
                        kind: ComponentKind::Assembly,
                        source: Some(source_ref(10)),
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "pin".into(),
                        kind: ComponentKind::Part,
                        source: Some(source_ref(11)),
                    },
                ],
                occurrences: vec![
                    StepImportOccurrence {
                        id: 1,
                        name: "fixture".into(),
                        definition_id: 1,
                        parent_id: None,
                        transform: root_transform,
                        body_indices: Vec::new(),
                        source: source_ref(10),
                    },
                    StepImportOccurrence {
                        id: 2,
                        name: "pin:1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        transform: AssemblyTransform {
                            translation: [2.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        body_indices: vec![0],
                        source: source_ref(20),
                    },
                ],
            }],
            standalone_body_indices: Vec::new(),
        };

        let plan = plan_step_import(&document, import, "fixture").unwrap();
        assert_eq!(plan.imported_features, vec![2]);
        document.apply_transaction(plan.commands).unwrap();
        let feature = document.feature(2).unwrap();
        assert!(
            feature
                .translation
                .as_array()
                .into_iter()
                .zip([10.0, 2.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        assert!((feature.rotation.z - 90.0).abs() < 1.0e-9);
        assert!(matches!(feature.primitive, Primitive::ImportedStep { .. }));
        assert_eq!(document.assemblies[0].occurrences[1].feature_ids, vec![2]);
    }
}
