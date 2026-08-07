//! Length-unit discovery for a STEP DATA section.
//!
//! Resolves the assigned length unit for a section, rejecting conflicting
//! assignments, and falls back to the assumed-millimeter legacy path when a
//! source declares no global unit context.

use std::collections::HashSet;

use cadx_core::domain::StepLengthUnit;
use ruststep::ast::{EntityInstance, Parameter, Record};

use crate::ExportError;

use super::ast::{
    collect_entity_refs, entity_by_id, entity_id, entity_records, parameter_list, parameter_ref,
};

pub(super) fn resolve_section_length_unit(
    entities: &[EntityInstance],
) -> Result<StepLengthUnit, ExportError> {
    let mut assigned_refs = Vec::new();
    let mut has_assigned_context = false;
    for entity in entities {
        for record in entity_records(entity) {
            if record.name == "GLOBAL_UNIT_ASSIGNED_CONTEXT" {
                has_assigned_context = true;
                collect_entity_refs(&record.parameter, &mut assigned_refs);
            }
        }
    }

    if has_assigned_context && assigned_refs.is_empty() {
        return Err(ExportError::InvalidStep(
            "STEP global unit context contains no unit references".into(),
        ));
    }
    if has_assigned_context
        && let Some(missing) = assigned_refs
            .iter()
            .find(|id| entity_by_id(entities, **id).is_none())
    {
        return Err(ExportError::InvalidStep(format!(
            "STEP global unit context references missing entity #{missing}"
        )));
    }

    let candidate_ids = if has_assigned_context {
        assigned_refs
    } else {
        entities.iter().map(entity_id).collect::<Vec<_>>()
    };
    let mut units = Vec::new();
    for id in candidate_ids {
        if let Some(unit) = resolve_length_unit(entities, id, &mut HashSet::new())?
            && !units.iter().any(|existing: &StepLengthUnit| {
                unit_scales_match(existing.millimeters_per_unit, unit.millimeters_per_unit)
            })
        {
            units.push(unit);
        }
    }
    match units.as_slice() {
        [] if has_assigned_context => Err(ExportError::InvalidStep(
            "STEP global unit context has no supported length unit".into(),
        )),
        [] => Ok(StepLengthUnit::assumed_millimeter()),
        [unit] => Ok(unit.clone()),
        _ => Err(ExportError::InvalidStep(
            "STEP DATA section assigns conflicting length units".into(),
        )),
    }
}

fn unit_scales_match(left: f64, right: f64) -> bool {
    (left - right).abs() <= f64::EPSILON * left.abs().max(right.abs()).max(1.0) * 8.0
}

fn resolve_length_unit(
    entities: &[EntityInstance],
    id: u64,
    visiting: &mut HashSet<u64>,
) -> Result<Option<StepLengthUnit>, ExportError> {
    if !visiting.insert(id) {
        return Err(ExportError::InvalidStep(format!(
            "STEP unit definition contains a reference cycle at #{id}"
        )));
    }
    let Some(entity) = entity_by_id(entities, id) else {
        visiting.remove(&id);
        return Ok(None);
    };
    let records = entity_records(entity);
    let explicitly_length = records.iter().any(|record| record.name == "LENGTH_UNIT");
    let result = if let Some(si) = records.iter().find(|record| record.name == "SI_UNIT") {
        parse_si_length_unit(si, explicitly_length)?
    } else if explicitly_length {
        records
            .iter()
            .find(|record| record.name == "CONVERSION_BASED_UNIT")
            .map(|record| parse_conversion_length_unit(entities, record, visiting))
            .transpose()?
            .flatten()
    } else {
        None
    };
    visiting.remove(&id);
    Ok(result)
}

