use std::panic::{AssertUnwindSafe, catch_unwind};

use cadx_core::{
    diagnostics::{
        AxisAlignedBounds, BooleanAttemptDiagnostic, BooleanDiagnostic, BooleanFailureReason,
        BooleanFailureStage, BooleanHealingStatus,
    },
    domain::{BooleanOperation, Feature, FeatureId},
    kernel::KernelError,
    tolerance::{BooleanHealingPolicy, BooleanTolerancePolicy},
};
use truck_modeling::Solid;

use crate::{
    panic_message, solid_bounds, supports_geometric_consistency_check,
    topology::{self, BooleanSourceGeometry, NamedFace, NamedSolid},
};

mod contact;

pub(crate) enum NonCrossingIntersection {
    Solid(Solid),
    Empty,
}

pub(crate) fn resolve_non_crossing_intersection(
    left: &Solid,
    right: &Solid,
    tolerance: f64,
) -> Option<Result<NonCrossingIntersection, String>> {
    contact::resolve(left, right, BooleanOperation::Intersect, tolerance).map(|result| {
        result.map(|resolution| match resolution {
            contact::ContactResolution::Solid { solid, .. } => {
                NonCrossingIntersection::Solid(solid)
            }
            contact::ContactResolution::Empty => NonCrossingIntersection::Empty,
        })
    })
}

#[derive(Debug)]
struct AttemptFailure {
    stage: BooleanFailureStage,
    reason: BooleanFailureReason,
    detail: String,
    operand_healing: BooleanHealingStatus,
    result_healing: BooleanHealingStatus,
}

#[derive(Debug)]
struct AttemptResult {
    solid: Solid,
    result_healing: BooleanHealingStatus,
    right_lineage: BooleanSourceGeometry,
}

#[derive(Debug, Clone, Copy)]
struct OperandContext<'a> {
    ids: [FeatureId; 2],
    solids: [&'a NamedSolid; 2],
    bounds: [AxisAlignedBounds; 2],
}

impl AttemptFailure {
    fn evidence(&self, tolerance_mm: f64) -> BooleanAttemptDiagnostic {
        BooleanAttemptDiagnostic {
            tolerance_mm,
            stage: self.stage,
            reason: self.reason,
            operand_healing: self.operand_healing,
            result_healing: self.result_healing,
        }
    }
}

pub(crate) fn evaluate(
    feature: &Feature,
    operation: BooleanOperation,
    operands: [FeatureId; 2],
    left: &NamedSolid,
    right: &NamedSolid,
    policy: BooleanTolerancePolicy,
) -> Result<(Solid, Vec<NamedFace>), KernelError> {
    let left_bounds = validate_operand(feature, operation, operands, left, "left", policy)?;
    let right_bounds = validate_operand(feature, operation, operands, right, "right", policy)?;
    let tolerances = policy
        .attempt_tolerances(left_bounds, right_bounds)
        .map_err(|error| {
            diagnostic(
                feature,
                operation,
                operands,
                policy.absolute_mm,
                left_bounds.into(),
                right_bounds.into(),
                BooleanFailureStage::OperandValidation,
                BooleanFailureReason::InvalidOperandGeometry,
                Vec::new(),
                error.to_string(),
            )
        })?;
    let nominal_tolerance = tolerances[0];
    let disjoint = left_bounds.is_disjoint_from(right_bounds, nominal_tolerance);
    let operand_context = OperandContext {
        ids: operands,
        solids: [left, right],
        bounds: [left_bounds, right_bounds],
    };
    if disjoint {
        return evaluate_disjoint(feature, operation, operand_context, nominal_tolerance);
    }

    let mut evidence = Vec::with_capacity(tolerances.len());
    let mut last_failure = None;
    for (attempt_index, tolerance) in tolerances.into_iter().enumerate() {
        let heal_operands =
            attempt_index > 0 && policy.healing == BooleanHealingPolicy::AfterFailure;
        match evaluate_attempt(
            feature,
            operation,
            [left, right],
            tolerance,
            heal_operands,
            policy.healing,
        ) {
            Ok(result) => return Ok(result),
            Err(failure) => {
                let stop_retrying = matches!(
                    failure.reason,
                    BooleanFailureReason::KernelPanic | BooleanFailureReason::EmptyResult
                );
                evidence.push(failure.evidence(tolerance));
                last_failure = Some((tolerance, failure));
                if stop_retrying {
                    break;
                }
            }
        }
    }

    let Some((tolerance, failure)) = last_failure else {
        return Err(diagnostic(
            feature,
            operation,
            operands,
            policy.absolute_mm,
            Some(left_bounds),
            Some(right_bounds),
            BooleanFailureStage::KernelOperation,
            BooleanFailureReason::KernelRejected,
            evidence,
            "validated boolean policy produced no kernel attempts".into(),
        )
        .into());
    };
    Err(diagnostic(
        feature,
        operation,
        operands,
        tolerance,
        Some(left_bounds),
        Some(right_bounds),
        failure.stage,
        failure.reason,
        evidence,
        failure.detail,
    )
    .into())
}

