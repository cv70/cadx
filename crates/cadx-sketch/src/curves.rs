use crate::SegmentId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_SUBDIVISION_DEPTH: u8 = 64;
const MAX_SAMPLE_POINTS: usize = 65_536;

/// Stable reference to a curve's internal control point.
///
/// `control` is local to `segment` and never enters the sketch endpoint
/// [`crate::PointId`] namespace. Curve-specific accessors validate the slot
/// before resolving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SketchControlPointRef {
    pub segment: SegmentId,
    pub control: u8,
}

/// A curve point and its first two parameter derivatives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveDerivatives2D {
    pub point: [f64; 2],
    pub first: [f64; 2],
    pub second: [f64; 2],
}

impl CurveDerivatives2D {
    /// Returns signed planar curvature. Positive curvature turns
    /// counterclockwise as the parameter increases.
    ///
    /// # Errors
    ///
    /// Returns an error when the tangent is degenerate or the curvature cannot
    /// be represented as a finite `f64`.
    pub fn signed_curvature(self, parameter: f64) -> Result<f64, CurveError> {
        let speed = self.first[0].hypot(self.first[1]);
        if speed == 0.0 {
            return Err(CurveError::DegenerateTangent { parameter });
        }
        if !speed.is_finite() {
            return Err(CurveError::NonFiniteEvaluation);
        }

        // Scaling both derivative orders before the final division avoids the
        // avoidable overflow in cross(first, second) / |first|^3.
        let tangent = [self.first[0] / speed, self.first[1] / speed];
        let acceleration = [self.second[0] / speed, self.second[1] / speed];
        let curvature = tangent[0].mul_add(acceleration[1], -tangent[1] * acceleration[0]) / speed;
        if curvature.is_finite() {
            Ok(curvature)
        } else {
            Err(CurveError::NonFiniteEvaluation)
        }
    }
}

/// Explicit safety and accuracy bounds for adaptive curve sampling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveSamplingOptions {
    /// Maximum control-hull distance from each accepted polyline chord.
    pub tolerance: f64,
    /// Maximum number of binary subdivisions along any branch.
    pub max_depth: u8,
    /// Maximum number of returned points, including both endpoints.
    pub max_points: usize,
}

