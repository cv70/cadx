//! Conservative resolutions for booleans whose boundaries do not cross.

use std::collections::{BTreeMap, BTreeSet};

use cadx_core::domain::BooleanOperation;
use truck_meshalgo::prelude::*;
use truck_modeling::{Curve, Line, Matrix4, Point3, Solid, Surface, Transformed, Vector3};
use truck_topology::compress::{
    CompressedEdge, CompressedEdgeIndex, CompressedFace, CompressedShell, CompressedSolid,
};

use crate::topology::BooleanSourceGeometry;

mod refit;

use refit::{local_surface_refits, replaced_surface_shell};

#[derive(Debug)]
pub(super) enum ContactResolution {
    Solid {
        solid: Solid,
        right_lineage: BooleanSourceGeometry,
    },
    Empty,
}

#[derive(Debug)]
struct FaceLoop {
    vertices: Vec<usize>,
    edges: Vec<usize>,
    normal: Vector3,
}

#[derive(Debug)]
struct ContactPair {
    left_face: usize,
    right_face: usize,
    vertex_pairs: Vec<(usize, usize)>,
    left_edges: Vec<usize>,
    right_edges: Vec<usize>,
    right_lineage: BooleanSourceGeometry,
}

/// Resolves exact identity and one complete planar interface within tolerance.
/// `None` means the proof did not apply and the ordinary shape operation
/// remains authoritative.
pub(super) fn resolve(
    left: &Solid,
    right: &Solid,
    operation: BooleanOperation,
    tolerance: f64,
) -> Option<Result<ContactResolution, String>> {
    let left_compressed = left.compress();
    let right_compressed = right.compress();
    let [left_shell] = left_compressed.boundaries.as_slice() else {
        return None;
    };
    let [right_shell] = right_compressed.boundaries.as_slice() else {
        return None;
    };
    if solids_equivalent(left_shell, right_shell, tolerance) {
        return Some(Ok(match operation {
            BooleanOperation::Union | BooleanOperation::Intersect => ContactResolution::Solid {
                solid: left.clone(),
                right_lineage: BooleanSourceGeometry::Identity,
            },
            BooleanOperation::Subtract => ContactResolution::Empty,
        }));
    }
    let pairs = complete_planar_contacts(left_shell, right_shell, tolerance);
    let [pair] = pairs.as_slice() else {
        return None;
    };

    Some(match operation {
        BooleanOperation::Union => {
            sew_contact(left_shell, right_shell, pair, tolerance).map(|solid| {
                ContactResolution::Solid {
                    solid,
                    right_lineage: pair.right_lineage.clone(),
                }
            })
        }
        BooleanOperation::Subtract => Ok(ContactResolution::Solid {
            solid: left.clone(),
            right_lineage: BooleanSourceGeometry::Identity,
        }),
        BooleanOperation::Intersect => Ok(ContactResolution::Empty),
    })
}

fn solids_equivalent(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    tolerance: f64,
) -> bool {
    if structurally_equivalent_shells(left, right, tolerance) {
        return true;
    }
    planar_solids_equivalent(left, right, tolerance)
}

fn structurally_equivalent_shells(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    tolerance: f64,
) -> bool {
    left.vertices.len() == right.vertices.len()
        && left.edges.len() == right.edges.len()
        && left.faces.len() == right.faces.len()
        && left
            .vertices
            .iter()
            .zip(&right.vertices)
            .all(|(left, right)| points_within(*left, *right, tolerance))
        && left.edges.iter().zip(&right.edges).all(|(left, right)| {
            left.vertices == right.vertices
                && curves_equivalent(&left.curve, &right.curve, tolerance)
        })
        && left.faces.iter().zip(&right.faces).all(|(left, right)| {
            left.boundaries == right.boundaries
                && left.orientation == right.orientation
                && surfaces_equivalent(&left.surface, &right.surface, tolerance)
        })
}

fn curves_equivalent(left: &Curve, right: &Curve, tolerance: f64) -> bool {
    match (left, right) {
        (Curve::Line(left), Curve::Line(right)) => {
            points_within(left.0, right.0, tolerance) && points_within(left.1, right.1, tolerance)
        }
        (Curve::BSplineCurve(left), Curve::BSplineCurve(right)) => left == right,
        (Curve::NurbsCurve(left), Curve::NurbsCurve(right)) => left == right,
        _ => false,
    }
}

fn surfaces_equivalent(left: &Surface, right: &Surface, tolerance: f64) -> bool {
    match (left, right) {
        (Surface::Plane(left), Surface::Plane(right)) => {
            directions_aligned(left.normal(), right.normal())
                && left.normal().dot(right.origin() - left.origin()).abs() <= tolerance
        }
        (Surface::BSplineSurface(left), Surface::BSplineSurface(right)) => left == right,
        (Surface::NurbsSurface(left), Surface::NurbsSurface(right)) => left == right,
        (Surface::RevolutedCurve(left), Surface::RevolutedCurve(right)) => {
            left.orientation() == right.orientation()
                && left.transform() == right.transform()
                && points_within(left.entity().origin(), right.entity().origin(), tolerance)
                && directions_aligned(left.entity().axis(), right.entity().axis())
                && curves_equivalent(
                    left.entity().entity_curve(),
                    right.entity().entity_curve(),
                    tolerance,
                )
        }
        _ => false,
    }
}

