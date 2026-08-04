use cadx_app::{
    SessionError, TransactionMetadata, TransactionOutcome, TransactionPreview, TransactionSource,
};
use cadx_core::domain::{FeatureId, ModelCommand, Primitive};

use crate::{CadxApp, StatusMessage};

impl CadxApp {
    pub(crate) fn clear_topology_selection(&mut self) {
        self.selected_face = None;
        self.selected_edges.clear();
        self.selected_vertex = None;
    }

    pub(crate) fn reconcile_selection(&mut self) {
        if self
            .selected
            .is_some_and(|id| self.session.document().feature(id).is_none())
        {
            self.selected = self
                .session
                .document()
                .features
                .last()
                .map(|feature| feature.id);
        }
        if self
            .selected_face
            .as_ref()
            .is_some_and(|reference| self.session.scene().face(reference).is_none())
        {
            self.selected_face = None;
        }
        self.selected_edges
            .retain(|reference| self.session.scene().edge(reference).is_some());
        if self
            .selected_vertex
            .as_ref()
            .is_some_and(|reference| self.session.scene().vertex(reference).is_none())
        {
            self.selected_vertex = None;
        }
        self.reconcile_measurement();
        self.sync_viewport();
    }

    pub(crate) fn sync_viewport(&self) {
        let measurement = self.measurement.topology(self.session.scene());
        let mut added_features = Vec::new();
        let mut modified_features = Vec::new();
        let mut removed_features = Vec::new();
        let pending_preview = self
            .pending_ai_candidates
            .get(self.active_ai_candidate)
            .map(|candidate| &candidate.preview)
            .filter(|preview| preview.base_revision() == self.session.revision());
        let preview = if let Some(preview) = pending_preview {
            added_features.extend(
                preview
                    .diff()
                    .added_features
                    .iter()
                    .map(|feature| feature.id),
            );
            modified_features.extend(
                preview
                    .diff()
                    .modified_features
                    .iter()
                    .map(|feature| feature.id),
            );
            removed_features.extend(
                preview
                    .diff()
                    .removed_features
                    .iter()
                    .map(|feature| feature.id),
            );
            Some(cadx_render::GhostPreview {
                scene: preview.scene(),
                added_features: &added_features,
                modified_features: &modified_features,
                removed_features: &removed_features,
            })
        } else {
            None
        };
        self.viewport_scene.update_with_topology_and_preview(
            self.session.scene(),
            self.selected,
            cadx_render::TopologySelection {
                face: self.selected_face.as_ref(),
                edges: &self.selected_edges,
                vertex: self.selected_vertex.as_ref(),
                measurement_faces: &measurement.faces,
                measurement_edges: &measurement.edges,
                measurement_vertices: &measurement.vertices,
                measurement_guides: &measurement.guides,
            },
            preview,
        );
    }

    pub(crate) fn execute(
        &mut self,
        commands: Vec<ModelCommand>,
        status: StatusMessage,
    ) -> Result<(), SessionError> {
        self.execute_from(commands, status, TransactionSource::Ui)
    }

    pub(crate) fn execute_from(
        &mut self,
        commands: Vec<ModelCommand>,
        status: StatusMessage,
        source: TransactionSource,
    ) -> Result<(), SessionError> {
        let selected_sketch = self.selected.filter(|id| {
            self.session
                .document()
                .feature(*id)
                .is_some_and(|feature| matches!(feature.primitive, Primitive::Sketch { .. }))
        });
        let outcome = match self.session.execute_with_source(commands, source) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.record_session_error(&error, selected_sketch);
                return Err(error);
            }
        };
        self.finish_transaction(&outcome, status, source);
        Ok(())
    }

    pub(crate) fn commit_preview_from(
        &mut self,
        preview: TransactionPreview,
        status: StatusMessage,
        source: TransactionSource,
        label: String,
    ) -> Result<(), SessionError> {
        let selected_sketch = self.selected.filter(|id| {
            self.session
                .document()
                .feature(*id)
                .is_some_and(|feature| matches!(feature.primitive, Primitive::Sketch { .. }))
        });
        let outcome = match self
            .session
            .commit_preview_with_metadata(preview, TransactionMetadata::new(source, label))
        {
            Ok(outcome) => outcome,
            Err(error) => {
                self.record_session_error(&error, selected_sketch);
                return Err(error);
            }
        };
        self.finish_transaction(&outcome, status, source);
        Ok(())
    }

    fn record_session_error(&mut self, error: &SessionError, selected_sketch: Option<FeatureId>) {
        if let Some(diagnostic) = error.boolean_diagnostic() {
            self.last_boolean_failure = Some(diagnostic.clone());
        }
        if let Some(diagnostic) = error.edge_modifier_diagnostic() {
            self.last_edge_modifier_failure = Some(diagnostic.clone());
        }
        if let Some(diagnostic) = error.sketch_constraint_diagnostic() {
            self.last_sketch_failure = Some(diagnostic.clone());
            self.last_sketch_failure_feature = selected_sketch;
        }
        self.status = StatusMessage::Text(error.to_string());
    }

    fn finish_transaction(
        &mut self,
        outcome: &TransactionOutcome,
        status: StatusMessage,
        source: TransactionSource,
    ) {
        if source != TransactionSource::Ai {
            self.cancel_ai_plan_for_document_change();
            self.discard_pending_ai_candidates();
        }
        self.last_boolean_failure = None;
        self.last_edge_modifier_failure = None;
        self.last_sketch_failure = None;
        self.last_sketch_failure_feature = None;
        self.sketch_dimension_editor = None;
        self.interference_dialog = None;
        if let Some(&id) = outcome.created_features.last() {
            self.selected = Some(id);
            self.clear_topology_selection();
        }
        if self
            .selected_face
            .as_ref()
            .is_some_and(|reference| self.session.scene().face(reference).is_none())
        {
            self.selected_face = None;
        }
        self.selected_edges
            .retain(|reference| self.session.scene().edge(reference).is_some());
        if self
            .selected_vertex
            .as_ref()
            .is_some_and(|reference| self.session.scene().vertex(reference).is_none())
        {
            self.selected_vertex = None;
        }
        self.reconcile_measurement();
        self.sync_domain_state();
        self.status = status;
        self.sync_viewport();
    }

    pub(crate) fn undo(&mut self) {
        match self.session.undo() {
            Ok(true) => {
                self.cancel_ai_plan_for_document_change();
                self.discard_pending_ai_candidates();
                self.interference_dialog = None;
                self.status = StatusMessage::Key("status.undo");
                self.reconcile_selection();
            }
            Ok(false) => {}
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    pub(crate) fn redo(&mut self) {
        match self.session.redo() {
            Ok(true) => {
                self.cancel_ai_plan_for_document_change();
                self.discard_pending_ai_candidates();
                self.interference_dialog = None;
                self.status = StatusMessage::Key("status.redo");
                self.reconcile_selection();
            }
            Ok(false) => {}
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    pub(crate) fn duplicate_feature(&mut self, id: FeatureId) {
        let Some(feature) = self.session.document().feature(id) else {
            return;
        };
        let position = feature.translation.as_array();
        let _ = self.execute(
            vec![ModelCommand::Duplicate {
                id,
                name: String::new(),
                position,
            }],
            StatusMessage::Key("status.duplicated"),
        );
    }
}
