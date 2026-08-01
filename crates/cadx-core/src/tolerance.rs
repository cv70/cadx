//! Kernel-neutral policies for bounded geometric tolerance escalation.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::diagnostics::AxisAlignedBounds;

/// Controls when a boolean adapter may retry and heal topology.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanHealingPolicy {
    Disabled,
    #[default]
    AfterFailure,
}

/// Bounded tolerance policy shared by kernel adapters and diagnostic consumers.
///
/// All distances are millimeters. The nominal tolerance is the larger of
/// `absolute_mm` and `model_scale * relative`, capped by `maximum_mm`.
/// Subsequent attempts multiply that value by `retry_multiplier` without ever
/// exceeding the cap or `max_attempts`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BooleanTolerancePolicy {
    pub absolute_mm: f64,
    pub relative: f64,
    pub maximum_mm: f64,
    pub retry_multiplier: f64,
    pub max_attempts: u8,
    pub healing: BooleanHealingPolicy,
}

impl Default for BooleanTolerancePolicy {
    fn default() -> Self {
        Self {
            absolute_mm: 0.05,
            relative: 1.0e-9,
            maximum_mm: 0.2,
            retry_multiplier: 2.0,
            max_attempts: 3,
            healing: BooleanHealingPolicy::AfterFailure,
        }
    }
}

impl BooleanTolerancePolicy {
    /// Creates a single-attempt policy for callers that require a fixed
    /// tolerance, including compatibility with the original Truck adapter.
    ///
    /// # Errors
    ///
    /// Returns an error when `tolerance_mm` is not finite and positive.
    pub fn uniform(tolerance_mm: f64) -> Result<Self, BooleanTolerancePolicyError> {
        let policy = Self {
            absolute_mm: tolerance_mm,
            relative: 0.0,
            maximum_mm: tolerance_mm,
            retry_multiplier: 1.0,
            max_attempts: 1,
            healing: BooleanHealingPolicy::Disabled,
        };
        policy.validate()?;
        Ok(policy)
    }

    /// Validates configuration before it reaches a concrete geometry kernel.
    ///
    /// # Errors
    ///
    /// Returns a stable field-level error for non-finite, non-positive, or
    /// internally inconsistent values.
    pub fn validate(self) -> Result<(), BooleanTolerancePolicyError> {
        if !self.absolute_mm.is_finite() || self.absolute_mm <= 0.0 {
            return Err(BooleanTolerancePolicyError::InvalidAbsolute);
        }
        if !self.relative.is_finite() || self.relative < 0.0 {
            return Err(BooleanTolerancePolicyError::InvalidRelative);
        }
        if !self.maximum_mm.is_finite() || self.maximum_mm < self.absolute_mm {
            return Err(BooleanTolerancePolicyError::InvalidMaximum);
        }
        if !self.retry_multiplier.is_finite() || self.retry_multiplier < 1.0 {
            return Err(BooleanTolerancePolicyError::InvalidRetryMultiplier);
        }
        if self.max_attempts == 0 {
            return Err(BooleanTolerancePolicyError::InvalidAttemptCount);
        }
        Ok(())
    }

    /// Returns the deterministic, deduplicated tolerance sequence for two
    /// finite operand bounds.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy or supplied bounds are invalid.
    pub fn attempt_tolerances(
        self,
        left: AxisAlignedBounds,
        right: AxisAlignedBounds,
    ) -> Result<Vec<f64>, BooleanTolerancePolicyError> {
        self.validate()?;
        if !left.is_finite() || !right.is_finite() {
            return Err(BooleanTolerancePolicyError::InvalidBounds);
        }

        let model_scale = (0..3)
            .map(|axis| left.min[axis].min(right.min[axis])..=left.max[axis].max(right.max[axis]))
            .map(|range| *range.end() - *range.start())
            .fold(0.0_f64, f64::max);
        let mut current = self
            .absolute_mm
            .max(model_scale * self.relative)
            .min(self.maximum_mm);
        let mut attempts = Vec::with_capacity(usize::from(self.max_attempts));
        for _ in 0..self.max_attempts {
            if attempts.last().copied() != Some(current) {
                attempts.push(current);
            }
            if current >= self.maximum_mm {
                break;
            }
            current = (current * self.retry_multiplier).min(self.maximum_mm);
        }
        Ok(attempts)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BooleanTolerancePolicyError {
    #[error("absolute boolean tolerance must be finite and greater than zero")]
    InvalidAbsolute,
    #[error("relative boolean tolerance must be finite and non-negative")]
    InvalidRelative,
    #[error("maximum boolean tolerance must be finite and at least the absolute tolerance")]
    InvalidMaximum,
    #[error("boolean retry multiplier must be finite and at least one")]
    InvalidRetryMultiplier,
    #[error("boolean tolerance policy must allow at least one attempt")]
    InvalidAttemptCount,
    #[error("boolean tolerance resolution requires finite ordered operand bounds")]
    InvalidBounds,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(min: [f64; 3], max: [f64; 3]) -> AxisAlignedBounds {
        AxisAlignedBounds { min, max }
    }

    #[test]
    fn boolean_policy_resolves_a_bounded_deterministic_sequence() {
        let policy = BooleanTolerancePolicy {
            absolute_mm: 0.001,
            relative: 0.001,
            maximum_mm: 0.5,
            retry_multiplier: 2.0,
            max_attempts: 4,
            healing: BooleanHealingPolicy::AfterFailure,
        };
        let attempts = policy
            .attempt_tolerances(bounds([0.0; 3], [10.0; 3]), bounds([5.0; 3], [25.0; 3]))
            .unwrap();
        assert_eq!(attempts, vec![0.025, 0.05, 0.1, 0.2]);
    }

    #[test]
    fn boolean_policy_rejects_invalid_configuration_and_bounds() {
        let invalid = BooleanTolerancePolicy {
            max_attempts: 0,
            ..BooleanTolerancePolicy::default()
        };
        assert_eq!(
            invalid.validate(),
            Err(BooleanTolerancePolicyError::InvalidAttemptCount)
        );
        let invalid_bounds = bounds([1.0, 0.0, 0.0], [0.0, 1.0, 1.0]);
        assert_eq!(
            BooleanTolerancePolicy::default()
                .attempt_tolerances(invalid_bounds, bounds([0.0; 3], [1.0; 3])),
            Err(BooleanTolerancePolicyError::InvalidBounds)
        );
    }

    #[test]
    fn uniform_policy_preserves_one_exact_attempt() {
        let policy = BooleanTolerancePolicy::uniform(0.005).unwrap();
        let attempts = policy
            .attempt_tolerances(bounds([0.0; 3], [1.0; 3]), bounds([0.0; 3], [1.0; 3]))
            .unwrap();
        assert_eq!(attempts, vec![0.005]);
        assert_eq!(policy.healing, BooleanHealingPolicy::Disabled);
    }
}
