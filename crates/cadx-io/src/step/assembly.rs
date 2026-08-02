use std::collections::{BTreeMap, BTreeSet};

use cadx_core::assembly::{
    AssemblyTransform, ComponentDefinition, ComponentDefinitionId, ComponentKind,
    ComponentOccurrenceId, MAX_COMPONENT_DEFINITIONS, MAX_COMPONENT_OCCURRENCES, StepEntityRef,
};
use ruststep::ast::{EntityInstance, Parameter, Record};

use crate::ExportError;

use super::{
    body::StepBodyDefinition, collect_entity_refs, entity_by_id, entity_id, entity_records,
    parameter_list, parameter_ref,
};

#[derive(Debug, Clone, PartialEq)]
pub struct StepImportAssembly {
    pub name: String,
    pub definitions: Vec<ComponentDefinition>,
    pub occurrences: Vec<StepImportOccurrence>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepImportOccurrence {
    pub id: ComponentOccurrenceId,
    pub name: String,
    pub definition_id: ComponentDefinitionId,
    pub parent_id: Option<ComponentOccurrenceId>,
    pub transform: AssemblyTransform,
    pub body_indices: Vec<usize>,
    pub source: StepEntityRef,
}

#[derive(Debug, Clone)]
struct Usage {
    source_id: u64,
    name: String,
    parent_definition: u64,
    child_definition: u64,
    transform: AssemblyTransform,
}

#[derive(Debug, Clone, Copy)]
struct Relationship {
    representation_1: u64,
    representation_2: u64,
    transform_id: Option<u64>,
}

pub(super) fn discover_assembly(
    entities: &[EntityInstance],
    data_section: usize,
    bodies: &[StepBodyDefinition],
    millimeters_per_unit: f64,
) -> Result<Option<StepImportAssembly>, ExportError> {
    let raw_usages = raw_usages(entities)?;
    if raw_usages.is_empty() {
        return Ok(None);
    }

    let product_names = product_names(entities);
    let formation_products = formation_products(entities);
    let definition_names = definition_names(entities, &formation_products, &product_names);
    let definition_representations = definition_representations(entities)?;
    let representation_bodies = representation_bodies(entities, bodies)?;
    let definition_bodies = map_definition_bodies(
        &definition_representations,
        &representation_bodies,
        bodies.len(),
    )?;
    let relationships = relationships(entities)?;
    let usage_relationships = usage_relationships(entities, &relationships)?;

    let mut usages = Vec::with_capacity(raw_usages.len());
    for (source_id, name, parent_definition, child_definition) in raw_usages {
        if !definition_names.contains_key(&parent_definition)
            || !definition_names.contains_key(&child_definition)
        {
            return Err(invalid(format!(
                "assembly occurrence #{source_id} references a missing product definition"
            )));
        }
        let relationship = resolve_usage_relationship(
            source_id,
            parent_definition,
            child_definition,
            &definition_representations,
            &relationships,
            &usage_relationships,
        )?;
        let transform = match relationship.transform_id {
            Some(transform_id) => {
                item_defined_transform(entities, transform_id, millimeters_per_unit)?
            }
            None => AssemblyTransform::IDENTITY,
        };
        usages.push(Usage {
            source_id,
            name,
            parent_definition,
            child_definition,
            transform,
        });
    }

    let mut definition_ids = BTreeSet::new();
    let mut child_definitions = BTreeSet::new();
    let mut outgoing = BTreeMap::<u64, Vec<usize>>::new();
    for (index, usage) in usages.iter().enumerate() {
        definition_ids.insert(usage.parent_definition);
        definition_ids.insert(usage.child_definition);
        child_definitions.insert(usage.child_definition);
        outgoing
            .entry(usage.parent_definition)
            .or_default()
            .push(index);
    }
    if definition_ids.len() > MAX_COMPONENT_DEFINITIONS {
        return Err(invalid(
            "assembly exceeds CADX's component-definition limit",
        ));
    }
    let roots = definition_ids
        .iter()
        .copied()
        .filter(|definition| !child_definitions.contains(definition))
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err(invalid(
            "assembly product-definition graph contains a cycle",
        ));
    }

