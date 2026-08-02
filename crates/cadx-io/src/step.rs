use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use cadx_core::domain::{StepLengthUnit, StepShellBoundary};
use ruststep::ast::{EntityInstance, Exchange, Name, Parameter, Record};

use crate::{ExportError, atomic::write_atomic};

mod assembly;
mod body;
mod color;

pub use assembly::{StepImportAssembly, StepImportOccurrence};
use body::discover_bodies;
pub use color::StepBodyColor;
use color::resolve_body_color;

/// A validated STEP source and the shell entities it can provide as CADX
/// imported solid features.
#[derive(Debug, Clone, PartialEq)]
pub struct StepImport {
    pub source: String,
    pub bodies: Vec<StepImportBody>,
    pub assemblies: Vec<StepImportAssembly>,
    pub standalone_body_indices: Vec<usize>,
}

/// One importable STEP body and the exchange interpretation required to
/// reconstruct it deterministically.
#[derive(Debug, Clone, PartialEq)]
pub struct StepImportBody {
    pub data_section: usize,
    pub shell_id: u64,
    pub void_shells: Vec<StepShellBoundary>,
    pub name: Option<String>,
    pub length_unit: StepLengthUnit,
    pub color: StepBodyColor,
}

/// Parses STEP physical-file syntax and requires at least one data entity.
///
/// # Errors
///
/// Returns [`ExportError::InvalidStep`] for malformed or empty exchange data.
pub fn validate_step(source: &str) -> Result<(), ExportError> {
    parse_exchange(source).map(|_| ())
}

fn parse_exchange(source: &str) -> Result<Exchange, ExportError> {
    let exchange = ruststep::parser::parse(source)
        .map_err(|error| ExportError::InvalidStep(error.to_string()))?;
    if exchange
        .data
        .iter()
        .all(|section| section.entities.is_empty())
    {
        return Err(ExportError::InvalidStep(
            "STEP document contains no data entities".into(),
        ));
    }
    Ok(exchange)
}

/// Reads and validates a STEP file for import into a CADX document.
///
/// The source is returned verbatim so the resulting document can embed it and
/// remain evaluable without the original file path.
///
/// # Errors
///
/// Returns [`ExportError`] when the file cannot be read, is malformed, or has
/// no supported closed shell entity.
pub fn read_step(path: impl Into<PathBuf>) -> Result<StepImport, ExportError> {
    let path = path.into();
    let source = fs::read_to_string(&path).map_err(|source| ExportError::Io {
        path: path.clone(),
        source,
    })?;
    parse_step(source)
}

/// Parses validated STEP source into importable body descriptors without
/// requiring a filesystem path.
///
/// # Errors
///
/// Returns [`ExportError::InvalidStep`] for malformed data, invalid unit
/// declarations, or sources without supported closed shells.
pub fn parse_step(source: String) -> Result<StepImport, ExportError> {
    let exchange = parse_exchange(&source)?;
    let mut bodies = Vec::new();
    let mut assemblies = Vec::new();
    let mut standalone_body_indices = Vec::new();
    for (data_section, section) in exchange.data.iter().enumerate() {
        let length_unit = resolve_section_length_unit(&section.entities)?;
        let discovered = discover_bodies(&section.entities)?;
        let body_offset = bodies.len();
        let assembly = assembly::discover_assembly(
            &section.entities,
            data_section,
            &discovered,
            length_unit.millimeters_per_unit,
        )?;
        let claimed = assembly
            .iter()
            .flat_map(|assembly| &assembly.occurrences)
            .flat_map(|occurrence| &occurrence.body_indices)
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        standalone_body_indices.extend(
            (0..discovered.len())
                .filter(|index| !claimed.contains(index))
                .map(|index| body_offset + index),
        );
        if let Some(mut assembly) = assembly {
            for occurrence in &mut assembly.occurrences {
                for body_index in &mut occurrence.body_indices {
                    *body_index += body_offset;
                }
            }
            assemblies.push(assembly);
        }
        bodies.extend(discovered.into_iter().map(|body| {
            let color = resolve_body_color(
                &section.entities,
                body.source_entity_id,
                &body.boundary_style_targets,
            );
            StepImportBody {
                data_section,
                shell_id: body.outer_shell_id,
                void_shells: body.void_shells,
                name: body.name,
                length_unit: length_unit.clone(),
                color,
            }
        }));
    }
    if bodies.is_empty() {
        return Err(ExportError::InvalidStep(
            "STEP document contains no supported CLOSED_SHELL entities".into(),
        ));
    }
    Ok(StepImport {
        source,
        bodies,
        assemblies,
        standalone_body_indices,
    })
}

