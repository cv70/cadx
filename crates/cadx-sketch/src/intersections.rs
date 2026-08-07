//! Curve, line, and arc intersection, containment, and crossing predicates.

use std::f64::consts::{PI, TAU};

use crate::{
    ANGULAR_EPSILON, CURVE_SAMPLING_TOLERANCE, GEOMETRY_EPSILON,
    geometry::{SketchLoop2D, SketchSegment2D},
    math::{arc_sweep, cross, deduplicate_points, distance, dot, point_segment_distance_squared},
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CurveIntersections {
    None,
    Points(Vec<[f64; 2]>),
    Overlap,
}

pub(crate) fn forbidden_intersection(
    first: &SketchSegment2D,
    second: &SketchSegment2D,
    allowed: &[[f64; 2]],
) -> bool {
    let allowed_tolerance = if matches!(
        first,
        SketchSegment2D::RationalQuadratic { .. } | SketchSegment2D::CubicBezier { .. }
    ) || matches!(
        second,
        SketchSegment2D::RationalQuadratic { .. } | SketchSegment2D::CubicBezier { .. }
    ) {
        GEOMETRY_EPSILON * 4.0
    } else {
        GEOMETRY_EPSILON
    };
    match curve_intersections(first, second) {
        CurveIntersections::None => false,
        CurveIntersections::Overlap => true,
        CurveIntersections::Points(points) => points.iter().any(|point| {
            allowed
                .iter()
                .all(|allowed| distance(*point, *allowed) > allowed_tolerance)
        }),
    }
}

pub(crate) fn loops_intersect(first: &SketchLoop2D, second: &SketchLoop2D) -> bool {
    first.segments.iter().any(|first| {
        second
            .segments
            .iter()
            .any(|second| !matches!(curve_intersections(first, second), CurveIntersections::None))
    })
}

pub(crate) fn curve_intersections(
    first: &SketchSegment2D,
    second: &SketchSegment2D,
) -> CurveIntersections {
    match (first, second) {
        (
            SketchSegment2D::Line {
                start: first_start,
                end: first_end,
            },
            SketchSegment2D::Line {
                start: second_start,
                end: second_end,
            },
        ) => line_line_intersections(*first_start, *first_end, *second_start, *second_end),
        (SketchSegment2D::Line { start, end }, arc @ SketchSegment2D::Arc { .. })
        | (arc @ SketchSegment2D::Arc { .. }, SketchSegment2D::Line { start, end }) => {
            line_arc_intersections(*start, *end, arc)
        }
        (first @ SketchSegment2D::Arc { .. }, second @ SketchSegment2D::Arc { .. }) => {
            arc_arc_intersections(first, second)
        }
        _ => sampled_curve_intersections(first, second),
    }
}

pub(crate) fn sampled_curve_intersections(
    first: &SketchSegment2D,
    second: &SketchSegment2D,
) -> CurveIntersections {
    const PAIR_BUDGET: usize = 131_072;
    let mut intersections = Vec::new();
    let mut budget = PAIR_BUDGET;
    for first_piece in segment_bezier_pieces(first) {
        for second_piece in segment_bezier_pieces(second) {
            match bezier_piece_intersections(first_piece.clone(), second_piece, &mut budget) {
                CurveIntersections::None => {}
                CurveIntersections::Points(mut points) => intersections.append(&mut points),
                CurveIntersections::Overlap => return CurveIntersections::Overlap,
            }
        }
    }
    deduplicate_points(&mut intersections);
    if intersections.is_empty() {
        CurveIntersections::None
    } else {
        CurveIntersections::Points(intersections)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IntersectionBezierPiece {
    Polynomial(Vec<[f64; 2]>),
    Rational(Vec<[f64; 3]>),
}

impl IntersectionBezierPiece {
    pub(crate) fn projected_controls(&self) -> Vec<[f64; 2]> {
        match self {
            Self::Polynomial(points) => points.clone(),
            Self::Rational(points) => points
                .iter()
                .map(|point| [point[0] / point[2], point[1] / point[2]])
                .collect(),
        }
    }

    fn endpoints(&self) -> [[f64; 2]; 2] {
        let points = self.projected_controls();
        [points[0], points[points.len() - 1]]
    }

    fn bounds(&self) -> [[f64; 2]; 2] {
        self.projected_controls().into_iter().fold(
            [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]],
            |mut bounds, point| {
                for axis in 0..2 {
                    bounds[0][axis] = bounds[0][axis].min(point[axis]);
                    bounds[1][axis] = bounds[1][axis].max(point[axis]);
                }
                bounds
            },
        )
    }

    fn extent(&self) -> f64 {
        let bounds = self.bounds();
        (bounds[1][0] - bounds[0][0]).max(bounds[1][1] - bounds[0][1])
    }

    fn split(&self) -> (Self, Self) {
        match self {
            Self::Polynomial(points) => {
                let (left, right) = split_bezier_controls(points, |first, second| {
                    [first[0].midpoint(second[0]), first[1].midpoint(second[1])]
                });
                (Self::Polynomial(left), Self::Polynomial(right))
            }
            Self::Rational(points) => {
                let (left, right) = split_bezier_controls(points, |first, second| {
                    [
                        first[0].midpoint(second[0]),
                        first[1].midpoint(second[1]),
                        first[2].midpoint(second[2]),
                    ]
                });
                (Self::Rational(left), Self::Rational(right))
            }
        }
    }

    fn coincides_with(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }
        match (self, other) {
            (Self::Polynomial(first), Self::Polynomial(second)) => {
                first.len() == second.len()
                    && first
                        .iter()
                        .zip(second.iter().rev())
                        .all(|(first, second)| distance(*first, *second) <= GEOMETRY_EPSILON)
            }
            (Self::Rational(first), Self::Rational(second)) => {
                first.len() == second.len()
                    && first
                        .iter()
                        .zip(second.iter().rev())
                        .all(|(first, second)| {
                            first
                                .iter()
                                .zip(second)
                                .all(|(first, second)| (first - second).abs() <= GEOMETRY_EPSILON)
                        })
            }
            (Self::Polynomial(_), Self::Rational(_)) | (Self::Rational(_), Self::Polynomial(_)) => {
                false
            }
        }
    }
}

pub(crate) fn split_bezier_controls<const N: usize>(
    points: &[[f64; N]],
    midpoint: impl Fn([f64; N], [f64; N]) -> [f64; N],
) -> (Vec<[f64; N]>, Vec<[f64; N]>) {
    let mut levels = vec![points.to_vec()];
    while levels.last().is_some_and(|level| level.len() > 1) {
        let next = levels
            .last()
            .expect("Bezier subdivision has one initial level")
            .windows(2)
            .map(|pair| midpoint(pair[0], pair[1]))
            .collect();
        levels.push(next);
    }
    let left = levels.iter().map(|level| level[0]).collect();
    let right = levels
        .iter()
        .rev()
        .map(|level| level[level.len() - 1])
        .collect();
    (left, right)
}

pub(crate) fn segment_bezier_pieces(segment: &SketchSegment2D) -> Vec<IntersectionBezierPiece> {
    match segment {
        SketchSegment2D::Line { start, end } => {
            vec![IntersectionBezierPiece::Polynomial(vec![*start, *end])]
        }
        SketchSegment2D::RationalQuadratic {
            start,
            control,
            end,
            weight,
        } => vec![IntersectionBezierPiece::Rational(vec![
            [start[0], start[1], 1.0],
            [control[0] * weight, control[1] * weight, *weight],
            [end[0], end[1], 1.0],
        ])],
        SketchSegment2D::CubicBezier {
            start,
            control1,
            control2,
            end,
        } => vec![IntersectionBezierPiece::Polynomial(vec![
            *start, *control1, *control2, *end,
        ])],
        SketchSegment2D::Arc {
            start,
            end,
            center,
            ccw,
        } => {
            let sweep = arc_sweep(*start, *end, *center, *ccw);
            let mut count = 1_u32;
            while f64::from(count) * (PI / 2.0) < sweep.abs() && count < 4 {
                count += 1;
            }
            let radius = distance(*start, *center);
            let start_angle = (start[1] - center[1]).atan2(start[0] - center[0]);
            (0..count)
                .map(|index| {
                    let first_angle =
                        sweep.mul_add(f64::from(index) / f64::from(count), start_angle);
                    let second_angle =
                        sweep.mul_add(f64::from(index + 1) / f64::from(count), start_angle);
                    let half_angle = (second_angle - first_angle) / 2.0;
                    let middle_angle = first_angle + half_angle;
                    let weight = half_angle.cos();
                    let first = if index == 0 {
                        *start
                    } else {
                        [
                            radius.mul_add(first_angle.cos(), center[0]),
                            radius.mul_add(first_angle.sin(), center[1]),
                        ]
                    };
                    let last = if index + 1 == count {
                        *end
                    } else {
                        [
                            radius.mul_add(second_angle.cos(), center[0]),
                            radius.mul_add(second_angle.sin(), center[1]),
                        ]
                    };
                    let control = [
                        (radius / weight).mul_add(middle_angle.cos(), center[0]),
                        (radius / weight).mul_add(middle_angle.sin(), center[1]),
                    ];
                    IntersectionBezierPiece::Rational(vec![
                        [first[0], first[1], 1.0],
                        [control[0] * weight, control[1] * weight, weight],
                        [last[0], last[1], 1.0],
                    ])
                })
                .collect()
        }
    }
}

pub(crate) fn bezier_piece_intersections(
    first: IntersectionBezierPiece,
    second: IntersectionBezierPiece,
    budget: &mut usize,
) -> CurveIntersections {
    const MAX_DEPTH: u8 = 96;
    const RESOLUTION: f64 = GEOMETRY_EPSILON * 0.5;

    if first.coincides_with(&second) {
        return CurveIntersections::Overlap;
    }
    let mut stack = vec![(first, second, 0_u8)];
    let mut intersections = Vec::new();
    while let Some((first, second, depth)) = stack.pop() {
        if *budget == 0 {
            return CurveIntersections::Overlap;
        }
        *budget -= 1;
        let first_bounds = first.bounds();
        let second_bounds = second.bounds();
        if bounds_are_separated(first_bounds, second_bounds) {
            continue;
        }
        let first_extent = first.extent();
        let second_extent = second.extent();
        if first_extent.max(second_extent) <= RESOLUTION {
            let endpoints = first.endpoints();
            let other_endpoints = second.endpoints();
            let candidate = endpoints
                .into_iter()
                .find_map(|first| {
                    other_endpoints
                        .into_iter()
                        .find(|second| distance(first, *second) <= GEOMETRY_EPSILON)
                        .map(|second| [first[0].midpoint(second[0]), first[1].midpoint(second[1])])
                })
                .unwrap_or_else(|| bounds_overlap_center(first_bounds, second_bounds));
            intersections.push(candidate);
            continue;
        }
        if depth == MAX_DEPTH {
            return CurveIntersections::Overlap;
        }
        if first_extent >= second_extent {
            let (left, right) = first.split();
            stack.push((right, second.clone(), depth + 1));
            stack.push((left, second, depth + 1));
        } else {
            let (left, right) = second.split();
            stack.push((first.clone(), right, depth + 1));
            stack.push((first, left, depth + 1));
        }
    }
    deduplicate_points(&mut intersections);
    if intersections.is_empty() {
        CurveIntersections::None
    } else {
        CurveIntersections::Points(intersections)
    }
}

pub(crate) fn bounds_are_separated(first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> bool {
    (0..2).any(|axis| first[1][axis] < second[0][axis] || second[1][axis] < first[0][axis])
}

pub(crate) fn bounds_overlap_center(first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> [f64; 2] {
    std::array::from_fn(|axis| {
        first[0][axis]
            .max(second[0][axis])
            .midpoint(first[1][axis].min(second[1][axis]))
    })
}

pub(crate) fn line_line_intersections(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
) -> CurveIntersections {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let cd = [d[0] - c[0], d[1] - c[1]];
    let denominator = cross(ab, cd);
    let ac = [c[0] - a[0], c[1] - a[1]];
    if denominator.abs() > GEOMETRY_EPSILON {
        let first = cross(ac, cd) / denominator;
        let second = cross(ac, ab) / denominator;
        if (-GEOMETRY_EPSILON..=1.0 + GEOMETRY_EPSILON).contains(&first)
            && (-GEOMETRY_EPSILON..=1.0 + GEOMETRY_EPSILON).contains(&second)
        {
            return CurveIntersections::Points(vec![[
                ab[0].mul_add(first, a[0]),
                ab[1].mul_add(first, a[1]),
            ]]);
        }
        return CurveIntersections::None;
    }
    if cross(ac, ab).abs() > GEOMETRY_EPSILON {
        return CurveIntersections::None;
    }
    let mut points = [a, b, c, d]
        .into_iter()
        .filter(|point| point_on_line(*point, a, b) && point_on_line(*point, c, d))
        .collect::<Vec<_>>();
    deduplicate_points(&mut points);
    match points.len() {
        0 => CurveIntersections::None,
        1 => CurveIntersections::Points(points),
        _ => CurveIntersections::Overlap,
    }
}

pub(crate) fn line_arc_intersections(
    start: [f64; 2],
    end: [f64; 2],
    arc: &SketchSegment2D,
) -> CurveIntersections {
    let SketchSegment2D::Arc {
        start: arc_start,
        end: arc_end,
        center,
        ccw,
    } = arc
    else {
        unreachable!("line_arc_intersections requires an arc")
    };
    let direction = [end[0] - start[0], end[1] - start[1]];
    let offset = [start[0] - center[0], start[1] - center[1]];
    let radius = distance(*arc_start, *center);
    let a = dot(direction, direction);
    let b = 2.0 * dot(offset, direction);
    let c = dot(offset, offset) - radius * radius;
    let discriminant = b.mul_add(b, -4.0 * a * c);
    if discriminant < -GEOMETRY_EPSILON {
        return CurveIntersections::None;
    }
    let mut points = Vec::new();
    let root = discriminant.max(0.0).sqrt();
    for parameter in [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)] {
        if (-GEOMETRY_EPSILON..=1.0 + GEOMETRY_EPSILON).contains(&parameter) {
            let point = [
                direction[0].mul_add(parameter, start[0]),
                direction[1].mul_add(parameter, start[1]),
            ];
            if arc_contains_point(*arc_start, *arc_end, *center, *ccw, point, true) {
                points.push(point);
            }
        }
    }
    deduplicate_points(&mut points);
    if points.is_empty() {
        CurveIntersections::None
    } else {
        CurveIntersections::Points(points)
    }
}

pub(crate) fn arc_arc_intersections(
    first: &SketchSegment2D,
    second: &SketchSegment2D,
) -> CurveIntersections {
    let SketchSegment2D::Arc {
        start: first_start,
        end: first_end,
        center: first_center,
        ccw: first_ccw,
    } = first
    else {
        unreachable!("arc_arc_intersections requires arcs")
    };
    let SketchSegment2D::Arc {
        start: second_start,
        end: second_end,
        center: second_center,
        ccw: second_ccw,
    } = second
    else {
        unreachable!("arc_arc_intersections requires arcs")
    };
    let first_radius = distance(*first_start, *first_center);
    let second_radius = distance(*second_start, *second_center);
    let center_distance = distance(*first_center, *second_center);
    if center_distance <= GEOMETRY_EPSILON
        && (first_radius - second_radius).abs() <= GEOMETRY_EPSILON
    {
        let first_midpoint = first.midpoint();
        let second_midpoint = second.midpoint();
        if arc_contains_point(
            *second_start,
            *second_end,
            *second_center,
            *second_ccw,
            first_midpoint,
            false,
        ) || arc_contains_point(
            *first_start,
            *first_end,
            *first_center,
            *first_ccw,
            second_midpoint,
            false,
        ) {
            return CurveIntersections::Overlap;
        }
        let mut points = [*first_start, *first_end, *second_start, *second_end]
            .into_iter()
            .filter(|point| {
                arc_contains_point(
                    *first_start,
                    *first_end,
                    *first_center,
                    *first_ccw,
                    *point,
                    true,
                ) && arc_contains_point(
                    *second_start,
                    *second_end,
                    *second_center,
                    *second_ccw,
                    *point,
                    true,
                )
            })
            .collect::<Vec<_>>();
        deduplicate_points(&mut points);
        return if points.is_empty() {
            CurveIntersections::None
        } else {
            CurveIntersections::Points(points)
        };
    }
    if center_distance > first_radius + second_radius + GEOMETRY_EPSILON
        || center_distance < (first_radius - second_radius).abs() - GEOMETRY_EPSILON
        || center_distance <= GEOMETRY_EPSILON
    {
        return CurveIntersections::None;
    }
    let along = (first_radius * first_radius - second_radius * second_radius
        + center_distance * center_distance)
        / (2.0 * center_distance);
    let height_squared = first_radius * first_radius - along * along;
    if height_squared < -GEOMETRY_EPSILON {
        return CurveIntersections::None;
    }
    let unit = [
        (second_center[0] - first_center[0]) / center_distance,
        (second_center[1] - first_center[1]) / center_distance,
    ];
    let base = [
        unit[0].mul_add(along, first_center[0]),
        unit[1].mul_add(along, first_center[1]),
    ];
    let height = height_squared.max(0.0).sqrt();
    let perpendicular = [-unit[1] * height, unit[0] * height];
    let mut points = [
        [base[0] + perpendicular[0], base[1] + perpendicular[1]],
        [base[0] - perpendicular[0], base[1] - perpendicular[1]],
    ]
    .into_iter()
    .filter(|point| {
        arc_contains_point(
            *first_start,
            *first_end,
            *first_center,
            *first_ccw,
            *point,
            true,
        ) && arc_contains_point(
            *second_start,
            *second_end,
            *second_center,
            *second_ccw,
            *point,
            true,
        )
    })
    .collect::<Vec<_>>();
    deduplicate_points(&mut points);
    if points.is_empty() {
        CurveIntersections::None
    } else {
        CurveIntersections::Points(points)
    }
}

pub(crate) fn point_on_segment(point: [f64; 2], segment: &SketchSegment2D) -> bool {
    match segment {
        SketchSegment2D::Line { start, end } => point_on_line(point, *start, *end),
        SketchSegment2D::Arc {
            start,
            end,
            center,
            ccw,
        } => arc_contains_point(*start, *end, *center, *ccw, point, true),
        SketchSegment2D::RationalQuadratic { .. } | SketchSegment2D::CubicBezier { .. } => {
            segment.distance_squared_to(point)
                <= (CURVE_SAMPLING_TOLERANCE + GEOMETRY_EPSILON).powi(2)
        }
    }
}

pub(crate) fn point_on_line(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> bool {
    point_segment_distance_squared(point, start, end) <= GEOMETRY_EPSILON.powi(2)
}

pub(crate) fn arc_contains_point(
    start: [f64; 2],
    end: [f64; 2],
    center: [f64; 2],
    ccw: bool,
    point: [f64; 2],
    include_endpoints: bool,
) -> bool {
    let radius = distance(start, center);
    let point_radius = distance(point, center);
    if (point_radius - radius).abs() > GEOMETRY_EPSILON * radius.max(1.0) {
        return false;
    }
    let sweep = arc_sweep(start, end, center, ccw);
    let start_angle = (start[1] - center[1]).atan2(start[0] - center[0]);
    let point_angle = (point[1] - center[1]).atan2(point[0] - center[0]);
    let progress = if ccw {
        (point_angle - start_angle).rem_euclid(TAU)
    } else {
        (start_angle - point_angle).rem_euclid(TAU)
    };
    let extent = sweep.abs();
    if include_endpoints {
        progress <= extent + ANGULAR_EPSILON
    } else {
        progress > ANGULAR_EPSILON && progress < extent - ANGULAR_EPSILON
    }
}

pub(crate) fn ray_crossings(point: [f64; 2], segment: &SketchSegment2D) -> usize {
    match segment {
        SketchSegment2D::Line { start, end } => usize::from(
            (start[1] > point[1]) != (end[1] > point[1])
                && point[0]
                    < (end[0] - start[0]) * (point[1] - start[1]) / (end[1] - start[1]) + start[0],
        ),
        SketchSegment2D::Arc {
            start,
            end,
            center,
            ccw,
        } => {
            let radius = distance(*start, *center);
            let vertical = point[1] - center[1];
            if vertical.abs() > radius {
                return 0;
            }
            let horizontal = (radius * radius - vertical * vertical).max(0.0).sqrt();
            let mut intersections = 0;
            for x in [center[0] - horizontal, center[0] + horizontal] {
                let candidate = [x, point[1]];
                if x > point[0]
                    && horizontal > GEOMETRY_EPSILON
                    && arc_contains_point(*start, *end, *center, *ccw, candidate, true)
                    && distance(candidate, *end) > GEOMETRY_EPSILON
                {
                    intersections += 1;
                }
            }
            intersections
        }
        SketchSegment2D::RationalQuadratic { .. } | SketchSegment2D::CubicBezier { .. } => segment
            .sampled_points(PI / 90.0)
            .windows(2)
            .map(|points| {
                usize::from(
                    (points[0][1] > point[1]) != (points[1][1] > point[1])
                        && point[0]
                            < (points[1][0] - points[0][0]) * (point[1] - points[0][1])
                                / (points[1][1] - points[0][1])
                                + points[0][0],
                )
            })
            .sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SketchGeometryError;

    #[test]
    fn bezier_convex_hull_intersection_finds_an_interior_tangent() {
        let loop_ = SketchLoop2D {
            segments: vec![
                SketchSegment2D::CubicBezier {
                    start: [0.0, 0.0],
                    control1: [10.0 / 3.0, 8.0],
                    control2: [20.0 / 3.0, 8.0],
                    end: [10.0, 0.0],
                },
                SketchSegment2D::Line {
                    start: [10.0, 0.0],
                    end: [10.0, 6.0],
                },
                SketchSegment2D::Line {
                    start: [10.0, 6.0],
                    end: [0.0, 6.0],
                },
                SketchSegment2D::Line {
                    start: [0.0, 6.0],
                    end: [0.0, 0.0],
                },
            ],
        };

        assert_eq!(
            loop_.validate(),
            Err(SketchGeometryError::SelfIntersection {
                first: 0,
                second: 2,
            })
        );
    }
}
