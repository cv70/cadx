//! Exact sketch segment, loop, and region data types and their validation.

use serde::{Deserialize, Deserializer, Serialize};
use std::f64::consts::PI;

use crate::{
    CURVE_SAMPLING_OPTIONS, GEOMETRY_EPSILON, SegmentId,
    curves::{CubicBezier2D, CurveError, RationalQuadraticBezier2D, SketchControlPointRef},
    error::SketchGeometryError,
    intersections::{
        arc_contains_point, forbidden_intersection, loops_intersect, point_on_segment,
        ray_crossings, segment_bezier_pieces,
    },
    math::{
        arc_sweep, closest_point_on_parametric_curve, distance, point_segment_distance_squared,
        sampled_curve_length, squared_distance,
    },
};

/// One exact segment in a two-dimensional sketch loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SketchSegment2D {
    Line {
        start: [f64; 2],
        end: [f64; 2],
    },
    Arc {
        start: [f64; 2],
        end: [f64; 2],
        center: [f64; 2],
        ccw: bool,
    },
    RationalQuadratic {
        start: [f64; 2],
        control: [f64; 2],
        end: [f64; 2],
        weight: f64,
    },
    CubicBezier {
        start: [f64; 2],
        control1: [f64; 2],
        control2: [f64; 2],
        end: [f64; 2],
    },
}

impl SketchSegment2D {
    #[must_use]
    pub const fn start(&self) -> [f64; 2] {
        match self {
            Self::Line { start, .. }
            | Self::Arc { start, .. }
            | Self::RationalQuadratic { start, .. }
            | Self::CubicBezier { start, .. } => *start,
        }
    }

    #[must_use]
    pub const fn end(&self) -> [f64; 2] {
        match self {
            Self::Line { end, .. }
            | Self::Arc { end, .. }
            | Self::RationalQuadratic { end, .. }
            | Self::CubicBezier { end, .. } => *end,
        }
    }

    #[must_use]
    pub const fn is_line(&self) -> bool {
        matches!(self, Self::Line { .. })
    }

    #[must_use]
    pub const fn is_arc(&self) -> bool {
        matches!(self, Self::Arc { .. })
    }

    #[must_use]
    pub const fn is_curved(&self) -> bool {
        !self.is_line()
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        match self {
            Self::Line { start, end } => distance(*start, *end),
            Self::Arc {
                start,
                end,
                center,
                ccw,
            } => distance(*start, *center) * arc_sweep(*start, *end, *center, *ccw).abs(),
            Self::RationalQuadratic {
                start,
                control,
                end,
                weight,
            } => RationalQuadraticBezier2D::new(*start, *control, *end, *weight)
                .and_then(|curve| curve.sample_adaptive(CURVE_SAMPLING_OPTIONS))
                .map_or(f64::NAN, |points| sampled_curve_length(&points)),
            Self::CubicBezier {
                start,
                control1,
                control2,
                end,
            } => CubicBezier2D::new(*start, *control1, *control2, *end)
                .and_then(|curve| curve.sample_adaptive(CURVE_SAMPLING_OPTIONS))
                .map_or(f64::NAN, |points| sampled_curve_length(&points)),
        }
    }

    #[must_use]
    pub fn midpoint(&self) -> [f64; 2] {
        match self {
            Self::Line { start, end } => [start[0].midpoint(end[0]), start[1].midpoint(end[1])],
            Self::Arc {
                start,
                end,
                center,
                ccw,
            } => {
                let sweep = arc_sweep(*start, *end, *center, *ccw);
                let radius = distance(*start, *center);
                let angle = (start[1] - center[1]).atan2(start[0] - center[0]) + sweep / 2.0;
                [
                    radius.mul_add(angle.cos(), center[0]),
                    radius.mul_add(angle.sin(), center[1]),
                ]
            }
            Self::RationalQuadratic {
                start,
                control,
                end,
                weight,
            } => RationalQuadraticBezier2D::new(*start, *control, *end, *weight)
                .and_then(|curve| curve.evaluate(0.5))
                .unwrap_or([f64::NAN; 2]),
            Self::CubicBezier {
                start,
                control1,
                control2,
                end,
            } => CubicBezier2D::new(*start, *control1, *control2, *end)
                .and_then(|curve| curve.evaluate(0.5))
                .unwrap_or([f64::NAN; 2]),
        }
    }

