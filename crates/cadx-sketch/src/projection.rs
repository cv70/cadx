//! Deterministic bounded projection solver for line-only constraint sets.

use crate::{PointId, SegmentId, constraints::Constraint};

pub(crate) fn project_constraint(
    points: &mut [[f64; 2]],
    constraint: &Constraint,
    relaxation: f64,
) {
    let point = |id: PointId| usize::try_from(id).expect("validated point id");
    let segment_index = |id: SegmentId| usize::try_from(id).expect("validated segment id");
    match constraint {
        Constraint::Coincident { first, second } => {
            let first = point(*first);
            let second = point(*second);
            let delta = [
                points[second][0] - points[first][0],
                points[second][1] - points[first][1],
            ];
            let correction = [delta[0] * 0.5 * relaxation, delta[1] * 0.5 * relaxation];
            points[first][0] += correction[0];
            points[first][1] += correction[1];
            points[second][0] -= correction[0];
            points[second][1] -= correction[1];
        }
        Constraint::Horizontal { segment } => {
            let start = segment_index(*segment);
            let end = (start + 1) % points.len();
            let correction = (points[end][1] - points[start][1]) * 0.5 * relaxation;
            points[start][1] += correction;
            points[end][1] -= correction;
        }
        Constraint::Vertical { segment } => {
            let start = segment_index(*segment);
            let end = (start + 1) % points.len();
            let correction = (points[end][0] - points[start][0]) * 0.5 * relaxation;
            points[start][0] += correction;
            points[end][0] -= correction;
        }
        Constraint::Fixed { point: id, x, y } => {
            let index = point(*id);
            points[index][0] += (*x - points[index][0]) * relaxation;
            points[index][1] += (*y - points[index][1]) * relaxation;
        }
        Constraint::Distance {
            first,
            second,
            distance,
        } => {
            let first = point(*first);
            let second = point(*second);
            let dx = points[second][0] - points[first][0];
            let dy = points[second][1] - points[first][1];
            let current = (dx * dx + dy * dy).sqrt();
            let (ux, uy) = if current > f64::EPSILON {
                (dx / current, dy / current)
            } else {
                (1.0, 0.0)
            };
            let correction = (distance - current) * 0.5 * relaxation;
            points[first][0] -= ux * correction;
            points[first][1] -= uy * correction;
            points[second][0] += ux * correction;
            points[second][1] += uy * correction;
        }
        Constraint::HorizontalDistance { .. }
        | Constraint::VerticalDistance { .. }
        | Constraint::PointLineDistance { .. }
        | Constraint::LineThroughCenter { .. }
        | Constraint::PointOnCurve { .. }
        | Constraint::Midpoint { .. }
        | Constraint::Symmetric { .. }
        | Constraint::Length { .. }
        | Constraint::EqualLength { .. }
        | Constraint::Parallel { .. }
        | Constraint::Perpendicular { .. }
        | Constraint::Angle { .. }
        | Constraint::Radius { .. }
        | Constraint::FixedCenter { .. }
        | Constraint::EqualRadius { .. }
        | Constraint::Concentric { .. }
        | Constraint::Tangent { .. }
        | Constraint::CurvatureContinuous { .. } => {
            unreachable!("nonlinear constraints bypass the projection solver")
        }
    }
}