impl CurveSamplingOptions {
    fn validate(self) -> Result<(), CurveError> {
        if !self.tolerance.is_finite() || self.tolerance <= 0.0 {
            return Err(CurveError::InvalidSamplingTolerance(self.tolerance));
        }
        if self.max_depth == 0 || self.max_depth > MAX_SUBDIVISION_DEPTH {
            return Err(CurveError::InvalidSamplingDepth(self.max_depth));
        }
        if !(2..=MAX_SAMPLE_POINTS).contains(&self.max_points) {
            return Err(CurveError::InvalidSamplingPointLimit(self.max_points));
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CurveError {
    #[error("curve {0} contains a non-finite coordinate")]
    NonFiniteControlPoint(&'static str),
    #[error("rational quadratic weight must be finite and greater than zero, got {0}")]
    InvalidWeight(f64),
    #[error("curve parameter must be finite and in [0, 1], got {0}")]
    InvalidParameter(f64),
    #[error("control slot {control} is invalid for a curve with {control_count} internal controls")]
    InvalidControlSlot { control: u8, control_count: u8 },
    #[error("control reference targets segment {actual}, expected segment {expected}")]
    MismatchedControlSegment {
        expected: SegmentId,
        actual: SegmentId,
    },
    #[error("curve tangent is degenerate at parameter {parameter}")]
    DegenerateTangent { parameter: f64 },
    #[error("curve evaluation produced a non-finite result")]
    NonFiniteEvaluation,
    #[error("sampling tolerance must be finite and greater than zero, got {0}")]
    InvalidSamplingTolerance(f64),
    #[error("sampling depth must be between 1 and {MAX_SUBDIVISION_DEPTH}, got {0}")]
    InvalidSamplingDepth(u8),
    #[error("sampling point limit must be between 2 and {MAX_SAMPLE_POINTS}, got {0}")]
    InvalidSamplingPointLimit(usize),
    #[error("curve sampling exceeded subdivision depth {max_depth}")]
    SamplingDepthExceeded { max_depth: u8 },
    #[error("curve sampling exceeded point limit {max_points}")]
    SamplingPointLimitExceeded { max_points: usize },
}

/// Exact rational quadratic Bezier segment with endpoint weights fixed at one.
/// Positive finite `weight` keeps its homogeneous denominator positive over
/// the entire closed parameter interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RationalQuadraticBezier2D {
    start: [f64; 2],
    control: [f64; 2],
    end: [f64; 2],
    weight: f64,
}

impl RationalQuadraticBezier2D {
    /// Creates a finite rational quadratic segment with a positive weight.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-finite coordinate or a non-positive or
    /// non-finite weight.
    pub fn new(
        start: [f64; 2],
        control: [f64; 2],
        end: [f64; 2],
        weight: f64,
    ) -> Result<Self, CurveError> {
        validate_point(start, "start")?;
        validate_point(control, "control")?;
        validate_point(end, "end")?;
        if !weight.is_finite() || weight <= 0.0 {
            return Err(CurveError::InvalidWeight(weight));
        }
        Ok(Self {
            start,
            control,
            end,
            weight,
        })
    }

    #[must_use]
    pub const fn start(self) -> [f64; 2] {
        self.start
    }

    #[must_use]
    pub const fn control(self) -> [f64; 2] {
        self.control
    }

    #[must_use]
    pub const fn end(self) -> [f64; 2] {
        self.end
    }

    #[must_use]
    pub const fn weight(self) -> f64 {
        self.weight
    }

    #[must_use]
    pub const fn control_point_ref(segment: SegmentId) -> SketchControlPointRef {
        SketchControlPointRef {
            segment,
            control: 0,
        }
    }

    /// Resolves a control reference for the supplied owning segment.
    ///
    /// # Errors
    ///
    /// Returns an error unless the reference has the expected owner and selects
    /// this curve's only internal control slot.
    pub fn control_point(
        self,
        segment: SegmentId,
        reference: SketchControlPointRef,
    ) -> Result<[f64; 2], CurveError> {
        validate_control_segment(segment, reference.segment)?;
        validate_control_slot(reference.control, 1)?;
        Ok(self.control)
    }

    /// Evaluates the exact rational segment on its trimmed `[0, 1]` domain.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid parameter or non-finite arithmetic.
    pub fn evaluate(self, parameter: f64) -> Result<[f64; 2], CurveError> {
        Ok(self.derivatives(parameter)?.point)
    }

    /// Evaluates the point and first two parameter derivatives.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid parameter or non-finite arithmetic.
    pub fn derivatives(self, parameter: f64) -> Result<CurveDerivatives2D, CurveError> {
        validate_parameter(parameter)?;
        let homogeneous = self.homogeneous_controls()?;
        let one_minus = 1.0 - parameter;

        let value = add3(
            scale3(homogeneous[0], one_minus * one_minus),
            add3(
                scale3(homogeneous[1], 2.0 * one_minus * parameter),
                scale3(homogeneous[2], parameter * parameter),
            ),
        );
        let first_h = scale3(
            add3(
                scale3(sub3(homogeneous[1], homogeneous[0]), one_minus),
                scale3(sub3(homogeneous[2], homogeneous[1]), parameter),
            ),
            2.0,
        );
        let second_h = scale3(
            add3(
                sub3(homogeneous[2], scale3(homogeneous[1], 2.0)),
                homogeneous[0],
            ),
            2.0,
        );
        if !value
            .into_iter()
            .chain(first_h)
            .chain(second_h)
            .all(f64::is_finite)
            || value[2] <= 0.0
        {
            return Err(CurveError::NonFiniteEvaluation);
        }

        let point = if parameter <= 0.0 {
            self.start
        } else if parameter >= 1.0 {
            self.end
        } else {
            [value[0] / value[2], value[1] / value[2]]
        };
        let first = [
            (first_h[0] - point[0] * first_h[2]) / value[2],
            (first_h[1] - point[1] * first_h[2]) / value[2],
        ];
        let second = [
            (second_h[0] - point[0] * second_h[2] - 2.0 * first[0] * first_h[2]) / value[2],
            (second_h[1] - point[1] * second_h[2] - 2.0 * first[1] * first_h[2]) / value[2],
        ];
        let result = CurveDerivatives2D {
            point,
            first,
            second,
        };
        validate_derivatives(result)?;
        Ok(result)
    }

    /// Evaluates signed curvature on the trimmed parameter domain.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid parameter, non-finite arithmetic, or a
    /// degenerate tangent.
    pub fn signed_curvature(self, parameter: f64) -> Result<f64, CurveError> {
        self.derivatives(parameter)?.signed_curvature(parameter)
    }

    /// Returns a deterministic control-hull-bounded polyline, including both
    /// exact endpoints.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid options, non-finite arithmetic, or when the
    /// requested tolerance cannot be met within either explicit budget.
    pub fn sample_adaptive(
        self,
        options: CurveSamplingOptions,
    ) -> Result<Vec<[f64; 2]>, CurveError> {
        sample_piece(BezierPiece::Rational(self.homogeneous_controls()?), options)
    }

    fn homogeneous_controls(self) -> Result<[[f64; 3]; 3], CurveError> {
        let control = [
            self.control[0] * self.weight,
            self.control[1] * self.weight,
            self.weight,
        ];
        if control.into_iter().all(f64::is_finite) {
            Ok([
                [self.start[0], self.start[1], 1.0],
                control,
                [self.end[0], self.end[1], 1.0],
            ])
        } else {
            Err(CurveError::NonFiniteEvaluation)
        }
    }
}

/// Exact cubic Bezier segment on the closed parameter interval `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier2D {
    start: [f64; 2],
    control1: [f64; 2],
    control2: [f64; 2],
    end: [f64; 2],
}

impl CubicBezier2D {
    /// Creates a cubic segment from four finite points.
    ///
    /// # Errors
    ///
    /// Returns an error when any coordinate is non-finite.
    pub fn new(
        start: [f64; 2],
        control1: [f64; 2],
        control2: [f64; 2],
        end: [f64; 2],
    ) -> Result<Self, CurveError> {
        validate_point(start, "start")?;
        validate_point(control1, "first control")?;
        validate_point(control2, "second control")?;
        validate_point(end, "end")?;
        Ok(Self {
            start,
            control1,
            control2,
            end,
        })
    }

    #[must_use]
    pub const fn start(self) -> [f64; 2] {
        self.start
    }

    #[must_use]
    pub const fn control1(self) -> [f64; 2] {
        self.control1
    }

    #[must_use]
    pub const fn control2(self) -> [f64; 2] {
        self.control2
    }

    #[must_use]
    pub const fn end(self) -> [f64; 2] {
        self.end
    }

    /// Produces a stable reference for internal control slot zero or one.
    ///
    /// # Errors
    ///
    /// Returns an error for any other slot.
    pub fn control_point_ref(
        segment: SegmentId,
        control: u8,
    ) -> Result<SketchControlPointRef, CurveError> {
        validate_control_slot(control, 2)?;
        Ok(SketchControlPointRef { segment, control })
    }

    /// Resolves a control reference for the supplied owning segment.
    ///
    /// # Errors
    ///
    /// Returns an error unless the reference has the expected owner and selects
    /// one of this curve's two internal control slots.
    pub fn control_point(
        self,
        segment: SegmentId,
        reference: SketchControlPointRef,
    ) -> Result<[f64; 2], CurveError> {
        validate_control_segment(segment, reference.segment)?;
        match reference.control {
            0 => Ok(self.control1),
            1 => Ok(self.control2),
            control => Err(CurveError::InvalidControlSlot {
                control,
                control_count: 2,
            }),
        }
    }

    /// Evaluates the exact cubic segment on its trimmed `[0, 1]` domain.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid parameter or non-finite arithmetic.
    pub fn evaluate(self, parameter: f64) -> Result<[f64; 2], CurveError> {
        Ok(self.derivatives(parameter)?.point)
    }

    /// Evaluates the point and first two parameter derivatives.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid parameter or non-finite arithmetic.
    pub fn derivatives(self, parameter: f64) -> Result<CurveDerivatives2D, CurveError> {
        validate_parameter(parameter)?;
        let one_minus = 1.0 - parameter;
        let point = if parameter <= 0.0 {
            self.start
        } else if parameter >= 1.0 {
            self.end
        } else {
            let first = lerp2(self.start, self.control1, parameter);
            let second = lerp2(self.control1, self.control2, parameter);
            let third = lerp2(self.control2, self.end, parameter);
            let left = lerp2(first, second, parameter);
            let right = lerp2(second, third, parameter);
            lerp2(left, right, parameter)
        };
        let first = add2(
            scale2(sub2(self.control1, self.start), 3.0 * one_minus * one_minus),
            add2(
                scale2(
                    sub2(self.control2, self.control1),
                    6.0 * one_minus * parameter,
                ),
                scale2(sub2(self.end, self.control2), 3.0 * parameter * parameter),
            ),
        );
        let second = add2(
            scale2(
                add2(sub2(self.control2, scale2(self.control1, 2.0)), self.start),
                6.0 * one_minus,
            ),
            scale2(
                add2(sub2(self.end, scale2(self.control2, 2.0)), self.control1),
                6.0 * parameter,
            ),
        );
        let result = CurveDerivatives2D {
            point,
            first,
            second,
        };
        validate_derivatives(result)?;
        Ok(result)
    }

    /// Evaluates signed curvature on the trimmed parameter domain.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid parameter, non-finite arithmetic, or a
    /// degenerate tangent.
    pub fn signed_curvature(self, parameter: f64) -> Result<f64, CurveError> {
        self.derivatives(parameter)?.signed_curvature(parameter)
    }

    /// Returns a deterministic control-hull-bounded polyline, including both
    /// exact endpoints.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid options, non-finite arithmetic, or when the
    /// requested tolerance cannot be met within either explicit budget.
    pub fn sample_adaptive(
        self,
        options: CurveSamplingOptions,
    ) -> Result<Vec<[f64; 2]>, CurveError> {
        sample_piece(
            BezierPiece::Cubic([self.start, self.control1, self.control2, self.end]),
            options,
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum BezierPiece {
    Rational([[f64; 3]; 3]),
    Cubic([[f64; 2]; 4]),
}

impl BezierPiece {
    fn endpoints(self) -> Result<[[f64; 2]; 2], CurveError> {
        match self {
            Self::Rational(points) => Ok([
                project_homogeneous(points[0])?,
                project_homogeneous(points[2])?,
            ]),
            Self::Cubic(points) => Ok([points[0], points[3]]),
        }
    }

    fn flatness(self) -> Result<f64, CurveError> {
        let [start, end] = self.endpoints()?;
        let flatness = match self {
            Self::Rational(points) => {
                point_segment_distance(project_homogeneous(points[1])?, start, end)?
            }
            Self::Cubic(points) => point_segment_distance(points[1], start, end)?
                .max(point_segment_distance(points[2], start, end)?),
        };
        if flatness.is_finite() {
            Ok(flatness)
        } else {
            Err(CurveError::NonFiniteEvaluation)
        }
    }

    fn split(self) -> (Self, Self) {
        match self {
            Self::Rational(points) => {
                let first = midpoint3(points[0], points[1]);
                let second = midpoint3(points[1], points[2]);
                let middle = midpoint3(first, second);
                (
                    Self::Rational([points[0], first, middle]),
                    Self::Rational([middle, second, points[2]]),
                )
            }
            Self::Cubic(points) => {
                let first = midpoint2(points[0], points[1]);
                let second = midpoint2(points[1], points[2]);
                let third = midpoint2(points[2], points[3]);
                let left_middle = midpoint2(first, second);
                let right_middle = midpoint2(second, third);
                let middle = midpoint2(left_middle, right_middle);
                (
                    Self::Cubic([points[0], first, left_middle, middle]),
                    Self::Cubic([middle, right_middle, third, points[3]]),
                )
            }
        }
    }
}

fn sample_piece(
    piece: BezierPiece,
    options: CurveSamplingOptions,
) -> Result<Vec<[f64; 2]>, CurveError> {
    options.validate()?;
    let [start, end] = piece.endpoints()?;
    let mut points = Vec::new();
    points.push(start);
    sample_piece_recursive(piece, 0, options, &mut points)?;
    points[0] = start;
    *points.last_mut().expect("sampling always retains a start") = end;
    Ok(points)
}

fn sample_piece_recursive(
    piece: BezierPiece,
    depth: u8,
    options: CurveSamplingOptions,
    points: &mut Vec<[f64; 2]>,
) -> Result<(), CurveError> {
    if piece.flatness()? <= options.tolerance {
        if points.len() == options.max_points {
            return Err(CurveError::SamplingPointLimitExceeded {
                max_points: options.max_points,
            });
        }
        points.push(piece.endpoints()?[1]);
        return Ok(());
    }
    if depth == options.max_depth {
        return Err(CurveError::SamplingDepthExceeded {
            max_depth: options.max_depth,
        });
    }
    let (left, right) = piece.split();
    sample_piece_recursive(left, depth + 1, options, points)?;
    sample_piece_recursive(right, depth + 1, options, points)
}

fn validate_point(point: [f64; 2], name: &'static str) -> Result<(), CurveError> {
    if point.into_iter().all(f64::is_finite) {
        Ok(())
    } else {
        Err(CurveError::NonFiniteControlPoint(name))
    }
}

fn validate_parameter(parameter: f64) -> Result<(), CurveError> {
    if parameter.is_finite() && (0.0..=1.0).contains(&parameter) {
        Ok(())
    } else {
        Err(CurveError::InvalidParameter(parameter))
    }
}

fn validate_control_slot(control: u8, control_count: u8) -> Result<(), CurveError> {
    if control < control_count {
        Ok(())
    } else {
        Err(CurveError::InvalidControlSlot {
            control,
            control_count,
        })
    }
}

fn validate_control_segment(expected: SegmentId, actual: SegmentId) -> Result<(), CurveError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CurveError::MismatchedControlSegment { expected, actual })
    }
}

fn validate_derivatives(derivatives: CurveDerivatives2D) -> Result<(), CurveError> {
    if derivatives
        .point
        .into_iter()
        .chain(derivatives.first)
        .chain(derivatives.second)
        .all(f64::is_finite)
    {
        Ok(())
    } else {
        Err(CurveError::NonFiniteEvaluation)
    }
}

fn project_homogeneous(point: [f64; 3]) -> Result<[f64; 2], CurveError> {
    if !point.into_iter().all(f64::is_finite) || point[2] <= 0.0 {
        return Err(CurveError::NonFiniteEvaluation);
    }
    let projected = [point[0] / point[2], point[1] / point[2]];
    validate_point(projected, "projected control").map_err(|_| CurveError::NonFiniteEvaluation)?;
    Ok(projected)
}

fn point_segment_distance(
    point: [f64; 2],
    start: [f64; 2],
    end: [f64; 2],
) -> Result<f64, CurveError> {
    let direction = sub2(end, start);
    let offset = sub2(point, start);
    if !direction.into_iter().chain(offset).all(f64::is_finite) {
        return Err(CurveError::NonFiniteEvaluation);
    }
    let length = direction[0].hypot(direction[1]);
    if length == 0.0 {
        return Ok(offset[0].hypot(offset[1]));
    }
    if !length.is_finite() {
        return Err(CurveError::NonFiniteEvaluation);
    }
    let unit = [direction[0] / length, direction[1] / length];
    let projection = offset[0].mul_add(unit[0], offset[1] * unit[1]);
    let distance = if projection <= 0.0 {
        offset[0].hypot(offset[1])
    } else if projection >= length {
        let end_offset = sub2(point, end);
        end_offset[0].hypot(end_offset[1])
    } else {
        offset[0].mul_add(unit[1], -offset[1] * unit[0]).abs()
    };
    if distance.is_finite() {
        Ok(distance)
    } else {
        Err(CurveError::NonFiniteEvaluation)
    }
}

fn add2(first: [f64; 2], second: [f64; 2]) -> [f64; 2] {
    [first[0] + second[0], first[1] + second[1]]
}

fn sub2(first: [f64; 2], second: [f64; 2]) -> [f64; 2] {
    [first[0] - second[0], first[1] - second[1]]
}

fn scale2(point: [f64; 2], scale: f64) -> [f64; 2] {
    [point[0] * scale, point[1] * scale]
}

fn lerp2(first: [f64; 2], second: [f64; 2], parameter: f64) -> [f64; 2] {
    add2(scale2(first, 1.0 - parameter), scale2(second, parameter))
}

fn midpoint2(first: [f64; 2], second: [f64; 2]) -> [f64; 2] {
    [first[0].midpoint(second[0]), first[1].midpoint(second[1])]
}

fn add3(first: [f64; 3], second: [f64; 3]) -> [f64; 3] {
    [
        first[0] + second[0],
        first[1] + second[1],
        first[2] + second[2],
    ]
}

fn sub3(first: [f64; 3], second: [f64; 3]) -> [f64; 3] {
    [
        first[0] - second[0],
        first[1] - second[1],
        first[2] - second[2],
    ]
}

fn scale3(point: [f64; 3], scale: f64) -> [f64; 3] {
    [point[0] * scale, point[1] * scale, point[2] * scale]
}

fn midpoint3(first: [f64; 3], second: [f64; 3]) -> [f64; 3] {
    [
        first[0].midpoint(second[0]),
        first[1].midpoint(second[1]),
        first[2].midpoint(second[2]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OPTIONS: CurveSamplingOptions = CurveSamplingOptions {
        tolerance: 1.0e-4,
        max_depth: 32,
        max_points: 4_096,
    };

    fn assert_point_near(actual: [f64; 2], expected: [f64; 2], tolerance: f64) {
        assert!((actual[0] - expected[0]).abs() <= tolerance);
        assert!((actual[1] - expected[1]).abs() <= tolerance);
    }

    #[test]
    fn control_references_use_a_separate_segment_local_namespace() {
        let cubic = CubicBezier2D::new([0.0, 0.0], [1.0, 2.0], [3.0, 4.0], [5.0, 6.0]).unwrap();
        assert_eq!(
            RationalQuadraticBezier2D::control_point_ref(17),
            SketchControlPointRef {
                segment: 17,
                control: 0,
            }
        );
        assert_eq!(
            CubicBezier2D::control_point_ref(9, 1).unwrap(),
            SketchControlPointRef {
                segment: 9,
                control: 1,
            }
        );
        assert_eq!(
            CubicBezier2D::control_point_ref(9, 2),
            Err(CurveError::InvalidControlSlot {
                control: 2,
                control_count: 2,
            })
        );
        assert_eq!(
            cubic.control_point(
                9,
                SketchControlPointRef {
                    segment: 8,
                    control: 0,
                },
            ),
            Err(CurveError::MismatchedControlSegment {
                expected: 9,
                actual: 8,
            })
        );
    }

    #[test]
    fn rational_quadratic_interpolates_endpoints_and_exact_quarter_circle() {
        let root_half = 0.5_f64.sqrt();
        let curve =
            RationalQuadraticBezier2D::new([1.0, 0.0], [1.0, 1.0], [0.0, 1.0], root_half).unwrap();

        assert_point_near(curve.evaluate(0.0).unwrap(), [1.0, 0.0], 0.0);
        assert_point_near(curve.evaluate(1.0).unwrap(), [0.0, 1.0], 0.0);
        assert_point_near(
            curve.evaluate(0.5).unwrap(),
            [root_half, root_half],
            1.0e-14,
        );
        assert!((curve.signed_curvature(0.5).unwrap() - 1.0).abs() <= 1.0e-12);
    }

    #[test]
    fn cubic_derivatives_match_closed_form_at_endpoints() {
        let curve = CubicBezier2D::new([1.0, 2.0], [3.0, 5.0], [7.0, 11.0], [13.0, 17.0]).unwrap();

        let start = curve.derivatives(0.0).unwrap();
        assert_point_near(start.point, [1.0, 2.0], 0.0);
        assert_point_near(start.first, [6.0, 9.0], 0.0);
        assert_point_near(start.second, [12.0, 18.0], 0.0);

        let end = curve.derivatives(1.0).unwrap();
        assert_point_near(end.point, [13.0, 17.0], 0.0);
        assert_point_near(end.first, [18.0, 18.0], 0.0);
        assert_point_near(end.second, [12.0, 0.0], 0.0);
    }

    #[test]
    fn collinear_cubic_has_zero_curvature_and_needs_one_chord() {
        let curve = CubicBezier2D::new([0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]).unwrap();

        assert!(curve.signed_curvature(0.4).unwrap().abs() <= 0.0);
        assert_eq!(
            curve.sample_adaptive(SAMPLE_OPTIONS).unwrap(),
            vec![[0.0, 0.0], [3.0, 0.0]]
        );
    }

    #[test]
    fn adaptive_sampling_is_deterministic_and_preserves_endpoints() {
        let curve = CubicBezier2D::new([-2.0, 1.0], [-1.0, 5.0], [3.0, -4.0], [7.0, 2.0]).unwrap();

        let first = curve.sample_adaptive(SAMPLE_OPTIONS).unwrap();
        let second = curve.sample_adaptive(SAMPLE_OPTIONS).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.first(), Some(&curve.start()));
        assert_eq!(first.last(), Some(&curve.end()));
        assert!(first.len() <= SAMPLE_OPTIONS.max_points);
    }

    #[test]
    fn sampling_detects_collinear_control_point_overshoot() {
        let curve = CubicBezier2D::new([0.0, 0.0], [10.0, 0.0], [-10.0, 0.0], [1.0, 0.0]).unwrap();
        let points = curve.sample_adaptive(SAMPLE_OPTIONS).unwrap();
        assert!(points.len() > 2);
    }

    #[test]
    fn invalid_curve_inputs_and_parameters_fail_closed() {
        assert!(matches!(
            RationalQuadraticBezier2D::new([0.0, 0.0], [1.0, 1.0], [2.0, 0.0], 0.0),
            Err(CurveError::InvalidWeight(0.0))
        ));
        assert!(matches!(
            RationalQuadraticBezier2D::new(
                [0.0, 0.0],
                [1.0, 1.0],
                [2.0, 0.0],
                f64::NAN
            ),
            Err(CurveError::InvalidWeight(weight)) if weight.is_nan()
        ));
        assert!(matches!(
            CubicBezier2D::new([0.0, 0.0], [f64::NAN, 0.0], [1.0, 1.0], [2.0, 0.0]),
            Err(CurveError::NonFiniteControlPoint("first control"))
        ));

        let curve = CubicBezier2D::new([0.0, 0.0], [1.0, 1.0], [2.0, 1.0], [3.0, 0.0]).unwrap();
        assert!(matches!(
            curve.evaluate(-0.1),
            Err(CurveError::InvalidParameter(-0.1))
        ));
        assert!(matches!(
            curve.evaluate(f64::NAN),
            Err(CurveError::InvalidParameter(parameter)) if parameter.is_nan()
        ));
    }

    #[test]
    fn degenerate_tangent_and_invalid_sampling_options_fail_closed() {
        let curve = CubicBezier2D::new([0.0, 0.0], [0.0, 0.0], [1.0, 1.0], [2.0, 0.0]).unwrap();
        assert_eq!(
            curve.signed_curvature(0.0),
            Err(CurveError::DegenerateTangent { parameter: 0.0 })
        );
        assert!(matches!(
            curve.sample_adaptive(CurveSamplingOptions {
                tolerance: f64::NAN,
                ..SAMPLE_OPTIONS
            }),
            Err(CurveError::InvalidSamplingTolerance(tolerance)) if tolerance.is_nan()
        ));
        assert_eq!(
            curve.sample_adaptive(CurveSamplingOptions {
                max_depth: 0,
                ..SAMPLE_OPTIONS
            }),
            Err(CurveError::InvalidSamplingDepth(0))
        );
        assert_eq!(
            curve.sample_adaptive(CurveSamplingOptions {
                max_points: 1,
                ..SAMPLE_OPTIONS
            }),
            Err(CurveError::InvalidSamplingPointLimit(1))
        );
    }

    #[test]
    fn adaptive_sampling_reports_each_exhausted_budget_without_overrun() {
        let curve =
            CubicBezier2D::new([0.0, 0.0], [0.0, 100.0], [100.0, 100.0], [100.0, 0.0]).unwrap();

        assert_eq!(
            curve.sample_adaptive(CurveSamplingOptions {
                tolerance: 1.0e-12,
                max_depth: 1,
                max_points: 100,
            }),
            Err(CurveError::SamplingDepthExceeded { max_depth: 1 })
        );
        assert_eq!(
            curve.sample_adaptive(CurveSamplingOptions {
                tolerance: 1.0e-12,
                max_depth: 32,
                max_points: 2,
            }),
            Err(CurveError::SamplingPointLimitExceeded { max_points: 2 })
        );
    }
}