fn planar_solids_equivalent(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    tolerance: f64,
) -> bool {
    if left.faces.len() != right.faces.len() || left.faces.is_empty() {
        return false;
    }
    let mut matched_right = vec![false; right.faces.len()];
    for face_a in &left.faces {
        let Some(loop_a) = planar_face_loop(left, face_a) else {
            return false;
        };
        let Some((right_index, _)) = right.faces.iter().enumerate().find(|(index, face_b)| {
            if matched_right[*index] {
                return false;
            }
            let Some(loop_b) = planar_face_loop(right, face_b) else {
                return false;
            };
            loop_a.normal.dot(loop_b.normal) > 1.0 - 1.0e-10
                && matching_loops(
                    left,
                    &loop_a.vertices,
                    &loop_a.edges,
                    right,
                    &loop_b.vertices,
                    &loop_b.edges,
                    tolerance,
                )
                .is_some()
        }) else {
            return false;
        };
        matched_right[right_index] = true;
    }
    matched_right.into_iter().all(|matched| matched)
}

fn complete_planar_contacts(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    tolerance: f64,
) -> Vec<ContactPair> {
    let mut pairs = Vec::new();
    for (left_face, face_a) in left.faces.iter().enumerate() {
        let Some(loop_a) = planar_face_loop(left, face_a) else {
            continue;
        };
        for (right_face, face_b) in right.faces.iter().enumerate() {
            let Some(loop_b) = planar_face_loop(right, face_b) else {
                continue;
            };
            if loop_a.normal.dot(loop_b.normal) > -1.0 + 1.0e-10 {
                continue;
            }
            let Some(vertex_pairs) = matching_loops(
                left,
                &loop_a.vertices,
                &loop_a.edges,
                right,
                &loop_b.vertices,
                &loop_b.edges,
                tolerance,
            ) else {
                continue;
            };
            let Some(offset) = interface_uniform_normal_offset(
                left,
                right,
                &vertex_pairs,
                loop_a.normal,
                tolerance,
            ) else {
                continue;
            };
            let right_lineage = if let Some(replacements) = local_surface_refits(
                left,
                right,
                right_face,
                &loop_b.edges,
                &vertex_pairs,
                offset,
                tolerance,
            ) {
                if replacements.is_empty() {
                    BooleanSourceGeometry::Identity
                } else {
                    BooleanSourceGeometry::SurfaceReplacements(replacements)
                }
            } else {
                let precision = tolerance.mul_add(1.0e-6, 1.0e-12);
                if offset.magnitude() <= precision || !supports_rigid_translation(right) {
                    continue;
                }
                BooleanSourceGeometry::Translation(offset)
            };
            if solids_are_separated_by_face(
                left,
                right,
                left.vertices[loop_a.vertices[0]],
                loop_a.normal,
                tolerance,
            ) {
                pairs.push(ContactPair {
                    left_face,
                    right_face,
                    vertex_pairs,
                    left_edges: loop_a.edges.clone(),
                    right_edges: loop_b.edges.clone(),
                    right_lineage,
                });
            }
        }
    }
    pairs
}

fn interface_uniform_normal_offset(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    vertex_pairs: &[(usize, usize)],
    normal: Vector3,
    tolerance: f64,
) -> Option<Vector3> {
    let (left_vertex, right_vertex) = vertex_pairs.first().copied()?;
    let offset = left.vertices[left_vertex] - right.vertices[right_vertex];
    let precision = tolerance.mul_add(1.0e-6, 1.0e-12);
    let tangent = offset - normal * offset.dot(normal);
    (tangent.magnitude() <= precision
        && vertex_pairs.iter().all(|(left_vertex, right_vertex)| {
            let candidate = left.vertices[*left_vertex] - right.vertices[*right_vertex];
            (candidate - offset).magnitude() <= precision
        }))
    .then_some(offset)
}

fn supports_rigid_translation(shell: &CompressedShell<Point3, Curve, Surface>) -> bool {
    shell
        .edges
        .iter()
        .all(|edge| !matches!(edge.curve, Curve::IntersectionCurve(_)))
}

fn planar_face_loop(
    shell: &CompressedShell<Point3, Curve, Surface>,
    face: &CompressedFace<Surface>,
) -> Option<FaceLoop> {
    let [wire] = face.boundaries.as_slice() else {
        return None;
    };
    if wire.is_empty() {
        return None;
    }
    let Surface::Plane(plane) = &face.surface else {
        return None;
    };
    let mut normal = plane.normal();
    if !face.orientation {
        normal = -normal;
    }
    let magnitude = normal.magnitude();
    if !magnitude.is_finite() || magnitude <= f64::EPSILON {
        return None;
    }
    normal /= magnitude;

    let mut vertices = Vec::with_capacity(wire.len());
    let mut edges = Vec::with_capacity(wire.len());
    for edge_ref in wire {
        let edge = shell.edges.get(edge_ref.index)?;
        vertices.push(if edge_ref.orientation {
            edge.vertices.0
        } else {
            edge.vertices.1
        });
        edges.push(edge_ref.index);
    }
    if vertices
        .iter()
        .any(|vertex| *vertex >= shell.vertices.len())
    {
        return None;
    }
    Some(FaceLoop {
        vertices,
        edges,
        normal,
    })
}