    let local_definition_ids = definition_ids
        .iter()
        .enumerate()
        .map(|(index, source_id)| (*source_id, index as u64 + 1))
        .collect::<BTreeMap<_, _>>();
    let definitions = definition_ids
        .iter()
        .map(|source_id| ComponentDefinition {
            id: local_definition_ids[source_id],
            name: definition_names[source_id].clone(),
            kind: if outgoing.contains_key(source_id) {
                ComponentKind::Assembly
            } else {
                ComponentKind::Part
            },
            source: Some(StepEntityRef {
                data_section,
                entity_id: *source_id,
            }),
        })
        .collect::<Vec<_>>();

    let mut occurrences = Vec::new();
    for root in roots {
        expand_occurrence(
            root,
            None,
            None,
            &definition_names,
            &local_definition_ids,
            &definition_bodies,
            &outgoing,
            &usages,
            data_section,
            &mut BTreeSet::new(),
            &mut occurrences,
        )?;
    }
    if occurrences.len() > MAX_COMPONENT_OCCURRENCES {
        return Err(invalid(
            "assembly exceeds CADX's component-occurrence limit",
        ));
    }
    let reachable_definitions = occurrences
        .iter()
        .map(|occurrence| occurrence.definition_id)
        .collect::<BTreeSet<_>>();
    if reachable_definitions.len() != definitions.len() {
        return Err(invalid(
            "assembly product-definition graph contains an unreachable cycle",
        ));
    }
    let name = if occurrences
        .iter()
        .filter(|occurrence| occurrence.parent_id.is_none())
        .count()
        == 1
    {
        occurrences
            .iter()
            .find(|occurrence| occurrence.parent_id.is_none())
            .expect("one root occurrence exists")
            .name
            .clone()
    } else {
        "STEP assembly".into()
    };
    Ok(Some(StepImportAssembly {
        name,
        definitions,
        occurrences,
    }))
}

#[allow(clippy::too_many_arguments)]
fn expand_occurrence(
    definition: u64,
    parent_id: Option<ComponentOccurrenceId>,
    usage: Option<&Usage>,
    names: &BTreeMap<u64, String>,
    definition_ids: &BTreeMap<u64, ComponentDefinitionId>,
    bodies: &BTreeMap<u64, Vec<usize>>,
    outgoing: &BTreeMap<u64, Vec<usize>>,
    usages: &[Usage],
    data_section: usize,
    path: &mut BTreeSet<u64>,
    occurrences: &mut Vec<StepImportOccurrence>,
) -> Result<(), ExportError> {
    if !path.insert(definition) {
        return Err(invalid(format!(
            "assembly product-definition graph contains a cycle at #{definition}"
        )));
    }
    if occurrences.len() >= MAX_COMPONENT_OCCURRENCES {
        return Err(invalid(
            "assembly exceeds CADX's component-occurrence limit",
        ));
    }
    let id = occurrences.len() as u64 + 1;
    let source_id = usage.map_or(definition, |usage| usage.source_id);
    let name = usage
        .map(|usage| usage.name.trim())
        .filter(|name| !name.is_empty())
        .map_or_else(|| names[&definition].clone(), str::to_owned);
    occurrences.push(StepImportOccurrence {
        id,
        name,
        definition_id: definition_ids[&definition],
        parent_id,
        transform: usage.map_or(AssemblyTransform::IDENTITY, |usage| usage.transform),
        body_indices: bodies.get(&definition).cloned().unwrap_or_default(),
        source: StepEntityRef {
            data_section,
            entity_id: source_id,
        },
    });
    if let Some(children) = outgoing.get(&definition) {
        for child in children {
            let usage = &usages[*child];
            expand_occurrence(
                usage.child_definition,
                Some(id),
                Some(usage),
                names,
                definition_ids,
                bodies,
                outgoing,
                usages,
                data_section,
                path,
                occurrences,
            )?;
        }
    }
    path.remove(&definition);
    Ok(())
}

