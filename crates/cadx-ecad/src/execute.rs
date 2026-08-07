//! ECAD tool execution and artifact helpers.

use crate::catalog::{FOOTPRINTS, PANELS};
use crate::netlist;
use cadx_domain_api::{
    DomainAction, DomainArtifact, DomainArtifactKind, DomainExecution, DomainExecutionError,
    DomainFieldValue, DomainId, DomainIssue, DomainIssueSeverity, DomainParameters,
    DomainToolRequest,
};
use serde::Serialize;

pub(crate) fn execute_tool(
    request: &DomainToolRequest,
) -> Result<DomainExecution, DomainExecutionError> {
    match request.tool_id.as_str() {
        "board" => {
            let values = PANELS[0]
                .resolve_parameters(&request.parameters)
                .map_err(DomainExecutionError::InvalidParameters)?;
            let width = decimal(&values, "board_width_mm");
            let height = decimal(&values, "board_height_mm");
            let thickness = decimal(&values, "board_thickness_mm");
            let layers = values["layer_count"]
                .as_text()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(4);
            if width <= 0.0 || height <= 0.0 || thickness <= 0.0 {
                return Err(invalid_parameters("Board dimensions must be positive"));
            }
            Ok(DomainExecution::with_action(
                "Create ECAD board and stackup",
                DomainAction::CreatePcbBoard {
                    name: "ECAD board".into(),
                    width_mm: width,
                    height_mm: height,
                    thickness_mm: thickness,
                    layers,
                },
            ))
        }
        "placement" | "3d-link" => {
            let values = PANELS[2]
                .resolve_parameters(&request.parameters)
                .map_err(DomainExecutionError::InvalidParameters)?;
            Ok(DomainExecution::with_action(
                "Place ECAD component and linked package",
                DomainAction::PlacePcbComponent {
                    reference: text(&values, "reference", "U1").into(),
                    value: text(&values, "value", "MCU").into(),
                    footprint: text(&values, "footprint", "QFN-32").into(),
                    position_mm: [
                        decimal(&values, "position_x_mm"),
                        decimal(&values, "position_y_mm"),
                    ],
                    rotation_deg: decimal(&values, "rotation_deg"),
                    side: text(&values, "side", "top").into(),
                    model_3d: values
                        .get("linked_3d_model")
                        .and_then(DomainFieldValue::as_text)
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string),
                },
            ))
        }
        "footprint-library" => json_artifact(
            "ECAD footprint library",
            "footprints.json",
            DomainArtifactKind::Report,
            &FOOTPRINTS,
        ),
        "schematic" | "netlist" => {
            let example = netlist::Netlist {
                components: vec![netlist::SchematicComponent {
                    reference: "U1".into(),
                    value: "MCU".into(),
                    footprint: "QFN-32".into(),
                    pins: vec!["1".into(), "2".into()],
                }],
                nets: vec![netlist::ElectricalNet {
                    name: "GND".into(),
                    class: "POWER".into(),
                    pins: vec![netlist::PinRef {
                        reference: "U1".into(),
                        pin: "1".into(),
                    }],
                    impedance_ohms: None,
                }],
            };
            example
                .validate()
                .map_err(|error| DomainExecutionError::ToolFailed(error.to_string()))?;
            json_artifact(
                "Validated ECAD netlist",
                "netlist.json",
                DomainArtifactKind::Report,
                &example,
            )
        }
        "drc" => Ok(DomainExecution::with_action(
            "Run electrical design-rule checks",
            DomainAction::RunCheck {
                check: "drc".into(),
            },
        )),
        "gerber" => Ok(DomainExecution::with_action(
            "Generate Gerber and drill manufacturing bundle",
            DomainAction::Export {
                format: "gerber".into(),
            },
        )),
        "bom" => Ok(DomainExecution::with_action(
            "Generate ECAD bill of materials",
            DomainAction::GenerateBom,
        )),
        "routing" | "diff-pair" | "via" | "stackup" => Ok(DomainExecution::with_action(
            format!("Open ECAD {}", request.tool_id),
            DomainAction::OpenPanel {
                panel: request.tool_id.clone(),
            },
        )),
        tool_id => Err(DomainExecutionError::UnknownTool {
            domain: DomainId::Ecad,
            tool_id: tool_id.into(),
        }),
    }
}

fn decimal(values: &DomainParameters, id: &str) -> f64 {
    values
        .get(id)
        .and_then(DomainFieldValue::as_decimal)
        .unwrap_or_default()
}

fn text<'a>(values: &'a DomainParameters, id: &str, fallback: &'a str) -> &'a str {
    values
        .get(id)
        .and_then(DomainFieldValue::as_text)
        .unwrap_or(fallback)
}

fn invalid_parameters(message: &str) -> DomainExecutionError {
    DomainExecutionError::InvalidParameters(vec![DomainIssue {
        code: "INVALID_PARAMETER.geometry".into(),
        severity: DomainIssueSeverity::Error,
        message: message.into(),
    }])
}

fn json_artifact(
    summary: &str,
    name: &str,
    kind: DomainArtifactKind,
    value: &impl Serialize,
) -> Result<DomainExecution, DomainExecutionError> {
    let contents = serde_json::to_string_pretty(value)
        .map_err(|error| DomainExecutionError::ToolFailed(error.to_string()))?;
    Ok(DomainExecution {
        summary: summary.into(),
        artifacts: vec![DomainArtifact {
            name: name.into(),
            media_type: "application/json".into(),
            kind,
            contents,
        }],
        ..DomainExecution::default()
    })
}

#[cfg(test)]
mod tests {
    use crate::EcadPack;
    use cadx_domain_api::{DomainAction, DomainContext, DomainPack, DomainToolRequest};

    #[test]
    fn board_tool_resolves_stackup_parameters() {
        let execution = EcadPack
            .execute_tool(&DomainToolRequest::new("board", DomainContext::default()))
            .unwrap();
        assert!(matches!(
            execution.actions[0],
            DomainAction::CreatePcbBoard {
                width_mm: 80.0,
                height_mm: 50.0,
                layers: 4,
                ..
            }
        ));
    }
}
