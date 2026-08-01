#![recursion_limit = "256"]

mod genai;

use std::{future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use cadx_analysis::{MeasurementResult, SceneAnalysis};
use cadx_core::{
    diagnostics::{BooleanDiagnostic, EdgeModifierDiagnostic, SketchConstraintDiagnostic},
    domain::{CadDocument, FeatureId, ModelCommand, SketchDimensionKind},
    kernel::{CadKernelCapabilities, SketchSolveDiagnostic},
    topology::{EdgeRef, FaceRef, VertexRef},
};

pub use genai::GenAiAssistant;

pub type AiFuture = Pin<Box<dyn Future<Output = Result<AiPlan, AiError>> + Send + 'static>>;

#[derive(Debug, Clone)]
pub struct AiRequest {
    pub prompt: String,
    pub document: CadDocument,
    /// Read-only geometric context computed from the last kernel evaluation.
    /// It is optional so headless callers can plan from a document snapshot.
    pub context: Option<AiContext>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiContext {
    /// Capabilities declared by the kernel that validated `scene_analysis`.
    pub kernel_capabilities: CadKernelCapabilities,
    pub selected_feature_id: Option<FeatureId>,
    pub selected_face: Option<FaceRef>,
    pub selected_edges: Vec<EdgeRef>,
    pub selected_vertex: Option<VertexRef>,
    /// Optional result from the user's active, validated measurement set.
    pub measurement: Option<MeasurementResult>,
    /// Most recent structured boolean rejection, retained for a corrective
    /// follow-up request and cleared after the next successful transaction.
    pub last_boolean_failure: Option<BooleanDiagnostic>,
    /// Most recent structured chamfer or fillet rejection. Backend detail is
    /// informational; correction logic branches on the stable reason code.
    pub last_edge_modifier_failure: Option<EdgeModifierDiagnostic>,
    /// Most recent atomic sketch-constraint rejection, including stable reason
    /// and attempted ordered constraint indices.
    pub last_sketch_failure: Option<SketchConstraintDiagnostic>,
    /// Rank-based report for the selected committed sketch, when applicable.
    pub selected_sketch_diagnostic: Option<SketchSolveDiagnostic>,
    /// Directly editable driving dimensions on the selected committed sketch.
    pub selected_sketch_dimensions: Vec<AiSketchDimension>,
    pub scene_analysis: SceneAnalysis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiSketchDimension {
    pub constraint_index: u32,
    pub kind: SketchDimensionKind,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiPlan {
    pub summary: String,
    pub commands: Vec<ModelCommand>,
}

/// Provider-neutral AI boundary. Implementations return declarative commands;
/// they never receive a mutable document or a CAD-kernel handle.
pub trait AiAssistant: Send + Sync {
    fn model_name(&self) -> &str;
    fn plan(&self, request: AiRequest) -> AiFuture;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_analysis::{LengthPrecision, MeasurementResult};
    use cadx_core::{
        diagnostics::{
            BooleanDiagnostic, BooleanFailureReason, BooleanFailureStage, EdgeModifierDiagnostic,
            EdgeModifierFailureReason, EdgeModifierFailureStage, EdgeModifierOperation,
            EdgeModifierParameter,
        },
        domain::BooleanOperation,
        topology::PrimitiveFace,
    };

    #[test]
    fn geometric_context_is_structured_and_serializable() {
        let edge = EdgeRef::new(
            7,
            FaceRef::primitive(7, PrimitiveFace::BoxXMin),
            FaceRef::primitive(7, PrimitiveFace::BoxYMin),
            0,
        );
        let context = AiContext {
            kernel_capabilities: CadKernelCapabilities::default(),
            selected_feature_id: Some(7),
            selected_face: None,
            selected_edges: Vec::new(),
            selected_vertex: None,
            measurement: Some(MeasurementResult::EdgeLength {
                edge: edge.clone(),
                length_mm: 25.0,
                precision: LengthPrecision::Exact,
            }),
            last_boolean_failure: Some(BooleanDiagnostic {
                feature_id: 9,
                operation: BooleanOperation::Intersect,
                operands: [7, 8],
                stage: BooleanFailureStage::BroadPhase,
                reason: BooleanFailureReason::DisjointOperands,
                tolerance_mm: 0.05,
                attempts: Vec::new(),
                left_bounds: None,
                right_bounds: None,
                detail: "no overlap".into(),
            }),
            last_edge_modifier_failure: Some(EdgeModifierDiagnostic {
                feature_id: 10,
                operation: EdgeModifierOperation::Fillet,
                source_feature_id: Some(7),
                edges: vec![edge],
                stage: EdgeModifierFailureStage::GeometryValidation,
                reason: EdgeModifierFailureReason::SharedVertexUnsupported,
                parameter: EdgeModifierParameter::Radius,
                parameter_value_mm: 2.0,
                tolerance_mm: 0.05,
                offending_edge_indices: Some(vec![0, 1]),
                detail: "corner patch required".into(),
            }),
            last_sketch_failure: None,
            selected_sketch_diagnostic: Some(SketchSolveDiagnostic {
                parameter_count: 8,
                equation_count: 6,
                rank: 6,
                degrees_of_freedom: 2,
                redundant_constraints: Vec::new(),
                residual: 1.0e-12,
                iterations: 4,
            }),
            selected_sketch_dimensions: vec![AiSketchDimension {
                constraint_index: 3,
                kind: SketchDimensionKind::Length,
                value: 12.5,
            }],
            scene_analysis: SceneAnalysis {
                total_volume_mm3: 42.0,
                ..SceneAnalysis::default()
            },
        };
        let encoded = serde_json::to_value(&context).unwrap();
        assert_eq!(
            encoded["kernel_capabilities"]["chamfer"]["edge_count"],
            "unsupported"
        );
        assert_eq!(encoded["selected_feature_id"], 7);
        assert_eq!(encoded["scene_analysis"]["total_volume_mm3"], 42.0);
        assert_eq!(encoded["measurement"]["length_mm"], 25.0);
        assert_eq!(encoded["measurement"]["precision"]["kind"], "exact");
        assert_eq!(
            encoded["last_boolean_failure"]["reason"],
            "disjoint_operands"
        );
        assert_eq!(
            encoded["last_edge_modifier_failure"]["reason"],
            "shared_vertex_unsupported"
        );
        assert_eq!(
            encoded["selected_sketch_diagnostic"]["degrees_of_freedom"],
            2
        );
        assert_eq!(encoded["selected_sketch_dimensions"][0]["kind"], "length");
        assert_eq!(encoded["selected_sketch_dimensions"][0]["value"], 12.5);
    }
}

#[derive(Debug, Error)]
pub enum AiError {
    #[error("AI configuration is invalid: {0}")]
    Configuration(String),
    #[error("AI request failed: {0}")]
    Request(String),
    #[error("model did not call the CAD planning tool: {0}")]
    MissingToolCall(String),
    #[error("model returned an invalid CAD plan: {0}")]
    InvalidPlan(String),
}
