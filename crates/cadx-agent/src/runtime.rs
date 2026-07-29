use std::collections::BTreeSet;
use std::fmt;

use cadx_core::{
    ActionFailureFeedback, ActionFailureKind, AgentKind, AgentRunId, AgentRunIdentity, Capability,
    CommitId, MAX_AUTOMATIC_REPAIR_ATTEMPTS, PrepareError, PromptChangeSetId, RemoteGrantId,
    TaskAction, TaskId, TaskStatus, TaskWorkspace, WorkspaceError,
};

use crate::error::AgentError;
use crate::provider::{
    AgentObservation, ExecutionBudget, PlanningDecision, ProviderConfig, ProviderDisclosure,
    RemoteContext, RemoteTaskPlanner, TaskPlanner, prepare_remote_context,
};
use crate::remote_plan::{RemotePlanningDecision, materialize_decision};

#[derive(Clone, Debug, Default)]
pub struct TaskAgent<P> {
    planner: P,
}

impl<P> TaskAgent<P> {
    pub fn new(planner: P) -> Self {
        Self { planner }
    }
}

impl<P> TaskAgent<P>
where
    P: TaskPlanner,
{
    pub fn run(
        &self,
        workspace: &mut TaskWorkspace,
        task_id: TaskId,
    ) -> Result<AgentRunReport, AgentError> {
        self.run_with_action_budget(workspace, task_id, None)
    }

    /// Runs a task for at most `action_budget` atomic actions. Reaching the
    /// budget pauses the task at a durable action boundary so it can be saved
    /// and resumed without asking the planner to recreate prior work.
    pub fn run_with_action_budget(
        &self,
        workspace: &mut TaskWorkspace,
        task_id: TaskId,
        action_budget: Option<usize>,
    ) -> Result<AgentRunReport, AgentError> {
        let status = workspace
            .task(task_id)
            .map(|task| task.status)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        match status {
            TaskStatus::Queued => {
                workspace.kernel().begin_iterative_task_as(
                    task_id,
                    AgentRunIdentity::local(std::any::type_name::<P>()),
                )?;
            }
            TaskStatus::Paused => workspace.kernel().resume_task(task_id)?,
            TaskStatus::Running => {}
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => {
                return Err(AgentError::Workspace(WorkspaceError::InvalidTaskState {
                    task_id,
                    expected: TaskStatus::Queued,
                    actual: status,
                }));
            }
        }

        if workspace
            .task(task_id)
            .and_then(|task| task.execution())
            .is_some_and(|execution| !execution.is_iterative())
        {
            return execute_batch_task_actions(workspace, task_id, action_budget);
        }
        execute_iterative_task_actions(workspace, task_id, action_budget, &self.planner)
    }
}

/// A single-use capability for one already authorized and audited provider call.
/// Its remote context is intentionally inaccessible to callers.
pub struct AuthorizedRemoteRound {
    task_id: TaskId,
    change_set_id: PromptChangeSetId,
    run_id: AgentRunId,
    source_revision: CommitId,
    config: ProviderConfig,
    requested_capabilities: BTreeSet<Capability>,
    context: RemoteContext,
}

impl fmt::Debug for AuthorizedRemoteRound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizedRemoteRound")
            .field("task_id", &self.task_id)
            .field("change_set_id", &self.change_set_id)
            .field("run_id", &self.run_id)
            .field("source_revision", &self.source_revision)
            .finish_non_exhaustive()
    }
}

