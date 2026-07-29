use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::command::{CommandTransaction, DocumentDiff};
use crate::document::{CadDocument, CommandError, next_id_after};
use crate::object::ObjectVersionIndex;
use crate::prepared::{PreparedAction, PreparedActionRecord};
use crate::task::ValidationReport;
use crate::validation::{ValidationEvidence, validate_candidate};
use crate::{
    ActionIdempotencyKey, ActionSource, AgentRunId, CommitId, ConstraintId, EntityId,
    INITIAL_SNAPSHOT_INTERVAL, LayerId, ParameterId, PromptChangeSetId, TaskId,
};

/// A deterministic comparison between two restored document versions.
///
/// The comparison is based on complete document states rather than a single
/// commit's direct diff, so it remains correct across branches and snapshots.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryComparison {
    pub base_commit: CommitId,
    pub target_commit: CommitId,
    pub added_entities: Vec<EntityId>,
    pub removed_entities: Vec<EntityId>,
    pub modified_entities: Vec<EntityId>,
    pub added_layers: Vec<LayerId>,
    pub removed_layers: Vec<LayerId>,
    pub modified_layers: Vec<LayerId>,
    pub added_parameters: Vec<ParameterId>,
    pub removed_parameters: Vec<ParameterId>,
    pub modified_parameters: Vec<ParameterId>,
    pub added_constraints: Vec<ConstraintId>,
    pub removed_constraints: Vec<ConstraintId>,
    pub modified_constraints: Vec<ConstraintId>,
    pub metadata_changed: bool,
    pub units_changed: bool,
}

impl HistoryComparison {
    pub fn between(
        base_commit: CommitId,
        target_commit: CommitId,
        base: &CadDocument,
        target: &CadDocument,
    ) -> Self {
        let (added_entities, removed_entities, modified_entities) =
            compare_maps(&base.entities, &target.entities);
        let (added_layers, removed_layers, modified_layers) =
            compare_maps(&base.layers, &target.layers);
        let (added_parameters, removed_parameters, modified_parameters) =
            compare_maps(&base.parameters, &target.parameters);
        let (added_constraints, removed_constraints, modified_constraints) =
            compare_maps(&base.constraints, &target.constraints);
        Self {
            base_commit,
            target_commit,
            added_entities,
            removed_entities,
            modified_entities,
            added_layers,
            removed_layers,
            modified_layers,
            added_parameters,
            removed_parameters,
            modified_parameters,
            added_constraints,
            removed_constraints,
            modified_constraints,
            metadata_changed: base.metadata != target.metadata,
            units_changed: base.units != target.units,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added_entities.is_empty()
            && self.removed_entities.is_empty()
            && self.modified_entities.is_empty()
            && self.added_layers.is_empty()
            && self.removed_layers.is_empty()
            && self.modified_layers.is_empty()
            && self.added_parameters.is_empty()
            && self.removed_parameters.is_empty()
            && self.modified_parameters.is_empty()
            && self.added_constraints.is_empty()
            && self.removed_constraints.is_empty()
            && self.modified_constraints.is_empty()
            && !self.metadata_changed
            && !self.units_changed
    }

    pub fn summary(&self) -> String {
        let additions = self.added_entities.len()
            + self.added_layers.len()
            + self.added_parameters.len()
            + self.added_constraints.len();
        let removals = self.removed_entities.len()
            + self.removed_layers.len()
            + self.removed_parameters.len()
            + self.removed_constraints.len();
        let modifications = self.modified_entities.len()
            + self.modified_layers.len()
            + self.modified_parameters.len()
            + self.modified_constraints.len()
            + usize::from(self.metadata_changed)
            + usize::from(self.units_changed);
        format!("{additions} added, {removals} removed, {modifications} modified")
    }
}

fn compare_maps<T: PartialEq>(
    base: &BTreeMap<u64, T>,
    target: &BTreeMap<u64, T>,
) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    let added = target
        .keys()
        .filter(|id| !base.contains_key(id))
        .copied()
        .collect();
    let removed = base
        .keys()
        .filter(|id| !target.contains_key(id))
        .copied()
        .collect();
    let modified = base
        .iter()
        .filter_map(|(id, value)| {
            target
                .get(id)
                .filter(|target_value| *target_value != value)
                .map(|_| *id)
        })
        .collect();
    (added, removed, modified)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SemanticCommit {
    pub id: CommitId,
    pub parent: Option<CommitId>,
    pub task_id: Option<TaskId>,
    #[serde(default)]
    pub change_set_id: Option<PromptChangeSetId>,
    #[serde(default)]
    pub agent_run_id: Option<AgentRunId>,
    pub intent: String,
    pub transaction: CommandTransaction,
    pub diff: DocumentDiff,
    /// Untrusted caller or planner claim retained for audit compatibility.
    pub validation: ValidationReport,
    #[serde(default)]
    evidence: Option<ValidationEvidence>,
    #[serde(default)]
    preparation: Option<PreparedActionRecord>,
}

impl SemanticCommit {
    pub const fn action_source(&self) -> Option<ActionSource> {
        match self.task_id {
            Some(task_id) => Some(ActionSource {
                task_id,
                change_set_id: self.change_set_id,
                agent_run_id: self.agent_run_id,
            }),
            None => None,
        }
    }

