//! Bounded control-net repairs for planar contact sewing.

use std::collections::{BTreeMap, BTreeSet};

use truck_modeling::{
    BSplineCurve, BSplineSurface, Curve, InnerSpace, Invertible, KnotVec, Matrix4, NurbsCurve,
    NurbsSurface, Point3, Surface, Transformed, Vector3, Vector4,
};
use truck_topology::compress::{CompressedFace, CompressedShell};

use super::{curves_equivalent_within, points_within};

pub(super) fn local_surface_refits(
    left: &CompressedShell<Point3, Curve, Surface>,
    right: &CompressedShell<Point3, Curve, Surface>,
    contact_face: usize,
    contact_edges: &[usize],
    vertex_pairs: &[(usize, usize)],
    offset: Vector3,
    tolerance: f64,
) -> Option<Vec<(Surface, Surface)>> {
    let mapped = vertex_pairs
        .iter()
        .map(|(left_vertex, right_vertex)| (*right_vertex, left.vertices[*left_vertex]))
        .collect::<BTreeMap<_, _>>();
    let precision = tolerance.mul_add(1.0e-6, 1.0e-12);
    if mapped.iter().all(|(right_vertex, point)| {
        points_within(*point, right.vertices[*right_vertex], precision)
    }) {
        return Some(Vec::new());
    }
    let contact_edges = contact_edges.iter().copied().collect::<BTreeSet<_>>();
    let mut replacements = Vec::new();
    let mut replaced_source_keys = BTreeSet::new();
    for (face_index, face) in right.faces.iter().enumerate() {
        if face_index == contact_face {
            continue;
        }
        let face_edges = face.boundaries.iter().flatten().collect::<Vec<_>>();
        let touches_mapped_vertex = face_edges.iter().any(|edge_ref| {
            right.edges.get(edge_ref.index).is_some_and(|edge| {
                [edge.vertices.0, edge.vertices.1]
                    .into_iter()
                    .any(|vertex| mapped.contains_key(&vertex))
            })
        });
        if !touches_mapped_vertex {
            continue;
        }
        if face_edges.iter().any(|edge_ref| {
            !contact_edges.contains(&edge_ref.index)
                && right.edges.get(edge_ref.index).is_none_or(|edge| {
                    !matches!(edge.curve, Curve::Line(_))
                        && [edge.vertices.0, edge.vertices.1]
                            .into_iter()
                            .any(|vertex| mapped.contains_key(&vertex))
                })
        }) {
            return None;
        }
        match &face.surface {
            Surface::Plane(plane) => {
                if !face_edges.iter().all(|edge_ref| {
                    right.edges.get(edge_ref.index).is_some_and(|edge| {
                        [edge.vertices.0, edge.vertices.1]
                            .into_iter()
                            .filter_map(|vertex| mapped.get(&vertex))
                            .all(|point| {
                                plane.normal().dot(*point - plane.origin()).abs() <= precision
                            })
                    })
                }) {
                    return None;
                }
            }
            Surface::BSplineSurface(_) | Surface::NurbsSurface(_) => {
                let incident = face_edges
                    .iter()
                    .filter(|edge_ref| contact_edges.contains(&edge_ref.index))
                    .collect::<Vec<_>>();
                let [contact_edge] = incident.as_slice() else {
                    return None;
                };
                let edge = right.edges.get(contact_edge.index)?;
                let replacement =
                    refit_surface_boundary(&face.surface, &edge.curve, offset, precision)?;
                let source_key = format!("{:?}", face.surface);
                if !replaced_source_keys.insert(source_key) {
                    return None;
                }
                replacements.push((face.surface.clone(), replacement));
            }
            Surface::RevolutedCurve(_) => return None,
        }
    }
    Some(replacements)
}

pub(super) fn refit_surface_boundary(
    surface: &Surface,
    boundary: &Curve,
    offset: Vector3,
    precision: f64,
) -> Option<Surface> {
    match surface {
        Surface::BSplineSurface(surface) => {
            let boundary_index =
                unique_bspline_boundary(surface.splitted_boundary(), boundary, precision)?;
            let mut refitted = surface.clone();
            move_bspline_boundary(&mut refitted, boundary_index, offset, precision)?;
            let moved = boundary.transformed(Matrix4::from_translation(offset));
            let candidate =
                Curve::BSplineCurve(refitted.splitted_boundary()[boundary_index].clone());
            curves_match_either_orientation(&moved, &candidate, precision)
                .then_some(Surface::BSplineSurface(refitted))
        }
        Surface::NurbsSurface(surface) => {
            let boundaries = surface.splitted_boundary();
            let boundary_index = unique_nurbs_boundary(boundaries, boundary, precision)?;
            let mut refitted = surface.clone();
            move_nurbs_boundary(&mut refitted, boundary_index, offset, precision)?;
            let moved = boundary.transformed(Matrix4::from_translation(offset));
            let candidate = Curve::NurbsCurve(refitted.splitted_boundary()[boundary_index].clone());
            curves_match_either_orientation(&moved, &candidate, precision)
                .then_some(Surface::NurbsSurface(refitted))
        }
        Surface::Plane(_) | Surface::RevolutedCurve(_) => None,
    }
}