pub struct RemoteRoundOutput {
    task_id: TaskId,
    change_set_id: PromptChangeSetId,
    run_id: AgentRunId,
    source_revision: CommitId,
    requested_capabilities: BTreeSet<Capability>,
    decision: RemotePlanningDecision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteRoundApply {
    ActionCommitted { commit_id: CommitId },
    ActionRejected { feedback: ActionFailureFeedback },
    Completed,
}

impl<P> TaskAgent<P>
where
    P: RemoteTaskPlanner,
{
    /// Builds the exact first-round disclosure on a disposable workspace clone.
    pub fn remote_disclosure(
        &self,
        workspace: &TaskWorkspace,
        task_id: TaskId,
    ) -> Result<ProviderDisclosure, AgentError> {
        let mut preview = workspace.clone();
        let (_, disclosure) =
            self.prepare_remote_round_context(&mut preview, task_id, ExecutionBudget::default())?;
        Ok(disclosure)
    }

    pub fn create_remote_access_grant(
        &self,
        workspace: &mut TaskWorkspace,
        task_id: TaskId,
        reviewed_disclosure: &ProviderDisclosure,
        granted_at_unix_seconds: u64,
        expires_at_unix_seconds: Option<u64>,
    ) -> Result<RemoteGrantId, AgentError> {
        let current_disclosure = self.remote_disclosure(workspace, task_id)?;
        if current_disclosure != *reviewed_disclosure {
            return Err(AgentError::DisclosureDoesNotMatch(task_id));
        }
        workspace
            .kernel()
            .create_remote_access_grant(
                current_disclosure.grant_request(granted_at_unix_seconds, expires_at_unix_seconds),
            )
            .map_err(AgentError::Workspace)
    }

    pub fn matching_remote_access_grant(
        &self,
        workspace: &TaskWorkspace,
        disclosure: &ProviderDisclosure,
        unix_seconds: u64,
    ) -> Option<RemoteGrantId> {
        workspace
            .remote_access_grants()
            .iter()
            .rev()
            .find_map(|(grant_id, grant)| {
                disclosure
                    .is_authorized_by(grant, unix_seconds)
                    .then_some(*grant_id)
            })
    }

    pub fn validate_remote_access_grant(
        &self,
        workspace: &TaskWorkspace,
        task_id: TaskId,
        grant_id: RemoteGrantId,
        unix_seconds: u64,
    ) -> Result<ProviderDisclosure, AgentError> {
        let disclosure = self.remote_disclosure(workspace, task_id)?;
        let grant = workspace
            .remote_access_grants()
            .get(&grant_id)
            .ok_or(AgentError::RemoteGrantDoesNotAuthorize(grant_id))?;
        if !disclosure.is_authorized_by(grant, unix_seconds) {
            return Err(AgentError::RemoteGrantDoesNotAuthorize(grant_id));
        }
        Ok(disclosure)
    }

    /// Re-observes, re-authorizes, and persists the exact audit before yielding
    /// a single-use capability that can perform one provider call.
    pub fn prepare_authorized_remote_round(
        &self,
        workspace: &mut TaskWorkspace,
        task_id: TaskId,
        grant_id: RemoteGrantId,
        unix_seconds: u64,
        budget: ExecutionBudget,
    ) -> Result<AuthorizedRemoteRound, AgentError> {
        budget.validate()?;
        let mut preview = workspace.clone();
        let (_, preview_disclosure) =
            self.prepare_remote_round_context(&mut preview, task_id, budget)?;
        let grant = workspace
            .remote_access_grants()
            .get(&grant_id)
            .ok_or(AgentError::RemoteGrantDoesNotAuthorize(grant_id))?;
        if !preview_disclosure.is_authorized_by(grant, unix_seconds) {
            return Err(AgentError::RemoteGrantDoesNotAuthorize(grant_id));
        }

        let (context, disclosure) =
            self.prepare_remote_round_context(workspace, task_id, budget)?;
        debug_assert_eq!(disclosure, preview_disclosure);
        workspace.kernel().record_event(
            task_id,
            disclosure.granted_audit_event(grant_id, unix_seconds),
        )?;
        Ok(AuthorizedRemoteRound {
            task_id,
            change_set_id: disclosure.change_set_id,
            run_id: disclosure.run_id,
            source_revision: disclosure.source_revision,
            config: disclosure.config,
            requested_capabilities: disclosure.requested_capabilities,
            context,
        })
    }

    /// Consumes one authorized token. No public API exposes its context for reuse.
    pub fn plan_authorized_remote_round(
        &self,
        round: AuthorizedRemoteRound,
    ) -> Result<RemoteRoundOutput, AgentError> {
        if self.planner.config() != &round.config {
            return Err(AgentError::Provider(
                "authorized remote round does not match this planner configuration".into(),
            ));
        }
        let decision = self.planner.plan_remote(round.context)?;
        Ok(RemoteRoundOutput {
            task_id: round.task_id,
            change_set_id: round.change_set_id,
            run_id: round.run_id,
            source_revision: round.source_revision,
            requested_capabilities: round.requested_capabilities,
            decision,
        })
    }

    /// Materializes and commits one provider decision on the authoritative local
    /// workspace. Object preconditions arbitrate edits made after observation.
    pub fn apply_remote_round_output(
        &self,
        workspace: &mut TaskWorkspace,
        output: RemoteRoundOutput,
    ) -> Result<RemoteRoundApply, AgentError> {
        let task = workspace
            .task(output.task_id)
            .ok_or(WorkspaceError::TaskMissing(output.task_id))?;
        let change_set = task.active_change_set().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {} does not have an active change set",
                output.task_id
            ))
        })?;
        let run = change_set.active_run().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {} does not have an active run",
                output.task_id
            ))
        })?;
        if change_set.id != output.change_set_id || run.id != output.run_id {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "remote decision does not belong to task {} active run",
                output.task_id
            ))
            .into());
        }
        let observed_document = workspace
            .history()
            .restore(output.source_revision)
            .map_err(WorkspaceError::History)?;
        let decision = materialize_decision(
            output.decision,
            &observed_document,
            &output.requested_capabilities,
        )?;
        match decision {
            PlanningDecision::Complete { summary } => {
                workspace.kernel().finish_iterative_plan(
                    output.task_id,
                    output.source_revision,
                    summary,
                )?;
                workspace.kernel().complete_task(output.task_id)?;
                Ok(RemoteRoundApply::Completed)
            }
            PlanningDecision::Action(action) => {
                if let Err(error) = workspace.kernel().stage_iterative_action(
                    output.task_id,
                    output.source_revision,
                    action.clone(),
                ) {
                    return reject_remote_round_action(
                        workspace,
                        output.task_id,
                        output.source_revision,
                        &action,
                        error,
                    );
                }
                match workspace.kernel().apply_next_task_action(output.task_id) {
                    Ok(Some(commit_id)) => Ok(RemoteRoundApply::ActionCommitted { commit_id }),
                    Ok(None) => Err(WorkspaceError::InvalidWorkspace(format!(
                        "task {} lost its staged remote action",
                        output.task_id
                    ))
                    .into()),
                    Err(error) => reject_remote_round_action(
                        workspace,
                        output.task_id,
                        output.source_revision,
                        &action,
                        error,
                    ),
                }
            }
        }
    }

    pub fn run_remote_with_grant(
        &self,
        workspace: &mut TaskWorkspace,
        task_id: TaskId,
        grant_id: RemoteGrantId,
        unix_seconds: u64,
        budget: ExecutionBudget,
    ) -> Result<AgentRunReport, AgentError> {
        budget.validate()?;
        let mut commit_ids = Vec::new();
        loop {
            if commit_ids.len() >= budget.max_actions_per_run {
                workspace
                    .kernel()
                    .pause_task(task_id, "Action budget reached")?;
                return active_report(workspace, task_id, TaskStatus::Paused, commit_ids);
            }
            let round = self.prepare_authorized_remote_round(
                workspace,
                task_id,
                grant_id,
                unix_seconds,
                budget,
            )?;
            let output = match self.plan_authorized_remote_round(round) {
                Ok(output) => output,
                Err(error) => {
                    workspace.kernel().fail_task(task_id, error.to_string())?;
                    return Err(error);
                }
            };
            match self.apply_remote_round_output(workspace, output) {
                Ok(RemoteRoundApply::ActionCommitted { commit_id }) => commit_ids.push(commit_id),
                Ok(RemoteRoundApply::ActionRejected { .. }) => {}
                Ok(RemoteRoundApply::Completed) => {
                    return active_report(workspace, task_id, TaskStatus::Completed, commit_ids);
                }
                Err(error) => {
                    if workspace
                        .task(task_id)
                        .is_some_and(|task| task.status == TaskStatus::Running)
                    {
                        workspace.kernel().fail_task(task_id, error.to_string())?;
                    }
                    return Err(error);
                }
            }
        }
    }

    fn prepare_remote_round_context(
        &self,
        workspace: &mut TaskWorkspace,
        task_id: TaskId,
        budget: ExecutionBudget,
    ) -> Result<(RemoteContext, ProviderDisclosure), AgentError> {
        budget.validate()?;
        let status = workspace
            .task(task_id)
            .map(|task| task.status)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        match status {
            TaskStatus::Queued => workspace.kernel().begin_iterative_task_as_with_budget(
                task_id,
                AgentRunIdentity::remote(
                    std::any::type_name::<P>(),
                    self.planner.config().endpoint.clone(),
                    self.planner.config().model.clone(),
                ),
                budget.planning_budget()?,
            )?,
            TaskStatus::Paused => workspace.kernel().resume_task(task_id)?,
            TaskStatus::Running => {}
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => {
                return Err(WorkspaceError::InvalidTaskState {
                    task_id,
                    expected: TaskStatus::Queued,
                    actual: status,
                }
                .into());
            }
        }
        let task = workspace
            .task(task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let run = task.active_run().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} has no active run"))
        })?;
        if run.identity.kind != AgentKind::Remote
            || run.identity.provider.as_deref() != Some(self.planner.config().endpoint.as_str())
            || run.identity.model.as_deref() != Some(self.planner.config().model.as_str())
            || task
                .execution()
                .is_none_or(|execution| !execution.is_iterative())
        {
            return Err(AgentError::Provider(
                "task is not bound to this remote iterative planner".into(),
            ));
        }
        let snapshot = workspace.snapshot();
        workspace
            .kernel()
            .record_iterative_observation(task_id, snapshot.revision())?;
        let observation = AgentObservation {
            task: workspace
                .task(task_id)
                .cloned()
                .ok_or(WorkspaceError::TaskMissing(task_id))?,
            snapshot,
        };
        prepare_remote_context(
            self.planner.config().clone(),
            self.planner.context_request(),
            workspace.project_id(),
            &observation,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRunReport {
    pub task_id: TaskId,
    pub change_set_id: PromptChangeSetId,
    pub run_id: AgentRunId,
    pub status: TaskStatus,
    pub commit_ids: Vec<u64>,
}

fn active_report(
    workspace: &TaskWorkspace,
    task_id: TaskId,
    status: TaskStatus,
    commit_ids: Vec<CommitId>,
) -> Result<AgentRunReport, AgentError> {
    let task = workspace
        .task(task_id)
        .ok_or(WorkspaceError::TaskMissing(task_id))?;
    let change_set = task.active_change_set().ok_or_else(|| {
        WorkspaceError::InvalidWorkspace(format!(
            "task {task_id} does not have an active change set"
        ))
    })?;
    let run = change_set.active_run().ok_or_else(|| {
        WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
    })?;
    Ok(AgentRunReport {
        task_id,
        change_set_id: change_set.id,
        run_id: run.id,
        status,
        commit_ids,
    })
}

fn reject_remote_round_action(
    workspace: &mut TaskWorkspace,
    task_id: TaskId,
    observed_revision: CommitId,
    action: &TaskAction,
    error: WorkspaceError,
) -> Result<RemoteRoundApply, AgentError> {
    let Some(kind) = repairable_failure_kind(&error) else {
        return Err(error.into());
    };
    let message = error.to_string();
    let will_retry = workspace.kernel().reject_iterative_action(
        task_id,
        observed_revision,
        action,
        kind,
        message.clone(),
    )?;
    if will_retry {
        let feedback = workspace
            .task(task_id)
            .and_then(|task| task.execution())
            .and_then(cadx_core::TaskExecution::last_failure)
            .cloned()
            .ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} lost its remote repair feedback"
                ))
            })?;
        return Ok(RemoteRoundApply::ActionRejected { feedback });
    }
    let exhausted = AgentError::AutomaticRepairExhausted {
        attempts: MAX_AUTOMATIC_REPAIR_ATTEMPTS,
        last_error: message,
    };
    workspace
        .kernel()
        .fail_task(task_id, exhausted.to_string())?;
    Err(exhausted)
}

