use std::fmt;

use crate::{
    ActionFailureKind, AgentRunId, AgentRunIdentity, ChangeSetRevertReport, CommandTransaction,
    CommitId, DocumentSnapshot, PreparedAction, PromptChangeSetId, RemoteAccessGrantRequest,
    RemoteGrantId, TaskAction, TaskAuthority, TaskEvent, TaskId, TaskPlanningBudget, TaskWorkspace,
    ValidationReport, WorkspaceError,
};

/// The only public mutable entry point into an authoritative workspace.
///
/// The facade borrows the workspace exclusively for the duration of a write
/// sequence. Callers can observe through `TaskWorkspace` and
/// `DocumentSnapshot`, but cannot obtain the underlying writable store.
pub struct KernelFacade<'workspace> {
    workspace: &'workspace mut TaskWorkspace,
}

impl<'workspace> KernelFacade<'workspace> {
    pub(crate) fn new(workspace: &'workspace mut TaskWorkspace) -> Self {
        Self { workspace }
    }

    pub fn revision(&self) -> CommitId {
        self.workspace.revision()
    }

    pub fn snapshot(&self) -> DocumentSnapshot {
        self.workspace.snapshot()
    }

    pub fn create_task(
        &mut self,
        title: impl Into<String>,
        goal: impl Into<String>,
        authority: TaskAuthority,
    ) -> TaskId {
        self.workspace.create_task(title, goal, authority)
    }

    pub fn create_remote_access_grant(
        &mut self,
        request: RemoteAccessGrantRequest,
    ) -> Result<RemoteGrantId, WorkspaceError> {
        self.workspace.create_remote_access_grant(request)
    }

    pub fn revoke_remote_access_grant(
        &mut self,
        grant_id: RemoteGrantId,
        revoked_at_unix_seconds: u64,
    ) -> Result<(), WorkspaceError> {
        self.workspace
            .revoke_remote_access_grant(grant_id, revoked_at_unix_seconds)
    }