fn raw_usages(entities: &[EntityInstance]) -> Result<Vec<(u64, String, u64, u64)>, ExportError> {
    let mut usages = Vec::new();
    for entity in entities {
        let Some(record) = unique_record(entity, "NEXT_ASSEMBLY_USAGE_OCCURRENCE")? else {
            continue;
        };
        let values = parameter_list(&record.parameter).ok_or_else(|| {
            invalid(format!(
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE #{} has invalid parameters",
                entity_id(entity)
            ))
        })?;
        if values.len() < 5 {
            return Err(invalid(format!(
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE #{} is incomplete",
                entity_id(entity)
            )));
        }
        let parent = values.get(3).and_then(parameter_ref).ok_or_else(|| {
            invalid(format!(
                "assembly occurrence #{} has no parent product definition",
                entity_id(entity)
            ))
        })?;
        let child = values.get(4).and_then(parameter_ref).ok_or_else(|| {
            invalid(format!(
                "assembly occurrence #{} has no child product definition",
                entity_id(entity)
            ))
        })?;
        usages.push((
            entity_id(entity),
            string_parameter(values.get(1)).unwrap_or_default(),
            parent,
            child,
        ));
    }
    Ok(usages)
}

fn product_names(entities: &[EntityInstance]) -> BTreeMap<u64, String> {
    entities
        .iter()
        .filter_map(|entity| {
            let record = entity_records(entity)
                .into_iter()
                .find(|record| record.name == "PRODUCT")?;
            let values = parameter_list(&record.parameter)?;
            let name = string_parameter(values.get(1))
                .filter(|name| !name.trim().is_empty())
                .or_else(|| string_parameter(values.first()))?;
            Some((entity_id(entity), name.trim().to_owned()))
        })
        .collect()
}

fn formation_products(entities: &[EntityInstance]) -> BTreeMap<u64, u64> {
    entities
        .iter()
        .filter_map(|entity| {
            let records = entity_records(entity);
            records
                .iter()
                .any(|record| record.name.starts_with("PRODUCT_DEFINITION_FORMATION"))
                .then(|| {
                    records
                        .iter()
                        .flat_map(|record| referenced_entities(&record.parameter))
                        .find(|reference| has_record(entities, *reference, "PRODUCT"))
                        .map(|product| (entity_id(entity), product))
                })
                .flatten()
        })
        .collect()
}

fn definition_names(
    entities: &[EntityInstance],
    formation_products: &BTreeMap<u64, u64>,
    product_names: &BTreeMap<u64, String>,
) -> BTreeMap<u64, String> {
    entities
        .iter()
        .filter_map(|entity| {
            let record = entity_records(entity)
                .into_iter()
                .find(|record| record.name == "PRODUCT_DEFINITION")?;
            let formation = referenced_entities(&record.parameter)
                .into_iter()
                .find(|reference| formation_products.contains_key(reference));
            let name = formation
                .and_then(|formation| formation_products.get(&formation))
                .and_then(|product| product_names.get(product))
                .cloned()
                .unwrap_or_else(|| format!("Product definition #{}", entity_id(entity)));
            Some((entity_id(entity), name))
        })
        .collect()
}