fn matching_loops(
    left: &CompressedShell<Point3, Curve, Surface>,
    left_vertices: &[usize],
    left_edges: &[usize],
    right: &CompressedShell<Point3, Curve, Surface>,
    right_vertices: &[usize],
    right_edges: &[usize],
    tolerance: f64,
) -> Option<Vec<(usize, usize)>> {
    if left_vertices.len() != right_vertices.len() {
        return None;
    }
    let count = left_vertices.len();
    for offset in 0..count {
        for reverse in [false, true] {
            let mut pairs = Vec::with_capacity(count);
            let mut matches = true;
            for (index, left_vertex) in left_vertices.iter().copied().enumerate() {
                let right_index = if reverse {
                    (offset + count - index) % count
                } else {
                    (offset + index) % count
                };
                let right_vertex = right_vertices[right_index];
                if !points_within(
                    left.vertices[left_vertex],
                    right.vertices[right_vertex],
                    tolerance,
                ) {
                    matches = false;
                    break;
                }
                pairs.push((left_vertex, right_vertex));
            }
            if matches && loop_edges_match(left, left_edges, right, right_edges, &pairs, tolerance)
            {
                return Some(pairs);
            }
        }
    }
    None
}

fn loop_edges_match(
    left: &CompressedShell<Point3, Curve, Surface>,
    left_edges: &[usize],
    right: &CompressedShell<Point3, Curve, Surface>,
    right_edges: &[usize],
    vertex_pairs: &[(usize, usize)],
    tolerance: f64,
) -> bool {
    if left_edges.len() != right_edges.len() || left_edges.is_empty() {
        return false;
    }
    let vertex_map = vertex_pairs.iter().copied().collect::<BTreeMap<_, _>>();
    let offset = left.vertices[vertex_pairs[0].0] - right.vertices[vertex_pairs[0].1];
    let mut used_right = BTreeSet::new();
    left_edges.iter().copied().all(|left_edge_index| {
        let left_edge = &left.edges[left_edge_index];
        let Some(mapped_front) = vertex_map.get(&left_edge.vertices.0).copied() else {
            return false;
        };
        let Some(mapped_back) = vertex_map.get(&left_edge.vertices.1).copied() else {
            return false;
        };
        let mapped = (mapped_front, mapped_back);
        let Some(right_edge_index) = right_edges.iter().copied().find(|right_edge_index| {
            if used_right.contains(right_edge_index) {
                return false;
            }
            let right_edge = &right.edges[*right_edge_index];
            let endpoints_match =
                right_edge.vertices == mapped || right_edge.vertices == (mapped.1, mapped.0);
            endpoints_match
                && curves_match_after_translation(
                    &left_edge.curve,
                    &right_edge.curve,
                    offset,
                    tolerance,
                )
        }) else {
            return false;
        };
        used_right.insert(right_edge_index);
        true
    })
}

fn curves_match_after_translation(
    left: &Curve,
    right: &Curve,
    offset: Vector3,
    tolerance: f64,
) -> bool {
    let precision = tolerance.mul_add(1.0e-6, 1.0e-12);
    if offset.magnitude() <= precision {
        let inverse = right.clone().inverse();
        return curves_equivalent(left, right, tolerance)
            || curves_equivalent(left, &inverse, tolerance);
    }
    if matches!(left, Curve::IntersectionCurve(_)) || matches!(right, Curve::IntersectionCurve(_)) {
        return false;
    }
    let translated = right.transformed(Matrix4::from_translation(offset));
    let inverse = translated.clone().inverse();
    curves_equivalent_within(left, &translated, precision)
        || curves_equivalent_within(left, &inverse, precision)
}

fn curves_equivalent_within(left: &Curve, right: &Curve, tolerance: f64) -> bool {
    match (left, right) {
        (Curve::Line(left), Curve::Line(right)) => {
            points_within(left.0, right.0, tolerance) && points_within(left.1, right.1, tolerance)
        }
        (Curve::BSplineCurve(left), Curve::BSplineCurve(right)) => {
            left.knot_vec() == right.knot_vec()
                && left.control_points().len() == right.control_points().len()
                && left
                    .control_points()
                    .iter()
                    .zip(right.control_points())
                    .all(|(left, right)| points_within(*left, *right, tolerance))
        }
        (Curve::NurbsCurve(left), Curve::NurbsCurve(right)) => {
            left.knot_vec() == right.knot_vec()
                && left.control_points().len() == right.control_points().len()
                && left
                    .control_points()
                    .iter()
                    .zip(right.control_points())
                    .all(|(left, right)| homogeneous_points_within(*left, *right, tolerance))
        }
        _ => false,
    }
}

