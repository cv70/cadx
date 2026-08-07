//! Deterministic mate constraints and their forward kinematics.

use serde::{Deserialize, Serialize};

use super::{
    AssemblyError, AssemblyMateId, AssemblyTransform, ComponentOccurrenceId, validate_name,
};

/// One scalar degree-of-freedom limit in the mate kind's declared unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AssemblyMateLimits {
    pub min: f64,
    pub max: f64,
}

/// Supported deterministic assembly motion constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssemblyMateKind {
    Fixed,
    /// Rotation in degrees about an axis expressed in the parent anchor frame.
    Revolute {
        axis: [f64; 3],
        #[serde(default)]
        limits_deg: Option<AssemblyMateLimits>,
    },
    /// Translation in millimeters along an axis expressed in the parent anchor frame.
    Slider {
        axis: [f64; 3],
        #[serde(default)]
        limits_mm: Option<AssemblyMateLimits>,
    },
}

impl AssemblyMateKind {
    fn axis_and_limits(&self) -> Option<([f64; 3], Option<AssemblyMateLimits>)> {
        match *self {
            Self::Fixed => None,
            Self::Revolute { axis, limits_deg } => Some((axis, limits_deg)),
            Self::Slider { axis, limits_mm } => Some((axis, limits_mm)),
        }
    }

    fn motion(&self, state: f64) -> AssemblyTransform {
        match *self {
            Self::Fixed => AssemblyTransform::IDENTITY,
            Self::Revolute { axis, .. } => {
                let angle = state.to_radians();
                let (sine, cosine) = angle.sin_cos();
                let one_minus_cosine = 1.0 - cosine;
                let [x, y, z] = axis;
                AssemblyTransform {
                    translation: [0.0; 3],
                    rotation: [
                        [
                            cosine + x * x * one_minus_cosine,
                            x * y * one_minus_cosine - z * sine,
                            x * z * one_minus_cosine + y * sine,
                        ],
                        [
                            y * x * one_minus_cosine + z * sine,
                            cosine + y * y * one_minus_cosine,
                            y * z * one_minus_cosine - x * sine,
                        ],
                        [
                            z * x * one_minus_cosine - y * sine,
                            z * y * one_minus_cosine + x * sine,
                            cosine + z * z * one_minus_cosine,
                        ],
                    ],
                }
            }
            Self::Slider { axis, .. } => AssemblyTransform {
                translation: axis.map(|component| component * state),
                rotation: AssemblyTransform::IDENTITY.rotation,
            },
        }
    }
}

/// A kinematic constraint that drives one occurrence from its hierarchy parent.
///
/// Anchor frames map mate-frame coordinates into their respective occurrence-local
/// coordinates. The solved child placement is
/// `parent_frame * motion(kind, state) * inverse(child_frame)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssemblyMate {
    pub id: AssemblyMateId,
    pub name: String,
    pub parent_occurrence_id: ComponentOccurrenceId,
    pub child_occurrence_id: ComponentOccurrenceId,
    pub parent_frame: AssemblyTransform,
    pub child_frame: AssemblyTransform,
    pub kind: AssemblyMateKind,
    pub state: f64,
}

impl AssemblyMate {
    /// Resolves the child occurrence's local placement at the current state.
    #[must_use]
    pub fn local_transform(&self) -> AssemblyTransform {
        self.parent_frame
            .compose(self.kind.motion(self.state))
            .compose(self.child_frame.inverse())
    }

    pub(super) fn validate(&self) -> Result<(), AssemblyError> {
        if self.id == 0 {
            return Err(AssemblyError::InvalidMateId(self.id));
        }
        validate_name(&self.name)?;
        self.parent_frame.validate()?;
        self.child_frame.validate()?;
        if !self.state.is_finite() {
            return Err(AssemblyError::NonFiniteMateState { mate: self.id });
        }
        let Some((axis, limits)) = self.kind.axis_and_limits() else {
            if self.state != 0.0 {
                return Err(AssemblyError::FixedMateState { mate: self.id });
            }
            return Ok(());
        };
        if axis.into_iter().any(|component| !component.is_finite()) {
            return Err(AssemblyError::InvalidMateAxis { mate: self.id });
        }
        let length_squared = axis
            .into_iter()
            .map(|component| component * component)
            .sum::<f64>();
        if !length_squared.is_finite() || (length_squared - 1.0).abs() > 1.0e-9 {
            return Err(AssemblyError::InvalidMateAxis { mate: self.id });
        }
        if let Some(limits) = limits {
            if !limits.min.is_finite() || !limits.max.is_finite() || limits.min > limits.max {
                return Err(AssemblyError::InvalidMateLimits { mate: self.id });
            }
            if self.state < limits.min || self.state > limits.max {
                return Err(AssemblyError::MateStateOutsideLimits { mate: self.id });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_anchor_frames_remain_coincident() {
        let parent_frame =
            AssemblyTransform::from_euler_xyz_degrees([12.0, -3.0, 8.0], [20.0, 10.0, 70.0]);
        let child_frame =
            AssemblyTransform::from_euler_xyz_degrees([2.0, 4.0, -1.0], [-15.0, 35.0, 5.0]);
        let mate = AssemblyMate {
            id: 1,
            name: "fixed".into(),
            parent_occurrence_id: 1,
            child_occurrence_id: 2,
            parent_frame,
            child_frame,
            kind: AssemblyMateKind::Fixed,
            state: 0.0,
        };

        assert!(
            mate.local_transform()
                .compose(child_frame)
                .approximately_equals(parent_frame, 1.0e-10)
        );
    }

    #[test]
    fn revolute_motion_supports_an_arbitrary_unit_axis() {
        let inverse_sqrt_two = 0.5_f64.sqrt();
        let mate = AssemblyMate {
            id: 1,
            name: "diagonal hinge".into(),
            parent_occurrence_id: 1,
            child_occurrence_id: 2,
            parent_frame: AssemblyTransform::from_euler_xyz_degrees(
                [4.0, 5.0, 6.0],
                [10.0, 20.0, 30.0],
            ),
            child_frame: AssemblyTransform {
                translation: [2.0, -3.0, 7.0],
                ..AssemblyTransform::IDENTITY
            },
            kind: AssemblyMateKind::Revolute {
                axis: [inverse_sqrt_two, inverse_sqrt_two, 0.0],
                limits_deg: None,
            },
            state: 73.0,
        };
        mate.validate().unwrap();

        let solved_parent_anchor = mate.local_transform().compose(mate.child_frame);
        let expected_parent_anchor = mate.parent_frame.compose(mate.kind.motion(mate.state));
        assert!(solved_parent_anchor.approximately_equals(expected_parent_anchor, 1.0e-10));
        assert!(
            mate.local_transform()
                .transform_point(mate.child_frame.translation)
                .into_iter()
                .zip(mate.parent_frame.translation)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-10)
        );
    }
}