fn execute_batch_task_actions(
    workspace: &mut TaskWorkspace,
    task_id: TaskId,
    action_budget: Option<usize>,
) -> Result<AgentRunReport, AgentError> {
    let task = workspace
        .task(task_id)
        .ok_or(WorkspaceError::TaskMissing(task_id))?;
    let change_set = task.active_change_set().ok_or_else(|| {
        WorkspaceError::InvalidWorkspace(format!(
            "task {task_id} does not have an active change set"
        ))
    })?;
    let change_set_id = change_set.id;
    let run_id = change_set
        .active_run()
        .ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?
        .id;
    let mut commit_ids = Vec::new();
    let mut executed_actions = 0;
    loop {
        if action_budget.is_some_and(|budget| executed_actions >= budget)
            && workspace.next_task_action(task_id)?.is_some()
        {
            workspace
                .kernel()
                .pause_task(task_id, "Action budget reached")?;
            return Ok(AgentRunReport {
                task_id,
                change_set_id,
                run_id,
                status: TaskStatus::Paused,
                commit_ids,
            });
        }
        let commit_id = match workspace.kernel().apply_next_task_action(task_id) {
            Ok(Some(commit_id)) => commit_id,
            Ok(None) => break,
            Err(error) => {
                workspace.kernel().fail_task(task_id, error.to_string())?;
                return Err(error.into());
            }
        };
        commit_ids.push(commit_id);
        executed_actions += 1;
    }
    workspace.kernel().complete_task(task_id)?;
    Ok(AgentRunReport {
        task_id,
        change_set_id,
        run_id,
        status: TaskStatus::Completed,
        commit_ids,
    })
}

