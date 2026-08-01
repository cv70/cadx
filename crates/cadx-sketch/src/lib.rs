//! Kernel-neutral exact 2D sketch regions and deterministic constraint solvers.
//!
//! Persistent loops contain explicit Line and circular Arc segments. The
//! projection-compatible Line constraint set uses bounded projection; advanced
//! relationships, Arc geometry, and independent non-solid construction
//! segments use bounded nonlinear solving over profile vertices, construction
//! endpoints, and arc centers. Converged systems expose numerical rank, degrees
//! of freedom, and ordered redundancy; conflicting systems identify unsatisfied
//! user constraints without returning partial geometry. Solved constraints can
//! also produce exact local annotation witnesses without introducing screen or
//! UI state. This crate has no
//! dependency on CADX documents, UI frameworks, or a B-Rep kernel.

use serde::{Deserialize, Deserializer, Serialize};
use std::f64::consts::{PI, TAU};
use thiserror::Error;

mod annotation;
mod curves;
mod nonlinear;

pub use annotation::{
    SketchAnnotationGeometry2D, SketchConstraintAnnotation2D, constraint_annotations,
};
pub use curves::{
    CubicBezier2D, CurveDerivatives2D, CurveError, CurveSamplingOptions, RationalQuadraticBezier2D,
    SketchControlPointRef,
};

pub type PointId = u32;
pub type SegmentId = u32;

