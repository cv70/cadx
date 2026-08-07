//! AEC tool execution, BIM model derivation, and artifact helpers.

use crate::catalog::PANELS;
use crate::{bim, ifc};
use bim::{BimElement, BimElementClass, BimModel, Bounds3, BuildingStorey, SlabSpec, WallSpec};
use cadx_domain_api::{
    DomainAction, DomainArtifact, DomainArtifactKind, DomainContext, DomainExecution,
    DomainExecutionError, DomainFieldValue, DomainId, DomainIssue, DomainIssueSeverity,
    DomainParameters, DomainToolRequest,
};

pub(crate) fn execute_tool(
    request: &DomainToolRequest,
) -> Result<DomainExecution, DomainExecutionError> {
    match request.tool_id.as_str() {
        "wall" => execute_wall(request),
        "slab" => execute_slab(request),
        "ifc" => execute_ifc(request),
        "schedule" => json_artifact(
            "AEC element schedule",
            "schedule.json",
            DomainArtifactKind::Schedule,
            &model_from_context(&request.context).schedule(None),
        ),
        "quantity-takeoff" => json_artifact(
            "AEC quantity takeoff",
            "quantity-takeoff.json",
            DomainArtifactKind::Report,
            &model_from_context(&request.context).quantity_takeoff(),
        ),
        "clash" => Ok(DomainExecution::with_action(
            "Run AEC clash review",
            DomainAction::RunCheck {
                check: "clash".into(),
            },
        )),
        "bim-attrs" => {
            let values = PANELS[2]
                .resolve_parameters(&request.parameters)
                .map_err(DomainExecutionError::InvalidParameters)?;
            let entity_key = request.context.selected_feature_ids.first().map_or_else(
                || request.context.document_name.clone(),
                |id| format!("feature-{id}"),
            );
            Ok(DomainExecution::with_action(
                "Update BIM identity and property values",
                DomainAction::UpsertDomainMetadata {
                    entity_key,
                    namespace: "aec.bim".into(),
                    values,
                },
            ))
        }
        "opening" | "levels" | "space" => Ok(DomainExecution::with_action(
            format!("Open AEC {}", request.tool_id),
            DomainAction::OpenPanel {
                panel: request.tool_id.clone(),
            },
        )),
        tool_id => Err(DomainExecutionError::UnknownTool {
            domain: DomainId::Aec,
            tool_id: tool_id.into(),
        }),
    }
}

fn execute_wall(request: &DomainToolRequest) -> Result<DomainExecution, DomainExecutionError> {
    let values = PANELS[0]
        .resolve_parameters(&request.parameters)
        .map_err(DomainExecutionError::InvalidParameters)?;
    let spec = WallSpec {
        length_mm: decimal(&values, "length_mm"),
        thickness_mm: decimal(&values, "thickness_mm"),
        height_mm: decimal(&values, "height_mm"),
        base_elevation_mm: decimal(&values, "base_elevation_mm"),
    };
    if !spec.is_valid() {
        return Err(invalid_geometry(
            "Wall dimensions must be finite and positive",
        ));
    }
    let name = text(&values, "name", "AEC wall").to_string();
    Ok(DomainExecution {
        summary: format!("Create BIM wall {name}"),
        actions: vec![
            DomainAction::CreateSolidBox {
                name: name.clone(),
                size_mm: spec.box_size_mm(),
                position_mm: [
                    -spec.length_mm * 0.5,
                    -spec.thickness_mm * 0.5,
                    spec.base_elevation_mm,
                ],
            },
            metadata_action(&name, "IFCWALL", values),
        ],
        ..DomainExecution::default()
    })
}

fn execute_slab(request: &DomainToolRequest) -> Result<DomainExecution, DomainExecutionError> {
    let values = PANELS[1]
        .resolve_parameters(&request.parameters)
        .map_err(DomainExecutionError::InvalidParameters)?;
    let spec = SlabSpec {
        width_mm: decimal(&values, "width_mm"),
        depth_mm: decimal(&values, "depth_mm"),
        thickness_mm: decimal(&values, "thickness_mm"),
        elevation_mm: decimal(&values, "elevation_mm"),
    };
    if !spec.is_valid() {
        return Err(invalid_geometry(
            "Slab dimensions must be finite and positive",
        ));
    }
    let name = text(&values, "name", "AEC slab").to_string();
    Ok(DomainExecution {
        summary: format!("Create BIM slab {name}"),
        actions: vec![
            DomainAction::CreateSolidBox {
                name: name.clone(),
                size_mm: spec.box_size_mm(),
                position_mm: [
                    -spec.width_mm * 0.5,
                    -spec.depth_mm * 0.5,
                    spec.elevation_mm,
                ],
            },
            metadata_action(&name, "IFCSLAB", values),
        ],
        ..DomainExecution::default()
    })
}

