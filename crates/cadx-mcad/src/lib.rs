//! Aggregated Mechanical domain pack.
//!
//! The sub-crates remain independently usable so future workbenches can ship
//! only standards, DFM, or BOM functionality as needed.

mod capabilities;
mod catalog;
mod execute;

pub use cadx_mcad_bom as bom;
pub use cadx_mcad_dfm as dfm;
pub use cadx_mcad_model as model;
pub use cadx_mcad_standards as standards;

pub use capabilities::McadCapabilities;
pub use catalog::{
    AssemblyConstraintTemplate, StandardPartDescriptor, assembly_constraint_templates,
    standard_parts,
};

use catalog::{AI_TOOLS, PANELS, SHADERS, SOLVERS, TOOLS};

use cadx_domain_api::{
    DomainAction, DomainAiTool, DomainContext, DomainExecution, DomainExecutionError, DomainId,
    DomainInspectorSchema, DomainIssue, DomainIssueSeverity, DomainManifest, DomainPack,
    DomainPanelSchema, DomainRoute, DomainShader, DomainSolver, DomainTool, DomainToolRequest,
    ExportFormat,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct McadPack;

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
        execute::execute_tool(request)
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

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_domain_api::DomainSolverStage;

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
}
