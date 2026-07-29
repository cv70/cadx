use crate::validation::validate_candidate;
use crate::{
    ActionIdempotencyKey, ActionSource, CadDocument, CommandError, CommandTransaction, CommitId,
    DocumentSnapshot, History, HistoryError, ObjectPrecondition, PreparedActionRecord,
    ValidationReport,
};

/// The private authoritative document and semantic-history state.
///
/// `TaskWorkspace` flattens this type during serialization so introducing the
/// in-memory ownership boundary does not change the current `.cadx` payload.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DocumentStore {
    document: CadDocument,
    history: History,
}

impl DocumentStore {
    pub(crate) fn new(document: CadDocument) -> Self {
        Self {
            history: History::new(document.clone()),
            document,
        }
    }

    pub(crate) fn from_parts(document: CadDocument, history: History) -> Self {
        Self { document, history }
    }

    pub(crate) fn revision(&self) -> CommitId {
        self.history.head()
    }

    pub(crate) fn document(&self) -> &CadDocument {
        &self.document
    }

    pub(crate) fn history(&self) -> &History {
        &self.history
    }

    pub(crate) fn snapshot(&self) -> DocumentSnapshot {
        let revision = self.revision();
        self.snapshot_at(revision)
            .expect("authoritative history must have a valid active ancestry")
    }

    pub(crate) fn snapshot_at(&self, revision: CommitId) -> Result<DocumentSnapshot, HistoryError> {
        let document = if revision == self.revision() {
            self.document.clone()
        } else {
            self.history.restore(revision)?
        };
        let object_versions = self.history.object_versions_at(revision)?;
        Ok(DocumentSnapshot::new(revision, document, object_versions))
    }

    pub(crate) fn is_ancestor(
        &self,
        ancestor: CommitId,
        descendant: CommitId,
    ) -> Result<bool, HistoryError> {
        self.history.is_ancestor(ancestor, descendant)
    }

    pub(crate) fn state_hash_at(&self, revision: CommitId) -> Result<[u8; 32], HistoryError> {
        let document = self.history.restore(revision)?;
        let evidence =
            validate_candidate(&document).map_err(HistoryError::CandidateValidationUnavailable)?;
        if !evidence.passed() {
            return Err(HistoryError::CandidateValidationFailed(evidence.summary()));
        }
        Ok(evidence.candidate_state_hash())
    }

    pub(crate) fn conflicting_precondition(
        &self,
        expected: &[ObjectPrecondition],
    ) -> Result<Option<(ObjectPrecondition, ObjectPrecondition)>, HistoryError> {
        let versions = self.history.object_versions_at(self.revision())?;
        Ok(expected.iter().find_map(|expected| {
            let actual = versions.precondition(expected.object);
            (*expected != actual).then_some((*expected, actual))
        }))
    }

    pub(crate) fn commit_for_idempotency_key(&self, key: ActionIdempotencyKey) -> Option<CommitId> {
        self.history
            .commit_for_idempotency_key(key)
            .map(|commit| commit.id)
    }

    pub(crate) fn commit(
        &mut self,
        source: Option<ActionSource>,
        intent: String,
        transaction: CommandTransaction,
        validation: ValidationReport,
        prepared_action: Option<PreparedActionRecord>,
    ) -> Result<CommitId, HistoryError> {
        let (document, commit_id) = self.history.commit_with_idempotency(
            &self.document,
            source,
            intent,
            transaction,
            validation,
            prepared_action,
        )?;
        self.document = document;
        Ok(commit_id)
    }

    pub(crate) fn bind_legacy_task_commit_sources(
        &mut self,
        sources: &std::collections::BTreeMap<CommitId, ActionSource>,
    ) -> Result<(), HistoryError> {
        self.history.bind_legacy_task_commit_sources(sources)
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub(crate) fn undo(&mut self) -> Result<CommitId, HistoryError> {
        let (document, commit_id) = self.history.undo()?;
        self.document = document;
        Ok(commit_id)
    }

    pub(crate) fn redo(&mut self) -> Result<CommitId, HistoryError> {
        let (document, commit_id) = self.history.redo()?;
        self.document = document;
        Ok(commit_id)
    }

    pub(crate) fn create_branch(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), HistoryError> {
        self.history.create_branch(name, commit_id)?;
        Ok(())
    }

    pub(crate) fn checkout_branch(&mut self, name: &str) -> Result<(), HistoryError> {
        self.document = self.history.checkout_branch(name)?;
        Ok(())
    }

    pub(crate) fn checkout_as_branch(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), HistoryError> {
        let name = name.into();
        if !self.history.branches.contains_key(&name) {
            self.history.create_branch(name.clone(), commit_id)?;
        }
        self.checkout_branch(&name)
    }

    pub(crate) fn migrate_document_to_current(&mut self) -> Result<(), CommandError> {
        self.document.migrate_to_current()
    }

    pub(crate) fn migrate_history_to_current(
        &mut self,
        regenerate_validation_evidence: bool,
    ) -> Result<(), HistoryError> {
        self.history
            .migrate_to_current(regenerate_validation_evidence)
    }

    pub(crate) fn infer_legacy_idempotency_keys(&mut self) -> Result<(), HistoryError> {
        self.history.infer_legacy_idempotency_keys()
    }
}
