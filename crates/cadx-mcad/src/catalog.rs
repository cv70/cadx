//! Static MCAD tool, schema, pipeline, and part-library tables.

use cadx_domain_api::{
    DomainAiTool, DomainFieldKind, DomainFieldSchema, DomainPanelSchema, DomainSelectOption,
    DomainShader, DomainShaderStage, DomainSolver, DomainSolverStage, DomainTool,
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

pub(crate) const TOOLS: [DomainTool; 12] = [
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
pub(crate) const PANELS: [DomainPanelSchema; 4] = [
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

pub(crate) const SOLVERS: [DomainSolver; 4] = [
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

pub(crate) const SHADERS: [DomainShader; 3] = [
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
pub(crate) const AI_TOOLS: [DomainAiTool; 4] = [
    DomainAiTool {
        id: "mcad_create_prismatic_part",
        label: "Create prismatic part",
        description: "Create a conservative box-based mechanical feature proposal",
        schema_id: "cadx.domain.mcad.create_prismatic_part.v1",
        executable_tool_id: "ai-part",
    },
    DomainAiTool {
        id: "mcad_open_sketch",
        label: "Open sketch",
        description: "Open the mechanical sketch workflow",
        schema_id: "cadx.domain.mcad.open_sketch.v1",
        executable_tool_id: "sketch",
    },
    DomainAiTool {
        id: "mcad_run_dfm",
        label: "Run DFM",
        description: "Run mechanical manufacturability checks",
        schema_id: "cadx.domain.mcad.run_dfm.v1",
        executable_tool_id: "dfm",
    },
    DomainAiTool {
        id: "mcad_generate_bom",
        label: "Generate BOM",
        description: "Generate a grouped mechanical bill of materials",
        schema_id: "cadx.domain.mcad.generate_bom.v1",
        executable_tool_id: "bom",
    },
];

pub(crate) const STANDARD_PARTS: [StandardPartDescriptor; 4] = [
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

pub(crate) const ASSEMBLY_TEMPLATES: [AssemblyConstraintTemplate; 3] = [
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

#[must_use]
pub const fn standard_parts() -> &'static [StandardPartDescriptor] {
    &STANDARD_PARTS
}

#[must_use]
pub const fn assembly_constraint_templates() -> &'static [AssemblyConstraintTemplate] {
    &ASSEMBLY_TEMPLATES
}

#[cfg(test)]
mod tests {
    use super::{assembly_constraint_templates, standard_parts};

    #[test]
    fn standard_part_catalog_is_deterministic() {
        assert_eq!(standard_parts()[0].id, "iso-4762-m6x20");
        assert_eq!(assembly_constraint_templates()[1].mate_kind, "revolute");
    }
}