fn homogeneous_points_within(
    left: truck_modeling::Vector4,
    right: truck_modeling::Vector4,
    tolerance: f64,
) -> bool {
    let weight_scale = left.w.abs().max(right.w.abs()).max(1.0);
    if (left.w - right.w).abs() > f64::EPSILON * weight_scale * 8.0
        || left.w.abs() <= f64::EPSILON
        || right.w.abs() <= f64::EPSILON
    {
        return false;
    }
    points_within(
        Point3::new(left.x / left.w, left.y / left.w, left.z / left.w),
        Point3::new(right.x / right.w, right.y / right.w, right.z / right.w),
        tolerance,
    )
}

fn solids_are_separated_by_face(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    origin: Point3,
    outward: Vector3,
    tolerance: f64,
) -> bool {
    let signed_distance = |point: &Point3| outward.dot(*point - origin);
    left.vertices
        .iter()
        .all(|point| signed_distance(point) <= tolerance)
        && right
            .vertices
            .iter()
            .all(|point| signed_distance(point) >= -tolerance)
}

fn sew_contact(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    contact: &ContactPair,
    tolerance: f64,
) -> Result<Solid, String> {
    let healed_right = match &contact.right_lineage {
        BooleanSourceGeometry::Identity => None,
        BooleanSourceGeometry::Translation(offset) => Some(translated_shell(right, *offset)),
        BooleanSourceGeometry::SurfaceReplacements(replacements) => {
            Some(replaced_surface_shell(right, replacements)?)
        }
    };
    let right = healed_right.as_ref().unwrap_or(right);
    let mut vertices = left.vertices.clone();
    let mut right_vertex_map = vec![None; right.vertices.len()];
    for (left_vertex, right_vertex) in &contact.vertex_pairs {
        right_vertex_map[*right_vertex] = Some(*left_vertex);
    }
    for (index, point) in right.vertices.iter().copied().enumerate() {
        if right_vertex_map[index].is_none() {
            right_vertex_map[index] = Some(vertices.len());
            vertices.push(point);
        }
    }
    let right_vertex_map = right_vertex_map
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "contact sewing did not map every right-hand vertex".to_owned())?;

    let mut edges = left.edges.clone();
    let mut contact_edge_map = BTreeMap::new();
    for right_edge in &contact.right_edges {
        let source = &right.edges[*right_edge];
        let mapped = (
            right_vertex_map[source.vertices.0],
            right_vertex_map[source.vertices.1],
        );
        let Some(left_edge) = contact.left_edges.iter().copied().find(|candidate| {
            let vertices = left.edges[*candidate].vertices;
            vertices == mapped || vertices == (mapped.1, mapped.0)
        }) else {
            return Err("contact boundary edges do not have a one-to-one match".into());
        };
        contact_edge_map.insert(*right_edge, left_edge);
    }
    if contact_edge_map.len() != contact.right_edges.len() {
        return Err("contact boundary contains duplicate edge identities".into());
    }

    let mut right_edge_map = Vec::with_capacity(right.edges.len());
    for (index, edge) in right.edges.iter().enumerate() {
        let mapped_vertices = (
            right_vertex_map[edge.vertices.0],
            right_vertex_map[edge.vertices.1],
        );
        if let Some(left_edge) = contact_edge_map.get(&index).copied() {
            let target_vertices = edges[left_edge].vertices;
            let same_orientation = target_vertices == mapped_vertices;
            if !same_orientation && target_vertices != (mapped_vertices.1, mapped_vertices.0) {
                return Err("contact edge endpoints changed during sewing".into());
            }
            right_edge_map.push((left_edge, same_orientation));
        } else {
            let target = edges.len();
            let precision = tolerance.mul_add(1.0e-6, 1.0e-12);
            let moved = !points_within(
                vertices[mapped_vertices.0],
                right.vertices[edge.vertices.0],
                precision,
            ) || !points_within(
                vertices[mapped_vertices.1],
                right.vertices[edge.vertices.1],
                precision,
            );
            let curve = if moved {
                let Curve::Line(_) = edge.curve else {
                    return Err("contact sewing cannot move a non-linear retained edge".into());
                };
                Curve::Line(Line(
                    vertices[mapped_vertices.0],
                    vertices[mapped_vertices.1],
                ))
            } else {
                edge.curve.clone()
            };
            edges.push(CompressedEdge {
                vertices: mapped_vertices,
                curve,
            });
            right_edge_map.push((target, true));
        }
    }

    let mut faces = left
        .faces
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != contact.left_face)
        .map(|(_, face)| face.clone())
        .collect::<Vec<_>>();
    for (index, face) in right.faces.iter().enumerate() {
        if index == contact.right_face {
            continue;
        }
        faces.push(CompressedFace {
            boundaries: face
                .boundaries
                .iter()
                .map(|wire| {
                    wire.iter()
                        .map(|edge_ref| {
                            let (target, same_orientation) = right_edge_map[edge_ref.index];
                            CompressedEdgeIndex {
                                index: target,
                                orientation: if same_orientation {
                                    edge_ref.orientation
                                } else {
                                    !edge_ref.orientation
                                },
                            }
                        })
                        .collect()
                })
                .collect(),
            orientation: face.orientation,
            surface: face.surface.clone(),
        });
    }

    Solid::extract(CompressedSolid {
        boundaries: vec![CompressedShell {
            vertices,
            edges,
            faces,
        }],
    })
    .map_err(|error| format!("contact topology sewing produced an invalid solid: {error}"))
}