fn execute_iterative_task_actions<P: TaskPlanner>(
    workspace: &mut TaskWorkspace,
    task_id: TaskId,
    action_budget: Option<usize>,
    planner: &P,
) -> Result<AgentRunReport, AgentError> {
    let (change_set_id, run_id) = active_run_ids(workspace, task_id)?;
    let mut commit_ids = Vec::new();
    let mut executed_actions = 0;
    loop {
        if action_budget.is_some_and(|budget| executed_actions >= budget) {
            workspace
                .kernel()
                .pause_task(task_id, "Action budget reached")?;
            return Ok(AgentRunReport {
                task_id,
                change_set_id,
                run_id,
                status: TaskStatus::Paused,
                commit_ids,
            });
        }

        let execution = workspace
            .task(task_id)
            .and_then(|task| task.execution())
            .ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} does not have an iterative execution"
                ))
            })?;
        if !execution.is_iterative() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} changed from iterative to batch execution"
            ))
            .into());
        }
        if execution.is_complete() {
            workspace.kernel().complete_task(task_id)?;
            return Ok(AgentRunReport {
                task_id,
                change_set_id,
                run_id,
                status: TaskStatus::Completed,
                commit_ids,
            });
        }

        if let Some(action) = workspace.next_task_action(task_id)? {
            let observed_revision = execution
                .next_action_preparation()
                .map_or(workspace.revision(), |preparation| {
                    preparation.base_revision()
                });
            match workspace.kernel().apply_next_task_action(task_id) {
                Ok(Some(commit_id)) => {
                    commit_ids.push(commit_id);
                    executed_actions += 1;
                }
                Ok(None) => {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "task {task_id} lost its staged iterative action"
                    ))
                    .into());
                }
                Err(error) => {
                    handle_iterative_action_failure(
                        workspace,
                        task_id,
                        observed_revision,
                        &action,
                        error,
                    )?;
                }
            }
            continue;
        }

        if !execution.is_awaiting_planner() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} has no action but is not waiting for its planner"
            ))
            .into());
        }
        let snapshot = workspace.snapshot();
        workspace
            .kernel()
            .record_iterative_observation(task_id, snapshot.revision())?;
        let observation = AgentObservation {
            task: workspace
                .task(task_id)
                .cloned()
                .ok_or(WorkspaceError::TaskMissing(task_id))?,
            snapshot: snapshot.clone(),
        };
        let decision = match planner.plan_next(&observation) {
            Ok(decision) => decision,
            Err(error) => {
                workspace.kernel().fail_task(task_id, error.to_string())?;
                return Err(error);
            }
        };
        match decision {
            PlanningDecision::Action(action) => {
                if let Err(error) = workspace.kernel().stage_iterative_action(
                    task_id,
                    snapshot.revision(),
                    action.clone(),
                ) {
                    handle_iterative_action_failure(
                        workspace,
                        task_id,
                        snapshot.revision(),
                        &action,
                        error,
                    )?;
                }
            }
            PlanningDecision::Complete { summary } => {
                workspace
                    .kernel()
                    .finish_iterative_plan(task_id, snapshot.revision(), summary)?;
            }
        }
    }
}