fn definition_representations(
    entities: &[EntityInstance],
) -> Result<BTreeMap<u64, Vec<u64>>, ExportError> {
    let product_shapes = entities
        .iter()
        .filter_map(|entity| {
            let record = entity_records(entity)
                .into_iter()
                .find(|record| record.name == "PRODUCT_DEFINITION_SHAPE")?;
            let target = referenced_entities(&record.parameter).into_iter().last()?;
            has_record(entities, target, "PRODUCT_DEFINITION")
                .then_some((entity_id(entity), target))
        })
        .collect::<BTreeMap<_, _>>();
    let mut definitions = BTreeMap::<u64, Vec<u64>>::new();
    for entity in entities {
        let Some(record) = unique_record(entity, "SHAPE_DEFINITION_REPRESENTATION")? else {
            continue;
        };
        let values = parameter_list(&record.parameter).ok_or_else(|| {
            invalid(format!(
                "SHAPE_DEFINITION_REPRESENTATION #{} has invalid parameters",
                entity_id(entity)
            ))
        })?;
        let Some(shape_id) = values.first().and_then(parameter_ref) else {
            continue;
        };
        let Some(definition) = product_shapes.get(&shape_id) else {
            continue;
        };
        let representation = values.get(1).and_then(parameter_ref).ok_or_else(|| {
            invalid(format!(
                "SHAPE_DEFINITION_REPRESENTATION #{} has no representation",
                entity_id(entity)
            ))
        })?;
        definitions
            .entry(*definition)
            .or_default()
            .push(representation);
    }
    Ok(definitions)
}

fn representation_bodies(
    entities: &[EntityInstance],
    bodies: &[StepBodyDefinition],
) -> Result<BTreeMap<u64, Vec<usize>>, ExportError> {
    let body_by_entity = bodies
        .iter()
        .enumerate()
        .filter_map(|(index, body)| body.source_entity_id.map(|id| (id, index)))
        .collect::<BTreeMap<_, _>>();
    let mut representations = BTreeMap::new();
    for entity in entities {
        let records = entity_records(entity);
        let Some(record) = records
            .iter()
            .copied()
            .find(|record| is_shape_representation(&record.name))
        else {
            continue;
        };
        let values = parameter_list(&record.parameter).ok_or_else(|| {
            invalid(format!(
                "shape representation #{} has invalid parameters",
                entity_id(entity)
            ))
        })?;
        let body_indices = values
            .get(1)
            .map(referenced_entities)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|reference| body_by_entity.get(&reference).copied())
            .collect::<Vec<_>>();
        representations.insert(entity_id(entity), body_indices);
    }
    Ok(representations)
}

fn is_shape_representation(name: &str) -> bool {
    matches!(
        name,
        "SHAPE_REPRESENTATION"
            | "ADVANCED_BREP_SHAPE_REPRESENTATION"
            | "FACETED_BREP_SHAPE_REPRESENTATION"
            | "MANIFOLD_SURFACE_SHAPE_REPRESENTATION"
    )
}

fn map_definition_bodies(
    definition_representations: &BTreeMap<u64, Vec<u64>>,
    representation_bodies: &BTreeMap<u64, Vec<usize>>,
    body_count: usize,
) -> Result<BTreeMap<u64, Vec<usize>>, ExportError> {
    let mut definitions = BTreeMap::new();
    let mut owners = vec![None; body_count];
    for (definition, representations) in definition_representations {
        let mut body_indices = representations
            .iter()
            .flat_map(|representation| {
                representation_bodies
                    .get(representation)
                    .into_iter()
                    .flatten()
                    .copied()
            })
            .collect::<Vec<_>>();
        body_indices.sort_unstable();
        body_indices.dedup();
        for body in &body_indices {
            if let Some(owner) = owners[*body].replace(*definition)
                && owner != *definition
            {
                return Err(invalid(format!(
                    "STEP body index {body} belongs to product definitions #{owner} and #{definition}"
                )));
            }
        }
        definitions.insert(*definition, body_indices);
    }
    Ok(definitions)
}