    pub fn begin_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        self.workspace.begin_task(task_id)
    }

    pub fn begin_task_as(
        &mut self,
        task_id: TaskId,
        identity: AgentRunIdentity,
    ) -> Result<(), WorkspaceError> {
        self.workspace.begin_task_as(task_id, identity)
    }

    pub fn begin_iterative_task_as(
        &mut self,
        task_id: TaskId,
        identity: AgentRunIdentity,
    ) -> Result<(), WorkspaceError> {
        self.workspace.begin_iterative_task_as(task_id, identity)
    }

    pub fn begin_iterative_task_as_with_budget(
        &mut self,
        task_id: TaskId,
        identity: AgentRunIdentity,
        planning_budget: TaskPlanningBudget,
    ) -> Result<(), WorkspaceError> {
        self.workspace
            .begin_iterative_task_as_with_budget(task_id, identity, planning_budget)
    }

    pub fn add_prompt(
        &mut self,
        task_id: TaskId,
        prompt: impl Into<String>,
        authorization: TaskAuthority,
    ) -> Result<PromptChangeSetId, WorkspaceError> {
        self.workspace.add_prompt(task_id, prompt, authorization)
    }

    pub fn retry_active_change_set(
        &mut self,
        task_id: TaskId,
    ) -> Result<AgentRunId, WorkspaceError> {
        self.workspace.retry_active_change_set(task_id)
    }

    pub fn revert_change_set(
        &mut self,
        task_id: TaskId,
        change_set_id: PromptChangeSetId,
    ) -> Result<ChangeSetRevertReport, WorkspaceError> {
        self.workspace.revert_change_set(task_id, change_set_id)
    }

    pub fn resume_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        self.workspace.resume_task(task_id)
    }

    pub fn pause_task(
        &mut self,
        task_id: TaskId,
        reason: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        self.workspace.pause_task(task_id, reason)
    }

    pub fn set_task_plan(
        &mut self,
        task_id: TaskId,
        base_revision: CommitId,
        actions: Vec<TaskAction>,
    ) -> Result<(), WorkspaceError> {
        self.workspace
            .set_task_plan(task_id, base_revision, actions)
    }

    pub fn record_iterative_observation(
        &mut self,
        task_id: TaskId,
        revision: CommitId,
    ) -> Result<(), WorkspaceError> {
        self.workspace
            .record_iterative_observation(task_id, revision)
    }

    pub fn stage_iterative_action(
        &mut self,
        task_id: TaskId,
        observed_revision: CommitId,
        action: TaskAction,
    ) -> Result<(), WorkspaceError> {
        self.workspace
            .stage_iterative_action(task_id, observed_revision, action)
    }

    pub fn reject_iterative_action(
        &mut self,
        task_id: TaskId,
        observed_revision: CommitId,
        action: &TaskAction,
        kind: ActionFailureKind,
        message: impl Into<String>,
    ) -> Result<bool, WorkspaceError> {
        self.workspace
            .reject_iterative_action(task_id, observed_revision, action, kind, message)
    }

    pub fn finish_iterative_plan(
        &mut self,
        task_id: TaskId,
        observed_revision: CommitId,
        summary: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        self.workspace
            .finish_iterative_plan(task_id, observed_revision, summary)
    }

    pub fn record_event(
        &mut self,
        task_id: TaskId,
        event: TaskEvent,
    ) -> Result<(), WorkspaceError> {
        self.workspace.record_event(task_id, event)
    }

    pub fn apply_next_task_action(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<CommitId>, WorkspaceError> {
        self.workspace.apply_next_task_action(task_id)
    }

    pub fn apply_user_transaction(
        &mut self,
        expected_revision: CommitId,
        intent: impl Into<String>,
        transaction: CommandTransaction,
        validation: ValidationReport,
    ) -> Result<CommitId, WorkspaceError> {
        self.workspace
            .apply_user_transaction(expected_revision, intent, transaction, validation)
    }

    pub fn prepare_action(
        &self,
        transaction: CommandTransaction,
    ) -> Result<PreparedAction, WorkspaceError> {
        self.workspace.prepare_action(transaction)
    }

    pub fn commit_prepared_user_action(
        &mut self,
        intent: impl Into<String>,
        prepared: PreparedAction,
        validation: ValidationReport,
    ) -> Result<CommitId, WorkspaceError> {
        self.workspace
            .commit_prepared_user_action(intent, prepared, validation)
    }

    pub fn undo(&mut self) -> Result<CommitId, WorkspaceError> {
        self.workspace.undo()
    }

    pub fn redo(&mut self) -> Result<CommitId, WorkspaceError> {
        self.workspace.redo()
    }

    pub fn complete_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        self.workspace.complete_task(task_id)
    }

    pub fn fail_task(
        &mut self,
        task_id: TaskId,
        message: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        self.workspace.fail_task(task_id, message)
    }

    pub fn cancel_task(
        &mut self,
        task_id: TaskId,
        reason: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        self.workspace.cancel_task(task_id, reason)
    }

    pub fn fork_at(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), WorkspaceError> {
        self.workspace.fork_at(name, commit_id)
    }

    pub fn checkout_branch(&mut self, name: &str) -> Result<(), WorkspaceError> {
        self.workspace.checkout_branch(name)
    }

    pub fn checkout_as_branch(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), WorkspaceError> {
        self.workspace.checkout_as_branch(name, commit_id)
    }

    pub fn migrate_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.workspace.migrate_to_current()
    }

    pub fn migrate_legacy_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.workspace.migrate_legacy_to_current()
    }

    pub fn migrate_legacy_executions_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.workspace.migrate_legacy_executions_to_current()
    }

    pub fn migrate_legacy_object_preconditions_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.workspace
            .migrate_legacy_object_preconditions_to_current()
    }

    pub fn migrate_legacy_task_hierarchy_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.workspace.migrate_legacy_task_hierarchy_to_current()
    }

    pub fn migrate_legacy_execution_strategies(&mut self) {
        self.workspace.migrate_legacy_execution_strategies();
    }

    pub fn migrate_legacy_remote_policy(&mut self) {
        self.workspace.migrate_legacy_remote_policy();
    }

    pub fn migrate_legacy_planning_budgets(&mut self) {
        self.workspace.migrate_legacy_planning_budgets();
    }
}

impl fmt::Debug for KernelFacade<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KernelFacade")
            .field("revision", &self.workspace.revision())
            .finish_non_exhaustive()
    }
}
