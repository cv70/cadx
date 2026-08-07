//! Persistent constraint and dimension types plus sketch entity resolution.

use serde::{Deserialize, Serialize};

use crate::{
    PointId, SegmentId,
    geometry::{SketchLoop2D, SketchSegment2D},
};

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
pub(crate) struct ConstraintGeometry<'a> {
    pub(crate) profile: &'a SketchLoop2D,
    pub(crate) construction: &'a [SketchSegment2D],
}

impl<'a> ConstraintGeometry<'a> {
    pub(crate) fn point_count(self) -> usize {
        self.profile
            .segments
            .len()
            .saturating_add(self.construction.len().saturating_mul(2))
    }

    pub(crate) fn segment_count(self) -> usize {
        self.profile
            .segments
            .len()
            .saturating_add(self.construction.len())
    }

    pub(crate) fn segment(self, id: SegmentId) -> &'a SketchSegment2D {
        let index = usize::try_from(id).expect("validated segment id");
        if index < self.profile.segments.len() {
            &self.profile.segments[index]
        } else {
            &self.construction[index - self.profile.segments.len()]
        }
    }

    pub(crate) fn segment_point_ids(self, id: SegmentId) -> (PointId, PointId) {
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

    pub(crate) fn point(self, id: PointId) -> [f64; 2] {
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

    pub(crate) const fn requires_curve(&self) -> bool {
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

    pub(crate) const fn requires_nonlinear(&self) -> bool {
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
