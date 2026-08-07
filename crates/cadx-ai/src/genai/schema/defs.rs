//! Shared `$defs` subschemas referenced by the `apply_cad_plan` command union.

use serde_json::{Value, json};

/// Reusable JSON Schema definitions for vectors, transforms, sketch geometry,
/// constraints, and persistent topology references.
pub(super) fn command_definitions() -> Value {
    json!({
    "vec2": {
        "type": "array",
        "items": { "type": "number" },
        "minItems": 2,
        "maxItems": 2
    },
    "vec3": {
        "type": "array",
        "items": { "type": "number" },
        "minItems": 3,
        "maxItems": 3
    },
    "mat3": {
        "type": "array",
        "items": { "$ref": "#/$defs/vec3" },
        "minItems": 3,
        "maxItems": 3,
        "description": "Right-handed orthonormal row-major rotation matrix"
    },
    "rigid_transform": {
        "type": "object",
        "properties": {
            "translation": { "$ref": "#/$defs/vec3" },
            "rotation": { "$ref": "#/$defs/mat3" }
        },
        "required": ["translation", "rotation"],
        "additionalProperties": false
    },
    "mate_limits": {
        "type": "object",
        "properties": {
            "min": { "type": "number" },
            "max": { "type": "number" }
        },
        "required": ["min", "max"],
        "additionalProperties": false
    },
    "assembly_mate_kind": {
        "oneOf": [
            {
                "type": "object",
                "properties": { "type": { "const": "fixed" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "revolute" },
                    "axis": {
                        "$ref": "#/$defs/vec3",
                        "description": "Normalized axis in the parent anchor frame"
                    },
                    "limits_deg": { "$ref": "#/$defs/mate_limits" }
                },
                "required": ["type", "axis"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "slider" },
                    "axis": {
                        "$ref": "#/$defs/vec3",
                        "description": "Normalized axis in the parent anchor frame"
                    },
                    "limits_mm": { "$ref": "#/$defs/mate_limits" }
                },
                "required": ["type", "axis"],
                "additionalProperties": false
            }
        ]
    },
    "assembly_mate": {
        "type": "object",
        "properties": {
            "id": { "type": "integer", "minimum": 1 },
            "name": { "type": "string", "minLength": 1, "maxLength": 160 },
            "parent_occurrence_id": { "type": "integer", "minimum": 1 },
            "child_occurrence_id": { "type": "integer", "minimum": 1 },
            "parent_frame": { "$ref": "#/$defs/rigid_transform" },
            "child_frame": { "$ref": "#/$defs/rigid_transform" },
            "kind": { "$ref": "#/$defs/assembly_mate_kind" },
            "state": { "type": "number" }
        },
        "required": ["id", "name", "parent_occurrence_id", "child_occurrence_id", "parent_frame", "child_frame", "kind", "state"],
        "additionalProperties": false
    },
    "positiveVec3": {
        "type": "array",
        "items": { "type": "number", "exclusiveMinimum": 0 },
        "minItems": 3,
        "maxItems": 3
    },
    "color": {
        "type": "array",
        "items": { "type": "number", "minimum": 0, "maximum": 1 },
        "minItems": 4,
        "maxItems": 4
    },
    "profile": {
        "type": "array",
        "items": {
            "type": "array",
            "items": { "type": "number" },
            "minItems": 2,
            "maxItems": 2
        },
        "minItems": 3,
        "maxItems": 128
    },
    "holes": {
        "type": "array",
        "items": { "$ref": "#/$defs/profile" },
        "maxItems": 32
    },
    "sketch_segment": {
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "const": "line" },
                    "start": { "$ref": "#/$defs/vec2" },
                    "end": { "$ref": "#/$defs/vec2" }
                },
                "required": ["type", "start", "end"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "arc" },
                    "start": { "$ref": "#/$defs/vec2" },
                    "end": { "$ref": "#/$defs/vec2" },
                    "center": { "$ref": "#/$defs/vec2" },
                    "ccw": { "type": "boolean" }
                },
                "required": ["type", "start", "end", "center", "ccw"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "rational_quadratic" },
                    "start": { "$ref": "#/$defs/vec2" },
                    "control": { "$ref": "#/$defs/vec2" },
                    "end": { "$ref": "#/$defs/vec2" },
                    "weight": { "type": "number", "exclusiveMinimum": 0 }
                },
                "required": ["type", "start", "control", "end", "weight"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "cubic_bezier" },
                    "start": { "$ref": "#/$defs/vec2" },
                    "control1": { "$ref": "#/$defs/vec2" },
                    "control2": { "$ref": "#/$defs/vec2" },
                    "end": { "$ref": "#/$defs/vec2" }
                },
                "required": ["type", "start", "control1", "control2", "end"],
                "additionalProperties": false
            }
        ]
    },
    "sketch_loop": {
        "type": "array",
        "description": "Ordered exact segments with end-to-start closure",
        "items": { "$ref": "#/$defs/sketch_segment" },
        "minItems": 2,
        "maxItems": 128
    },
    "sketch_holes": {
        "type": "array",
        "items": { "$ref": "#/$defs/sketch_loop" },
        "maxItems": 32
    },
    "sketch_construction": {
        "type": "array",
        "description": "Independent exact curve segments that do not form solid boundaries",
        "items": { "$ref": "#/$defs/sketch_segment" },
        "maxItems": 128
    },
    "sketch_region": {
        "type": "object",
        "properties": {
            "profile": { "$ref": "#/$defs/sketch_loop" },
            "holes": { "$ref": "#/$defs/sketch_holes" }
        },
        "required": ["profile"],
        "additionalProperties": false
    },
    "sketch_plane": {
        "oneOf": [
            {
                "type": "object",
                "properties": { "type": { "const": "world_xy" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": { "type": { "const": "world_xz" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": { "type": { "const": "world_yz" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "datum_plane" },
                    "datum_id": { "type": "integer", "minimum": 1 }
                },
                "required": ["type", "datum_id"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "planar_face" },
                    "face": { "$ref": "#/$defs/face_ref" }
                },
                "required": ["type", "face"],
                "additionalProperties": false
            }
        ]
    },
    "constraint": {
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "const": "coincident" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "horizontal" },
                    "segment": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "segment"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "vertical" },
                    "segment": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "segment"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "fixed" },
                    "point": { "type": "integer", "minimum": 0 },
                    "x": { "type": "number" },
                    "y": { "type": "number" }
                },
                "required": ["type", "point", "x", "y"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "distance" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 },
                    "distance": { "type": "number", "minimum": 0 }
                },
                "required": ["type", "first", "second", "distance"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fix the signed X delta from the first distinct point to the second",
                "properties": {
                    "type": { "const": "horizontal_distance" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 },
                    "distance": { "type": "number" }
                },
                "required": ["type", "first", "second", "distance"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fix the signed Y delta from the first distinct point to the second",
                "properties": {
                    "type": { "const": "vertical_distance" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 },
                    "distance": { "type": "number" }
                },
                "required": ["type", "first", "second", "distance"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fix the non-negative perpendicular distance from a point to one Line support",
                "properties": {
                    "type": { "const": "point_line_distance" },
                    "point": { "type": "integer", "minimum": 0 },
                    "line": { "type": "integer", "minimum": 0 },
                    "distance": { "type": "number", "minimum": 0 }
                },
                "required": ["type", "point", "line", "distance"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require one Line support to pass through one Arc center",
                "properties": {
                    "type": { "const": "line_through_center" },
                    "line": { "type": "integer", "minimum": 0 },
                    "arc": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "line", "arc"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Place a point on one finite Line or Arc segment",
                "properties": {
                    "type": { "const": "point_on_curve" },
                    "point": { "type": "integer", "minimum": 0 },
                    "segment": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "point", "segment"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Place a point at the midpoint of a Line whose endpoints are different entities",
                "properties": {
                    "type": { "const": "midpoint" },
                    "point": { "type": "integer", "minimum": 0 },
                    "segment": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "point", "segment"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Reflect two distinct points about one Line axis",
                "properties": {
                    "type": { "const": "symmetric" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 },
                    "axis": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second", "axis"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fix the positive length of one Line segment",
                "properties": {
                    "type": { "const": "length" },
                    "segment": { "type": "integer", "minimum": 0 },
                    "length": { "type": "number", "exclusiveMinimum": 0 }
                },
                "required": ["type", "segment", "length"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require two distinct Line segments to have equal lengths",
                "properties": {
                    "type": { "const": "equal_length" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require two distinct Line segments to be parallel",
                "properties": {
                    "type": { "const": "parallel" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require two distinct Line segments to be perpendicular",
                "properties": {
                    "type": { "const": "perpendicular" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fix the directed angle from the first Line segment to the second in degrees",
                "properties": {
                    "type": { "const": "angle" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 },
                    "angle_degrees": { "type": "number", "minimum": -180, "maximum": 180 }
                },
                "required": ["type", "first", "second", "angle_degrees"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fix the radius of one Arc segment",
                "properties": {
                    "type": { "const": "radius" },
                    "segment": { "type": "integer", "minimum": 0 },
                    "radius": { "type": "number", "exclusiveMinimum": 0 }
                },
                "required": ["type", "segment", "radius"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fix the center coordinates of one Arc segment",
                "properties": {
                    "type": { "const": "fixed_center" },
                    "segment": { "type": "integer", "minimum": 0 },
                    "x": { "type": "number" },
                    "y": { "type": "number" }
                },
                "required": ["type", "segment", "x", "y"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require two Arc segments to have equal radii",
                "properties": {
                    "type": { "const": "equal_radius" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require two Arc segments to share a center",
                "properties": {
                    "type": { "const": "concentric" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require adjacent segments to be tangent; at least one must be curved",
                "properties": {
                    "type": { "const": "tangent" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Require two adjacent curved segments to have the same traversal tangent and signed curvature",
                "properties": {
                    "type": { "const": "curvature_continuous" },
                    "first": { "type": "integer", "minimum": 0 },
                    "second": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "first", "second"],
                "additionalProperties": false
            }
        ]
    },
    "face_ref": {
        "type": "object",
        "description": "Persistent face reference from the evaluated CAD scene",
        "properties": {
            "feature_id": { "type": "integer", "minimum": 1 },
            "name": {
                "type": "object",
                "description": "Serialized FaceName, including primitive or derived origin"
            }
        },
        "required": ["feature_id", "name"],
        "additionalProperties": false
    },
    "edge_ref": {
        "type": "object",
        "description": "Persistent edge reference from the evaluated CAD scene",
        "properties": {
            "feature_id": { "type": "integer", "minimum": 1 },
            "adjacent_faces": {
                "type": "array",
                "items": { "$ref": "#/$defs/face_ref" },
                "minItems": 2,
                "maxItems": 2
            },
            "fragment": { "type": "integer", "minimum": 0 }
        },
        "required": ["feature_id", "adjacent_faces", "fragment"],
        "additionalProperties": false
    },
    "vertex_ref": {
        "type": "object",
        "description": "Persistent vertex reference from the evaluated CAD scene",
        "properties": {
            "feature_id": { "type": "integer", "minimum": 1 },
            "incident_edges": {
                "type": "array",
                "items": { "$ref": "#/$defs/edge_ref" },
                "minItems": 1,
                "uniqueItems": true
            },
            "fragment": { "type": "integer", "minimum": 0 }
        },
        "required": ["feature_id", "incident_edges", "fragment"],
        "additionalProperties": false
    }
    })
}
