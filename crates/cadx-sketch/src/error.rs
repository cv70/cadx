//! Geometry and constraint error types reported by the sketch crate.

use thiserror::Error;

use crate::{PointId, SegmentId, curves::CurveError};

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