fn parse_si_length_unit(
    record: &Record,
    explicitly_length: bool,
) -> Result<Option<StepLengthUnit>, ExportError> {
    let Some(values) = parameter_list(&record.parameter) else {
        return Err(ExportError::InvalidStep(
            "STEP SI_UNIT has invalid parameters".into(),
        ));
    };
    let Some(Parameter::Enumeration(unit_name)) = values.last() else {
        return Err(ExportError::InvalidStep(
            "STEP SI_UNIT has no unit name".into(),
        ));
    };
    if unit_name != "METRE" {
        return if explicitly_length {
            Err(ExportError::InvalidStep(format!(
                "STEP LENGTH_UNIT uses non-length SI unit .{unit_name}."
            )))
        } else {
            Ok(None)
        };
    }
    if !explicitly_length && values.len() < 2 {
        return Ok(None);
    }
    let prefix = values.first().and_then(|value| match value {
        Parameter::Enumeration(prefix) => Some(prefix.as_str()),
        Parameter::NotProvided => Some(""),
        _ => None,
    });
    let Some(prefix) = prefix else {
        return Err(ExportError::InvalidStep(
            "STEP SI length unit has an invalid prefix".into(),
        ));
    };
    let (name, factor) = si_prefix(prefix).ok_or_else(|| {
        ExportError::InvalidStep(format!("unsupported STEP SI prefix .{prefix}."))
    })?;
    Ok(Some(StepLengthUnit {
        name: format!("{name}metre"),
        millimeters_per_unit: factor * 1_000.0,
        declared: true,
    }))
}

fn si_prefix(prefix: &str) -> Option<(&'static str, f64)> {
    Some(match prefix {
        "" => ("", 1.0),
        "EXA" => ("exa", 1.0e18),
        "PETA" => ("peta", 1.0e15),
        "TERA" => ("tera", 1.0e12),
        "GIGA" => ("giga", 1.0e9),
        "MEGA" => ("mega", 1.0e6),
        "KILO" => ("kilo", 1.0e3),
        "HECTO" => ("hecto", 1.0e2),
        "DECA" => ("deca", 1.0e1),
        "DECI" => ("deci", 1.0e-1),
        "CENTI" => ("centi", 1.0e-2),
        "MILLI" => ("milli", 1.0e-3),
        "MICRO" => ("micro", 1.0e-6),
        "NANO" => ("nano", 1.0e-9),
        "PICO" => ("pico", 1.0e-12),
        "FEMTO" => ("femto", 1.0e-15),
        "ATTO" => ("atto", 1.0e-18),
        _ => return None,
    })
}

fn parse_conversion_length_unit(
    entities: &[EntityInstance],
    record: &Record,
    visiting: &mut HashSet<u64>,
) -> Result<Option<StepLengthUnit>, ExportError> {
    let values = parameter_list(&record.parameter).ok_or_else(|| {
        ExportError::InvalidStep("STEP CONVERSION_BASED_UNIT has invalid parameters".into())
    })?;
    let name = values.iter().find_map(|value| match value {
        Parameter::String(name) => Some(name.trim()),
        _ => None,
    });
    let factor_id = values.iter().find_map(parameter_ref);
    let (Some(name), Some(factor_id)) = (name, factor_id) else {
        return Err(ExportError::InvalidStep(
            "STEP conversion-based length unit is incomplete".into(),
        ));
    };
    let factor_entity = entity_by_id(entities, factor_id).ok_or_else(|| {
        ExportError::InvalidStep(format!(
            "STEP unit conversion references missing #{factor_id}"
        ))
    })?;
    let measure = entity_records(factor_entity)
        .into_iter()
        .find(|candidate| {
            matches!(
                candidate.name.as_str(),
                "LENGTH_MEASURE_WITH_UNIT" | "MEASURE_WITH_UNIT"
            )
        })
        .ok_or_else(|| {
            ExportError::InvalidStep(format!(
                "STEP unit conversion #{factor_id} is not a length measure"
            ))
        })?;
    let measure_values = parameter_list(&measure.parameter).ok_or_else(|| {
        ExportError::InvalidStep(format!(
            "STEP length measure #{factor_id} has invalid parameters"
        ))
    })?;
    let magnitude = measure_values
        .iter()
        .find_map(parameter_number)
        .ok_or_else(|| {
            ExportError::InvalidStep(format!(
                "STEP length measure #{factor_id} has no numeric magnitude"
            ))
        })?;
    let base_id = measure_values
        .iter()
        .find_map(parameter_ref)
        .ok_or_else(|| {
            ExportError::InvalidStep(format!("STEP length measure #{factor_id} has no base unit"))
        })?;
    let base = resolve_length_unit(entities, base_id, visiting)?.ok_or_else(|| {
        ExportError::InvalidStep(format!(
            "STEP length measure #{factor_id} does not reference a length unit"
        ))
    })?;
    let millimeters_per_unit = magnitude * base.millimeters_per_unit;
    if name.is_empty() || !millimeters_per_unit.is_finite() || millimeters_per_unit <= 0.0 {
        return Err(ExportError::InvalidStep(
            "STEP conversion-based length unit has an invalid name or factor".into(),
        ));
    }
    Ok(Some(StepLengthUnit {
        name: name.to_owned(),
        millimeters_per_unit,
        declared: true,
    }))
}

