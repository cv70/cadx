//! Static AEC tool, schema, and pipeline descriptor tables.

use cadx_domain_api::{
    DomainAiTool, DomainFieldKind, DomainFieldSchema, DomainPanelSchema, DomainSelectOption,
    DomainShader, DomainShaderStage, DomainSolver, DomainSolverStage, DomainTool,
};

pub(crate) const TOOLS: [DomainTool; 10] = [
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

pub(crate) const PANELS: [DomainPanelSchema; 4] = [
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
pub(crate) const SOLVERS: [DomainSolver; 5] = [
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

pub(crate) const SHADERS: [DomainShader; 3] = [
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

pub(crate) const AI_TOOLS: [DomainAiTool; 5] = [
    DomainAiTool {
        id: "aec_create_wall",
        label: "Create wall",
        description: "Create a wall solid with BIM metadata",
        schema_id: "cadx.domain.aec.create_wall.v1",
        executable_tool_id: "wall",
    },
    DomainAiTool {
        id: "aec_create_slab",
        label: "Create slab",
        description: "Create a slab solid with BIM metadata",
        schema_id: "cadx.domain.aec.create_slab.v1",
        executable_tool_id: "slab",
    },
    DomainAiTool {
        id: "aec_update_bim_properties",
        label: "Update BIM properties",
        description: "Apply IFC class and property-set values",
        schema_id: "cadx.domain.aec.update_bim_properties.v1",
        executable_tool_id: "bim-attrs",
    },
    DomainAiTool {
        id: "aec_run_clash",
        label: "Run clash review",
        description: "Run spatial coordination checks",
        schema_id: "cadx.domain.aec.run_clash.v1",
        executable_tool_id: "clash",
    },
    DomainAiTool {
        id: "aec_export_ifc",
        label: "Export IFC",
        description: "Validate and export IFC4/IFC4X3",
        schema_id: "cadx.domain.aec.export_ifc.v1",
        executable_tool_id: "ifc",
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
