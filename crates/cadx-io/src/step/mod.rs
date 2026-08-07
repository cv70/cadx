//! STEP AP203/AP214/AP242 exchange: validation, import discovery, and output.
//!
//! [`parse_step`] is the single entry point that turns validated physical-file
//! source into [`StepImport`], delegating unit, body, color, and product-structure
//! discovery to the submodules below.

use std::{fs, path::PathBuf};

use cadx_core::domain::{StepLengthUnit, StepShellBoundary};

use crate::ExportError;

mod assembly;
mod ast;
mod body;
mod color;
mod encode;
#[cfg(test)]
mod test_support;
mod unit;

pub use assembly::{StepImportAssembly, StepImportOccurrence};
use ast::parse_exchange;
use body::discover_bodies;
pub use color::StepBodyColor;
use color::resolve_body_color;
pub use encode::write_step;
use unit::resolve_section_length_unit;

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

#[cfg(test)]
mod tests {
    use super::{StepBodyColor, StepLengthUnit, validate_step};
    use crate::{
        ExportError,
        step::test_support::{VALID_STEP, read_source},
    };

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
    fn finds_bodies_in_every_data_section() {
        let source = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('CADX'),'2;1');\nFILE_NAME('multi.step','',(''),(''),'CADX','CADX','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1=CLOSED_SHELL('',(#99));\nENDSEC;\nDATA;\n#1=CLOSED_SHELL('',(#99));\nENDSEC;\nEND-ISO-10303-21;\n";
        let imported = read_source(source);
        assert_eq!(imported.bodies.len(), 2);
        assert_eq!(imported.bodies[0].data_section, 0);
        assert_eq!(imported.bodies[1].data_section, 1);
        assert_eq!(imported.bodies[0].shell_id, 1);
        assert_eq!(imported.bodies[1].shell_id, 1);
    }
}
