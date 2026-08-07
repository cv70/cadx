//! Nonlinear parameter vector layout and constraint residual assembly.

use crate::{
    GEOMETRY_EPSILON, PointId, SegmentId,
    constraints::{Constraint, ConstraintGeometry},
    geometry::SketchSegment2D,
    math::{cross, distance, dot, normalize_angle, normalize_vector},
    parameters::{
        closest_parameter_curve_point, constraint_tangent, parameter_center, parameter_control,
        parameter_point, parameter_segment_points, parameterized_profile_segment,
        segment_curvature_at_shared, shared_vertex,
    },
};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SegmentParameterOffsets {
    pub(crate) center: Option<usize>,
    pub(crate) controls: [Option<usize>; 2],
}

pub(crate) fn sketch_parameters(
    geometry: ConstraintGeometry<'_>,
) -> (Vec<f64>, Vec<SegmentParameterOffsets>) {
    let mut parameters = geometry
        .profile
        .vertices()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    for segment in geometry.construction {
        parameters.extend(segment.start());
        parameters.extend(segment.end());
    }
    let mut center_offsets = vec![SegmentParameterOffsets::default(); geometry.segment_count()];
    for (index, offsets) in center_offsets.iter_mut().enumerate() {
        let segment = geometry.segment(SegmentId::try_from(index).expect("segment limit fits u32"));
        match segment {
            SketchSegment2D::Arc { center, .. } => {
                offsets.center = Some(parameters.len());
                parameters.extend(center);
            }
            SketchSegment2D::RationalQuadratic { control, .. } => {
                offsets.controls[0] = Some(parameters.len());
                parameters.extend(control);
            }
            SketchSegment2D::CubicBezier {
                control1, control2, ..
            } => {
                offsets.controls[0] = Some(parameters.len());
                parameters.extend(control1);
                offsets.controls[1] = Some(parameters.len());
                parameters.extend(control2);
            }
            SketchSegment2D::Line { .. } => {}
        }
    }
    (parameters, center_offsets)
}

pub(crate) fn rebuild_parameter_segment(
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    id: SegmentId,
    template: &SketchSegment2D,
) -> SketchSegment2D {
    let (start, end) = parameter_segment_points(geometry, parameters, id);
    match template {
        SketchSegment2D::Line { .. } => SketchSegment2D::Line { start, end },
        SketchSegment2D::Arc { ccw, .. } => SketchSegment2D::Arc {
            start,
            end,
            center: parameter_center(
                parameters,
                center_offsets,
                usize::try_from(id).expect("validated segment id"),
            ),
            ccw: *ccw,
        },
        SketchSegment2D::RationalQuadratic { weight, .. } => SketchSegment2D::RationalQuadratic {
            start,
            control: parameter_control(
                parameters,
                center_offsets,
                usize::try_from(id).expect("validated segment id"),
                0,
            ),
            end,
            weight: *weight,
        },
        SketchSegment2D::CubicBezier { .. } => SketchSegment2D::CubicBezier {
            start,
            control1: parameter_control(
                parameters,
                center_offsets,
                usize::try_from(id).expect("validated segment id"),
                0,
            ),
            control2: parameter_control(
                parameters,
                center_offsets,
                usize::try_from(id).expect("validated segment id"),
                1,
            ),
            end,
        },
    }
}

pub(crate) fn nonlinear_residuals(
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    constraints: &[Constraint],
) -> Vec<f64> {
    let mut residuals = Vec::with_capacity(geometry.segment_count() + constraints.len() * 2);
    append_intrinsic_residuals(&mut residuals, geometry, center_offsets, parameters);
    for constraint in constraints {
        append_nonlinear_constraint_residuals(
            &mut residuals,
            geometry,
            center_offsets,
            parameters,
            constraint,
        );
    }
    residuals
}

pub(crate) fn append_intrinsic_residuals(
    residuals: &mut Vec<f64>,
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
) {
    for index in 0..geometry.segment_count() {
        let id = SegmentId::try_from(index).expect("segment limit fits u32");
        let segment = geometry.segment(id);
        if segment.is_arc() {
            let (start, end) = parameter_segment_points(geometry, parameters, id);
            let center = parameter_center(parameters, center_offsets, index);
            residuals.push(distance(start, center) - distance(end, center));
        }
    }
}