fn relationships(entities: &[EntityInstance]) -> Result<BTreeMap<u64, Relationship>, ExportError> {
    let mut relationships = BTreeMap::new();
    for entity in entities {
        let records = entity_records(entity);
        let with_transform = records
            .iter()
            .copied()
            .find(|record| record.name == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION");
        let shape_relationship = records
            .iter()
            .copied()
            .find(|record| record.name == "SHAPE_REPRESENTATION_RELATIONSHIP");
        if with_transform.is_none() && shape_relationship.is_none() {
            continue;
        }
        let base = records
            .iter()
            .copied()
            .find(|record| record.name == "REPRESENTATION_RELATIONSHIP");
        let (representation_1, representation_2, transform_id) = if let Some(base) = base {
            let values = parameter_list(&base.parameter).ok_or_else(|| {
                invalid(format!(
                    "representation relationship #{} has invalid parameters",
                    entity_id(entity)
                ))
            })?;
            let first = values.get(2).and_then(parameter_ref).ok_or_else(|| {
                invalid(format!(
                    "representation relationship #{} has no first representation",
                    entity_id(entity)
                ))
            })?;
            let second = values.get(3).and_then(parameter_ref).ok_or_else(|| {
                invalid(format!(
                    "representation relationship #{} has no second representation",
                    entity_id(entity)
                ))
            })?;
            let transform = with_transform
                .and_then(|record| referenced_entities(&record.parameter).into_iter().next());
            (first, second, transform)
        } else {
            let record = with_transform
                .or(shape_relationship)
                .expect("record exists");
            let values = parameter_list(&record.parameter).ok_or_else(|| {
                invalid(format!(
                    "representation relationship #{} has invalid parameters",
                    entity_id(entity)
                ))
            })?;
            let first = values.get(2).and_then(parameter_ref).ok_or_else(|| {
                invalid(format!(
                    "representation relationship #{} has no first representation",
                    entity_id(entity)
                ))
            })?;
            let second = values.get(3).and_then(parameter_ref).ok_or_else(|| {
                invalid(format!(
                    "representation relationship #{} has no second representation",
                    entity_id(entity)
                ))
            })?;
            let transform = with_transform.and_then(|_| values.get(4).and_then(parameter_ref));
            (first, second, transform)
        };
        if with_transform.is_some() && transform_id.is_none() {
            return Err(invalid(format!(
                "representation relationship #{} has no transformation operator",
                entity_id(entity)
            )));
        }
        relationships.insert(
            entity_id(entity),
            Relationship {
                representation_1,
                representation_2,
                transform_id,
            },
        );
    }
    Ok(relationships)
}

fn usage_relationships(
    entities: &[EntityInstance],
    relationships: &BTreeMap<u64, Relationship>,
) -> Result<BTreeMap<u64, u64>, ExportError> {
    let product_shapes = entities
        .iter()
        .filter_map(|entity| {
            let record = entity_records(entity)
                .into_iter()
                .find(|record| record.name == "PRODUCT_DEFINITION_SHAPE")?;
            let target = referenced_entities(&record.parameter).into_iter().last()?;
            has_record(entities, target, "NEXT_ASSEMBLY_USAGE_OCCURRENCE")
                .then_some((entity_id(entity), target))
        })
        .collect::<BTreeMap<_, _>>();
    let mut result = BTreeMap::new();
    for entity in entities {
        let Some(record) = unique_record(entity, "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")? else {
            continue;
        };
        let refs = referenced_entities(&record.parameter);
        let relationship = refs
            .iter()
            .find(|reference| relationships.contains_key(reference));
        let occurrence = refs
            .iter()
            .find_map(|reference| product_shapes.get(reference));
        let (Some(relationship), Some(occurrence)) = (relationship, occurrence) else {
            return Err(invalid(format!(
                "context-dependent shape representation #{} is incomplete",
                entity_id(entity)
            )));
        };
        if result.insert(*occurrence, *relationship).is_some() {
            return Err(invalid(format!(
                "assembly occurrence #{occurrence} has multiple transformation relationships"
            )));
        }
    }
    Ok(result)
}

fn resolve_usage_relationship(
    usage_id: u64,
    parent_definition: u64,
    child_definition: u64,
    definition_representations: &BTreeMap<u64, Vec<u64>>,
    relationships: &BTreeMap<u64, Relationship>,
    usage_relationships: &BTreeMap<u64, u64>,
) -> Result<Relationship, ExportError> {
    if let Some(relationship) = usage_relationships.get(&usage_id) {
        let relationship = relationships.get(relationship).copied().ok_or_else(|| {
            invalid(format!(
                "assembly occurrence #{usage_id} references missing relationship #{relationship}"
            ))
        })?;
        let parent_representations = definition_representations
            .get(&parent_definition)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let child_representations = definition_representations
            .get(&child_definition)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if !parent_representations.contains(&relationship.representation_1)
            || !child_representations.contains(&relationship.representation_2)
        {
            return Err(invalid(format!(
                "assembly occurrence #{usage_id} transformation does not relate its parent and child representations"
            )));
        }
        return Ok(relationship);
    }
    let parent_representations = definition_representations
        .get(&parent_definition)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let child_representations = definition_representations
        .get(&child_definition)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let candidates = relationships
        .values()
        .filter(|relationship| {
            parent_representations.contains(&relationship.representation_1)
                && child_representations.contains(&relationship.representation_2)
        })
        .copied()
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [relationship] => Ok(*relationship),
        [] => Err(invalid(format!(
            "assembly occurrence #{usage_id} has no explicit representation relationship"
        ))),
        _ => Err(invalid(format!(
            "assembly occurrence #{usage_id} has ambiguous representation relationships"
        ))),
    }
}