fn execute_ifc(request: &DomainToolRequest) -> Result<DomainExecution, DomainExecutionError> {
    let values = PANELS[3]
        .resolve_parameters(&request.parameters)
        .map_err(DomainExecutionError::InvalidParameters)?;
    let profile = ifc::IfcExportProfile {
        schema: text(&values, "ifc_schema", "IFC4").into(),
        authoring_tool: "CADX".into(),
        length_unit: "MILLI METRE".into(),
        export_property_sets: values["export_property_sets"].as_boolean().unwrap_or(true),
    };
    let contents = ifc::export_spf(&model_from_context(&request.context), &profile)
        .map_err(|error| DomainExecutionError::ToolFailed(error.to_string()))?;
    Ok(DomainExecution {
        summary: format!("Export {} model", profile.schema),
        artifacts: vec![DomainArtifact {
            name: "model.ifc".into(),
            media_type: "application/x-step".into(),
            kind: DomainArtifactKind::Exchange,
            contents,
        }],
        ..DomainExecution::default()
    })
}

fn model_from_context(context: &DomainContext) -> BimModel {
    let mut model = BimModel {
        project_id: format!("cadx-{}", context.document_name),
        project_name: context.document_name.clone(),
        storeys: vec![BuildingStorey {
            id: "level-1".into(),
            name: "Level 1".into(),
            elevation_mm: 0.0,
            height_mm: 2_800.0,
        }],
        elements: Vec::new(),
    };
    model.elements = context
        .spatial_entities
        .iter()
        .map(|entity| BimElement {
            id: format!("feature-{}", entity.feature_id),
            name: entity.name.clone(),
            class: BimElementClass::Proxy,
            storey_id: "level-1".into(),
            attributes: Vec::new(),
            bounds: Some(Bounds3 {
                minimum_mm: entity.minimum_mm,
                maximum_mm: entity.maximum_mm,
            }),
            linked_feature_id: Some(entity.feature_id),
        })
        .collect();
    if model.elements.is_empty() {
        model.elements = context
            .selected_feature_ids
            .iter()
            .map(|id| BimElement {
                id: format!("feature-{id}"),
                name: context
                    .selected_feature_name
                    .clone()
                    .unwrap_or_else(|| format!("CADX feature {id}")),
                class: BimElementClass::Proxy,
                storey_id: "level-1".into(),
                attributes: Vec::new(),
                bounds: None,
                linked_feature_id: Some(*id),
            })
            .collect();
    }
    model
}

fn metadata_action(
    entity_key: &str,
    ifc_class: &str,
    mut values: DomainParameters,
) -> DomainAction {
    values.insert("ifc_class".into(), DomainFieldValue::Text(ifc_class.into()));
    DomainAction::UpsertDomainMetadata {
        entity_key: entity_key.into(),
        namespace: "aec.bim".into(),
        values,
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

fn invalid_geometry(message: &str) -> DomainExecutionError {
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
    value: &impl serde::Serialize,
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
    use super::bim;
    use crate::AecPack;
    use crate::tests::context;
    use cadx_domain_api::{DomainAction, DomainPack, DomainToolRequest};

    #[test]
    fn wall_tool_returns_geometry_and_bim_metadata_atomically() {
        let result = AecPack
            .execute_tool(&DomainToolRequest::new("wall", context()))
            .unwrap();
        assert_eq!(result.actions.len(), 2);
        assert!(matches!(
            result.actions[0],
            DomainAction::CreateSolidBox { .. }
        ));
        assert!(matches!(
            result.actions[1],
            DomainAction::UpsertDomainMetadata { .. }
        ));
    }

    #[test]
    fn ifc_tool_emits_exchange_artifact() {
        let result = AecPack
            .execute_tool(&DomainToolRequest::new("ifc", context()))
            .unwrap();
        assert!(
            result.artifacts[0]
                .contents
                .contains("IFCBUILDINGELEMENTPROXY")
        );
    }

    #[test]
    fn quantity_tool_uses_spatial_context_bounds() {
        let result = AecPack
            .execute_tool(&DomainToolRequest::new("quantity-takeoff", context()))
            .unwrap();
        let takeoff: bim::QuantityTakeoff =
            serde_json::from_str(&result.artifacts[0].contents).unwrap();
        let expected = 3_000.0 * 200.0 * 2_800.0;
        assert!((takeoff.gross_volume_mm3 - expected).abs() < f64::EPSILON);
    }
}