    pub fn validation_evidence(&self) -> Option<&ValidationEvidence> {
        self.evidence.as_ref()
    }

    pub const fn idempotency_key(&self) -> Option<ActionIdempotencyKey> {
        match &self.preparation {
            Some(preparation) => Some(preparation.idempotency_key()),
            None => None,
        }
    }

    pub fn preparation(&self) -> Option<&PreparedActionRecord> {
        self.preparation.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub commit_id: CommitId,
    pub document: CadDocument,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignBranch {
    pub name: String,
    pub head: CommitId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub commits: BTreeMap<CommitId, SemanticCommit>,
    pub snapshots: BTreeMap<CommitId, Snapshot>,
    pub branches: BTreeMap<String, DesignBranch>,
    pub active_branch: String,
    #[serde(default)]
    redo_stacks: BTreeMap<String, Vec<CommitId>>,
    #[serde(default = "default_snapshot_interval")]
    snapshot_interval: CommitId,
    #[serde(default = "default_next_commit_id")]
    next_commit_id: CommitId,
}

const fn default_snapshot_interval() -> CommitId {
    INITIAL_SNAPSHOT_INTERVAL
}

const fn default_next_commit_id() -> CommitId {
    1
}

impl History {
    pub fn new(initial_document: CadDocument) -> Self {
        let root_evidence = validate_candidate(&initial_document)
            .ok()
            .filter(ValidationEvidence::passed);
        let root = SemanticCommit {
            id: 0,
            parent: None,
            task_id: None,
            change_set_id: None,
            agent_run_id: None,
            intent: "Project created".into(),
            transaction: CommandTransaction::default(),
            diff: DocumentDiff::default(),
            validation: ValidationReport::default(),
            evidence: root_evidence,
            preparation: None,
        };
        let mut commits = BTreeMap::new();
        commits.insert(0, root);
        let mut snapshots = BTreeMap::new();
        snapshots.insert(
            0,
            Snapshot {
                commit_id: 0,
                document: initial_document,
            },
        );
        let mut branches = BTreeMap::new();
        branches.insert(
            "main".into(),
            DesignBranch {
                name: "main".into(),
                head: 0,
            },
        );
        Self {
            commits,
            snapshots,
            branches,
            active_branch: "main".into(),
            redo_stacks: BTreeMap::new(),
            snapshot_interval: INITIAL_SNAPSHOT_INTERVAL,
            next_commit_id: 1,
        }
    }

    pub fn head(&self) -> CommitId {
        self.branches[&self.active_branch].head
    }

    pub fn compare(
        &self,
        base_commit: CommitId,
        target_commit: CommitId,
    ) -> Result<HistoryComparison, HistoryError> {
        let base = self.restore(base_commit)?;
        let target = self.restore(target_commit)?;
        Ok(HistoryComparison::between(
            base_commit,
            target_commit,
            &base,
            &target,
        ))
    }

    /// Migrates every historical document before verifying the semantic log.
    pub(crate) fn migrate_to_current(
        &mut self,
        regenerate_validation_evidence: bool,
    ) -> Result<(), HistoryError> {
        for snapshot in self.snapshots.values_mut() {
            snapshot.document.migrate_to_current()?;
        }
        if self.snapshot_interval == 0 {
            self.snapshot_interval = INITIAL_SNAPSHOT_INTERVAL;
        }
        if self.next_commit_id == 0 {
            self.next_commit_id = default_next_commit_id();
        }
        self.normalize_next_commit_id()?;
        if regenerate_validation_evidence {
            self.regenerate_validation_evidence()?;
        }
        self.validate_integrity()
    }

    pub(crate) fn infer_legacy_idempotency_keys(&mut self) -> Result<(), HistoryError> {
        let commit_ids = self
            .commits
            .keys()
            .copied()
            .filter(|id| *id != 0)
            .collect::<Vec<_>>();
        let mut inferred = Vec::new();
        for commit_id in commit_ids {
            let commit = self
                .commits
                .get(&commit_id)
                .ok_or(HistoryError::CommitMissing(commit_id))?;
            if commit.preparation.is_some() {
                continue;
            }
            inferred.push((commit_id, self.prepare_record_for_commit(commit_id)?));
        }
        for (commit_id, preparation) in inferred {
            self.commits
                .get_mut(&commit_id)
                .ok_or(HistoryError::CommitMissing(commit_id))?
                .preparation = Some(preparation);
        }
        Ok(())
    }

    pub(crate) fn bind_legacy_task_commit_sources(
        &mut self,
        sources: &BTreeMap<CommitId, ActionSource>,
    ) -> Result<(), HistoryError> {
        for commit in self.commits.values_mut().filter(|commit| commit.id != 0) {
            match (commit.task_id, sources.get(&commit.id).copied()) {
                (Some(task_id), Some(source)) if source.task_id == task_id => {
                    commit.change_set_id = source.change_set_id;
                    commit.agent_run_id = source.agent_run_id;
                }
                (Some(_), Some(_)) => {
                    return Err(HistoryError::InvalidHistory(format!(
                        "commit {} migration source belongs to another task",
                        commit.id
                    )));
                }
                (Some(_), None) => {
                    return Err(HistoryError::InvalidHistory(format!(
                        "task commit {} has no legacy run ownership",
                        commit.id
                    )));
                }
                (None, Some(_)) => {
                    return Err(HistoryError::InvalidHistory(format!(
                        "user commit {} cannot be assigned to an agent run",
                        commit.id
                    )));
                }
                (None, None) => {}
            }
        }
        if sources
            .keys()
            .any(|commit_id| !self.commits.contains_key(commit_id))
        {
            return Err(HistoryError::InvalidHistory(
                "legacy run ownership references a missing commit".into(),
            ));
        }
        let commit_ids = self
            .commits
            .keys()
            .copied()
            .filter(|commit_id| *commit_id != 0)
            .collect::<Vec<_>>();
        let preparations = commit_ids
            .into_iter()
            .map(|commit_id| Ok((commit_id, self.prepare_record_for_commit(commit_id)?)))
            .collect::<Result<Vec<_>, HistoryError>>()?;
        for (commit_id, preparation) in preparations {
            self.commits
                .get_mut(&commit_id)
                .ok_or(HistoryError::CommitMissing(commit_id))?
                .preparation = Some(preparation);
        }
        self.validate_integrity()
    }

    pub(crate) fn commit_for_idempotency_key(
        &self,
        key: ActionIdempotencyKey,
    ) -> Option<&SemanticCommit> {
        self.commits
            .values()
            .find(|commit| commit.idempotency_key() == Some(key))
    }

    /// Verifies that every commit is replayable, every snapshot matches its
    /// transaction history, and branch references are internally consistent.
    pub fn validate_integrity(&self) -> Result<(), HistoryError> {
        let root = self
            .commits
            .get(&0)
            .ok_or_else(|| HistoryError::InvalidHistory("missing root commit".into()))?;
        if root.id != 0
            || root.parent.is_some()
            || !root.transaction.commands.is_empty()
            || root.task_id.is_some()
            || root.change_set_id.is_some()
            || root.agent_run_id.is_some()
            || root.preparation.is_some()
        {
            return Err(HistoryError::InvalidHistory(
                "root commit must be an empty parentless transaction".into(),
            ));
        }
        let root_snapshot = self
            .snapshots
            .get(&0)
            .ok_or_else(|| HistoryError::InvalidHistory("missing root snapshot".into()))?;
        if root_snapshot.commit_id != 0 {
            return Err(HistoryError::InvalidHistory(
                "root snapshot has an invalid commit id".into(),
            ));
        }
        root_snapshot.document.validate()?;
        verify_validation_evidence(0, root, &root_snapshot.document)?;

        let mut restored = BTreeMap::new();
        restored.insert(0, root_snapshot.document.clone());
        for (id, commit) in self.commits.iter().filter(|(id, _)| **id != 0) {
            if *id != commit.id {
                return Err(HistoryError::InvalidHistory(format!(
                    "commit map key {id} does not match commit id {}",
                    commit.id
                )));
            }
            let parent = commit.parent.ok_or_else(|| {
                HistoryError::InvalidHistory(format!("commit {id} is missing a parent"))
            })?;
            if parent >= *id {
                return Err(HistoryError::InvalidHistory(format!(
                    "commit {id} has a non-ancestral parent {parent}"
                )));
            }
            let mut document = restored.get(&parent).cloned().ok_or_else(|| {
                HistoryError::InvalidHistory(format!("commit {id} has a missing parent {parent}"))
            })?;
            let calculated_diff = commit.transaction.apply(&mut document)?;
            if calculated_diff != commit.diff {
                return Err(HistoryError::InvalidHistory(format!(
                    "commit {id} has a diff that does not match its transaction"
                )));
            }
            verify_validation_evidence(*id, commit, &document)?;
            if let Some(preparation) = &commit.preparation {
                self.verify_preparation(
                    parent,
                    commit.action_source(),
                    &commit.transaction,
                    preparation,
                )?;
            }
            restored.insert(*id, document);
        }
        for (id, snapshot) in &self.snapshots {
            if *id != snapshot.commit_id || !self.commits.contains_key(id) {
                return Err(HistoryError::InvalidHistory(format!(
                    "snapshot {id} does not reference an existing matching commit"
                )));
            }
            snapshot.document.validate()?;
            if restored.get(id) != Some(&snapshot.document) {
                return Err(HistoryError::InvalidHistory(format!(
                    "snapshot {id} does not match replayed history"
                )));
            }
        }
        for (name, branch) in &self.branches {
            if name.trim().is_empty() || name != &branch.name {
                return Err(HistoryError::InvalidHistory(
                    "branch names must be non-empty and match their map keys".into(),
                ));
            }
            if !self.commits.contains_key(&branch.head) {
                return Err(HistoryError::InvalidHistory(format!(
                    "branch {name} points to missing commit {}",
                    branch.head
                )));
            }
        }
        if !self.branches.contains_key(&self.active_branch) {
            return Err(HistoryError::InvalidHistory(
                "active branch does not exist".into(),
            ));
        }
        for (branch_name, stack) in &self.redo_stacks {
            let branch = self.branches.get(branch_name).ok_or_else(|| {
                HistoryError::InvalidHistory(format!(
                    "redo stack references missing branch {branch_name}"
                ))
            })?;
            let mut parent = branch.head;
            for commit_id in stack.iter().rev() {
                let commit = self.commits.get(commit_id).ok_or_else(|| {
                    HistoryError::InvalidHistory(format!(
                        "redo stack on branch {branch_name} references missing commit {commit_id}"
                    ))
                })?;
                if commit.parent != Some(parent) {
                    return Err(HistoryError::InvalidHistory(format!(
                        "redo stack on branch {branch_name} is not contiguous at commit {commit_id}"
                    )));
                }
                parent = *commit_id;
            }
        }
        if self.snapshot_interval == 0 {
            return Err(HistoryError::InvalidHistory(
                "snapshot interval must be greater than zero".into(),
            ));
        }
        let minimum_next = next_id_after("commit", self.commits.keys().copied())?;
        if self.next_commit_id < minimum_next {
            return Err(HistoryError::InvalidHistory(
                "next commit id is behind existing history".into(),
            ));
        }
        Ok(())
    }

    fn normalize_next_commit_id(&mut self) -> Result<(), HistoryError> {
        let minimum_next = next_id_after("commit", self.commits.keys().copied())?;
        self.next_commit_id = self.next_commit_id.max(minimum_next);
        Ok(())
    }

    pub fn commit(
        &mut self,
        document: &CadDocument,
        task_id: Option<TaskId>,
        intent: impl Into<String>,
        transaction: CommandTransaction,
        validation: ValidationReport,
    ) -> Result<(CadDocument, CommitId), HistoryError> {
        self.commit_with_idempotency(
            document,
            task_id.map(ActionSource::legacy_task),
            intent,
            transaction,
            validation,
            None,
        )
    }

    pub(crate) fn commit_with_idempotency(
        &mut self,
        document: &CadDocument,
        source: Option<ActionSource>,
        intent: impl Into<String>,
        transaction: CommandTransaction,
        validation: ValidationReport,
        prepared_action: Option<PreparedActionRecord>,
    ) -> Result<(CadDocument, CommitId), HistoryError> {
        document.validate()?;
        let parent = self.active_head()?;
        if self.next_commit_id == CommitId::MAX {
            return Err(HistoryError::InvalidHistory(
                "commit id space is exhausted".into(),
            ));
        }
        let mut next_document = document.clone();
        let diff = transaction.apply(&mut next_document)?;
        let evidence = validate_candidate(&next_document)
            .map_err(HistoryError::CandidateValidationUnavailable)?;
        if !evidence.passed() {
            return Err(HistoryError::CandidateValidationFailed(evidence.summary()));
        }
        let preparation = match prepared_action {
            Some(preparation) => {
                self.verify_preparation(parent, source, &transaction, &preparation)?;
                preparation
            }
            None => PreparedAction::prepare_with_source(
                &crate::DocumentSnapshot::new(
                    parent,
                    document.clone(),
                    self.object_versions_at(parent)?,
                ),
                source,
                transaction.clone(),
            )
            .map_err(|error| HistoryError::CandidateValidationUnavailable(error.to_string()))?
            .record(),
        };
        let id = self.next_commit_id;
        self.next_commit_id += 1;
        let commit = SemanticCommit {
            id,
            parent: Some(parent),
            task_id: source.map(|source| source.task_id),
            change_set_id: source.and_then(|source| source.change_set_id),
            agent_run_id: source.and_then(|source| source.agent_run_id),
            intent: intent.into(),
            transaction,
            diff,
            validation,
            evidence: Some(evidence),
            preparation: Some(preparation),
        };
        self.commits.insert(id, commit);
        self.branches
            .get_mut(&self.active_branch)
            .ok_or_else(|| HistoryError::BranchMissing(self.active_branch.clone()))?
            .head = id;
        self.redo_stacks.remove(&self.active_branch);
        if self.snapshot_interval != 0 && id.is_multiple_of(self.snapshot_interval) {
            self.snapshots.insert(
                id,
                Snapshot {
                    commit_id: id,
                    document: next_document.clone(),
                },
            );
        }
        Ok((next_document, id))
    }

    fn prepare_record_for_commit(
        &self,
        commit_id: CommitId,
    ) -> Result<PreparedActionRecord, HistoryError> {
        let commit = self
            .commits
            .get(&commit_id)
            .ok_or(HistoryError::CommitMissing(commit_id))?;
        let parent = commit.parent.ok_or_else(|| {
            HistoryError::InvalidHistory(format!(
                "commit {commit_id} is missing a parent for action preparation migration"
            ))
        })?;
        let snapshot = crate::DocumentSnapshot::new(
            parent,
            self.restore(parent)?,
            self.object_versions_at(parent)?,
        );
        PreparedAction::prepare_with_source(
            &snapshot,
            commit.action_source(),
            commit.transaction.clone(),
        )
        .map(|prepared| prepared.record())
        .map_err(|error| HistoryError::CandidateValidationUnavailable(error.to_string()))
    }

    fn verify_preparation(
        &self,
        parent: CommitId,
        source: Option<ActionSource>,
        transaction: &CommandTransaction,
        actual: &PreparedActionRecord,
    ) -> Result<(), HistoryError> {
        if !self.is_ancestor(actual.base_revision(), parent)? {
            return Err(HistoryError::InvalidHistory(format!(
                "action preparation base {} is not an ancestor of commit parent {parent}",
                actual.base_revision()
            )));
        }
        let snapshot = crate::DocumentSnapshot::new(
            actual.base_revision(),
            self.restore(actual.base_revision())?,
            self.object_versions_at(actual.base_revision())?,
        );
        let expected = PreparedAction::prepare_with_source(&snapshot, source, transaction.clone())
            .map_err(|error| HistoryError::CandidateValidationUnavailable(error.to_string()))?
            .record();
        if &expected != actual {
            return Err(HistoryError::InvalidHistory(
                "action preparation does not match its base revision and transaction".into(),
            ));
        }
        Ok(())
    }

    pub fn create_branch(
        &mut self,
        name: impl Into<String>,
        from: CommitId,
    ) -> Result<(), HistoryError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(HistoryError::InvalidBranchName);
        }
        if self.branches.contains_key(&name) {
            return Err(HistoryError::BranchExists(name));
        }
        if !self.commits.contains_key(&from) {
            return Err(HistoryError::CommitMissing(from));
        }
        self.branches
            .insert(name.clone(), DesignBranch { name, head: from });
        Ok(())
    }