fn item_defined_transform(
    entities: &[EntityInstance],
    transform_id: u64,
    millimeters_per_unit: f64,
) -> Result<AssemblyTransform, ExportError> {
    let entity = entity_by_id(entities, transform_id).ok_or_else(|| {
        invalid(format!(
            "assembly transformation references missing entity #{transform_id}"
        ))
    })?;
    let record = unique_record(entity, "ITEM_DEFINED_TRANSFORMATION")?.ok_or_else(|| {
        invalid(format!(
            "assembly transformation #{transform_id} is not ITEM_DEFINED_TRANSFORMATION"
        ))
    })?;
    let values = parameter_list(&record.parameter).ok_or_else(|| {
        invalid(format!(
            "ITEM_DEFINED_TRANSFORMATION #{transform_id} has invalid parameters"
        ))
    })?;
    let placements = values.iter().filter_map(parameter_ref).collect::<Vec<_>>();
    let [parent_id, child_id] = placements.as_slice() else {
        return Err(invalid(format!(
            "ITEM_DEFINED_TRANSFORMATION #{transform_id} requires two placement items"
        )));
    };
    let parent = axis_placement(entities, *parent_id, millimeters_per_unit)?;
    let child = axis_placement(entities, *child_id, millimeters_per_unit)?;
    Ok(parent.compose(child.inverse()))
}

fn axis_placement(
    entities: &[EntityInstance],
    placement_id: u64,
    millimeters_per_unit: f64,
) -> Result<AssemblyTransform, ExportError> {
    let entity = entity_by_id(entities, placement_id).ok_or_else(|| {
        invalid(format!(
            "assembly placement references missing entity #{placement_id}"
        ))
    })?;
    let record = unique_record(entity, "AXIS2_PLACEMENT_3D")?.ok_or_else(|| {
        invalid(format!(
            "assembly placement #{placement_id} is not AXIS2_PLACEMENT_3D"
        ))
    })?;
    let values = parameter_list(&record.parameter).ok_or_else(|| {
        invalid(format!(
            "AXIS2_PLACEMENT_3D #{placement_id} has invalid parameters"
        ))
    })?;
    let location_id = values.get(1).and_then(parameter_ref).ok_or_else(|| {
        invalid(format!(
            "AXIS2_PLACEMENT_3D #{placement_id} has no location"
        ))
    })?;
    let translation =
        point_coordinates(entities, location_id)?.map(|value| value * millimeters_per_unit);
    let z = match values.get(2).and_then(parameter_ref) {
        Some(direction) => direction_ratios(entities, direction)?,
        None => [0.0, 0.0, 1.0],
    };
    let reference_x = match values.get(3).and_then(parameter_ref) {
        Some(direction) => direction_ratios(entities, direction)?,
        None => [1.0, 0.0, 0.0],
    };
    let x_projection = dot(reference_x, z);
    let x = normalize(std::array::from_fn(|axis| {
        reference_x[axis] - x_projection * z[axis]
    }))
    .ok_or_else(|| {
        invalid(format!(
            "AXIS2_PLACEMENT_3D #{placement_id} has parallel axis directions"
        ))
    })?;
    let y = cross(z, x);
    Ok(AssemblyTransform {
        translation,
        rotation: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
    })
}

