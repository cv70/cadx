use std::collections::{BTreeMap, BTreeSet};

use cadx_core::domain::{MAX_STEP_VOID_SHELLS, StepShellBoundary};
use ruststep::ast::{EntityInstance, Parameter, Record};

use crate::ExportError;

use super::ast::{entity_by_id, entity_id, entity_records, parameter_list, parameter_ref};

#[derive(Debug)]
pub(super) struct StepBodyDefinition {
    pub source_entity_id: Option<u64>,
    pub outer_shell_id: u64,
    pub void_shells: Vec<StepShellBoundary>,
    pub boundary_style_targets: Vec<(u64, Option<u64>)>,
    pub name: Option<String>,
}

pub(super) fn discover_bodies(
    entities: &[EntityInstance],
) -> Result<Vec<StepBodyDefinition>, ExportError> {
    let closed_shells = entities
        .iter()
        .filter(|entity| {
            entity_records(entity)
                .iter()
                .any(|record| record.name == "CLOSED_SHELL")
        })
        .map(entity_id)
        .collect::<BTreeSet<_>>();
    let mut claimed = BTreeMap::<u64, u64>::new();
    let mut bodies = Vec::new();

    for entity in entities {
        let records = entity_records(entity);
        if let Some(record) = one_record(&records, "BREP_WITH_VOIDS")? {
            let body_id = entity_id(entity);
            let values = exact_parameters(record, 3)?;
            let outer_shell_id = required_closed_shell(entities, &closed_shells, &values[1])?;
            let void_values = parameter_list(&values[2]).ok_or_else(|| {
                invalid_body(body_id, "BREP_WITH_VOIDS has no void-shell aggregate")
            })?;
            if void_values.is_empty() {
                return Err(invalid_body(
                    body_id,
                    "BREP_WITH_VOIDS must contain at least one void shell",
                ));
            }
            if void_values.len() > MAX_STEP_VOID_SHELLS {
                return Err(invalid_body(body_id, "exceeds CADX's void-shell limit"));
            }

            claim_shell(&mut claimed, outer_shell_id, body_id)?;
            let mut void_shells = Vec::with_capacity(void_values.len());
            let mut boundary_style_targets = vec![(outer_shell_id, None)];
            for value in void_values {
                let oriented_id = parameter_ref(value).ok_or_else(|| {
                    invalid_body(body_id, "void-shell aggregate contains a non-entity value")
                })?;
                let boundary = oriented_boundary(entities, &closed_shells, oriented_id)?;
                claim_shell(&mut claimed, boundary.shell_id, body_id)?;
                boundary_style_targets.push((boundary.shell_id, Some(oriented_id)));
                void_shells.push(boundary);
            }
            bodies.push(StepBodyDefinition {
                source_entity_id: Some(body_id),
                outer_shell_id,
                void_shells,
                boundary_style_targets,
                name: body_name(values),
            });
        } else if let Some(record) = solid_record(&records)? {
            let body_id = entity_id(entity);
            let values = exact_parameters(record, 2)?;
            let outer_shell_id = required_closed_shell(entities, &closed_shells, &values[1])?;
            claim_shell(&mut claimed, outer_shell_id, body_id)?;
            bodies.push(StepBodyDefinition {
                source_entity_id: Some(body_id),
                outer_shell_id,
                void_shells: Vec::new(),
                boundary_style_targets: vec![(outer_shell_id, None)],
                name: body_name(values),
            });
        }
    }

    discover_surface_model_shells(entities, &closed_shells, &mut claimed, &mut bodies)?;
    for shell_id in closed_shells {
        if !claimed.contains_key(&shell_id) {
            bodies.push(StepBodyDefinition {
                source_entity_id: None,
                outer_shell_id: shell_id,
                void_shells: Vec::new(),
                boundary_style_targets: vec![(shell_id, None)],
                name: None,
            });
        }
    }
    Ok(bodies)
}

fn solid_record<'a>(records: &[&'a Record]) -> Result<Option<&'a Record>, ExportError> {
    let manifold = one_record(records, "MANIFOLD_SOLID_BREP")?;
    let faceted = one_record(records, "FACETED_BREP")?;
    match (manifold, faceted) {
        (Some(_), Some(_)) => Err(ExportError::InvalidStep(
            "STEP entity has conflicting manifold and faceted B-Rep records".into(),
        )),
        (record, None) | (None, record) => Ok(record),
    }
}

