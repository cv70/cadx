//! Bounded Levenberg-Marquardt solver for exact sketch constraints.

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NonlinearSolution {
    pub parameters: Vec<f64>,
    pub residual: f64,
    pub iterations: u32,
    pub converged: bool,
}

pub(crate) fn solve(
    initial: &[f64],
    max_iterations: u32,
    tolerance: f64,
    relaxation: f64,
    residuals: impl Fn(&[f64]) -> Vec<f64>,
) -> NonlinearSolution {
    let mut parameters = initial.to_vec();
    let mut current = residuals(&parameters);
    let mut current_cost = squared_norm(&current);
    let mut current_residual = max_abs(&current);
    if current_residual <= tolerance {
        return NonlinearSolution {
            parameters,
            residual: current_residual,
            iterations: 0,
            converged: true,
        };
    }

    let parameter_count = parameters.len();
    let residual_count = current.len();
    let mut damping = 1.0e-3;
    for iteration in 1..=max_iterations {
        let jacobian = finite_difference_jacobian(&parameters, &current, &residuals);

        let mut normal = vec![0.0; parameter_count * parameter_count];
        let mut gradient = vec![0.0; parameter_count];
        for row in 0..residual_count {
            for column in 0..parameter_count {
                let derivative = jacobian[row * parameter_count + column];
                gradient[column] += derivative * current[row];
                for other in column..parameter_count {
                    normal[column * parameter_count + other] +=
                        derivative * jacobian[row * parameter_count + other];
                }
            }
        }
        for row in 0..parameter_count {
            for column in 0..row {
                normal[row * parameter_count + column] = normal[column * parameter_count + row];
            }
            let diagonal = normal[row * parameter_count + row].abs() + 1.0;
            normal[row * parameter_count + row] += damping * diagonal;
            gradient[row] = -gradient[row];
        }

        let Some(delta) = solve_dense(normal, gradient, parameter_count) else {
            damping *= 10.0;
            if damping > 1.0e16 {
                break;
            }
            continue;
        };
        let candidate = parameters
            .iter()
            .zip(delta)
            .map(|(value, delta)| delta.mul_add(relaxation, *value))
            .collect::<Vec<_>>();
        let candidate_residuals = residuals(&candidate);
        let candidate_cost = squared_norm(&candidate_residuals);
        if candidate_cost.is_finite() && candidate_cost < current_cost {
            parameters = candidate;
            current = candidate_residuals;
            current_cost = candidate_cost;
            current_residual = max_abs(&current);
            damping = (damping * 0.25).max(1.0e-12);
            if current_residual <= tolerance {
                return NonlinearSolution {
                    parameters,
                    residual: current_residual,
                    iterations: iteration,
                    converged: true,
                };
            }
        } else {
            damping *= 10.0;
            if damping > 1.0e16 {
                break;
            }
        }
    }

    NonlinearSolution {
        parameters,
        residual: current_residual,
        iterations: max_iterations,
        converged: false,
    }
}

pub(crate) fn finite_difference_jacobian(
    parameters: &[f64],
    current: &[f64],
    residuals: &impl Fn(&[f64]) -> Vec<f64>,
) -> Vec<f64> {
    let parameter_count = parameters.len();
    let residual_count = current.len();
    let mut jacobian = vec![0.0; residual_count * parameter_count];
    for column in 0..parameter_count {
        let mut perturbed = parameters.to_vec();
        let step = f64::EPSILON.sqrt() * (parameters[column].abs() + 1.0);
        perturbed[column] += step;
        let values = residuals(&perturbed);
        if values.len() != residual_count {
            return Vec::new();
        }
        for row in 0..residual_count {
            jacobian[row * parameter_count + column] = (values[row] - current[row]) / step;
        }
    }
    jacobian
}

/// Incremental numerical row-rank estimator used for sketch DOF diagnostics.
/// Rows are accepted in declaration order so later dependent constraints can
/// be reported deterministically as redundant.
pub(crate) struct RowRank {
    columns: usize,
    basis: Vec<Vec<f64>>,
}

impl RowRank {
    pub(crate) const fn new(columns: usize) -> Self {
        Self {
            columns,
            basis: Vec::new(),
        }
    }

    pub(crate) fn rank(&self) -> usize {
        self.basis.len()
    }

