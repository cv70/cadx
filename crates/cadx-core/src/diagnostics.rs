//! Stable, kernel-neutral diagnostics for failed modeling operations.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    domain::{BooleanOperation, FeatureId},
    topology::EdgeRef,
};

/// Axis-aligned model-space bounds in millimeters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AxisAlignedBounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl AxisAlignedBounds {
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.min
            .iter()
            .chain(&self.max)
            .all(|value| value.is_finite())
            && (0..3).all(|axis| self.min[axis] <= self.max[axis])
    }

    /// Per-axis non-overlap gap. A zero component means the intervals touch
    /// or overlap on that axis.
    #[must_use]
    pub fn separation_from(self, other: Self) -> [f64; 3] {
        std::array::from_fn(|axis| {
            (other.min[axis] - self.max[axis])
                .max(self.min[axis] - other.max[axis])
                .max(0.0)
        })
    }

    #[must_use]
    pub fn is_disjoint_from(self, other: Self, tolerance_mm: f64) -> bool {
        self.separation_from(other)
            .into_iter()
            .any(|gap| gap > tolerance_mm.max(0.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanFailureStage {
    OperandResolution,
    OperandValidation,
    BroadPhase,
    KernelOperation,
    ResultValidation,
    TopologyHealing,
    TopologyNaming,
}

impl BooleanFailureStage {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::OperandResolution => "operand_resolution",
            Self::OperandValidation => "operand_validation",
            Self::BroadPhase => "broad_phase",
            Self::KernelOperation => "kernel_operation",
            Self::ResultValidation => "result_validation",
            Self::TopologyHealing => "topology_healing",
            Self::TopologyNaming => "topology_naming",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanFailureReason {
    MissingOperand,
    InvalidOperandTopology,
    InvalidOperandGeometry,
    DisjointOperands,
    KernelRejected,
    KernelPanic,
    EmptyResult,
    InvalidResultTopology,
    HealingFailed,
    ResultEvaluationFailed,
    TopologyNamingFailed,
}

impl BooleanFailureReason {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::MissingOperand => "missing_operand",
            Self::InvalidOperandTopology => "invalid_operand_topology",
            Self::InvalidOperandGeometry => "invalid_operand_geometry",
            Self::DisjointOperands => "disjoint_operands",
            Self::KernelRejected => "kernel_rejected",
            Self::KernelPanic => "kernel_panic",
            Self::EmptyResult => "empty_result",
            Self::InvalidResultTopology => "invalid_result_topology",
            Self::HealingFailed => "healing_failed",
            Self::ResultEvaluationFailed => "result_evaluation_failed",
            Self::TopologyNamingFailed => "topology_naming_failed",
        }
    }
}

/// Whether bounded topology normalization or non-crossing contact recovery was
/// used during one boolean attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanHealingStatus {
    NotAttempted,
    Applied,
    Failed,
}

/// Machine-readable evidence from one failed tolerance attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BooleanAttemptDiagnostic {
    pub tolerance_mm: f64,
    pub stage: BooleanFailureStage,
    pub reason: BooleanFailureReason,
    pub operand_healing: BooleanHealingStatus,
    pub result_healing: BooleanHealingStatus,
}

/// Machine-readable failure report for one boolean feature evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BooleanDiagnostic {
    pub feature_id: FeatureId,
    pub operation: BooleanOperation,
    pub operands: [FeatureId; 2],
    pub stage: BooleanFailureStage,
    pub reason: BooleanFailureReason,
    pub tolerance_mm: f64,
    /// Ordered evidence from every bounded attempt. An operand-resolution or
    /// broad-phase failure has at most one entry because no kernel retry ran.
    #[serde(default)]
    pub attempts: Vec<BooleanAttemptDiagnostic>,
    pub left_bounds: Option<AxisAlignedBounds>,
    pub right_bounds: Option<AxisAlignedBounds>,
    /// Backend detail for engineering diagnosis. Consumers must branch on
    /// `reason`, never parse this text as a protocol.
    pub detail: String,
}

impl BooleanDiagnostic {
    #[must_use]
    pub fn operand_separation_mm(&self) -> Option<[f64; 3]> {
        Some(self.left_bounds?.separation_from(self.right_bounds?))
    }
}

