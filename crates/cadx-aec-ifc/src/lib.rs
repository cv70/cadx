//! Deterministic IFC4/IFC4X3 STEP physical-file export.

use cadx_aec_bim::{BimError, BimModel};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IfcExportProfile {
    pub schema: String,
    pub authoring_tool: String,
    pub length_unit: String,
    pub export_property_sets: bool,
}

impl Default for IfcExportProfile {
    fn default() -> Self {
        Self {
            schema: "IFC4".into(),
            authoring_tool: "CADX".into(),
            length_unit: "MILLI METRE".into(),
            export_property_sets: true,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IfcExportError {
    #[error(transparent)]
    InvalidModel(#[from] BimError),
    #[error("unsupported IFC schema {0}")]
    UnsupportedSchema(String),
    #[error("failed to format IFC output")]
    Formatting,
}

/// Emits a deterministic IFC STEP physical file with project, storeys and
/// classified product placeholders. Geometry representations remain a host
/// adapter concern until the B-Rep IFC bridge supplies mapped items.
///
/// # Errors
///
/// Returns a BIM validation error or unsupported schema error.
pub fn export_spf(model: &BimModel, profile: &IfcExportProfile) -> Result<String, IfcExportError> {
    model.validate()?;
    let schema = profile.schema.to_ascii_uppercase();
    if !matches!(schema.as_str(), "IFC4" | "IFC4X3") {
        return Err(IfcExportError::UnsupportedSchema(profile.schema.clone()));
    }

    let mut output = format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');\nFILE_NAME('{}','','',(''),(''),'{}','CADX');\nFILE_SCHEMA(('{}'));\nENDSEC;\nDATA;\n",
        escape(&model.project_name),
        escape(&profile.authoring_tool),
        schema
    );
    output.push_str("#1=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n");
    output.push_str("#2=IFCUNITASSIGNMENT((#1));\n");
    writeln!(
        output,
        "#3=IFCPROJECT('{}',$,'{}',$,$,$,$,$,#2);",
        stable_global_id(&model.project_id),
        escape(&model.project_name)
    )
    .map_err(|_| IfcExportError::Formatting)?;

    let mut entity_number = 4_u64;
    for storey in &model.storeys {
        writeln!(
            output,
            "#{entity_number}=IFCBUILDINGSTOREY('{}',$,'{}',$,$,$,$,$,.ELEMENT.,{});",
            stable_global_id(&storey.id),
            escape(&storey.name),
            storey.elevation_mm
        )
        .map_err(|_| IfcExportError::Formatting)?;
        entity_number += 1;
    }
    for element in &model.elements {
        writeln!(
            output,
            "#{entity_number}={}('{}',$,'{}',$,$,$,$,$);",
            element.class.ifc_name(),
            stable_global_id(&element.id),
            escape(&element.name)
        )
        .map_err(|_| IfcExportError::Formatting)?;
        entity_number += 1;
        if profile.export_property_sets {
            for attribute in &element.attributes {
                writeln!(
                    output,
                    "/* {}.{}={} */",
                    escape(attribute.property_set.as_deref().unwrap_or("Pset_CADX")),
                    escape(&attribute.name),
                    escape(&attribute.value.display_value())
                )
                .map_err(|_| IfcExportError::Formatting)?;
            }
        }
    }
    output.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok(output)
}

fn escape(value: &str) -> String {
    value.replace('\'', "''").replace(['\n', '\r'], " ")
}

fn stable_global_id(value: &str) -> String {
    const ALPHABET: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut result = String::with_capacity(22);
    let mut state = hash;
    for index in 0_u64..22 {
        if index > 0 && index % 10 == 0 {
            state = state.rotate_left(17) ^ hash.wrapping_mul(index + 1);
        }
        result.push(char::from(ALPHABET[(state & 63) as usize]));
        state = state.rotate_right(6) ^ 0x9e37_79b9_7f4a_7c15;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_a_deterministic_ifc4_project() {
        let model = BimModel::default();
        let first = export_spf(&model, &IfcExportProfile::default()).unwrap();
        let second = export_spf(&model, &IfcExportProfile::default()).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("FILE_SCHEMA(('IFC4'))"));
        assert!(first.contains("IFCBUILDINGSTOREY"));
    }
}
