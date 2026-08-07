//! Parameterized geometry queries used by the nonlinear residual assembly.

use std::f64::consts::TAU;

use crate::{
    ANGULAR_EPSILON, GEOMETRY_EPSILON, SegmentId,
    constraints::{Constraint, ConstraintGeometry},
    curves::{CubicBezier2D, RationalQuadraticBezier2D},
    error::SketchError,
    geometry::{SketchLoop2D, SketchSegment2D},
    math::{
        arc_sweep, closest_point_on_line_segment, closest_point_on_parametric_curve, distance,
        normalize_vector, squared_distance,
    },
    residuals::{SegmentParameterOffsets, rebuild_parameter_segment},
};

pub(crate) fn parameter_segment_points(
    geometry: ConstraintGeometry<'_>,
    parameters: &[f64],
    id: SegmentId,
) -> ([f64; 2], [f64; 2]) {
    let (start, end) = geometry.segment_point_ids(id);
    (
        parameter_point(
            parameters,
            usize::try_from(start).expect("validated point id"),
        ),
        parameter_point(
            parameters,
            usize::try_from(end).expect("validated point id"),
        ),
    )
}

pub(crate) fn closest_parameter_curve_point(
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    id: SegmentId,
    point: [f64; 2],
) -> [f64; 2] {
    let (start, end) = parameter_segment_points(geometry, parameters, id);
    match geometry.segment(id) {
        SketchSegment2D::Line { .. } => closest_point_on_line_segment(point, start, end),
        SketchSegment2D::Arc { ccw, .. } => {
            let center = parameter_center(
                parameters,
                center_offsets,
                usize::try_from(id).expect("validated segment id"),
            );
            let offset = [point[0] - center[0], point[1] - center[1]];
            let point_radius = offset[0].hypot(offset[1]);
            let radius = f64::midpoint(distance(start, center), distance(end, center));
            let radial = if point_radius > GEOMETRY_EPSILON {
                [
                    radius.mul_add(offset[0] / point_radius, center[0]),
                    radius.mul_add(offset[1] / point_radius, center[1]),
                ]
            } else {
                start
            };
            if direction_is_within_arc(start, end, center, *ccw, radial) {
                radial
            } else if squared_distance(point, start) <= squared_distance(point, end) {
                start
            } else {
                end
            }
        }
        SketchSegment2D::RationalQuadratic { .. } | SketchSegment2D::CubicBezier { .. } => {
            let segment = rebuild_parameter_segment(
                geometry,
                center_offsets,
                parameters,
                id,
                geometry.segment(id),
            );
            match segment {
                SketchSegment2D::RationalQuadratic {
                    start,
                    control,
                    end,
                    weight,
                } => RationalQuadraticBezier2D::new(start, control, end, weight)
                    .ok()
                    .and_then(|curve| {
                        closest_point_on_parametric_curve(point, |parameter| {
                            curve.derivatives(parameter)
                        })
                    })
                    .unwrap_or(start),
                SketchSegment2D::CubicBezier {
                    start,
                    control1,
                    control2,
                    end,
                } => CubicBezier2D::new(start, control1, control2, end)
                    .ok()
                    .and_then(|curve| {
                        closest_point_on_parametric_curve(point, |parameter| {
                            curve.derivatives(parameter)
                        })
                    })
                    .unwrap_or(start),
                SketchSegment2D::Line { .. } | SketchSegment2D::Arc { .. } => {
                    unreachable!("rebuilt template retains its Bezier variant")
                }
            }
        }
    }
}

pub(crate) fn direction_is_within_arc(
    start: [f64; 2],
    end: [f64; 2],
    center: [f64; 2],
    ccw: bool,
    point: [f64; 2],
) -> bool {
    let sweep = arc_sweep(start, end, center, ccw);
    let start_angle = (start[1] - center[1]).atan2(start[0] - center[0]);
    let point_angle = (point[1] - center[1]).atan2(point[0] - center[0]);
    let progress = if ccw {
        (point_angle - start_angle).rem_euclid(TAU)
    } else {
        (start_angle - point_angle).rem_euclid(TAU)
    };
    progress <= sweep.abs() + ANGULAR_EPSILON
}