fn discover_surface_model_shells(
    entities: &[EntityInstance],
    closed_shells: &BTreeSet<u64>,
    claimed: &mut BTreeMap<u64, u64>,
    bodies: &mut Vec<StepBodyDefinition>,
) -> Result<(), ExportError> {
    for entity in entities {
        let records = entity_records(entity);
        let Some(record) = one_record(&records, "SHELL_BASED_SURFACE_MODEL")? else {
            continue;
        };
        let body_id = entity_id(entity);
        let values = exact_parameters(record, 2)?;
        let shells = parameter_list(&values[1]).ok_or_else(|| {
            invalid_body(body_id, "SHELL_BASED_SURFACE_MODEL has no shell aggregate")
        })?;
        for value in shells {
            let Some(shell_id) = parameter_ref(value) else {
                return Err(invalid_body(
                    body_id,
                    "surface-model shell aggregate contains a non-entity value",
                ));
            };
            if !closed_shells.contains(&shell_id) || claimed.contains_key(&shell_id) {
                continue;
            }
            claim_shell(claimed, shell_id, body_id)?;
            bodies.push(StepBodyDefinition {
                source_entity_id: Some(body_id),
                outer_shell_id: shell_id,
                void_shells: Vec::new(),
                boundary_style_targets: vec![(shell_id, None)],
                name: body_name(values),
            });
        }
    }
    Ok(())
}

fn one_record<'a>(records: &[&'a Record], name: &str) -> Result<Option<&'a Record>, ExportError> {
    let mut matching = records.iter().copied().filter(|record| record.name == name);
    let first = matching.next();
    if matching.next().is_some() {
        return Err(ExportError::InvalidStep(format!(
            "STEP entity contains multiple {name} records"
        )));
    }
    Ok(first)
}

fn exact_parameters(record: &Record, expected: usize) -> Result<&[Parameter], ExportError> {
    let values = parameter_list(&record.parameter).ok_or_else(|| {
        ExportError::InvalidStep(format!("STEP {} has invalid parameters", record.name))
    })?;
    if values.len() != expected {
        return Err(ExportError::InvalidStep(format!(
            "STEP {} requires {expected} parameters",
            record.name
        )));
    }
    Ok(values)
}

fn required_closed_shell(
    entities: &[EntityInstance],
    closed_shells: &BTreeSet<u64>,
    parameter: &Parameter,
) -> Result<u64, ExportError> {
    let shell_id = parameter_ref(parameter).ok_or_else(|| {
        ExportError::InvalidStep("STEP solid has no outer-shell entity reference".into())
    })?;
    if !closed_shells.contains(&shell_id) || entity_by_id(entities, shell_id).is_none() {
        return Err(ExportError::InvalidStep(format!(
            "STEP solid references missing or non-closed outer shell #{shell_id}"
        )));
    }
    Ok(shell_id)
}

fn oriented_boundary(
    entities: &[EntityInstance],
    closed_shells: &BTreeSet<u64>,
    oriented_id: u64,
) -> Result<StepShellBoundary, ExportError> {
    let entity = entity_by_id(entities, oriented_id).ok_or_else(|| {
        ExportError::InvalidStep(format!(
            "STEP void boundary references missing oriented shell #{oriented_id}"
        ))
    })?;
    let records = entity_records(entity);
    let record = one_record(&records, "ORIENTED_CLOSED_SHELL")?.ok_or_else(|| {
        ExportError::InvalidStep(format!(
            "STEP void boundary #{oriented_id} is not an ORIENTED_CLOSED_SHELL"
        ))
    })?;
    let values = exact_parameters(record, 4)?;
    let shell_id = required_closed_shell(entities, closed_shells, &values[2])?;
    let orientation = match &values[3] {
        Parameter::Enumeration(value) if value == "T" || value == "TRUE" => true,
        Parameter::Enumeration(value) if value == "F" || value == "FALSE" => false,
        _ => {
            return Err(ExportError::InvalidStep(format!(
                "STEP oriented void shell #{oriented_id} has invalid orientation"
            )));
        }
    };
    Ok(StepShellBoundary {
        shell_id,
        orientation,
    })
}