    /// Returns points in traversal order, including both endpoints.
    #[must_use]
    pub fn sampled_points(&self, max_angle: f64) -> Vec<[f64; 2]> {
        match self {
            Self::Line { start, end } => vec![*start, *end],
            Self::Arc {
                start,
                end,
                center,
                ccw,
            } => {
                let sweep = arc_sweep(*start, *end, *center, *ccw);
                let radius = distance(*start, *center);
                let max_angle = max_angle.clamp(PI / 180.0, PI);
                let mut divisions = 1_u32;
                while f64::from(divisions) * max_angle < sweep.abs() && divisions < 360 {
                    divisions += 1;
                }
                let start_angle = (start[1] - center[1]).atan2(start[0] - center[0]);
                (0..=divisions)
                    .map(|index| {
                        if index == divisions {
                            *end
                        } else {
                            let factor = f64::from(index) / f64::from(divisions);
                            let angle = sweep.mul_add(factor, start_angle);
                            [
                                radius.mul_add(angle.cos(), center[0]),
                                radius.mul_add(angle.sin(), center[1]),
                            ]
                        }
                    })
                    .collect()
            }
            Self::RationalQuadratic {
                start,
                control,
                end,
                weight,
            } => RationalQuadraticBezier2D::new(*start, *control, *end, *weight)
                .and_then(|curve| curve.sample_adaptive(CURVE_SAMPLING_OPTIONS))
                .unwrap_or_default(),
            Self::CubicBezier {
                start,
                control1,
                control2,
                end,
            } => CubicBezier2D::new(*start, *control1, *control2, *end)
                .and_then(|curve| curve.sample_adaptive(CURVE_SAMPLING_OPTIONS))
                .unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn distance_squared_to(&self, point: [f64; 2]) -> f64 {
        match self {
            Self::Line { start, end } => point_segment_distance_squared(point, *start, *end),
            Self::Arc {
                start,
                end,
                center,
                ccw,
            } => {
                let offset = [point[0] - center[0], point[1] - center[1]];
                let point_radius = offset[0].hypot(offset[1]);
                let radius = distance(*start, *center);
                let radial = if point_radius > GEOMETRY_EPSILON {
                    [
                        radius.mul_add(offset[0] / point_radius, center[0]),
                        radius.mul_add(offset[1] / point_radius, center[1]),
                    ]
                } else {
                    *start
                };
                if arc_contains_point(*start, *end, *center, *ccw, radial, false) {
                    (point_radius - radius).powi(2)
                } else {
                    squared_distance(point, *start).min(squared_distance(point, *end))
                }
            }
            Self::RationalQuadratic {
                start,
                control,
                end,
                weight,
            } => RationalQuadraticBezier2D::new(*start, *control, *end, *weight)
                .ok()
                .and_then(|curve| {
                    closest_point_on_parametric_curve(point, |parameter| {
                        curve.derivatives(parameter)
                    })
                })
                .map_or(f64::INFINITY, |closest| squared_distance(point, closest)),
            Self::CubicBezier {
                start,
                control1,
                control2,
                end,
            } => CubicBezier2D::new(*start, *control1, *control2, *end)
                .ok()
                .and_then(|curve| {
                    closest_point_on_parametric_curve(point, |parameter| {
                        curve.derivatives(parameter)
                    })
                })
                .map_or(f64::INFINITY, |closest| squared_distance(point, closest)),
        }
    }

    /// Resolves a segment-local internal control reference without mixing it
    /// into the persistent endpoint id namespace.
    ///
    /// # Errors
    ///
    /// Returns a structured curve error when the owner or control slot does
    /// not match this segment.
    pub fn control_point(
        &self,
        segment: SegmentId,
        reference: SketchControlPointRef,
    ) -> Result<[f64; 2], CurveError> {
        match self {
            Self::RationalQuadratic {
                start,
                control,
                end,
                weight,
            } => RationalQuadraticBezier2D::new(*start, *control, *end, *weight)?
                .control_point(segment, reference),
            Self::CubicBezier {
                start,
                control1,
                control2,
                end,
            } => CubicBezier2D::new(*start, *control1, *control2, *end)?
                .control_point(segment, reference),
            Self::Line { .. } | Self::Arc { .. } => Err(CurveError::InvalidControlSlot {
                control: reference.control,
                control_count: 0,
            }),
        }
    }

    pub(crate) fn validate(&self, index: usize) -> Result<(), SketchGeometryError> {
        let points = match self {
            Self::Line { start, end } => vec![*start, *end],
            Self::Arc {
                start, end, center, ..
            } => vec![*start, *end, *center],
            Self::RationalQuadratic {
                start,
                control,
                end,
                ..
            } => vec![*start, *control, *end],
            Self::CubicBezier {
                start,
                control1,
                control2,
                end,
            } => vec![*start, *control1, *control2, *end],
        };
        if points.iter().flatten().any(|value| !value.is_finite()) {
            return Err(SketchGeometryError::NonFiniteSegment(index));
        }
        if distance(self.start(), self.end()) <= GEOMETRY_EPSILON {
            return Err(SketchGeometryError::DegenerateSegment(index));
        }
        if let Self::Arc {
            start, end, center, ..
        } = self
        {
            let start_radius = distance(*start, *center);
            let end_radius = distance(*end, *center);
            if start_radius <= GEOMETRY_EPSILON
                || (start_radius - end_radius).abs()
                    > GEOMETRY_EPSILON * start_radius.max(end_radius).max(1.0)
            {
                return Err(SketchGeometryError::InvalidArcRadius(index));
            }
        }
        match self {
            Self::RationalQuadratic {
                start,
                control,
                end,
                weight,
            } => {
                RationalQuadraticBezier2D::new(*start, *control, *end, *weight)
                    .and_then(|curve| curve.sample_adaptive(CURVE_SAMPLING_OPTIONS))
                    .map_err(|reason| SketchGeometryError::InvalidCurve {
                        segment: index,
                        reason,
                    })?;
            }
            Self::CubicBezier {
                start,
                control1,
                control2,
                end,
            } => {
                CubicBezier2D::new(*start, *control1, *control2, *end)
                    .and_then(|curve| curve.sample_adaptive(CURVE_SAMPLING_OPTIONS))
                    .map_err(|reason| SketchGeometryError::InvalidCurve {
                        segment: index,
                        reason,
                    })?;
            }
            Self::Line { .. } | Self::Arc { .. } => {}
        }
        Ok(())
    }
}

/// One closed, ordered sketch loop. The first point is not repeated.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SketchLoop2D {
    pub segments: Vec<SketchSegment2D>,
}

impl<'de> Deserialize<'de> for SketchLoop2D {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Representation {
            Segments(Vec<SketchSegment2D>),
            Polygon(Vec<[f64; 2]>),
        }