pub(crate) fn append_nonlinear_constraint_residuals(
    residuals: &mut Vec<f64>,
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    constraint: &Constraint,
) {
    let point =
        |id: PointId| parameter_point(parameters, usize::try_from(id).expect("validated point id"));
    let segment_index = |id: SegmentId| usize::try_from(id).expect("validated segment id");
    let segment_points = |id: SegmentId| parameter_segment_points(geometry, parameters, id);
    let arc_center =
        |id: SegmentId| parameter_center(parameters, center_offsets, segment_index(id));
    let arc_radius = |id: SegmentId| {
        let (start, end) = segment_points(id);
        let center = arc_center(id);
        f64::midpoint(distance(start, center), distance(end, center))
    };
    let line_direction = |id: SegmentId| {
        let (start, end) = segment_points(id);
        normalize_vector([end[0] - start[0], end[1] - start[1]])
    };
    match constraint {
        Constraint::Coincident { first, second } => {
            let first = point(*first);
            let second = point(*second);
            residuals.extend([first[0] - second[0], first[1] - second[1]]);
        }
        Constraint::Horizontal { segment } => {
            let (start, end) = segment_points(*segment);
            residuals.push(end[1] - start[1]);
        }
        Constraint::Vertical { segment } => {
            let (start, end) = segment_points(*segment);
            residuals.push(end[0] - start[0]);
        }
        Constraint::Fixed { point: id, x, y } => {
            let actual = point(*id);
            residuals.extend([actual[0] - x, actual[1] - y]);
        }
        Constraint::Distance {
            first,
            second,
            distance: expected,
        } => residuals.push(distance(point(*first), point(*second)) - expected),
        Constraint::HorizontalDistance {
            first,
            second,
            distance,
        } => residuals.push(point(*second)[0] - point(*first)[0] - distance),
        Constraint::VerticalDistance {
            first,
            second,
            distance,
        } => residuals.push(point(*second)[1] - point(*first)[1] - distance),
        Constraint::PointLineDistance {
            point: point_id,
            line,
            distance,
        } => {
            let actual = point(*point_id);
            let (start, _) = segment_points(*line);
            let direction = line_direction(*line);
            let reference_point = geometry.point(*point_id);
            let reference_segment = geometry.segment(*line);
            let reference_direction = normalize_vector([
                reference_segment.end()[0] - reference_segment.start()[0],
                reference_segment.end()[1] - reference_segment.start()[1],
            ]);
            let reference_side = cross(
                reference_direction,
                [
                    reference_point[0] - reference_segment.start()[0],
                    reference_point[1] - reference_segment.start()[1],
                ],
            );
            let signed_distance = distance.copysign(if reference_side.abs() > GEOMETRY_EPSILON {
                reference_side
            } else {
                1.0
            });
            residuals.push(
                cross(direction, [actual[0] - start[0], actual[1] - start[1]]) - signed_distance,
            );
        }
        Constraint::LineThroughCenter { line, arc } => {
            let (start, _) = segment_points(*line);
            let center = arc_center(*arc);
            residuals.push(cross(
                line_direction(*line),
                [center[0] - start[0], center[1] - start[1]],
            ));
        }
        Constraint::PointOnCurve {
            point: point_id,
            segment,
        } => {
            let actual = point(*point_id);
            let closest = closest_parameter_curve_point(
                geometry,
                center_offsets,
                parameters,
                *segment,
                actual,
            );
            residuals.extend([actual[0] - closest[0], actual[1] - closest[1]]);
        }
        Constraint::Midpoint { point: id, segment } => {
            let actual = point(*id);
            let (start, end) = segment_points(*segment);
            residuals.extend([
                actual[0] - start[0].midpoint(end[0]),
                actual[1] - start[1].midpoint(end[1]),
            ]);
        }
        Constraint::Symmetric {
            first,
            second,
            axis,
        } => {
            let first = point(*first);
            let second = point(*second);
            let (axis_start, axis_end) = segment_points(*axis);
            let axis_direction =
                normalize_vector([axis_end[0] - axis_start[0], axis_end[1] - axis_start[1]]);
            let midpoint = [first[0].midpoint(second[0]), first[1].midpoint(second[1])];
            residuals.extend([
                cross(
                    axis_direction,
                    [midpoint[0] - axis_start[0], midpoint[1] - axis_start[1]],
                ),
                dot([second[0] - first[0], second[1] - first[1]], axis_direction),
            ]);
        }
        Constraint::Length { segment, length } => {
            let (start, end) = segment_points(*segment);
            residuals.push(distance(start, end) - length);
        }
        Constraint::EqualLength { first, second } => {
            let (first_start, first_end) = segment_points(*first);
            let (second_start, second_end) = segment_points(*second);
            residuals.push(distance(first_start, first_end) - distance(second_start, second_end));
        }
        Constraint::Parallel { first, second } => {
            residuals.push(cross(line_direction(*first), line_direction(*second)));
        }
        Constraint::Perpendicular { first, second } => {
            residuals.push(dot(line_direction(*first), line_direction(*second)));
        }
        Constraint::Angle {
            first,
            second,
            angle_degrees,
        } => {
            let first = line_direction(*first);
            let second = line_direction(*second);
            let actual = cross(first, second).atan2(dot(first, second));
            residuals.push(normalize_angle(actual - angle_degrees.to_radians()));
        }
        Constraint::Radius { segment, radius } => residuals.push(arc_radius(*segment) - radius),
        Constraint::FixedCenter { segment, x, y } => {
            let center = arc_center(*segment);
            residuals.extend([center[0] - x, center[1] - y]);
        }
        Constraint::EqualRadius { first, second } => {
            residuals.push(arc_radius(*first) - arc_radius(*second));
        }
        Constraint::Concentric { first, second } => {
            let first = arc_center(*first);
            let second = arc_center(*second);
            residuals.extend([first[0] - second[0], first[1] - second[1]]);
        }
        Constraint::Tangent { first, second } => {
            let shared = shared_vertex(geometry.profile.segments.len(), *first, *second)
                .expect("validated tangent adjacency");
            let point = parameter_point(parameters, shared);
            let first = constraint_tangent(
                geometry.profile,
                center_offsets,
                parameters,
                segment_index(*first),
                point,
            );
            let second = constraint_tangent(
                geometry.profile,
                center_offsets,
                parameters,
                segment_index(*second),
                point,
            );
            residuals.push(cross(first, second));
        }
        Constraint::CurvatureContinuous { first, second } => {
            let shared = shared_vertex(geometry.profile.segments.len(), *first, *second)
                .expect("validated curvature-continuity adjacency");
            let point = parameter_point(parameters, shared);
            let first_tangent = constraint_tangent(
                geometry.profile,
                center_offsets,
                parameters,
                segment_index(*first),
                point,
            );
            let second_tangent = constraint_tangent(
                geometry.profile,
                center_offsets,
                parameters,
                segment_index(*second),
                point,
            );
            let first_curve = parameterized_profile_segment(
                geometry.profile,
                center_offsets,
                parameters,
                segment_index(*first),
            );
            let second_curve = parameterized_profile_segment(
                geometry.profile,
                center_offsets,
                parameters,
                segment_index(*second),
            );
            let first_curvature = segment_curvature_at_shared(&first_curve, point);
            let second_curvature = segment_curvature_at_shared(&second_curve, point);
            let curvature_scale = first_curve
                .length()
                .max(second_curve.length())
                .max(GEOMETRY_EPSILON);
            let tangent_angle =
                cross(first_tangent, second_tangent).atan2(dot(first_tangent, second_tangent));
            residuals.extend([
                normalize_angle(tangent_angle),
                (first_curvature - second_curvature) * curvature_scale,
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SketchError, SketchLoop2D, SketchRegion2D, SolverConfig, construction_point_ids,
        construction_segment_id,
        curves::{CubicBezier2D, RationalQuadraticBezier2D},
        solve_region, solve_sketch,
        test_support::circle,
    };
    use std::f64::consts::PI;

    #[test]
    fn solves_advanced_line_relationships_deterministically() {
        let region = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [9.0, 1.0], [10.0, 6.0], [1.0, 5.0]],
            Vec::new(),
        );
        let constraints = vec![
            Constraint::Fixed {
                point: 0,
                x: 0.0,
                y: 0.0,
            },
            Constraint::Length {
                segment: 0,
                length: 10.0,
            },
            Constraint::Length {
                segment: 1,
                length: 5.0,
            },
            Constraint::Parallel {
                first: 0,
                second: 2,
            },
            Constraint::EqualLength {
                first: 0,
                second: 2,
            },
            Constraint::Perpendicular {
                first: 0,
                second: 1,
            },
            Constraint::Parallel {
                first: 1,
                second: 3,
            },
            Constraint::EqualLength {
                first: 1,
                second: 3,
            },
        ];

        let first = solve_region(&region, &constraints, SolverConfig::default()).unwrap();
        let second = solve_region(&region, &constraints, SolverConfig::default()).unwrap();
        assert_eq!(first, second);
        let directions = first
            .profile
            .segments
            .iter()
            .map(|segment| {
                let start = segment.start();
                let end = segment.end();
                normalize_vector([end[0] - start[0], end[1] - start[1]])
            })
            .collect::<Vec<_>>();
        let lengths = first
            .profile
            .segments
            .iter()
            .map(SketchSegment2D::length)
            .collect::<Vec<_>>();
        assert!((lengths[0] - 10.0).abs() < 1.0e-8);
        assert!((lengths[1] - 5.0).abs() < 1.0e-8);
        assert!((lengths[0] - lengths[2]).abs() < 1.0e-8);
        assert!((lengths[1] - lengths[3]).abs() < 1.0e-8);
        assert!(cross(directions[0], directions[2]).abs() < 1.0e-8);
        assert!(cross(directions[1], directions[3]).abs() < 1.0e-8);
        assert!(dot(directions[0], directions[1]).abs() < 1.0e-8);
        first.validate().unwrap();
    }

    #[test]
    fn solves_point_relationships_with_construction_geometry_deterministically() {
        let region = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [12.0, 0.0], [12.0, 8.0], [0.0, 8.0]],
            Vec::new(),
        );
        let construction = vec![
            SketchSegment2D::Line {
                start: [0.0, 0.0],
                end: [10.0, 0.0],
            },
            SketchSegment2D::Line {
                start: [4.0, 4.0],
                end: [4.0, 8.0],
            },
            SketchSegment2D::Line {
                start: [0.0, 2.0],
                end: [10.0, 2.0],
            },
            SketchSegment2D::Line {
                start: [7.0, 6.0],
                end: [7.0, 10.0],
            },
            SketchSegment2D::Line {
                start: [0.0, -5.0],
                end: [0.0, 5.0],
            },
            SketchSegment2D::Line {
                start: [-3.0, 1.0],
                end: [-6.0, 1.0],
            },
            SketchSegment2D::Line {
                start: [4.0, 3.0],
                end: [7.0, 3.0],
            },
        ];
        let fixed = |point, x, y| Constraint::Fixed { point, x, y };
        let constraints = vec![
            fixed(4, 0.0, 0.0),
            fixed(5, 10.0, 0.0),
            fixed(7, 4.0, 8.0),
            Constraint::PointOnCurve {
                point: 6,
                segment: 4,
            },
            fixed(8, 0.0, 2.0),
            fixed(9, 10.0, 2.0),
            fixed(11, 7.0, 10.0),
            Constraint::Midpoint {
                point: 10,
                segment: 6,
            },
            fixed(12, 0.0, -5.0),
            fixed(13, 0.0, 5.0),
            fixed(15, -6.0, 1.0),
            fixed(17, 7.0, 3.0),
            Constraint::Symmetric {
                first: 14,
                second: 16,
                axis: 8,
            },
        ];

        let first = solve_sketch(
            &region,
            &construction,
            &constraints,
            SolverConfig::default(),
        )
        .unwrap();
        let second = solve_sketch(
            &region,
            &construction,
            &constraints,
            SolverConfig::default(),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.region, region);
        assert_eq!(construction_segment_id(4, 0), Some(4));
        assert_eq!(construction_point_ids(4, 0), Some([4, 5]));
        assert_eq!(construction_point_ids(4, 6), Some([16, 17]));

        let point_on_curve = first.construction[1].start();
        assert!((0.0..=10.0).contains(&point_on_curve[0]));
        assert!(point_on_curve[1].abs() < 1.0e-7);
        let midpoint = first.construction[3].start();
        assert!((midpoint[0] - 5.0).abs() < 1.0e-7);
        assert!((midpoint[1] - 2.0).abs() < 1.0e-7);
        let symmetric_first = first.construction[5].start();
        let symmetric_second = first.construction[6].start();
        assert!((symmetric_first[0] + symmetric_second[0]).abs() < 1.0e-7);
        assert!((symmetric_first[1] - symmetric_second[1]).abs() < 1.0e-7);
    }

    #[test]
    fn solves_exact_circle_radius_center_and_equality_constraints() {
        let region = SketchRegion2D {
            profile: circle([1.0, -2.0], 4.0, true),
            holes: Vec::new(),
        };
        let constraints = vec![
            Constraint::FixedCenter {
                segment: 0,
                x: 3.0,
                y: 5.0,
            },
            Constraint::Concentric {
                first: 0,
                second: 1,
            },
            Constraint::Radius {
                segment: 0,
                radius: 6.0,
            },
            Constraint::EqualRadius {
                first: 0,
                second: 1,
            },
        ];

        let first = solve_region(&region, &constraints, SolverConfig::default()).unwrap();
        let second = solve_region(&region, &constraints, SolverConfig::default()).unwrap();
        assert_eq!(first, second);
        first.validate().unwrap();
        for segment in &first.profile.segments {
            let SketchSegment2D::Arc {
                start, end, center, ..
            } = segment
            else {
                panic!("expected arc");
            };
            assert!((center[0] - 3.0).abs() < 1.0e-8);
            assert!((center[1] - 5.0).abs() < 1.0e-8);
            assert!((distance(*start, *center) - 6.0).abs() < 1.0e-8);
            assert!((distance(*end, *center) - 6.0).abs() < 1.0e-8);
        }
        assert!((first.profile.signed_area() - PI * 36.0).abs() < 1.0e-6);
    }

    #[test]
    fn conflicting_curved_constraints_do_not_return_partial_geometry() {
        let region = SketchRegion2D {
            profile: circle([0.0, 0.0], 4.0, true),
            holes: Vec::new(),
        };
        let error = solve_region(
            &region,
            &[
                Constraint::FixedCenter {
                    segment: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::FixedCenter {
                    segment: 0,
                    x: 10.0,
                    y: 0.0,
                },
            ],
            SolverConfig {
                max_iterations: 64,
                ..SolverConfig::default()
            },
        )
        .unwrap_err();
        assert!(matches!(error, SketchError::ConstraintConflict { .. }));
    }

    #[test]
    fn solves_point_dimensions_and_line_through_arc_center() {
        let region = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]],
            Vec::new(),
        );
        let construction = vec![
            SketchSegment2D::Line {
                start: [0.0, 0.0],
                end: [3.0, 1.0],
            },
            SketchSegment2D::Arc {
                start: [8.0, 2.0],
                end: [4.0, 2.0],
                center: [6.0, 0.0],
                ccw: true,
            },
        ];
        let constraints = vec![
            Constraint::Fixed {
                point: 4,
                x: 0.0,
                y: 0.0,
            },
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
        ];
        let solved = solve_sketch(
            &region,
            &construction,
            &constraints,
            SolverConfig::default(),
        )
        .unwrap();
        let SketchSegment2D::Line { start, end } = solved.construction[0] else {
            panic!("expected solved construction line");
        };
        assert!(distance(start, [0.0, 0.0]) < 1.0e-8);
        assert!(distance(end, [4.0, 0.0]) < 1.0e-8);
        let SketchSegment2D::Arc {
            start: arc_start,
            center,
            ..
        } = solved.construction[1]
        else {
            panic!("expected solved construction arc");
        };
        assert!(center[1].abs() < 1.0e-8);
        assert!((arc_start[1].abs() - 2.0).abs() < 1.0e-7);
        assert!(solved.diagnostic.rank >= 7);
    }

    #[test]
    fn rational_and_cubic_segments_validate_solve_and_keep_control_identity() {
        let region = SketchRegion2D {
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
        };

        region.validate().unwrap();
        let solved = solve_sketch(&region, &[], &[], SolverConfig::default()).unwrap();
        assert_eq!(solved.region, region);
        assert_eq!(solved.diagnostic.parameter_count, 14);
        assert!(
            distance(
                region.profile.segments[0]
                    .control_point(0, CubicBezier2D::control_point_ref(0, 1).unwrap(),)
                    .unwrap(),
                [7.0, -2.0],
            ) < f64::EPSILON
        );
        assert!(
            distance(
                region.profile.segments[2]
                    .control_point(2, RationalQuadraticBezier2D::control_point_ref(2))
                    .unwrap(),
                [5.0, 14.0],
            ) < f64::EPSILON
        );
        let cubic_midpoint = CubicBezier2D::new([0.0, 0.0], [3.0, -2.0], [7.0, -2.0], [10.0, 0.0])
            .unwrap()
            .evaluate(0.5)
            .unwrap();
        assert!(region.profile.segments[0].distance_squared_to(cubic_midpoint) < 1.0e-20);
    }
}
