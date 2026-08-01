use super::{
    Constraint, ConstraintGeometry, PointId, SketchError, SketchLoop2D, SketchSegment2D,
    validate_construction,
};

/// Kernel-neutral geometry used to place one solved sketch constraint in a
/// viewport. Coordinates remain in the sketch's exact local frame.
#[derive(Debug, Clone, PartialEq)]
pub enum SketchAnnotationGeometry2D {
    Glyph {
        anchors: Vec<[f64; 2]>,
    },
    LinearDimension {
        first: [f64; 2],
        second: [f64; 2],
        /// Constrains the dimension baseline to this local direction. `None`
        /// uses the direction between the two witness points.
        axis: Option<[f64; 2]>,
    },
    AngularDimension {
        center: [f64; 2],
        first_ray: [f64; 2],
        second_ray: [f64; 2],
    },
    RadialDimension {
        center: [f64; 2],
        rim: [f64; 2],
    },
}

/// One ordered user constraint paired with display geometry derived from the
/// committed solved sketch rather than its unsolved input coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchConstraintAnnotation2D {
    pub constraint_index: u32,
    pub constraint: Constraint,
    pub geometry: SketchAnnotationGeometry2D,
}

/// Builds exact, deterministic annotation anchors for a solved sketch.
///
/// # Errors
///
/// Returns the same structured validation failures as sketch solving when the
/// supplied geometry and constraints are not a valid solved snapshot.
pub fn constraint_annotations(
    profile: &SketchLoop2D,
    construction: &[SketchSegment2D],
    constraints: &[Constraint],
) -> Result<Vec<SketchConstraintAnnotation2D>, SketchError> {
    profile.validate()?;
    validate_construction(profile.segments.len(), construction)?;
    let geometry = ConstraintGeometry {
        profile,
        construction,
    };
    constraints
        .iter()
        .enumerate()
        .map(|(index, constraint)| {
            constraint.validate_for_geometry(geometry)?;
            Ok(SketchConstraintAnnotation2D {
                constraint_index: u32::try_from(index).unwrap_or(u32::MAX),
                constraint: constraint.clone(),
                geometry: annotation_geometry(geometry, constraint),
            })
        })
        .collect()
}

fn annotation_geometry(
    geometry: ConstraintGeometry<'_>,
    constraint: &Constraint,
) -> SketchAnnotationGeometry2D {
    match constraint {
        Constraint::Coincident { first, second } => glyph(vec![midpoint(
            geometry.point(*first),
            geometry.point(*second),
        )]),
        Constraint::Horizontal { segment } | Constraint::Vertical { segment } => {
            glyph(vec![geometry.segment(*segment).midpoint()])
        }
        Constraint::Fixed { point, .. } => glyph(vec![geometry.point(*point)]),
        Constraint::Distance { first, second, .. } => {
            linear(geometry.point(*first), geometry.point(*second), None)
        }
        Constraint::HorizontalDistance { first, second, .. } => linear(
            geometry.point(*first),
            geometry.point(*second),
            Some([1.0, 0.0]),
        ),
        Constraint::VerticalDistance { first, second, .. } => linear(
            geometry.point(*first),
            geometry.point(*second),
            Some([0.0, 1.0]),
        ),
        Constraint::PointLineDistance { point, line, .. } => {
            let point = geometry.point(*point);
            let line = geometry.segment(*line);
            linear(
                point,
                project_to_support(point, line.start(), line.end()),
                None,
            )
        }
        Constraint::LineThroughCenter { arc, .. } => {
            glyph(vec![arc_center(geometry.segment(*arc))])
        }
        Constraint::PointOnCurve { point, .. } | Constraint::Midpoint { point, .. } => {
            glyph(vec![geometry.point(*point)])
        }
        Constraint::Symmetric { first, second, .. } => {
            glyph(vec![geometry.point(*first), geometry.point(*second)])
        }
        Constraint::Length { segment, .. } => {
            let segment = geometry.segment(*segment);
            linear(segment.start(), segment.end(), None)
        }
        Constraint::EqualLength { first, second }
        | Constraint::Parallel { first, second }
        | Constraint::EqualRadius { first, second } => glyph(vec![
            geometry.segment(*first).midpoint(),
            geometry.segment(*second).midpoint(),
        ]),
        Constraint::Perpendicular { first, second } => glyph(vec![midpoint(
            geometry.segment(*first).midpoint(),
            geometry.segment(*second).midpoint(),
        )]),
        Constraint::Angle { first, second, .. } => {
            angle_geometry(geometry.segment(*first), geometry.segment(*second))
        }
        Constraint::Radius { segment, .. } => {
            let segment = geometry.segment(*segment);
            SketchAnnotationGeometry2D::RadialDimension {
                center: arc_center(segment),
                rim: segment.midpoint(),
            }
        }
        Constraint::FixedCenter { segment, .. } => {
            glyph(vec![arc_center(geometry.segment(*segment))])
        }
        Constraint::Concentric { first, second } => glyph(vec![midpoint(
            arc_center(geometry.segment(*first)),
            arc_center(geometry.segment(*second)),
        )]),
        Constraint::Tangent { first, second }
        | Constraint::CurvatureContinuous { first, second } => {
            let count = geometry.profile.segments.len();
            let first = usize::try_from(*first).expect("validated profile segment id");
            let second = usize::try_from(*second).expect("validated profile segment id");
            let shared = if (first + 1) % count == second {
                second
            } else {
                first
            };
            glyph(vec![geometry.point(
                PointId::try_from(shared).expect("profile segment limit fits point id"),
            )])
        }
    }
}