fn point_coordinates(entities: &[EntityInstance], point_id: u64) -> Result<[f64; 3], ExportError> {
    vector_values(entities, point_id, "CARTESIAN_POINT")
}

fn direction_ratios(
    entities: &[EntityInstance],
    direction_id: u64,
) -> Result<[f64; 3], ExportError> {
    normalize(vector_values(entities, direction_id, "DIRECTION")?).ok_or_else(|| {
        invalid(format!(
            "assembly direction #{direction_id} has zero magnitude"
        ))
    })
}

fn vector_values(
    entities: &[EntityInstance],
    id: u64,
    record_name: &str,
) -> Result<[f64; 3], ExportError> {
    let entity = entity_by_id(entities, id)
        .ok_or_else(|| invalid(format!("assembly references missing {record_name} #{id}")))?;
    let record = unique_record(entity, record_name)?
        .ok_or_else(|| invalid(format!("assembly entity #{id} is not {record_name}")))?;
    let values = parameter_list(&record.parameter)
        .and_then(|values| values.get(1))
        .and_then(parameter_list)
        .ok_or_else(|| {
            invalid(format!(
                "assembly {record_name} #{id} has invalid coordinates"
            ))
        })?;
    if values.len() != 3 {
        return Err(invalid(format!(
            "assembly {record_name} #{id} requires three coordinates"
        )));
    }
    let result = std::array::from_fn(|axis| parameter_number(&values[axis]).unwrap_or(f64::NAN));
    if result.into_iter().any(|value| !value.is_finite()) {
        return Err(invalid(format!(
            "assembly {record_name} #{id} contains non-finite coordinates"
        )));
    }
    Ok(result)
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left.into_iter().zip(right).map(|(a, b)| a * b).sum()
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn normalize(vector: [f64; 3]) -> Option<[f64; 3]> {
    let magnitude = dot(vector, vector).sqrt();
    (magnitude.is_finite() && magnitude > 1.0e-12).then(|| vector.map(|value| value / magnitude))
}

fn parameter_number(parameter: &Parameter) -> Option<f64> {
    match parameter {
        Parameter::Real(value) if value.is_finite() => Some(*value),
        Parameter::Integer(value) => i32::try_from(*value).ok().map(f64::from),
        Parameter::Typed { parameter, .. } => parameter_number(parameter),
        _ => None,
    }
}

fn string_parameter(parameter: Option<&Parameter>) -> Option<String> {
    match parameter? {
        Parameter::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn referenced_entities(parameter: &Parameter) -> Vec<u64> {
    let mut refs = Vec::new();
    collect_entity_refs(parameter, &mut refs);
    refs
}

fn unique_record<'a>(
    entity: &'a EntityInstance,
    name: &str,
) -> Result<Option<&'a Record>, ExportError> {
    let mut records = entity_records(entity)
        .into_iter()
        .filter(|record| record.name == name);
    let first = records.next();
    if records.next().is_some() {
        return Err(invalid(format!(
            "STEP entity #{} contains multiple {name} records",
            entity_id(entity)
        )));
    }
    Ok(first)
}

fn has_record(entities: &[EntityInstance], id: u64, name: &str) -> bool {
    entity_by_id(entities, id).is_some_and(|entity| {
        entity_records(entity)
            .into_iter()
            .any(|record| record.name == name)
    })
}

fn invalid(message: impl Into<String>) -> ExportError {
    ExportError::InvalidStep(message.into())
}