fn translated_shell(
    shell: &CompressedShell<Point3, Curve, Surface>,
    offset: Vector3,
) -> CompressedShell<Point3, Curve, Surface> {
    let transform = Matrix4::from_translation(offset);
    CompressedShell {
        vertices: shell.vertices.iter().map(|point| *point + offset).collect(),
        edges: shell
            .edges
            .iter()
            .map(|edge| CompressedEdge {
                vertices: edge.vertices,
                curve: edge.curve.transformed(transform),
            })
            .collect(),
        faces: shell
            .faces
            .iter()
            .map(|face| CompressedFace {
                boundaries: face.boundaries.clone(),
                orientation: face.orientation,
                surface: face.surface.transformed(transform),
            })
            .collect(),
    }
}

fn points_within(left: Point3, right: Point3, tolerance: f64) -> bool {
    let delta = left - right;
    delta.dot(delta) <= tolerance * tolerance
}

fn directions_aligned(left: Vector3, right: Vector3) -> bool {
    left.dot(right) > 1.0 - 1.0e-10
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use truck_modeling::{
        BSplineCurve, Edge, Face, KnotVec, NurbsCurve, Processor, Rad, RevolutedCurve, Vector4,
        Wire, builder,
    };

    use super::*;

    const TOLERANCE: f64 = 0.05;

    #[test]
    fn partial_planar_overlap_is_not_a_complete_interface() {
        let left = cuboid([0.0; 3], [10.0; 3]);
        let right = cuboid([10.0, 2.0, 0.0], [10.0; 3]);
        assert!(resolve(&left, &right, BooleanOperation::Union, TOLERANCE).is_none());
    }

    #[test]
    fn edge_and_vertex_contacts_are_not_planar_interfaces() {
        let left = cuboid([0.0; 3], [10.0; 3]);
        for origin in [[10.0, 10.0, 0.0], [10.0; 3]] {
            let right = cuboid(origin, [10.0; 3]);
            assert!(resolve(&left, &right, BooleanOperation::Union, TOLERANCE).is_none());
        }
    }

    #[test]
    fn multiple_complete_contact_faces_are_ambiguous() {
        let left = prism(&[
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 5.0],
            [10.0, 10.0],
            [0.0, 10.0],
        ]);
        let right = prism(&[
            [10.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [10.0, 10.0],
            [10.0, 5.0],
        ]);
        let left_compressed = left.compress();
        let right_compressed = right.compress();
        let contacts = complete_planar_contacts(
            &left_compressed.boundaries[0],
            &right_compressed.boundaries[0],
            TOLERANCE,
        );
        assert_eq!(contacts.len(), 2);
        assert!(resolve(&left, &right, BooleanOperation::Union, TOLERANCE).is_none());
    }

    #[test]
    fn exact_circular_planar_contact_sews_curved_side_faces() {
        let left = cylinder(0.0, 10.0);
        let right = cylinder(10.0, 10.0);
        let Some(Ok(ContactResolution::Solid { solid: result, .. })) =
            resolve(&left, &right, BooleanOperation::Union, TOLERANCE)
        else {
            panic!("stacked cylinders should have one complete circular interface");
        };
        assert!(result.is_geometric_consistent());
        assert_eq!(result.boundaries().len(), 1);
        assert_eq!(result.boundaries()[0].len(), 8);
    }

    #[test]
    fn curved_gaps_align_rigidly_and_freeform_gaps_refit_locally() {
        let left = analytic_cylinder(0.0, 10.0);
        let gap = analytic_cylinder(10.01, 10.0);
        let resolution = resolve(&left, &gap, BooleanOperation::Union, TOLERANCE);
        let Some(Ok(ContactResolution::Solid {
            solid,
            right_lineage: BooleanSourceGeometry::Translation(translation),
        })) = &resolution
        else {
            panic!("a uniformly translated curved B-Rep should align rigidly: {resolution:?}");
        };
        assert!((translation.z + 0.01).abs() <= 1.0e-12);
        assert_eq!(solid.boundaries()[0].len(), 8);

        let left = freeform_prism(0.0, 0.0);
        let gap = freeform_prism(10.01, 0.0);
        let Some(Ok(ContactResolution::Solid {
            solid,
            right_lineage: BooleanSourceGeometry::SurfaceReplacements(replacements),
        })) = resolve(&left, &gap, BooleanOperation::Union, TOLERANCE)
        else {
            panic!("B-spline and NURBS sidewalls should refit their contact boundary");
        };
        assert_eq!(replacements.len(), 2);
        assert_eq!(solid.boundaries()[0].len(), 10);
        assert!(solid.is_geometric_consistent());
        let max_z = solid
            .vertex_iter()
            .map(|vertex| vertex.point().z)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((max_z - 20.01).abs() <= 1.0e-12);
    }

    #[test]
    fn degree_elevated_freeform_gaps_preserve_surface_lineage() {
        let left = degree_elevated_freeform_prism(0.0);
        let gap = degree_elevated_freeform_prism(10.01);
        let Some(Ok(ContactResolution::Solid {
            solid,
            right_lineage: BooleanSourceGeometry::SurfaceReplacements(replacements),
        })) = resolve(&left, &gap, BooleanOperation::Union, TOLERANCE)
        else {
            panic!("multi-row ruled sidewalls should refit with explicit lineage");
        };
        assert_eq!(replacements.len(), 2);
        assert!(replacements.iter().all(|(source, replacement)| {
            [source, replacement]
                .into_iter()
                .all(|surface| match surface {
                    Surface::BSplineSurface(surface) => {
                        surface.udegree() > 1 && surface.vdegree() > 1
                    }
                    Surface::NurbsSurface(surface) => {
                        surface.udegree() > 1 && surface.vdegree() > 1
                    }
                    Surface::Plane(_) | Surface::RevolutedCurve(_) => false,
                })
        }));
        assert_eq!(solid.boundaries()[0].len(), 10);
        assert!(solid.is_geometric_consistent());
        let max_z = solid
            .vertex_iter()
            .map(|vertex| vertex.point().z)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((max_z - 20.01).abs() <= 1.0e-12);
    }

    #[test]
    fn tangent_and_mismatched_freeform_contacts_stay_with_the_shape_operation() {
        let left = cylinder(0.0, 10.0);
        let tangent = builder::translated(&left, Vector3::new(10.0, 0.0, 0.0));
        assert!(resolve(&left, &tangent, BooleanOperation::Union, TOLERANCE).is_none());

        let left = freeform_prism(0.0, 0.0);
        let mismatched = freeform_prism(10.01, 1.0e-4);
        assert!(resolve(&left, &mismatched, BooleanOperation::Union, TOLERANCE).is_none());
    }

    #[test]
    fn local_refit_accepts_degree_elevated_ruled_surfaces() {
        let control_points = vec![
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 0.0, 1.0)],
            vec![Point3::new(5.0, -2.0, 0.0), Point3::new(5.0, -2.0, 1.0)],
            vec![Point3::new(10.0, 0.0, 0.0), Point3::new(10.0, 0.0, 1.0)],
        ];
        let mut surface = truck_modeling::BSplineSurface::new(
            (KnotVec::bezier_knot(2), KnotVec::bezier_knot(1)),
            control_points.clone(),
        );
        surface.elevate_vdegree().elevate_vdegree();
        let boundary = Curve::BSplineCurve(surface.splitted_boundary()[0].clone());
        let Some(Surface::BSplineSurface(refitted)) = refit::refit_surface_boundary(
            &Surface::BSplineSurface(surface),
            &boundary,
            Vector3::new(0.0, 0.0, -0.01),
            1.0e-8,
        ) else {
            panic!("a degree-elevated ruled B-spline should refit locally");
        };
        assert_eq!(refitted.vdegree(), 3);
        for row in refitted.control_points() {
            assert!((row[0].z + 0.01).abs() <= 1.0e-12);
            assert!((row[row.len() - 1].z - 1.0).abs() <= 1.0e-12);
        }

        let mut surface = truck_modeling::NurbsSurface::try_from_bspline_and_weights(
            truck_modeling::BSplineSurface::new(
                (KnotVec::bezier_knot(2), KnotVec::bezier_knot(1)),
                control_points,
            ),
            vec![vec![1.0; 2], vec![0.8; 2], vec![1.0; 2]],
        )
        .unwrap();
        surface.elevate_vdegree().elevate_vdegree();
        let boundary = Curve::NurbsCurve(surface.splitted_boundary()[0].clone());
        let Some(Surface::NurbsSurface(refitted)) = refit::refit_surface_boundary(
            &Surface::NurbsSurface(surface),
            &boundary,
            Vector3::new(0.0, 0.0, -0.01),
            1.0e-8,
        ) else {
            panic!("a degree-elevated ruled NURBS should refit locally");
        };
        assert_eq!(refitted.vdegree(), 3);
        for row in refitted.control_points() {
            assert!((row[0].z / row[0].w + 0.01).abs() <= 1.0e-12);
            let remote = row[row.len() - 1];
            assert!((remote.z / remote.w - 1.0).abs() <= 1.0e-12);
        }
    }

    #[test]
    fn local_refit_rejects_unproven_control_nets_and_boundaries() {
        let control_points = vec![
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.0, 0.0, 0.5),
                Point3::new(0.0, 0.0, 1.0),
            ],
            vec![
                Point3::new(5.0, -2.0, 0.0),
                Point3::new(5.0, -2.0, 0.6),
                Point3::new(5.0, -2.0, 1.0),
            ],
            vec![
                Point3::new(10.0, 0.0, 0.0),
                Point3::new(10.0, 0.0, 0.5),
                Point3::new(10.0, 0.0, 1.0),
            ],
        ];
        let surface = truck_modeling::BSplineSurface::new(
            (KnotVec::bezier_knot(2), KnotVec::bezier_knot(2)),
            control_points.clone(),
        );
        let boundary = Curve::BSplineCurve(surface.splitted_boundary()[0].clone());
        assert!(
            refit::refit_surface_boundary(
                &Surface::BSplineSurface(surface),
                &boundary,
                Vector3::new(0.0, 0.0, -0.01),
                1.0e-8,
            )
            .is_none()
        );

        let surface = truck_modeling::BSplineSurface::new(
            (
                KnotVec::bezier_knot(2),
                KnotVec::from(vec![0.0, 0.0, 0.5, 1.0, 1.0, 1.0]),
            ),
            control_points
                .iter()
                .map(|row| {
                    let mut row = row.clone();
                    row[1].z = 0.5;
                    row
                })
                .collect(),
        );
        let boundary = Curve::BSplineCurve(surface.splitted_boundary()[0].clone());
        assert!(
            refit::refit_surface_boundary(
                &Surface::BSplineSurface(surface),
                &boundary,
                Vector3::new(0.0, 0.0, -0.01),
                1.0e-8,
            )
            .is_none()
        );

        let base = truck_modeling::BSplineSurface::new(
            (KnotVec::bezier_knot(2), KnotVec::bezier_knot(2)),
            control_points
                .iter()
                .map(|row| {
                    let mut row = row.clone();
                    row[1].z = 0.5;
                    row
                })
                .collect(),
        );
        let surface = truck_modeling::NurbsSurface::try_from_bspline_and_weights(
            base,
            vec![vec![1.0, 0.8, 1.0]; 3],
        )
        .unwrap();
        let boundary = Curve::NurbsCurve(surface.splitted_boundary()[0].clone());
        assert!(
            refit::refit_surface_boundary(
                &Surface::NurbsSurface(surface),
                &boundary,
                Vector3::new(0.0, 0.0, -0.01),
                1.0e-8,
            )
            .is_none()
        );

        let surface = truck_modeling::BSplineSurface::new(
            (KnotVec::bezier_knot(2), KnotVec::bezier_knot(1)),
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 0.0, 1.0)],
                vec![Point3::new(5.0, -2.0, 0.0), Point3::new(5.0, -2.0, 1.0)],
                vec![Point3::new(10.0, 0.0, 0.0), Point3::new(10.0, 0.0, 1.0)],
            ],
        );
        let mismatched = Curve::BSplineCurve(surface.splitted_boundary()[0].clone())
            .transformed(Matrix4::from_translation(Vector3::unit_x()));
        assert!(
            refit::refit_surface_boundary(
                &Surface::BSplineSurface(surface),
                &mismatched,
                Vector3::new(0.0, 0.0, -0.01),
                1.0e-8,
            )
            .is_none()
        );
    }

    #[test]
    fn coincident_curved_solids_resolve_by_structural_geometry() {
        for (left, right) in [
            (cylinder(0.0, 10.0), cylinder(0.0, 10.0)),
            (sphere(), sphere()),
            (cone(), cone()),
            (torus(), torus()),
        ] {
            assert!(matches!(
                resolve(&left, &right, BooleanOperation::Union, TOLERANCE),
                Some(Ok(ContactResolution::Solid { .. }))
            ));
            assert!(matches!(
                resolve(&left, &right, BooleanOperation::Subtract, TOLERANCE),
                Some(Ok(ContactResolution::Empty))
            ));
        }
    }

    #[test]
    fn multi_shell_operands_are_not_sewn() {
        let first = cuboid([0.0; 3], [10.0; 3]);
        let second = cuboid([0.0, 20.0, 0.0], [10.0; 3]);
        let mut boundaries = first.boundaries().clone();
        boundaries.extend(second.boundaries().iter().cloned());
        let left = Solid::try_new(boundaries).unwrap();
        let right = cuboid([10.0, 0.0, 0.0], [10.0; 3]);
        assert!(resolve(&left, &right, BooleanOperation::Union, TOLERANCE).is_none());
    }

    fn cuboid(origin: [f64; 3], size: [f64; 3]) -> Solid {
        let vertex = builder::vertex(Point3::new(origin[0], origin[1], origin[2]));
        let edge = builder::tsweep(&vertex, Vector3::new(size[0], 0.0, 0.0));
        let face = builder::tsweep(&edge, Vector3::new(0.0, size[1], 0.0));
        builder::tsweep(&face, Vector3::new(0.0, 0.0, size[2]))
    }

    fn prism(profile: &[[f64; 2]]) -> Solid {
        let vertices = profile
            .iter()
            .map(|point| builder::vertex(Point3::new(point[0], point[1], 0.0)))
            .collect::<Vec<_>>();
        let edges = (0..vertices.len())
            .map(|index| builder::line(&vertices[index], &vertices[(index + 1) % vertices.len()]))
            .collect::<Vec<_>>();
        let face: Face = builder::try_attach_plane(&[Wire::from(edges)]).unwrap();
        builder::tsweep(&face, Vector3::new(0.0, 0.0, 10.0))
    }

    fn cylinder(z: f64, height: f64) -> Solid {
        let center = Point3::new(0.0, 0.0, z);
        let vertex = builder::vertex(Point3::new(5.0, 0.0, z));
        let circle: Wire = builder::rsweep(&vertex, center, Vector3::unit_z(), Rad(TAU));
        let disk = builder::try_attach_plane(&[circle]).unwrap();
        builder::tsweep(&disk, Vector3::new(0.0, 0.0, height))
    }

    fn analytic_cylinder(z: f64, height: f64) -> Solid {
        let mut compressed = cylinder(z, height).compress();
        let shell = &mut compressed.boundaries[0];
        let surface = Surface::RevolutedCurve(Processor::new(RevolutedCurve::by_revolution(
            Curve::Line(Line(
                Point3::new(5.0, 0.0, z),
                Point3::new(5.0, 0.0, z + height),
            )),
            Point3::new(0.0, 0.0, z),
            Vector3::unit_z(),
        )));
        for face in &mut shell.faces {
            if !matches!(face.surface, Surface::Plane(_)) {
                face.surface = surface.clone();
            }
        }
        Solid::extract(compressed).unwrap()
    }

    fn freeform_prism(z: f64, control_offset: f64) -> Solid {
        let points = [
            Point3::new(0.0, 0.0, z),
            Point3::new(10.0, 0.0, z),
            Point3::new(10.0, 10.0, z),
            Point3::new(0.0, 10.0, z),
        ];
        let vertices = points.map(builder::vertex);
        let cubic = builder::bezier(
            &vertices[0],
            &vertices[1],
            vec![
                Point3::new(3.0, -2.0 - control_offset, z),
                Point3::new(7.0, -2.0, z),
            ],
        );
        let rational = NurbsCurve::new(BSplineCurve::new(
            KnotVec::bezier_knot(2),
            vec![
                Vector4::new(10.0, 10.0, z, 1.0),
                Vector4::new(4.0, 11.2, z * 0.8, 0.8),
                Vector4::new(0.0, 10.0, z, 1.0),
            ],
        ));
        let wire: Wire = vec![
            cubic,
            builder::line(&vertices[1], &vertices[2]),
            Edge::new(&vertices[2], &vertices[3], Curve::NurbsCurve(rational)),
            builder::line(&vertices[3], &vertices[0]),
        ]
        .into();
        let face = builder::try_attach_plane(&[wire]).unwrap();
        builder::tsweep(&face, Vector3::new(0.0, 0.0, 10.0))
    }

    fn degree_elevated_freeform_prism(z: f64) -> Solid {
        let mut compressed = freeform_prism(z, 0.0).compress();
        let mut elevated = 0;
        for face in &mut compressed.boundaries[0].faces {
            match &mut face.surface {
                Surface::BSplineSurface(surface) => {
                    assert_eq!(surface.control_points()[0].len(), 2);
                    surface.elevate_vdegree().elevate_vdegree();
                    elevated += 1;
                }
                Surface::NurbsSurface(surface) => {
                    assert_eq!(surface.control_points()[0].len(), 2);
                    surface.elevate_vdegree().elevate_vdegree();
                    elevated += 1;
                }
                Surface::Plane(_) | Surface::RevolutedCurve(_) => {}
            }
        }
        assert_eq!(elevated, 2);
        Solid::extract(compressed).unwrap()
    }

    fn sphere() -> Solid {
        let center = Point3::origin();
        let vertex = builder::vertex(Point3::new(0.0, 5.0, 0.0));
        let meridian: Wire = builder::rsweep(&vertex, center, Vector3::unit_x(), Rad(TAU / 2.0));
        Solid::new(vec![builder::cone(&meridian, Vector3::unit_y(), Rad(TAU))])
    }

    fn cone() -> Solid {
        let center = builder::vertex(Point3::origin());
        let outer = builder::vertex(Point3::new(5.0, 0.0, 0.0));
        let top = builder::vertex(Point3::new(0.0, 0.0, 10.0));
        let top_outer = builder::vertex(Point3::new(2.0, 0.0, 10.0));
        let profile: Wire = vec![
            builder::line(&center, &outer),
            builder::line(&outer, &top_outer),
            builder::line(&top_outer, &top),
        ]
        .into();
        Solid::new(vec![builder::cone(&profile, Vector3::unit_z(), Rad(TAU))])
    }

    fn torus() -> Solid {
        let section_start = builder::vertex(Point3::new(8.0, 0.0, 2.0));
        let section = builder::rsweep(
            &section_start,
            Point3::new(8.0, 0.0, 0.0),
            Vector3::unit_y(),
            Rad(TAU),
        );
        Solid::new(vec![builder::rsweep(
            &section,
            Point3::origin(),
            Vector3::unit_z(),
            Rad(TAU),
        )])
    }
}
