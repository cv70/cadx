//! Aggregated Mechanical domain pack.
//!
//! The sub-crates remain independently usable so future workbenches can ship
//! only standards, DFM, or BOM functionality as needed.

pub use cadx_mcad_bom as bom;
pub use cadx_mcad_dfm as dfm;
pub use cadx_mcad_model as model;
pub use cadx_mcad_standards as standards;

use cadx_domain_api::{
    DomainAction, DomainAiTool, DomainArtifact, DomainArtifactKind, DomainContext, DomainExecution,
    DomainExecutionError, DomainFieldKind, DomainFieldSchema, DomainId, DomainInspectorSchema,
    DomainIssue, DomainIssueSeverity, DomainManifest, DomainPack, DomainPanelSchema, DomainRoute,
    DomainSelectOption, DomainShader, DomainShaderStage, DomainSolver, DomainSolverStage,
    DomainTool, DomainToolRequest, ExportFormat,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct StandardPartDescriptor {
    pub id: &'static str,
    pub family: &'static str,
    pub standard: &'static str,
    pub name: &'static str,
    pub default_material: &'static str,
    pub parameters: &'static [(&'static str, f64)],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AssemblyConstraintTemplate {
    pub id: &'static str,
    pub label: &'static str,
    pub mate_kind: &'static str,
    pub degrees_of_freedom: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct McadCapabilities {
    pub feature_tree: bool,
    pub sketch_and_feature_tools: bool,
    pub engineering_drawing: bool,
    pub tolerance_and_fit_tables: bool,
    pub standard_parts: bool,
    pub assembly_constraints: bool,
    pub interference_check: bool,
    pub dfm_review: bool,
    pub ai_natural_language_parts: bool,
    pub bom: bool,
}

impl Default for McadCapabilities {
    fn default() -> Self {
        Self {
            feature_tree: true,
            sketch_and_feature_tools: true,
            engineering_drawing: true,
            tolerance_and_fit_tables: true,
            standard_parts: true,
            assembly_constraints: true,
            interference_check: true,
            dfm_review: true,
            ai_natural_language_parts: true,
            bom: true,
        }
    }
}

const TOOLS: [DomainTool; 12] = [
    DomainTool {
        id: "feature-tree",
        label: "Feature tree",
        icon: "list-tree",
        category: "MCAD",
    },
    DomainTool {
        id: "sketch",
        label: "Sketch",
        icon: "pencil-ruler",
        category: "2D",
    },
    DomainTool {
        id: "extrude",
        label: "Extrude",
        icon: "box",
        category: "3D",
    },
    DomainTool {
        id: "edge-modifiers",
        label: "Chamfer / fillet",
        icon: "corner-down-right",
        category: "3D",
    },
    DomainTool {
        id: "drawing",
        label: "Engineering drawing",
        icon: "pencil",
        category: "2D",
    },
    DomainTool {
        id: "standards-check",
        label: "Standards check",
        icon: "scan-search",
        category: "2D",
    },
    DomainTool {
        id: "standard-parts",
        label: "Standard parts",
        icon: "boxes",
        category: "3D",
    },
    DomainTool {
        id: "assembly",
        label: "Assembly constraints",
        icon: "boxes",
        category: "3D",
    },
    DomainTool {
        id: "interference",
        label: "Interference",
        icon: "scan-search",
        category: "3D",
    },
    DomainTool {
        id: "dfm",
        label: "DFM review",
        icon: "triangle-alert",
        category: "AI",
    },
    DomainTool {
        id: "bom",
        label: "BOM",
        icon: "layers",
        category: "AI",
    },
    DomainTool {
        id: "ai-part",
        label: "Natural-language part",
        icon: "sparkles",
        category: "AI",
    },
];

const PROCESS_OPTIONS: [DomainSelectOption; 4] = [
    DomainSelectOption {
        value: "milling",
        label: "Milling",
    },
    DomainSelectOption {
        value: "turning",
        label: "Turning",
    },
    DomainSelectOption {
        value: "sheet_metal",
        label: "Sheet metal",
    },
    DomainSelectOption {
        value: "additive",
        label: "Additive",
    },
];

const MATE_OPTIONS: [DomainSelectOption; 3] = [
    DomainSelectOption {
        value: "fixed",
        label: "Fixed",
    },
    DomainSelectOption {
        value: "revolute",
        label: "Revolute",
    },
    DomainSelectOption {
        value: "slider",
        label: "Slider",
    },
];

const PART_FIELDS: [DomainFieldSchema; 5] = [
    DomainFieldSchema {
        id: "part_number",
        label: "Part number",
        kind: DomainFieldKind::Text,
        default_value: Some("MCAD-001"),
        unit: None,
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "material",
        label: "Material",
        kind: DomainFieldKind::Text,
        default_value: Some("Aluminum 6061"),
        unit: None,
        options: &[],
        required: false,
    },
    DomainFieldSchema {
        id: "manufacturing_process",
        label: "Manufacturing process",
        kind: DomainFieldKind::Select,
        default_value: Some("milling"),
        unit: None,
        options: &PROCESS_OPTIONS,
        required: true,
    },
    DomainFieldSchema {
        id: "default_tolerance_mm",
        label: "Default tolerance",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("0.05"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "feature_regeneration",
        label: "Feature regeneration",
        kind: DomainFieldKind::Boolean,
        default_value: Some("true"),
        unit: None,
        options: &[],
        required: true,
    },
];

const EDGE_FIELDS: [DomainFieldSchema; 2] = [
    DomainFieldSchema {
        id: "default_chamfer_mm",
        label: "Default chamfer",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("1.0"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "default_fillet_mm",
        label: "Default fillet",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("2.0"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
];

const ASSEMBLY_FIELDS: [DomainFieldSchema; 3] = [
    DomainFieldSchema {
        id: "mate_kind",
        label: "Mate kind",
        kind: DomainFieldKind::Select,
        default_value: Some("fixed"),
        unit: None,
        options: &MATE_OPTIONS,
        required: true,
    },
    DomainFieldSchema {
        id: "limit_min",
        label: "Lower limit",
        kind: DomainFieldKind::Decimal,
        default_value: None,
        unit: None,
        options: &[],
        required: false,
    },
    DomainFieldSchema {
        id: "limit_max",
        label: "Upper limit",
        kind: DomainFieldKind::Decimal,
        default_value: None,
        unit: None,
        options: &[],
        required: false,
    },
];

const FEATURE_FIELDS: [DomainFieldSchema; 4] = [
    DomainFieldSchema {
        id: "name",
        label: "Feature name",
        kind: DomainFieldKind::Text,
        default_value: Some("MCAD extrusion"),
        unit: None,
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "width_mm",
        label: "Width",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("40"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "depth_mm",
        label: "Depth",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("30"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "height_mm",
        label: "Extrusion height",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("10"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
];

const PANELS: [DomainPanelSchema; 4] = [
    DomainPanelSchema {
        id: "mcad_feature",
        label: "Parametric feature",
        fields: &FEATURE_FIELDS,
    },
    DomainPanelSchema {
        id: "mcad_part",
        label: "MCAD part",
        fields: &PART_FIELDS,
    },
    DomainPanelSchema {
        id: "mcad_edge_modifiers",
        label: "Edge modifiers",
        fields: &EDGE_FIELDS,
    },
    DomainPanelSchema {
        id: "mcad_assembly",
        label: "Assembly constraints",
        fields: &ASSEMBLY_FIELDS,
    },
];

const SOLVERS: [DomainSolver; 4] = [
    DomainSolver {
        id: "mcad-feature-regeneration",
        label: "Feature regeneration",
        stage: DomainSolverStage::Modeling,
        description: "Orders parametric features and rebuilds impacted dependents",
        inputs: &["feature_tree", "dirty_marks"],
        outputs: &["model_commands"],
    },
    DomainSolver {
        id: "mcad-sketch-constraints",
        label: "Sketch constraints",
        stage: DomainSolverStage::Constraint,
        description: "Routes sketch constraints to projection or nonlinear solve",
        inputs: &["sketch_region", "constraints"],
        outputs: &["solved_region", "dof_report"],
    },
    DomainSolver {
        id: "mcad-assembly-mates",
        label: "Assembly mates",
        stage: DomainSolverStage::Constraint,
        description: "Evaluates fixed, revolute, and slider mate templates",
        inputs: &["occurrence_frames", "mate_state"],
        outputs: &["assembly_transforms"],
    },
    DomainSolver {
        id: "mcad-dfm-review",
        label: "DFM review",
        stage: DomainSolverStage::Analysis,
        description: "Runs envelope, wall, hole, and material manufacturability checks",
        inputs: &["evaluated_scene", "materials"],
        outputs: &["dfm_report"],
    },
];

const SHADERS: [DomainShader; 3] = [
    DomainShader {
        id: "mcad-feature-highlight",
        label: "Feature highlight",
        stage: DomainShaderStage::Overlay,
        entry_point: "mcad_feature_highlight",
        description: "Highlights selected feature-tree ownership in the viewport",
    },
    DomainShader {
        id: "mcad-ghost-diff",
        label: "Ghost diff",
        stage: DomainShaderStage::Render,
        entry_point: "mcad_ghost_diff",
        description: "Draws AI preview and rollback geometry with source colors",
    },
    DomainShader {
        id: "mcad-tolerance-band",
        label: "Tolerance band",
        stage: DomainShaderStage::Overlay,
        entry_point: "mcad_tolerance_band",
        description: "Displays tolerance envelopes and edge-modifier diagnostics",
    },
];

const AI_TOOLS: [DomainAiTool; 4] = [
    DomainAiTool {
        id: "mcad_create_prismatic_part",
        label: "Create prismatic part",
        description: "Create a conservative box-based mechanical feature proposal",
        schema_id: "cadx.domain.mcad.create_prismatic_part.v1",
    },
    DomainAiTool {
        id: "mcad_open_feature_tool",
        label: "Open feature tool",
        description: "Open sketch, extrude, chamfer, fillet, or assembly tooling",
        schema_id: "cadx.domain.mcad.open_feature_tool.v1",
    },
    DomainAiTool {
        id: "mcad_run_dfm",
        label: "Run DFM",
        description: "Run mechanical manufacturability checks",
        schema_id: "cadx.domain.mcad.run_dfm.v1",
    },
    DomainAiTool {
        id: "mcad_generate_bom",
        label: "Generate BOM",
        description: "Generate a grouped mechanical bill of materials",
        schema_id: "cadx.domain.mcad.generate_bom.v1",
    },
];

const STANDARD_PARTS: [StandardPartDescriptor; 4] = [
    StandardPartDescriptor {
        id: "iso-4762-m6x20",
        family: "socket_head_cap_screw",
        standard: "ISO 4762",
        name: "M6 x 20 socket head cap screw",
        default_material: "Steel 8.8",
        parameters: &[("diameter_mm", 6.0), ("length_mm", 20.0)],
    },
    StandardPartDescriptor {
        id: "iso-7089-m6",
        family: "plain_washer",
        standard: "ISO 7089",
        name: "M6 plain washer",
        default_material: "Steel 200 HV",
        parameters: &[("inner_diameter_mm", 6.4), ("outer_diameter_mm", 12.0)],
    },
    StandardPartDescriptor {
        id: "gb-t-6170-m6",
        family: "hex_nut",
        standard: "GB/T 6170",
        name: "M6 hex nut",
        default_material: "Steel 8",
        parameters: &[("diameter_mm", 6.0), ("height_mm", 5.2)],
    },
    StandardPartDescriptor {
        id: "asme-b18-3-1m-m8x30",
        family: "socket_head_cap_screw",
        standard: "ASME B18.3.1M",
        name: "M8 x 30 socket head cap screw",
        default_material: "Alloy steel",
        parameters: &[("diameter_mm", 8.0), ("length_mm", 30.0)],
    },
];

const ASSEMBLY_TEMPLATES: [AssemblyConstraintTemplate; 3] = [
    AssemblyConstraintTemplate {
        id: "fixed",
        label: "Fixed mate",
        mate_kind: "fixed",
        degrees_of_freedom: 0,
    },
    AssemblyConstraintTemplate {
        id: "revolute",
        label: "Revolute mate",
        mate_kind: "revolute",
        degrees_of_freedom: 1,
    },
    AssemblyConstraintTemplate {
        id: "slider",
        label: "Slider mate",
        mate_kind: "slider",
        degrees_of_freedom: 1,
    },
];

#[derive(Debug, Default, Clone, Copy)]
pub struct McadPack;

#[must_use]
pub const fn standard_parts() -> &'static [StandardPartDescriptor] {
    &STANDARD_PARTS
}

#[must_use]
pub const fn assembly_constraint_templates() -> &'static [AssemblyConstraintTemplate] {
    &ASSEMBLY_TEMPLATES
}

impl DomainPack for McadPack {
    fn manifest(&self) -> DomainManifest {
        DomainManifest {
            id: DomainId::Mcad,
            name: "MCAD",
            version: "0.2",
            description: "Feature tree, sketch/extrude/fillet tools, assemblies and DFM",
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
            "extrude" | "ai-part" => Some(PANELS[0]),
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
            "sketch" => Ok(DomainExecution::with_action(
                "Open MCAD sketch tools",
                DomainAction::OpenPanel {
                    panel: "sketch".into(),
                },
            )),
            "extrude" | "ai-part" => {
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

    fn route_natural_language(&self, input: &str, context: &DomainContext) -> DomainRoute {
        let normalized = input.to_ascii_lowercase();
        let (action, confidence) = if normalized.contains("bom") || input.contains("物料") {
            (DomainAction::GenerateBom, 0.92)
        } else if normalized.contains("feature tree") || input.contains("特征树") {
            (
                DomainAction::OpenPanel {
                    panel: "feature-tree".into(),
                },
                0.96,
            )
        } else if normalized.contains("assembly") || input.contains("装配") {
            (
                DomainAction::OpenPanel {
                    panel: "assembly".into(),
                },
                0.95,
            )
        } else if normalized.contains("fillet")
            || normalized.contains("chamfer")
            || input.contains("圆角")
            || input.contains("倒角")
        {
            (
                DomainAction::OpenPanel {
                    panel: "edge-modifiers".into(),
                },
                0.96,
            )
        } else if normalized.contains("extrude") || input.contains("拉伸") {
            (
                DomainAction::OpenPanel {
                    panel: "extrude".into(),
                },
                0.95,
            )
        } else if normalized.contains("dfm") || input.contains("制造") {
            (
                DomainAction::RunCheck {
                    check: "dfm".into(),
                },
                0.93,
            )
        } else if input.contains("工程图")
            || input.contains("标注")
            || normalized.contains("drawing")
        {
            (
                DomainAction::RunCheck {
                    check: "drawing".into(),
                },
                0.93,
            )
        } else if input.contains("干涉") || normalized.contains("interference") {
            (
                DomainAction::RunCheck {
                    check: "interference".into(),
                },
                0.94,
            )
        } else if normalized.contains("mcad")
            || normalized.contains("mechanical")
            || input.contains("机械")
        {
            (
                DomainAction::CreateSolidBox {
                    name: context
                        .selected_feature_name
                        .clone()
                        .unwrap_or_else(|| "AI MCAD part".into()),
                    size_mm: [40.0, 30.0, 10.0],
                    position_mm: [0.0, 0.0, 0.0],
                },
                0.9,
            )
        } else {
            (
                DomainAction::OpenPanel {
                    panel: "feature-tree".into(),
                },
                0.2,
            )
        };
        DomainRoute {
            action,
            confidence,
            rationale: "MCAD pack keyword, feature-tree, and selection routing".into(),
        }
    }

    fn validate_export(&self, format: ExportFormat, context: &DomainContext) -> Vec<DomainIssue> {
        match format {
            ExportFormat::Gerber | ExportFormat::Drill => vec![DomainIssue {
                code: "WRONG_DOMAIN_FORMAT".into(),
                severity: DomainIssueSeverity::Error,
                message: "Gerber and drill exports belong to the PCB pack".into(),
            }],
            ExportFormat::Bom if context.visible_solid_count == 0 => vec![DomainIssue {
                code: "EMPTY_BOM".into(),
                severity: DomainIssueSeverity::Warning,
                message: "No visible solids are available for a mechanical BOM".into(),
            }],
            _ => Vec::new(),
        }
    }
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
    use super::*;

    #[test]
    fn mcad_pack_exposes_full_pipeline_descriptors() {
        let pack = McadPack;
        assert_eq!(pack.manifest().name, "MCAD");
        assert!(pack.tools().iter().any(|tool| tool.id == "feature-tree"));
        assert!(pack.inspector_schema().panels.len() >= 3);
        assert!(pack.solvers().iter().any(|solver| {
            solver.id == "mcad-assembly-mates" && solver.stage == DomainSolverStage::Constraint
        }));
        assert!(
            pack.shaders()
                .iter()
                .any(|shader| shader.id == "mcad-ghost-diff")
        );
        assert!(
            pack.ai_tools()
                .iter()
                .any(|tool| tool.id == "mcad_generate_bom")
        );
    }

    #[test]
    fn routes_edge_modifier_prompts_to_the_correct_panel() {
        let route = McadPack.route_natural_language(
            "add a 2 mm fillet",
            &DomainContext {
                document_name: "part".into(),
                selected_feature_ids: vec![1],
                visible_solid_count: 1,
                active_feature_count: 1,
                selected_feature_name: Some("bracket".into()),
                spatial_entities: Vec::new(),
            },
        );
        assert!(matches!(
            route.action,
            DomainAction::OpenPanel { panel } if panel == "edge-modifiers"
        ));
    }

    #[test]
    fn standard_part_catalog_is_deterministic() {
        assert_eq!(standard_parts()[0].id, "iso-4762-m6x20");
        assert_eq!(assembly_constraint_templates()[1].mate_kind, "revolute");
    }

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
