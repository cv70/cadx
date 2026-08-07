//! AP242 product-structure discovery: the component definitions and occurrence
//! tree a STEP DATA section describes.

use std::collections::{BTreeMap, BTreeSet};

use cadx_core::assembly::{
    AssemblyTransform, ComponentDefinition, ComponentDefinitionId, ComponentKind,
    ComponentOccurrenceId, MAX_COMPONENT_DEFINITIONS, MAX_COMPONENT_OCCURRENCES, StepEntityRef,
};
use ruststep::ast::{EntityInstance, Record};

use crate::ExportError;

use super::{
    ast::{entity_id, entity_records},
    body::StepBodyDefinition,
};

mod graph;
mod placement;

use graph::{
    Usage, definition_names, definition_representations, formation_products, map_definition_bodies,
    product_names, raw_usages, relationships, representation_bodies, resolve_usage_relationship,
    usage_relationships,
};
use placement::item_defined_transform;

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

fn invalid(message: impl Into<String>) -> ExportError {
    ExportError::InvalidStep(message.into())
}

#[cfg(test)]
mod tests {
    use cadx_core::assembly::ComponentKind;

    use crate::step::{parse_step, test_support::VALID_STEP};

    #[test]
    fn discovers_repeated_ap242_occurrences_with_effective_placements() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#10=PRODUCT('ROOT','Top fixture','',(#13));\n\
             #11=PRODUCT('PIN','Locating pin','',(#13));\n\
             #12=APPLICATION_CONTEXT('mechanical design');\n\
             #13=PRODUCT_CONTEXT('',#12,'mechanical');\n\
             #20=PRODUCT_DEFINITION_FORMATION('','',#10);\n\
             #21=PRODUCT_DEFINITION_FORMATION('','',#11);\n\
             #22=PRODUCT_DEFINITION_CONTEXT('',#12,'design');\n\
             #30=PRODUCT_DEFINITION('design','',#20,#22);\n\
             #31=PRODUCT_DEFINITION('design','',#21,#22);\n\
             #40=PRODUCT_DEFINITION_SHAPE('','',#30);\n\
             #41=PRODUCT_DEFINITION_SHAPE('','',#31);\n\
             #50=CLOSED_SHELL('',(#99));\n\
             #51=MANIFOLD_SOLID_BREP('Pin body',#50);\n\
             #60=CARTESIAN_POINT('',(0.,0.,0.));\n\
             #61=CARTESIAN_POINT('',(30.,0.,0.));\n\
             #62=CARTESIAN_POINT('',(-10.,0.,0.));\n\
             #63=CARTESIAN_POINT('',(10.,0.,0.));\n\
             #64=DIRECTION('',(0.,0.,1.));\n\
             #65=DIRECTION('',(1.,0.,0.));\n\
             #66=AXIS2_PLACEMENT_3D('',#60,#64,#65);\n\
             #67=AXIS2_PLACEMENT_3D('',#61,#64,#65);\n\
             #68=AXIS2_PLACEMENT_3D('',#62,#64,#65);\n\
             #69=AXIS2_PLACEMENT_3D('',#63,#64,#65);\n\
             #70=SHAPE_REPRESENTATION('',(#66),#90);\n\
             #71=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#51,#66),#90);\n\
             #72=SHAPE_DEFINITION_REPRESENTATION(#40,#70);\n\
             #73=SHAPE_DEFINITION_REPRESENTATION(#41,#71);\n\
             #80=NEXT_ASSEMBLY_USAGE_OCCURRENCE('PIN-1','Pin left','',#30,#31,$);\n\
             #81=NEXT_ASSEMBLY_USAGE_OCCURRENCE('PIN-2','Pin right','',#30,#31,$);\n\
             #82=PRODUCT_DEFINITION_SHAPE('','',#80);\n\
             #83=PRODUCT_DEFINITION_SHAPE('','',#81);\n\
             #84=ITEM_DEFINED_TRANSFORMATION('','',#67,#69);\n\
             #85=ITEM_DEFINED_TRANSFORMATION('','',#68,#69);\n\
             #86=(REPRESENTATION_RELATIONSHIP('','',#70,#71) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#84) SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #87=(REPRESENTATION_RELATIONSHIP('','',#70,#71) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#85) SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #88=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#86,#82);\n\
             #89=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#87,#83);\n\
             #90=GEOMETRIC_REPRESENTATION_CONTEXT(3);",
        );
        let imported = parse_step(source).unwrap();
        assert_eq!(imported.bodies.len(), 1);
        assert!(imported.standalone_body_indices.is_empty());
        assert_eq!(imported.assemblies.len(), 1);
        let assembly = &imported.assemblies[0];
        assert_eq!(assembly.name, "Top fixture");
        assert_eq!(assembly.definitions.len(), 2);
        assert!(
            assembly
                .definitions
                .iter()
                .any(|definition| definition.name == "Top fixture"
                    && definition.kind == ComponentKind::Assembly)
        );
        assert!(
            assembly
                .definitions
                .iter()
                .any(|definition| definition.name == "Locating pin"
                    && definition.kind == ComponentKind::Part)
        );
        assert_eq!(assembly.occurrences.len(), 3);
        let root = assembly
            .occurrences
            .iter()
            .find(|occurrence| occurrence.parent_id.is_none())
            .unwrap();
        let children = assembly
            .occurrences
            .iter()
            .filter(|occurrence| occurrence.parent_id == Some(root.id))
            .collect::<Vec<_>>();
        assert_eq!(children.len(), 2);
        assert!(
            children
                .iter()
                .all(|occurrence| occurrence.body_indices == vec![0])
        );
        for (actual, expected) in [
            (children[0].transform.translation, [20.0, 0.0, 0.0]),
            (children[1].transform.translation, [-20.0, 0.0, 0.0]),
        ] {
            assert!(
                actual
                    .into_iter()
                    .zip(expected)
                    .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
            );
        }
        assert_eq!(children[0].definition_id, children[1].definition_id);
        assert_ne!(children[0].source.entity_id, children[1].source.entity_id);
    }
}
