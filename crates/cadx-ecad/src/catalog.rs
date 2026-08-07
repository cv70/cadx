//! Static ECAD tool, schema, pipeline, footprint, and net-class tables.

use cadx_domain_api::{
    DomainAiTool, DomainFieldKind, DomainFieldSchema, DomainPanelSchema, DomainSelectOption,
    DomainShader, DomainShaderStage, DomainSolver, DomainSolverStage, DomainTool,
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

pub(crate) const TOOLS: [DomainTool; 13] = [
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

pub(crate) const PANELS: [DomainPanelSchema; 3] = [
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
pub(crate) const SOLVERS: [DomainSolver; 5] = [
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

pub(crate) const SHADERS: [DomainShader; 4] = [
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

pub(crate) const AI_TOOLS: [DomainAiTool; 5] = [
    DomainAiTool {
        id: "ecad_create_board",
        label: "Create board",
        description: "Create a board outline and stackup proposal",
        schema_id: "cadx.domain.ecad.create_board.v1",
        executable_tool_id: "board",
    },
    DomainAiTool {
        id: "ecad_place_component",
        label: "Place component",
        description: "Place a footprint and optional linked 3D package",
        schema_id: "cadx.domain.ecad.place_component.v1",
        executable_tool_id: "placement",
    },
    DomainAiTool {
        id: "ecad_route_net",
        label: "Route net",
        description: "Route a net with width, layer, and clearance constraints",
        schema_id: "cadx.domain.ecad.route_net.v1",
        executable_tool_id: "routing",
    },
    DomainAiTool {
        id: "ecad_run_drc",
        label: "Run DRC",
        description: "Run deterministic electrical design-rule checks",
        schema_id: "cadx.domain.ecad.run_drc.v1",
        executable_tool_id: "drc",
    },
    DomainAiTool {
        id: "ecad_export_manufacturing",
        label: "Export manufacturing",
        description: "Validate and preview Gerber/drill manufacturing outputs",
        schema_id: "cadx.domain.ecad.export_manufacturing.v1",
        executable_tool_id: "gerber",
    },
];
pub(crate) const FOOTPRINTS: [FootprintDescriptor; 4] = [
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

#[must_use]
pub const fn footprint_library() -> &'static [FootprintDescriptor] {
    &FOOTPRINTS
}

#[must_use]
pub const fn default_net_classes() -> &'static [NetClassDescriptor] {
    &NET_CLASSES
}
