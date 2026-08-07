//! Entity-reference and value validation for persistent sketch constraints.

use crate::{
    PointId, SegmentId,
    constraints::{Constraint, ConstraintGeometry},
    error::SketchError,
};

impl Constraint {
    pub(crate) fn validate(
        &self,
        point_count: usize,
        segment_count: usize,
    ) -> Result<(), SketchError> {
        let point = |id: PointId| {
            (usize::try_from(id).ok().is_some_and(|id| id < point_count))
                .then_some(())
                .ok_or(SketchError::InvalidPoint(id))
        };
        let segment_index = |id: SegmentId| {
            (usize::try_from(id)
                .ok()
                .is_some_and(|id| id < segment_count))
            .then_some(())
            .ok_or(SketchError::InvalidSegment(id))
        };
        match self {
            Self::Coincident { first, second } => {
                point(*first)?;
                point(*second)
            }
            Self::Horizontal { segment } | Self::Vertical { segment } => segment_index(*segment),
            Self::Fixed { point: id, x, y } => {
                point(*id)?;
                if x.is_finite() && y.is_finite() {
                    Ok(())
                } else {
                    Err(SketchError::NonFiniteValue)
                }
            }
            Self::Distance {
                first,
                second,
                distance,
            } => {
                point(*first)?;
                point(*second)?;
                if distance.is_finite() && *distance >= 0.0 {
                    Ok(())
                } else {
                    Err(SketchError::InvalidDistance(*distance))
                }
            }
            Self::HorizontalDistance {
                first,
                second,
                distance,
            }
            | Self::VerticalDistance {
                first,
                second,
                distance,
            } => {
                point(*first)?;
                point(*second)?;
                if first == second {
                    return Err(SketchError::IdenticalPoints {
                        first: *first,
                        second: *second,
                    });
                }
                if distance.is_finite() {
                    Ok(())
                } else {
                    Err(SketchError::NonFiniteValue)
                }
            }
            Self::PointLineDistance {
                point: id,
                line,
                distance,
            } => {
                point(*id)?;
                segment_index(*line)?;
                if distance.is_finite() && *distance >= 0.0 {
                    Ok(())
                } else {
                    Err(SketchError::InvalidDistance(*distance))
                }
            }
            Self::LineThroughCenter { line, arc } => {
                segment_index(*line)?;
                segment_index(*arc)
            }
            Self::PointOnCurve { point: id, segment } | Self::Midpoint { point: id, segment } => {
                point(*id)?;
                segment_index(*segment)
            }
            Self::Symmetric {
                first,
                second,
                axis,
            } => {
                point(*first)?;
                point(*second)?;
                segment_index(*axis)?;
                if first == second {
                    Err(SketchError::IdenticalPoints {
                        first: *first,
                        second: *second,
                    })
                } else {
                    Ok(())
                }
            }
            Self::Length { segment, length } => {
                segment_index(*segment)?;
                if length.is_finite() && *length > 0.0 {
                    Ok(())
                } else {
                    Err(SketchError::InvalidLength(*length))
                }
            }
            Self::EqualLength { first, second }
            | Self::Parallel { first, second }
            | Self::Perpendicular { first, second }
            | Self::EqualRadius { first, second }
            | Self::Concentric { first, second }
            | Self::Tangent { first, second }
            | Self::CurvatureContinuous { first, second } => {
                segment_index(*first)?;
                segment_index(*second)
            }
            Self::Angle {
                first,
                second,
                angle_degrees,
            } => {
                segment_index(*first)?;
                segment_index(*second)?;
                if angle_degrees.is_finite() && (-180.0..=180.0).contains(angle_degrees) {
                    Ok(())
                } else {
                    Err(SketchError::InvalidAngle(*angle_degrees))
                }
            }
            Self::Radius { segment, radius } => {
                segment_index(*segment)?;
                if radius.is_finite() && *radius > 0.0 {
                    Ok(())
                } else {
                    Err(SketchError::InvalidRadius(*radius))
                }
            }
            Self::FixedCenter { segment, x, y } => {
                segment_index(*segment)?;
                if x.is_finite() && y.is_finite() {
                    Ok(())
                } else {
                    Err(SketchError::NonFiniteValue)
                }
            }
        }
    }