pub(crate) fn validate_finite_curve_constraints(
    profile: &SketchLoop2D,
    construction: &[SketchSegment2D],
    constraints: &[Constraint],
) -> Result<(), SketchError> {
    let geometry = ConstraintGeometry {
        profile,
        construction,
    };
    for constraint in constraints {
        let Constraint::PointOnCurve { point, segment } = constraint else {
            continue;
        };
        let point_index = usize::try_from(*point).expect("validated point id");
        let actual = if point_index < profile.segments.len() {
            profile.segments[point_index].start()
        } else {
            let construction_point = point_index - profile.segments.len();
            let segment = &construction[construction_point / 2];
            if construction_point.is_multiple_of(2) {
                segment.start()
            } else {
                segment.end()
            }
        };
        if geometry.segment(*segment).distance_squared_to(actual) > GEOMETRY_EPSILON.powi(2) {
            return Err(SketchError::PointNotOnCurve {
                point: *point,
                segment: *segment,
            });
        }
    }
    Ok(())
}

pub(crate) fn parameter_point(parameters: &[f64], index: usize) -> [f64; 2] {
    [parameters[index * 2], parameters[index * 2 + 1]]
}

pub(crate) fn parameter_center(
    parameters: &[f64],
    center_offsets: &[SegmentParameterOffsets],
    index: usize,
) -> [f64; 2] {
    let offset = center_offsets[index]
        .center
        .expect("arc segment has a center parameter");
    [parameters[offset], parameters[offset + 1]]
}

pub(crate) fn parameter_control(
    parameters: &[f64],
    segment_offsets: &[SegmentParameterOffsets],
    index: usize,
    control: usize,
) -> [f64; 2] {
    let offset = segment_offsets[index].controls[control]
        .expect("Bezier segment has the requested control parameter");
    [parameters[offset], parameters[offset + 1]]
}

pub(crate) fn shared_vertex(count: usize, first: SegmentId, second: SegmentId) -> Option<usize> {
    let first = usize::try_from(first).ok()?;
    let second = usize::try_from(second).ok()?;
    if (first + 1) % count == second {
        Some(second)
    } else if (second + 1) % count == first {
        Some(first)
    } else {
        None
    }
}

pub(crate) fn constraint_tangent(
    profile: &SketchLoop2D,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    index: usize,
    shared: [f64; 2],
) -> [f64; 2] {
    let segment = parameterized_profile_segment(profile, center_offsets, parameters, index);
    let vector = match &segment {
        SketchSegment2D::Line { .. } => {
            let start = segment.start();
            let end = segment.end();
            [end[0] - start[0], end[1] - start[1]]
        }
        SketchSegment2D::Arc { center, ccw, .. } => {
            let radius = [shared[0] - center[0], shared[1] - center[1]];
            if *ccw {
                [-radius[1], radius[0]]
            } else {
                [radius[1], -radius[0]]
            }
        }
        SketchSegment2D::RationalQuadratic {
            start,
            control,
            end,
            weight,
        } => {
            let parameter = if distance(shared, *start) <= GEOMETRY_EPSILON {
                0.0
            } else {
                1.0
            };
            RationalQuadraticBezier2D::new(*start, *control, *end, *weight)
                .and_then(|curve| curve.derivatives(parameter))
                .map_or([0.0; 2], |derivatives| derivatives.first)
        }
        SketchSegment2D::CubicBezier {
            start,
            control1,
            control2,
            end,
        } => {
            let parameter = if distance(shared, *start) <= GEOMETRY_EPSILON {
                0.0
            } else {
                1.0
            };
            CubicBezier2D::new(*start, *control1, *control2, *end)
                .and_then(|curve| curve.derivatives(parameter))
                .map_or([0.0; 2], |derivatives| derivatives.first)
        }
    };
    normalize_vector(vector)
}

