use cadx_core::{
    kernel::EvaluatedScene,
    topology::{
        CurveKind, EdgeRef, EvaluatedEdge, EvaluatedFace, EvaluatedVertex, FaceRef,
        TopologyResolution, VertexRef,
    },
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DIRECTION_EPSILON: f64 = 1.0e-12;
const PARALLEL_ANGLE_EPSILON_RADIANS: f64 = 1.0e-10;

/// Persistent topology selected as a measurement operand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reference", rename_all = "snake_case")]
pub enum MeasurementEntity {
    Face(FaceRef),
    Edge(EdgeRef),
    Vertex(VertexRef),
}

impl MeasurementEntity {
    #[must_use]
    pub const fn kind(&self) -> MeasurementEntityKind {
        match self {
            Self::Face(_) => MeasurementEntityKind::Face,
            Self::Edge(_) => MeasurementEntityKind::Edge,
            Self::Vertex(_) => MeasurementEntityKind::Vertex,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementEntityKind {
    Face,
    Edge,
    Vertex,
}

/// Provenance of a reported B-Rep curve length.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LengthPrecision {
    Exact,
    Numerical { estimated_error_mm: f64 },
}

/// A kernel-neutral engineering measurement over persistent topology.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MeasurementResult {
    EdgeLength {
        edge: EdgeRef,
        length_mm: f64,
        precision: LengthPrecision,
    },
    PointDistance {
        first: VertexRef,
        second: VertexRef,
        delta_mm: [f64; 3],
        distance_mm: f64,
    },
    LinearEdgeAngle {
        first: EdgeRef,
        second: EdgeRef,
        angle_degrees: f64,
    },
    PlanarFaceRelationship {
        first: FaceRef,
        second: FaceRef,
        angle_degrees: f64,
        /// Support-plane spacing. Present only for parallel planes.
        parallel_distance_mm: Option<f64>,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MeasurementError {
    #[error("measurement requires one edge or two entities of the same kind")]
    UnsupportedSelection,
    #[error("selected {kind:?} topology was lost")]
    LostTopology { kind: MeasurementEntityKind },
    #[error("selected {kind:?} topology is ambiguous ({candidates} candidates)")]
    AmbiguousTopology {
        kind: MeasurementEntityKind,
        candidates: usize,
    },
    #[error("curve length accuracy is unavailable")]
    LengthAccuracyUnavailable,
    #[error("edge angle requires two linear edges")]
    NonLinearEdge,
    #[error("face relationship requires two analytic planar faces")]
    NonPlanarFace,
    #[error("selected geometry is non-finite or degenerate")]
    DegenerateGeometry,
}

/// Measures one edge or a compatible pair of persistent topology references.
///
/// Resolution is fail-closed: lost and ambiguous references are distinct
/// errors, and unsupported curve/surface combinations are never approximated.
///
/// # Errors
///
/// Returns [`MeasurementError`] when the selection shape is unsupported, a
/// persistent reference does not resolve uniquely, the requested analytic
/// relationship is unavailable, or the evaluated geometry is degenerate.
pub fn measure(
    scene: &EvaluatedScene,
    entities: &[MeasurementEntity],
) -> Result<MeasurementResult, MeasurementError> {
    match entities {
        [MeasurementEntity::Edge(reference)] => edge_length(scene, reference),
        [
            MeasurementEntity::Vertex(first),
            MeasurementEntity::Vertex(second),
        ] => point_distance(scene, first, second),
        [
            MeasurementEntity::Edge(first),
            MeasurementEntity::Edge(second),
        ] => edge_angle(scene, first, second),
        [
            MeasurementEntity::Face(first),
            MeasurementEntity::Face(second),
        ] => face_relationship(scene, first, second),
        _ => Err(MeasurementError::UnsupportedSelection),
    }
}

fn edge_length(
    scene: &EvaluatedScene,
    reference: &EdgeRef,
) -> Result<MeasurementResult, MeasurementError> {
    let edge = resolve_edge(scene, reference)?;
    let precision = match (edge.geometry.curve, edge.geometry.length_error_estimate) {
        (CurveKind::Line, Some(error)) if error <= f64::EPSILON => LengthPrecision::Exact,
        (_, Some(estimated_error_mm))
            if estimated_error_mm.is_finite() && estimated_error_mm >= 0.0 =>
        {
            LengthPrecision::Numerical { estimated_error_mm }
        }
        _ => return Err(MeasurementError::LengthAccuracyUnavailable),
    };
    if !edge.geometry.length.is_finite() || edge.geometry.length <= 0.0 {
        return Err(MeasurementError::DegenerateGeometry);
    }
    Ok(MeasurementResult::EdgeLength {
        edge: reference.clone(),
        length_mm: edge.geometry.length,
        precision,
    })
}

fn point_distance(
    scene: &EvaluatedScene,
    first: &VertexRef,
    second: &VertexRef,
) -> Result<MeasurementResult, MeasurementError> {
    let first_vertex = resolve_vertex(scene, first)?;
    let second_vertex = resolve_vertex(scene, second)?;
    let delta_mm = subtract(
        second_vertex.geometry.position,
        first_vertex.geometry.position,
    );
    let distance_mm = norm(delta_mm);
    if !distance_mm.is_finite() {
        return Err(MeasurementError::DegenerateGeometry);
    }
    Ok(MeasurementResult::PointDistance {
        first: first.clone(),
        second: second.clone(),
        delta_mm,
        distance_mm,
    })
}

fn edge_angle(
    scene: &EvaluatedScene,
    first: &EdgeRef,
    second: &EdgeRef,
) -> Result<MeasurementResult, MeasurementError> {
    let first_edge = resolve_edge(scene, first)?;
    let second_edge = resolve_edge(scene, second)?;
    if first_edge.geometry.curve != CurveKind::Line || second_edge.geometry.curve != CurveKind::Line
    {
        return Err(MeasurementError::NonLinearEdge);
    }
    let first_direction = subtract(
        first_edge.geometry.endpoints[1],
        first_edge.geometry.endpoints[0],
    );
    let second_direction = subtract(
        second_edge.geometry.endpoints[1],
        second_edge.geometry.endpoints[0],
    );
    let angle_degrees = unoriented_angle(first_direction, second_direction)?.to_degrees();
    Ok(MeasurementResult::LinearEdgeAngle {
        first: first.clone(),
        second: second.clone(),
        angle_degrees,
    })
}

fn face_relationship(
    scene: &EvaluatedScene,
    first: &FaceRef,
    second: &FaceRef,
) -> Result<MeasurementResult, MeasurementError> {
    let first_face = resolve_face(scene, first)?;
    let second_face = resolve_face(scene, second)?;
    let (Some(first_plane), Some(second_plane)) =
        (first_face.geometry.plane, second_face.geometry.plane)
    else {
        return Err(MeasurementError::NonPlanarFace);
    };
    let first_normal = normalized(first_plane.normal)?;
    let second_normal = normalized(second_plane.normal)?;
    let angle = unoriented_angle(first_normal, second_normal)?;
    let parallel_distance_mm = (angle.sin().abs() <= PARALLEL_ANGLE_EPSILON_RADIANS).then(|| {
        dot(
            subtract(second_plane.origin, first_plane.origin),
            first_normal,
        )
        .abs()
    });
    if parallel_distance_mm.is_some_and(|distance| !distance.is_finite()) {
        return Err(MeasurementError::DegenerateGeometry);
    }
    Ok(MeasurementResult::PlanarFaceRelationship {
        first: first.clone(),
        second: second.clone(),
        angle_degrees: angle.to_degrees(),
        parallel_distance_mm,
    })
}

fn resolve_edge<'a>(
    scene: &'a EvaluatedScene,
    reference: &EdgeRef,
) -> Result<&'a EvaluatedEdge, MeasurementError> {
    match scene.resolve_edge(reference) {
        TopologyResolution::Resolved(edge) => Ok(edge),
        TopologyResolution::Ambiguous(candidates) => Err(MeasurementError::AmbiguousTopology {
            kind: MeasurementEntityKind::Edge,
            candidates: candidates.len(),
        }),
        TopologyResolution::Lost => Err(MeasurementError::LostTopology {
            kind: MeasurementEntityKind::Edge,
        }),
    }
}

fn resolve_vertex<'a>(
    scene: &'a EvaluatedScene,
    reference: &VertexRef,
) -> Result<&'a EvaluatedVertex, MeasurementError> {
    match scene.resolve_vertex(reference) {
        TopologyResolution::Resolved(vertex) => Ok(vertex),
        TopologyResolution::Ambiguous(candidates) => Err(MeasurementError::AmbiguousTopology {
            kind: MeasurementEntityKind::Vertex,
            candidates: candidates.len(),
        }),
        TopologyResolution::Lost => Err(MeasurementError::LostTopology {
            kind: MeasurementEntityKind::Vertex,
        }),
    }
}

