use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{CadDocument, DocumentError, Primitive, SketchPlane, Vec3};

pub const FORMAT_NAME: &str = "cadx.document";
pub const CURRENT_VERSION: u32 = 22;

#[derive(Debug, Serialize, Deserialize)]
struct DocumentEnvelope {
    format: String,
    version: u32,
    document: CadDocument,
}

/// Encodes a document into the versioned CADX interchange format.
///
/// # Errors
///
/// Returns [`PersistenceError`] when validation or JSON serialization fails.
pub fn encode(document: &CadDocument) -> Result<String, PersistenceError> {
    let mut checked = document.clone();
    checked.validate_and_repair()?;
    let envelope = DocumentEnvelope {
        format: FORMAT_NAME.into(),
        version: CURRENT_VERSION,
        document: checked,
    };
    serde_json::to_string_pretty(&envelope).map_err(PersistenceError::Json)
}

/// Decodes and validates a CADX document without mutating application state.
///
/// # Errors
///
/// Returns [`PersistenceError`] for malformed, unsupported, or invalid files.
pub fn decode(json: &str) -> Result<CadDocument, PersistenceError> {
    let mut envelope: DocumentEnvelope = serde_json::from_str(json)?;
    if envelope.format != FORMAT_NAME {
        return Err(PersistenceError::WrongFormat(envelope.format));
    }
    if envelope.version == 0 || envelope.version > CURRENT_VERSION {
        return Err(PersistenceError::UnsupportedVersion(envelope.version));
    }
    migrate(&mut envelope.document, envelope.version)?;
    envelope.document.validate_and_repair()?;
    Ok(envelope.document)
}

