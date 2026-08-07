//! Schema-shape and plan-decoding tests for [`super::cad_plan_tool`].

use crate::AiPlan;
use cadx_core::domain::{BooleanOperation, ModelCommand, Primitive, SketchPlane, SketchSegment2D};

use super::*;

#[test]
fn planning_tool_schema_limits_independent_alternatives() {
    let schema = cad_plan_tool().schema.expect("planning tool has a schema");
    assert_eq!(schema["required"], json!(["summary", "commands"]));
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["alternatives"]["maxItems"], 2);
    assert_eq!(
        schema["properties"]["alternatives"]["items"]["required"],
        json!(["summary", "commands"])
    );
    assert_eq!(
        schema["properties"]["alternatives"]["items"]["properties"]["commands"]["items"],
        schema["properties"]["commands"]["items"]
    );
}

#[test]
fn tool_arguments_deserialize_into_commands() {
    let value = json!({
        "summary": "Add a mounting block",
        "commands": [{
            "op": "create_box",
            "name": "Mount",
            "size": [20.0, 10.0, 5.0],
            "position": [1.0, 2.0, 3.0]
        }]
    });

    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert_eq!(plan.commands.len(), 1);
    assert!(matches!(plan.commands[0], ModelCommand::CreateBox { .. }));
}

#[test]
fn material_commands_deserialize_for_ai_plans() {
    let value = json!({
        "summary": "Assign aluminum and clear the old tool material",
        "commands": [
            {
                "op": "set_material",
                "id": 4,
                "name": "Aluminum 6061",
                "density_kg_m3": 2700.0
            },
            {
                "op": "clear_material",
                "id": 3
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::SetMaterial { name, density_kg_m3, .. }
            if name == "Aluminum 6061" && (*density_kg_m3 - 2700.0).abs() < f64::EPSILON
    ));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::ClearMaterial { id: 3 }
    ));
}

#[test]
fn occurrence_transform_commands_deserialize_for_ai_plans() {
    let value = json!({
        "summary": "Reposition the locating pin occurrence",
        "commands": [{
            "op": "set_occurrence_transform",
            "assembly_id": 2,
            "occurrence_id": 7,
            "position": [25.0, 5.0, 0.0],
            "rotation": [0.0, 0.0, 90.0]
        }]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        plan.commands[0],
        ModelCommand::SetOccurrenceTransform {
            assembly_id: 2,
            occurrence_id: 7,
            position: [25.0, 5.0, 0.0],
            rotation: [0.0, 0.0, 90.0]
        }
    ));
}

#[test]
fn assembly_mate_commands_deserialize_for_ai_plans() {
    let identity = json!({
        "translation": [0.0, 0.0, 0.0],
        "rotation": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    });
    let value = json!({
        "summary": "Add and position the carriage travel mate",
        "commands": [
            {
                "op": "create_assembly_mate",
                "assembly_id": 2,
                "mate": {
                    "id": 9,
                    "name": "carriage travel",
                    "parent_occurrence_id": 4,
                    "child_occurrence_id": 7,
                    "parent_frame": identity,
                    "child_frame": {
                        "translation": [5.0, 0.0, 0.0],
                        "rotation": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                    },
                    "kind": {
                        "type": "slider",
                        "axis": [1.0, 0.0, 0.0],
                        "limits_mm": { "min": -20.0, "max": 20.0 }
                    },
                    "state": 3.0
                }
            },
            {
                "op": "set_assembly_mate_state",
                "assembly_id": 2,
                "mate_id": 9,
                "state": 8.0
            },
            {
                "op": "delete_assembly_mate",
                "assembly_id": 2,
                "mate_id": 9
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateAssemblyMate { assembly_id: 2, mate }
            if mate.id == 9
                && matches!(&mate.kind, cadx_core::assembly::AssemblyMateKind::Slider { .. })
    ));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::SetAssemblyMateState {
            assembly_id: 2,
            mate_id: 9,
            state: 8.0
        }
    ));
    assert!(matches!(
        plan.commands[2],
        ModelCommand::DeleteAssemblyMate {
            assembly_id: 2,
            mate_id: 9
        }
    ));

    let schema = serde_json::to_string(&cad_plan_tool()).unwrap();
    assert!(schema.contains("create_assembly_mate"));
    assert!(schema.contains("assembly_mate_kind"));
}