fn entity_id(entity: &EntityInstance) -> u64 {
    match entity {
        EntityInstance::Simple { id, .. } | EntityInstance::Complex { id, .. } => *id,
    }
}

fn entity_records(entity: &EntityInstance) -> Vec<&Record> {
    match entity {
        EntityInstance::Simple { record, .. } => vec![record],
        EntityInstance::Complex { subsuper, .. } => subsuper.0.iter().collect(),
    }
}

fn entity_by_id(entities: &[EntityInstance], id: u64) -> Option<&EntityInstance> {
    entities.iter().find(|entity| entity_id(entity) == id)
}

fn parameter_list(parameter: &Parameter) -> Option<&[Parameter]> {
    match parameter {
        Parameter::List(values) => Some(values),
        _ => None,
    }
}

fn parameter_ref(parameter: &Parameter) -> Option<u64> {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => Some(*id),
        _ => None,
    }
}

fn collect_entity_refs(parameter: &Parameter, refs: &mut Vec<u64>) {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => refs.push(*id),
        Parameter::List(values) => {
            for value in values {
                collect_entity_refs(value, refs);
            }
        }
        Parameter::Typed { parameter, .. } => collect_entity_refs(parameter, refs),
        _ => {}
    }
}

fn resolve_section_length_unit(entities: &[EntityInstance]) -> Result<StepLengthUnit, ExportError> {
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

/// Validates and atomically writes a STEP physical file.
///
/// # Errors
///
/// Returns [`ExportError`] when parsing or file output fails.
pub fn write_step(source: &str, path: impl AsRef<Path>) -> Result<(), ExportError> {
    validate_step(source)?;
    write_atomic(path.as_ref(), source.as_bytes()).map_err(ExportError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const VALID_STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('CADX'),'2;1');\nFILE_NAME('model.step','',(''),(''),'CADX','CADX','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";
    static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

    fn read_source(source: &str) -> StepImport {
        let path = std::env::temp_dir().join(format!(
            "cadx-step-import-{}-{}-{}.step",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, source).unwrap();
        let imported = read_step(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        imported
    }

    #[test]
    fn validates_a_step_exchange() {
        validate_step(VALID_STEP).unwrap();
    }

    #[test]
    fn rejects_empty_step_data() {
        let empty = VALID_STEP.replace("#1=CARTESIAN_POINT('',(0.,0.,0.));\n", "");
        assert!(matches!(
            validate_step(&empty),
            Err(ExportError::InvalidStep(_))
        ));
    }

    #[test]
    fn reads_shell_entity_ids_for_import() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#1=OPEN_SHELL('',(#99));\n#2=CLOSED_SHELL('',(#99));",
        );
        let imported = read_source(&source);
        assert_eq!(imported.source, source);
        assert_eq!(imported.bodies.len(), 1);
        assert_eq!(imported.bodies[0].data_section, 0);
        assert_eq!(imported.bodies[0].shell_id, 2);
        assert!(imported.bodies[0].void_shells.is_empty());
        assert_eq!(imported.bodies[0].length_unit, StepLengthUnit::default());
        assert_eq!(imported.bodies[0].color, StepBodyColor::Absent);
        assert!(imported.assemblies.is_empty());
        assert_eq!(imported.standalone_body_indices, vec![0]);
    }

    #[test]
    fn discovers_repeated_ap242_occurrences_with_effective_placements() {
        use cadx_core::assembly::ComponentKind;

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
    fn shell_style_on_only_one_boundary_is_not_flattened() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('outer',(#90));\n\
             #21=CLOSED_SHELL('void',(#91));\n\
             #22=ORIENTED_CLOSED_SHELL('',*,#21,.T.);\n\
             #23=BREP_WITH_VOIDS('Partially styled',#20,(#22));\n\
             #30=COLOUR_RGB('',0.15,0.3,0.75);\n\
             #31=SURFACE_STYLE_SHADING(#30);\n\
             #32=PRESENTATION_STYLE_ASSIGNMENT((#31));\n\
             #33=STYLED_ITEM('',(#32),#20);",
        );
        let imported = read_source(&source);
        assert_eq!(imported.bodies.len(), 1);
        assert_eq!(imported.bodies[0].color, StepBodyColor::Unsupported);
    }

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
    fn finds_bodies_in_every_data_section() {
        let source = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('CADX'),'2;1');\nFILE_NAME('multi.step','',(''),(''),'CADX','CADX','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1=CLOSED_SHELL('',(#99));\nENDSEC;\nDATA;\n#1=CLOSED_SHELL('',(#99));\nENDSEC;\nEND-ISO-10303-21;\n";
        let imported = read_source(source);
        assert_eq!(imported.bodies.len(), 2);
        assert_eq!(imported.bodies[0].data_section, 0);
        assert_eq!(imported.bodies[1].data_section, 1);
        assert_eq!(imported.bodies[0].shell_id, 1);
        assert_eq!(imported.bodies[1].shell_id, 1);
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

    #[test]
    fn reads_solid_level_ap214_color_and_transparency() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('',(#99));\n\
             #21=MANIFOLD_SOLID_BREP('Painted housing',#20);\n\
             #30=COLOUR_RGB('Supplier blue',0.1,0.2,0.8);\n\
             #31=SURFACE_STYLE_RENDERING(#30,0.25);\n\
             #32=PRESENTATION_STYLE_ASSIGNMENT((#31));\n\
             #33=STYLED_ITEM('',(#32),#21);",
        );
        let imported = read_source(&source);
        assert_eq!(
            imported.bodies[0].color,
            StepBodyColor::Uniform([0.1, 0.2, 0.8, 0.75])
        );
    }

    #[test]
    fn promotes_only_complete_uniform_face_color_to_a_body_color() {
        let style = "#30=COLOUR_RGB('',0.8,0.3,0.1);\n\
                     #31=FILL_AREA_STYLE_COLOUR('',#30);\n\
                     #32=FILL_AREA_STYLE('',(#31));\n\
                     #33=SURFACE_STYLE_FILL_AREA(#32);\n\
                     #34=SURFACE_SIDE_STYLE('',(#33));\n\
                     #35=SURFACE_STYLE_USAGE(.BOTH.,#34);\n\
                     #36=PRESENTATION_STYLE_ASSIGNMENT((#35));";
        let geometry = "#20=CLOSED_SHELL('',(#21,#22));\n\
                        #21=ADVANCED_FACE('',(#91),#92,.T.);\n\
                        #22=ADVANCED_FACE('',(#93),#94,.T.);\n\
                        #23=MANIFOLD_SOLID_BREP('Uniform faces',#20);";
        let complete = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            &format!(
                "{geometry}\n{style}\n#37=STYLED_ITEM('',(#36),#21);\n#38=STYLED_ITEM('',(#36),#22);"
            ),
        );
        let partial = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            &format!("{geometry}\n{style}\n#37=STYLED_ITEM('',(#36),#21);"),
        );

        assert_eq!(
            read_source(&complete).bodies[0].color,
            StepBodyColor::Uniform([0.8, 0.3, 0.1, 1.0])
        );
        assert_eq!(
            read_source(&partial).bodies[0].color,
            StepBodyColor::Unsupported
        );
    }

    #[test]
    fn preserves_malformed_or_unrecognized_style_attachments_as_unsupported() {
        let geometry = "#20=CLOSED_SHELL('',(#21));\n\
                        #21=ADVANCED_FACE('',(#91),#92,.T.);\n\
                        #22=MANIFOLD_SOLID_BREP('Styled body',#20);";
        let style_cases = [
            "#30=STYLED_ITEM('',('not a style reference'),#22);",
            "#30=STYLED_ITEM('',(#999),#22,'extra');",
            "#30=CURVE_STYLE('',#99,$,#98);\n#31=STYLED_ITEM('',(#30),#22);",
            "#30=PRESENTATION_STYLE_ASSIGNMENT((#999));\n#31=STYLED_ITEM('',(#30),#22);",
        ];

        for style in style_cases {
            let source = VALID_STEP.replace(
                "#1=CARTESIAN_POINT('',(0.,0.,0.));",
                &format!("{geometry}\n{style}"),
            );
            assert_eq!(
                read_source(&source).bodies[0].color,
                StepBodyColor::Unsupported
            );
        }
    }
}
