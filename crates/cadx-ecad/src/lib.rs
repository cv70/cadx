//! Aggregated ECAD/PCB domain pack.

mod capabilities;
mod catalog;
mod estimate;
mod execute;

pub use cadx_ecad_drc as drc;
pub use cadx_ecad_export as export;
pub use cadx_ecad_layout as layout;
pub use cadx_ecad_netlist as netlist;
pub use cadx_ecad_router as router;

pub use capabilities::EcadCapabilities;
pub use catalog::{
    FootprintDescriptor, NetClassDescriptor, default_net_classes, footprint_library,
};
pub use estimate::{RoutingEstimate, routing_estimate};

use catalog::{AI_TOOLS, PANELS, SHADERS, SOLVERS, TOOLS};

use cadx_domain_api::{
    DomainAction, DomainAiTool, DomainContext, DomainExecution, DomainExecutionError, DomainId,
    DomainInspectorSchema, DomainIssue, DomainIssueSeverity, DomainManifest, DomainPack,
    DomainPanelSchema, DomainRoute, DomainShader, DomainSolver, DomainTool, DomainToolRequest,
    ExportFormat,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct EcadPack;

impl DomainPack for EcadPack {
    fn manifest(&self) -> DomainManifest {
        DomainManifest {
            id: DomainId::Ecad,
            name: "ECAD",
            version: "0.2",
            description: "Netlist, layered PCB layout, routing, DRC, footprints and manufacturing export",
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
            "board" => Some(PANELS[0]),
            "routing" => Some(PANELS[1]),
            "placement" | "3d-link" => Some(PANELS[2]),
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
        let (action, confidence) = if normalized.contains("gerber") || input.contains("制造文件")
        {
            (
                DomainAction::Export {
                    format: "gerber".into(),
                },
                0.97,
            )
        } else if normalized.contains("netlist") || input.contains("网表") {
            (
                DomainAction::OpenPanel {
                    panel: "netlist".into(),
                },
                0.96,
            )
        } else if normalized.contains("route")
            || normalized.contains("routing")
            || input.contains("布线")
        {
            (
                DomainAction::OpenPanel {
                    panel: "routing".into(),
                },
                0.95,
            )
        } else if normalized.contains("footprint") || input.contains("封装") {
            (
                DomainAction::OpenPanel {
                    panel: "footprint-library".into(),
                },
                0.94,
            )
        } else if normalized.contains("drc") || input.contains("规则") || input.contains("短路")
        {
            (
                DomainAction::RunCheck {
                    check: "drc".into(),
                },
                0.95,
            )
        } else if normalized.contains("bom") || input.contains("物料") {
            (DomainAction::GenerateBom, 0.9)
        } else if input.contains("元件") || normalized.contains("component") {
            (
                DomainAction::PlacePcbComponent {
                    reference: "U1".into(),
                    value: "MCU".into(),
                    footprint: "QFN-32".into(),
                    position_mm: [40.0, 25.0],
                    rotation_deg: 0.0,
                    side: "top".into(),
                    model_3d: Some("QFN-32.step".into()),
                },
                0.9,
            )
        } else if normalized.contains("pcb")
            || normalized.contains("ecad")
            || normalized.contains("board")
            || input.contains("电路板")
        {
            (
                DomainAction::CreatePcbBoard {
                    name: "AI ECAD board".into(),
                    width_mm: 80.0,
                    height_mm: 50.0,
                    thickness_mm: 1.6,
                    layers: 4,
                },
                0.91,
            )
        } else {
            (
                DomainAction::OpenPanel {
                    panel: "schematic".into(),
                },
                0.2,
            )
        };
        DomainRoute {
            action,
            confidence,
            rationale: "ECAD pack netlist, routing, DRC, and manufacturing workflow routing".into(),
        }
    }

    fn validate_export(&self, format: ExportFormat, context: &DomainContext) -> Vec<DomainIssue> {
        match format {
            ExportFormat::Ifc => vec![DomainIssue {
                code: "WRONG_DOMAIN_FORMAT".into(),
                severity: DomainIssueSeverity::Error,
                message: "IFC export belongs to the AEC pack".into(),
            }],
            ExportFormat::Step | ExportFormat::Gerber | ExportFormat::Drill
                if context.active_feature_count == 0 =>
            {
                vec![DomainIssue {
                    code: "EMPTY_ECAD_EXPORT".into(),
                    severity: DomainIssueSeverity::Warning,
                    message: "The ECAD project has no linked geometry".into(),
                }]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecad_pack_exposes_full_pipeline_descriptors() {
        let pack = EcadPack;
        assert_eq!(pack.manifest().name, "ECAD");
        assert!(pack.tools().iter().any(|tool| tool.id == "netlist"));
        assert!(
            pack.inspector_schema()
                .panels
                .iter()
                .any(|panel| panel.id == "ecad_routing_rules")
        );
        assert!(
            pack.solvers()
                .iter()
                .any(|solver| solver.id == "ecad-interactive-router")
        );
        assert!(
            pack.shaders()
                .iter()
                .any(|shader| shader.id == "ecad-ratsnest")
        );
        assert!(
            pack.ai_tools()
                .iter()
                .any(|tool| tool.id == "ecad_route_net")
        );
    }

    #[test]
    fn routes_netlist_prompt_to_panel() {
        let route = EcadPack.route_natural_language(
            "import this netlist",
            &DomainContext {
                document_name: "board".into(),
                selected_feature_ids: Vec::new(),
                visible_solid_count: 0,
                active_feature_count: 0,
                selected_feature_name: None,
                spatial_entities: Vec::new(),
            },
        );
        assert!(matches!(
            route.action,
            DomainAction::OpenPanel { panel } if panel == "netlist"
        ));
    }
}