pub(crate) fn parameterized_profile_segment(
    profile: &SketchLoop2D,
    segment_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    index: usize,
) -> SketchSegment2D {
    let start = parameter_point(parameters, index);
    let end = parameter_point(parameters, (index + 1) % profile.segments.len());
    match &profile.segments[index] {
        SketchSegment2D::Line { .. } => SketchSegment2D::Line { start, end },
        SketchSegment2D::Arc { ccw, .. } => SketchSegment2D::Arc {
            start,
            end,
            center: parameter_center(parameters, segment_offsets, index),
            ccw: *ccw,
        },
        SketchSegment2D::RationalQuadratic { weight, .. } => SketchSegment2D::RationalQuadratic {
            start,
            control: parameter_control(parameters, segment_offsets, index, 0),
            end,
            weight: *weight,
        },
        SketchSegment2D::CubicBezier { .. } => SketchSegment2D::CubicBezier {
            start,
            control1: parameter_control(parameters, segment_offsets, index, 0),
            control2: parameter_control(parameters, segment_offsets, index, 1),
            end,
        },
    }
}

pub(crate) fn segment_curvature_at_shared(segment: &SketchSegment2D, shared: [f64; 2]) -> f64 {
    let parameter = if distance(shared, segment.start()) <= GEOMETRY_EPSILON {
        0.0
    } else {
        1.0
    };
    match segment {
        SketchSegment2D::Arc {
            start, center, ccw, ..
        } => {
            let sign = if *ccw { 1.0 } else { -1.0 };
            sign / distance(*start, *center)
        }
        SketchSegment2D::RationalQuadratic {
            start,
            control,
            end,
            weight,
        } => RationalQuadraticBezier2D::new(*start, *control, *end, *weight)
            .and_then(|curve| curve.signed_curvature(parameter))
            .unwrap_or(f64::NAN),
        SketchSegment2D::CubicBezier {
            start,
            control1,
            control2,
            end,
        } => CubicBezier2D::new(*start, *control1, *control2, *end)
            .and_then(|curve| curve.signed_curvature(parameter))
            .unwrap_or(f64::NAN),
        SketchSegment2D::Line { .. } => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SketchRegion2D, SolverConfig,
        math::{cross, dot},
        solve_region, solve_sketch,
    };

    #[test]
    fn point_on_curve_uses_a_finite_arc_and_rejects_an_infinite_extension() {
        let region = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [12.0, 0.0], [12.0, 8.0], [0.0, 8.0]],
            Vec::new(),
        );
        let arc_construction = vec![
            SketchSegment2D::Arc {
                start: [5.0, 0.0],
                end: [-5.0, 0.0],
                center: [0.0, 0.0],
                ccw: true,
            },
            SketchSegment2D::Line {
                start: [1.0, 7.0],
                end: [1.0, 10.0],
            },
        ];
        let solved = solve_sketch(
            &region,
            &arc_construction,
            &[
                Constraint::Fixed {
                    point: 4,
                    x: 5.0,
                    y: 0.0,
                },
                Constraint::Fixed {
                    point: 5,
                    x: -5.0,
                    y: 0.0,
                },
                Constraint::FixedCenter {
                    segment: 4,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Radius {
                    segment: 4,
                    radius: 5.0,
                },
                Constraint::Fixed {
                    point: 7,
                    x: 1.0,
                    y: 10.0,
                },
                Constraint::PointOnCurve {
                    point: 6,
                    segment: 4,
                },
            ],
            SolverConfig::default(),
        )
        .unwrap();
        let point = solved.construction[1].start();
        assert!((point[0].hypot(point[1]) - 5.0).abs() < 1.0e-7);
        assert!(point[1] > 0.0);

        let line_construction = vec![
            SketchSegment2D::Line {
                start: [0.0, 0.0],
                end: [1.0, 0.0],
            },
            SketchSegment2D::Line {
                start: [2.0, 0.0],
                end: [2.0, 1.0],
            },
        ];
        let result = solve_sketch(
            &region,
            &line_construction,
            &[
                Constraint::Fixed {
                    point: 4,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Fixed {
                    point: 5,
                    x: 1.0,
                    y: 0.0,
                },
                Constraint::Fixed {
                    point: 6,
                    x: 2.0,
                    y: 0.0,
                },
                Constraint::PointOnCurve {
                    point: 6,
                    segment: 4,
                },
            ],
            SolverConfig::default(),
        );
        assert!(matches!(
            result,
            Err(SketchError::ConstraintConflict { .. })
        ));
    }

    #[test]
    fn solves_line_arc_tangency_at_their_shared_vertex() {
        let region = SketchRegion2D {
            profile: SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Line {
                        start: [0.0, 0.0],
                        end: [10.0, 1.0],
                    },
                    SketchSegment2D::Arc {
                        start: [10.0, 1.0],
                        end: [10.0, 9.0],
                        center: [10.0, 5.0],
                        ccw: true,
                    },
                    SketchSegment2D::Line {
                        start: [10.0, 9.0],
                        end: [0.0, 10.0],
                    },
                    SketchSegment2D::Line {
                        start: [0.0, 10.0],
                        end: [0.0, 0.0],
                    },
                ],
            },
            holes: Vec::new(),
        };
        let solved = solve_region(
            &region,
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 1.0,
                },
                Constraint::FixedCenter {
                    segment: 1,
                    x: 10.0,
                    y: 5.0,
                },
                Constraint::Radius {
                    segment: 1,
                    radius: 4.0,
                },
                Constraint::Tangent {
                    first: 0,
                    second: 1,
                },
            ],
            SolverConfig::default(),
        )
        .unwrap();
        let line = &solved.profile.segments[0];
        let arc = &solved.profile.segments[1];
        let line_direction = [
            line.end()[0] - line.start()[0],
            line.end()[1] - line.start()[1],
        ];
        let SketchSegment2D::Arc { center, start, .. } = arc else {
            panic!("expected arc");
        };
        let radius = [start[0] - center[0], start[1] - center[1]];
        assert!(dot(line_direction, radius).abs() < 1.0e-7);
        solved.validate().unwrap();
    }

    #[test]
    fn solves_counterclockwise_to_clockwise_arc_tangency() {
        let region = SketchRegion2D {
            profile: SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Arc {
                        start: [0.0, 0.0],
                        end: [2.2, 2.0],
                        center: [21.0 / 110.0, 2.0],
                        ccw: true,
                    },
                    SketchSegment2D::Arc {
                        start: [2.2, 2.0],
                        end: [4.0, 4.0],
                        center: [4.0, 2.19],
                        ccw: false,
                    },
                    SketchSegment2D::Line {
                        start: [4.0, 4.0],
                        end: [0.0, 6.0],
                    },
                    SketchSegment2D::Line {
                        start: [0.0, 6.0],
                        end: [0.0, 0.0],
                    },
                ],
            },
            holes: Vec::new(),
        };
        let solved = solve_region(
            &region,
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Fixed {
                    point: 2,
                    x: 4.0,
                    y: 4.0,
                },
                Constraint::FixedCenter {
                    segment: 0,
                    x: 0.0,
                    y: 2.0,
                },
                Constraint::FixedCenter {
                    segment: 1,
                    x: 4.0,
                    y: 2.0,
                },
                Constraint::Tangent {
                    first: 0,
                    second: 1,
                },
            ],
            SolverConfig::default(),
        )
        .unwrap();
        let shared = solved.profile.segments[0].end();
        assert!((shared[0] - 2.0).abs() < 1.0e-8);
        assert!((shared[1] - 2.0).abs() < 1.0e-8);
        let SketchSegment2D::Arc {
            center: first_center,
            ..
        } = &solved.profile.segments[0]
        else {
            panic!("expected arc");
        };
        let SketchSegment2D::Arc {
            center: second_center,
            ..
        } = &solved.profile.segments[1]
        else {
            panic!("expected arc");
        };
        let first_radius = [shared[0] - first_center[0], shared[1] - first_center[1]];
        let second_radius = [shared[0] - second_center[0], shared[1] - second_center[1]];
        assert!(cross(first_radius, second_radius).abs() < 1.0e-8);
        solved.validate().unwrap();
    }

    #[test]
    fn solves_same_direction_tangent_and_signed_curvature_continuity() {
        let region = SketchRegion2D {
            profile: SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Arc {
                        start: [0.0, 0.0],
                        end: [2.0, 2.0],
                        center: [0.0, 2.0],
                        ccw: true,
                    },
                    SketchSegment2D::Arc {
                        start: [2.0, 2.0],
                        end: [0.0, 4.0],
                        center: [0.1, 2.1],
                        ccw: true,
                    },
                    SketchSegment2D::Line {
                        start: [0.0, 4.0],
                        end: [-2.0, 2.0],
                    },
                    SketchSegment2D::Line {
                        start: [-2.0, 2.0],
                        end: [0.0, 0.0],
                    },
                ],
            },
            holes: Vec::new(),
        };
        let solved = solve_sketch(
            &region,
            &[],
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Fixed {
                    point: 1,
                    x: 2.0,
                    y: 2.0,
                },
                Constraint::Fixed {
                    point: 2,
                    x: 0.0,
                    y: 4.0,
                },
                Constraint::Fixed {
                    point: 3,
                    x: -2.0,
                    y: 2.0,
                },
                Constraint::CurvatureContinuous {
                    first: 0,
                    second: 1,
                },
            ],
            SolverConfig::default(),
        )
        .unwrap();

        let SketchSegment2D::Arc {
            center: first_center,
            ..
        } = solved.region.profile.segments[0]
        else {
            panic!("expected arc");
        };
        let SketchSegment2D::Arc {
            center: second_center,
            ..
        } = solved.region.profile.segments[1]
        else {
            panic!("expected arc");
        };
        assert!(distance(first_center, second_center) < 1.0e-7);
        assert_eq!(solved.diagnostic.equation_count, 12);
        assert_eq!(solved.diagnostic.degrees_of_freedom, 0);
        assert!(solved.diagnostic.redundant_constraints.is_empty());
        solved.region.validate().unwrap();
    }

    #[test]
    fn curvature_continuity_rejects_opposite_signed_curvature() {
        let region = SketchRegion2D {
            profile: SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Arc {
                        start: [0.0, 0.0],
                        end: [2.0, 2.0],
                        center: [0.0, 2.0],
                        ccw: true,
                    },
                    SketchSegment2D::Arc {
                        start: [2.0, 2.0],
                        end: [4.0, 4.0],
                        center: [4.0, 2.0],
                        ccw: false,
                    },
                    SketchSegment2D::Line {
                        start: [4.0, 4.0],
                        end: [0.0, 4.0],
                    },
                    SketchSegment2D::Line {
                        start: [0.0, 4.0],
                        end: [0.0, 0.0],
                    },
                ],
            },
            holes: Vec::new(),
        };
        let result = solve_sketch(
            &region,
            &[],
            &[Constraint::CurvatureContinuous {
                first: 0,
                second: 1,
            }],
            SolverConfig::default(),
        );
        assert!(
            matches!(
                &result,
            Err(SketchError::ConstraintConflict {
                constraints,
                ..
            }) if *constraints == [0]
            ),
            "unexpected solve result: {result:?}"
        );
    }
}