    pub(crate) fn validate_for_geometry(
        &self,
        geometry: ConstraintGeometry<'_>,
    ) -> Result<(), SketchError> {
        self.validate(geometry.point_count(), geometry.segment_count())?;
        let segment = |id: SegmentId| geometry.segment(id);
        match self {
            Self::Horizontal { segment: id }
            | Self::Vertical { segment: id }
            | Self::Length { segment: id, .. }
                if !segment(*id).is_line() =>
            {
                Err(SketchError::InvalidConstraintEntity {
                    segment: *id,
                    expected: "line",
                })
            }
            Self::PointLineDistance { line, .. } if !segment(*line).is_line() => {
                Err(SketchError::InvalidConstraintEntity {
                    segment: *line,
                    expected: "line",
                })
            }
            Self::LineThroughCenter { line, arc } => {
                if !segment(*line).is_line() {
                    return Err(SketchError::InvalidConstraintEntity {
                        segment: *line,
                        expected: "line",
                    });
                }
                if !segment(*arc).is_arc() {
                    return Err(SketchError::InvalidConstraintEntity {
                        segment: *arc,
                        expected: "arc",
                    });
                }
                Ok(())
            }
            Self::Midpoint { point, segment: id } => {
                if !segment(*id).is_line() {
                    return Err(SketchError::InvalidConstraintEntity {
                        segment: *id,
                        expected: "line",
                    });
                }
                let (start, end) = geometry.segment_point_ids(*id);
                if *point == start || *point == end {
                    return Err(SketchError::PointIsSegmentEndpoint {
                        point: *point,
                        segment: *id,
                    });
                }
                Ok(())
            }
            Self::Symmetric { axis, .. } if !segment(*axis).is_line() => {
                Err(SketchError::InvalidConstraintEntity {
                    segment: *axis,
                    expected: "line",
                })
            }
            Self::EqualLength { first, second }
            | Self::Parallel { first, second }
            | Self::Perpendicular { first, second }
            | Self::Angle { first, second, .. } => {
                if !segment(*first).is_line() || !segment(*second).is_line() {
                    let invalid = if segment(*first).is_line() {
                        *second
                    } else {
                        *first
                    };
                    return Err(SketchError::InvalidConstraintEntity {
                        segment: invalid,
                        expected: "line",
                    });
                }
                if first == second {
                    return Err(SketchError::IdenticalSegments {
                        first: *first,
                        second: *second,
                    });
                }
                Ok(())
            }
            Self::Radius { segment: id, .. } | Self::FixedCenter { segment: id, .. }
                if !segment(*id).is_arc() =>
            {
                Err(SketchError::InvalidConstraintEntity {
                    segment: *id,
                    expected: "arc",
                })
            }
            Self::EqualRadius { first, second } | Self::Concentric { first, second }
                if !segment(*first).is_arc() || !segment(*second).is_arc() =>
            {
                let invalid = if segment(*first).is_arc() {
                    *second
                } else {
                    *first
                };
                Err(SketchError::InvalidConstraintEntity {
                    segment: invalid,
                    expected: "arc",
                })
            }
            Self::Tangent { first, second } | Self::CurvatureContinuous { first, second } => {
                let count = geometry.profile.segments.len();
                let first_index = usize::try_from(*first).expect("validated segment id");
                let second_index = usize::try_from(*second).expect("validated segment id");
                if first_index >= count
                    || second_index >= count
                    || first == second
                    || ((first_index + 1) % count != second_index
                        && (second_index + 1) % count != first_index)
                {
                    return Err(SketchError::NonAdjacentSegments {
                        first: *first,
                        second: *second,
                    });
                }
                if matches!(self, Self::CurvatureContinuous { .. })
                    && (!segment(*first).is_curved() || !segment(*second).is_curved())
                {
                    let invalid = if segment(*first).is_curved() {
                        *second
                    } else {
                        *first
                    };
                    return Err(SketchError::InvalidConstraintEntity {
                        segment: invalid,
                        expected: "curve",
                    });
                }
                if matches!(self, Self::Tangent { .. })
                    && segment(*first).is_line()
                    && segment(*second).is_line()
                {
                    return Err(SketchError::InvalidConstraintEntity {
                        segment: *first,
                        expected: "at least one curve",
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MAX_CONSTRUCTION_SEGMENTS, SketchRegion2D, SolverConfig,
        geometry::{SketchLoop2D, SketchSegment2D},
        solve_profile, solve_region, solve_sketch,
    };

    #[test]
    fn advanced_line_constraints_validate_values_entities_and_conflicts() {
        let region = SketchRegion2D {
            profile: SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Line {
                        start: [0.0, 0.0],
                        end: [4.0, 0.0],
                    },
                    SketchSegment2D::Arc {
                        start: [4.0, 0.0],
                        end: [4.0, 4.0],
                        center: [4.0, 2.0],
                        ccw: true,
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
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Length {
                    segment: 1,
                    length: 2.0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 1,
                expected: "line"
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Parallel {
                    first: 0,
                    second: 1,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 1,
                expected: "line"
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::EqualLength {
                    first: 0,
                    second: 0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::IdenticalSegments {
                first: 0,
                second: 0
            })
        ));
        for length in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                solve_region(
                    &region,
                    &[Constraint::Length {
                        segment: 0,
                        length,
                    }],
                    SolverConfig::default(),
                ),
                Err(SketchError::InvalidLength(value)) if value.to_bits() == length.to_bits()
            ));
        }
        for angle_degrees in [-180.1, 180.1, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                solve_region(
                    &region,
                    &[Constraint::Angle {
                        first: 0,
                        second: 2,
                        angle_degrees,
                    }],
                    SolverConfig::default(),
                ),
                Err(SketchError::InvalidAngle(value))
                    if value.to_bits() == angle_degrees.to_bits()
            ));
        }

        let rectangle = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
            Vec::new(),
        );
        assert!(matches!(
            solve_region(
                &rectangle,
                &[
                    Constraint::Length {
                        segment: 0,
                        length: 4.0,
                    },
                    Constraint::Length {
                        segment: 0,
                        length: 5.0,
                    },
                ],
                SolverConfig {
                    max_iterations: 64,
                    ..SolverConfig::default()
                },
            ),
            Err(SketchError::ConstraintConflict { .. })
        ));
    }

    #[test]
    fn point_relationships_validate_entities_and_degenerate_references() {
        let region = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]],
            Vec::new(),
        );
        let construction = vec![SketchSegment2D::Arc {
            start: [5.0, 0.0],
            end: [-5.0, 0.0],
            center: [0.0, 0.0],
            ccw: true,
        }];
        assert!(matches!(
            solve_sketch(
                &region,
                &construction,
                &[Constraint::Midpoint {
                    point: 0,
                    segment: 4,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 4,
                expected: "line",
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Midpoint {
                    point: 0,
                    segment: 0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::PointIsSegmentEndpoint {
                point: 0,
                segment: 0,
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Symmetric {
                    first: 2,
                    second: 2,
                    axis: 0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::IdenticalPoints {
                first: 2,
                second: 2,
            })
        ));
        assert!(matches!(
            solve_sketch(
                &region,
                &vec![
                    SketchSegment2D::Line {
                        start: [0.0, 0.0],
                        end: [1.0, 0.0],
                    };
                    MAX_CONSTRUCTION_SEGMENTS + 1
                ],
                &[],
                SolverConfig::default(),
            ),
            Err(SketchError::TooManyConstructionSegments(129))
        ));
    }

    #[test]
    fn validates_entity_references_and_dimension_values() {
        assert!(matches!(
            solve_profile(
                &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
                &[Constraint::Horizontal { segment: 5 }],
                SolverConfig::default()
            ),
            Err(SketchError::InvalidSegment(5))
        ));
        assert!(matches!(
            solve_profile(
                &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
                &[Constraint::Distance {
                    first: 0,
                    second: 1,
                    distance: -1.0
                }],
                SolverConfig::default()
            ),
            Err(SketchError::InvalidDistance(_))
        ));
    }

    #[test]
    fn point_dimensions_validate_values_and_line_arc_roles() {
        let region = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]],
            Vec::new(),
        );
        let construction = [SketchSegment2D::Arc {
            start: [8.0, 2.0],
            end: [4.0, 2.0],
            center: [6.0, 0.0],
            ccw: true,
        }];
        assert!(matches!(
            solve_sketch(
                &region,
                &construction,
                &[Constraint::HorizontalDistance {
                    first: 0,
                    second: 0,
                    distance: 1.0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::IdenticalPoints {
                first: 0,
                second: 0
            })
        ));
        assert!(matches!(
            solve_sketch(
                &region,
                &construction,
                &[Constraint::VerticalDistance {
                    first: 0,
                    second: 1,
                    distance: f64::NAN,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::NonFiniteValue)
        ));
        assert!(matches!(
            solve_sketch(
                &region,
                &construction,
                &[Constraint::PointLineDistance {
                    point: 0,
                    line: 4,
                    distance: 1.0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 4,
                expected: "line"
            })
        ));
        assert!(matches!(
            solve_sketch(
                &region,
                &construction,
                &[Constraint::PointLineDistance {
                    point: 0,
                    line: 0,
                    distance: -1.0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidDistance(value)) if value.to_bits() == (-1.0_f64).to_bits()
        ));
        assert!(matches!(
            solve_sketch(
                &region,
                &construction,
                &[Constraint::LineThroughCenter { line: 4, arc: 0 }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 4,
                expected: "line"
            })
        ));
        assert!(matches!(
            solve_sketch(
                &region,
                &construction,
                &[Constraint::LineThroughCenter { line: 0, arc: 1 }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 1,
                expected: "arc"
            })
        ));
    }

    #[test]
    fn curved_constraints_validate_entity_kinds_and_adjacency() {
        let region = SketchRegion2D {
            profile: SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Line {
                        start: [0.0, 0.0],
                        end: [4.0, 0.0],
                    },
                    SketchSegment2D::Arc {
                        start: [4.0, 0.0],
                        end: [4.0, 4.0],
                        center: [4.0, 2.0],
                        ccw: true,
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
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Radius {
                    segment: 0,
                    radius: 2.0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity { .. })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Tangent {
                    first: 1,
                    second: 3,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::NonAdjacentSegments { .. })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Horizontal { segment: 1 }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 1,
                expected: "line"
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::CurvatureContinuous {
                    first: 0,
                    second: 1,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 0,
                expected: "curve"
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::CurvatureContinuous {
                    first: 1,
                    second: 3,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::NonAdjacentSegments {
                first: 1,
                second: 3
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::EqualRadius {
                    first: 0,
                    second: 1,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 0,
                expected: "arc"
            })
        ));
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::Tangent {
                    first: 2,
                    second: 3,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::InvalidConstraintEntity {
                segment: 2,
                expected: "at least one curve"
            })
        ));
        for radius in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                solve_region(
                    &region,
                    &[Constraint::Radius { segment: 1, radius }],
                    SolverConfig::default(),
                ),
                Err(SketchError::InvalidRadius(value)) if value.to_bits() == radius.to_bits()
            ));
        }
        assert!(matches!(
            solve_region(
                &region,
                &[Constraint::FixedCenter {
                    segment: 1,
                    x: f64::NAN,
                    y: 0.0,
                }],
                SolverConfig::default(),
            ),
            Err(SketchError::NonFiniteValue)
        ));
    }
}