impl fmt::Display for BooleanDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "boolean feature {} failed at {} with {} for operands {} and {}: {}",
            self.feature_id,
            self.stage.code(),
            self.reason.code(),
            self.operands[0],
            self.operands[1],
            self.detail
        )
    }
}

impl std::error::Error for BooleanDiagnostic {}

/// The class of edge treatment being evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeModifierOperation {
    Chamfer,
    Fillet,
}

impl EdgeModifierOperation {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Chamfer => "chamfer",
            Self::Fillet => "fillet",
        }
    }
}

/// The dimensional input controlling an edge treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeModifierParameter {
    Distance,
    Radius,
}

impl EdgeModifierParameter {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Radius => "radius",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeModifierFailureStage {
    ReferenceResolution,
    GeometryValidation,
    Construction,
    ResultValidation,
    TopologyNaming,
}

impl EdgeModifierFailureStage {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ReferenceResolution => "reference_resolution",
            Self::GeometryValidation => "geometry_validation",
            Self::Construction => "construction",
            Self::ResultValidation => "result_validation",
            Self::TopologyNaming => "topology_naming",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeModifierFailureReason {
    EmptyEdgeSet,
    MixedSourceFeatures,
    LostReference,
    AmbiguousReference,
    NonLinearEdge,
    NonPlanarSupport,
    NonConvexEdge,
    SharedVertexUnsupported,
    NonConvexSource,
    ParameterBelowTolerance,
    ParameterExceedsTopology,
    KernelRejected,
    KernelPanic,
    InvalidResultTopology,
    TopologyNamingFailed,
}

impl EdgeModifierFailureReason {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EmptyEdgeSet => "empty_edge_set",
            Self::MixedSourceFeatures => "mixed_source_features",
            Self::LostReference => "lost_reference",
            Self::AmbiguousReference => "ambiguous_reference",
            Self::NonLinearEdge => "non_linear_edge",
            Self::NonPlanarSupport => "non_planar_support",
            Self::NonConvexEdge => "non_convex_edge",
            Self::SharedVertexUnsupported => "shared_vertex_unsupported",
            Self::NonConvexSource => "non_convex_source",
            Self::ParameterBelowTolerance => "parameter_below_tolerance",
            Self::ParameterExceedsTopology => "parameter_exceeds_topology",
            Self::KernelRejected => "kernel_rejected",
            Self::KernelPanic => "kernel_panic",
            Self::InvalidResultTopology => "invalid_result_topology",
            Self::TopologyNamingFailed => "topology_naming_failed",
        }
    }
}

/// Machine-readable failure report for one chamfer or fillet evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeModifierDiagnostic {
    pub feature_id: FeatureId,
    pub operation: EdgeModifierOperation,
    pub source_feature_id: Option<FeatureId>,
    pub edges: Vec<EdgeRef>,
    pub stage: EdgeModifierFailureStage,
    pub reason: EdgeModifierFailureReason,
    pub parameter: EdgeModifierParameter,
    pub parameter_value_mm: f64,
    pub tolerance_mm: f64,
    /// Zero-based positions in `edges` associated with the failure, when the
    /// backend can identify them without guessing.
    pub offending_edge_indices: Option<Vec<usize>>,
    /// Backend detail for engineering diagnosis. Consumers must branch on
    /// `reason`, never parse this text as a protocol.
    pub detail: String,
}

impl fmt::Display for EdgeModifierDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} feature {} failed at {} with {} for source {:?}: {}",
            self.operation.code(),
            self.feature_id,
            self.stage.code(),
            self.reason.code(),
            self.source_feature_id,
            self.detail
        )
    }
}

impl std::error::Error for EdgeModifierDiagnostic {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SketchConstraintFailureReason {
    Conflict,
    NonConvergence,
}

impl SketchConstraintFailureReason {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::NonConvergence => "non_convergence",
        }
    }
}