    pub fn checkout_branch(&mut self, name: &str) -> Result<CadDocument, HistoryError> {
        let branch = self
            .branches
            .get(name)
            .ok_or_else(|| HistoryError::BranchMissing(name.into()))?;
        let document = self.restore(branch.head)?;
        self.active_branch = name.into();
        Ok(document)
    }

    pub fn can_undo(&self) -> bool {
        self.commits
            .get(&self.head())
            .is_some_and(|commit| commit.parent.is_some())
    }

    pub fn can_redo(&self) -> bool {
        self.redo_stacks
            .get(&self.active_branch)
            .is_some_and(|stack| !stack.is_empty())
    }

    /// Moves the active branch to its parent without deleting semantic history.
    /// The abandoned head is retained on a branch-local redo stack.
    pub fn undo(&mut self) -> Result<(CadDocument, CommitId), HistoryError> {
        let current = self.active_head()?;
        let parent = self
            .commits
            .get(&current)
            .ok_or(HistoryError::CommitMissing(current))?
            .parent
            .ok_or(HistoryError::NothingToUndo)?;
        let document = self.restore(parent)?;
        self.branches
            .get_mut(&self.active_branch)
            .ok_or_else(|| HistoryError::BranchMissing(self.active_branch.clone()))?
            .head = parent;
        self.redo_stacks
            .entry(self.active_branch.clone())
            .or_default()
            .push(current);
        Ok((document, parent))
    }