        Ok(match Representation::deserialize(deserializer)? {
            Representation::Segments(segments) => Self { segments },
            Representation::Polygon(points) => Self::from_polygon(points),
        })
    }
}

impl SketchLoop2D {
    #[must_use]
    pub fn from_polygon(points: Vec<[f64; 2]>) -> Self {
        let mut points = points.into_iter();
        let Some(first) = points.next() else {
            return Self {
                segments: Vec::new(),
            };
        };
        let mut previous = first;
        let mut segments = points
            .map(|point| {
                let segment = SketchSegment2D::Line {
                    start: previous,
                    end: point,
                };
                previous = point;
                segment
            })
            .collect::<Vec<_>>();
        segments.push(SketchSegment2D::Line {
            start: previous,
            end: first,
        });
        Self { segments }
    }

    #[must_use]
    pub fn is_linear(&self) -> bool {
        self.segments.iter().all(SketchSegment2D::is_line)
    }

    #[must_use]
    pub fn vertices(&self) -> Vec<[f64; 2]> {
        self.segments.iter().map(SketchSegment2D::start).collect()
    }

    #[must_use]
    pub fn sampled_points(&self, max_angle: f64) -> Vec<[f64; 2]> {
        self.segments
            .iter()
            .flat_map(|segment| {
                let mut points = segment.sampled_points(max_angle);
                points.pop();
                points
            })
            .collect()
    }

    #[must_use]
    pub fn signed_area(&self) -> f64 {
        self.segments
            .iter()
            .map(|segment| match segment {
                SketchSegment2D::Line { start, end } => {
                    (start[0] * end[1] - end[0] * start[1]) / 2.0
                }
                SketchSegment2D::Arc {
                    start,
                    end,
                    center,
                    ccw,
                } => {
                    let radius = distance(*start, *center);
                    let sweep = arc_sweep(*start, *end, *center, *ccw);
                    f64::midpoint(
                        center[0] * (end[1] - start[1]) - center[1] * (end[0] - start[0]),
                        radius * radius * sweep,
                    )
                }
                segment @ (SketchSegment2D::RationalQuadratic { .. }
                | SketchSegment2D::CubicBezier { .. }) => segment
                    .sampled_points(PI / 90.0)
                    .windows(2)
                    .map(|points| (points[0][0] * points[1][1] - points[1][0] * points[0][1]) / 2.0)
                    .sum(),
            })
            .sum()
    }