fn glyph(anchors: Vec<[f64; 2]>) -> SketchAnnotationGeometry2D {
    SketchAnnotationGeometry2D::Glyph { anchors }
}

fn linear(first: [f64; 2], second: [f64; 2], axis: Option<[f64; 2]>) -> SketchAnnotationGeometry2D {
    SketchAnnotationGeometry2D::LinearDimension {
        first,
        second,
        axis,
    }
}

fn midpoint(first: [f64; 2], second: [f64; 2]) -> [f64; 2] {
    [first[0].midpoint(second[0]), first[1].midpoint(second[1])]
}

fn project_to_support(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> [f64; 2] {
    let direction = [end[0] - start[0], end[1] - start[1]];
    let length_squared = direction[0].mul_add(direction[0], direction[1] * direction[1]);
    let factor = ((point[0] - start[0])
        .mul_add(direction[0], (point[1] - start[1]) * direction[1]))
        / length_squared;
    [
        direction[0].mul_add(factor, start[0]),
        direction[1].mul_add(factor, start[1]),
    ]
}

fn arc_center(segment: &SketchSegment2D) -> [f64; 2] {
    match segment {
        SketchSegment2D::Arc { center, .. } => *center,
        SketchSegment2D::Line { .. }
        | SketchSegment2D::RationalQuadratic { .. }
        | SketchSegment2D::CubicBezier { .. } => unreachable!("validated arc constraint"),
    }
}

fn angle_geometry(first: &SketchSegment2D, second: &SketchSegment2D) -> SketchAnnotationGeometry2D {
    let first_midpoint = first.midpoint();
    let second_midpoint = second.midpoint();
    let center = midpoint(first_midpoint, second_midpoint);
    let ray_length = (first.length().max(second.length()) * 0.35).max(1.0);
    let ray = |segment: &SketchSegment2D| {
        let direction = [
            segment.end()[0] - segment.start()[0],
            segment.end()[1] - segment.start()[1],
        ];
        let length = direction[0].hypot(direction[1]);
        [
            (direction[0] / length).mul_add(ray_length, center[0]),
            (direction[1] / length).mul_add(ray_length, center[1]),
        ]
    };
    SketchAnnotationGeometry2D::AngularDimension {
        center,
        first_ray: ray(first),
        second_ray: ray(second),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> SketchLoop2D {
        SketchLoop2D::from_polygon(vec![[0.0, 0.0], [8.0, 0.0], [8.0, 6.0], [0.0, 6.0]])
    }

    #[test]
    fn dimensions_use_exact_witness_geometry_and_value_domains() {
        let constraints = vec![
            Constraint::HorizontalDistance {
                first: 0,
                second: 2,
                distance: 8.0,
            },
            Constraint::VerticalDistance {
                first: 0,
                second: 2,
                distance: 6.0,
            },
            Constraint::Length {
                segment: 0,
                length: 8.0,
            },
        ];
        let annotations = constraint_annotations(&square(), &[], &constraints).unwrap();
        assert_eq!(annotations.len(), 3);
        assert_eq!(
            annotations[0].geometry,
            SketchAnnotationGeometry2D::LinearDimension {
                first: [0.0, 0.0],
                second: [8.0, 6.0],
                axis: Some([1.0, 0.0]),
            }
        );
        assert!((constraints[0].dimension().unwrap().value - 8.0).abs() < 1.0e-12);
        assert!(constraints[0].with_dimension_value(-3.0).is_some());
        assert!(constraints[2].with_dimension_value(0.0).is_none());
    }

    #[test]
    fn point_line_and_arc_annotations_follow_solved_entities() {
        let construction = vec![
            SketchSegment2D::Line {
                start: [-2.0, 2.0],
                end: [10.0, 2.0],
            },
            SketchSegment2D::Arc {
                start: [5.0, 3.0],
                end: [3.0, 5.0],
                center: [3.0, 3.0],
                ccw: true,
            },
        ];
        let constraints = vec![
            Constraint::PointLineDistance {
                point: 0,
                line: 4,
                distance: 2.0,
            },
            Constraint::Radius {
                segment: 5,
                radius: 2.0,
            },
        ];
        let annotations = constraint_annotations(&square(), &construction, &constraints).unwrap();
        let SketchAnnotationGeometry2D::LinearDimension {
            first,
            second,
            axis,
        } = annotations[0].geometry.clone()
        else {
            panic!("point-line distance must produce a linear dimension");
        };
        assert!(first.into_iter().all(|value| value.abs() < 1.0e-12));
        assert!(second[0].abs() < 1.0e-12);
        assert!((second[1] - 2.0).abs() < 1.0e-12);
        assert_eq!(axis, None);
        assert_eq!(
            annotations[1].geometry,
            SketchAnnotationGeometry2D::RadialDimension {
                center: [3.0, 3.0],
                rim: [4.414_213_562_373_095, 4.414_213_562_373_095],
            }
        );
    }

    #[test]
    fn invalid_references_fail_instead_of_producing_partial_annotations() {
        let error =
            constraint_annotations(&square(), &[], &[Constraint::Horizontal { segment: 99 }])
                .unwrap_err();
        assert_eq!(error, SketchError::InvalidSegment(99));
    }

    #[test]
    fn curvature_continuity_uses_the_shared_vertex_as_its_glyph_witness() {
        let profile = SketchLoop2D {
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
        };
        let annotations = constraint_annotations(
            &profile,
            &[],
            &[Constraint::CurvatureContinuous {
                first: 0,
                second: 1,
            }],
        )
        .unwrap();

        assert_eq!(
            annotations[0].geometry,
            SketchAnnotationGeometry2D::Glyph {
                anchors: vec![[-5.0, 0.0]],
            }
        );
    }
}
