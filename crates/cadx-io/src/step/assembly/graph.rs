//! Extraction of the raw AP242 product-structure graph: usage occurrences,
//! product-definition naming, shape representations, and the representation
//! relationships that carry occurrence transforms.

use std::collections::BTreeMap;

use cadx_core::assembly::AssemblyTransform;
use ruststep::ast::{EntityInstance, Parameter};

use crate::ExportError;

use super::{
    super::{
        ast::{
            collect_entity_refs, entity_by_id, entity_id, entity_records, parameter_list,
            parameter_ref,
        },
        body::StepBodyDefinition,
    },
    invalid, unique_record,
};

#[derive(Debug, Clone)]
pub(super) struct Usage {
    pub(super) source_id: u64,
    pub(super) name: String,
    pub(super) parent_definition: u64,
    pub(super) child_definition: u64,
    pub(super) transform: AssemblyTransform,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Relationship {
    pub(super) representation_1: u64,
    pub(super) representation_2: u64,
    pub(super) transform_id: Option<u64>,
}

pub(super) fn raw_usages(
    entities: &[EntityInstance],
) -> Result<Vec<(u64, String, u64, u64)>, ExportError> {
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

pub(super) fn product_names(entities: &[EntityInstance]) -> BTreeMap<u64, String> {
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

pub(super) fn formation_products(entities: &[EntityInstance]) -> BTreeMap<u64, u64> {
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

pub(super) fn definition_names(
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

pub(super) fn definition_representations(
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

pub(super) fn representation_bodies(
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

pub(super) fn map_definition_bodies(
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

pub(super) fn relationships(
    entities: &[EntityInstance],
) -> Result<BTreeMap<u64, Relationship>, ExportError> {
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

pub(super) fn usage_relationships(
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

pub(super) fn resolve_usage_relationship(
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

fn has_record(entities: &[EntityInstance], id: u64, name: &str) -> bool {
    entity_by_id(entities, id).is_some_and(|entity| {
        entity_records(entity)
            .into_iter()
            .any(|record| record.name == name)
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        ExportError,
        step::{parse_step, test_support::VALID_STEP},
    };

    #[test]
    fn rejects_ambiguous_assembly_relationship_fallback() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#10=PRODUCT('ROOT','Root','',(#99));\n\
             #11=PRODUCT('CHILD','Child','',(#99));\n\
             #20=PRODUCT_DEFINITION_FORMATION('','',#10);\n\
             #21=PRODUCT_DEFINITION_FORMATION('','',#11);\n\
             #30=PRODUCT_DEFINITION('','',#20,#99);\n\
             #31=PRODUCT_DEFINITION('','',#21,#99);\n\
             #40=PRODUCT_DEFINITION_SHAPE('','',#30);\n\
             #41=PRODUCT_DEFINITION_SHAPE('','',#31);\n\
             #50=CLOSED_SHELL('',(#98));\n\
             #51=MANIFOLD_SOLID_BREP('',#50);\n\
             #60=SHAPE_REPRESENTATION('',(#99),#99);\n\
             #61=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#51),#99);\n\
             #62=SHAPE_DEFINITION_REPRESENTATION(#40,#60);\n\
             #63=SHAPE_DEFINITION_REPRESENTATION(#41,#61);\n\
             #70=NEXT_ASSEMBLY_USAGE_OCCURRENCE('U','Use','',#30,#31,$);\n\
             #71=SHAPE_REPRESENTATION_RELATIONSHIP('','',#60,#61);\n\
             #72=SHAPE_REPRESENTATION_RELATIONSHIP('','',#60,#61);",
        );
        let result = parse_step(source);
        assert!(
            matches!(
            result,
            Err(ExportError::InvalidStep(ref message))
                if message.contains("ambiguous representation relationships")
            ),
            "{result:?}"
        );
    }

    #[test]
    fn rejects_canonical_assembly_relationship_for_the_wrong_representations() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#10=PRODUCT('ROOT','Root','',(#99));\n\
             #11=PRODUCT('CHILD','Child','',(#99));\n\
             #20=PRODUCT_DEFINITION_FORMATION('','',#10);\n\
             #21=PRODUCT_DEFINITION_FORMATION('','',#11);\n\
             #30=PRODUCT_DEFINITION('','',#20,#99);\n\
             #31=PRODUCT_DEFINITION('','',#21,#99);\n\
             #40=PRODUCT_DEFINITION_SHAPE('','',#30);\n\
             #41=PRODUCT_DEFINITION_SHAPE('','',#31);\n\
             #50=CLOSED_SHELL('',(#98));\n\
             #51=MANIFOLD_SOLID_BREP('',#50);\n\
             #60=SHAPE_REPRESENTATION('',(#99),#99);\n\
             #61=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#51),#99);\n\
             #62=SHAPE_DEFINITION_REPRESENTATION(#40,#60);\n\
             #63=SHAPE_DEFINITION_REPRESENTATION(#41,#61);\n\
             #70=NEXT_ASSEMBLY_USAGE_OCCURRENCE('U','Use','',#30,#31,$);\n\
             #71=PRODUCT_DEFINITION_SHAPE('','',#70);\n\
             #72=SHAPE_REPRESENTATION_RELATIONSHIP('','',#60,#60);\n\
             #73=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#72,#71);",
        );
        let result = parse_step(source);
        assert!(
            matches!(
                result,
                Err(ExportError::InvalidStep(ref message))
                    if message.contains("does not relate its parent and child representations")
            ),
            "{result:?}"
        );
    }
}