    #[must_use]
    pub fn contains_point_strict(&self, point: [f64; 2]) -> bool {
        if self
            .segments
            .iter()
            .any(|segment| point_on_segment(point, segment))
        {
            return false;
        }
        !self
            .segments
            .iter()
            .map(|segment| ray_crossings(point, segment))
            .sum::<usize>()
            .is_multiple_of(2)
    }

    #[must_use]
    pub fn signed_distance_range_to_line(
        &self,
        origin: [f64; 2],
        direction: [f64; 2],
    ) -> Option<[f64; 2]> {
        let length = direction[0].hypot(direction[1]);
        if !length.is_finite() || length <= GEOMETRY_EPSILON {
            return None;
        }
        let normal = [-direction[1] / length, direction[0] / length];
        let signed_distance = |point: [f64; 2]| {
            normal[0].mul_add(point[0] - origin[0], normal[1] * (point[1] - origin[1]))
        };
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for segment in &self.segments {
            for point in [segment.start(), segment.end()] {
                let value = signed_distance(point);
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
            if let SketchSegment2D::Arc {
                start,
                end,
                center,
                ccw,
            } = segment
            {
                let radius = distance(*start, *center);
                for sign in [-1.0, 1.0] {
                    let point = [
                        radius.mul_add(normal[0] * sign, center[0]),
                        radius.mul_add(normal[1] * sign, center[1]),
                    ];
                    if arc_contains_point(*start, *end, *center, *ccw, point, true) {
                        let value = signed_distance(point);
                        minimum = minimum.min(value);
                        maximum = maximum.max(value);
                    }
                }
            } else if segment.is_curved() {
                for piece in segment_bezier_pieces(segment) {
                    for point in piece.projected_controls() {
                        let value = signed_distance(point);
                        minimum = minimum.min(value);
                        maximum = maximum.max(value);
                    }
                }
            }
        }
        (minimum.is_finite() && maximum.is_finite()).then_some([minimum, maximum])
    }

    /// Validates exact segment geometry, closure, intersections, and area.
    ///
    /// # Errors
    ///
    /// Returns a structured geometry error for the first invalid invariant.
    pub fn validate(&self) -> Result<(), SketchGeometryError> {
        if !(2..=128).contains(&self.segments.len()) {
            return Err(SketchGeometryError::InvalidSegmentCount(
                self.segments.len(),
            ));
        }
        for (index, segment) in self.segments.iter().enumerate() {
            segment.validate(index)?;
            let next = (index + 1) % self.segments.len();
            let gap = distance(segment.end(), self.segments[next].start());
            if gap > GEOMETRY_EPSILON {
                return Err(SketchGeometryError::NotClosed {
                    segment: index,
                    gap,
                });
            }
        }
        for first in 0..self.segments.len() {
            for second in (first + 1)..self.segments.len() {
                let adjacent =
                    second == first + 1 || (first == 0 && second + 1 == self.segments.len());
                let allowed = if self.segments.len() == 2 {
                    vec![self.segments[first].start(), self.segments[first].end()]
                } else if adjacent {
                    vec![if second == first + 1 {
                        self.segments[first].end()
                    } else {
                        self.segments[second].end()
                    }]
                } else {
                    Vec::new()
                };
                if forbidden_intersection(&self.segments[first], &self.segments[second], &allowed) {
                    return Err(SketchGeometryError::SelfIntersection { first, second });
                }
            }
        }
        if self.signed_area().abs() <= GEOMETRY_EPSILON {
            return Err(SketchGeometryError::DegenerateArea);
        }
        Ok(())
    }
}

/// One bounded sketch region with an outer loop and explicit inner loops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchRegion2D {
    pub profile: SketchLoop2D,
    #[serde(default)]
    pub holes: Vec<SketchLoop2D>,
}

impl SketchRegion2D {
    #[must_use]
    pub fn from_polygons(profile: Vec<[f64; 2]>, holes: Vec<Vec<[f64; 2]>>) -> Self {
        Self {
            profile: SketchLoop2D::from_polygon(profile),
            holes: holes.into_iter().map(SketchLoop2D::from_polygon).collect(),
        }
    }