    /// Replays the most recently undone commit on the active branch.
    pub fn redo(&mut self) -> Result<(CadDocument, CommitId), HistoryError> {
        let current = self.active_head()?;
        let commit_id = self
            .redo_stacks
            .get(&self.active_branch)
            .and_then(|stack| stack.last())
            .copied()
            .ok_or(HistoryError::NothingToRedo)?;
        let commit = self
            .commits
            .get(&commit_id)
            .ok_or(HistoryError::CommitMissing(commit_id))?;
        if commit.parent != Some(current) {
            return Err(HistoryError::InvalidHistory(format!(
                "redo commit {commit_id} does not follow active head {current}"
            )));
        }
        let document = self.restore(commit_id)?;
        self.branches
            .get_mut(&self.active_branch)
            .ok_or_else(|| HistoryError::BranchMissing(self.active_branch.clone()))?
            .head = commit_id;
        let stack = self
            .redo_stacks
            .get_mut(&self.active_branch)
            .ok_or(HistoryError::NothingToRedo)?;
        stack.pop();
        if stack.is_empty() {
            self.redo_stacks.remove(&self.active_branch);
        }
        Ok((document, commit_id))
    }

    pub fn restore(&self, target: CommitId) -> Result<CadDocument, HistoryError> {
        if !self.commits.contains_key(&target) {
            return Err(HistoryError::CommitMissing(target));
        }
        let mut replay = Vec::new();
        let mut visited = BTreeSet::new();
        let mut cursor = target;
        let snapshot = loop {
            if !visited.insert(cursor) {
                return Err(HistoryError::InvalidHistory(
                    "commit ancestry contains a cycle".into(),
                ));
            }
            if let Some(snapshot) = self.snapshots.get(&cursor) {
                break snapshot;
            }
            let commit = self
                .commits
                .get(&cursor)
                .ok_or(HistoryError::CommitMissing(cursor))?;
            replay.push(cursor);
            cursor = commit.parent.ok_or_else(|| {
                HistoryError::InvalidHistory("root commit is missing a snapshot".into())
            })?;
        };
        let mut document = snapshot.document.clone();
        document.validate()?;
        for commit_id in replay.into_iter().rev() {
            let commit = self
                .commits
                .get(&commit_id)
                .ok_or(HistoryError::CommitMissing(commit_id))?;
            commit.transaction.apply(&mut document)?;
        }
        Ok(document)
    }

