//! Solver configuration, solved sketch results, and solve routing entry points.

use serde::{Deserialize, Serialize};

use crate::{
    GEOMETRY_EPSILON, MAX_CONSTRUCTION_SEGMENTS, SegmentId,
    constraints::construction_segment_id,
    constraints::{Constraint, ConstraintGeometry},
    diagnostics::{
        SketchSolveDiagnostic, conflicting_constraint_indices, diagnose_linear_profile,
        diagnose_solution,
    },
    error::SketchError,
    geometry::{SketchLoop2D, SketchRegion2D, SketchSegment2D},
    nonlinear,
    parameters::validate_finite_curve_constraints,
    projection::{constraint_residual, project_constraint},
    residuals::{nonlinear_residuals, rebuild_parameter_segment, sketch_parameters},
};

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
    pub(crate) fn validate(self) -> Result<Self, SketchError> {
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

pub(crate) fn validate_construction(
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

pub(crate) fn solve_nonlinear_sketch(
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