fn migrate(document: &mut CadDocument, source_version: u32) -> Result<(), DocumentError> {
    if source_version >= 12 {
        return Ok(());
    }

    let mut usage = std::collections::HashMap::<_, (bool, bool)>::new();
    for feature in &document.features {
        match feature.primitive {
            Primitive::ExtrusionFromSketch { sketch_id, .. } => {
                usage.entry(sketch_id).or_default().0 = true;
            }
            Primitive::RevolveFromSketch { sketch_id, .. } => {
                usage.entry(sketch_id).or_default().1 = true;
            }
            _ => {}
        }
    }
    let mut next_id = document
        .features
        .iter()
        .map(|feature| feature.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(DocumentError::IdOverflow)?;
    let mut revolve_sketches = std::collections::HashMap::new();
    let mut clones = Vec::new();
    for feature in &mut document.features {
        if let Primitive::Sketch { plane, .. } = &mut feature.primitive {
            let (used_by_extrusion, used_by_revolve) =
                usage.get(&feature.id).copied().unwrap_or_default();
            *plane = if used_by_revolve && !used_by_extrusion {
                SketchPlane::WorldXz
            } else {
                SketchPlane::WorldXy
            };
            feature.translation = Vec3::ZERO;
            feature.rotation = Vec3::ZERO;
            if used_by_extrusion && used_by_revolve {
                let mut revolve_sketch = feature.clone();
                revolve_sketch.id = next_id;
                revolve_sketch.name = format!("{} (revolve plane)", feature.name);
                if let Primitive::Sketch { plane, .. } = &mut revolve_sketch.primitive {
                    *plane = SketchPlane::WorldXz;
                }
                revolve_sketches.insert(feature.id, next_id);
                clones.push(revolve_sketch);
                next_id = next_id.checked_add(1).ok_or(DocumentError::IdOverflow)?;
            }
        }
    }
    for feature in &mut document.features {
        if let Primitive::RevolveFromSketch { sketch_id, .. } = &mut feature.primitive
            && let Some(replacement) = revolve_sketches.get(sketch_id)
        {
            *sketch_id = *replacement;
        }
    }
    document.features.extend(clones);
    Ok(())
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("document is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported document format: {0}")]
    WrongFormat(String),
    #[error("unsupported document version: {0}")]
    UnsupportedVersion(u32),
    #[error("invalid document: {0}")]
    InvalidDocument(#[from] DocumentError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_a_document() {
        let mut source = CadDocument::demo();
        source
            .apply(crate::domain::ModelCommand::SetMaterial {
                id: 1,
                name: "Aluminum 6061".into(),
                density_kg_m3: 2_700.0,
            })
            .unwrap();
        let encoded = encode(&source).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(encoded.contains("\"format\": \"cadx.document\""));
    }

    #[test]
    fn rejects_future_versions() {
        let encoded = encode(&CadDocument::default()).unwrap();
        let future = encoded.replace(
            &format!("\"version\": {CURRENT_VERSION}"),
            "\"version\": 99",
        );
        assert!(matches!(
            decode(&future),
            Err(PersistenceError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn reads_legacy_documents() {
        let encoded = encode(&CadDocument::demo()).unwrap();
        for version in 1..=19 {
            let legacy = encoded.replace(
                &format!("\"version\": {CURRENT_VERSION}"),
                &format!("\"version\": {version}"),
            );
            assert_eq!(decode(&legacy).unwrap(), CadDocument::demo());
        }
    }

    #[test]
    fn reads_v3_sketches_without_constraint_fields() {
        let mut source = CadDocument::default();
        source
            .apply(crate::domain::ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "legacy sketch".into(),
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 3.into();
        legacy["document"]["features"][0]["primitive"]
            .as_object_mut()
            .unwrap()
            .remove("constraints");
        assert_eq!(decode(&legacy.to_string()).unwrap(), source);
    }

    #[test]
    fn reads_v6_features_without_material_fields() {
        let source = CadDocument::demo();
        let mut legacy: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 6.into();
        for feature in legacy["document"]["features"].as_array_mut().unwrap() {
            feature.as_object_mut().unwrap().remove("material");
        }
        assert_eq!(decode(&legacy.to_string()).unwrap(), source);
    }

    #[test]
    fn round_trip_preserves_boolean_dependencies() {
        use crate::domain::{BooleanOperation, ModelCommand};

        let mut source = CadDocument::default();
        let ids = source
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "base".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateCylinder {
                    name: "tool".into(),
                    radius: 3.0,
                    height: 10.0,
                    position: [2.0, 2.0, 0.0],
                },
            ])
            .unwrap();
        source
            .apply(ModelCommand::CreateBoolean {
                name: "cut".into(),
                operation: BooleanOperation::Subtract,
                left: ids[0],
                right: ids[1],
            })
            .unwrap();
        let decoded = decode(&encode(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
        assert_eq!(decoded.feature_graph().unwrap().order().len(), 3);
    }

    #[test]
    fn round_trip_preserves_sketch_dependencies() {
        let mut source = CadDocument::default();
        let sketch_id = source
            .apply(crate::domain::ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "profile".into(),
                profile: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]],
                holes: Vec::new(),
                constraints: vec![crate::domain::Constraint::Horizontal { segment: 0 }],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        source
            .apply(crate::domain::ModelCommand::CreateExtrusionFromSketch {
                name: "pad".into(),
                sketch_id,
                height: 4.0,
                position: [0.0; 3],
            })
            .unwrap();
        let decoded = decode(&encode(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
    }

    #[test]
    fn round_trip_preserves_v16_curve_constraints() {
        use crate::domain::{Constraint, ModelCommand, Primitive, SketchLoop2D, SketchRegion2D};
        use cadx_sketch::SketchSegment2D;

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketchRegion {
                name: "constrained circle".into(),
                plane: SketchPlane::WorldXy,
                region: SketchRegion2D {
                    profile: SketchLoop2D {
                        segments: vec![
                            SketchSegment2D::Arc {
                                start: [5.0, -2.0],
                                end: [-3.0, -2.0],
                                center: [1.0, -2.0],
                                ccw: true,
                            },
                            SketchSegment2D::Arc {
                                start: [-3.0, -2.0],
                                end: [5.0, -2.0],
                                center: [1.0, -2.0],
                                ccw: true,
                            },
                        ],
                    },
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: vec![
                    Constraint::FixedCenter {
                        segment: 0,
                        x: 3.0,
                        y: 5.0,
                    },
                    Constraint::Radius {
                        segment: 0,
                        radius: 6.0,
                    },
                    Constraint::Concentric {
                        first: 0,
                        second: 1,
                    },
                    Constraint::EqualRadius {
                        first: 0,
                        second: 1,
                    },
                    Constraint::Tangent {
                        first: 0,
                        second: 1,
                    },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let encoded = encode(&source).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(encoded.contains(&format!("\"version\": {CURRENT_VERSION}")));
        assert!(encoded.contains("\"type\": \"fixed_center\""));
        assert!(encoded.contains("\"type\": \"radius\""));
        assert!(encoded.contains("\"type\": \"concentric\""));
        assert!(encoded.contains("\"type\": \"equal_radius\""));
        assert!(encoded.contains("\"type\": \"tangent\""));
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch { constraints, .. } if constraints.len() == 5
        ));
    }

    #[test]
    fn round_trip_preserves_v17_advanced_line_constraints() {
        use crate::domain::{Constraint, ModelCommand, Primitive};

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketch {
                name: "advanced constrained profile".into(),
                plane: SketchPlane::WorldXy,
                profile: vec![[0.0, 0.0], [9.0, 1.0], [10.0, 6.0], [1.0, 5.0]],
                holes: Vec::new(),
                constraints: vec![
                    Constraint::Length {
                        segment: 0,
                        length: 10.0,
                    },
                    Constraint::EqualLength {
                        first: 0,
                        second: 2,
                    },
                    Constraint::Parallel {
                        first: 0,
                        second: 2,
                    },
                    Constraint::Perpendicular {
                        first: 0,
                        second: 1,
                    },
                    Constraint::Angle {
                        first: 0,
                        second: 3,
                        angle_degrees: -90.0,
                    },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let encoded = encode(&source).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(encoded.contains(&format!("\"version\": {CURRENT_VERSION}")));
        for constraint_type in [
            "length",
            "equal_length",
            "parallel",
            "perpendicular",
            "angle",
        ] {
            assert!(encoded.contains(&format!("\"type\": \"{constraint_type}\"")));
        }
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch { constraints, .. } if constraints.len() == 5
        ));
    }

    #[test]
    fn round_trip_preserves_v18_construction_and_point_relationships() {
        use crate::domain::{
            Constraint, ModelCommand, Primitive, SketchLoop2D, SketchRegion2D, SketchSegment2D,
        };

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketchRegion {
                name: "construction constrained profile".into(),
                plane: SketchPlane::WorldXy,
                region: SketchRegion2D {
                    profile: SketchLoop2D::from_polygon(vec![
                        [0.0, 0.0],
                        [10.0, 0.0],
                        [10.0, 8.0],
                        [0.0, 8.0],
                    ]),
                    holes: Vec::new(),
                },
                construction: vec![
                    SketchSegment2D::Line {
                        start: [0.0, -5.0],
                        end: [0.0, 5.0],
                    },
                    SketchSegment2D::Line {
                        start: [-3.0, 2.0],
                        end: [-5.0, 2.0],
                    },
                    SketchSegment2D::Line {
                        start: [3.0, 2.0],
                        end: [5.0, 2.0],
                    },
                    SketchSegment2D::Line {
                        start: [-5.0, 0.0],
                        end: [-5.0, 4.0],
                    },
                    SketchSegment2D::Line {
                        start: [5.0, 0.0],
                        end: [5.0, 4.0],
                    },
                ],
                constraints: vec![
                    Constraint::Symmetric {
                        first: 6,
                        second: 8,
                        axis: 4,
                    },
                    Constraint::Midpoint {
                        point: 7,
                        segment: 7,
                    },
                    Constraint::PointOnCurve {
                        point: 9,
                        segment: 8,
                    },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let encoded = encode(&source).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(encoded.contains(&format!("\"version\": {CURRENT_VERSION}")));
        assert!(encoded.contains("\"construction\""));
        for constraint_type in ["point_on_curve", "midpoint", "symmetric"] {
            assert!(encoded.contains(&format!("\"type\": \"{constraint_type}\"")));
        }
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch {
                construction,
                constraints,
                ..
            } if construction.len() == 5 && constraints.len() == 3
        ));
    }

    #[test]
    fn reads_v17_sketches_without_construction_geometry() {
        use crate::domain::ModelCommand;

        let mut source = CadDocument::default();
        source
            .apply(ModelCommand::CreateSketch {
                name: "legacy profile".into(),
                plane: SketchPlane::WorldXy,
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap();
        let mut legacy =
            serde_json::from_str::<serde_json::Value>(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 17.into();
        legacy["document"]["features"][0]["primitive"]
            .as_object_mut()
            .unwrap()
            .remove("construction");
        assert_eq!(decode(&legacy.to_string()).unwrap(), source);
    }

    #[test]
    fn round_trip_preserves_v19_point_dimensions_and_center_relationship() {
        use crate::domain::{Constraint, ModelCommand, Primitive, SketchRegion2D, SketchSegment2D};

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketchRegion {
                name: "dimensioned reference geometry".into(),
                plane: SketchPlane::WorldXy,
                region: SketchRegion2D::from_polygons(
                    vec![[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]],
                    Vec::new(),
                ),
                construction: vec![
                    SketchSegment2D::Line {
                        start: [0.0, 0.0],
                        end: [4.0, 0.0],
                    },
                    SketchSegment2D::Arc {
                        start: [8.0, 2.0],
                        end: [4.0, 2.0],
                        center: [6.0, 0.0],
                        ccw: true,
                    },
                ],
                constraints: vec![
                    Constraint::HorizontalDistance {
                        first: 4,
                        second: 5,
                        distance: 4.0,
                    },
                    Constraint::VerticalDistance {
                        first: 4,
                        second: 5,
                        distance: 0.0,
                    },
                    Constraint::PointLineDistance {
                        point: 6,
                        line: 4,
                        distance: 2.0,
                    },
                    Constraint::LineThroughCenter { line: 4, arc: 5 },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let encoded = encode(&source).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        for constraint_type in [
            "horizontal_distance",
            "vertical_distance",
            "point_line_distance",
            "line_through_center",
        ] {
            assert!(encoded.contains(&format!("\"type\": \"{constraint_type}\"")));
        }
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch { constraints, .. } if constraints.len() == 4
        ));
    }

    #[test]
    fn round_trip_preserves_v20_curvature_continuity() {
        use crate::domain::{Constraint, ModelCommand, Primitive, SketchRegion2D, SketchSegment2D};

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketchRegion {
                name: "G2 circle".into(),
                plane: SketchPlane::WorldXy,
                region: SketchRegion2D {
                    profile: crate::domain::SketchLoop2D {
                        segments: vec![
                            SketchSegment2D::Arc {
                                start: [5.0, 0.0],
                                end: [-5.0, 0.0],
                                center: [0.0, 0.0],
                                ccw: true,
                            },
                            SketchSegment2D::Arc {
                                start: [-5.0, 0.0],
                                end: [5.0, 0.0],
                                center: [0.0, 0.0],
                                ccw: true,
                            },
                        ],
                    },
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: vec![Constraint::CurvatureContinuous {
                    first: 0,
                    second: 1,
                }],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let current = encode(&source).unwrap();
        assert!(current.contains(&format!("\"version\": {CURRENT_VERSION}")));
        assert!(current.contains("\"type\": \"curvature_continuous\""));
        let mut encoded: serde_json::Value = serde_json::from_str(&current).unwrap();
        encoded["version"] = 20.into();
        let encoded = encoded.to_string();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch { constraints, .. }
                if matches!(constraints.as_slice(), [Constraint::CurvatureContinuous {
                    first: 0,
                    second: 1,
                }])
        ));
    }

    #[test]
    fn round_trip_preserves_sketch_holes_and_cached_extrusion_loops() {
        use crate::domain::{ModelCommand, Primitive, SketchPlane};

        let mut source = CadDocument::default();
        let hole = vec![[6.0, 4.0], [10.0, 4.0], [10.0, 8.0], [6.0, 8.0]];
        let sketch = source
            .apply(ModelCommand::CreateSketch {
                name: "window profile".into(),
                plane: SketchPlane::WorldXy,
                profile: vec![[0.0, 0.0], [16.0, 0.0], [16.0, 12.0], [0.0, 12.0]],
                holes: vec![hole.clone()],
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let extrusion = source
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "window plate".into(),
                sketch_id: sketch,
                height: 3.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let encoded = encode(&source).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(encoded.contains(&format!("\"version\": {CURRENT_VERSION}")));
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch { region, .. }
                if region.holes.len() == 1 && region.holes[0].vertices() == hole
        ));
        assert!(matches!(
            &decoded.feature(extrusion).unwrap().primitive,
            Primitive::ExtrusionFromSketch { region, .. }
                if region.holes.len() == 1 && region.holes[0].vertices() == hole
        ));
    }

    #[test]
    fn reads_v14_point_loops_as_exact_linear_segments() {
        use crate::domain::{ModelCommand, Primitive, SketchPlane};

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketch {
                name: "legacy window".into(),
                plane: SketchPlane::WorldXy,
                profile: vec![[0.0, 0.0], [16.0, 0.0], [16.0, 12.0], [0.0, 12.0]],
                holes: vec![vec![[6.0, 4.0], [10.0, 4.0], [10.0, 8.0], [6.0, 8.0]]],
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        source
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "legacy plate".into(),
                sketch_id: sketch,
                height: 3.0,
                position: [0.0; 3],
            })
            .unwrap();

        let mut legacy: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 14.into();
        for feature in legacy["document"]["features"].as_array_mut().unwrap() {
            let primitive = &mut feature["primitive"];
            if !matches!(
                primitive["type"].as_str(),
                Some("sketch" | "extrusion_from_sketch")
            ) {
                continue;
            }
            let profile = primitive["profile"]
                .as_array()
                .unwrap()
                .iter()
                .map(|segment| segment["start"].clone())
                .collect();
            primitive["profile"] = serde_json::Value::Array(profile);
            let holes = primitive["holes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|hole| {
                    serde_json::Value::Array(
                        hole.as_array()
                            .unwrap()
                            .iter()
                            .map(|segment| segment["start"].clone())
                            .collect(),
                    )
                })
                .collect();
            primitive["holes"] = serde_json::Value::Array(holes);
        }

        let decoded = decode(&legacy.to_string()).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch { region, .. }
                if region.profile.is_linear() && region.holes[0].is_linear()
        ));
    }

    #[test]
    fn reads_v13_sketches_without_hole_fields_as_single_loop_profiles() {
        let mut source = CadDocument::default();
        source
            .apply(crate::domain::ModelCommand::CreateSketch {
                name: "v13 profile".into(),
                plane: SketchPlane::WorldXy,
                profile: vec![[0.0, 0.0], [8.0, 0.0], [8.0, 6.0], [0.0, 6.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [2.0, 3.0, 4.0],
            })
            .unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 13.into();
        legacy["document"]["features"][0]["primitive"]
            .as_object_mut()
            .unwrap()
            .remove("holes");

        assert_eq!(decode(&legacy.to_string()).unwrap(), source);
    }

    #[test]
    fn round_trip_preserves_datum_attached_sketch_planes() {
        use crate::domain::{ModelCommand, Primitive, SketchPlane};
        use crate::topology::{FaceRef, PrimitiveFace};

        let mut source = CadDocument::default();
        let body = source
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let datum = source
            .apply(ModelCommand::CreateDatumPlane {
                name: "top datum".into(),
                face: FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                offset: 2.0,
            })
            .unwrap()
            .unwrap();
        source
            .apply(ModelCommand::CreateSketch {
                name: "attached".into(),
                plane: SketchPlane::DatumPlane { datum_id: datum },
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [1.0, 2.0, 0.0],
            })
            .unwrap();
        let decoded = decode(&encode(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            decoded.features.last().map(|feature| &feature.primitive),
            Some(Primitive::Sketch {
                plane: SketchPlane::DatumPlane { datum_id },
                ..
            }) if *datum_id == datum
        ));
    }

    #[test]
    fn round_trip_preserves_planar_face_attached_sketch_planes() {
        use crate::domain::{ModelCommand, Primitive, SketchPlane};
        use crate::topology::{FaceRef, PrimitiveFace};

        let mut source = CadDocument::default();
        let body = source
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let face = FaceRef::primitive(body, PrimitiveFace::BoxYMax);
        let sketch = source
            .apply(ModelCommand::CreateSketch {
                name: "face attached".into(),
                plane: SketchPlane::PlanarFace { face: face.clone() },
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [1.0, 2.0, 0.0],
            })
            .unwrap()
            .unwrap();

        let encoded = encode(&source).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(encoded.contains(&format!("\"version\": {CURRENT_VERSION}")));
        assert_eq!(
            decoded.feature_graph().unwrap().dependencies(sketch),
            Some(&[body][..])
        );
        assert!(matches!(
            &decoded.feature(sketch).unwrap().primitive,
            Primitive::Sketch {
                plane: SketchPlane::PlanarFace { face: actual },
                ..
            } if actual == &face
        ));
    }

    #[test]
    fn reads_v12_sketch_planes_without_legacy_migration() {
        use crate::domain::{ModelCommand, Primitive, SketchPlane};

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketch {
                name: "v12 profile".into(),
                plane: SketchPlane::WorldXy,
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [7.0, 8.0, 9.0],
            })
            .unwrap()
            .unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 12.into();
        legacy["document"]["features"][0]["primitive"]
            .as_object_mut()
            .unwrap()
            .remove("plane");

        let decoded = decode(&legacy.to_string()).unwrap();
        let feature = decoded.feature(sketch).unwrap();
        assert_eq!(feature.translation, Vec3::new(7.0, 8.0, 9.0));
        assert!(matches!(
            feature.primitive,
            Primitive::Sketch {
                plane: SketchPlane::WorldXy,
                ..
            }
        ));
    }

    #[test]
    fn reads_v11_sketch_drivers_without_changing_legacy_geometry_semantics() {
        use crate::domain::{ModelCommand, Primitive, SketchPlane};

        let mut source = CadDocument::default();
        let sketch = source
            .apply(ModelCommand::CreateSketch {
                name: "shared legacy profile".into(),
                plane: SketchPlane::WorldXy,
                profile: vec![[5.0, 0.0], [10.0, 0.0], [10.0, 12.0], [5.0, 12.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [7.0, 8.0, 9.0],
            })
            .unwrap()
            .unwrap();
        let extrusion = source
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "legacy pad".into(),
                sketch_id: sketch,
                height: 4.0,
                position: [2.0, 3.0, 4.0],
            })
            .unwrap()
            .unwrap();
        let revolve = source
            .apply(ModelCommand::CreateRevolveFromSketch {
                name: "legacy turn".into(),
                sketch_id: sketch,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 360.0,
                position: [11.0, 12.0, 13.0],
            })
            .unwrap()
            .unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 11.into();
        legacy["document"]["features"][0]["primitive"]
            .as_object_mut()
            .unwrap()
            .remove("plane");

        let decoded = decode(&legacy.to_string()).unwrap();
        let original = decoded.feature(sketch).unwrap();
        assert_eq!(original.translation, Vec3::ZERO);
        assert!(matches!(
            original.primitive,
            Primitive::Sketch {
                plane: SketchPlane::WorldXy,
                ..
            }
        ));
        assert!(matches!(
            decoded.feature(extrusion).unwrap().primitive,
            Primitive::ExtrusionFromSketch { sketch_id, .. } if sketch_id == sketch
        ));
        let Primitive::RevolveFromSketch {
            sketch_id: revolve_sketch,
            ..
        } = decoded.feature(revolve).unwrap().primitive
        else {
            panic!("expected legacy revolve");
        };
        assert_ne!(revolve_sketch, sketch);
        assert!(matches!(
            decoded.feature(revolve_sketch).unwrap().primitive,
            Primitive::Sketch {
                plane: SketchPlane::WorldXz,
                ..
            }
        ));
        assert_eq!(
            decoded.feature(extrusion).unwrap().translation,
            Vec3::new(2.0, 3.0, 4.0)
        );
        assert_eq!(
            decoded.feature(revolve).unwrap().translation,
            Vec3::new(11.0, 12.0, 13.0)
        );
    }

    #[test]
    fn round_trip_preserves_revolve_parameters() {
        let mut source = CadDocument::default();
        let sketch_id = source
            .apply(crate::domain::ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "turning profile".into(),
                profile: vec![[5.0, 0.0], [10.0, 0.0], [10.0, 12.0], [5.0, 12.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        source
            .apply(crate::domain::ModelCommand::CreateRevolveFromSketch {
                name: "turned body".into(),
                sketch_id,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 270.0,
                position: [2.0, 3.0, 4.0],
            })
            .unwrap();
        assert_eq!(decode(&encode(&source).unwrap()).unwrap(), source);
    }

    #[test]
    fn round_trip_preserves_imported_step_source() {
        let mut source = CadDocument::default();
        source
            .apply(crate::domain::ModelCommand::ImportStep {
                name: "imported bracket".into(),
                source:
                    "ISO-10303-21;\nDATA;\n#42=CLOSED_SHELL('',());\nENDSEC;\nEND-ISO-10303-21;"
                        .into(),
                shell_id: 42,
                position: [1.0, 2.0, 3.0],
            })
            .unwrap();
        assert_eq!(decode(&encode(&source).unwrap()).unwrap(), source);
    }

    #[test]
    fn round_trip_preserves_datum_face_references() {
        use crate::domain::{ModelCommand, Primitive};
        use crate::topology::{FaceRef, PrimitiveFace};

        let mut source = CadDocument::default();
        let body = source
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [12.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        source
            .apply(ModelCommand::CreateDatumPlane {
                name: "datum".into(),
                face: FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                offset: 2.0,
            })
            .unwrap();
        let decoded = decode(&encode(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            decoded.features.last().map(|feature| &feature.primitive),
            Some(Primitive::DatumPlane { .. })
        ));
    }

    #[test]
    fn round_trip_preserves_datum_vertex_references() {
        use crate::domain::{ModelCommand, Primitive};
        use crate::topology::{EdgeRef, FaceRef, PrimitiveFace, VertexRef};

        let mut source = CadDocument::default();
        let body = source
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [12.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let edge = EdgeRef::new(
            body,
            FaceRef::primitive(body, PrimitiveFace::BoxXMin),
            FaceRef::primitive(body, PrimitiveFace::BoxYMin),
            0,
        );
        source
            .apply(ModelCommand::CreateDatumPoint {
                name: "origin datum".into(),
                vertex: VertexRef::new(body, vec![edge], 0),
                offset: [1.0, 2.0, 3.0],
            })
            .unwrap();
        let decoded = decode(&encode(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            decoded.features.last().map(|feature| &feature.primitive),
            Some(Primitive::DatumPoint { .. })
        ));
    }

    #[test]
    fn round_trip_preserves_chamfer_edge_references() {
        use crate::domain::{ModelCommand, Primitive};
        use crate::topology::{EdgeRef, FaceRef, PrimitiveFace};

        let mut source = CadDocument::default();
        let body = source
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [12.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        source
            .apply(ModelCommand::CreateChamfer {
                name: "edge break".into(),
                edges: vec![EdgeRef::new(
                    body,
                    FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                    FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                    0,
                )],
                distance: 1.5,
            })
            .unwrap();
        let decoded = decode(&encode(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            decoded.features.last().map(|feature| &feature.primitive),
            Some(Primitive::Chamfer { distance, .. }) if (*distance - 1.5).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn reads_v9_single_edge_modifier_fields() {
        use crate::domain::ModelCommand;
        use crate::topology::{EdgeRef, FaceRef, PrimitiveFace};

        let mut source = CadDocument::default();
        let body = source
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [12.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        source
            .apply(ModelCommand::CreateChamfer {
                name: "legacy edge break".into(),
                edges: vec![EdgeRef::new(
                    body,
                    FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                    FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                    0,
                )],
                distance: 1.5,
            })
            .unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        legacy["version"] = 9.into();
        let primitive = legacy["document"]["features"][1]["primitive"]
            .as_object_mut()
            .unwrap();
        let edge = primitive.remove("edges").unwrap()[0].clone();
        primitive.insert("edge".into(), edge);

        assert_eq!(decode(&legacy.to_string()).unwrap(), source);
    }

    #[test]
    fn round_trip_preserves_fillet_edge_references() {
        use crate::domain::{ModelCommand, Primitive};
        use crate::topology::{EdgeRef, FaceRef, PrimitiveFace};

        let mut source = CadDocument::default();
        let body = source
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [12.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        source
            .apply(ModelCommand::CreateFillet {
                name: "round".into(),
                edges: vec![EdgeRef::new(
                    body,
                    FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                    FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                    0,
                )],
                radius: 1.5,
            })
            .unwrap();
        let decoded = decode(&encode(&source).unwrap()).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            decoded.features.last().map(|feature| &feature.primitive),
            Some(Primitive::Fillet { radius, .. }) if (*radius - 1.5).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn round_trip_preserves_v21_rational_and_cubic_sketch_segments() {
        use crate::domain::{
            ModelCommand, SketchLoop2D, SketchPlane, SketchRegion2D, SketchSegment2D,
        };

        let mut source = CadDocument::default();
        source
            .apply(ModelCommand::CreateSketchRegion {
                name: "Bezier profile".into(),
                plane: SketchPlane::WorldXy,
                region: SketchRegion2D {
                    profile: SketchLoop2D {
                        segments: vec![
                            SketchSegment2D::CubicBezier {
                                start: [0.0, 0.0],
                                control1: [3.0, -2.0],
                                control2: [7.0, -2.0],
                                end: [10.0, 0.0],
                            },
                            SketchSegment2D::Line {
                                start: [10.0, 0.0],
                                end: [10.0, 10.0],
                            },
                            SketchSegment2D::RationalQuadratic {
                                start: [10.0, 10.0],
                                control: [5.0, 14.0],
                                end: [0.0, 10.0],
                                weight: 0.8,
                            },
                            SketchSegment2D::Line {
                                start: [0.0, 10.0],
                                end: [0.0, 0.0],
                            },
                        ],
                    },
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap();

        let mut encoded: serde_json::Value =
            serde_json::from_str(&encode(&source).unwrap()).unwrap();
        encoded["version"] = 21.into();
        let encoded = encoded.to_string();
        assert!(encoded.contains("\"type\":\"rational_quadratic\""));
        assert!(encoded.contains("\"type\":\"cubic_bezier\""));
        assert_eq!(decode(&encoded).unwrap(), source);
    }

    #[test]
    fn round_trip_preserves_v22_loft_sections() {
        use crate::domain::{ModelCommand, Primitive, SketchPlane};

        let mut source = CadDocument::default();
        let mut sections = Vec::new();
        for (index, z) in [0.0, 20.0].into_iter().enumerate() {
            sections.push(
                source
                    .apply(ModelCommand::CreateSketch {
                        name: format!("section {index}"),
                        plane: SketchPlane::WorldXy,
                        profile: vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]],
                        holes: Vec::new(),
                        constraints: Vec::new(),
                        position: [0.0, 0.0, z],
                    })
                    .unwrap()
                    .unwrap(),
            );
        }
        source
            .apply(ModelCommand::CreateLoftFromSketches {
                name: "loft".into(),
                sketch_ids: sections.clone(),
                position: [1.0, 2.0, 3.0],
            })
            .unwrap();

        let encoded = encode(&source).unwrap();
        assert!(encoded.contains("\"version\": 22"));
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert!(matches!(
            &decoded.features.last().unwrap().primitive,
            Primitive::LoftFromSketches { sketch_ids, profiles }
                if sketch_ids == &sections && profiles.len() == 2
        ));
    }
}