fn active_run_ids(
    workspace: &TaskWorkspace,
    task_id: TaskId,
) -> Result<(PromptChangeSetId, AgentRunId), WorkspaceError> {
    let task = workspace
        .task(task_id)
        .ok_or(WorkspaceError::TaskMissing(task_id))?;
    let change_set = task.active_change_set().ok_or_else(|| {
        WorkspaceError::InvalidWorkspace(format!(
            "task {task_id} does not have an active change set"
        ))
    })?;
    let run = change_set.active_run().ok_or_else(|| {
        WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
    })?;
    Ok((change_set.id, run.id))
}

fn handle_iterative_action_failure(
    workspace: &mut TaskWorkspace,
    task_id: TaskId,
    observed_revision: u64,
    action: &TaskAction,
    error: WorkspaceError,
) -> Result<(), AgentError> {
    let Some(kind) = repairable_failure_kind(&error) else {
        workspace.kernel().fail_task(task_id, error.to_string())?;
        return Err(error.into());
    };
    let message = error.to_string();
    let will_retry = workspace.kernel().reject_iterative_action(
        task_id,
        observed_revision,
        action,
        kind,
        message.clone(),
    )?;
    if will_retry {
        return Ok(());
    }
    let exhausted = AgentError::AutomaticRepairExhausted {
        attempts: MAX_AUTOMATIC_REPAIR_ATTEMPTS,
        last_error: message,
    };
    workspace
        .kernel()
        .fail_task(task_id, exhausted.to_string())?;
    Err(exhausted)
}