#[test]
fn occurrence_suppression_commands_deserialize_for_ai_plans() {
    let value = json!({
        "summary": "Suppress the alternate locating pin",
        "commands": [{
            "op": "set_occurrence_suppressed",
            "assembly_id": 2,
            "occurrence_id": 8,
            "suppressed": true
        }]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        plan.commands[0],
        ModelCommand::SetOccurrenceSuppressed {
            assembly_id: 2,
            occurrence_id: 8,
            suppressed: true,
        }
    ));
}

#[test]
fn new_primitive_and_duplicate_commands_are_supported() {
    let value = json!({
        "summary": "Add a seal and a second one",
        "commands": [
            {
                "op": "create_torus",
                "name": "Seal",
                "major_radius": 12.0,
                "minor_radius": 3.0,
                "position": [0.0, 0.0, 5.0]
            },
            {
                "op": "create_extrusion",
                "name": "Plate",
                "profile": [[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]],
                "height": 8.0,
                "position": [0.0, 0.0, 0.0]
            },
            {
                "op": "create_sketch",
                "name": "Outline",
                "profile": [[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]],
                "position": [0.0, 0.0, 0.0]
            },
            {
                "op": "create_extrusion_from_sketch",
                "name": "Pad",
                "sketch_id": 3,
                "height": 5.0,
                "position": [0.0, 0.0, 0.0]
            },
            {
                "op": "duplicate",
                "id": 1,
                "position": [30.0, 0.0, 5.0]
            },
            {
                "op": "set_color",
                "id": 2,
                "color": [0.1, 0.2, 0.3, 1.0]
            },
            {
                "op": "create_boolean",
                "name": "Union",
                "operation": "union",
                "left": 1,
                "right": 2
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(plan.commands[0], ModelCommand::CreateTorus { .. }));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::CreateExtrusion { .. }
    ));
    assert!(matches!(
        plan.commands[2],
        ModelCommand::CreateSketch { .. }
    ));
    assert!(matches!(
        plan.commands[3],
        ModelCommand::CreateExtrusionFromSketch { .. }
    ));
    assert!(matches!(plan.commands[4], ModelCommand::Duplicate { .. }));
    assert!(matches!(plan.commands[5], ModelCommand::SetColor { .. }));
    assert!(matches!(
        plan.commands[6],
        ModelCommand::CreateBoolean {
            operation: BooleanOperation::Union,
            left: 1,
            right: 2,
            ..
        }
    ));
}

#[test]
fn sketch_constraint_commands_deserialize_for_ai_plans() {
    let value = json!({
        "summary": "Constrain the outline",
        "commands": [
            {
                "op": "create_sketch",
                "name": "Outline",
                "profile": [[0.0, 0.0], [9.0, 1.0], [10.0, 5.0], [0.0, 5.0]],
                "constraints": [
                    { "type": "horizontal", "segment": 0 },
                    { "type": "distance", "first": 0, "second": 1, "distance": 10.0 }
                ],
                "position": [0.0, 0.0, 0.0]
            },
            {
                "op": "set_sketch_constraints",
                "id": 1,
                "constraints": [{ "type": "fixed", "point": 0, "x": 0.0, "y": 0.0 }]
            },
            {
                "op": "set_sketch_definition",
                "id": 1,
                "profile": [
                    { "type": "line", "start": [0.0, 0.0], "end": [4.0, 0.0] },
                    { "type": "line", "start": [4.0, 0.0], "end": [0.0, 3.0] },
                    { "type": "line", "start": [0.0, 3.0], "end": [0.0, 0.0] }
                ],
                "holes": [],
                "construction": [
                    { "type": "line", "start": [-1.0, 1.0], "end": [5.0, 1.0] }
                ],
                "constraints": [
                    { "type": "point_on_curve", "point": 1, "segment": 3 }
                ]
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateSketch { constraints, .. } if constraints.len() == 2
    ));
    assert!(matches!(
        &plan.commands[1],
        ModelCommand::SetSketchConstraints { constraints, .. } if constraints.len() == 1
    ));
    assert!(matches!(
        &plan.commands[2],
        ModelCommand::SetSketchDefinition {
            construction,
            constraints,
            ..
        } if construction.len() == 1 && constraints.len() == 1
    ));
}

#[test]
fn sketch_hole_commands_deserialize_for_ai_plans() {
    let first_hole = json!([[6.0, 4.0], [10.0, 4.0], [10.0, 8.0], [6.0, 8.0]]);
    let value = json!({
        "summary": "Create a plate profile with a window",
        "commands": [
            {
                "op": "create_sketch",
                "name": "Window plate",
                "profile": [[0.0, 0.0], [16.0, 0.0], [16.0, 12.0], [0.0, 12.0]],
                "holes": [first_hole.clone()],
                "position": [0.0, 0.0, 0.0]
            },
            {
                "op": "set_sketch_holes",
                "id": 1,
                "holes": [first_hole]
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateSketch { holes, .. } if holes.len() == 1 && holes[0].len() == 4
    ));
    assert!(matches!(
        &plan.commands[1],
        ModelCommand::SetSketchHoles { holes, .. } if holes.len() == 1
    ));
}

#[test]
fn sketch_plane_commands_deserialize_for_ai_plans() {
    let value = json!({
        "summary": "Attach a profile to the machining datum",
        "commands": [
            {
                "op": "create_sketch",
                "name": "Datum profile",
                "plane": { "type": "datum_plane", "datum_id": 8 },
                "profile": [[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]],
                "position": [0.0, 0.0, 0.0]
            },
            {
                "op": "set_sketch_plane",
                "id": 9,
                "plane": { "type": "world_yz" }
            },
            {
                "op": "set_sketch_plane",
                "id": 10,
                "plane": {
                    "type": "planar_face",
                    "face": {
                        "feature_id": 4,
                        "name": {
                            "origin": "primitive",
                            "face": { "role": "box_z_max" }
                        }
                    }
                }
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        plan.commands[0],
        ModelCommand::CreateSketch {
            plane: SketchPlane::DatumPlane { datum_id: 8 },
            ..
        }
    ));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::SetSketchPlane {
            id: 9,
            plane: SketchPlane::WorldYz
        }
    ));
    assert!(matches!(
        &plan.commands[2],
        ModelCommand::SetSketchPlane {
            id: 10,
            plane: SketchPlane::PlanarFace { face }
        } if face.feature_id == 4
    ));
}

#[test]
fn revolve_commands_deserialize_for_ai_plans() {
    let value = json!({
        "summary": "Turn the profile into a turned body",
        "commands": [
            {
                "op": "create_revolve_from_sketch",
                "name": "Turned body",
                "sketch_id": 3,
                "axis_origin": [20.0, 0.0],
                "axis_direction": [0.0, 1.0],
                "angle": 270.0,
                "position": [0.0, 0.0, 0.0]
            },
            {
                "op": "resize_revolve",
                "id": 4,
                "axis_origin": [25.0, 0.0],
                "axis_direction": [0.0, 1.0],
                "angle": 360.0
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        plan.commands[0],
        ModelCommand::CreateRevolveFromSketch { angle, .. } if (angle - 270.0).abs() < f64::EPSILON
    ));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::ResizeRevolve { .. }
    ));
}

#[test]
fn loft_commands_deserialize_for_ai_plans() {
    let value = json!({
        "summary": "Loft three ordered exact sections",
        "commands": [{
            "op": "create_loft_from_sketches",
            "name": "Transition body",
            "sketch_ids": [3, 5, 8],
            "position": [1.0, 2.0, 3.0]
        }]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateLoftFromSketches {
            sketch_ids,
            position,
            ..
        } if sketch_ids == &[3, 5, 8]
            && position
                .iter()
                .zip([1.0, 2.0, 3.0])
                .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
    ));
}

#[test]
fn datum_plane_tool_arguments_deserialize_with_a_face_ref() {
    let value = json!({
        "summary": "Create a machining datum",
        "commands": [{
            "op": "create_datum_plane",
            "name": "Top datum",
            "face": {
                "feature_id": 12,
                "name": {
                    "origin": "primitive",
                    "face": { "role": "box_z_max" }
                }
            },
            "offset": 0.0
        }]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateDatumPlane { face, .. } if face.feature_id == 12
    ));
}

#[test]
fn datum_point_tool_arguments_deserialize_with_a_vertex_ref() {
    let face = |role| {
        json!({
            "feature_id": 12,
            "name": {
                "origin": "primitive",
                "face": { "role": role }
            }
        })
    };
    let value = json!({
        "summary": "Create and move a setup point",
        "commands": [
            {
                "op": "create_datum_point",
                "name": "Setup origin",
                "vertex": {
                    "feature_id": 12,
                    "incident_edges": [{
                        "feature_id": 12,
                        "adjacent_faces": [face("box_x_min"), face("box_y_min")],
                        "fragment": 0
                    }],
                    "fragment": 0
                },
                "offset": [1.0, 2.0, 3.0]
            },
            {
                "op": "set_datum_point_offset",
                "id": 13,
                "offset": [4.0, 5.0, 6.0]
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateDatumPoint { vertex, offset, .. }
            if vertex.feature_id == 12 && offset.iter().zip([1.0, 2.0, 3.0])
                .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
    ));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::SetDatumPointOffset { id: 13, offset }
            if offset.iter().zip([4.0, 5.0, 6.0])
                .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
    ));
}

#[test]
fn chamfer_tool_arguments_deserialize_with_a_persistent_edge_ref() {
    let face = |role| {
        json!({
            "feature_id": 4,
            "name": {
                "origin": "primitive",
                "face": { "role": role }
            }
        })
    };
    let value = json!({
        "summary": "Break the selected edge",
        "commands": [
            {
                "op": "create_chamfer",
                "name": "Edge break",
                "edges": [{
                    "feature_id": 4,
                    "adjacent_faces": [face("box_x_max"), face("box_z_max")],
                    "fragment": 0
                }],
                "distance": 1.5
            },
            {
                "op": "set_chamfer_distance",
                "id": 5,
                "distance": 2.0
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateChamfer { edges, distance, .. }
            if edges.len() == 1 && edges[0].feature_id == 4
                && (*distance - 1.5).abs() < f64::EPSILON
    ));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::SetChamferDistance { id: 5, distance }
            if (distance - 2.0).abs() < f64::EPSILON
    ));
}

#[test]
fn fillet_tool_arguments_deserialize_with_a_persistent_edge_ref() {
    let face = |role| {
        json!({
            "feature_id": 4,
            "name": {
                "origin": "primitive",
                "face": { "role": role }
            }
        })
    };
    let value = json!({
        "summary": "Round the selected edge",
        "commands": [
            {
                "op": "create_fillet",
                "name": "Edge round",
                "edges": [{
                    "feature_id": 4,
                    "adjacent_faces": [face("box_x_max"), face("box_z_max")],
                    "fragment": 0
                }],
                "radius": 1.5
            },
            {
                "op": "set_fillet_radius",
                "id": 5,
                "radius": 2.0
            }
        ]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    assert!(matches!(
        &plan.commands[0],
        ModelCommand::CreateFillet { edges, radius, .. }
            if edges.len() == 1 && edges[0].feature_id == 4
                && (*radius - 1.5).abs() < f64::EPSILON
    ));
    assert!(matches!(
        plan.commands[1],
        ModelCommand::SetFilletRadius { id: 5, radius }
            if (radius - 2.0).abs() < f64::EPSILON
    ));
}

#[test]
fn freeform_curve_plan_deserializes_and_executes_atomically() {
    let value = json!({
        "summary": "Create an exact freeform sketch",
        "commands": [{
            "op": "create_sketch_region",
            "name": "Freeform profile",
            "plane": { "type": "world_xy" },
            "profile": [
                {
                    "type": "cubic_bezier",
                    "start": [0.0, 0.0],
                    "control1": [3.0, -2.0],
                    "control2": [7.0, -2.0],
                    "end": [10.0, 0.0]
                },
                { "type": "line", "start": [10.0, 0.0], "end": [10.0, 10.0] },
                {
                    "type": "rational_quadratic",
                    "start": [10.0, 10.0],
                    "control": [5.0, 14.0],
                    "end": [0.0, 10.0],
                    "weight": 0.8
                },
                { "type": "line", "start": [0.0, 10.0], "end": [0.0, 0.0] }
            ],
            "holes": [],
            "construction": [],
            "constraints": [],
            "position": [0.0, 0.0, 0.0]
        }]
    });
    let plan: AiPlan = serde_json::from_value(value).unwrap();
    let mut document = cadx_core::domain::CadDocument::default();
    document.apply_transaction(plan.commands).unwrap();
    let Primitive::Sketch { region, .. } = &document.features[0].primitive else {
        panic!("expected exact freeform sketch");
    };
    assert!(matches!(
        region.profile.segments[0],
        SketchSegment2D::CubicBezier { .. }
    ));
    assert!(matches!(
        region.profile.segments[2],
        SketchSegment2D::RationalQuadratic { weight, .. } if (weight - 0.8).abs() < f64::EPSILON
    ));
}