fn evaluate_disjoint(
    feature: &Feature,
    operation: BooleanOperation,
    operands: OperandContext<'_>,
    tolerance: f64,
) -> Result<(Solid, Vec<NamedFace>), KernelError> {
    let [left, right] = operands.solids;
    let [left_bounds, right_bounds] = operands.bounds;
    if operation == BooleanOperation::Intersect {
        let attempt = BooleanAttemptDiagnostic {
            tolerance_mm: tolerance,
            stage: BooleanFailureStage::BroadPhase,
            reason: BooleanFailureReason::DisjointOperands,
            operand_healing: BooleanHealingStatus::NotAttempted,
            result_healing: BooleanHealingStatus::NotAttempted,
        };
        return Err(diagnostic(
            feature,
            operation,
            operands.ids,
            tolerance,
            Some(left_bounds),
            Some(right_bounds),
            BooleanFailureStage::BroadPhase,
            BooleanFailureReason::DisjointOperands,
            vec![attempt],
            "operand bounds do not overlap within the nominal modeling tolerance".into(),
        )
        .into());
    }

    let result = match operation {
        BooleanOperation::Subtract => left.solid.clone(),
        BooleanOperation::Union => {
            let mut boundaries = left.solid.boundaries().clone();
            boundaries.extend(right.solid.boundaries().iter().cloned());
            Solid::try_new(boundaries).map_err(|error| {
                diagnostic(
                    feature,
                    operation,
                    operands.ids,
                    tolerance,
                    Some(left_bounds),
                    Some(right_bounds),
                    BooleanFailureStage::ResultValidation,
                    BooleanFailureReason::InvalidResultTopology,
                    vec![BooleanAttemptDiagnostic {
                        tolerance_mm: tolerance,
                        stage: BooleanFailureStage::ResultValidation,
                        reason: BooleanFailureReason::InvalidResultTopology,
                        operand_healing: BooleanHealingStatus::NotAttempted,
                        result_healing: BooleanHealingStatus::NotAttempted,
                    }],
                    format!("disjoint operand shells could not form a valid solid: {error}"),
                )
            })?
        }
        BooleanOperation::Intersect => unreachable!("handled above"),
    };
    validate_result(&result).map_err(|failure| {
        diagnostic(
            feature,
            operation,
            operands.ids,
            tolerance,
            Some(left_bounds),
            Some(right_bounds),
            failure.stage,
            failure.reason,
            vec![failure.evidence(tolerance)],
            failure.detail,
        )
    })?;
    let faces = topology::name_boolean_faces(
        feature,
        &result,
        [left, right],
        [
            BooleanSourceGeometry::Identity,
            BooleanSourceGeometry::Identity,
        ],
        tolerance,
    )
    .map_err(|error| {
        diagnostic(
            feature,
            operation,
            operands.ids,
            tolerance,
            Some(left_bounds),
            Some(right_bounds),
            BooleanFailureStage::TopologyNaming,
            BooleanFailureReason::TopologyNamingFailed,
            vec![BooleanAttemptDiagnostic {
                tolerance_mm: tolerance,
                stage: BooleanFailureStage::TopologyNaming,
                reason: BooleanFailureReason::TopologyNamingFailed,
                operand_healing: BooleanHealingStatus::NotAttempted,
                result_healing: BooleanHealingStatus::NotAttempted,
            }],
            error.to_string(),
        )
    })?;
    Ok((result, faces))
}