    pub fn ordered_commits(&self) -> Vec<&SemanticCommit> {
        self.commits.values().collect()
    }

    pub(crate) fn object_versions_at(
        &self,
        target: CommitId,
    ) -> Result<ObjectVersionIndex, HistoryError> {
        if !self.commits.contains_key(&target) {
            return Err(HistoryError::CommitMissing(target));
        }
        let root = self
            .snapshots
            .get(&0)
            .ok_or_else(|| HistoryError::InvalidHistory("missing root snapshot".into()))?;
        let mut path = Vec::new();
        let mut visited = BTreeSet::new();
        let mut cursor = target;
        while cursor != 0 {
            if !visited.insert(cursor) {
                return Err(HistoryError::InvalidHistory(
                    "commit ancestry contains a cycle".into(),
                ));
            }
            let commit = self
                .commits
                .get(&cursor)
                .ok_or(HistoryError::CommitMissing(cursor))?;
            path.push(commit);
            cursor = commit.parent.ok_or_else(|| {
                HistoryError::InvalidHistory("non-root commit is missing a parent".into())
            })?;
        }
        let mut versions = ObjectVersionIndex::from_root(&root.document);
        for commit in path.into_iter().rev() {
            versions.apply_transaction(commit.id, &commit.transaction);
        }
        Ok(versions)
    }