const GEOMETRY_EPSILON: f64 = 1.0e-9;
const ANGULAR_EPSILON: f64 = 1.0e-10;
pub const MAX_CONSTRUCTION_SEGMENTS: usize = 128;
const CURVE_SAMPLING_TOLERANCE: f64 = 1.0e-7;
const CURVE_SAMPLING_OPTIONS: CurveSamplingOptions = CurveSamplingOptions {
    tolerance: CURVE_SAMPLING_TOLERANCE,
    max_depth: 48,
    max_points: 16_384,
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

    fn validate(&self, index: usize) -> Result<(), SketchGeometryError> {
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

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SketchGeometryError {
    #[error("sketch loop must contain between 2 and 128 segments, got {0}")]
    InvalidSegmentCount(usize),
    #[error("sketch segment {0} contains a non-finite coordinate")]
    NonFiniteSegment(usize),
    #[error("sketch segment {0} is degenerate")]
    DegenerateSegment(usize),
    #[error("sketch arc {0} must have a finite positive radius shared by both endpoints")]
    InvalidArcRadius(usize),
    #[error("sketch curve {segment} is invalid: {reason}")]
    InvalidCurve { segment: usize, reason: CurveError },
    #[error("sketch loop is not closed after segment {segment}; gap is {gap} mm")]
    NotClosed { segment: usize, gap: f64 },
    #[error("sketch segments {first} and {second} intersect")]
    SelfIntersection { first: usize, second: usize },
    #[error("sketch loop has zero signed area")]
    DegenerateArea,
    #[error("sketch contains {0} holes; at most 32 are supported")]
    TooManyHoles(usize),
    #[error("sketch contains {0} total segments; at most 1024 are supported")]
    TooManySegments(usize),
    #[error("sketch hole {hole} is invalid: {source}")]
    InvalidHole {
        hole: usize,
        source: Box<SketchGeometryError>,
    },
    #[error("sketch hole {0} must lie strictly inside the outer profile")]
    HoleOutsideProfile(usize),
    #[error("sketch holes {first} and {second} touch, intersect, overlap, or nest")]
    IntersectingHoles { first: usize, second: usize },
}

#[derive(Debug, Clone, PartialEq)]
enum CurveIntersections {
    None,
    Points(Vec<[f64; 2]>),
    Overlap,
}

fn forbidden_intersection(
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

fn loops_intersect(first: &SketchLoop2D, second: &SketchLoop2D) -> bool {
    first.segments.iter().any(|first| {
        second
            .segments
            .iter()
            .any(|second| !matches!(curve_intersections(first, second), CurveIntersections::None))
    })
}

fn curve_intersections(first: &SketchSegment2D, second: &SketchSegment2D) -> CurveIntersections {
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

fn sampled_curve_intersections(
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
enum IntersectionBezierPiece {
    Polynomial(Vec<[f64; 2]>),
    Rational(Vec<[f64; 3]>),
}

impl IntersectionBezierPiece {
    fn projected_controls(&self) -> Vec<[f64; 2]> {
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

fn split_bezier_controls<const N: usize>(
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

fn segment_bezier_pieces(segment: &SketchSegment2D) -> Vec<IntersectionBezierPiece> {
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

fn bezier_piece_intersections(
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

fn bounds_are_separated(first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> bool {
    (0..2).any(|axis| first[1][axis] < second[0][axis] || second[1][axis] < first[0][axis])
}

fn bounds_overlap_center(first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> [f64; 2] {
    std::array::from_fn(|axis| {
        first[0][axis]
            .max(second[0][axis])
            .midpoint(first[1][axis].min(second[1][axis]))
    })
}

fn line_line_intersections(
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

fn line_arc_intersections(
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

fn arc_arc_intersections(first: &SketchSegment2D, second: &SketchSegment2D) -> CurveIntersections {
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

fn point_on_segment(point: [f64; 2], segment: &SketchSegment2D) -> bool {
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

fn point_on_line(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> bool {
    point_segment_distance_squared(point, start, end) <= GEOMETRY_EPSILON.powi(2)
}

fn arc_contains_point(
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

fn ray_crossings(point: [f64; 2], segment: &SketchSegment2D) -> usize {
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

fn arc_sweep(start: [f64; 2], end: [f64; 2], center: [f64; 2], ccw: bool) -> f64 {
    let start_angle = (start[1] - center[1]).atan2(start[0] - center[0]);
    let end_angle = (end[1] - center[1]).atan2(end[0] - center[0]);
    if ccw {
        (end_angle - start_angle).rem_euclid(TAU)
    } else {
        -(start_angle - end_angle).rem_euclid(TAU)
    }
}

fn point_segment_distance_squared(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> f64 {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let length_squared = dot(delta, delta);
    let factor = if length_squared > f64::EPSILON {
        (dot([point[0] - start[0], point[1] - start[1]], delta) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    squared_distance(
        point,
        [
            delta[0].mul_add(factor, start[0]),
            delta[1].mul_add(factor, start[1]),
        ],
    )
}

fn sampled_curve_length(points: &[[f64; 2]]) -> f64 {
    points
        .windows(2)
        .map(|pair| distance(pair[0], pair[1]))
        .sum()
}

fn deduplicate_points(points: &mut Vec<[f64; 2]>) {
    let mut unique = Vec::with_capacity(points.len());
    for point in points.drain(..) {
        if unique
            .iter()
            .all(|other| distance(point, *other) > GEOMETRY_EPSILON)
        {
            unique.push(point);
        }
    }
    *points = unique;
}

fn squared_distance(first: [f64; 2], second: [f64; 2]) -> f64 {
    (first[0] - second[0]).mul_add(
        first[0] - second[0],
        (first[1] - second[1]) * (first[1] - second[1]),
    )
}

fn distance(first: [f64; 2], second: [f64; 2]) -> f64 {
    squared_distance(first, second).sqrt()
}

fn dot(first: [f64; 2], second: [f64; 2]) -> f64 {
    first[0].mul_add(second[0], first[1] * second[1])
}

fn cross(first: [f64; 2], second: [f64; 2]) -> f64 {
    first[0].mul_add(second[1], -first[1] * second[0])
}

/// Returns the persistent segment id of an appended construction segment.
#[must_use]
pub fn construction_segment_id(
    profile_segment_count: usize,
    construction_index: usize,
) -> Option<SegmentId> {
    profile_segment_count
        .checked_add(construction_index)
        .and_then(|id| SegmentId::try_from(id).ok())
}

/// Returns the two persistent point ids owned by a construction segment.
#[must_use]
pub fn construction_point_ids(
    profile_point_count: usize,
    construction_index: usize,
) -> Option<[PointId; 2]> {
    let start = profile_point_count.checked_add(construction_index.checked_mul(2)?)?;
    Some([
        PointId::try_from(start).ok()?,
        PointId::try_from(start.checked_add(1)?).ok()?,
    ])
}

#[derive(Clone, Copy)]
struct ConstraintGeometry<'a> {
    profile: &'a SketchLoop2D,
    construction: &'a [SketchSegment2D],
}

impl<'a> ConstraintGeometry<'a> {
    fn point_count(self) -> usize {
        self.profile
            .segments
            .len()
            .saturating_add(self.construction.len().saturating_mul(2))
    }

    fn segment_count(self) -> usize {
        self.profile
            .segments
            .len()
            .saturating_add(self.construction.len())
    }

    fn segment(self, id: SegmentId) -> &'a SketchSegment2D {
        let index = usize::try_from(id).expect("validated segment id");
        if index < self.profile.segments.len() {
            &self.profile.segments[index]
        } else {
            &self.construction[index - self.profile.segments.len()]
        }
    }

    fn segment_point_ids(self, id: SegmentId) -> (PointId, PointId) {
        let index = usize::try_from(id).expect("validated segment id");
        let profile_count = self.profile.segments.len();
        if index < profile_count {
            (
                id,
                PointId::try_from((index + 1) % profile_count)
                    .expect("profile segment limit fits u32"),
            )
        } else {
            let [start, end] = construction_point_ids(profile_count, index - profile_count)
                .expect("construction segment limit fits u32");
            (start, end)
        }
    }

    fn point(self, id: PointId) -> [f64; 2] {
        let index = usize::try_from(id).expect("validated point id");
        let profile_count = self.profile.segments.len();
        if index < profile_count {
            self.profile.segments[index].start()
        } else {
            let construction_point = index - profile_count;
            let segment = &self.construction[construction_point / 2];
            if construction_point.is_multiple_of(2) {
                segment.start()
            } else {
                segment.end()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Constraint {
    Coincident {
        first: PointId,
        second: PointId,
    },
    Horizontal {
        segment: SegmentId,
    },
    Vertical {
        segment: SegmentId,
    },
    Fixed {
        point: PointId,
        x: f64,
        y: f64,
    },
    Distance {
        first: PointId,
        second: PointId,
        distance: f64,
    },
    HorizontalDistance {
        first: PointId,
        second: PointId,
        distance: f64,
    },
    VerticalDistance {
        first: PointId,
        second: PointId,
        distance: f64,
    },
    PointLineDistance {
        point: PointId,
        line: SegmentId,
        distance: f64,
    },
    LineThroughCenter {
        line: SegmentId,
        arc: SegmentId,
    },
    PointOnCurve {
        point: PointId,
        segment: SegmentId,
    },
    Midpoint {
        point: PointId,
        segment: SegmentId,
    },
    Symmetric {
        first: PointId,
        second: PointId,
        axis: SegmentId,
    },
    Length {
        segment: SegmentId,
        length: f64,
    },
    EqualLength {
        first: SegmentId,
        second: SegmentId,
    },
    Parallel {
        first: SegmentId,
        second: SegmentId,
    },
    Perpendicular {
        first: SegmentId,
        second: SegmentId,
    },
    Angle {
        first: SegmentId,
        second: SegmentId,
        angle_degrees: f64,
    },
    Radius {
        segment: SegmentId,
        radius: f64,
    },
    FixedCenter {
        segment: SegmentId,
        x: f64,
        y: f64,
    },
    EqualRadius {
        first: SegmentId,
        second: SegmentId,
    },
    Concentric {
        first: SegmentId,
        second: SegmentId,
    },
    Tangent {
        first: SegmentId,
        second: SegmentId,
    },
    CurvatureContinuous {
        first: SegmentId,
        second: SegmentId,
    },
}

impl Constraint {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Coincident { .. } => "Coincident",
            Self::Horizontal { .. } => "Horizontal",
            Self::Vertical { .. } => "Vertical",
            Self::Fixed { .. } => "Fixed",
            Self::Distance { .. } => "Distance",
            Self::HorizontalDistance { .. } => "Horizontal distance",
            Self::VerticalDistance { .. } => "Vertical distance",
            Self::PointLineDistance { .. } => "Point-line distance",
            Self::LineThroughCenter { .. } => "Line through center",
            Self::PointOnCurve { .. } => "Point on curve",
            Self::Midpoint { .. } => "Midpoint",
            Self::Symmetric { .. } => "Symmetric",
            Self::Length { .. } => "Length",
            Self::EqualLength { .. } => "Equal length",
            Self::Parallel { .. } => "Parallel",
            Self::Perpendicular { .. } => "Perpendicular",
            Self::Angle { .. } => "Angle",
            Self::Radius { .. } => "Radius",
            Self::FixedCenter { .. } => "Fixed center",
            Self::EqualRadius { .. } => "Equal radius",
            Self::Concentric { .. } => "Concentric",
            Self::Tangent { .. } => "Tangent",
            Self::CurvatureContinuous { .. } => "Curvature continuous",
        }
    }

    /// Returns the directly editable driving dimension carried by this
    /// constraint. Placement locks expose glyphs rather than two independent
    /// coordinate dimensions and therefore return `None`.
    #[must_use]
    pub const fn dimension(&self) -> Option<SketchDimension> {
        match self {
            Self::Distance { distance, .. } => Some(SketchDimension {
                kind: SketchDimensionKind::Distance,
                value: *distance,
            }),
            Self::HorizontalDistance { distance, .. } => Some(SketchDimension {
                kind: SketchDimensionKind::HorizontalDistance,
                value: *distance,
            }),
            Self::VerticalDistance { distance, .. } => Some(SketchDimension {
                kind: SketchDimensionKind::VerticalDistance,
                value: *distance,
            }),
            Self::PointLineDistance { distance, .. } => Some(SketchDimension {
                kind: SketchDimensionKind::PointLineDistance,
                value: *distance,
            }),
            Self::Length { length, .. } => Some(SketchDimension {
                kind: SketchDimensionKind::Length,
                value: *length,
            }),
            Self::Angle { angle_degrees, .. } => Some(SketchDimension {
                kind: SketchDimensionKind::Angle,
                value: *angle_degrees,
            }),
            Self::Radius { radius, .. } => Some(SketchDimension {
                kind: SketchDimensionKind::Radius,
                value: *radius,
            }),
            Self::Coincident { .. }
            | Self::Horizontal { .. }
            | Self::Vertical { .. }
            | Self::Fixed { .. }
            | Self::LineThroughCenter { .. }
            | Self::PointOnCurve { .. }
            | Self::Midpoint { .. }
            | Self::Symmetric { .. }
            | Self::EqualLength { .. }
            | Self::Parallel { .. }
            | Self::Perpendicular { .. }
            | Self::FixedCenter { .. }
            | Self::EqualRadius { .. }
            | Self::Concentric { .. }
            | Self::Tangent { .. }
            | Self::CurvatureContinuous { .. } => None,
        }
    }

    /// Clones this constraint with a validated driving dimension replacement.
    /// Returns `None` for non-dimensional constraints or values outside the
    /// variant's persistent domain.
    #[must_use]
    pub fn with_dimension_value(&self, value: f64) -> Option<Self> {
        let dimension = self.dimension()?;
        if !dimension.kind.accepts(value) {
            return None;
        }
        let mut constraint = self.clone();
        match &mut constraint {
            Self::Distance { distance, .. }
            | Self::HorizontalDistance { distance, .. }
            | Self::VerticalDistance { distance, .. }
            | Self::PointLineDistance { distance, .. } => *distance = value,
            Self::Length { length, .. } => *length = value,
            Self::Angle { angle_degrees, .. } => *angle_degrees = value,
            Self::Radius { radius, .. } => *radius = value,
            _ => return None,
        }
        Some(constraint)
    }

    const fn requires_curve(&self) -> bool {
        matches!(
            self,
            Self::Radius { .. }
                | Self::FixedCenter { .. }
                | Self::EqualRadius { .. }
                | Self::Concentric { .. }
                | Self::Tangent { .. }
                | Self::CurvatureContinuous { .. }
        )
    }

    const fn requires_nonlinear(&self) -> bool {
        self.requires_curve()
            || matches!(
                self,
                Self::Length { .. }
                    | Self::HorizontalDistance { .. }
                    | Self::VerticalDistance { .. }
                    | Self::PointLineDistance { .. }
                    | Self::LineThroughCenter { .. }
                    | Self::PointOnCurve { .. }
                    | Self::Midpoint { .. }
                    | Self::Symmetric { .. }
                    | Self::EqualLength { .. }
                    | Self::Parallel { .. }
                    | Self::Perpendicular { .. }
                    | Self::Angle { .. }
            )
    }

    fn validate(&self, point_count: usize, segment_count: usize) -> Result<(), SketchError> {
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

    fn validate_for_geometry(&self, geometry: ConstraintGeometry<'_>) -> Result<(), SketchError> {
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

/// Persistent value domain of a directly editable sketch dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SketchDimensionKind {
    Distance,
    HorizontalDistance,
    VerticalDistance,
    PointLineDistance,
    Length,
    Angle,
    Radius,
}

impl SketchDimensionKind {
    #[must_use]
    pub const fn accepts(self, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        match self {
            Self::HorizontalDistance | Self::VerticalDistance => true,
            Self::Distance | Self::PointLineDistance => value >= 0.0,
            Self::Length | Self::Radius => value > 0.0,
            Self::Angle => value >= -180.0 && value <= 180.0,
        }
    }
}

/// One directly editable value and its unit/domain semantics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SketchDimension {
    pub kind: SketchDimensionKind,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SolverConfig {
    pub max_iterations: u32,
    pub tolerance: f64,
    pub relaxation: f64,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_iterations: 512,
            tolerance: 1.0e-8,
            relaxation: 0.85,
        }
    }
}

impl SolverConfig {
    fn validate(self) -> Result<Self, SketchError> {
        if self.max_iterations == 0
            || !self.tolerance.is_finite()
            || self.tolerance <= 0.0
            || !self.relaxation.is_finite()
            || !(0.0..=1.0).contains(&self.relaxation)
            || self.relaxation == 0.0
        {
            return Err(SketchError::InvalidSolverConfig);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SolvedProfile {
    pub profile: Vec<[f64; 2]>,
    pub residual: f64,
    pub iterations: u32,
    pub diagnostic: SketchSolveDiagnostic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SolvedSketch2D {
    pub region: SketchRegion2D,
    pub construction: Vec<SketchSegment2D>,
    pub residual: f64,
    pub iterations: u32,
    pub diagnostic: SketchSolveDiagnostic,
}

/// Rank-based, kernel-neutral report for one converged constraint system.
/// `degrees_of_freedom` counts locally independent parameter directions at the
/// solved geometry; it is not inferred from the raw number of constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchSolveDiagnostic {
    pub parameter_count: u32,
    pub equation_count: u32,
    pub rank: u32,
    pub degrees_of_freedom: u32,
    /// Ordered user-constraint indices containing at least one equation that
    /// does not increase rank after all earlier constraints.
    pub redundant_constraints: Vec<u32>,
    pub residual: f64,
    pub iterations: u32,
}

impl SketchSolveDiagnostic {
    #[must_use]
    pub const fn is_fully_constrained(&self) -> bool {
        self.degrees_of_freedom == 0
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SketchError {
    #[error("sketch profile must contain at least three points")]
    TooFewPoints,
    #[error("sketch point {0} is not finite")]
    NonFinitePoint(PointId),
    #[error("constraint references invalid point {0}")]
    InvalidPoint(PointId),
    #[error("constraint references invalid segment {0}")]
    InvalidSegment(SegmentId),
    #[error("constraint contains a non-finite value")]
    NonFiniteValue,
    #[error("distance constraint must be finite and non-negative, got {0}")]
    InvalidDistance(f64),
    #[error("length constraint must be finite and greater than zero, got {0}")]
    InvalidLength(f64),
    #[error("angle constraint must be finite and between -180 and 180 degrees, got {0}")]
    InvalidAngle(f64),
    #[error("radius constraint must be finite and greater than zero, got {0}")]
    InvalidRadius(f64),
    #[error("constraint segment {segment} must reference a {expected}")]
    InvalidConstraintEntity {
        segment: SegmentId,
        expected: &'static str,
    },
    #[error("tangent constraint segments {first} and {second} are not adjacent")]
    NonAdjacentSegments { first: SegmentId, second: SegmentId },
    #[error("constraint requires two distinct segments, got {first} and {second}")]
    IdenticalSegments { first: SegmentId, second: SegmentId },
    #[error("constraint requires two distinct points, got {first} and {second}")]
    IdenticalPoints { first: PointId, second: PointId },
    #[error("point {point} is an endpoint of midpoint segment {segment}")]
    PointIsSegmentEndpoint { point: PointId, segment: SegmentId },
    #[error("sketch contains {0} construction segments; at most 128 are supported")]
    TooManyConstructionSegments(usize),
    #[error("invalid construction segment {segment}: {error}")]
    InvalidConstructionSegment {
        segment: SegmentId,
        error: SketchGeometryError,
    },
    #[error("solved point {point} does not lie on finite curve segment {segment}")]
    PointNotOnCurve { point: PointId, segment: SegmentId },
    #[error("solver configuration is invalid")]
    InvalidSolverConfig,
    #[error("invalid sketch region: {0}")]
    InvalidGeometry(#[from] SketchGeometryError),
    #[error("curve-specific constraints require an exact sketch region")]
    CurveConstraintRequiresRegion,
    #[error(
        "constraint system conflicts at indices {constraints:?} after {iterations} iterations (residual {residual})"
    )]
    ConstraintConflict {
        iterations: u32,
        residual: f64,
        constraints: Vec<u32>,
    },
    #[error("constraints did not converge after {iterations} iterations (residual {residual})")]
    NotConverged { iterations: u32, residual: f64 },
}

/// Solves a cyclic polygon profile using a deterministic bounded solver.
///
/// Projection-compatible constraints are applied in declaration order. Length
/// and advanced Line relationships use the bounded nonlinear solver. A failed
/// solve is reported instead of silently accepting a partial result.
///
/// # Errors
///
/// Returns an error when the input or constraints are invalid, the solver
/// configuration is unsafe, or the system does not converge.
pub fn solve_profile(
    profile: &[[f64; 2]],
    constraints: &[Constraint],
    config: SolverConfig,
) -> Result<SolvedProfile, SketchError> {
    if profile.len() < 3 {
        return Err(SketchError::TooFewPoints);
    }
    for (index, point) in profile.iter().enumerate() {
        if !point[0].is_finite() || !point[1].is_finite() {
            return Err(SketchError::NonFinitePoint(
                u32::try_from(index).unwrap_or(u32::MAX),
            ));
        }
    }
    let config = config.validate()?;
    for constraint in constraints {
        constraint.validate(profile.len(), profile.len())?;
    }
    if constraints.iter().any(Constraint::requires_curve) {
        return Err(SketchError::CurveConstraintRequiresRegion);
    }
    if constraints.is_empty() {
        let diagnostic = diagnose_linear_profile(profile, constraints, 0.0, 0);
        return Ok(SolvedProfile {
            profile: profile.to_vec(),
            residual: 0.0,
            iterations: 0,
            diagnostic,
        });
    }
    if constraints.iter().any(Constraint::requires_nonlinear) {
        let region = SketchRegion2D::from_polygons(profile.to_vec(), Vec::new());
        let solved = solve_sketch(&region, &[], constraints, config)?;
        return Ok(SolvedProfile {
            profile: solved.region.profile.vertices(),
            residual: solved.residual,
            iterations: solved.iterations,
            diagnostic: solved.diagnostic,
        });
    }

    let mut points = profile.to_vec();
    let mut residual = f64::INFINITY;
    for iteration in 1..=config.max_iterations {
        for constraint in constraints {
            project_constraint(&mut points, constraint, config.relaxation);
        }
        residual = constraints
            .iter()
            .map(|constraint| constraint_residual(&points, constraint))
            .fold(0.0, f64::max);
        if residual <= config.tolerance {
            let diagnostic = diagnose_linear_profile(&points, constraints, residual, iteration);
            return Ok(SolvedProfile {
                profile: points,
                residual,
                iterations: iteration,
                diagnostic,
            });
        }
    }
    let conflicts = constraints
        .iter()
        .enumerate()
        .filter(|(_, constraint)| constraint_residual(&points, constraint) > config.tolerance)
        .map(|(index, _)| u32::try_from(index).unwrap_or(u32::MAX))
        .collect::<Vec<_>>();
    if conflicts.is_empty() {
        Err(SketchError::NotConverged {
            iterations: config.max_iterations,
            residual,
        })
    } else {
        Err(SketchError::ConstraintConflict {
            iterations: config.max_iterations,
            residual,
            constraints: conflicts,
        })
    }
}

/// Solves the outer loop of a bounded sketch region while preserving holes.
///
/// Line-only regions with projection-compatible constraints retain the
/// deterministic projection solver. Advanced Line relationships or Arc
/// segments use a bounded Levenberg-Marquardt solve over shared loop vertices
/// and arc centers, including an implicit equal-radius equation for every arc.
///
/// # Errors
///
/// Returns an error for invalid loop topology, invalid constraint references,
/// or a constraint system that does not converge to exact valid geometry.
pub fn solve_region(
    region: &SketchRegion2D,
    constraints: &[Constraint],
    config: SolverConfig,
) -> Result<SketchRegion2D, SketchError> {
    solve_sketch(region, &[], constraints, config).map(|solved| solved.region)
}

/// Solves one exact region and its non-solid construction geometry together.
///
/// Profile ids retain their historical prefix. Construction segment ids are
/// appended after profile segments, and each construction segment owns two
/// appended point ids. Construction geometry participates in constraints but
/// is not part of the returned region.
///
/// # Errors
///
/// Returns an error for invalid region or construction geometry, invalid
/// entity references, non-convergence, or a solved point outside its finite
/// target curve.
pub fn solve_sketch(
    region: &SketchRegion2D,
    construction: &[SketchSegment2D],
    constraints: &[Constraint],
    config: SolverConfig,
) -> Result<SolvedSketch2D, SketchError> {
    region.validate()?;
    validate_construction(region.profile.segments.len(), construction)?;
    let config = config.validate()?;
    let geometry = ConstraintGeometry {
        profile: &region.profile,
        construction,
    };
    if constraints.is_empty() {
        let geometry = ConstraintGeometry {
            profile: &region.profile,
            construction,
        };
        let (parameters, center_offsets) = sketch_parameters(geometry);
        let diagnostic =
            diagnose_solution(geometry, &center_offsets, &parameters, constraints, 0.0, 0);
        return Ok(SolvedSketch2D {
            region: region.clone(),
            construction: construction.to_vec(),
            residual: 0.0,
            iterations: 0,
            diagnostic,
        });
    }
    for constraint in constraints {
        constraint.validate_for_geometry(geometry)?;
    }
    if construction.is_empty()
        && region.profile.is_linear()
        && !constraints.iter().any(Constraint::requires_nonlinear)
    {
        let solved = solve_profile(&region.profile.vertices(), constraints, config)?;
        let region = SketchRegion2D {
            profile: SketchLoop2D::from_polygon(solved.profile),
            holes: region.holes.clone(),
        };
        region.validate()?;
        return Ok(SolvedSketch2D {
            region,
            construction: Vec::new(),
            residual: solved.residual,
            iterations: solved.iterations,
            diagnostic: solved.diagnostic,
        });
    }
    solve_nonlinear_sketch(region, construction, constraints, config)
}

fn validate_construction(
    profile_segment_count: usize,
    construction: &[SketchSegment2D],
) -> Result<(), SketchError> {
    if construction.len() > MAX_CONSTRUCTION_SEGMENTS {
        return Err(SketchError::TooManyConstructionSegments(construction.len()));
    }
    for (index, segment) in construction.iter().enumerate() {
        segment
            .validate(index)
            .map_err(|error| SketchError::InvalidConstructionSegment {
                segment: construction_segment_id(profile_segment_count, index)
                    .expect("construction segment limit fits u32"),
                error,
            })?;
    }
    Ok(())
}

fn solve_nonlinear_sketch(
    region: &SketchRegion2D,
    construction: &[SketchSegment2D],
    constraints: &[Constraint],
    config: SolverConfig,
) -> Result<SolvedSketch2D, SketchError> {
    let profile = &region.profile;
    let geometry = ConstraintGeometry {
        profile,
        construction,
    };
    let (parameters, center_offsets) = sketch_parameters(geometry);
    let tolerance = config.tolerance.min(GEOMETRY_EPSILON * 0.1);
    let solution = nonlinear::solve(
        &parameters,
        config.max_iterations,
        tolerance,
        config.relaxation,
        |parameters| nonlinear_residuals(geometry, &center_offsets, parameters, constraints),
    );
    if !solution.converged {
        let conflicts = conflicting_constraint_indices(
            geometry,
            &center_offsets,
            &solution.parameters,
            constraints,
            tolerance,
        );
        return if conflicts.is_empty() {
            Err(SketchError::NotConverged {
                iterations: solution.iterations,
                residual: solution.residual,
            })
        } else {
            Err(SketchError::ConstraintConflict {
                iterations: solution.iterations,
                residual: solution.residual,
                constraints: conflicts,
            })
        };
    }
    let profile_segments = profile
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            rebuild_parameter_segment(
                geometry,
                &center_offsets,
                &solution.parameters,
                SegmentId::try_from(index).expect("profile segment limit fits u32"),
                segment,
            )
        })
        .collect();
    let solved_region = SketchRegion2D {
        profile: SketchLoop2D {
            segments: profile_segments,
        },
        holes: region.holes.clone(),
    };
    let solved_construction = construction
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let id = construction_segment_id(profile.segments.len(), index)
                .expect("construction segment limit fits u32");
            rebuild_parameter_segment(geometry, &center_offsets, &solution.parameters, id, segment)
        })
        .collect::<Vec<_>>();
    solved_region.validate()?;
    validate_construction(solved_region.profile.segments.len(), &solved_construction)?;
    validate_finite_curve_constraints(&solved_region.profile, &solved_construction, constraints)?;
    let diagnostic = diagnose_solution(
        geometry,
        &center_offsets,
        &solution.parameters,
        constraints,
        solution.residual,
        solution.iterations,
    );
    Ok(SolvedSketch2D {
        region: solved_region,
        construction: solved_construction,
        residual: solution.residual,
        iterations: solution.iterations,
        diagnostic,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct SegmentParameterOffsets {
    center: Option<usize>,
    controls: [Option<usize>; 2],
}

fn sketch_parameters(geometry: ConstraintGeometry<'_>) -> (Vec<f64>, Vec<SegmentParameterOffsets>) {
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

fn rebuild_parameter_segment(
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

fn nonlinear_residuals(
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

fn append_intrinsic_residuals(
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

fn residual_layout(
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    constraints: &[Constraint],
) -> (Vec<f64>, usize, Vec<(usize, usize)>) {
    let mut residuals = Vec::with_capacity(geometry.segment_count() + constraints.len() * 2);
    append_intrinsic_residuals(&mut residuals, geometry, center_offsets, parameters);
    let intrinsic_end = residuals.len();
    let mut ranges = Vec::with_capacity(constraints.len());
    for constraint in constraints {
        let start = residuals.len();
        append_nonlinear_constraint_residuals(
            &mut residuals,
            geometry,
            center_offsets,
            parameters,
            constraint,
        );
        ranges.push((start, residuals.len()));
    }
    (residuals, intrinsic_end, ranges)
}

fn diagnose_linear_profile(
    profile: &[[f64; 2]],
    constraints: &[Constraint],
    residual: f64,
    iterations: u32,
) -> SketchSolveDiagnostic {
    let profile = SketchLoop2D::from_polygon(profile.to_vec());
    let geometry = ConstraintGeometry {
        profile: &profile,
        construction: &[],
    };
    let parameters = profile.vertices().into_iter().flatten().collect::<Vec<_>>();
    let center_offsets = vec![SegmentParameterOffsets::default(); geometry.segment_count()];
    diagnose_solution(
        geometry,
        &center_offsets,
        &parameters,
        constraints,
        residual,
        iterations,
    )
}

fn diagnose_solution(
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    constraints: &[Constraint],
    residual: f64,
    iterations: u32,
) -> SketchSolveDiagnostic {
    let (values, intrinsic_end, ranges) =
        residual_layout(geometry, center_offsets, parameters, constraints);
    let jacobian = nonlinear::finite_difference_jacobian(parameters, &values, &|parameters| {
        nonlinear_residuals(geometry, center_offsets, parameters, constraints)
    });
    let mut rank = nonlinear::RowRank::new(parameters.len());
    if !jacobian.is_empty() {
        rank.add_rows(&jacobian, 0, intrinsic_end);
    }
    let mut redundant_constraints = Vec::new();
    for (index, (start, end)) in ranges.into_iter().enumerate() {
        let added = if jacobian.is_empty() {
            0
        } else {
            rank.add_rows(&jacobian, start, end)
        };
        if added < end - start {
            redundant_constraints.push(u32::try_from(index).unwrap_or(u32::MAX));
        }
    }
    let numerical_rank = rank.rank().min(parameters.len());
    SketchSolveDiagnostic {
        parameter_count: u32::try_from(parameters.len()).unwrap_or(u32::MAX),
        equation_count: u32::try_from(values.len()).unwrap_or(u32::MAX),
        rank: u32::try_from(numerical_rank).unwrap_or(u32::MAX),
        degrees_of_freedom: u32::try_from(parameters.len() - numerical_rank).unwrap_or(u32::MAX),
        redundant_constraints,
        residual,
        iterations,
    }
}

fn conflicting_constraint_indices(
    geometry: ConstraintGeometry<'_>,
    center_offsets: &[SegmentParameterOffsets],
    parameters: &[f64],
    constraints: &[Constraint],
    tolerance: f64,
) -> Vec<u32> {
    let (values, _, ranges) = residual_layout(geometry, center_offsets, parameters, constraints);
    ranges
        .into_iter()
        .enumerate()
        .filter(|(_, (start, end))| {
            values[*start..*end]
                .iter()
                .any(|residual| residual.abs() > tolerance)
        })
        .map(|(index, _)| u32::try_from(index).unwrap_or(u32::MAX))
        .collect()
}

fn append_nonlinear_constraint_residuals(
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

fn parameter_segment_points(
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

fn closest_parameter_curve_point(
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

fn closest_point_on_line_segment(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> [f64; 2] {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let length_squared = dot(delta, delta);
    let factor = if length_squared > f64::EPSILON {
        (dot([point[0] - start[0], point[1] - start[1]], delta) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [
        delta[0].mul_add(factor, start[0]),
        delta[1].mul_add(factor, start[1]),
    ]
}

fn closest_point_on_parametric_curve(
    point: [f64; 2],
    derivatives: impl Fn(f64) -> Result<CurveDerivatives2D, CurveError>,
) -> Option<[f64; 2]> {
    const SEED_COUNT: u32 = 32;
    const MAX_ITERATIONS: usize = 24;
    let mut best = None;
    let mut best_distance = f64::INFINITY;
    for seed in 0..=SEED_COUNT {
        let mut parameter = f64::from(seed) / f64::from(SEED_COUNT);
        for _ in 0..MAX_ITERATIONS {
            let value = derivatives(parameter).ok()?;
            let offset = [value.point[0] - point[0], value.point[1] - point[1]];
            let gradient = dot(offset, value.first);
            let hessian = dot(value.first, value.first) + dot(offset, value.second);
            if !gradient.is_finite() || !hessian.is_finite() || hessian.abs() <= f64::EPSILON {
                break;
            }
            let next = (parameter - gradient / hessian).clamp(0.0, 1.0);
            if (next - parameter).abs() <= 1.0e-13 {
                parameter = next;
                break;
            }
            parameter = next;
        }
        let candidate = derivatives(parameter).ok()?.point;
        let candidate_distance = squared_distance(point, candidate);
        if candidate_distance < best_distance {
            best = Some(candidate);
            best_distance = candidate_distance;
        }
    }
    best
}

fn direction_is_within_arc(
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

fn validate_finite_curve_constraints(
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

fn parameter_point(parameters: &[f64], index: usize) -> [f64; 2] {
    [parameters[index * 2], parameters[index * 2 + 1]]
}

fn parameter_center(
    parameters: &[f64],
    center_offsets: &[SegmentParameterOffsets],
    index: usize,
) -> [f64; 2] {
    let offset = center_offsets[index]
        .center
        .expect("arc segment has a center parameter");
    [parameters[offset], parameters[offset + 1]]
}

fn parameter_control(
    parameters: &[f64],
    segment_offsets: &[SegmentParameterOffsets],
    index: usize,
    control: usize,
) -> [f64; 2] {
    let offset = segment_offsets[index].controls[control]
        .expect("Bezier segment has the requested control parameter");
    [parameters[offset], parameters[offset + 1]]
}

fn shared_vertex(count: usize, first: SegmentId, second: SegmentId) -> Option<usize> {
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

fn constraint_tangent(
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

fn parameterized_profile_segment(
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

fn segment_curvature_at_shared(segment: &SketchSegment2D, shared: [f64; 2]) -> f64 {
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

fn normalize_vector(vector: [f64; 2]) -> [f64; 2] {
    let length = vector[0].hypot(vector[1]).max(GEOMETRY_EPSILON);
    [vector[0] / length, vector[1] / length]
}

fn normalize_angle(angle: f64) -> f64 {
    (angle + PI).rem_euclid(TAU) - PI
}

fn project_constraint(points: &mut [[f64; 2]], constraint: &Constraint, relaxation: f64) {
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

fn constraint_residual(points: &[[f64; 2]], constraint: &Constraint) -> f64 {
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

    fn circle(center: [f64; 2], radius: f64, ccw: bool) -> SketchLoop2D {
        let right = [center[0] + radius, center[1]];
        let left = [center[0] - radius, center[1]];
        let segments = if ccw {
            vec![
                SketchSegment2D::Arc {
                    start: right,
                    end: left,
                    center,
                    ccw: true,
                },
                SketchSegment2D::Arc {
                    start: left,
                    end: right,
                    center,
                    ccw: true,
                },
            ]
        } else {
            vec![
                SketchSegment2D::Arc {
                    start: right,
                    end: left,
                    center,
                    ccw: false,
                },
                SketchSegment2D::Arc {
                    start: left,
                    end: right,
                    center,
                    ccw: false,
                },
            ]
        };
        SketchLoop2D { segments }
    }

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
    fn rank_diagnostics_distinguish_dof_and_redundant_constraints() {
        let region =
            SketchRegion2D::from_polygons(vec![[0.0, 0.0], [10.0, 0.0], [2.0, 6.0]], Vec::new());
        let under = solve_sketch(
            &region,
            &[],
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Horizontal { segment: 0 },
                Constraint::Length {
                    segment: 0,
                    length: 10.0,
                },
            ],
            SolverConfig::default(),
        )
        .unwrap();
        assert_eq!(under.diagnostic.parameter_count, 6);
        assert_eq!(under.diagnostic.rank, 4);
        assert_eq!(under.diagnostic.degrees_of_freedom, 2);
        assert!(under.diagnostic.redundant_constraints.is_empty());

        let fully = solve_sketch(
            &region,
            &[],
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Horizontal { segment: 0 },
                Constraint::Length {
                    segment: 0,
                    length: 10.0,
                },
                Constraint::Fixed {
                    point: 2,
                    x: 2.0,
                    y: 6.0,
                },
                Constraint::Horizontal { segment: 0 },
            ],
            SolverConfig::default(),
        )
        .unwrap();
        assert!(fully.diagnostic.is_fully_constrained());
        assert_eq!(fully.diagnostic.rank, 6);
        assert_eq!(fully.diagnostic.equation_count, 7);
        assert_eq!(fully.diagnostic.redundant_constraints, vec![4]);

        let unconstrained_circle = solve_sketch(
            &SketchRegion2D {
                profile: circle([0.0, 0.0], 5.0, true),
                holes: Vec::new(),
            },
            &[],
            &[],
            SolverConfig::default(),
        )
        .unwrap();
        assert_eq!(unconstrained_circle.diagnostic.parameter_count, 8);
        assert_eq!(unconstrained_circle.diagnostic.equation_count, 2);
        assert_eq!(unconstrained_circle.diagnostic.rank, 2);
        assert_eq!(unconstrained_circle.diagnostic.degrees_of_freedom, 6);
    }

    #[test]
    fn conflicting_constraints_report_ordered_unsatisfied_indices() {
        let result = solve_profile(
            &[[0.0, 0.0], [10.0, 0.0], [0.0, 5.0]],
            &[
                Constraint::Fixed {
                    point: 0,
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Fixed {
                    point: 0,
                    x: 1.0,
                    y: 1.0,
                },
            ],
            SolverConfig {
                max_iterations: 16,
                ..SolverConfig::default()
            },
        );
        assert!(matches!(
            result,
            Err(SketchError::ConstraintConflict {
                constraints,
                ..
            }) if constraints == vec![0, 1]
        ));
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