fn parameter_number(parameter: &Parameter) -> Option<f64> {
    match parameter {
        Parameter::Real(value) => Some(*value),
        Parameter::Integer(value) => i32::try_from(*value).ok().map(f64::from),
        Parameter::Typed { parameter, .. } => parameter_number(parameter),
        Parameter::List(values) => values.iter().find_map(parameter_number),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use cadx_core::domain::StepLengthUnit;

    use crate::{
        ExportError,
        step::{
            read_step,
            test_support::{VALID_STEP, read_source},
        },
    };

    #[test]
    fn reads_assigned_si_units_and_product_body_names() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#10=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\n\
             #11=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#10)) REPRESENTATION_CONTEXT('',''));\n\
             #20=CLOSED_SHELL('',(#99));\n\
             #21=MANIFOLD_SOLID_BREP('Gear housing',#20);",
        );
        let imported = read_source(&source);
        assert_eq!(imported.bodies.len(), 1);
        let body = &imported.bodies[0];
        assert_eq!(body.name.as_deref(), Some("Gear housing"));
        assert_eq!(body.length_unit, StepLengthUnit::millimeter());
    }

    #[test]
    fn resolves_conversion_based_inch_units() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#10=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\n\
             #11=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#10);\n\
             #12=(CONVERSION_BASED_UNIT('inch',#11) LENGTH_UNIT() NAMED_UNIT(*));\n\
             #13=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#12)) REPRESENTATION_CONTEXT('',''));\n\
             #20=CLOSED_SHELL('',(#99));",
        );
        let imported = read_source(&source);
        assert_eq!(imported.bodies[0].length_unit.name, "inch");
        assert!((imported.bodies[0].length_unit.millimeters_per_unit - 25.4).abs() < f64::EPSILON);
        assert!(imported.bodies[0].length_unit.declared);
    }

    #[test]
    fn rejects_conflicting_assigned_length_units() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#10=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\n\
             #11=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.));\n\
             #12=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#10,#11)) REPRESENTATION_CONTEXT('',''));\n\
             #20=CLOSED_SHELL('',(#99));",
        );
        let path = std::env::temp_dir().join(format!(
            "cadx-step-conflicting-units-{}-{}.step",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, source).unwrap();
        let result = read_step(&path);
        std::fs::remove_file(path).unwrap();
        assert!(matches!(
            result,
            Err(ExportError::InvalidStep(message)) if message.contains("conflicting length units")
        ));
    }

    #[test]
    fn rejects_an_explicit_context_without_a_resolvable_length_unit() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#10=(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.));\n\
             #11=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#10)) REPRESENTATION_CONTEXT('',''));\n\
             #20=CLOSED_SHELL('',(#99));",
        );
        let path = std::env::temp_dir().join(format!(
            "cadx-step-missing-length-unit-{}-{}.step",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, source).unwrap();
        let result = read_step(&path);
        std::fs::remove_file(path).unwrap();
        assert!(matches!(
            result,
            Err(ExportError::InvalidStep(message)) if message.contains("no supported length unit")
        ));
    }
}
