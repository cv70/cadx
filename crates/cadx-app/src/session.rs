use std::sync::Arc;

use cadx_core::{
    domain::{CadDocument, FeatureId, ModelCommand},
    kernel::{CadKernel, CadKernelCapabilities, EvaluatedScene, InterferenceAnalysis},
};

use crate::SessionError;

pub const DEFAULT_HISTORY_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentState {
    Clean,
    Dirty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionOutcome {
    pub created_features: Vec<FeatureId>,
}

#[derive(Clone)]
struct Revision {
    number: u64,
    document: CadDocument,
}

/// Owns the active document and all kernel-validated state transitions.
pub struct DocumentSession {
    kernel: Arc<dyn CadKernel>,
    document: CadDocument,
    evaluated: EvaluatedScene,
    revision: u64,
    saved_revision: Option<u64>,
    next_revision: u64,
    undo: Vec<Revision>,
    redo: Vec<Revision>,
    history_limit: usize,
}

impl DocumentSession {
    /// Creates a clean session after evaluating the initial document.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the kernel rejects the initial document.
    pub fn new(kernel: Arc<dyn CadKernel>, document: CadDocument) -> Result<Self, SessionError> {
        Self::with_history_limit(kernel, document, DEFAULT_HISTORY_LIMIT)
    }

    /// Creates a session with an explicit maximum number of undo revisions.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the kernel rejects the initial document.
    pub fn with_history_limit(
        kernel: Arc<dyn CadKernel>,
        document: CadDocument,
        history_limit: usize,
    ) -> Result<Self, SessionError> {
        let evaluated = kernel.evaluate(&document)?;
        Ok(Self {
            kernel,
            document,
            evaluated,
            revision: 1,
            saved_revision: Some(1),
            next_revision: 2,
            undo: Vec::new(),
            redo: Vec::new(),
            history_limit,
        })
    }

    #[must_use]
    pub fn document(&self) -> &CadDocument {
        &self.document
    }

    #[must_use]
    pub fn scene(&self) -> &EvaluatedScene {
        &self.evaluated
    }

    #[must_use]
    pub fn kernel_name(&self) -> &'static str {
        self.kernel.name()
    }

    #[must_use]
    pub fn kernel_capabilities(&self) -> CadKernelCapabilities {
        self.kernel.capabilities()
    }

    /// Runs exact product-level interference analysis over the active revision.
    ///
    /// The operation is read-only and does not affect undo history.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the active kernel does not support the
    /// analysis or cannot materialize the document.
    pub fn analyze_interference(&self) -> Result<InterferenceAnalysis, SessionError> {
        self.kernel
            .analyze_interference(&self.document)
            .map_err(Into::into)
    }

    #[must_use]
    pub fn state(&self) -> DocumentState {
        if self.saved_revision == Some(self.revision) {
            DocumentState::Clean
        } else {
            DocumentState::Dirty
        }
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Validates commands against a staged document and the active kernel.
    ///
    /// The session is never mutated.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when domain or kernel validation fails.
    pub fn validate(&self, commands: &[ModelCommand]) -> Result<(), SessionError> {
        let mut staged = self.document.clone();
        staged.apply_transaction(commands.iter().cloned())?;
        self.kernel.evaluate(&staged)?;
        Ok(())
    }

    /// Executes a command batch as one undoable, kernel-validated transaction.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing the session when domain or
    /// kernel validation fails.
    pub fn execute(
        &mut self,
        commands: Vec<ModelCommand>,
    ) -> Result<TransactionOutcome, SessionError> {
        if commands.is_empty() {
            return Ok(TransactionOutcome {
                created_features: Vec::new(),
            });
        }
        let mut staged = self.document.clone();
        let created_features = staged.apply_transaction(commands)?;
        let evaluated = self.kernel.evaluate(&staged)?;
        let revision = self.allocate_revision()?;
        let previous = Revision {
            number: self.revision,
            document: std::mem::replace(&mut self.document, staged),
        };
        self.evaluated = evaluated;
        self.revision = revision;
        self.push_undo(previous);
        self.redo.clear();
        Ok(TransactionOutcome { created_features })
    }

    /// Restores the previous revision after re-evaluating it.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing history when evaluation fails.
    pub fn undo(&mut self) -> Result<bool, SessionError> {
        let Some(previous) = self.undo.last().cloned() else {
            return Ok(false);
        };
        let evaluated = self.kernel.evaluate(&previous.document)?;
        self.undo.pop();
        self.redo.push(Revision {
            number: self.revision,
            document: std::mem::replace(&mut self.document, previous.document),
        });
        self.revision = previous.number;
        self.evaluated = evaluated;
        Ok(true)
    }

    /// Restores the next revision after re-evaluating it.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing history when evaluation fails.
    pub fn redo(&mut self) -> Result<bool, SessionError> {
        let Some(next) = self.redo.last().cloned() else {
            return Ok(false);
        };
        let evaluated = self.kernel.evaluate(&next.document)?;
        self.redo.pop();
        let current = Revision {
            number: self.revision,
            document: std::mem::replace(&mut self.document, next.document),
        };
        self.push_undo(current);
        self.revision = next.number;
        self.evaluated = evaluated;
        Ok(true)
    }

    /// Replaces the active document only after kernel evaluation succeeds.
    ///
    /// Replacement starts a new clean history root, as used by New and Open.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing the session when evaluation fails.
    pub fn replace_document(&mut self, document: CadDocument) -> Result<(), SessionError> {
        let evaluated = self.kernel.evaluate(&document)?;
        let revision = self.allocate_revision()?;
        self.document = document;
        self.evaluated = evaluated;
        self.revision = revision;
        self.saved_revision = Some(revision);
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = Some(self.revision);
    }

    fn allocate_revision(&mut self) -> Result<u64, SessionError> {
        let revision = self.next_revision;
        self.next_revision = self
            .next_revision
            .checked_add(1)
            .ok_or(SessionError::RevisionExhausted)?;
        Ok(revision)
    }

    fn push_undo(&mut self, revision: Revision) {
        if self.history_limit == 0 {
            return;
        }
        if self.undo.len() == self.history_limit {
            self.undo.remove(0);
        }
        self.undo.push(revision);
    }
}

#[cfg(test)]
mod tests {
    use cadx_core::assembly::{
        AssemblyMate, AssemblyMateKind, AssemblyTransform, ComponentDefinition, ComponentKind,
        ComponentOccurrence,
    };
    use cadx_core::diagnostics::{
        BooleanDiagnostic, BooleanFailureReason, BooleanFailureStage, EdgeModifierDiagnostic,
        EdgeModifierFailureReason, EdgeModifierFailureStage, EdgeModifierOperation,
        EdgeModifierParameter,
    };
    use cadx_core::domain::{BooleanOperation, SketchPlane};
    use cadx_core::kernel::{EvaluatedPart, KernelError, TriangleMesh};
    use cadx_core::topology::{EdgeRef, FaceRef, PrimitiveFace};

    use super::*;

    #[derive(Debug)]
    struct TestKernel;

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

    fn session() -> DocumentSession {
        DocumentSession::new(Arc::new(TestKernel), CadDocument::default()).unwrap()
    }

    fn create_box(name: &str) -> ModelCommand {
        ModelCommand::CreateBox {
            name: name.into(),
            size: [1.0; 3],
            position: [0.0; 3],
        }
    }

    #[test]
    fn commits_only_after_kernel_acceptance() {
        let mut session = session();
        assert!(session.execute(vec![create_box("reject")]).is_err());
        assert!(session.document().features.is_empty());
        assert!(!session.can_undo());
        assert_eq!(session.state(), DocumentState::Clean);
    }

    #[test]
    fn interference_analysis_is_read_only_and_delegates_to_the_active_kernel() {
        let mut session = session();
        session.execute(vec![create_box("body")]).unwrap();
        let document_before = session.document().clone();
        let report = session.analyze_interference().unwrap();

        assert_eq!(report.candidate_feature_ids, vec![1]);
        assert_eq!(session.document(), &document_before);
        assert!(session.can_undo());
    }

    #[test]
    fn structured_boolean_rejection_is_preserved_without_committing() {
        let mut session = session();
        let error = session
            .execute(vec![create_box("boolean reject")])
            .unwrap_err();
        let diagnostic = error.boolean_diagnostic().unwrap();
        assert_eq!(diagnostic.reason, BooleanFailureReason::DisjointOperands);
        assert!(session.document().features.is_empty());
        assert!(!session.can_undo());
    }

    #[test]
    fn structured_edge_modifier_rejection_is_preserved_without_committing() {
        let mut session = session();
        let error = session
            .execute(vec![create_box("edge reject")])
            .unwrap_err();
        let diagnostic = error.edge_modifier_diagnostic().unwrap();
        assert_eq!(
            diagnostic.reason,
            EdgeModifierFailureReason::SharedVertexUnsupported
        );
        assert!(session.document().features.is_empty());
        assert!(!session.can_undo());
    }

    #[test]
    fn truck_rejected_folded_loft_does_not_commit_session_state() {
        let mut session = DocumentSession::new(
            Arc::new(cadx_kernel_truck::TruckKernel::default()),
            CadDocument::default(),
        )
        .unwrap();
        let commands = [0.0, 20.0, 10.0]
            .into_iter()
            .enumerate()
            .map(|(index, z)| ModelCommand::CreateSketch {
                name: format!("folded section {index}"),
                plane: SketchPlane::WorldXy,
                profile: vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0, 0.0, z],
            })
            .collect();
        let sections = session.execute(commands).unwrap().created_features;
        session.mark_saved();
        let document_before = session.document().clone();
        let scene_before = session.scene().clone();

        let error = session
            .execute(vec![ModelCommand::CreateLoftFromSketches {
                name: "folded loft".into(),
                sketch_ids: sections,
                position: [0.0; 3],
            }])
            .unwrap_err();

        assert!(error.to_string().contains("does not advance monotonically"));
        assert_eq!(session.document(), &document_before);
        assert_eq!(session.scene(), &scene_before);
        assert_eq!(session.state(), DocumentState::Clean);
        assert!(session.can_undo());
        assert!(!session.can_redo());
    }

    #[test]
    fn undo_and_redo_restore_revisions() {
        let mut session = session();
        session.execute(vec![create_box("box")]).unwrap();
        assert_eq!(session.state(), DocumentState::Dirty);
        assert!(session.undo().unwrap());
        assert!(session.document().features.is_empty());
        assert_eq!(session.state(), DocumentState::Clean);
        assert!(session.redo().unwrap());
        assert_eq!(session.document().features.len(), 1);
        assert_eq!(session.state(), DocumentState::Dirty);
    }

    #[test]
    fn material_edits_update_evaluated_state_and_history() {
        let mut session = session();
        let id = session
            .execute(vec![create_box("body")])
            .unwrap()
            .created_features[0];
        session
            .execute(vec![ModelCommand::SetMaterial {
                id,
                name: "Steel".into(),
                density_kg_m3: 7_850.0,
            }])
            .unwrap();
        assert_eq!(
            session.scene().parts[0].material.as_ref().unwrap().name,
            "Steel"
        );
        assert!(session.undo().unwrap());
        assert!(session.scene().parts[0].material.is_none());
        assert!(session.redo().unwrap());
        assert!(
            (session.scene().parts[0]
                .material
                .as_ref()
                .unwrap()
                .density_kg_m3
                - 7_850.0)
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn occurrence_placement_is_kernel_validated_and_undoable() {
        let mut document = CadDocument::default();
        let body = document
            .apply(create_box("assembly body"))
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateAssembly {
                name: "fixture".into(),
                definitions: vec![ComponentDefinition {
                    id: 1,
                    name: "body".into(),
                    kind: ComponentKind::Part,
                    source: None,
                }],
                occurrences: vec![ComponentOccurrence {
                    id: 1,
                    name: "body:1".into(),
                    definition_id: 1,
                    parent_id: None,
                    suppressed: false,
                    transform: AssemblyTransform::IDENTITY,
                    feature_ids: vec![body],
                    source: None,
                }],
            })
            .unwrap();
        let mut session = DocumentSession::new(Arc::new(TestKernel), document).unwrap();

        session
            .execute(vec![ModelCommand::SetOccurrenceTransform {
                assembly_id: 1,
                occurrence_id: 1,
                position: [5.0, 6.0, 7.0],
                rotation: [10.0, 20.0, 30.0],
            }])
            .unwrap();
        assert_eq!(
            session.document().feature(body).unwrap().translation,
            cadx_core::domain::Vec3::new(5.0, 6.0, 7.0)
        );
        assert!(session.undo().unwrap());
        assert_eq!(
            session.document().feature(body).unwrap().translation,
            cadx_core::domain::Vec3::ZERO
        );
        assert!(session.redo().unwrap());
        assert_eq!(
            session.document().feature(body).unwrap().translation,
            cadx_core::domain::Vec3::new(5.0, 6.0, 7.0)
        );

        session
            .execute(vec![ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 1,
                suppressed: true,
            }])
            .unwrap();
        assert!(session.document().assembly(1).unwrap().occurrences[0].suppressed);
        assert!(session.undo().unwrap());
        assert!(!session.document().assembly(1).unwrap().occurrences[0].suppressed);
        assert!(session.redo().unwrap());
        assert!(session.document().assembly(1).unwrap().occurrences[0].suppressed);
    }

    #[test]
    fn assembly_mate_creation_and_state_are_undoable() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "carriage".into(),
                size: [1.0; 3],
                position: [10.0, 0.0, 0.0],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateAssembly {
                name: "stage".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "stage".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "carriage".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "stage:1".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "carriage:1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [10.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![body],
                        source: None,
                    },
                ],
            })
            .unwrap();
        let mut session = DocumentSession::new(Arc::new(TestKernel), document).unwrap();

        session
            .execute(vec![ModelCommand::CreateAssemblyMate {
                assembly_id: 1,
                mate: AssemblyMate {
                    id: 1,
                    name: "travel".into(),
                    parent_occurrence_id: 1,
                    child_occurrence_id: 2,
                    parent_frame: AssemblyTransform {
                        translation: [10.0, 0.0, 0.0],
                        ..AssemblyTransform::IDENTITY
                    },
                    child_frame: AssemblyTransform::IDENTITY,
                    kind: AssemblyMateKind::Slider {
                        axis: [1.0, 0.0, 0.0],
                        limits_mm: None,
                    },
                    state: 0.0,
                },
            }])
            .unwrap();
        session
            .execute(vec![ModelCommand::SetAssemblyMateState {
                assembly_id: 1,
                mate_id: 1,
                state: 5.0,
            }])
            .unwrap();
        assert_eq!(
            session.document().feature(body).unwrap().translation,
            cadx_core::domain::Vec3::new(15.0, 0.0, 0.0)
        );

        assert!(session.undo().unwrap());
        assert!(session.document().assembly(1).unwrap().mates[0].state.abs() < f64::EPSILON);
        assert_eq!(
            session.document().feature(body).unwrap().translation,
            cadx_core::domain::Vec3::new(10.0, 0.0, 0.0)
        );
        assert!(session.redo().unwrap());
        assert!(
            (session.document().assembly(1).unwrap().mates[0].state - 5.0).abs() < f64::EPSILON
        );
        assert_eq!(
            session.document().feature(body).unwrap().translation,
            cadx_core::domain::Vec3::new(15.0, 0.0, 0.0)
        );
    }

    #[test]
    fn saved_revision_is_tracked_across_history() {
        let mut session = session();
        session.execute(vec![create_box("box")]).unwrap();
        session.mark_saved();
        assert_eq!(session.state(), DocumentState::Clean);
        session
            .execute(vec![ModelCommand::Move {
                id: 1,
                position: [2.0, 0.0, 0.0],
            }])
            .unwrap();
        assert_eq!(session.state(), DocumentState::Dirty);
        session.undo().unwrap();
        assert_eq!(session.state(), DocumentState::Clean);
    }

    #[test]
    fn revision_exhaustion_does_not_mutate_the_document() {
        let mut session = session();
        session.next_revision = u64::MAX;
        assert!(matches!(
            session.execute(vec![create_box("box")]),
            Err(SessionError::RevisionExhausted)
        ));
        assert!(session.document().features.is_empty());
        assert_eq!(session.state(), DocumentState::Clean);
        assert!(!session.can_undo());
    }

    #[test]
    fn datum_face_dependency_round_trips_through_undo_and_redo() {
        let mut session = session();
        let source = session
            .execute(vec![create_box("source")])
            .unwrap()
            .created_features[0];
        let datum = session
            .execute(vec![ModelCommand::CreateDatumPlane {
                name: "datum".into(),
                face: FaceRef::primitive(source, PrimitiveFace::BoxZMax),
                offset: 3.0,
            }])
            .unwrap()
            .created_features[0];
        assert!(session.document().feature(datum).is_some());
        assert!(session.undo().unwrap());
        assert!(session.document().feature(datum).is_none());
        assert!(session.redo().unwrap());
        assert!(matches!(
            session
                .document()
                .feature(datum)
                .map(|feature| &feature.primitive),
            Some(cadx_core::domain::Primitive::DatumPlane { .. })
        ));
    }
}
