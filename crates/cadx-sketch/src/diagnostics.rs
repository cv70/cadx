//! Rank, degree-of-freedom, and redundancy diagnostics for solved systems.

use serde::{Deserialize, Serialize};

use crate::{
    constraints::{Constraint, ConstraintGeometry},
    geometry::SketchLoop2D,
    nonlinear,
    residuals::{
        SegmentParameterOffsets, append_intrinsic_residuals, append_nonlinear_constraint_residuals,
        nonlinear_residuals,
    },
};

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

pub(crate) fn residual_layout(
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

pub(crate) fn diagnose_linear_profile(
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

pub(crate) fn diagnose_solution(
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

pub(crate) fn conflicting_constraint_indices(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SketchError, SketchRegion2D, SolverConfig, solve_profile, solve_sketch,
        test_support::circle,
    };

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
}