    /// Validates the outer loop, hole loops, containment, and complexity limits.
    ///
    /// # Errors
    ///
    /// Returns a structured geometry error for the first invalid invariant.
    pub fn validate(&self) -> Result<(), SketchGeometryError> {
        const MAX_HOLES: usize = 32;
        const MAX_SEGMENTS: usize = 1_024;

        self.profile.validate()?;
        if self.holes.len() > MAX_HOLES {
            return Err(SketchGeometryError::TooManyHoles(self.holes.len()));
        }
        let segment_count = self.profile.segments.len()
            + self
                .holes
                .iter()
                .map(|hole| hole.segments.len())
                .sum::<usize>();
        if segment_count > MAX_SEGMENTS {
            return Err(SketchGeometryError::TooManySegments(segment_count));
        }
        for (index, hole) in self.holes.iter().enumerate() {
            hole.validate()
                .map_err(|source| SketchGeometryError::InvalidHole {
                    hole: index,
                    source: Box::new(source),
                })?;
            if loops_intersect(&self.profile, hole)
                || hole
                    .segments
                    .iter()
                    .any(|segment| !self.profile.contains_point_strict(segment.start()))
            {
                return Err(SketchGeometryError::HoleOutsideProfile(index));
            }
            for (other_index, other) in self.holes[index + 1..].iter().enumerate() {
                let other_index = index + 1 + other_index;
                if loops_intersect(hole, other)
                    || hole.contains_point_strict(other.segments[0].start())
                    || other.contains_point_strict(hole.segments[0].start())
                {
                    return Err(SketchGeometryError::IntersectingHoles {
                        first: index,
                        second: other_index,
                    });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::circle;

    #[test]
    fn exact_arc_loops_validate_area_containment_and_distance_extrema() {
        let loop_ = circle([2.0, 3.0], 4.0, true);
        loop_.validate().unwrap();
        assert!((loop_.signed_area() - PI * 16.0).abs() < 1.0e-10);
        assert!(loop_.contains_point_strict([2.0, 3.0]));
        assert!(!loop_.contains_point_strict([6.0, 3.0]));
        assert!(!loop_.contains_point_strict([7.0, 3.0]));
        let range = loop_
            .signed_distance_range_to_line([0.0, 0.0], [0.0, 1.0])
            .unwrap();
        assert!((range[0] + 6.0).abs() < 1.0e-10);
        assert!((range[1] - 2.0).abs() < 1.0e-10);

        let clockwise = circle([2.0, 3.0], 4.0, false);
        clockwise.validate().unwrap();
        assert!((clockwise.signed_area() + PI * 16.0).abs() < 1.0e-10);
    }

    #[test]
    fn curved_regions_reject_tangent_overlapping_and_nested_holes() {
        let profile =
            SketchLoop2D::from_polygon(vec![[0.0, 0.0], [20.0, 0.0], [20.0, 16.0], [0.0, 16.0]]);
        SketchRegion2D {
            profile: profile.clone(),
            holes: vec![
                circle([6.0, 8.0], 2.0, true),
                circle([14.0, 8.0], 2.0, false),
            ],
        }
        .validate()
        .unwrap();

        for holes in [
            vec![circle([2.0, 8.0], 2.0, true)],
            vec![
                circle([8.0, 8.0], 3.0, true),
                circle([12.0, 8.0], 3.0, true),
            ],
            vec![
                circle([10.0, 8.0], 5.0, true),
                circle([10.0, 8.0], 2.0, true),
            ],
        ] {
            assert!(
                SketchRegion2D {
                    profile: profile.clone(),
                    holes,
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn sketch_loop_json_reads_legacy_points_and_writes_typed_segments() {
        let legacy = "[[0.0,0.0],[4.0,0.0],[4.0,3.0],[0.0,3.0]]";
        let loop_: SketchLoop2D = serde_json::from_str(legacy).unwrap();
        loop_.validate().unwrap();
        assert!(loop_.is_linear());
        let encoded = serde_json::to_string(&loop_).unwrap();
        assert!(encoded.contains("\"type\":\"line\""));
        assert_eq!(
            serde_json::from_str::<SketchLoop2D>(&encoded).unwrap(),
            loop_
        );
    }

    #[test]
    fn invalid_rational_weight_fails_before_a_sketch_can_commit() {
        let mut region = SketchRegion2D::from_polygons(
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            Vec::new(),
        );
        region.profile.segments[0] = SketchSegment2D::RationalQuadratic {
            start: [0.0, 0.0],
            control: [5.0, -2.0],
            end: [10.0, 0.0],
            weight: 0.0,
        };

        assert!(matches!(
            region.validate(),
            Err(SketchGeometryError::InvalidCurve {
                segment: 0,
                reason: CurveError::InvalidWeight(weight),
            }) if weight == 0.0
        ));
    }
}