pub(super) fn replaced_surface_shell(
    shell: &CompressedShell<Point3, Curve, Surface>,
    replacements: &[(Surface, Surface)],
) -> Result<CompressedShell<Point3, Curve, Surface>, String> {
    let mut replacements_by_key = BTreeMap::new();
    for (source, replacement) in replacements {
        if replacements_by_key
            .insert(format!("{source:?}"), replacement)
            .is_some()
        {
            return Err("contact surface refit has ambiguous source geometry".into());
        }
    }
    let mut used = BTreeSet::new();
    let faces = shell
        .faces
        .iter()
        .map(|face| {
            let key = format!("{:?}", face.surface);
            let surface = replacements_by_key.get(&key).map_or_else(
                || face.surface.clone(),
                |replacement| {
                    used.insert(key);
                    (*replacement).clone()
                },
            );
            CompressedFace {
                boundaries: face.boundaries.clone(),
                orientation: face.orientation,
                surface,
            }
        })
        .collect();
    if used.len() != replacements.len() {
        return Err("contact surface refit did not resolve every source surface".into());
    }
    Ok(CompressedShell {
        vertices: shell.vertices.clone(),
        edges: shell.edges.clone(),
        faces,
    })
}

fn unique_bspline_boundary(
    boundaries: [BSplineCurve<Point3>; 4],
    target: &Curve,
    precision: f64,
) -> Option<usize> {
    let boundaries = boundaries.map(Curve::BSplineCurve);
    unique_boundary_index(&boundaries, target, precision)
}

fn unique_nurbs_boundary(
    boundaries: [NurbsCurve<Vector4>; 4],
    target: &Curve,
    precision: f64,
) -> Option<usize> {
    let boundaries = boundaries.map(Curve::NurbsCurve);
    unique_boundary_index(&boundaries, target, precision)
}

fn unique_boundary_index(boundaries: &[Curve; 4], target: &Curve, precision: f64) -> Option<usize> {
    let matches = boundaries
        .iter()
        .enumerate()
        .filter_map(|(index, boundary)| {
            curves_match_either_orientation(target, boundary, precision).then_some(index)
        })
        .collect::<Vec<_>>();
    let [index] = matches.as_slice() else {
        return None;
    };
    Some(*index)
}

fn curves_match_either_orientation(left: &Curve, right: &Curve, tolerance: f64) -> bool {
    let inverse = right.clone().inverse();
    curves_equivalent_within(left, right, tolerance)
        || curves_equivalent_within(left, &inverse, tolerance)
}

fn move_bspline_boundary(
    surface: &mut BSplineSurface<Point3>,
    boundary: usize,
    offset: Vector3,
    precision: f64,
) -> Option<()> {
    if !surface.is_clamped() {
        return None;
    }
    let rows = surface.control_points().len();
    let columns = surface.control_points().first()?.len();
    match boundary {
        0 | 2 => {
            let factors = boundary_blend_factors(
                surface.vknot_vec(),
                surface.vdegree(),
                columns,
                boundary == 2,
            )?;
            for row in 0..rows {
                let start = *surface.control_point(row, 0);
                let end = *surface.control_point(row, columns - 1);
                if !(0..columns).all(|column| {
                    let parameter = if boundary == 2 {
                        factors[column]
                    } else {
                        1.0 - factors[column]
                    };
                    points_within(
                        *surface.control_point(row, column),
                        start + (end - start) * parameter,
                        precision,
                    )
                }) {
                    return None;
                }
                for (column, factor) in factors.iter().copied().enumerate() {
                    *surface.control_point_mut(row, column) += offset * factor;
                }
            }
        }
        1 | 3 => {
            let factors = boundary_blend_factors(
                surface.uknot_vec(),
                surface.udegree(),
                rows,
                boundary == 1,
            )?;
            for column in 0..columns {
                let start = *surface.control_point(0, column);
                let end = *surface.control_point(rows - 1, column);
                if !(0..rows).all(|row| {
                    let parameter = if boundary == 1 {
                        factors[row]
                    } else {
                        1.0 - factors[row]
                    };
                    points_within(
                        *surface.control_point(row, column),
                        start + (end - start) * parameter,
                        precision,
                    )
                }) {
                    return None;
                }
                for (row, factor) in factors.iter().copied().enumerate() {
                    *surface.control_point_mut(row, column) += offset * factor;
                }
            }
        }
        _ => return None,
    }
    Some(())
}

