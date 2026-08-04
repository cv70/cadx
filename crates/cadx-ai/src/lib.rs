#![recursion_limit = "256"]

pub mod context;
mod genai;
pub mod intent;
pub mod tools;

use std::{future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use cadx_analysis::{MeasurementResult, SceneAnalysis};
use cadx_core::{
    diagnostics::{BooleanDiagnostic, EdgeModifierDiagnostic, SketchConstraintDiagnostic},
    domain::{CadDocument, ModelCommand, SketchDimensionKind},
    kernel::{CadKernelCapabilities, InterferenceAnalysis, SketchSolveDiagnostic},
};
use cadx_domain_api::{DomainContext, DomainId, DomainParameters};

use crate::tools::DomainAiToolBinding;

pub use genai::GenAiAssistant;

pub type AiFuture = Pin<Box<dyn Future<Output = Result<AiPlan, AiError>> + Send + 'static>>;
pub type DomainAiFuture =
    Pin<Box<dyn Future<Output = Result<DomainAiPlan, AiError>> + Send + 'static>>;

#[derive(Debug, Clone)]
pub struct AiRequest {
    pub prompt: String,
    pub document: CadDocument,
    /// Read-only geometric context computed from the last kernel evaluation.
    /// It is optional so headless callers can plan from a document snapshot.
    pub context: Option<AiContext>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DomainAiRequest {
    pub prompt: String,
    pub domain: DomainId,
    pub context: DomainContext,
    /// Request-scoped allow-list already bound to executable domain tools.
    pub tools: Vec<DomainAiToolBinding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DomainAiPlan {
    pub domain: DomainId,
    pub ai_tool_id: String,
    pub executable_tool_id: String,
    pub parameters: DomainParameters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiContext {
    /// Bounded interaction, topology, document-graph, and spatial focus state
    /// collected for this exact document revision.
    pub interaction: context::ContextSnapshot,
    /// Capabilities declared by the kernel that validated `scene_analysis`.
    pub kernel_capabilities: CadKernelCapabilities,
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
    /// Optional exact-B-Rep product interference evidence for this snapshot.
    pub interference_analysis: Option<InterferenceAnalysis>,
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
    /// Optional independent approaches. Engineering metrics are intentionally
    /// absent: the host computes them from each kernel-evaluated sandbox.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<AiPlanCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiPlanCandidate {
    pub summary: String,
    pub commands: Vec<ModelCommand>,
}

impl AiPlan {
    /// Flattens the primary proposal and at most two independent alternatives.
    #[must_use]
    pub fn into_candidates(self) -> Vec<AiPlanCandidate> {
        let mut candidates = Vec::with_capacity(1 + self.alternatives.len().min(2));
        candidates.push(AiPlanCandidate {
            summary: self.summary,
            commands: self.commands,
        });
        candidates.extend(self.alternatives.into_iter().take(2));
        candidates
    }
}

/// Provider-neutral AI boundary. Implementations return declarative commands;
/// they never receive a mutable document or a CAD-kernel handle.
pub trait AiAssistant: Send + Sync {
    fn model_name(&self) -> &str;
    fn plan(&self, request: AiRequest) -> AiFuture;
    fn plan_domain(&self, request: DomainAiRequest) -> DomainAiFuture;
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
        topology::{EdgeRef, FaceRef, PrimitiveFace},
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
            interaction: context::ContextSnapshot {
                document_revision: 11,
                selection: context::ContextSelection {
                    selected_feature_id: Some(7),
                    ..context::ContextSelection::default()
                },
                ..context::ContextSnapshot::default()
            },
            kernel_capabilities: CadKernelCapabilities::default(),
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
            interference_analysis: Some(InterferenceAnalysis {
                candidate_feature_ids: vec![7, 8],
                total_pair_count: 1,
                clear_pair_count: 1,
                ..InterferenceAnalysis::default()
            }),
        };
        let encoded = serde_json::to_value(&context).unwrap();
        assert_eq!(
            encoded["kernel_capabilities"]["chamfer"]["edge_count"],
            "unsupported"
        );
        assert_eq!(
            encoded["interaction"]["selection"]["selected_feature_id"],
            7
        );
        assert_eq!(encoded["interaction"]["document_revision"], 11);
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
        assert_eq!(
            encoded["interference_analysis"]["candidate_feature_ids"],
            serde_json::json!([7, 8])
        );
    }

    #[test]
    fn plan_flattens_at_most_three_independent_candidates() {
        let command = |name: &str| ModelCommand::CreateBox {
            name: name.into(),
            size: [1.0; 3],
            position: [0.0; 3],
        };
        let plan = AiPlan {
            summary: "primary".into(),
            commands: vec![command("primary")],
            alternatives: vec![
                AiPlanCandidate {
                    summary: "second".into(),
                    commands: vec![command("second")],
                },
                AiPlanCandidate {
                    summary: "third".into(),
                    commands: vec![command("third")],
                },
                AiPlanCandidate {
                    summary: "ignored".into(),
                    commands: vec![command("ignored")],
                },
            ],
        };

        let candidates = plan.into_candidates();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].summary, "primary");
        assert_eq!(candidates[2].summary, "third");
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
    #[error("model returned an invalid domain tool call: {0}")]
    InvalidDomainToolCall(String),
}
