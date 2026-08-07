//! MCAD tool execution and artifact helpers.

use crate::catalog::{ASSEMBLY_TEMPLATES, PANELS, STANDARD_PARTS};
use cadx_domain_api::{
    DomainAction, DomainArtifact, DomainArtifactKind, DomainExecution, DomainExecutionError,
    DomainId, DomainIssue, DomainIssueSeverity, DomainToolRequest,
};
use serde::Serialize;

pub(crate) fn execute_tool(
    request: &DomainToolRequest,
) -> Result<DomainExecution, DomainExecutionError> {
    match request.tool_id.as_str() {
        "sketch" => Ok(DomainExecution::with_action(
            "Open MCAD sketch tools",
            DomainAction::OpenPanel {
                panel: "sketch".into(),
            },
        )),
        "extrude" | "ai-part" => execute_extrusion(request),
        "standard-parts" => {
            let part = &STANDARD_PARTS[0];
            let radius = part.parameters[0].1 * 0.5;
            let height = part.parameters[1].1;
            Ok(DomainExecution::with_action(
                format!("Create {}", part.name),
                DomainAction::CreateSolidCylinder {
                    name: part.name.into(),
                    radius_mm: radius,
                    height_mm: height,
                    position_mm: [0.0; 3],
                },
            ))
        }
        "feature-tree" => artifact_execution(
            "MCAD feature regeneration context",
            "feature-tree.json",
            DomainArtifactKind::Report,
            &request.context.selected_feature_ids,
        ),
        "assembly" => artifact_execution(
            "MCAD assembly constraint templates",
            "assembly-mates.json",
            DomainArtifactKind::Report,
            &ASSEMBLY_TEMPLATES,
        ),
        "drawing" | "standards-check" => Ok(DomainExecution::with_action(
            "Run engineering drawing standards check",
            DomainAction::RunCheck {
                check: "drawing".into(),
            },
        )),
        "edge-modifiers" => Ok(DomainExecution::with_action(
            "Open chamfer and fillet tools",
            DomainAction::OpenPanel {
                panel: "edge-modifiers".into(),
            },
        )),
        "interference" => Ok(DomainExecution::with_action(
            "Run assembly interference analysis",
            DomainAction::RunCheck {
                check: "interference".into(),
            },
        )),
        "dfm" => Ok(DomainExecution::with_action(
            "Run manufacturability review",
            DomainAction::RunCheck {
                check: "dfm".into(),
            },
        )),
        "bom" => Ok(DomainExecution::with_action(
            "Generate mechanical BOM",
            DomainAction::GenerateBom,
        )),
        tool_id => Err(DomainExecutionError::UnknownTool {
            domain: DomainId::Mcad,
            tool_id: tool_id.into(),
        }),
    }
}

fn execute_extrusion(request: &DomainToolRequest) -> Result<DomainExecution, DomainExecutionError> {
    let values = PANELS[0]
        .resolve_parameters(&request.parameters)
        .map_err(DomainExecutionError::InvalidParameters)?;
    let name = values["name"].as_text().unwrap_or("MCAD extrusion");
    let width = values["width_mm"].as_decimal().unwrap_or_default();
    let depth = values["depth_mm"].as_decimal().unwrap_or_default();
    let height = values["height_mm"].as_decimal().unwrap_or_default();
    if width <= 0.0 || depth <= 0.0 || height <= 0.0 {
        return Err(DomainExecutionError::InvalidParameters(vec![DomainIssue {
            code: "INVALID_PARAMETER.feature_dimensions".into(),
            severity: DomainIssueSeverity::Error,
            message: "Feature dimensions must be positive".into(),
        }]));
    }
    Ok(DomainExecution::with_action(
        format!("Create {name}"),
        DomainAction::CreateProfileExtrusion {
            name: name.into(),
            profile_mm: vec![
                [-width * 0.5, -depth * 0.5],
                [width * 0.5, -depth * 0.5],
                [width * 0.5, depth * 0.5],
                [-width * 0.5, depth * 0.5],
            ],
            height_mm: height,
            position_mm: [0.0; 3],
        },
    ))
}

fn artifact_execution(
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
    use crate::McadPack;
    use cadx_domain_api::{DomainAction, DomainContext, DomainPack, DomainToolRequest};

    #[test]
    fn extrude_tool_uses_schema_defaults() {
        let execution = McadPack
            .execute_tool(&DomainToolRequest::new("extrude", DomainContext::default()))
            .unwrap();
        assert!(matches!(
            &execution.actions[0],
            DomainAction::CreateProfileExtrusion {
                height_mm,
                profile_mm,
                ..
            } if (*height_mm - 10.0).abs() < f64::EPSILON && profile_mm.len() == 4
        ));
    }
}
