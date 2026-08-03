//! Aggregated ECAD/PCB domain pack.

pub use cadx_ecad_drc as drc;
pub use cadx_ecad_export as export;
pub use cadx_ecad_layout as layout;
pub use cadx_ecad_netlist as netlist;
pub use cadx_ecad_router as router;

use cadx_domain_api::{
    DomainAction, DomainAiTool, DomainArtifact, DomainArtifactKind, DomainContext, DomainExecution,
    DomainExecutionError, DomainFieldKind, DomainFieldSchema, DomainFieldValue, DomainId,
    DomainInspectorSchema, DomainIssue, DomainIssueSeverity, DomainManifest, DomainPack,
    DomainPanelSchema, DomainRoute, DomainSelectOption, DomainShader, DomainShaderStage,
    DomainSolver, DomainSolverStage, DomainTool, DomainToolRequest, ExportFormat,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FootprintDescriptor {
    pub id: &'static str,
    pub package: &'static str,
    pub pads: u16,
    pub body_size_mm: [f64; 2],
    pub courtyard_mm: [f64; 2],
    pub default_height_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NetClassDescriptor {
    pub name: &'static str,
    pub width_mm: f64,
    pub clearance_mm: f64,
    pub via_diameter_mm: f64,
    pub differential_pair_gap_mm: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoutingEstimate {
    pub declared_net_count: usize,
    pub routeable_layer_count: usize,
    pub unrouted_pin_count: usize,
    pub estimated_segment_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct EcadCapabilities {
    pub schematic_capture: bool,
    pub netlist_import: bool,
    pub multilayer_layout: bool,
    pub electrical_drc: bool,
    pub impedance_rules: bool,
    pub automatic_routing: bool,
    pub component_3d_link: bool,
    pub enclosure_interference: bool,
    pub gerber_export: bool,
    pub step_export: bool,
}

impl Default for EcadCapabilities {
    fn default() -> Self {
        Self {
            schematic_capture: true,
            netlist_import: true,
            multilayer_layout: true,
            electrical_drc: true,
            impedance_rules: true,
            automatic_routing: true,
            component_3d_link: true,
            enclosure_interference: true,
            gerber_export: true,
            step_export: true,
        }
    }
}

const TOOLS: [DomainTool; 13] = [
    DomainTool {
        id: "schematic",
        label: "Schematic",
        icon: "workflow",
        category: "ECAD",
    },
    DomainTool {
        id: "netlist",
        label: "Netlist",
        icon: "list-tree",
        category: "ECAD",
    },
    DomainTool {
        id: "board",
        label: "Board outline",
        icon: "box",
        category: "2D",
    },
    DomainTool {
        id: "footprint-library",
        label: "Footprint library",
        icon: "archive",
        category: "ECAD",
    },
    DomainTool {
        id: "placement",
        label: "Component placement",
        icon: "circle-dot",
        category: "2D",
    },
    DomainTool {
        id: "routing",
        label: "Interactive routing",
        icon: "combine",
        category: "2D",
    },
    DomainTool {
        id: "diff-pair",
        label: "Differential pair",
        icon: "git-compare-arrows",
        category: "2D",
    },
    DomainTool {
        id: "via",
        label: "Via",
        icon: "circle-dot-dashed",
        category: "2D",
    },
    DomainTool {
        id: "drc",
        label: "Electrical DRC",
        icon: "triangle-alert",
        category: "2D",
    },
    DomainTool {
        id: "stackup",
        label: "Layer stackup",
        icon: "layers",
        category: "2D",
    },
    DomainTool {
        id: "3d-link",
        label: "3D component link",
        icon: "box",
        category: "3D",
    },
    DomainTool {
        id: "gerber",
        label: "Gerber / drill",
        icon: "file-input",
        category: "Export",
    },
    DomainTool {
        id: "bom",
        label: "BOM",
        icon: "layers",
        category: "Export",
    },
];

const LAYER_COUNT_OPTIONS: [DomainSelectOption; 4] = [
    DomainSelectOption {
        value: "2",
        label: "2 layers",
    },
    DomainSelectOption {
        value: "4",
        label: "4 layers",
    },
    DomainSelectOption {
        value: "6",
        label: "6 layers",
    },
    DomainSelectOption {
        value: "8",
        label: "8 layers",
    },
];

const SIDE_OPTIONS: [DomainSelectOption; 2] = [
    DomainSelectOption {
        value: "top",
        label: "Top",
    },
    DomainSelectOption {
        value: "bottom",
        label: "Bottom",
    },
];

const BOARD_FIELDS: [DomainFieldSchema; 5] = [
    DomainFieldSchema {
        id: "board_width_mm",
        label: "Board width",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("80"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "board_height_mm",
        label: "Board height",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("50"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "board_thickness_mm",
        label: "Board thickness",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("1.6"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "layer_count",
        label: "Layer count",
        kind: DomainFieldKind::Select,
        default_value: Some("4"),
        unit: None,
        options: &LAYER_COUNT_OPTIONS,
        required: true,
    },
    DomainFieldSchema {
        id: "copper_weight_oz",
        label: "Copper weight",
        kind: DomainFieldKind::Decimal,
        default_value: Some("1"),
        unit: Some("oz"),
        options: &[],
        required: true,
    },
];

const ROUTING_FIELDS: [DomainFieldSchema; 5] = [
    DomainFieldSchema {
        id: "min_trace_width_mm",
        label: "Min trace width",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("0.15"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "min_clearance_mm",
        label: "Min clearance",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("0.15"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "via_diameter_mm",
        label: "Via diameter",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("0.45"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "differential_pair_gap_mm",
        label: "Differential pair gap",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("0.18"),
        unit: Some("mm"),
        options: &[],
        required: false,
    },
    DomainFieldSchema {
        id: "impedance_ohms",
        label: "Target impedance",
        kind: DomainFieldKind::Decimal,
        default_value: Some("90"),
        unit: Some("ohm"),
        options: &[],
        required: false,
    },
];

const COMPONENT_FIELDS: [DomainFieldSchema; 8] = [
    DomainFieldSchema {
        id: "reference",
        label: "Reference",
        kind: DomainFieldKind::Text,
        default_value: Some("U1"),
        unit: None,
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "value",
        label: "Value",
        kind: DomainFieldKind::Text,
        default_value: Some("MCU"),
        unit: None,
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "footprint",
        label: "Footprint",
        kind: DomainFieldKind::Text,
        default_value: Some("QFN-32"),
        unit: None,
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "position_x_mm",
        label: "Position X",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("40"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "position_y_mm",
        label: "Position Y",
        kind: DomainFieldKind::LengthMm,
        default_value: Some("25"),
        unit: Some("mm"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "rotation_deg",
        label: "Rotation",
        kind: DomainFieldKind::AngleDeg,
        default_value: Some("0"),
        unit: Some("deg"),
        options: &[],
        required: true,
    },
    DomainFieldSchema {
        id: "side",
        label: "Side",
        kind: DomainFieldKind::Select,
        default_value: Some("top"),
        unit: None,
        options: &SIDE_OPTIONS,
        required: true,
    },
    DomainFieldSchema {
        id: "linked_3d_model",
        label: "Linked 3D model",
        kind: DomainFieldKind::Text,
        default_value: None,
        unit: None,
        options: &[],
        required: false,
    },
];

const PANELS: [DomainPanelSchema; 3] = [
    DomainPanelSchema {
        id: "ecad_board",
        label: "Board stackup",
        fields: &BOARD_FIELDS,
    },
    DomainPanelSchema {
        id: "ecad_routing_rules",
        label: "Routing rules",
        fields: &ROUTING_FIELDS,
    },
    DomainPanelSchema {
        id: "ecad_component",
        label: "Component",
        fields: &COMPONENT_FIELDS,
    },
];

const SOLVERS: [DomainSolver; 5] = [
    DomainSolver {
        id: "ecad-netlist-connectivity",
        label: "Netlist connectivity",
        stage: DomainSolverStage::Constraint,
        description: "Checks reference designators, pins, and declared nets",
        inputs: &["schematic_netlist", "footprints"],
        outputs: &["connectivity_graph"],
    },
    DomainSolver {
        id: "ecad-interactive-router",
        label: "Interactive router",
        stage: DomainSolverStage::Routing,
        description: "Routes traces against layer, width, clearance, and keepout rules",
        inputs: &["board", "nets", "routing_rules"],
        outputs: &["trace_segments"],
    },
    DomainSolver {
        id: "ecad-differential-pair",
        label: "Differential pair router",
        stage: DomainSolverStage::Routing,
        description: "Keeps differential pairs width, gap, and length matched",
        inputs: &["paired_nets", "impedance_rules"],
        outputs: &["matched_routes"],
    },
    DomainSolver {
        id: "ecad-drc",
        label: "Design-rule check",
        stage: DomainSolverStage::Analysis,
        description: "Runs deterministic component, trace, layer, and net checks",
        inputs: &["board_layout"],
        outputs: &["drc_report"],
    },
    DomainSolver {
        id: "ecad-manufacturing-export",
        label: "Manufacturing export",
        stage: DomainSolverStage::Export,
        description: "Validates and emits Gerber, drill, and board-outline outputs",
        inputs: &["board_layout", "drc_report"],
        outputs: &["gerber_bundle", "drill_file"],
    },
];

const SHADERS: [DomainShader; 4] = [
    DomainShader {
        id: "ecad-copper-layers",
        label: "Copper layers",
        stage: DomainShaderStage::Render,
        entry_point: "ecad_copper_layers",
        description: "Draws layer-specific copper, mask, and silkscreen overlays",
    },
    DomainShader {
        id: "ecad-ratsnest",
        label: "Ratsnest",
        stage: DomainShaderStage::Overlay,
        entry_point: "ecad_ratsnest",
        description: "Displays unrouted connectivity and selected net highlights",
    },
    DomainShader {
        id: "ecad-drc-heatmap",
        label: "DRC heatmap",
        stage: DomainShaderStage::Overlay,
        entry_point: "ecad_drc_heatmap",
        description: "Highlights clearance, edge, and width violations",
    },
    DomainShader {
        id: "ecad-spatial-sort",
        label: "Spatial sorting",
        stage: DomainShaderStage::Compute,
        entry_point: "ecad_spatial_sort",
        description: "Builds GPU-friendly trace and component broad-phase bins",
    },
];

const AI_TOOLS: [DomainAiTool; 5] = [
    DomainAiTool {
        id: "ecad_create_board",
        label: "Create board",
        description: "Create a board outline and stackup proposal",
        schema_id: "cadx.domain.ecad.create_board.v1",
    },
    DomainAiTool {
        id: "ecad_place_component",
        label: "Place component",
        description: "Place a footprint and optional linked 3D package",
        schema_id: "cadx.domain.ecad.place_component.v1",
    },
    DomainAiTool {
        id: "ecad_route_net",
        label: "Route net",
        description: "Route a net with width, layer, and clearance constraints",
        schema_id: "cadx.domain.ecad.route_net.v1",
    },
    DomainAiTool {
        id: "ecad_run_drc",
        label: "Run DRC",
        description: "Run deterministic electrical design-rule checks",
        schema_id: "cadx.domain.ecad.run_drc.v1",
    },
    DomainAiTool {
        id: "ecad_export_manufacturing",
        label: "Export manufacturing",
        description: "Validate and preview Gerber/drill manufacturing outputs",
        schema_id: "cadx.domain.ecad.export_manufacturing.v1",
    },
];

const FOOTPRINTS: [FootprintDescriptor; 4] = [
    FootprintDescriptor {
        id: "qfn-32",
        package: "QFN-32",
        pads: 32,
        body_size_mm: [5.0, 5.0],
        courtyard_mm: [6.0, 6.0],
        default_height_mm: 1.0,
    },
    FootprintDescriptor {
        id: "sot-23-6",
        package: "SOT-23-6",
        pads: 6,
        body_size_mm: [2.9, 1.6],
        courtyard_mm: [3.4, 2.2],
        default_height_mm: 1.1,
    },
    FootprintDescriptor {
        id: "0603",
        package: "0603",
        pads: 2,
        body_size_mm: [1.6, 0.8],
        courtyard_mm: [2.0, 1.2],
        default_height_mm: 0.8,
    },
    FootprintDescriptor {
        id: "usb-c-16p",
        package: "USB-C-16P",
        pads: 16,
        body_size_mm: [9.0, 7.0],
        courtyard_mm: [10.5, 8.5],
        default_height_mm: 3.2,
    },
];

const NET_CLASSES: [NetClassDescriptor; 3] = [
    NetClassDescriptor {
        name: "DEFAULT",
        width_mm: 0.15,
        clearance_mm: 0.15,
        via_diameter_mm: 0.45,
        differential_pair_gap_mm: None,
    },
    NetClassDescriptor {
        name: "POWER",
        width_mm: 0.4,
        clearance_mm: 0.2,
        via_diameter_mm: 0.6,
        differential_pair_gap_mm: None,
    },
    NetClassDescriptor {
        name: "USB_HS",
        width_mm: 0.18,
        clearance_mm: 0.15,
        via_diameter_mm: 0.45,
        differential_pair_gap_mm: Some(0.18),
    },
];

#[derive(Debug, Default, Clone, Copy)]
pub struct EcadPack;

#[must_use]
pub const fn footprint_library() -> &'static [FootprintDescriptor] {
    &FOOTPRINTS
}

#[must_use]
pub const fn default_net_classes() -> &'static [NetClassDescriptor] {
    &NET_CLASSES
}

#[must_use]
pub fn routing_estimate(board: &layout::PcbBoard) -> RoutingEstimate {
    let routeable_layer_count = board
        .layers
        .iter()
        .filter(|layer| matches!(layer.kind, layout::LayerKind::Copper))
        .count();
    let routed_pins = board.traces.len().saturating_mul(2);
    let declared_pins = board.nets.iter().map(|net| net.pins.len()).sum::<usize>();
    RoutingEstimate {
        declared_net_count: board.nets.len(),
        routeable_layer_count,
        unrouted_pin_count: declared_pins.saturating_sub(routed_pins),
        estimated_segment_count: board.traces.len().saturating_add(declared_pins / 2),
    }
}

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

fn decimal(values: &cadx_domain_api::DomainParameters, id: &str) -> f64 {
    values
        .get(id)
        .and_then(DomainFieldValue::as_decimal)
        .unwrap_or_default()
}

fn text<'a>(values: &'a cadx_domain_api::DomainParameters, id: &str, fallback: &'a str) -> &'a str {
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
    use super::*;
    use cadx_ecad_layout::PcbBoard;

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

    #[test]
    fn footprint_library_and_routing_estimate_are_deterministic() {
        assert_eq!(footprint_library()[0].package, "QFN-32");
        assert_eq!(default_net_classes()[2].name, "USB_HS");

        let estimate = routing_estimate(&PcbBoard::demo());
        assert_eq!(estimate.declared_net_count, 1);
        assert_eq!(estimate.routeable_layer_count, 4);
        assert_eq!(estimate.unrouted_pin_count, 1);
    }

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
