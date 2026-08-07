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

mod annotation;
mod constraint_validation;
mod constraints;
mod curves;
mod diagnostics;
mod error;
mod geometry;
mod intersections;
mod math;
mod nonlinear;
mod parameters;
mod projection;
mod residuals;
mod solver;
#[cfg(test)]
mod test_support;

pub use annotation::{
    SketchAnnotationGeometry2D, SketchConstraintAnnotation2D, constraint_annotations,
};
pub use constraints::{
    Constraint, SketchDimension, SketchDimensionKind, construction_point_ids,
    construction_segment_id,
};
pub use curves::{
    CubicBezier2D, CurveDerivatives2D, CurveError, CurveSamplingOptions, RationalQuadraticBezier2D,
    SketchControlPointRef,
};
pub use diagnostics::SketchSolveDiagnostic;
pub use error::{SketchError, SketchGeometryError};
pub use geometry::{SketchLoop2D, SketchRegion2D, SketchSegment2D};
pub use solver::{
    SolvedProfile, SolvedSketch2D, SolverConfig, solve_profile, solve_region, solve_sketch,
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