/// Machine-readable report retained when a sketch edit fails atomically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchConstraintDiagnostic {
    pub reason: SketchConstraintFailureReason,
    /// Zero-based indices into the attempted ordered constraint list. Empty
    /// for a numerical failure that could not be attributed safely.
    pub constraint_indices: Vec<u32>,
    pub iterations: u32,
    pub residual: f64,
    /// Informational detail. Consumers branch on `reason` and indices rather
    /// than parsing this text.
    pub detail: String,
}

impl fmt::Display for SketchConstraintDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "sketch constraints failed with {} at indices {:?} after {} iterations (residual {}): {}",
            self.reason.code(),
            self.constraint_indices,
            self.iterations,
            self.residual,
            self.detail
        )
    }
}

impl std::error::Error for SketchConstraintDiagnostic {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_report_axis_separation_with_tolerance() {
        let first = AxisAlignedBounds {
            min: [0.0, 0.0, 0.0],
            max: [10.0, 10.0, 10.0],
        };
        let second = AxisAlignedBounds {
            min: [12.0, 4.0, -3.0],
            max: [20.0, 6.0, 3.0],
        };
        let separation = first.separation_from(second);
        assert!((separation[0] - 2.0).abs() < f64::EPSILON);
        assert!(separation[1].abs() < f64::EPSILON);
        assert!(separation[2].abs() < f64::EPSILON);
        assert!(first.is_disjoint_from(second, 0.05));
        assert!(!first.is_disjoint_from(second, 2.0));
    }

    #[test]
    fn boolean_diagnostic_is_structured_and_serializable() {
        let diagnostic = BooleanDiagnostic {
            feature_id: 3,
            operation: BooleanOperation::Intersect,
            operands: [1, 2],
            stage: BooleanFailureStage::BroadPhase,
            reason: BooleanFailureReason::DisjointOperands,
            tolerance_mm: 0.05,
            attempts: vec![BooleanAttemptDiagnostic {
                tolerance_mm: 0.05,
                stage: BooleanFailureStage::BroadPhase,
                reason: BooleanFailureReason::DisjointOperands,
                operand_healing: BooleanHealingStatus::NotAttempted,
                result_healing: BooleanHealingStatus::NotAttempted,
            }],
            left_bounds: None,
            right_bounds: None,
            detail: "operands do not overlap".into(),
        };
        let value = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(value["reason"], "disjoint_operands");
        assert_eq!(value["operation"], "intersect");
        assert_eq!(value["attempts"][0]["operand_healing"], "not_attempted");
        assert!(diagnostic.to_string().contains("broad_phase"));
    }

    #[test]
    fn edge_modifier_diagnostic_is_structured_and_serializable() {
        use crate::topology::{FaceRef, PrimitiveFace};

        let edge = EdgeRef::new(
            4,
            FaceRef::primitive(4, PrimitiveFace::BoxXMax),
            FaceRef::primitive(4, PrimitiveFace::BoxZMax),
            0,
        );
        let diagnostic = EdgeModifierDiagnostic {
            feature_id: 5,
            operation: EdgeModifierOperation::Fillet,
            source_feature_id: Some(4),
            edges: vec![edge],
            stage: EdgeModifierFailureStage::GeometryValidation,
            reason: EdgeModifierFailureReason::ParameterBelowTolerance,
            parameter: EdgeModifierParameter::Radius,
            parameter_value_mm: 0.01,
            tolerance_mm: 0.05,
            offending_edge_indices: None,
            detail: "radius is below tolerance".into(),
        };
        let value = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(value["operation"], "fillet");
        assert_eq!(value["reason"], "parameter_below_tolerance");
        assert_eq!(value["parameter"], "radius");
        assert!(diagnostic.to_string().contains("geometry_validation"));
    }

    #[test]
    fn sketch_constraint_diagnostic_is_structured_and_serializable() {
        let diagnostic = SketchConstraintDiagnostic {
            reason: SketchConstraintFailureReason::Conflict,
            constraint_indices: vec![1, 3],
            iterations: 64,
            residual: 0.5,
            detail: "two dimensions disagree".into(),
        };
        let value = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(value["reason"], "conflict");
        assert_eq!(value["constraint_indices"], serde_json::json!([1, 3]));
        assert!(diagnostic.to_string().contains("[1, 3]"));
    }
}
