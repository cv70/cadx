//! Aggregate AEC/BIM domain pack.

pub use cadx_aec_analysis as analysis;
pub use cadx_aec_bim as bim;
pub use cadx_aec_ifc as ifc;

use bim::{BimElement, BimElementClass, BimModel, Bounds3, BuildingStorey, SlabSpec, WallSpec};
use cadx_domain_api::{
    DomainAction, DomainAiTool, DomainArtifact, DomainArtifactKind, DomainContext, DomainExecution,
    DomainExecutionError, DomainFieldKind, DomainFieldSchema, DomainFieldValue, DomainId,
    DomainInspectorSchema, DomainIssue, DomainIssueSeverity, DomainManifest, DomainPack,
    DomainPanelSchema, DomainParameters, DomainRoute, DomainSelectOption, DomainShader,
    DomainShaderStage, DomainSolver, DomainSolverStage, DomainTool, DomainToolRequest,
    ExportFormat,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AecCapabilities {
    pub wall_and_slab_solids: bool,
    pub opening_solids: bool,
    pub bim_attributes: bool,
    pub level_management: bool,
    pub ifc_exchange: bool,
    pub clash_review: bool,
    pub quantity_takeoff: bool,
}

impl Default for AecCapabilities {
    fn default() -> Self {
        Self {
            wall_and_slab_solids: true,
            opening_solids: true,
            bim_attributes: true,
            level_management: true,
            ifc_exchange: true,
            clash_review: true,
            quantity_takeoff: true,
        }
    }
}

const TOOLS: [DomainTool; 10] = [
    DomainTool {
        id: "wall",
        label: "Wall",
        icon: "panel-top",
        category: "3D",
    },
    DomainTool {
        id: "slab",
        label: "Slab",
        icon: "square",
        category: "3D",
    },
    DomainTool {
        id: "opening",
        label: "Opening",
        icon: "door-open",
        category: "3D",
    },
    DomainTool {
        id: "levels",
        label: "Levels",
        icon: "layers",
        category: "BIM",
    },
    DomainTool {
        id: "space",
        label: "Space",
        icon: "cuboid",
        category: "BIM",
    },
    DomainTool {
        id: "bim-attrs",
        label: "BIM attributes",
        icon: "list-tree",
        category: "BIM",
    },
    DomainTool {
        id: "ifc",
        label: "IFC",
        icon: "file-output",
        category: "Export",
    },
    DomainTool {
        id: "schedule",
        label: "Schedule",
        icon: "table",
        category: "BIM",
    },
    DomainTool {
        id: "quantity-takeoff",
        label: "Quantity takeoff",
        icon: "calculator",
        category: "BIM",
    },
    DomainTool {
        id: "clash",
        label: "Clash review",
        icon: "scan-search",
        category: "Analysis",
    },
];

const IFC_OPTIONS: [DomainSelectOption; 2] = [
    DomainSelectOption {
        value: "IFC4",
        label: "IFC4",
    },
    DomainSelectOption {
        value: "IFC4X3",
        label: "IFC4x3",
    },
];

const CLASS_OPTIONS: [DomainSelectOption; 8] = [
    DomainSelectOption {
        value: "wall",
        label: "Wall",
    },
    DomainSelectOption {
        value: "slab",
        label: "Slab",
    },
    DomainSelectOption {
        value: "door",
        label: "Door",
    },
    DomainSelectOption {
        value: "window",
        label: "Window",
    },
    DomainSelectOption {
        value: "column",
        label: "Column",
    },
    DomainSelectOption {
        value: "beam",
        label: "Beam",
    },
    DomainSelectOption {
        value: "space",
        label: "Space",
    },
    DomainSelectOption {
        value: "proxy",
        label: "Proxy",
    },
];

const WALL_FIELDS: [DomainFieldSchema; 5] = [
    text_field("name", "Name", Some("AEC wall"), true),
    length_field("length_mm", "Length", "3000", true),
    length_field("thickness_mm", "Thickness", "200", true),
    length_field("height_mm", "Height", "2800", true),
    length_field("base_elevation_mm", "Base elevation", "0", true),
];

const SLAB_FIELDS: [DomainFieldSchema; 5] = [
    text_field("name", "Name", Some("AEC slab"), true),
    length_field("width_mm", "Width", "4000", true),
    length_field("depth_mm", "Depth", "3000", true),
    length_field("thickness_mm", "Thickness", "180", true),
    length_field("elevation_mm", "Elevation", "0", true),
];

const BIM_FIELDS: [DomainFieldSchema; 5] = [
    DomainFieldSchema {
        id: "ifc_class",
        label: "IFC class",
        kind: DomainFieldKind::Select,
        default_value: Some("wall"),
        unit: None,
        options: &CLASS_OPTIONS,
        required: true,
    },
    text_field("storey", "Storey", Some("Level 1"), true),
    text_field("type_mark", "Type mark", Some("Generic"), false),
    text_field("fire_rating", "Fire rating", None, false),
    DomainFieldSchema {
        id: "load_bearing",
        label: "Load bearing",
        kind: DomainFieldKind::Boolean,
        default_value: Some("false"),
        unit: None,
        options: &[],
        required: false,
    },
];

const IFC_FIELDS: [DomainFieldSchema; 3] = [
    DomainFieldSchema {
        id: "ifc_schema",
        label: "IFC schema",
        kind: DomainFieldKind::Select,
        default_value: Some("IFC4"),
        unit: None,
        options: &IFC_OPTIONS,
        required: true,
    },
    DomainFieldSchema {
        id: "export_property_sets",
        label: "Export property sets",
        kind: DomainFieldKind::Boolean,
        default_value: Some("true"),
        unit: None,
        options: &[],
        required: true,
    },
    length_field("clash_tolerance_mm", "Clash tolerance", "0.1", true),
];

const PANELS: [DomainPanelSchema; 4] = [
    DomainPanelSchema {
        id: "aec_wall",
        label: "Wall",
        fields: &WALL_FIELDS,
    },
    DomainPanelSchema {
        id: "aec_slab",
        label: "Slab",
        fields: &SLAB_FIELDS,
    },
    DomainPanelSchema {
        id: "aec_bim_identity",
        label: "BIM identity",
        fields: &BIM_FIELDS,
    },
    DomainPanelSchema {
        id: "aec_ifc_export",
        label: "IFC and coordination",
        fields: &IFC_FIELDS,
    },
];

const SOLVERS: [DomainSolver; 5] = [
    DomainSolver {
        id: "aec-wall-axis",
        label: "Wall axis solver",
        stage: DomainSolverStage::Modeling,
        description: "Builds wall solids from axis, thickness, height, and level",
        inputs: &["wall_axis", "wall_type", "level"],
        outputs: &["solid_actions", "bim_metadata"],
    },
    DomainSolver {
        id: "aec-slab-boundary",
        label: "Slab boundary solver",
        stage: DomainSolverStage::Modeling,
        description: "Builds slab solids from closed boundaries and offsets",
        inputs: &["slab_boundary", "slab_type", "level"],
        outputs: &["solid_actions", "bim_metadata"],
    },
    DomainSolver {
        id: "aec-opening-host",
        label: "Hosted opening solver",
        stage: DomainSolverStage::Constraint,
        description: "Keeps doors, windows, and openings attached to hosts",
        inputs: &["host", "opening", "offsets"],
        outputs: &["void_action", "host_relation"],
    },
    DomainSolver {
        id: "aec-clash",
        label: "Spatial clash analysis",
        stage: DomainSolverStage::Analysis,
        description: "Checks bounded BIM elements with deterministic broad phase",
        inputs: &["bim_bounds", "tolerance"],
        outputs: &["clash_report"],
    },
    DomainSolver {
        id: "aec-ifc-export",
        label: "IFC exchange",
        stage: DomainSolverStage::Export,
        description: "Validates BIM identity and emits deterministic IFC4/IFC4X3 SPF",
        inputs: &["bim_model", "ifc_profile"],
        outputs: &["ifc_spf"],
    },
];

const SHADERS: [DomainShader; 3] = [
    DomainShader {
        id: "aec-category-color",
        label: "BIM category colors",
        stage: DomainShaderStage::Render,
        entry_point: "aec_category_color",
        description: "Colors elements by BIM class and discipline",
    },
    DomainShader {
        id: "aec-space-overlay",
        label: "Space overlay",
        stage: DomainShaderStage::Overlay,
        entry_point: "aec_space_overlay",
        description: "Displays space boundaries, names, levels, and occupancy",
    },
    DomainShader {
        id: "aec-clash-overlay",
        label: "Clash overlay",
        stage: DomainShaderStage::Overlay,
        entry_point: "aec_clash_overlay",
        description: "Highlights coordinated element overlaps",
    },
];

const AI_TOOLS: [DomainAiTool; 5] = [
    DomainAiTool {
        id: "aec_create_wall",
        label: "Create wall",
        description: "Create a wall solid with BIM metadata",
        schema_id: "cadx.domain.aec.create_wall.v1",
    },
    DomainAiTool {
        id: "aec_create_slab",
        label: "Create slab",
        description: "Create a slab solid with BIM metadata",
        schema_id: "cadx.domain.aec.create_slab.v1",
    },
    DomainAiTool {
        id: "aec_update_bim_properties",
        label: "Update BIM properties",
        description: "Apply IFC class and property-set values",
        schema_id: "cadx.domain.aec.update_bim_properties.v1",
    },
    DomainAiTool {
        id: "aec_run_clash",
        label: "Run clash review",
        description: "Run spatial coordination checks",
        schema_id: "cadx.domain.aec.run_clash.v1",
    },
    DomainAiTool {
        id: "aec_export_ifc",
        label: "Export IFC",
        description: "Validate and export IFC4/IFC4X3",
        schema_id: "cadx.domain.aec.export_ifc.v1",
    },
];

const fn text_field(
    id: &'static str,
    label: &'static str,
    default_value: Option<&'static str>,
    required: bool,
) -> DomainFieldSchema {
    DomainFieldSchema {
        id,
        label,
        kind: DomainFieldKind::Text,
        default_value,
        unit: None,
        options: &[],
        required,
    }
}

const fn length_field(
    id: &'static str,
    label: &'static str,
    default_value: &'static str,
    required: bool,
) -> DomainFieldSchema {
    DomainFieldSchema {
        id,
        label,
        kind: DomainFieldKind::LengthMm,
        default_value: Some(default_value),
        unit: Some("mm"),
        options: &[],
        required,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AecPack;

impl DomainPack for AecPack {
    fn manifest(&self) -> DomainManifest {
        DomainManifest {
            id: DomainId::Aec,
            name: "AEC/BIM",
            version: "0.2",
            description: "BIM elements, parametric walls/slabs, coordination and IFC exchange",
            priority: 10,
        }
    }

    fn tools(&self) -> &'static [DomainTool] {
        &TOOLS
    }

    fn inspector_schema(&self) -> DomainInspectorSchema {
        DomainInspectorSchema { panels: &PANELS }
    }

    fn tool_panel(&self, tool_id: &str) -> Option<DomainPanelSchema> {
        match tool_id {
            "wall" => Some(PANELS[0]),
            "slab" => Some(PANELS[1]),
            "bim-attrs" => Some(PANELS[2]),
            "ifc" => Some(PANELS[3]),
            _ => None,
        }
    }

    fn solvers(&self) -> &'static [DomainSolver] {
        &SOLVERS
    }

    fn shaders(&self) -> &'static [DomainShader] {
        &SHADERS
    }

    fn ai_tools(&self) -> &'static [DomainAiTool] {
        &AI_TOOLS
    }

    fn execute_tool(
        &self,
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

    fn route_natural_language(&self, input: &str, _context: &DomainContext) -> DomainRoute {
        let normalized = input.to_ascii_lowercase();
        let (action, confidence) = if normalized.contains("ifc") || input.contains("交换") {
            (
                DomainAction::Export {
                    format: "ifc".into(),
                },
                0.96,
            )
        } else if normalized.contains("clash") || input.contains("碰撞") {
            (
                DomainAction::RunCheck {
                    check: "clash".into(),
                },
                0.94,
            )
        } else if normalized.contains("schedule") || input.contains("明细表") {
            (DomainAction::GenerateBom, 0.9)
        } else if normalized.contains("slab") || input.contains("楼板") {
            let spec = SlabSpec {
                width_mm: 4_000.0,
                depth_mm: 3_000.0,
                thickness_mm: 180.0,
                elevation_mm: 0.0,
            };
            (
                DomainAction::CreateSolidBox {
                    name: "AEC slab".into(),
                    size_mm: spec.box_size_mm(),
                    position_mm: [-2_000.0, -1_500.0, spec.elevation_mm],
                },
                0.92,
            )
        } else if normalized.contains("wall") || input.contains('墙') {
            let spec = WallSpec {
                length_mm: 3_000.0,
                thickness_mm: 200.0,
                height_mm: 2_800.0,
                base_elevation_mm: 0.0,
            };
            (
                DomainAction::CreateSolidBox {
                    name: "AEC wall".into(),
                    size_mm: spec.box_size_mm(),
                    position_mm: [-1_500.0, -100.0, spec.base_elevation_mm],
                },
                0.93,
            )
        } else {
            (
                DomainAction::OpenPanel {
                    panel: "bim-attrs".into(),
                },
                0.35,
            )
        };
        DomainRoute {
            action,
            confidence,
            rationale: "AEC intent routing across BIM, geometry, coordination, and IFC".into(),
        }
    }

    fn validate_export(&self, format: ExportFormat, context: &DomainContext) -> Vec<DomainIssue> {
        match format {
            ExportFormat::Ifc if context.active_feature_count == 0 => vec![DomainIssue {
                code: "EMPTY_IFC_MODEL".into(),
                severity: DomainIssueSeverity::Error,
                message: "IFC export requires at least one document feature".into(),
            }],
            ExportFormat::Gerber | ExportFormat::Drill => vec![DomainIssue {
                code: "WRONG_DOMAIN_FORMAT".into(),
                severity: DomainIssueSeverity::Error,
                message: "Gerber and drill exports belong to the ECAD pack".into(),
            }],
            _ => Vec::new(),
        }
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
    use super::*;

    fn context() -> DomainContext {
        DomainContext {
            document_name: "Building".into(),
            selected_feature_ids: vec![7],
            visible_solid_count: 1,
            active_feature_count: 1,
            selected_feature_name: Some("Core wall".into()),
            spatial_entities: vec![cadx_domain_api::DomainSpatialEntity {
                feature_id: 7,
                name: "Core wall".into(),
                minimum_mm: [0.0; 3],
                maximum_mm: [3_000.0, 200.0, 2_800.0],
            }],
        }
    }

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

    #[test]
    fn aec_route_does_not_claim_unrelated_prompts() {
        let route = AecPack.route_natural_language("make a gear", &context());
        assert!(route.confidence < 0.5);
    }
}