fn resolve_face<'a>(
    scene: &'a EvaluatedScene,
    reference: &FaceRef,
) -> Result<&'a EvaluatedFace, MeasurementError> {
    match scene.resolve_face(reference) {
        TopologyResolution::Resolved(face) => Ok(face),
        TopologyResolution::Ambiguous(candidates) => Err(MeasurementError::AmbiguousTopology {
            kind: MeasurementEntityKind::Face,
            candidates: candidates.len(),
        }),
        TopologyResolution::Lost => Err(MeasurementError::LostTopology {
            kind: MeasurementEntityKind::Face,
        }),
    }
}

fn unoriented_angle(first: [f64; 3], second: [f64; 3]) -> Result<f64, MeasurementError> {
    let first = normalized(first)?;
    let second = normalized(second)?;
    Ok(dot(first, second).abs().clamp(0.0, 1.0).acos())
}

fn normalized(vector: [f64; 3]) -> Result<[f64; 3], MeasurementError> {
    let length = norm(vector);
    if !length.is_finite() || length <= DIRECTION_EPSILON {
        return Err(MeasurementError::DegenerateGeometry);
    }
    Ok(vector.map(|value| value / length))
}

fn subtract(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn norm(vector: [f64; 3]) -> f64 {
    dot(vector, vector).sqrt()
}

#[cfg(test)]
mod tests {
    use cadx_core::{
        kernel::{EvaluatedPart, TriangleMesh},
        topology::{
            EdgeGeometry, EvaluatedFace, EvaluatedVertex, FaceGeometry, PlaneGeometry,
            PrimitiveFace, SurfaceKind, VertexGeometry,
        },
    };

    use super::*;

    fn measurement_scene() -> (EvaluatedScene, Vec<MeasurementEntity>) {
        let bottom = FaceRef::primitive(1, PrimitiveFace::BoxZMin);
        let top = FaceRef::primitive(1, PrimitiveFace::BoxZMax);
        let side = FaceRef::primitive(1, PrimitiveFace::BoxXMax);
        let x_edge = EdgeRef::new(1, bottom.clone(), side.clone(), 0);
        let y_edge = EdgeRef::new(1, top.clone(), side.clone(), 0);
        let first_vertex = VertexRef::new(1, vec![x_edge.clone()], 0);
        let second_vertex = VertexRef::new(1, vec![y_edge.clone()], 0);
        let face = |reference, origin, normal: [f64; 3]| {
            let x_direction = if normal[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            let y_direction = [
                normal[1].mul_add(x_direction[2], -normal[2] * x_direction[1]),
                normal[2].mul_add(x_direction[0], -normal[0] * x_direction[2]),
                normal[0].mul_add(x_direction[1], -normal[1] * x_direction[0]),
            ];
            EvaluatedFace {
                reference,
                geometry: FaceGeometry {
                    surface: SurfaceKind::Plane,
                    plane: Some(PlaneGeometry {
                        origin,
                        x_direction,
                        y_direction,
                        normal,
                    }),
                    area: 1.0,
                    centroid: origin,
                    mean_normal: normal,
                },
                triangles: 0..0,
            }
        };
        let edge = |reference, endpoints: [[f64; 3]; 2]| EvaluatedEdge {
            reference,
            geometry: EdgeGeometry {
                curve: CurveKind::Line,
                endpoints,
                midpoint: endpoints[0],
                length: norm(subtract(endpoints[1], endpoints[0])),
                length_error_estimate: Some(0.0),
                polyline: endpoints.into(),
            },
        };
        let scene = EvaluatedScene {
            parts: vec![EvaluatedPart {
                feature_id: 1,
                name: "measurement fixture".into(),
                color: [1.0; 4],
                material: None,
                mesh: TriangleMesh::default(),
                faces: vec![
                    face(bottom.clone(), [0.0; 3], [0.0, 0.0, 1.0]),
                    face(top.clone(), [0.0, 0.0, 5.0], [0.0, 0.0, -1.0]),
                    face(side, [2.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
                ],
                edges: vec![
                    edge(x_edge.clone(), [[0.0; 3], [3.0, 0.0, 0.0]]),
                    edge(y_edge.clone(), [[0.0; 3], [0.0, 4.0, 0.0]]),
                ],
                vertices: vec![
                    EvaluatedVertex {
                        reference: first_vertex.clone(),
                        geometry: VertexGeometry { position: [0.0; 3] },
                    },
                    EvaluatedVertex {
                        reference: second_vertex.clone(),
                        geometry: VertexGeometry {
                            position: [3.0, 4.0, 12.0],
                        },
                    },
                ],
            }],
            ..EvaluatedScene::default()
        };
        (
            scene,
            vec![
                MeasurementEntity::Face(bottom),
                MeasurementEntity::Face(top),
                MeasurementEntity::Edge(x_edge),
                MeasurementEntity::Edge(y_edge),
                MeasurementEntity::Vertex(first_vertex),
                MeasurementEntity::Vertex(second_vertex),
            ],
        )
    }

    #[test]
    fn measures_edge_length_point_distance_and_linear_angle() {
        let (scene, entities) = measurement_scene();
        assert!(matches!(
            measure(&scene, &entities[2..3]).unwrap(),
            MeasurementResult::EdgeLength {
                length_mm: 3.0,
                precision: LengthPrecision::Exact,
                ..
            }
        ));
        assert!(matches!(
            measure(&scene, &entities[4..6]).unwrap(),
            MeasurementResult::PointDistance {
                delta_mm: [3.0, 4.0, 12.0],
                distance_mm: 13.0,
                ..
            }
        ));
        assert!(matches!(
            measure(&scene, &entities[2..4]).unwrap(),
            MeasurementResult::LinearEdgeAngle { angle_degrees, .. }
                if (angle_degrees - 90.0).abs() < 1.0e-12
        ));
    }

    #[test]
    fn measures_parallel_plane_spacing_and_unoriented_angle() {
        let (scene, entities) = measurement_scene();
        assert!(matches!(
            measure(&scene, &entities[0..2]).unwrap(),
            MeasurementResult::PlanarFaceRelationship {
                angle_degrees,
                parallel_distance_mm: Some(distance),
                ..
            } if angle_degrees.abs() < 1.0e-12 && (distance - 5.0).abs() < 1.0e-12
        ));
        assert!(matches!(
            measure(&scene, &[entities[0].clone(), MeasurementEntity::Face(
                FaceRef::primitive(1, PrimitiveFace::BoxXMax)
            )]).unwrap(),
            MeasurementResult::PlanarFaceRelationship {
                angle_degrees,
                parallel_distance_mm: None,
                ..
            } if (angle_degrees - 90.0).abs() < 1.0e-12
        ));
    }

    #[test]
    fn topology_resolution_fails_closed_for_measurement() {
        let (mut scene, entities) = measurement_scene();
        let missing = EdgeRef::new(
            99,
            FaceRef::primitive(99, PrimitiveFace::BoxXMin),
            FaceRef::primitive(99, PrimitiveFace::BoxXMax),
            0,
        );
        assert_eq!(
            measure(&scene, &[MeasurementEntity::Edge(missing)]),
            Err(MeasurementError::LostTopology {
                kind: MeasurementEntityKind::Edge
            })
        );

        let duplicate = scene.parts[0].edges[0].clone();
        scene.parts[0].edges.push(duplicate);
        assert!(matches!(
            measure(&scene, &entities[2..3]),
            Err(MeasurementError::AmbiguousTopology {
                kind: MeasurementEntityKind::Edge,
                candidates: 2
            })
        ));
    }
}
