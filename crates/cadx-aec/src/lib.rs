//! Aggregate AEC/BIM domain pack.

mod capabilities;
mod catalog;
mod execute;

pub use cadx_aec_analysis as analysis;
pub use cadx_aec_bim as bim;
pub use cadx_aec_ifc as ifc;

pub use capabilities::AecCapabilities;

use bim::{SlabSpec, WallSpec};
use catalog::{AI_TOOLS, PANELS, SHADERS, SOLVERS, TOOLS};

use cadx_domain_api::{
    DomainAction, DomainAiTool, DomainContext, DomainExecution, DomainExecutionError, DomainId,
    DomainInspectorSchema, DomainIssue, DomainIssueSeverity, DomainManifest, DomainPack,
    DomainPanelSchema, DomainRoute, DomainShader, DomainSolver, DomainTool, DomainToolRequest,
    ExportFormat,
};

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
        execute::execute_tool(request)
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

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn context() -> DomainContext {
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
    fn aec_route_does_not_claim_unrelated_prompts() {
        let route = AecPack.route_natural_language("make a gear", &context());
        assert!(route.confidence < 0.5);
    }
}