fn repairable_failure_kind(error: &WorkspaceError) -> Option<ActionFailureKind> {
    match error {
        WorkspaceError::Prepare(PrepareError::Command(_)) | WorkspaceError::Command(_) => {
            Some(ActionFailureKind::ToolRejected)
        }
        WorkspaceError::Prepare(PrepareError::ValidationFailed(_)) => {
            Some(ActionFailureKind::ValidationFailed)
        }
        WorkspaceError::StaleRevision { .. }
        | WorkspaceError::PreparedBaseNotAncestor { .. }
        | WorkspaceError::ObjectPreconditionFailed { .. }
        | WorkspaceError::IdempotencyConflict { .. } => Some(ActionFailureKind::StaleObservation),
        WorkspaceError::History(_)
        | WorkspaceError::TaskMissing(_)
        | WorkspaceError::ChangeSetMissing(_)
        | WorkspaceError::ChangeSetNotRevertible(_)
        | WorkspaceError::ChangeSetAlreadyReverted(_)
        | WorkspaceError::ChangeSetNotOnActiveBranch { .. }
        | WorkspaceError::Unauthorized(_)
        | WorkspaceError::HistoryNavigationBlocked(_)
        | WorkspaceError::PreparedInputMismatch { .. }
        | WorkspaceError::PreparedSourceMismatch
        | WorkspaceError::Prepare(PrepareError::ValidationUnavailable(_))
        | WorkspaceError::RemotePolicy(_)
        | WorkspaceError::InvalidTaskState { .. }
        | WorkspaceError::InvalidWorkspace(_) => None,
    }
}
