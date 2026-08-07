//! JSON tool schema for the single `apply_cad_plan` function call.

use genai::chat::Tool;
use serde_json::json;

mod defs;

#[cfg(test)]
mod tests;

pub(super) fn cad_plan_tool() -> Tool {
    let mut tool = Tool::new("apply_cad_plan")
        .with_description(
            "Propose one primary atomic CAD edit and optional independent design alternatives",
        )
        .with_schema(json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Short description of the resulting edit"
                },
                "commands": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 24,
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_box" },
                                    "name": { "type": "string" },
                                    "size": { "$ref": "#/$defs/positiveVec3" },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "size", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_cylinder" },
                                    "name": { "type": "string" },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "radius", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_sphere" },
                                    "name": { "type": "string" },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "radius", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_cone" },
                                    "name": { "type": "string" },
                                    "bottom_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "top_radius": { "type": "number", "minimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "bottom_radius", "top_radius", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_torus" },
                                    "name": { "type": "string" },
                                    "major_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "minor_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "major_radius", "minor_radius", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_extrusion" },
                                    "name": { "type": "string" },
                                    "profile": { "$ref": "#/$defs/profile" },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "profile", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_sketch" },
                                    "name": { "type": "string" },
                                    "plane": { "$ref": "#/$defs/sketch_plane" },
                                    "profile": { "$ref": "#/$defs/profile" },
                                    "holes": { "$ref": "#/$defs/holes" },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256,
                                        "description": "Point and Line constraints only; use create_sketch_region for curve-specific constraints"
                                    },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "profile", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_sketch_region" },
                                    "name": { "type": "string" },
                                    "plane": { "$ref": "#/$defs/sketch_plane" },
                                    "profile": { "$ref": "#/$defs/sketch_loop" },
                                    "holes": { "$ref": "#/$defs/sketch_holes" },
                                    "construction": { "$ref": "#/$defs/sketch_construction" },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256,
                                        "description": "Constraints on the outer exact loop; curve constraints must reference compatible exact-curve segment ids"
                                    },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "profile", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_extrusion_from_sketch" },
                                    "name": { "type": "string" },
                                    "sketch_id": { "type": "integer", "minimum": 1 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "sketch_id", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_revolve_from_sketch" },
                                    "name": { "type": "string" },
                                    "sketch_id": { "type": "integer", "minimum": 1 },
                                    "axis_origin": { "$ref": "#/$defs/vec2" },
                                    "axis_direction": { "$ref": "#/$defs/vec2" },
                                    "angle": { "type": "number", "exclusiveMinimum": 0, "maximum": 360 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "sketch_id", "axis_origin", "axis_direction", "angle", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_loft_from_sketches" },
                                    "name": { "type": "string" },
                                    "sketch_ids": {
                                        "type": "array",
                                        "items": { "type": "integer", "minimum": 1 },
                                        "minItems": 2,
                                        "maxItems": 32,
                                        "uniqueItems": true,
                                        "description": "Ordered section sketch ids from start cap to end cap"
                                    },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "sketch_ids", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_datum_plane" },
                                    "name": { "type": "string" },
                                    "face": { "$ref": "#/$defs/face_ref" },
                                    "offset": { "type": "number" }
                                },
                                "required": ["op", "name", "face", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_datum_point" },
                                    "name": { "type": "string" },
                                    "vertex": { "$ref": "#/$defs/vertex_ref" },
                                    "offset": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "vertex", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_chamfer" },
                                    "name": { "type": "string" },
                                    "edges": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/edge_ref" },
                                        "minItems": 1,
                                        "uniqueItems": true
                                    },
                                    "distance": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "name", "edges", "distance"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_fillet" },
                                    "name": { "type": "string" },
                                    "edges": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/edge_ref" },
                                        "minItems": 1,
                                        "uniqueItems": true
                                    },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "name", "edges", "radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_boolean" },
                                    "name": { "type": "string" },
                                    "operation": { "enum": ["union", "subtract", "intersect"] },
                                    "left": { "type": "integer", "minimum": 1 },
                                    "right": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["op", "name", "operation", "left", "right"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "duplicate" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "name": { "type": "string" },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "move" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "rotate" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "rotation": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "rotation"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "Set one assembly occurrence's absolute local rigid placement and update its materialized descendant bodies atomically",
                                "properties": {
                                    "op": { "const": "set_occurrence_transform" },
                                    "assembly_id": { "type": "integer", "minimum": 1 },
                                    "occurrence_id": { "type": "integer", "minimum": 1 },
                                    "position": { "$ref": "#/$defs/vec3" },
                                    "rotation": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "assembly_id", "occurrence_id", "position", "rotation"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "Create one deterministic mate on an existing parent-child occurrence edge and solve its descendant bodies atomically",
                                "properties": {
                                    "op": { "const": "create_assembly_mate" },
                                    "assembly_id": { "type": "integer", "minimum": 1 },
                                    "mate": { "$ref": "#/$defs/assembly_mate" }
                                },
                                "required": ["op", "assembly_id", "mate"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "Set a revolute angle in degrees or slider displacement in millimeters and solve descendants atomically",
                                "properties": {
                                    "op": { "const": "set_assembly_mate_state" },
                                    "assembly_id": { "type": "integer", "minimum": 1 },
                                    "mate_id": { "type": "integer", "minimum": 1 },
                                    "state": { "type": "number" }
                                },
                                "required": ["op", "assembly_id", "mate_id", "state"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "Delete a mate while retaining its child's last solved local placement",
                                "properties": {
                                    "op": { "const": "delete_assembly_mate" },
                                    "assembly_id": { "type": "integer", "minimum": 1 },
                                    "mate_id": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["op", "assembly_id", "mate_id"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "Suppress or restore one assembly occurrence and its complete descendant subtree without changing feature visibility",
                                "properties": {
                                    "op": { "const": "set_occurrence_suppressed" },
                                    "assembly_id": { "type": "integer", "minimum": 1 },
                                    "occurrence_id": { "type": "integer", "minimum": 1 },
                                    "suppressed": { "type": "boolean" }
                                },
                                "required": ["op", "assembly_id", "occurrence_id", "suppressed"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_box" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "size": { "$ref": "#/$defs/positiveVec3" }
                                },
                                "required": ["op", "id", "size"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_cylinder" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "radius", "height"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_sphere" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_cone" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "bottom_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "top_radius": { "type": "number", "minimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "bottom_radius", "top_radius", "height"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_torus" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "major_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "minor_radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "major_radius", "minor_radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_extrusion" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/profile" },
                                    "height": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "profile", "height"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_revolve" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "axis_origin": { "$ref": "#/$defs/vec2" },
                                    "axis_direction": { "$ref": "#/$defs/vec2" },
                                    "angle": { "type": "number", "exclusiveMinimum": 0, "maximum": 360 }
                                },
                                "required": ["op", "id", "axis_origin", "axis_direction", "angle"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_sketch" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/profile" }
                                },
                                "required": ["op", "id", "profile"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_holes" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "holes": { "$ref": "#/$defs/holes" }
                                },
                                "required": ["op", "id", "holes"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_region" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/sketch_loop" },
                                    "holes": { "$ref": "#/$defs/sketch_holes" }
                                },
                                "required": ["op", "id", "profile"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_constraints" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256
                                    }
                                },
                                "required": ["op", "id", "constraints"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "Atomically replace one exact sketch region, construction geometry, and constraints",
                                "properties": {
                                    "op": { "const": "set_sketch_definition" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/sketch_loop" },
                                    "holes": { "$ref": "#/$defs/sketch_holes" },
                                    "construction": { "$ref": "#/$defs/sketch_construction" },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256
                                    }
                                },
                                "required": ["op", "id", "profile", "construction", "constraints"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_plane" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "plane": { "$ref": "#/$defs/sketch_plane" }
                                },
                                "required": ["op", "id", "plane"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_datum_plane_offset" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "offset": { "type": "number" }
                                },
                                "required": ["op", "id", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_datum_point_offset" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "offset": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_chamfer_distance" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "distance": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "distance"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_fillet_radius" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "rename" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "name": { "type": "string" }
                                },
                                "required": ["op", "id", "name"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_visibility" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "visible": { "type": "boolean" }
                                },
                                "required": ["op", "id", "visible"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_color" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "color": { "$ref": "#/$defs/color" }
                                },
                                "required": ["op", "id", "color"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_material" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "name": { "type": "string", "minLength": 1, "maxLength": 80 },
                                    "density_kg_m3": {
                                        "type": "number",
                                        "exclusiveMinimum": 0,
                                        "maximum": 100_000,
                                        "description": "Material density in kilograms per cubic meter"
                                    }
                                },
                                "required": ["op", "id", "name", "density_kg_m3"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "clear_material" },
                                    "id": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["op", "id"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "delete" },
                                    "id": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["op", "id"],
                                "additionalProperties": false
                            }
                        ]
                    }
                }
            },
            "required": ["summary", "commands"],
            "additionalProperties": false,
            "$defs": defs::command_definitions()
        }));
    let schema = tool.schema.as_mut().expect("planning tool has a schema");
    let command_schema = schema["properties"]["commands"]["items"].clone();
    schema["properties"]["alternatives"] = json!({
        "type": "array",
        "description": "Up to two complete, independent design alternatives evaluated against the same current document",
        "maxItems": 2,
        "items": {
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Short description of this distinct approach"
                },
                "commands": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 24,
                    "items": command_schema
                }
            },
            "required": ["summary", "commands"],
            "additionalProperties": false
        }
    });
    schema["required"] = json!(["summary", "commands"]);
    schema["additionalProperties"] = json!(false);
    tool
}