    pub fn is_ancestor(
        &self,
        ancestor: CommitId,
        descendant: CommitId,
    ) -> Result<bool, HistoryError> {
        if !self.commits.contains_key(&ancestor) {
            return Err(HistoryError::CommitMissing(ancestor));
        }
        let mut visited = BTreeSet::new();
        let mut cursor = descendant;
        loop {
            if !visited.insert(cursor) {
                return Err(HistoryError::InvalidHistory(
                    "commit ancestry contains a cycle".into(),
                ));
            }
            if cursor == ancestor {
                return Ok(true);
            }
            let commit = self
                .commits
                .get(&cursor)
                .ok_or(HistoryError::CommitMissing(cursor))?;
            let Some(parent) = commit.parent else {
                return Ok(false);
            };
            cursor = parent;
        }
    }

    pub(crate) fn active_head(&self) -> Result<CommitId, HistoryError> {
        self.branches
            .get(&self.active_branch)
            .map(|branch| branch.head)
            .ok_or_else(|| HistoryError::BranchMissing(self.active_branch.clone()))
    }

    fn regenerate_validation_evidence(&mut self) -> Result<(), HistoryError> {
        let root_document = self
            .snapshots
            .get(&0)
            .ok_or_else(|| HistoryError::InvalidHistory("missing root snapshot".into()))?
            .document
            .clone();
        let root_evidence = validate_candidate(&root_document)
            .map_err(HistoryError::CandidateValidationUnavailable)?;
        if !root_evidence.passed() {
            return Err(HistoryError::CandidateValidationFailed(
                root_evidence.summary(),
            ));
        }
        self.commits
            .get_mut(&0)
            .ok_or_else(|| HistoryError::InvalidHistory("missing root commit".into()))?
            .evidence = Some(root_evidence);

        let mut restored = BTreeMap::from([(0, root_document)]);
        let commit_ids = self
            .commits
            .keys()
            .copied()
            .filter(|id| *id != 0)
            .collect::<Vec<_>>();
        for id in commit_ids {
            let commit = self
                .commits
                .get(&id)
                .ok_or(HistoryError::CommitMissing(id))?;
            let parent = commit.parent.ok_or_else(|| {
                HistoryError::InvalidHistory(format!("commit {id} is missing a parent"))
            })?;
            let transaction = commit.transaction.clone();
            let mut document = restored.get(&parent).cloned().ok_or_else(|| {
                HistoryError::InvalidHistory(format!("commit {id} has a missing parent {parent}"))
            })?;
            transaction.apply(&mut document)?;
            let evidence = validate_candidate(&document)
                .map_err(HistoryError::CandidateValidationUnavailable)?;
            if !evidence.passed() {
                return Err(HistoryError::CandidateValidationFailed(evidence.summary()));
            }
            self.commits
                .get_mut(&id)
                .expect("commit id was collected from the same map")
                .evidence = Some(evidence);
            restored.insert(id, document);
        }
        Ok(())
    }
}

fn verify_validation_evidence(
    commit_id: CommitId,
    commit: &SemanticCommit,
    document: &CadDocument,
) -> Result<(), HistoryError> {
    let actual = commit.evidence.as_ref().ok_or_else(|| {
        HistoryError::InvalidHistory(format!(
            "commit {commit_id} is missing local validation evidence"
        ))
    })?;
    if !actual.is_current() {
        return Err(HistoryError::InvalidHistory(format!(
            "commit {commit_id} uses unsupported validator {} version {}",
            actual.validator_id(),
            actual.validator_version()
        )));
    }
    let expected = validate_candidate(document).map_err(|error| {
        HistoryError::InvalidHistory(format!("commit {commit_id} cannot be revalidated: {error}"))
    })?;
    if !expected.passed() {
        return Err(HistoryError::InvalidHistory(format!(
            "commit {commit_id} fails local candidate validation: {}",
            expected.summary()
        )));
    }
    if actual != &expected {
        return Err(HistoryError::InvalidHistory(format!(
            "commit {commit_id} validation evidence does not match its replayed candidate state"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryError {
    Command(CommandError),
    CommitMissing(CommitId),
    BranchExists(String),
    BranchMissing(String),
    InvalidBranchName,
    NothingToUndo,
    NothingToRedo,
    CandidateValidationUnavailable(String),
    CandidateValidationFailed(String),
    InvalidHistory(String),
}

impl From<CommandError> for HistoryError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => error.fmt(formatter),
            Self::CommitMissing(id) => write!(formatter, "commit {id} does not exist"),
            Self::BranchExists(name) => write!(formatter, "branch {name} already exists"),
            Self::BranchMissing(name) => write!(formatter, "branch {name} does not exist"),
            Self::InvalidBranchName => formatter.write_str("branch name cannot be empty"),
            Self::NothingToUndo => formatter.write_str("active branch is already at its root"),
            Self::NothingToRedo => formatter.write_str("active branch has no change to redo"),
            Self::CandidateValidationUnavailable(message) => {
                write!(
                    formatter,
                    "local candidate validation is unavailable: {message}"
                )
            }
            Self::CandidateValidationFailed(summary) => {
                write!(formatter, "local candidate validation failed: {summary}")
            }
            Self::InvalidHistory(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for HistoryError {}
