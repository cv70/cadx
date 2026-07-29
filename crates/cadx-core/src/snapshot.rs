use std::fmt;
use std::sync::Arc;

use crate::object::{ObjectVersionIndex, transaction_objects};
use crate::{
    CadDocument, CommandTransaction, CommitId, DocumentSummary, ObjectId, ObjectPrecondition,
};

/// An immutable view of one authoritative document revision.
///
/// Snapshots can be cloned and shared with planners, renderers, and query
/// adapters without exposing the workspace's writable document store.
#[derive(Clone, PartialEq)]
pub struct DocumentSnapshot {
    revision: CommitId,
    document: Arc<CadDocument>,
    object_versions: Arc<ObjectVersionIndex>,
}

impl DocumentSnapshot {
    pub(crate) fn new(
        revision: CommitId,
        document: CadDocument,
        object_versions: ObjectVersionIndex,
    ) -> Self {
        Self {
            revision,
            document: Arc::new(document),
            object_versions: Arc::new(object_versions),
        }
    }

    pub const fn revision(&self) -> CommitId {
        self.revision
    }

    pub fn document(&self) -> &CadDocument {
        &self.document
    }

    pub fn summary(&self) -> DocumentSummary {
        self.document.summary()
    }

    pub fn object_precondition(&self, object: ObjectId) -> ObjectPrecondition {
        self.object_versions.precondition(object)
    }

    pub(crate) fn preconditions_for(
        &self,
        transaction: &CommandTransaction,
    ) -> Vec<ObjectPrecondition> {
        transaction_objects(transaction, &self.document)
            .into_iter()
            .map(|object| self.object_precondition(object))
            .collect()
    }
}

impl fmt::Debug for DocumentSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let summary = self.summary();
        formatter
            .debug_struct("DocumentSnapshot")
            .field("revision", &self.revision)
            .field("schema_version", &self.document.schema_version)
            .field("entity_count", &summary.entity_count)
            .field("layer_count", &summary.layer_count)
            .finish_non_exhaustive()
    }
}