    pub(crate) fn add_rows(&mut self, jacobian: &[f64], start: usize, end: usize) -> usize {
        let previous = self.rank();
        for row in start..end {
            let offset = row * self.columns;
            self.add_row(&jacobian[offset..offset + self.columns]);
        }
        self.rank() - previous
    }

    fn add_row(&mut self, row: &[f64]) {
        const ABSOLUTE_TOLERANCE: f64 = 1.0e-10;
        const RELATIVE_TOLERANCE: f64 = 1.0e-7;

        let original_norm = squared_norm(row).sqrt();
        if !original_norm.is_finite() || original_norm <= ABSOLUTE_TOLERANCE {
            return;
        }
        let mut candidate = row.to_vec();
        // A second orthogonalization pass keeps nearly dependent geometric
        // equations from becoming false degrees of freedom through roundoff.
        for _ in 0..2 {
            for basis in &self.basis {
                let projection = candidate
                    .iter()
                    .zip(basis)
                    .map(|(value, basis)| value * basis)
                    .sum::<f64>();
                for (value, basis) in candidate.iter_mut().zip(basis) {
                    *value -= projection * basis;
                }
            }
        }
        let remaining_norm = squared_norm(&candidate).sqrt();
        if remaining_norm <= ABSOLUTE_TOLERANCE.max(original_norm * RELATIVE_TOLERANCE) {
            return;
        }
        for value in &mut candidate {
            *value /= remaining_norm;
        }
        self.basis.push(candidate);
    }
}

fn squared_norm(values: &[f64]) -> f64 {
    values.iter().map(|value| value * value).sum()
}

fn max_abs(values: &[f64]) -> f64 {
    values.iter().map(|value| value.abs()).fold(0.0, f64::max)
}

fn solve_dense(mut matrix: Vec<f64>, mut right: Vec<f64>, size: usize) -> Option<Vec<f64>> {
    for pivot_column in 0..size {
        let pivot_row = (pivot_column..size).max_by(|left, right_row| {
            matrix[*left * size + pivot_column]
                .abs()
                .total_cmp(&matrix[*right_row * size + pivot_column].abs())
        })?;
        if matrix[pivot_row * size + pivot_column].abs() <= f64::EPSILON {
            return None;
        }
        if pivot_row != pivot_column {
            for column in 0..size {
                matrix.swap(pivot_row * size + column, pivot_column * size + column);
            }
            right.swap(pivot_row, pivot_column);
        }
        for row in (pivot_column + 1)..size {
            let factor =
                matrix[row * size + pivot_column] / matrix[pivot_column * size + pivot_column];
            matrix[row * size + pivot_column] = 0.0;
            for column in (pivot_column + 1)..size {
                matrix[row * size + column] -= factor * matrix[pivot_column * size + column];
            }
            right[row] -= factor * right[pivot_column];
        }
    }

    let mut solution = vec![0.0; size];
    for row in (0..size).rev() {
        let known = ((row + 1)..size)
            .map(|column| matrix[row * size + column] * solution[column])
            .sum::<f64>();
        solution[row] = (right[row] - known) / matrix[row * size + row];
    }
    solution
        .iter()
        .all(|value| value.is_finite())
        .then_some(solution)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_an_underdetermined_nonlinear_system_deterministically() {
        let residuals = |parameters: &[f64]| {
            vec![
                parameters[0].mul_add(parameters[0], parameters[1] * parameters[1]) - 25.0,
                parameters[1] - 4.0,
            ]
        };
        let first = solve(&[2.0, 2.0], 128, 1.0e-10, 1.0, residuals);
        let second = solve(&[2.0, 2.0], 128, 1.0e-10, 1.0, residuals);
        assert!(first.converged);
        assert_eq!(first, second);
        assert!((first.parameters[0] - 3.0).abs() < 1.0e-8);
        assert!((first.parameters[1] - 4.0).abs() < 1.0e-8);
    }

    #[test]
    fn reports_conflicting_residuals_without_returning_success() {
        let result = solve(&[0.0], 32, 1.0e-10, 1.0, |parameters| {
            vec![parameters[0], parameters[0] - 1.0]
        });
        assert!(!result.converged);
        assert!(result.residual >= 0.49);
    }
}
