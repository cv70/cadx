use std::{
    fs,
    path::{Path, PathBuf},
};

use ruststep::ast::EntityInstance;

use crate::{ExportError, atomic::write_atomic};

/// A validated STEP source and the shell entities it can provide as CADX
/// imported solid features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepImport {
    pub source: String,
    pub shell_ids: Vec<u64>,
}

/// Parses STEP physical-file syntax and requires at least one data entity.
///
/// # Errors
///
/// Returns [`ExportError::InvalidStep`] for malformed or empty exchange data.
pub fn validate_step(source: &str) -> Result<(), ExportError> {
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
    Ok(())
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
    validate_step(&source)?;
    let exchange = ruststep::parser::parse(&source)
        .map_err(|error| ExportError::InvalidStep(error.to_string()))?;
    let mut shell_ids = exchange
        .data
        .first()
        .into_iter()
        .flat_map(|section| section.entities.iter())
        .filter_map(|entity| match entity {
            EntityInstance::Simple { id, record } if record.name == "CLOSED_SHELL" => Some(*id),
            EntityInstance::Complex { id, subsuper }
                if subsuper
                    .0
                    .iter()
                    .any(|record| record.name == "CLOSED_SHELL") =>
            {
                Some(*id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    shell_ids.sort_unstable();
    shell_ids.dedup();
    if shell_ids.is_empty() {
        return Err(ExportError::InvalidStep(
            "STEP document contains no supported CLOSED_SHELL entities".into(),
        ));
    }
    Ok(StepImport { source, shell_ids })
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

    const VALID_STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('CADX'),'2;1');\nFILE_NAME('model.step','',(''),(''),'CADX','CADX','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";

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
        let path = std::env::temp_dir().join(format!(
            "cadx-step-import-{}-{}.step",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, &source).unwrap();
        let imported = read_step(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(imported.source, source);
        assert_eq!(imported.shell_ids, vec![2]);
    }
}