pub(crate) fn constraint_residual(points: &[[f64; 2]], constraint: &Constraint) -> f64 {
    let point = |id: PointId| usize::try_from(id).expect("validated point id");
    let segment_index = |id: SegmentId| usize::try_from(id).expect("validated segment id");
    match constraint {
        Constraint::Coincident { first, second } => {
            let first = points[point(*first)];
            let second = points[point(*second)];
            (second[0] - first[0]).hypot(second[1] - first[1])
        }
        Constraint::Horizontal { segment } => {
            let start = segment_index(*segment);
            (points[(start + 1) % points.len()][1] - points[start][1]).abs()
        }
        Constraint::Vertical { segment } => {
            let start = segment_index(*segment);
            (points[(start + 1) % points.len()][0] - points[start][0]).abs()
        }
        Constraint::Fixed { point: id, x, y } => {
            let actual = points[point(*id)];
            (actual[0] - x).hypot(actual[1] - y)
        }
        Constraint::Distance {
            first,
            second,
            distance,
        } => {
            let first = points[point(*first)];
            let second = points[point(*second)];
            ((second[0] - first[0]).hypot(second[1] - first[1]) - distance).abs()
        }
        Constraint::HorizontalDistance { .. }
        | Constraint::VerticalDistance { .. }
        | Constraint::PointLineDistance { .. }
        | Constraint::LineThroughCenter { .. }
        | Constraint::PointOnCurve { .. }
        | Constraint::Midpoint { .. }
        | Constraint::Symmetric { .. }
        | Constraint::Length { .. }
        | Constraint::EqualLength { .. }
        | Constraint::Parallel { .. }
        | Constraint::Perpendicular { .. }
        | Constraint::Angle { .. }
        | Constraint::Radius { .. }
        | Constraint::FixedCenter { .. }
        | Constraint::EqualRadius { .. }
        | Constraint::Concentric { .. }
        | Constraint::Tangent { .. }
        | Constraint::CurvatureContinuous { .. } => {
            unreachable!("nonlinear constraints bypass the projection solver")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SketchError, SolverConfig,
        math::{cross, distance, dot, normalize_vector},
        solve_profile,
    };

    #[test]
    fn solves_a_constrained_rectangle_deterministically() {
        let profile = [[0.0, 0.0], [9.0, 1.0], [10.0, 6.0], [1.0, 5.0]];
        let constraints = vec![
            Constraint::Fixed {
                point: 0,
                x: 0.0,
                y: 0.0,
            },
            Constraint::Horizontal { segment: 0 },
            Constraint::Vertical { segment: 1 },
            Constraint::Horizontal { segment: 2 },
            Constraint::Vertical { segment: 3 },
            Constraint::Distance {
                first: 0,
                second: 1,
                distance: 10.0,
            },
            Constraint::Distance {
                first: 1,
                second: 2,
                distance: 5.0,
            },
        ];
        let first = solve_profile(&profile, &constraints, SolverConfig::default()).unwrap();
        let second = solve_profile(&profile, &constraints, SolverConfig::default()).unwrap();
        assert_eq!(first, second);
        assert!((first.profile[0][0]).abs() < 1.0e-6);
        assert!((first.profile[0][1]).abs() < 1.0e-6);
        assert!((first.profile[1][0] - 10.0).abs() < 1.0e-5);
        assert!((first.profile[0][1] - first.profile[1][1]).abs() < 1.0e-7);
        assert!((first.profile[1][0] - first.profile[2][0]).abs() < 1.0e-7);
        assert!((first.profile[2][1] - first.profile[3][1]).abs() < 1.0e-7);
        assert!((first.profile[3][0] - first.profile[0][0]).abs() < 1.0e-7);
        assert!(
            ((first.profile[0][0] - first.profile[1][0])
                .hypot(first.profile[0][1] - first.profile[1][1])
                - 10.0)
                .abs()
                < 1.0e-7
        );
        assert!(
            first
                .profile
                .iter()
                .all(|point| point.iter().all(|value| value.is_finite()))
        );
    }

    #[test]
    fn solves_a_directed_line_angle_through_the_profile_api() {
        let solved = solve_profile(
            &[[0.0, 0.0], [7.0, 1.0], [3.0, -5.0]],
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Length {
                    segment: 0,
                    length: 8.0,
                },
                Constraint::Length {
                    segment: 1,
                    length: 6.0,
                },
                Constraint::Angle {
                    first: 0,
                    second: 1,
                    angle_degrees: -120.0,
                },
            ],
            SolverConfig::default(),
        )
        .unwrap();
        let first = normalize_vector([
            solved.profile[1][0] - solved.profile[0][0],
            solved.profile[1][1] - solved.profile[0][1],
        ]);
        let second = normalize_vector([
            solved.profile[2][0] - solved.profile[1][0],
            solved.profile[2][1] - solved.profile[1][1],
        ]);
        let angle = cross(first, second).atan2(dot(first, second)).to_degrees();
        assert!((angle + 120.0).abs() < 1.0e-8);
        assert!((distance(solved.profile[0], solved.profile[1]) - 8.0).abs() < 1.0e-8);
        assert!((distance(solved.profile[1], solved.profile[2]) - 6.0).abs() < 1.0e-8);
        assert!(solved.iterations > 0);
    }

    #[test]
    fn rejects_conflicting_constraints_without_returning_partial_geometry() {
        let error = solve_profile(
            &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Fixed {
                    point: 0,
                    x: 10.0,
                    y: 10.0,
                },
            ],
            SolverConfig {
                max_iterations: 8,
                ..SolverConfig::default()
            },
        )
        .unwrap_err();
        assert!(matches!(error, SketchError::ConstraintConflict { .. }));
    }
}