fn evaluate_attempt(
    feature: &Feature,
    operation: BooleanOperation,
    operands: [&NamedSolid; 2],
    tolerance: f64,
    heal_operands: bool,
    healing_policy: BooleanHealingPolicy,
) -> Result<(Solid, Vec<NamedFace>), AttemptFailure> {
    let healed;
    let solids = if heal_operands {
        healed = [
            heal_solid(&operands[0].solid, tolerance),
            heal_solid(&operands[1].solid, tolerance),
        ];
        let [left, right] = healed;
        match (left, right) {
            (Ok(left), Ok(right)) => [left, right],
            (left, right) => {
                let details = [left.err(), right.err()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(AttemptFailure {
                    stage: BooleanFailureStage::TopologyHealing,
                    reason: BooleanFailureReason::HealingFailed,
                    detail: format!("operand topology healing failed: {details}"),
                    operand_healing: BooleanHealingStatus::Failed,
                    result_healing: BooleanHealingStatus::NotAttempted,
                });
            }
        }
    } else {
        [operands[0].solid.clone(), operands[1].solid.clone()]
    };
    let operand_healing = if heal_operands {
        BooleanHealingStatus::Applied
    } else {
        BooleanHealingStatus::NotAttempted
    };

    let contact_resolution = resolve_non_crossing_contact(
        &solids,
        operation,
        tolerance,
        healing_policy,
        operand_healing,
    );
    let attempt = if let Some(resolution) = contact_resolution {
        resolution?
    } else {
        let operation_result = catch_unwind(AssertUnwindSafe(|| match operation {
            BooleanOperation::Union => truck_shapeops::or(&solids[0], &solids[1], tolerance),
            BooleanOperation::Intersect => truck_shapeops::and(&solids[0], &solids[1], tolerance),
            BooleanOperation::Subtract => {
                let mut complement = solids[1].clone();
                complement.not();
                truck_shapeops::and(&solids[0], &complement, tolerance)
            }
        }));
        match operation_result {
            Ok(Some(solid)) => AttemptResult {
                solid,
                result_healing: BooleanHealingStatus::NotAttempted,
                right_lineage: BooleanSourceGeometry::Identity,
            },
            Ok(None) => recover_non_crossing_contact(
                &solids,
                operation,
                tolerance,
                healing_policy,
                AttemptFailure {
                    stage: BooleanFailureStage::KernelOperation,
                    reason: BooleanFailureReason::KernelRejected,
                    detail: "Truck returned no solid result".into(),
                    operand_healing,
                    result_healing: BooleanHealingStatus::NotAttempted,
                },
            )?,
            Err(payload) => recover_non_crossing_contact(
                &solids,
                operation,
                tolerance,
                healing_policy,
                AttemptFailure {
                    stage: BooleanFailureStage::KernelOperation,
                    reason: BooleanFailureReason::KernelPanic,
                    detail: format!("Truck panicked: {}", panic_message(payload.as_ref())),
                    operand_healing,
                    result_healing: BooleanHealingStatus::NotAttempted,
                },
            )?,
        }
    };
    let AttemptResult {
        solid: mut result,
        mut result_healing,
        right_lineage,
    } = attempt;

    if let Err(mut failure) = validate_result_caught(&result) {
        if healing_policy == BooleanHealingPolicy::AfterFailure {
            match heal_solid(&result, tolerance).and_then(|healed| {
                validate_result_caught(&healed)
                    .map(|()| healed)
                    .map_err(|error| error.detail)
            }) {
                Ok(healed) => {
                    result = healed;
                    result_healing = BooleanHealingStatus::Applied;
                }
                Err(healing_error) => {
                    failure.detail = format!(
                        "{}; result topology healing failed: {healing_error}",
                        failure.detail
                    );
                    failure.operand_healing = operand_healing;
                    failure.result_healing = BooleanHealingStatus::Failed;
                    return Err(failure);
                }
            }
        } else {
            failure.operand_healing = operand_healing;
            return Err(failure);
        }
    }

    let naming = catch_unwind(AssertUnwindSafe(|| {
        topology::name_boolean_faces(
            feature,
            &result,
            operands,
            [BooleanSourceGeometry::Identity, right_lineage],
            tolerance,
        )
    }));
    match naming {
        Ok(Ok(faces)) => Ok((result, faces)),
        Ok(Err(error)) => Err(AttemptFailure {
            stage: BooleanFailureStage::TopologyNaming,
            reason: BooleanFailureReason::TopologyNamingFailed,
            detail: error.to_string(),
            operand_healing,
            result_healing,
        }),
        Err(payload) => Err(AttemptFailure {
            stage: BooleanFailureStage::TopologyNaming,
            reason: BooleanFailureReason::KernelPanic,
            detail: format!(
                "Truck panicked while naming boolean topology: {}",
                panic_message(payload.as_ref())
            ),
            operand_healing,
            result_healing,
        }),
    }
}

fn recover_non_crossing_contact(
    operands: &[Solid; 2],
    operation: BooleanOperation,
    tolerance: f64,
    healing_policy: BooleanHealingPolicy,
    fallback: AttemptFailure,
) -> Result<AttemptResult, AttemptFailure> {
    resolve_non_crossing_contact(
        operands,
        operation,
        tolerance,
        healing_policy,
        fallback.operand_healing,
    )
    .unwrap_or(Err(fallback))
}

fn resolve_non_crossing_contact(
    operands: &[Solid; 2],
    operation: BooleanOperation,
    tolerance: f64,
    healing_policy: BooleanHealingPolicy,
    operand_healing: BooleanHealingStatus,
) -> Option<Result<AttemptResult, AttemptFailure>> {
    if healing_policy != BooleanHealingPolicy::AfterFailure {
        return None;
    }
    match contact::resolve(&operands[0], &operands[1], operation, tolerance) {
        None => None,
        Some(Ok(contact::ContactResolution::Solid {
            solid,
            right_lineage,
        })) => Some(Ok(AttemptResult {
            solid,
            result_healing: BooleanHealingStatus::Applied,
            right_lineage,
        })),
        Some(Ok(contact::ContactResolution::Empty)) => Some(Err(AttemptFailure {
            stage: BooleanFailureStage::ResultValidation,
            reason: BooleanFailureReason::EmptyResult,
            detail: "non-crossing contact classification proved that the boolean result is empty"
                .into(),
            operand_healing,
            result_healing: BooleanHealingStatus::Applied,
        })),
        Some(Err(detail)) => Some(Err(AttemptFailure {
            stage: BooleanFailureStage::TopologyHealing,
            reason: BooleanFailureReason::HealingFailed,
            detail,
            operand_healing,
            result_healing: BooleanHealingStatus::Failed,
        })),
    }
}

fn validate_operand(
    feature: &Feature,
    operation: BooleanOperation,
    operands: [FeatureId; 2],
    operand: &NamedSolid,
    label: &str,
    policy: BooleanTolerancePolicy,
) -> Result<AxisAlignedBounds, KernelError> {
    if let Err(error) = Solid::try_new(operand.solid.boundaries().clone()) {
        return Err(diagnostic(
            feature,
            operation,
            operands,
            policy.absolute_mm,
            None,
            None,
            BooleanFailureStage::OperandValidation,
            BooleanFailureReason::InvalidOperandTopology,
            Vec::new(),
            format!("{label} operand is not a closed manifold solid: {error}"),
        )
        .into());
    }
    let Some(bounds) = solid_bounds(&operand.solid) else {
        return Err(diagnostic(
            feature,
            operation,
            operands,
            policy.absolute_mm,
            None,
            None,
            BooleanFailureStage::OperandValidation,
            BooleanFailureReason::InvalidOperandGeometry,
            Vec::new(),
            format!("{label} operand has non-finite vertex geometry"),
        )
        .into());
    };
    let consistency = catch_unwind(AssertUnwindSafe(|| {
        !supports_geometric_consistency_check(&operand.solid)
            || operand.solid.is_geometric_consistent()
    }));
    match consistency {
        Ok(true) => {}
        Ok(false) => {
            return Err(diagnostic(
                feature,
                operation,
                operands,
                policy.absolute_mm,
                None,
                None,
                BooleanFailureStage::OperandValidation,
                BooleanFailureReason::InvalidOperandGeometry,
                Vec::new(),
                format!("{label} operand has inconsistent edge or surface geometry"),
            )
            .into());
        }
        Err(payload) => {
            return Err(diagnostic(
                feature,
                operation,
                operands,
                policy.absolute_mm,
                None,
                None,
                BooleanFailureStage::OperandValidation,
                BooleanFailureReason::KernelPanic,
                Vec::new(),
                format!(
                    "Truck panicked while validating the {label} operand: {}",
                    panic_message(payload.as_ref())
                ),
            )
            .into());
        }
    }
    Ok(bounds)
}

fn validate_result_caught(result: &Solid) -> Result<(), AttemptFailure> {
    catch_unwind(AssertUnwindSafe(|| validate_result(result))).unwrap_or_else(|payload| {
        Err(AttemptFailure {
            stage: BooleanFailureStage::ResultValidation,
            reason: BooleanFailureReason::KernelPanic,
            detail: format!(
                "Truck panicked while validating the boolean result: {}",
                panic_message(payload.as_ref())
            ),
            operand_healing: BooleanHealingStatus::NotAttempted,
            result_healing: BooleanHealingStatus::NotAttempted,
        })
    })
}

fn validate_result(result: &Solid) -> Result<(), AttemptFailure> {
    let failure = |reason, detail| AttemptFailure {
        stage: BooleanFailureStage::ResultValidation,
        reason,
        detail,
        operand_healing: BooleanHealingStatus::NotAttempted,
        result_healing: BooleanHealingStatus::NotAttempted,
    };
    if result.boundaries().is_empty() {
        return Err(failure(
            BooleanFailureReason::EmptyResult,
            "the boolean produced an empty solid".into(),
        ));
    }
    Solid::try_new(result.boundaries().clone()).map_err(|error| {
        failure(
            BooleanFailureReason::InvalidResultTopology,
            format!("Truck produced invalid solid topology: {error}"),
        )
    })?;
    if solid_bounds(result).is_none() {
        return Err(failure(
            BooleanFailureReason::InvalidResultTopology,
            "the boolean result contains non-finite vertex geometry".into(),
        ));
    }
    if supports_geometric_consistency_check(result) && !result.is_geometric_consistent() {
        return Err(failure(
            BooleanFailureReason::InvalidResultTopology,
            "result edge geometry is inconsistent with its vertices or surfaces".into(),
        ));
    }
    Ok(())
}

fn heal_solid(solid: &Solid, _tolerance: f64) -> Result<Solid, String> {
    catch_unwind(AssertUnwindSafe(|| {
        // Truck's closed-edge/face healer cannot operate on its polymorphic
        // modeling Curve enum. A compressed round trip still gives us a
        // bounded, lossless topology normalization and revalidates every
        // reconstructed vertex, edge, face, shell, and solid relationship.
        Solid::extract(solid.compress()).map_err(|error| error.to_string())
    }))
    .map_err(|payload| format!("Truck panicked: {}", panic_message(payload.as_ref())))?
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn diagnostic(
    feature: &Feature,
    operation: BooleanOperation,
    operands: [FeatureId; 2],
    tolerance_mm: f64,
    left_bounds: Option<AxisAlignedBounds>,
    right_bounds: Option<AxisAlignedBounds>,
    stage: BooleanFailureStage,
    reason: BooleanFailureReason,
    attempts: Vec<BooleanAttemptDiagnostic>,
    detail: String,
) -> BooleanDiagnostic {
    BooleanDiagnostic {
        feature_id: feature.id,
        operation,
        operands,
        stage,
        reason,
        tolerance_mm,
        attempts,
        left_bounds,
        right_bounds,
        detail,
    }
}
