//! Shared kernel stub and command fixtures for the session unit tests.

use std::sync::Arc;

use cadx_core::{
    diagnostics::{
        BooleanDiagnostic, BooleanFailureReason, BooleanFailureStage, EdgeModifierDiagnostic,
        EdgeModifierFailureReason, EdgeModifierFailureStage, EdgeModifierOperation,
        EdgeModifierParameter,
    },
    domain::{BooleanOperation, CadDocument, ModelCommand},
    kernel::{
        CadKernel, EvaluatedPart, EvaluatedScene, InterferenceAnalysis, KernelError, TriangleMesh,
    },
    topology::{EdgeRef, FaceRef, PrimitiveFace},
};

use super::DocumentSession;

#[derive(Debug)]
pub(super) struct TestKernel;

impl CadKernel for TestKernel {
    fn name(&self) -> &'static str {
        "test"
    }

    fn evaluate(&self, document: &CadDocument) -> Result<EvaluatedScene, KernelError> {
        if document
            .features
            .iter()
            .any(|feature| feature.name == "reject")
        {
            return Err(KernelError::Evaluation {
                feature_id: 1,
                message: "rejected by test kernel".into(),
            });
        }
        if document
            .features
            .iter()
            .any(|feature| feature.name == "boolean reject")
        {
            return Err(BooleanDiagnostic {
                feature_id: 3,
                operation: BooleanOperation::Intersect,
                operands: [1, 2],
                stage: BooleanFailureStage::BroadPhase,
                reason: BooleanFailureReason::DisjointOperands,
                tolerance_mm: 0.05,
                attempts: Vec::new(),
                left_bounds: None,
                right_bounds: None,
                detail: "test diagnostic".into(),
            }
            .into());
        }
        if document
            .features
            .iter()
            .any(|feature| feature.name == "edge reject")
        {
            return Err(EdgeModifierDiagnostic {
                feature_id: 2,
                operation: EdgeModifierOperation::Fillet,
                source_feature_id: Some(1),
                edges: vec![EdgeRef::new(
                    1,
                    FaceRef::primitive(1, PrimitiveFace::BoxXMax),
                    FaceRef::primitive(1, PrimitiveFace::BoxZMax),
                    0,
                )],
                stage: EdgeModifierFailureStage::GeometryValidation,
                reason: EdgeModifierFailureReason::SharedVertexUnsupported,
                parameter: EdgeModifierParameter::Radius,
                parameter_value_mm: 2.0,
                tolerance_mm: 0.05,
                offending_edge_indices: Some(vec![0, 1]),
                detail: "test edge diagnostic".into(),
            }
            .into());
        }
        Ok(EvaluatedScene {
            parts: document
                .features
                .iter()
                .map(|feature| EvaluatedPart {
                    feature_id: feature.id,
                    name: feature.name.clone(),
                    color: feature.color,
                    material: feature.material.clone(),
                    mesh: TriangleMesh::default(),
                    faces: Vec::new(),
                    edges: Vec::new(),
                    vertices: Vec::new(),
                })
                .collect(),
            ..EvaluatedScene::default()
        })
    }

    fn analyze_interference(
        &self,
        document: &CadDocument,
    ) -> Result<InterferenceAnalysis, KernelError> {
        Ok(InterferenceAnalysis {
            candidate_feature_ids: document.features.iter().map(|feature| feature.id).collect(),
            ..InterferenceAnalysis::default()
        })
    }
}

pub(super) fn session() -> DocumentSession {
    DocumentSession::new(Arc::new(TestKernel), CadDocument::default()).unwrap()
}

pub(super) fn create_box(name: &str) -> ModelCommand {
    ModelCommand::CreateBox {
        name: name.into(),
        size: [1.0; 3],
        position: [0.0; 3],
    }
}