fn move_nurbs_boundary(
    surface: &mut NurbsSurface<Vector4>,
    boundary: usize,
    offset: Vector3,
    precision: f64,
) -> Option<()> {
    if !surface.is_clamped() {
        return None;
    }
    let rows = surface.control_points().len();
    let columns = surface.control_points().first()?.len();
    match boundary {
        0 | 2 => {
            let factors = boundary_blend_factors(
                surface.vknot_vec(),
                surface.vdegree(),
                columns,
                boundary == 2,
            )?;
            for row in 0..rows {
                let start = *surface.control_point(row, 0);
                let end = *surface.control_point(row, columns - 1);
                if !nurbs_sequence_is_affine(
                    (0..columns).map(|column| *surface.control_point(row, column)),
                    start,
                    end,
                    &factors,
                    boundary == 2,
                    precision,
                ) {
                    return None;
                }
                for (column, factor) in factors.iter().copied().enumerate() {
                    translate_homogeneous(surface.control_point_mut(row, column), offset * factor);
                }
            }
        }
        1 | 3 => {
            let factors = boundary_blend_factors(
                surface.uknot_vec(),
                surface.udegree(),
                rows,
                boundary == 1,
            )?;
            for column in 0..columns {
                let start = *surface.control_point(0, column);
                let end = *surface.control_point(rows - 1, column);
                if !nurbs_sequence_is_affine(
                    (0..rows).map(|row| *surface.control_point(row, column)),
                    start,
                    end,
                    &factors,
                    boundary == 1,
                    precision,
                ) {
                    return None;
                }
                for (row, factor) in factors.iter().copied().enumerate() {
                    translate_homogeneous(surface.control_point_mut(row, column), offset * factor);
                }
            }
        }
        _ => return None,
    }
    Some(())
}

fn boundary_blend_factors(
    knots: &KnotVec,
    degree: usize,
    control_count: usize,
    contact_at_end: bool,
) -> Option<Vec<f64>> {
    if degree == 0 || knots.len() != control_count + degree + 1 {
        return None;
    }
    let degree_scalar = f64::from(u32::try_from(degree).ok()?);
    let start = knots[0];
    let range = knots[knots.len() - 1] - start;
    if !range.is_finite() || range <= f64::EPSILON {
        return None;
    }
    (0..control_count)
        .map(|index| {
            let greville = knots[index + 1..=index + degree].iter().sum::<f64>() / degree_scalar;
            let parameter = (greville - start) / range;
            (parameter.is_finite() && (-f64::EPSILON..=1.0 + f64::EPSILON).contains(&parameter))
                .then_some(if contact_at_end {
                    parameter.clamp(0.0, 1.0)
                } else {
                    1.0 - parameter.clamp(0.0, 1.0)
                })
        })
        .collect()
}

fn nurbs_sequence_is_affine(
    points: impl Iterator<Item = Vector4>,
    start: Vector4,
    end: Vector4,
    factors: &[f64],
    contact_at_end: bool,
    precision: f64,
) -> bool {
    let weight_scale = start.w.abs().max(end.w.abs()).max(1.0);
    let weight_precision = f64::EPSILON * weight_scale * 16.0;
    if !start.w.is_finite()
        || start.w.abs() <= f64::EPSILON
        || (start.w - end.w).abs() > weight_precision
    {
        return false;
    }
    let start_point = homogeneous_point(start);
    let end_point = homogeneous_point(end);
    points.zip(factors).all(|(point, factor)| {
        let parameter = if contact_at_end {
            *factor
        } else {
            1.0 - *factor
        };
        point.w.is_finite()
            && (point.w - start.w).abs() <= weight_precision
            && points_within(
                homogeneous_point(point),
                start_point + (end_point - start_point) * parameter,
                precision,
            )
    })
}

fn homogeneous_point(point: Vector4) -> Point3 {
    Point3::new(point.x / point.w, point.y / point.w, point.z / point.w)
}

fn translate_homogeneous(point: &mut Vector4, offset: Vector3) {
    point.x += offset.x * point.w;
    point.y += offset.y * point.w;
    point.z += offset.z * point.w;
}