fn claim_shell(
    claimed: &mut BTreeMap<u64, u64>,
    shell_id: u64,
    body_id: u64,
) -> Result<(), ExportError> {
    if let Some(owner) = claimed.insert(shell_id, body_id) {
        return Err(ExportError::InvalidStep(format!(
            "STEP closed shell #{shell_id} is owned by multiple solids #{owner} and #{body_id}"
        )));
    }
    Ok(())
}

fn body_name(values: &[Parameter]) -> Option<String> {
    values.first().and_then(|value| match value {
        Parameter::String(name) if !name.trim().is_empty() => Some(name.trim().to_owned()),
        _ => None,
    })
}

fn invalid_body(body_id: u64, message: &str) -> ExportError {
    ExportError::InvalidStep(format!("STEP body #{body_id} {message}"))
}

#[cfg(test)]
mod tests {
    use cadx_core::domain::StepShellBoundary;

    use crate::{
        ExportError, StepBodyColor,
        step::{
            parse_step,
            test_support::{VALID_STEP, read_source},
        },
    };

    #[test]
    fn groups_brep_with_voids_as_one_oriented_body() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('outer',(#90));\n\
             #21=CLOSED_SHELL('void',(#91));\n\
             #22=ORIENTED_CLOSED_SHELL('',*,#21,.F.);\n\
             #23=BREP_WITH_VOIDS('Valve housing',#20,(#22));\n\
             #30=COLOUR_RGB('',0.15,0.3,0.75);\n\
             #31=SURFACE_STYLE_SHADING(#30);\n\
             #32=PRESENTATION_STYLE_ASSIGNMENT((#31));\n\
             #33=STYLED_ITEM('',(#32),#23);",
        );
        let imported = read_source(&source);
        assert_eq!(imported.bodies.len(), 1);
        let body = &imported.bodies[0];
        assert_eq!(body.name.as_deref(), Some("Valve housing"));
        assert_eq!(body.shell_id, 20);
        assert_eq!(
            body.void_shells,
            vec![StepShellBoundary {
                shell_id: 21,
                orientation: false,
            }]
        );
        assert_eq!(body.color, StepBodyColor::Uniform([0.15, 0.3, 0.75, 1.0]));
    }

    #[test]
    fn keeps_unowned_closed_shells_separate_from_void_ownership() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('outer',(#90));\n\
             #21=CLOSED_SHELL('void',(#91));\n\
             #22=ORIENTED_CLOSED_SHELL('',*,#21,.T.);\n\
             #23=BREP_WITH_VOIDS('Hollow body',#20,(#22));\n\
             #24=CLOSED_SHELL('standalone',(#92));",
        );
        let imported = read_source(&source);
        assert_eq!(imported.bodies.len(), 2);
        assert_eq!(imported.bodies[0].shell_id, 20);
        assert_eq!(imported.bodies[0].void_shells[0].shell_id, 21);
        assert_eq!(imported.bodies[1].shell_id, 24);
        assert!(imported.bodies[1].void_shells.is_empty());
    }

    #[test]
    fn rejects_invalid_or_multiply_owned_void_shells() {
        let invalid_void = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('outer',(#90));\n\
             #21=CLOSED_SHELL('void',(#91));\n\
             #23=BREP_WITH_VOIDS('Invalid',#20,(#21));",
        );
        assert!(matches!(
            parse_step(invalid_void),
            Err(ExportError::InvalidStep(message))
                if message.contains("not an ORIENTED_CLOSED_SHELL")
        ));

        let shared_shell = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('shared',(#90));\n\
             #21=MANIFOLD_SOLID_BREP('First',#20);\n\
             #22=MANIFOLD_SOLID_BREP('Second',#20);",
        );
        assert!(matches!(
            parse_step(shared_shell),
            Err(ExportError::InvalidStep(message)) if message.contains("owned by multiple solids")
        ));
    }

    #[test]
    fn recognizes_faceted_brep_as_the_shell_owner() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('',(#99));\n\
             #21=FACETED_BREP('Supplier tessellation',#20);",
        );
        let imported = read_source(&source);
        assert_eq!(imported.bodies.len(), 1);
        assert_eq!(imported.bodies[0].shell_id, 20);
        assert_eq!(
            imported.bodies[0].name.as_deref(),
            Some("Supplier tessellation")
        );
    }
}
