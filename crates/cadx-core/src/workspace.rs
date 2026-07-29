use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::command::{CadCommand, CommandTransaction};
use crate::document::{CadDocument, CommandError, next_id_after};
use crate::history::{History, HistoryError};
use crate::object::transaction_writes;
use crate::remote_policy::RemoteAccessPolicy;
use crate::snapshot::DocumentSnapshot;
use crate::store::DocumentStore;
use crate::task::{
    ActionFailureFeedback, ActionFailureKind, AgentRun, AgentRunIdentity, AgentRunStatus,
    ChangeSetActionCommit, ChangeSetCompensation, ChangeSetDiagnostic, ChangeSetRevertReport,
    ChangeSetStatus, DesignTask, MAX_AUTOMATIC_REPAIR_ATTEMPTS, MAX_ITERATIVE_ACTIONS_PER_RUN,
    MAX_REMOTE_CONTEXT_BYTES, MIN_HASH_BOUND_REMOTE_CONTEXT_SCHEMA_VERSION, PromptChangeSet,
    REMOTE_CONTEXT_SCHEMA_VERSION, RevertConflict, RevertConflictReason, StructuredGoal,
    TaskAction, TaskAuthority, TaskEvent, TaskExecution, TaskExecutionStrategy, TaskPlanningBudget,
    TaskStatus, ValidationReport,
};
use crate::{
    ActionSource, AgentRunId, CommitId, ObjectId, ObjectPrecondition, PrepareError, PreparedAction,
    ProjectId, PromptChangeSetId, RemoteAccessCheck, RemoteAccessGrant, RemoteAccessGrantRequest,
    RemoteGrantId, RemotePolicyError, RemotePolicyEvent, TaskId,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TaskWorkspace {
    store: DocumentStore,
    project_id: ProjectId,
    remote_access_policy: RemoteAccessPolicy,
    pub(crate) tasks: BTreeMap<TaskId, DesignTask>,
    next_task_id: TaskId,
    next_change_set_id: PromptChangeSetId,
    next_agent_run_id: AgentRunId,
    legacy_missing_remote_policy: bool,
}

#[derive(Serialize)]
struct WorkspaceWireRef<'workspace> {
    document: &'workspace CadDocument,
    history: &'workspace History,
    project_id: ProjectId,
    remote_access_policy: &'workspace RemoteAccessPolicy,
    tasks: &'workspace BTreeMap<TaskId, DesignTask>,
    next_task_id: TaskId,
    next_change_set_id: PromptChangeSetId,
    next_agent_run_id: AgentRunId,
}

#[derive(Deserialize)]
struct WorkspaceWire {
    document: CadDocument,
    history: History,
    project_id: Option<ProjectId>,
    remote_access_policy: Option<RemoteAccessPolicy>,
    tasks: BTreeMap<TaskId, DesignTask>,
    #[serde(default = "default_next_task_id")]
    next_task_id: TaskId,
    #[serde(default = "default_next_change_set_id")]
    next_change_set_id: PromptChangeSetId,
    #[serde(default = "default_next_agent_run_id")]
    next_agent_run_id: AgentRunId,
}

impl Serialize for TaskWorkspace {
    fn serialize<SerializerType>(
        &self,
        serializer: SerializerType,
    ) -> Result<SerializerType::Ok, SerializerType::Error>
    where
        SerializerType: Serializer,
    {
        WorkspaceWireRef {
            document: self.store.document(),
            history: self.store.history(),
            project_id: self.project_id,
            remote_access_policy: &self.remote_access_policy,
            tasks: &self.tasks,
            next_task_id: self.next_task_id,
            next_change_set_id: self.next_change_set_id,
            next_agent_run_id: self.next_agent_run_id,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TaskWorkspace {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let wire = WorkspaceWire::deserialize(deserializer)?;
        let legacy_missing_remote_policy =
            wire.project_id.is_none() || wire.remote_access_policy.is_none();
        Ok(Self {
            store: DocumentStore::from_parts(wire.document, wire.history),
            project_id: wire.project_id.unwrap_or_default(),
            remote_access_policy: wire.remote_access_policy.unwrap_or_default(),
            tasks: wire.tasks,
            next_task_id: wire.next_task_id,
            next_change_set_id: wire.next_change_set_id,
            next_agent_run_id: wire.next_agent_run_id,
            legacy_missing_remote_policy,
        })
    }
}

const fn default_next_task_id() -> TaskId {
    1
}

const fn default_next_change_set_id() -> PromptChangeSetId {
    1
}

const fn default_next_agent_run_id() -> AgentRunId {
    1
}

fn latest_iterative_observation(run: &AgentRun) -> Option<(CommitId, usize)> {
    run.events.iter().rev().find_map(|event| {
        if let TaskEvent::Reobserved {
            revision,
            action_index,
            ..
        } = event
        {
            Some((*revision, *action_index))
        } else {
            None
        }
    })
}

fn pending_iterative_observation(run: &AgentRun) -> Option<(CommitId, usize)> {
    let mut pending = None;
    for event in &run.events {
        match event {
            TaskEvent::Reobserved {
                revision,
                action_index,
                ..
            } => pending = Some((*revision, *action_index)),
            TaskEvent::Planned { .. }
            | TaskEvent::PlanningCompleted { .. }
            | TaskEvent::ActionRejected { .. }
            | TaskEvent::Paused { .. }
            | TaskEvent::Failed { .. }
            | TaskEvent::Cancelled { .. } => pending = None,
            _ => {}
        }
    }
    pending
}

impl TaskWorkspace {
    pub fn new(document: CadDocument) -> Self {
        Self {
            store: DocumentStore::new(document),
            project_id: ProjectId::new(),
            remote_access_policy: RemoteAccessPolicy::default(),
            tasks: BTreeMap::new(),
            next_task_id: 1,
            next_change_set_id: 1,
            next_agent_run_id: 1,
            legacy_missing_remote_policy: false,
        }
    }

    pub fn document(&self) -> &CadDocument {
        self.store.document()
    }

    pub fn history(&self) -> &History {
        self.store.history()
    }

    pub fn tasks(&self) -> &BTreeMap<TaskId, DesignTask> {
        &self.tasks
    }

    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    pub fn remote_access_grants(&self) -> &BTreeMap<RemoteGrantId, RemoteAccessGrant> {
        self.remote_access_policy.grants()
    }

    pub fn remote_policy_events(&self) -> &[RemotePolicyEvent] {
        self.remote_access_policy.events()
    }

    pub fn task(&self, task_id: TaskId) -> Option<&DesignTask> {
        self.tasks.get(&task_id)
    }

    pub fn revision(&self) -> CommitId {
        self.store.revision()
    }

    pub fn snapshot(&self) -> DocumentSnapshot {
        self.store.snapshot()
    }

    pub fn kernel(&mut self) -> crate::KernelFacade<'_> {
        crate::KernelFacade::new(self)
    }

    pub(crate) fn create_remote_access_grant(
        &mut self,
        request: RemoteAccessGrantRequest,
    ) -> Result<RemoteGrantId, WorkspaceError> {
        self.remote_access_policy
            .create_grant(self.project_id, request)
            .map_err(WorkspaceError::RemotePolicy)
    }

    pub(crate) fn revoke_remote_access_grant(
        &mut self,
        grant_id: RemoteGrantId,
        revoked_at_unix_seconds: u64,
    ) -> Result<(), WorkspaceError> {
        self.remote_access_policy
            .revoke_grant(grant_id, revoked_at_unix_seconds)
            .map_err(WorkspaceError::RemotePolicy)
    }

    pub(crate) fn create_task(
        &mut self,
        title: impl Into<String>,
        goal: impl Into<String>,
        authority: TaskAuthority,
    ) -> TaskId {
        let id = self.next_task_id;
        self.next_task_id += 1;
        let change_set_id = self.next_change_set_id;
        self.next_change_set_id += 1;
        let run_id = self.next_agent_run_id;
        self.next_agent_run_id += 1;
        let goal = goal.into();
        let authority_for_change_set = authority.clone();
        self.tasks.insert(
            id,
            DesignTask {
                id,
                title: title.into(),
                goal: goal.clone(),
                authority,
                status: TaskStatus::Queued,
                change_sets: vec![PromptChangeSet {
                    id: change_set_id,
                    task_id: id,
                    prompt: goal.clone(),
                    structured_goal: StructuredGoal::from_prompt(goal),
                    authorization: authority_for_change_set,
                    status: ChangeSetStatus::Running,
                    runs: vec![AgentRun {
                        id: run_id,
                        task_id: id,
                        change_set_id,
                        attempt: 1,
                        identity: AgentRunIdentity::pending(),
                        status: AgentRunStatus::Queued,
                        events: Vec::new(),
                        action_commits: Vec::new(),
                        execution: None,
                    }],
                    active_run_id: run_id,
                    diagnostics: Vec::new(),
                    compensation: None,
                    reverted_by: None,
                }],
                active_change_set_id: change_set_id,
                legacy_layout: false,
            },
        );
        id
    }

    pub(crate) fn begin_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        self.begin_task_as(task_id, AgentRunIdentity::local("local-agent"))
    }

    pub(crate) fn begin_task_as(
        &mut self,
        task_id: TaskId,
        identity: AgentRunIdentity,
    ) -> Result<(), WorkspaceError> {
        let entity_count = self.store.document().entities.len();
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Queued {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Queued,
                actual: task.status,
            });
        }
        let change_set = task.active_change_set_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have its active change set"
            ))
        })?;
        if change_set.status != ChangeSetStatus::Running {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} active change set is not running"
            )));
        }
        let run = change_set.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} active change set does not have its active run"
            ))
        })?;
        if run.status != AgentRunStatus::Queued {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} active run is not queued"
            )));
        }
        run.status = AgentRunStatus::Running;
        run.identity = identity;
        run.events.push(TaskEvent::Observed { entity_count });
        task.status = TaskStatus::Running;
        Ok(())
    }

    pub(crate) fn begin_iterative_task_as(
        &mut self,
        task_id: TaskId,
        identity: AgentRunIdentity,
    ) -> Result<(), WorkspaceError> {
        let planning_budget = TaskPlanningBudget::iterative(MAX_ITERATIVE_ACTIONS_PER_RUN)
            .expect("the core iterative action limit is a valid planning budget");
        self.begin_iterative_task_as_with_budget(task_id, identity, planning_budget)
    }

    pub(crate) fn begin_iterative_task_as_with_budget(
        &mut self,
        task_id: TaskId,
        identity: AgentRunIdentity,
        planning_budget: TaskPlanningBudget,
    ) -> Result<(), WorkspaceError> {
        let base_revision = self.revision();
        self.begin_task_as(task_id, identity)?;
        self.tasks
            .get_mut(&task_id)
            .and_then(DesignTask::active_run_mut)
            .expect("begin_task_as established the active run")
            .execution = Some(TaskExecution::iterative(base_revision, planning_budget));
        Ok(())
    }

    pub(crate) fn resume_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Paused {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Paused,
                actual: task.status,
            });
        }
        self.validate_task_next_action_preconditions(task_id)?;
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let run = task.active_run().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "paused task {task_id} does not have an active run"
            ))
        })?;
        let execution = run.execution.as_ref().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "paused task {task_id} does not have a persisted execution plan"
            ))
        })?;
        let completed_actions = execution.next_action_index;
        let remaining_actions = execution.remaining_actions();
        let run = task.active_run_mut().expect("active run checked above");
        run.status = AgentRunStatus::Running;
        run.events.push(TaskEvent::Resumed {
            completed_actions,
            remaining_actions,
        });
        task.status = TaskStatus::Running;
        Ok(())
    }

    pub(crate) fn pause_task(
        &mut self,
        task_id: TaskId,
        reason: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Running {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Running,
                actual: task.status,
            });
        }
        let run = task.active_run().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "running task {task_id} does not have an active run"
            ))
        })?;
        let execution = run.execution.as_ref().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "running task {task_id} does not have an execution plan"
            ))
        })?;
        if execution.is_complete() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} has no remaining actions to pause"
            )));
        }
        let completed_actions = execution.next_action_index;
        let remaining_actions = execution.remaining_actions();
        let run = task.active_run_mut().expect("active run checked above");
        run.status = AgentRunStatus::Paused;
        run.events.push(TaskEvent::Paused {
            completed_actions,
            remaining_actions,
            reason: reason.into(),
        });
        task.status = TaskStatus::Paused;
        Ok(())
    }

    pub(crate) fn set_task_plan(
        &mut self,
        task_id: TaskId,
        base_revision: CommitId,
        actions: Vec<TaskAction>,
    ) -> Result<(), WorkspaceError> {
        let current_revision = self.revision();
        if !self.store.is_ancestor(base_revision, current_revision)? {
            return Err(WorkspaceError::PreparedBaseNotAncestor {
                base: base_revision,
                current: current_revision,
            });
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Running {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Running,
                actual: task.status,
            });
        }
        if task
            .active_run()
            .and_then(|run| run.execution.as_ref())
            .is_some()
        {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} already has an execution plan"
            )));
        }
        let source = task
            .active_change_set()
            .and_then(|change_set| {
                change_set
                    .active_run()
                    .map(|run| ActionSource::for_run(task_id, change_set.id, run.id))
            })
            .ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} does not have an active run"
                ))
            })?;
        let planning_snapshot = self.store.snapshot_at(base_revision)?;
        let preparation_result = actions
            .first()
            .map(|action| {
                PreparedAction::prepare_for_run(
                    &planning_snapshot,
                    source.task_id,
                    source.change_set_id.expect("run-bound source"),
                    source.agent_run_id.expect("run-bound source"),
                    action.transaction.clone(),
                )
                .map(|prepared| prepared.record())
            })
            .transpose();
        let next_action_preparation = match preparation_result {
            Ok(preparation) => preparation,
            Err(error) => {
                self.fail_task(task_id, error.to_string())?;
                return Err(error.into());
            }
        };
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let run = task.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        run.events.push(TaskEvent::Planned {
            action_count: actions.len(),
        });
        run.execution = Some(TaskExecution::new(
            actions,
            base_revision,
            next_action_preparation,
        ));
        Ok(())
    }

    pub(crate) fn record_iterative_observation(
        &mut self,
        task_id: TaskId,
        revision: CommitId,
    ) -> Result<(), WorkspaceError> {
        if revision != self.revision() {
            return Err(WorkspaceError::StaleRevision {
                expected: revision,
                actual: self.revision(),
            });
        }
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Running {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Running,
                actual: task.status,
            });
        }
        let entity_count = self.store.document().entities.len();
        let run = task.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        let execution = run.execution.as_ref().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an iterative execution"
            ))
        })?;
        if !execution.is_awaiting_planner() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} is not waiting for an iterative planning decision"
            )));
        }
        if pending_iterative_observation(run).is_some() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} already has an unconsumed iterative observation"
            )));
        }
        let decision_count = run
            .events
            .iter()
            .filter(|event| matches!(event, TaskEvent::Reobserved { .. }))
            .count();
        if decision_count >= execution.planning_budget.max_decisions() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} exhausted its persisted planning-decision budget"
            )));
        }
        let action_index = execution.next_action_index;
        run.events.push(TaskEvent::Reobserved {
            revision,
            action_index,
            entity_count,
        });
        Ok(())
    }

    pub(crate) fn stage_iterative_action(
        &mut self,
        task_id: TaskId,
        observed_revision: CommitId,
        action: TaskAction,
    ) -> Result<(), WorkspaceError> {
        let current_revision = self.revision();
        if !self
            .store
            .is_ancestor(observed_revision, current_revision)?
        {
            return Err(WorkspaceError::PreparedBaseNotAncestor {
                base: observed_revision,
                current: current_revision,
            });
        }
        let (source, action_count) = {
            let task = self
                .tasks
                .get(&task_id)
                .ok_or(WorkspaceError::TaskMissing(task_id))?;
            if task.status != TaskStatus::Running {
                return Err(WorkspaceError::InvalidTaskState {
                    task_id,
                    expected: TaskStatus::Running,
                    actual: task.status,
                });
            }
            let execution = task.execution().ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} does not have an iterative execution"
                ))
            })?;
            if !execution.is_awaiting_planner() {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} is not waiting for an iterative planning decision"
                )));
            }
            if execution.actions.len() >= execution.planning_budget.max_actions() {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} exhausted its persisted iterative action budget"
                )));
            }
            if pending_iterative_observation(
                task.active_run()
                    .expect("execution belongs to the active run"),
            ) != Some((observed_revision, execution.next_action_index))
            {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} action is not bound to its pending iterative observation"
                )));
            }
            (self.active_action_source(task_id)?, execution.actions.len())
        };
        let planning_snapshot = self.store.snapshot_at(observed_revision)?;
        let preparation = PreparedAction::prepare_for_run(
            &planning_snapshot,
            source.task_id,
            source.change_set_id.expect("run-bound source"),
            source.agent_run_id.expect("run-bound source"),
            action.transaction.clone(),
        )?
        .record();
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let run = task.active_run_mut().expect("active run checked above");
        let execution = run
            .execution
            .as_mut()
            .expect("iterative execution checked above");
        execution.actions.push(action);
        execution.next_action_preparation = Some(preparation);
        run.events.push(TaskEvent::Planned { action_count: 1 });
        debug_assert_eq!(execution.actions.len(), action_count + 1);
        Ok(())
    }

    pub(crate) fn reject_iterative_action(
        &mut self,
        task_id: TaskId,
        observed_revision: CommitId,
        action: &TaskAction,
        kind: ActionFailureKind,
        message: impl Into<String>,
    ) -> Result<bool, WorkspaceError> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Running {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Running,
                actual: task.status,
            });
        }
        let run = task.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        let latest_observation = latest_iterative_observation(run);
        let pending_observation = pending_iterative_observation(run);
        let execution = run.execution.as_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an iterative execution"
            ))
        })?;
        let TaskExecutionStrategy::Iterative {
            planner_complete,
            last_failure,
        } = &mut execution.strategy
        else {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not use iterative planning"
            )));
        };
        if *planner_complete {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} iterative planner is already complete"
            )));
        }
        if latest_observation != Some((observed_revision, execution.next_action_index)) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} rejected action is not bound to its latest iterative observation"
            )));
        }
        if let Some(pending) = execution.actions.get(execution.next_action_index) {
            if pending != action {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} rejected action does not match its pending action"
                )));
            }
            execution.actions.pop();
            execution.next_action_preparation = None;
        } else if execution.next_action_index != execution.actions.len() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} has an invalid iterative action checkpoint"
            )));
        } else if pending_observation != Some((observed_revision, execution.next_action_index)) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} rejected unstaged action without a pending observation"
            )));
        }
        let previous_attempt = last_failure
            .as_ref()
            .map_or(0, |feedback| feedback.repair_attempt);
        let will_retry = previous_attempt < MAX_AUTOMATIC_REPAIR_ATTEMPTS;
        let repair_attempt = if will_retry {
            previous_attempt + 1
        } else {
            previous_attempt
        };
        let feedback = ActionFailureFeedback {
            action_index: execution.next_action_index,
            observed_revision,
            repair_attempt,
            kind,
            intent: action.intent.clone(),
            tool_name: action.tool_name.clone(),
            message: message.into(),
        };
        *last_failure = Some(feedback.clone());
        run.events.push(TaskEvent::ActionRejected {
            feedback,
            will_retry,
        });
        Ok(will_retry)
    }

    pub(crate) fn finish_iterative_plan(
        &mut self,
        task_id: TaskId,
        observed_revision: CommitId,
        summary: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        if observed_revision != self.revision() {
            return Err(WorkspaceError::StaleRevision {
                expected: observed_revision,
                actual: self.revision(),
            });
        }
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Running {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Running,
                actual: task.status,
            });
        }
        let run = task.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        let pending_observation = pending_iterative_observation(run);
        let execution = run.execution.as_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an iterative execution"
            ))
        })?;
        if !execution.is_awaiting_planner() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} is not waiting for an iterative planning decision"
            )));
        }
        if pending_observation != Some((observed_revision, execution.next_action_index)) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} completion is not bound to its pending iterative observation"
            )));
        }
        let TaskExecutionStrategy::Iterative {
            planner_complete,
            last_failure,
        } = &mut execution.strategy
        else {
            unreachable!("is_awaiting_planner only accepts iterative executions");
        };
        *planner_complete = true;
        *last_failure = None;
        run.events.push(TaskEvent::PlanningCompleted {
            revision: observed_revision,
            action_count: execution.next_action_index,
            summary: summary.into(),
        });
        Ok(())
    }

    pub fn next_task_action(&self, task_id: TaskId) -> Result<Option<TaskAction>, WorkspaceError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Running {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Running,
                actual: task.status,
            });
        }
        let Some(execution) = task.execution() else {
            return Ok(None);
        };
        Ok(execution.actions.get(execution.next_action_index).cloned())
    }

    pub(crate) fn record_event(
        &mut self,
        task_id: TaskId,
        event: TaskEvent,
    ) -> Result<(), WorkspaceError> {
        self.tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?
            .active_run_mut()
            .ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} does not have an active run"
                ))
            })?
            .events
            .push(event);
        Ok(())
    }

    /// Applies exactly one persisted action, advancing its checkpoint only
    /// after the history transaction has been committed successfully.
    pub(crate) fn apply_next_task_action(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<CommitId>, WorkspaceError> {
        let Some(action) = self.next_task_action(task_id)? else {
            return Ok(None);
        };
        self.validate_task_next_action_preconditions(task_id)?;
        let action_preparation = self
            .tasks
            .get(&task_id)
            .and_then(DesignTask::active_run)
            .and_then(|run| run.execution.as_ref())
            .and_then(TaskExecution::next_action_preparation)
            .cloned()
            .expect("next action preparation was validated above");
        let source = self.active_action_source(task_id)?;
        self.record_event(
            task_id,
            TaskEvent::ToolCall {
                name: action.tool_name.clone(),
                detail: action.detail.clone(),
            },
        )?;
        let commit_id = self.commit_task_transaction(
            task_id,
            action.intent,
            action.transaction,
            action.validation,
            Some(action_preparation),
        )?;
        let next_transaction = {
            let task = self
                .tasks
                .get_mut(&task_id)
                .ok_or(WorkspaceError::TaskMissing(task_id))?;
            let execution = task
                .active_run_mut()
                .and_then(|run| run.execution.as_mut())
                .ok_or_else(|| {
                    WorkspaceError::InvalidWorkspace(format!(
                        "task {task_id} lost its execution plan while applying an action"
                    ))
                })?;
            execution.next_action_index += 1;
            execution.expected_revision = Some(commit_id);
            execution
                .actions
                .get(execution.next_action_index)
                .map(|action| action.transaction.clone())
        };
        let preparation_result = next_transaction
            .map(|transaction| {
                PreparedAction::prepare_for_run(
                    &self.snapshot(),
                    source.task_id,
                    source.change_set_id.expect("run-bound source"),
                    source.agent_run_id.expect("run-bound source"),
                    transaction,
                )
                .map(|prepared| prepared.record())
            })
            .transpose();
        let next_action_preparation = match preparation_result {
            Ok(preparation) => preparation,
            Err(error) => {
                self.tasks
                    .get_mut(&task_id)
                    .and_then(DesignTask::active_run_mut)
                    .and_then(|run| run.execution.as_mut())
                    .expect("task execution checked above")
                    .next_action_preparation = None;
                self.fail_task(task_id, error.to_string())?;
                return Err(error.into());
            }
        };
        let execution = self
            .tasks
            .get_mut(&task_id)
            .and_then(DesignTask::active_run_mut)
            .and_then(|run| run.execution.as_mut())
            .expect("task execution checked above");
        execution.next_action_preparation = next_action_preparation;
        if let TaskExecutionStrategy::Iterative { last_failure, .. } = &mut execution.strategy {
            *last_failure = None;
        }
        Ok(Some(commit_id))
    }

    fn commit_task_transaction(
        &mut self,
        task_id: TaskId,
        intent: impl Into<String>,
        transaction: CommandTransaction,
        validation: ValidationReport,
        prepared_action: Option<crate::PreparedActionRecord>,
    ) -> Result<CommitId, WorkspaceError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let change_set = task.active_change_set().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an active change set"
            ))
        })?;
        let run = change_set.active_run().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        if !change_set
            .authorization
            .permits(&transaction, self.store.document())
        {
            return Err(WorkspaceError::Unauthorized(task_id));
        }
        let source = ActionSource::for_run(task_id, change_set.id, run.id);
        let intent = intent.into();
        let commit_id = self.store.commit(
            Some(source),
            intent.clone(),
            transaction,
            validation,
            prepared_action,
        )?;
        let evidence = self
            .store
            .history()
            .commits
            .get(&commit_id)
            .and_then(|commit| commit.validation_evidence())
            .expect("new commits always contain local validation evidence");
        let validator_id = evidence.validator_id().to_owned();
        let validator_version = evidence.validator_version();
        let candidate_state_hash = evidence.candidate_state_hash_hex();
        let evidence_summary = evidence.summary();
        let task = self.tasks.get_mut(&task_id).expect("task checked above");
        let run = task.active_run_mut().expect("active run checked above");
        let action_index = run
            .execution
            .as_ref()
            .map_or(run.action_commits.len(), |execution| {
                execution.next_action_index
            });
        run.action_commits.push(ChangeSetActionCommit {
            action_index,
            commit_id,
        });
        run.events.push(TaskEvent::Committed {
            commit_id,
            summary: intent,
        });
        run.events.push(TaskEvent::Validated {
            validator_id,
            validator_version,
            candidate_state_hash,
            summary: evidence_summary,
        });
        Ok(commit_id)
    }

    fn validate_task_next_action_preconditions(
        &self,
        task_id: TaskId,
    ) -> Result<(), WorkspaceError> {
        let execution = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?
            .execution()
            .ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} does not have a persisted execution plan"
                ))
            })?;
        if execution.is_complete() {
            return Ok(());
        }
        let checkpoint_revision = execution.expected_revision().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have a revision-bound execution plan"
            ))
        })?;
        if !self
            .store
            .is_ancestor(checkpoint_revision, self.revision())?
        {
            return Err(WorkspaceError::PreparedBaseNotAncestor {
                base: checkpoint_revision,
                current: self.revision(),
            });
        }
        if execution.is_awaiting_planner() {
            return Ok(());
        }
        let preparation = execution.next_action_preparation().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} next action is missing its preparation record"
            ))
        })?;
        let action = execution
            .actions
            .get(execution.next_action_index)
            .expect("incomplete execution must have a next action");
        let source = self.active_action_source(task_id)?;
        let prepared =
            PreparedAction::from_record(source, action.transaction.clone(), preparation.clone());
        self.validate_prepared_origin(&prepared)?;
        self.validate_prepared_preconditions(&prepared)
    }

    fn active_action_source(&self, task_id: TaskId) -> Result<ActionSource, WorkspaceError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let change_set = task.active_change_set().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an active change set"
            ))
        })?;
        let run = change_set.active_run().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        Ok(ActionSource::for_run(task_id, change_set.id, run.id))
    }

    /// Applies a user-initiated edit through the same validated, replayable
    /// history path used by task actions. User edits have no task authority
    /// because the local user is the source of the command.
    pub(crate) fn apply_user_transaction(
        &mut self,
        expected_revision: CommitId,
        intent: impl Into<String>,
        transaction: CommandTransaction,
        validation: ValidationReport,
    ) -> Result<CommitId, WorkspaceError> {
        self.ensure_revision(expected_revision)?;
        let prepared = PreparedAction::prepare(&self.snapshot(), None, transaction)?;
        self.commit_prepared_user_action(intent, prepared, validation)
    }

    pub(crate) fn prepare_action(
        &self,
        transaction: CommandTransaction,
    ) -> Result<PreparedAction, WorkspaceError> {
        Ok(PreparedAction::prepare(
            &self.snapshot(),
            None,
            transaction,
        )?)
    }

    pub(crate) fn commit_prepared_user_action(
        &mut self,
        intent: impl Into<String>,
        prepared: PreparedAction,
        validation: ValidationReport,
    ) -> Result<CommitId, WorkspaceError> {
        if prepared.task_id().is_some() {
            return Err(WorkspaceError::PreparedSourceMismatch);
        }
        self.validate_prepared_origin(&prepared)?;
        let current_revision = self.revision();
        if let Some(existing_commit) = self
            .store
            .commit_for_idempotency_key(prepared.idempotency_key())
        {
            if self.store.is_ancestor(existing_commit, current_revision)? {
                return Ok(existing_commit);
            }
            return Err(WorkspaceError::IdempotencyConflict {
                existing_commit,
                current: current_revision,
            });
        }
        self.validate_prepared_preconditions(&prepared)?;
        let idempotency_key = prepared.idempotency_key();
        let action_preparation = prepared.record();
        let commit_id = self.store.commit(
            None,
            intent.into(),
            prepared.into_transaction(),
            validation,
            Some(action_preparation),
        )?;
        debug_assert_eq!(
            self.store.history().commits[&commit_id].idempotency_key(),
            Some(idempotency_key)
        );
        Ok(commit_id)
    }

    fn validate_prepared_origin(&self, prepared: &PreparedAction) -> Result<(), WorkspaceError> {
        let current_revision = self.revision();
        if !self
            .store
            .is_ancestor(prepared.base_revision(), current_revision)?
        {
            return Err(WorkspaceError::PreparedBaseNotAncestor {
                base: prepared.base_revision(),
                current: current_revision,
            });
        }
        let actual_input_hash = self.store.state_hash_at(prepared.base_revision())?;
        if actual_input_hash != prepared.input_state_hash() {
            return Err(WorkspaceError::PreparedInputMismatch {
                base: prepared.base_revision(),
            });
        }
        Ok(())
    }

    fn validate_prepared_preconditions(
        &self,
        prepared: &PreparedAction,
    ) -> Result<(), WorkspaceError> {
        if let Some((expected, actual)) = self
            .store
            .conflicting_precondition(prepared.preconditions())?
        {
            return Err(WorkspaceError::ObjectPreconditionFailed { expected, actual });
        }
        Ok(())
    }

    pub fn can_undo(&self) -> bool {
        self.store.can_undo() && self.undo_blocking_task().is_none()
    }

    pub fn can_redo(&self) -> bool {
        self.store.can_redo()
    }

    /// Restores the parent of the active branch head through deterministic
    /// history replay. Task audit records remain immutable.
    pub(crate) fn undo(&mut self) -> Result<CommitId, WorkspaceError> {
        if let Some(task_id) = self.undo_blocking_task() {
            return Err(WorkspaceError::HistoryNavigationBlocked(task_id));
        }
        Ok(self.store.undo()?)
    }

    /// Restores the next branch-local commit retained by the most recent undo.
    pub(crate) fn redo(&mut self) -> Result<CommitId, WorkspaceError> {
        Ok(self.store.redo()?)
    }

    fn undo_blocking_task(&self) -> Option<TaskId> {
        let history = self.store.history();
        let commit = history.commits.get(&history.head())?;
        let task_id = commit.task_id?;
        self.tasks
            .get(&task_id)
            .filter(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Paused))
            .map(|task| task.id)
    }

    pub(crate) fn complete_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status != TaskStatus::Running {
            return Err(WorkspaceError::InvalidTaskState {
                task_id,
                expected: TaskStatus::Running,
                actual: task.status,
            });
        }
        if task
            .execution()
            .is_some_and(|execution| !execution.is_complete())
        {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} still has actions to execute"
            )));
        }
        let change_set = task.active_change_set_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an active change set"
            ))
        })?;
        let run = change_set.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        run.status = AgentRunStatus::Completed;
        change_set.status = ChangeSetStatus::Completed;
        task.status = TaskStatus::Completed;
        Ok(())
    }

    pub(crate) fn fail_task(
        &mut self,
        task_id: TaskId,
        message: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if task.status == TaskStatus::Failed {
            return Ok(());
        }
        let message = message.into();
        let change_set = task.active_change_set_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an active change set"
            ))
        })?;
        let run = change_set.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        let run_id = run.id;
        let action_index = run
            .execution
            .as_ref()
            .map(|execution| execution.next_action_index);
        run.status = AgentRunStatus::Failed;
        run.events.push(TaskEvent::Failed {
            message: message.clone(),
        });
        change_set.status = ChangeSetStatus::PartiallyFailed;
        change_set.diagnostics.push(ChangeSetDiagnostic {
            run_id,
            action_index,
            message,
        });
        task.status = TaskStatus::Failed;
        Ok(())
    }

    pub(crate) fn cancel_task(
        &mut self,
        task_id: TaskId,
        reason: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if !matches!(
            task.status,
            TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused
        ) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} is not active and cannot be cancelled"
            )));
        }
        let reason = reason.into();
        let change_set = task.active_change_set_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an active change set"
            ))
        })?;
        let run = change_set.active_run_mut().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!("task {task_id} does not have an active run"))
        })?;
        let run_id = run.id;
        let action_index = run
            .execution
            .as_ref()
            .map(|execution| execution.next_action_index);
        run.status = AgentRunStatus::Cancelled;
        run.events.push(TaskEvent::Cancelled {
            reason: reason.clone(),
        });
        change_set.status = ChangeSetStatus::Cancelled;
        change_set.diagnostics.push(ChangeSetDiagnostic {
            run_id,
            action_index,
            message: reason,
        });
        task.status = TaskStatus::Cancelled;
        Ok(())
    }

    pub(crate) fn add_prompt(
        &mut self,
        task_id: TaskId,
        prompt: impl Into<String>,
        authorization: TaskAuthority,
    ) -> Result<PromptChangeSetId, WorkspaceError> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            return Err(WorkspaceError::InvalidWorkspace(
                "prompt must not be empty".into(),
            ));
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if !matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} already has an active prompt"
            )));
        }
        let change_set_id = self.allocate_change_set_id()?;
        let run_id = self.allocate_agent_run_id()?;
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        task.authority = authorization.clone();
        task.change_sets.push(PromptChangeSet {
            id: change_set_id,
            task_id,
            prompt: prompt.clone(),
            structured_goal: StructuredGoal::from_prompt(prompt),
            authorization,
            status: ChangeSetStatus::Running,
            runs: vec![AgentRun {
                id: run_id,
                task_id,
                change_set_id,
                attempt: 1,
                identity: AgentRunIdentity::pending(),
                status: AgentRunStatus::Queued,
                events: Vec::new(),
                action_commits: Vec::new(),
                execution: None,
            }],
            active_run_id: run_id,
            diagnostics: Vec::new(),
            compensation: None,
            reverted_by: None,
        });
        task.active_change_set_id = change_set_id;
        task.status = TaskStatus::Queued;
        Ok(change_set_id)
    }

    pub(crate) fn retry_active_change_set(
        &mut self,
        task_id: TaskId,
    ) -> Result<AgentRunId, WorkspaceError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let change_set = task.active_change_set().ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} does not have an active change set"
            ))
        })?;
        if !matches!(
            change_set.status,
            ChangeSetStatus::PartiallyFailed | ChangeSetStatus::Cancelled
        ) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} active change set is not retryable"
            )));
        }
        if change_set
            .active_run()
            .is_some_and(|run| !run.status.is_terminal())
        {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} active run is not terminal"
            )));
        }
        let attempt = u32::try_from(change_set.runs.len())
            .ok()
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} exhausted its agent run attempts"
                ))
            })?;
        let change_set_id = change_set.id;
        let run_id = self.allocate_agent_run_id()?;
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let change_set = task
            .active_change_set_mut()
            .expect("active change set checked above");
        change_set.runs.push(AgentRun {
            id: run_id,
            task_id,
            change_set_id,
            attempt,
            identity: AgentRunIdentity::pending(),
            status: AgentRunStatus::Queued,
            events: Vec::new(),
            action_commits: Vec::new(),
            execution: None,
        });
        change_set.active_run_id = run_id;
        change_set.status = ChangeSetStatus::Running;
        task.status = TaskStatus::Queued;
        Ok(run_id)
    }

    pub(crate) fn revert_change_set(
        &mut self,
        task_id: TaskId,
        target_change_set_id: PromptChangeSetId,
    ) -> Result<ChangeSetRevertReport, WorkspaceError> {
        let mut candidate = self.clone();
        let report = candidate.revert_change_set_in_place(task_id, target_change_set_id)?;
        candidate.validate_integrity()?;
        *self = candidate;
        Ok(report)
    }

    fn revert_change_set_in_place(
        &mut self,
        task_id: TaskId,
        target_change_set_id: PromptChangeSetId,
    ) -> Result<ChangeSetRevertReport, WorkspaceError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if !matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} must be terminal before a change set can be reverted"
            )));
        }
        let target = task
            .change_sets
            .iter()
            .find(|change_set| change_set.id == target_change_set_id)
            .ok_or(WorkspaceError::ChangeSetMissing(target_change_set_id))?;
        if !matches!(
            target.status,
            ChangeSetStatus::Completed
                | ChangeSetStatus::PartiallyFailed
                | ChangeSetStatus::Cancelled
        ) || target.compensation.is_some()
        {
            return Err(WorkspaceError::ChangeSetNotRevertible(target_change_set_id));
        }
        if target.reverted_by.is_some() {
            return Err(WorkspaceError::ChangeSetAlreadyReverted(
                target_change_set_id,
            ));
        }
        let authorization = target.authorization.clone();
        let mut target_commits = target.output_commits().collect::<Vec<_>>();
        target_commits.sort_unstable();
        target_commits.dedup();
        if target_commits.is_empty() {
            return Err(WorkspaceError::ChangeSetNotRevertible(target_change_set_id));
        }

        let requested_at_revision = self.revision();
        for commit_id in &target_commits {
            if !self.store.is_ancestor(*commit_id, requested_at_revision)? {
                return Err(WorkspaceError::ChangeSetNotOnActiveBranch {
                    change_set_id: target_change_set_id,
                    commit_id: *commit_id,
                    current: requested_at_revision,
                });
            }
        }
        let (baseline, last_target_revision) =
            compensation_target_state(self.store.history(), &target_commits)?;
        let requested_snapshot = self.store.snapshot();
        let mut eligible = BTreeSet::new();
        let mut conflicts = Vec::new();
        for object in baseline.keys().copied() {
            let target_revision = last_target_revision[&object];
            let actual = requested_snapshot.object_precondition(object);
            if actual.last_modified_revision == Some(target_revision) {
                eligible.insert(object);
            } else {
                conflicts.push(RevertConflict {
                    object,
                    target_revision,
                    conflicting_revision: actual.last_modified_revision,
                    reason: RevertConflictReason::ModifiedAfterTarget,
                    detail: format!(
                        "object was modified at revision {:?} after target revision {target_revision}",
                        actual.last_modified_revision
                    ),
                });
            }
        }

        let ordered = compensation_object_order(&eligible, &baseline);
        let full_transaction =
            build_compensation_transaction(self.store.document(), &baseline, &eligible);
        let (reverted_objects, transaction) = match full_transaction.preview(self.store.document())
        {
            Ok(_) => (eligible, full_transaction),
            Err(_) => {
                let mut accepted = BTreeSet::new();
                let mut pending = ordered;
                let mut failures = BTreeMap::new();
                loop {
                    let mut progressed = false;
                    let mut next_pending = Vec::new();
                    for object in pending {
                        let mut trial = accepted.clone();
                        trial.insert(object);
                        let trial_transaction = build_compensation_transaction(
                            self.store.document(),
                            &baseline,
                            &trial,
                        );
                        match trial_transaction.preview(self.store.document()) {
                            Ok(_) => {
                                accepted = trial;
                                failures.remove(&object);
                                progressed = true;
                            }
                            Err(error) => {
                                failures.insert(object, error.to_string());
                                next_pending.push(object);
                            }
                        }
                    }
                    if !progressed || next_pending.is_empty() {
                        pending = next_pending;
                        break;
                    }
                    pending = next_pending;
                }
                for object in pending {
                    conflicts.push(RevertConflict {
                        object,
                        target_revision: last_target_revision[&object],
                        conflicting_revision: None,
                        reason: RevertConflictReason::DependencyValidationFailed,
                        detail: failures.remove(&object).unwrap_or_else(|| {
                            "compensation would violate a document dependency".into()
                        }),
                    });
                }
                let transaction =
                    build_compensation_transaction(self.store.document(), &baseline, &accepted);
                transaction.preview(self.store.document())?;
                (accepted, transaction)
            }
        };
        conflicts.sort_by_key(|conflict| conflict.object);
        let reverted_objects = reverted_objects.into_iter().collect::<Vec<_>>();

        if !transaction.commands.is_empty()
            && !authorization.permits(&transaction, self.store.document())
        {
            return Err(WorkspaceError::Unauthorized(task_id));
        }

        let compensation_change_set_id = self.add_prompt(
            task_id,
            format!("Compensate change set {target_change_set_id}"),
            authorization,
        )?;
        self.begin_task_as(
            task_id,
            AgentRunIdentity::local("cadx.compensating-revert@1"),
        )?;
        let actions = (!transaction.commands.is_empty())
            .then(|| TaskAction {
                intent: format!("Compensate change set {target_change_set_id}"),
                tool_name: "cadx.compensating_revert".into(),
                detail: format!(
                    "Restore {} unchanged object(s); preserve {} conflict(s)",
                    reverted_objects.len(),
                    conflicts.len()
                ),
                transaction,
                validation: ValidationReport::default(),
            })
            .into_iter()
            .collect::<Vec<_>>();
        self.set_task_plan(task_id, requested_at_revision, actions)?;
        let commit_id = self.apply_next_task_action(task_id)?;
        self.complete_task(task_id)?;

        let compensation = ChangeSetCompensation {
            target_change_set_id,
            requested_at_revision,
            reverted_objects: reverted_objects.clone(),
            conflicts: conflicts.clone(),
            commit_id,
        };
        let task = self
            .tasks
            .get_mut(&task_id)
            .expect("compensation task checked above");
        let target = task
            .change_sets
            .iter_mut()
            .find(|change_set| change_set.id == target_change_set_id)
            .expect("target change set checked above");
        target.reverted_by = Some(compensation_change_set_id);
        target.status = if conflicts.is_empty() {
            ChangeSetStatus::Reverted
        } else {
            ChangeSetStatus::RevertedWithConflicts
        };
        task.active_change_set_mut()
            .expect("compensation change set is active")
            .compensation = Some(compensation);

        Ok(ChangeSetRevertReport {
            target_change_set_id,
            compensation_change_set_id,
            commit_id,
            reverted_objects,
            conflicts,
        })
    }

    pub(crate) fn fork_at(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), WorkspaceError> {
        self.store.create_branch(name, commit_id)?;
        Ok(())
    }

    pub(crate) fn checkout_branch(&mut self, name: &str) -> Result<(), WorkspaceError> {
        self.store.checkout_branch(name)?;
        Ok(())
    }

    pub(crate) fn checkout_as_branch(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), WorkspaceError> {
        self.store.checkout_as_branch(name, commit_id)?;
        Ok(())
    }

    /// Runs schema migration on the active document and every historical
    /// snapshot, then verifies that the active document is exactly the active
    /// branch head.
    pub(crate) fn migrate_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.migrate_to_current_with_legacy_fields(false, false, false, false)
    }

    /// Migrates an archive created before local validation evidence became a
    /// required history field. Current-format archives must never use this path.
    pub(crate) fn migrate_legacy_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.migrate_to_current_with_legacy_fields(true, true, true, true)
    }

    /// Migrates archives that already contain local validation evidence but
    /// predate revision-bound persisted task executions.
    pub(crate) fn migrate_legacy_executions_to_current(&mut self) -> Result<(), WorkspaceError> {
        self.migrate_to_current_with_legacy_fields(false, true, true, true)
    }

    /// Migrates revision-bound task plans that predate persisted object
    /// preconditions for their next action.
    pub(crate) fn migrate_legacy_object_preconditions_to_current(
        &mut self,
    ) -> Result<(), WorkspaceError> {
        self.migrate_to_current_with_legacy_fields(false, false, true, true)
    }

    /// Migrates format-v6 tasks into the PromptChangeSet/AgentRun hierarchy.
    pub(crate) fn migrate_legacy_task_hierarchy_to_current(
        &mut self,
    ) -> Result<(), WorkspaceError> {
        self.migrate_to_current_with_legacy_fields(false, false, false, true)
    }

    /// Marks pre-v9 executions as the legacy batch strategy. The archive
    /// loader may call this only after verifying an older manifest version.
    pub(crate) fn migrate_legacy_execution_strategies(&mut self) {
        for task in self.tasks.values_mut() {
            for change_set in &mut task.change_sets {
                for run in &mut change_set.runs {
                    if let Some(execution) = &mut run.execution
                        && execution.legacy_missing_strategy
                    {
                        execution.strategy = TaskExecutionStrategy::Batch;
                        execution.legacy_missing_strategy = false;
                    }
                }
            }
        }
    }

    /// Adds a project identity and empty remote-policy ledger to pre-v10
    /// archives. The loader may call this only after verifying an older
    /// manifest version.
    pub(crate) fn migrate_legacy_remote_policy(&mut self) {
        self.legacy_missing_remote_policy = false;
    }

    /// Adds explicit planning budgets to pre-v11 executions. The loader may
    /// call this only after verifying an older manifest version.
    pub(crate) fn migrate_legacy_planning_budgets(&mut self) {
        for task in self.tasks.values_mut() {
            for change_set in &mut task.change_sets {
                for run in &mut change_set.runs {
                    if let Some(execution) = &mut run.execution
                        && execution.legacy_missing_planning_budget
                    {
                        execution.planning_budget = match execution.strategy {
                            TaskExecutionStrategy::Batch => {
                                TaskPlanningBudget::batch(execution.actions.len())
                            }
                            TaskExecutionStrategy::Iterative { .. } => {
                                TaskPlanningBudget::iterative(MAX_ITERATIVE_ACTIONS_PER_RUN)
                                    .expect("the core iterative limit is valid")
                            }
                        };
                        execution.legacy_missing_planning_budget = false;
                    }
                }
            }
        }
    }

    fn migrate_to_current_with_legacy_fields(
        &mut self,
        regenerate_validation_evidence: bool,
        infer_execution_revisions: bool,
        infer_object_preconditions: bool,
        migrate_task_hierarchy: bool,
    ) -> Result<(), WorkspaceError> {
        self.store.migrate_document_to_current()?;
        self.store
            .migrate_history_to_current(regenerate_validation_evidence)?;
        if infer_execution_revisions {
            self.infer_legacy_execution_revisions()?;
        }
        if infer_object_preconditions {
            self.store.infer_legacy_idempotency_keys()?;
            self.infer_legacy_object_preconditions()?;
        }
        if migrate_task_hierarchy {
            self.migrate_legacy_task_hierarchy()?;
        }
        self.recover_interrupted_tasks();
        self.normalize_next_ids()?;
        self.validate_integrity()
    }

    pub fn validate_integrity(&self) -> Result<(), WorkspaceError> {
        if self.legacy_missing_remote_policy {
            return Err(WorkspaceError::InvalidWorkspace(
                "workspace is missing its project identity or remote policy ledger".into(),
            ));
        }
        self.remote_access_policy
            .validate(self.project_id)
            .map_err(WorkspaceError::RemotePolicy)?;
        self.store.document().validate()?;
        self.store.history().validate_integrity()?;
        let active_document = self
            .store
            .history()
            .restore(self.store.history().active_head()?)?;
        if active_document != *self.store.document() {
            return Err(WorkspaceError::InvalidWorkspace(
                "active document does not match the active branch head".into(),
            ));
        }
        let mut change_set_ids = BTreeSet::new();
        let mut run_ids = BTreeSet::new();
        let mut claimed_commits = BTreeMap::new();
        for (id, task) in &self.tasks {
            if *id != task.id {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task map key {id} does not match task id {}",
                    task.id
                )));
            }
            if task.id == TaskId::MAX {
                return Err(WorkspaceError::InvalidWorkspace(
                    "task id space is exhausted".into(),
                ));
            }
            if task.legacy_layout {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {id} still uses the legacy execution layout"
                )));
            }
            if task.goal.trim().is_empty() || task.change_sets.is_empty() {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {id} must have a goal and at least one change set"
                )));
            }
            let active_change_set = task.active_change_set().ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {id} does not contain active change set {}",
                    task.active_change_set_id
                ))
            })?;
            for (change_set_index, change_set) in task.change_sets.iter().enumerate() {
                if change_set.task_id != *id
                    || change_set.id == PromptChangeSetId::MAX
                    || !change_set_ids.insert(change_set.id)
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "task {id} has a duplicate or incorrectly bound change set {}",
                        change_set.id
                    )));
                }
                if change_set.prompt.trim().is_empty()
                    || change_set.structured_goal.objective.trim().is_empty()
                    || change_set.runs.is_empty()
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "change set {} is missing its prompt, structured goal, or runs",
                        change_set.id
                    )));
                }
                if change_set_index + 1 != task.change_sets.len()
                    && !change_set.status.is_terminal()
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "historical change set {} is not terminal",
                        change_set.id
                    )));
                }
                let active_run = change_set.active_run().ok_or_else(|| {
                    WorkspaceError::InvalidWorkspace(format!(
                        "change set {} does not contain active run {}",
                        change_set.id, change_set.active_run_id
                    ))
                })?;
                for (run_index, run) in change_set.runs.iter().enumerate() {
                    let expected_attempt = u32::try_from(run_index)
                        .ok()
                        .and_then(|index| index.checked_add(1))
                        .ok_or_else(|| {
                            WorkspaceError::InvalidWorkspace(format!(
                                "change set {} has too many runs",
                                change_set.id
                            ))
                        })?;
                    if run.task_id != *id
                        || run.change_set_id != change_set.id
                        || run.id == AgentRunId::MAX
                        || !run_ids.insert(run.id)
                        || run.attempt != expected_attempt
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "change set {} has a duplicate, incorrectly bound, or misordered run {}",
                            change_set.id, run.id
                        )));
                    }
                    if run.identity.agent.trim().is_empty()
                        || (run.identity.kind == crate::AgentKind::Remote
                            && (run.identity.provider.as_deref().is_none_or(str::is_empty)
                                || run.identity.model.as_deref().is_none_or(str::is_empty)))
                        || (run.identity.kind == crate::AgentKind::Local
                            && (run.identity.provider.is_some() || run.identity.model.is_some()))
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "agent run {} has an invalid identity",
                            run.id
                        )));
                    }
                    if run_index + 1 != change_set.runs.len() && !run.status.is_terminal() {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "historical agent run {} is not terminal",
                            run.id
                        )));
                    }
                    if let Some(execution) = &run.execution {
                        self.validate_task_execution(*id, change_set.id, run, execution)?;
                    } else if !run.action_commits.is_empty()
                        || matches!(run.status, AgentRunStatus::Paused)
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "agent run {} has commits or a paused state without an execution",
                            run.id
                        )));
                    }
                    for event in &run.events {
                        self.validate_remote_audit_event(*id, event)?;
                    }
                    for action_commit in &run.action_commits {
                        if claimed_commits
                            .insert(action_commit.commit_id, (*id, change_set.id, run.id))
                            .is_some()
                        {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "commit {} is claimed by more than one agent run",
                                action_commit.commit_id
                            )));
                        }
                        let commit = self
                            .store
                            .history()
                            .commits
                            .get(&action_commit.commit_id)
                            .ok_or(HistoryError::CommitMissing(action_commit.commit_id))?;
                        if commit.action_source()
                            != Some(ActionSource::for_run(*id, change_set.id, run.id))
                        {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "agent run {} claims commit {} from another source",
                                run.id, action_commit.commit_id
                            )));
                        }
                        let parent = commit.parent.ok_or_else(|| {
                            WorkspaceError::InvalidWorkspace(format!(
                                "agent commit {} is missing its parent",
                                commit.id
                            ))
                        })?;
                        let parent_document = self.store.history().restore(parent)?;
                        if !change_set
                            .authorization
                            .permits(&commit.transaction, &parent_document)
                        {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "change set {} authorization does not permit commit {}",
                                change_set.id, commit.id
                            )));
                        }
                    }
                }
                match change_set.status {
                    ChangeSetStatus::Running if active_run.status.is_terminal() => {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "running change set {} has a terminal active run",
                            change_set.id
                        )));
                    }
                    ChangeSetStatus::Completed
                        if active_run.status != AgentRunStatus::Completed =>
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "completed change set {} has an incomplete active run",
                            change_set.id
                        )));
                    }
                    ChangeSetStatus::PartiallyFailed
                        if active_run.status != AgentRunStatus::Failed
                            || change_set.diagnostics.is_empty() =>
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "partially failed change set {} lacks a failed run or diagnostic",
                            change_set.id
                        )));
                    }
                    ChangeSetStatus::Cancelled
                        if active_run.status != AgentRunStatus::Cancelled
                            || change_set.diagnostics.is_empty() =>
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "cancelled change set {} lacks a cancelled run or diagnostic",
                            change_set.id
                        )));
                    }
                    _ => {}
                }
                for diagnostic in &change_set.diagnostics {
                    if diagnostic.message.trim().is_empty()
                        || !change_set
                            .runs
                            .iter()
                            .any(|run| run.id == diagnostic.run_id)
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "change set {} has an invalid diagnostic",
                            change_set.id
                        )));
                    }
                }
            }
            let active_run = active_change_set.active_run().expect("checked above");
            let status_matches = matches!(
                (task.status, active_change_set.status, active_run.status),
                (
                    TaskStatus::Queued,
                    ChangeSetStatus::Running,
                    AgentRunStatus::Queued
                ) | (
                    TaskStatus::Running,
                    ChangeSetStatus::Running,
                    AgentRunStatus::Running
                ) | (
                    TaskStatus::Paused,
                    ChangeSetStatus::Running,
                    AgentRunStatus::Paused
                ) | (
                    TaskStatus::Completed,
                    ChangeSetStatus::Completed,
                    AgentRunStatus::Completed
                ) | (
                    TaskStatus::Failed,
                    ChangeSetStatus::PartiallyFailed,
                    AgentRunStatus::Failed
                ) | (
                    TaskStatus::Cancelled,
                    ChangeSetStatus::Cancelled,
                    AgentRunStatus::Cancelled
                )
            );
            if !status_matches {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {id}, its active change set, and active run have inconsistent status"
                )));
            }
            if task.status == TaskStatus::Paused
                && active_run
                    .execution
                    .as_ref()
                    .is_none_or(TaskExecution::is_complete)
            {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "paused task {id} has no remaining actions"
                )));
            }
            if task.status == TaskStatus::Completed
                && active_run
                    .execution
                    .as_ref()
                    .is_some_and(|execution| !execution.is_complete())
            {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "completed task {id} still has pending actions"
                )));
            }
        }
        self.validate_compensations()?;
        let mut idempotency_keys = BTreeSet::new();
        for commit in self.store.history().commits.values() {
            if commit.task_id.is_none()
                && (commit.change_set_id.is_some() || commit.agent_run_id.is_some())
            {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "user commit {} has agent ownership fields",
                    commit.id
                )));
            }
            if commit.id != 0 {
                let key = commit.idempotency_key().ok_or_else(|| {
                    WorkspaceError::InvalidWorkspace(format!(
                        "commit {} is missing its idempotency key",
                        commit.id
                    ))
                })?;
                if !idempotency_keys.insert(key) {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "commit {} reuses an existing idempotency key",
                        commit.id
                    )));
                }
            }
            if let Some(task_id) = commit.task_id {
                let source = commit.action_source().expect("task id is present");
                if !source.is_run_bound() {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "task commit {} is not bound to a change set and agent run",
                        commit.id
                    )));
                }
                if !self.tasks.contains_key(&task_id) {
                    return Err(WorkspaceError::TaskMissing(task_id));
                }
                if claimed_commits.get(&commit.id)
                    != Some(&(
                        source.task_id,
                        source.change_set_id.expect("run-bound source"),
                        source.agent_run_id.expect("run-bound source"),
                    ))
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "commit {} is not listed by an agent run in originating task {task_id}",
                        commit.id
                    )));
                }
            }
        }
        let minimum_next = next_id_after("task", self.tasks.keys().copied())?;
        if self.next_task_id < minimum_next {
            return Err(WorkspaceError::InvalidWorkspace(
                "next task id is behind existing tasks".into(),
            ));
        }
        let minimum_next_change_set = next_id_after("change set", change_set_ids.into_iter())?;
        if self.next_change_set_id < minimum_next_change_set {
            return Err(WorkspaceError::InvalidWorkspace(
                "next change set id is behind existing change sets".into(),
            ));
        }
        let minimum_next_run = next_id_after("agent run", run_ids.into_iter())?;
        if self.next_agent_run_id < minimum_next_run {
            return Err(WorkspaceError::InvalidWorkspace(
                "next agent run id is behind existing runs".into(),
            ));
        }
        Ok(())
    }

    fn validate_compensations(&self) -> Result<(), WorkspaceError> {
        for task in self.tasks.values() {
            for change_set in &task.change_sets {
                match (change_set.status, change_set.reverted_by) {
                    (ChangeSetStatus::Reverted | ChangeSetStatus::RevertedWithConflicts, None) => {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "reverted change set {} has no compensation link",
                            change_set.id
                        )));
                    }
                    (
                        ChangeSetStatus::Running
                        | ChangeSetStatus::Completed
                        | ChangeSetStatus::PartiallyFailed
                        | ChangeSetStatus::Cancelled,
                        Some(_),
                    ) => {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "change set {} has a compensation link but is not reverted",
                            change_set.id
                        )));
                    }
                    _ => {}
                }
                if let Some(compensation_id) = change_set.reverted_by {
                    let compensation = task
                        .change_sets
                        .iter()
                        .find(|candidate| candidate.id == compensation_id)
                        .and_then(|candidate| candidate.compensation.as_ref());
                    if compensation.is_none_or(|compensation| {
                        compensation.target_change_set_id != change_set.id
                    }) {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "reverted change set {} points to an invalid compensation",
                            change_set.id
                        )));
                    }
                }
                let Some(compensation) = &change_set.compensation else {
                    continue;
                };
                if change_set.status != ChangeSetStatus::Completed
                    || change_set.reverted_by.is_some()
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "compensation change set {} must be completed and cannot itself be reverted",
                        change_set.id
                    )));
                }
                let target = task
                    .change_sets
                    .iter()
                    .find(|candidate| candidate.id == compensation.target_change_set_id)
                    .ok_or_else(|| {
                        WorkspaceError::InvalidWorkspace(format!(
                            "compensation change set {} references missing target {}",
                            change_set.id, compensation.target_change_set_id
                        ))
                    })?;
                if target.id == change_set.id
                    || target.compensation.is_some()
                    || target.reverted_by != Some(change_set.id)
                    || change_set.authorization != target.authorization
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "compensation change set {} does not have a valid target back-reference",
                        change_set.id
                    )));
                }
                let expected_target_status = if compensation.conflicts.is_empty() {
                    ChangeSetStatus::Reverted
                } else {
                    ChangeSetStatus::RevertedWithConflicts
                };
                if target.status != expected_target_status {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "target change set {} status does not match its compensation result",
                        target.id
                    )));
                }
                if !self
                    .store
                    .history()
                    .commits
                    .contains_key(&compensation.requested_at_revision)
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "compensation change set {} has a missing request revision",
                        change_set.id
                    )));
                }
                let mut target_commits = target.output_commits().collect::<Vec<_>>();
                target_commits.sort_unstable();
                target_commits.dedup();
                if target_commits.is_empty() {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "compensation change set {} target is not on its request ancestry",
                        change_set.id
                    )));
                }
                for commit_id in &target_commits {
                    if !self
                        .store
                        .is_ancestor(*commit_id, compensation.requested_at_revision)?
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "compensation change set {} target is not on its request ancestry",
                            change_set.id
                        )));
                    }
                }
                let (baseline, last_target_revision) =
                    compensation_target_state(self.store.history(), &target_commits)?;
                let request_versions = self
                    .store
                    .history()
                    .object_versions_at(compensation.requested_at_revision)?;
                let mut recorded_objects = BTreeSet::new();
                for object in &compensation.reverted_objects {
                    if !baseline.contains_key(object)
                        || !recorded_objects.insert(*object)
                        || request_versions
                            .precondition(*object)
                            .last_modified_revision
                            != Some(last_target_revision[object])
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "compensation change set {} has an invalid reverted object list",
                            change_set.id
                        )));
                    }
                }
                for conflict in &compensation.conflicts {
                    if !baseline.contains_key(&conflict.object)
                        || !recorded_objects.insert(conflict.object)
                        || last_target_revision.get(&conflict.object)
                            != Some(&conflict.target_revision)
                        || conflict.detail.trim().is_empty()
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "compensation change set {} has an invalid conflict record",
                            change_set.id
                        )));
                    }
                    let actual = request_versions.precondition(conflict.object);
                    match conflict.reason {
                        RevertConflictReason::ModifiedAfterTarget
                            if actual.last_modified_revision != conflict.conflicting_revision
                                || actual.last_modified_revision
                                    == Some(conflict.target_revision) =>
                        {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "compensation change set {} has a false modification conflict",
                                change_set.id
                            )));
                        }
                        RevertConflictReason::DependencyValidationFailed
                            if conflict.conflicting_revision.is_some()
                                || actual.last_modified_revision
                                    != Some(conflict.target_revision) =>
                        {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "compensation change set {} has a false dependency conflict",
                                change_set.id
                            )));
                        }
                        _ => {}
                    }
                }
                if recorded_objects != baseline.keys().copied().collect() {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "compensation change set {} does not account for every target object",
                        change_set.id
                    )));
                }
                let output_commits = change_set.output_commits().collect::<Vec<_>>();
                match compensation.commit_id {
                    Some(commit_id) => {
                        if output_commits != [commit_id] {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "compensation change set {} does not uniquely own its compensation commit",
                                change_set.id
                            )));
                        }
                        let commit = self
                            .store
                            .history()
                            .commits
                            .get(&commit_id)
                            .ok_or(HistoryError::CommitMissing(commit_id))?;
                        if commit.parent != Some(compensation.requested_at_revision)
                            || commit.action_source()
                                != Some(ActionSource::for_run(
                                    task.id,
                                    change_set.id,
                                    change_set.active_run_id,
                                ))
                            || !transaction_writes(&commit.transaction)
                                .is_subset(&compensation.reverted_objects.iter().copied().collect())
                            || commit.transaction.commands.is_empty()
                        {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "compensation commit {commit_id} does not match its recorded request"
                            )));
                        }
                    }
                    None if !output_commits.is_empty() => {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "compensation change set {} has an unrecorded output commit",
                            change_set.id
                        )));
                    }
                    None => {}
                }
                let result_revision = compensation
                    .commit_id
                    .unwrap_or(compensation.requested_at_revision);
                let result = self.store.history().restore(result_revision)?;
                for object in &compensation.reverted_objects {
                    if object_value(&result, *object) != baseline[object] {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "compensation change set {} did not restore object {object:?}",
                            change_set.id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_remote_audit_event(
        &self,
        task_id: TaskId,
        event: &TaskEvent,
    ) -> Result<(), WorkspaceError> {
        let TaskEvent::ProviderDisclosure {
            endpoint,
            model,
            project_id,
            grant_id,
            sent_at_unix_seconds,
            requested_capabilities,
            selected_entity_ids,
            includes_source_files,
            context_schema_version,
            source_revision,
            data_categories,
            payload_bytes,
            payload_hash,
            ..
        } = event
        else {
            return Ok(());
        };
        let legacy = *context_schema_version == 0
            && data_categories.is_empty()
            && *payload_bytes == 0
            && payload_hash.is_empty();
        if legacy {
            return Ok(());
        }
        let grant_binding_count = usize::from(project_id.is_some())
            + usize::from(grant_id.is_some())
            + usize::from(sent_at_unix_seconds.is_some());
        if !(MIN_HASH_BOUND_REMOTE_CONTEXT_SCHEMA_VERSION..=REMOTE_CONTEXT_SCHEMA_VERSION)
            .contains(context_schema_version)
            || !self.store.history().commits.contains_key(source_revision)
            || endpoint.trim().is_empty()
            || model.trim().is_empty()
            || *includes_source_files
            || data_categories.is_empty()
            || selected_entity_ids
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || *payload_bytes == 0
            || *payload_bytes > MAX_REMOTE_CONTEXT_BYTES
            || payload_hash.len() != 64
            || !payload_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !matches!(grant_binding_count, 0 | 3)
        {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} has an invalid remote-context audit event"
            )));
        }
        if let (Some(project_id), Some(grant_id), Some(sent_at_unix_seconds)) =
            (project_id, grant_id, sent_at_unix_seconds)
        {
            let Some(grant) = self.remote_access_policy.grants().get(grant_id) else {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} remote-context audit references missing grant {grant_id}"
                )));
            };
            if !grant.authorizes(RemoteAccessCheck {
                project_id: *project_id,
                endpoint,
                model,
                data_categories,
                capabilities: requested_capabilities,
                selected_entity_ids,
                payload_bytes: *payload_bytes,
                unix_seconds: *sent_at_unix_seconds,
            }) {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} remote-context audit exceeds grant {grant_id}"
                )));
            }
        }
        Ok(())
    }

    fn normalize_next_ids(&mut self) -> Result<(), WorkspaceError> {
        let minimum_next = next_id_after("task", self.tasks.keys().copied())?;
        self.next_task_id = self.next_task_id.max(minimum_next);
        let change_set_ids = self
            .tasks
            .values()
            .flat_map(|task| task.change_sets.iter().map(|change_set| change_set.id));
        let minimum_next_change_set = next_id_after("change set", change_set_ids)?;
        self.next_change_set_id = self.next_change_set_id.max(minimum_next_change_set);
        let run_ids = self.tasks.values().flat_map(|task| {
            task.change_sets
                .iter()
                .flat_map(|change_set| change_set.runs.iter().map(|run| run.id))
        });
        let minimum_next_run = next_id_after("agent run", run_ids)?;
        self.next_agent_run_id = self.next_agent_run_id.max(minimum_next_run);
        Ok(())
    }

    fn allocate_change_set_id(&mut self) -> Result<PromptChangeSetId, WorkspaceError> {
        if self.next_change_set_id == PromptChangeSetId::MAX {
            return Err(WorkspaceError::InvalidWorkspace(
                "change set id space is exhausted".into(),
            ));
        }
        let id = self.next_change_set_id;
        self.next_change_set_id += 1;
        Ok(id)
    }

    fn allocate_agent_run_id(&mut self) -> Result<AgentRunId, WorkspaceError> {
        if self.next_agent_run_id == AgentRunId::MAX {
            return Err(WorkspaceError::InvalidWorkspace(
                "agent run id space is exhausted".into(),
            ));
        }
        let id = self.next_agent_run_id;
        self.next_agent_run_id += 1;
        Ok(id)
    }

    fn migrate_legacy_task_hierarchy(&mut self) -> Result<(), WorkspaceError> {
        let mut sources = BTreeMap::new();
        for task in self.tasks.values() {
            for change_set in &task.change_sets {
                for run in &change_set.runs {
                    let source = ActionSource::for_run(task.id, change_set.id, run.id);
                    for action_commit in &run.action_commits {
                        if sources.insert(action_commit.commit_id, source).is_some() {
                            return Err(WorkspaceError::InvalidWorkspace(format!(
                                "legacy commit {} is claimed more than once",
                                action_commit.commit_id
                            )));
                        }
                    }
                }
            }
        }
        self.store.bind_legacy_task_commit_sources(&sources)?;
        for task in self.tasks.values_mut() {
            if !task.legacy_layout {
                continue;
            }
            let task_id = task.id;
            let change_set = task.active_change_set_mut().ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "legacy task {task_id} did not produce a change set"
                ))
            })?;
            if matches!(
                change_set.status,
                ChangeSetStatus::PartiallyFailed | ChangeSetStatus::Cancelled
            ) && change_set.diagnostics.is_empty()
            {
                let run = change_set.active_run().ok_or_else(|| {
                    WorkspaceError::InvalidWorkspace(format!(
                        "legacy task {task_id} did not produce an agent run"
                    ))
                })?;
                let message = run
                    .events
                    .iter()
                    .rev()
                    .find_map(|event| match event {
                        TaskEvent::Failed { message } => Some(message.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "Legacy task ended without a diagnostic.".into());
                change_set.diagnostics.push(ChangeSetDiagnostic {
                    run_id: run.id,
                    action_index: run
                        .execution
                        .as_ref()
                        .map(|execution| execution.next_action_index),
                    message,
                });
            }
            task.legacy_layout = false;
        }
        let next_actions = self
            .tasks
            .values()
            .filter_map(|task| {
                let change_set = task.active_change_set()?;
                let run = change_set.active_run()?;
                let execution = run.execution.as_ref()?;
                let action = execution.actions.get(execution.next_action_index)?;
                Some((
                    ActionSource::for_run(task.id, change_set.id, run.id),
                    execution.expected_revision,
                    action.transaction.clone(),
                ))
            })
            .collect::<Vec<_>>();
        for (source, revision, transaction) in next_actions {
            let revision = revision.ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {} cannot bind its next action without a checkpoint revision",
                    source.task_id
                ))
            })?;
            let preparation = PreparedAction::prepare_for_run(
                &self.store.snapshot_at(revision)?,
                source.task_id,
                source.change_set_id.expect("run-bound source"),
                source.agent_run_id.expect("run-bound source"),
                transaction,
            )?
            .record();
            self.tasks
                .get_mut(&source.task_id)
                .and_then(DesignTask::active_run_mut)
                .and_then(|run| run.execution.as_mut())
                .expect("legacy next action checked above")
                .next_action_preparation = Some(preparation);
        }
        Ok(())
    }

    fn ensure_revision(&self, expected: CommitId) -> Result<(), WorkspaceError> {
        let actual = self.revision();
        if expected != actual {
            return Err(WorkspaceError::StaleRevision { expected, actual });
        }
        Ok(())
    }

    fn infer_legacy_execution_revisions(&mut self) -> Result<(), WorkspaceError> {
        let history = self.store.history();
        for task in self.tasks.values_mut() {
            let task_id = task.id;
            let output_commits = task
                .active_run()
                .map(|run| run.output_commits().collect::<Vec<_>>())
                .unwrap_or_default();
            let Some(execution) = task.active_run_mut().and_then(|run| run.execution.as_mut())
            else {
                continue;
            };
            if execution.base_revision.is_some() && execution.expected_revision.is_some() {
                continue;
            }
            if output_commits.len() < execution.next_action_index {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {} has fewer output commits than completed actions",
                    task_id
                )));
            }
            let base_revision = match output_commits.first() {
                Some(first) => history
                    .commits
                    .get(first)
                    .ok_or(HistoryError::CommitMissing(*first))?
                    .parent
                    .ok_or_else(|| {
                        WorkspaceError::InvalidWorkspace(format!(
                            "task {} output starts at the root commit",
                            task_id
                        ))
                    })?,
                None => history.head(),
            };
            let expected_revision = if execution.next_action_index == 0 {
                base_revision
            } else {
                output_commits[execution.next_action_index - 1]
            };
            execution.base_revision = Some(base_revision);
            execution.expected_revision = Some(expected_revision);
        }
        Ok(())
    }

    fn infer_legacy_object_preconditions(&mut self) -> Result<(), WorkspaceError> {
        let task_ids = self.tasks.keys().copied().collect::<Vec<_>>();
        for task_id in task_ids {
            let next = self
                .tasks
                .get(&task_id)
                .and_then(DesignTask::active_run)
                .and_then(|run| run.execution.as_ref())
                .and_then(|execution| {
                    execution
                        .actions
                        .get(execution.next_action_index)
                        .map(|action| {
                            (
                                execution.expected_revision,
                                action.transaction.clone(),
                                execution.next_action_preparation.is_none(),
                            )
                        })
                });
            let Some((checkpoint_revision, transaction, missing)) = next else {
                continue;
            };
            if !missing {
                continue;
            }
            let checkpoint_revision = checkpoint_revision.ok_or_else(|| {
                WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} cannot infer object preconditions without an execution revision"
                ))
            })?;
            let snapshot = self.store.snapshot_at(checkpoint_revision)?;
            let source = self.active_action_source(task_id)?;
            let preparation = PreparedAction::prepare_for_run(
                &snapshot,
                source.task_id,
                source.change_set_id.expect("run-bound source"),
                source.agent_run_id.expect("run-bound source"),
                transaction,
            )?;
            self.tasks
                .get_mut(&task_id)
                .and_then(DesignTask::active_run_mut)
                .and_then(|run| run.execution.as_mut())
                .expect("task execution was checked above")
                .next_action_preparation = Some(preparation.record());
        }
        Ok(())
    }

    fn validate_task_execution(
        &self,
        task_id: TaskId,
        change_set_id: PromptChangeSetId,
        run: &AgentRun,
        execution: &TaskExecution,
    ) -> Result<(), WorkspaceError> {
        if execution.legacy_missing_strategy {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} execution is missing its explicit strategy"
            )));
        }
        if execution.legacy_missing_planning_budget {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} execution is missing its explicit planning budget"
            )));
        }
        let base_revision = execution.base_revision.ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} execution is missing its base revision"
            ))
        })?;
        let expected_revision = execution.expected_revision.ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} execution is missing its expected revision"
            ))
        })?;
        if !self.store.history().commits.contains_key(&base_revision)
            || !self
                .store
                .history()
                .commits
                .contains_key(&expected_revision)
        {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} execution references a missing revision"
            )));
        }
        if execution.next_action_index > execution.actions.len() {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "agent run {} has an invalid action checkpoint",
                run.id
            )));
        }
        let has_iterative_events = run.events.iter().any(|event| {
            matches!(
                event,
                TaskEvent::Reobserved { .. }
                    | TaskEvent::PlanningCompleted { .. }
                    | TaskEvent::ActionRejected { .. }
            )
        });
        match &execution.strategy {
            TaskExecutionStrategy::Batch => {
                if has_iterative_events
                    || execution.planning_budget
                        != TaskPlanningBudget::batch(execution.actions.len())
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "agent run {} has an invalid batch planning contract",
                        run.id
                    )));
                }
            }
            TaskExecutionStrategy::Iterative {
                planner_complete,
                last_failure,
            } => {
                if TaskPlanningBudget::iterative(execution.planning_budget.max_actions())
                    != Some(execution.planning_budget)
                    || execution.actions.len() > execution.planning_budget.max_actions()
                    || run
                        .events
                        .iter()
                        .filter(|event| matches!(event, TaskEvent::Reobserved { .. }))
                        .count()
                        > execution.planning_budget.max_decisions()
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "agent run {} exceeds its persisted iterative planning budget",
                        run.id
                    )));
                }
                let mut pending_observation = None;
                let mut staged_observation = None;
                let mut committed_action_count = 0;
                for event in &run.events {
                    match event {
                        TaskEvent::Reobserved {
                            revision,
                            action_index,
                            entity_count,
                        } => {
                            let observed = self.store.history().restore(*revision)?;
                            if pending_observation.is_some()
                                || staged_observation.is_some()
                                || *action_index != committed_action_count
                                || observed.entities.len() != *entity_count
                                || !self
                                    .store
                                    .history()
                                    .is_ancestor(*revision, self.store.history().active_head()?)?
                            {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} has an invalid iterative observation sequence",
                                    run.id
                                )));
                            }
                            pending_observation = Some((*revision, *action_index));
                        }
                        TaskEvent::Planned { action_count } => {
                            let Some(observation) = pending_observation.take() else {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} planned an iterative action without an observation",
                                    run.id
                                )));
                            };
                            if *action_count != 1 {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} persisted a non-atomic iterative decision",
                                    run.id
                                )));
                            }
                            staged_observation = Some(observation);
                        }
                        TaskEvent::ActionRejected { feedback, .. } => {
                            let expected = (feedback.observed_revision, feedback.action_index);
                            if pending_observation == Some(expected) {
                                pending_observation = None;
                            } else if staged_observation == Some(expected) {
                                staged_observation = None;
                            } else {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} rejected an action outside its observed decision",
                                    run.id
                                )));
                            }
                        }
                        TaskEvent::Committed { .. } => {
                            let Some((_, action_index)) = staged_observation.take() else {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} committed an iterative action without a staged decision",
                                    run.id
                                )));
                            };
                            if action_index != committed_action_count {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} committed an out-of-order iterative decision",
                                    run.id
                                )));
                            }
                            committed_action_count += 1;
                        }
                        TaskEvent::PlanningCompleted {
                            revision,
                            action_count,
                            ..
                        } => {
                            if pending_observation.take() != Some((*revision, *action_count))
                                || staged_observation.is_some()
                                || *action_count != committed_action_count
                            {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} completed outside its observed decision",
                                    run.id
                                )));
                            }
                        }
                        TaskEvent::Paused { .. }
                        | TaskEvent::Failed { .. }
                        | TaskEvent::Cancelled { .. } => pending_observation = None,
                        _ => {}
                    }
                }
                if committed_action_count != run.action_commits.len()
                    || (pending_observation.is_some()
                        && (run.status != AgentRunStatus::Running
                            || !execution.is_awaiting_planner()))
                    || (execution.is_awaiting_planner() && staged_observation.is_some())
                    || (execution.remaining_actions() > 0 && staged_observation.is_none())
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "agent run {} has an incomplete iterative decision sequence",
                        run.id
                    )));
                }
                if *planner_complete && execution.next_action_index != execution.actions.len() {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "agent run {} completed planning with an uncommitted action",
                        run.id
                    )));
                }
                let planning_completed = run
                    .events
                    .iter()
                    .filter_map(|event| {
                        if let TaskEvent::PlanningCompleted {
                            revision,
                            action_count,
                            summary,
                        } = event
                        {
                            Some((*revision, *action_count, summary))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                if *planner_complete {
                    let [(revision, action_count, summary)] = planning_completed.as_slice() else {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "agent run {} completed iterative planning without one completion event",
                            run.id
                        )));
                    };
                    if *action_count != execution.next_action_index
                        || summary.trim().is_empty()
                        || !self.store.history().commits.contains_key(revision)
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "agent run {} has an invalid planning completion event",
                            run.id
                        )));
                    }
                } else if !planning_completed.is_empty() {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "agent run {} has a completion event while planning remains active",
                        run.id
                    )));
                }
                for event in &run.events {
                    match event {
                        TaskEvent::Reobserved {
                            revision,
                            action_index,
                            entity_count,
                        } => {
                            let observed = self.store.history().restore(*revision)?;
                            if *action_index > execution.next_action_index
                                || observed.entities.len() != *entity_count
                            {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} has an invalid re-observation event",
                                    run.id
                                )));
                            }
                        }
                        TaskEvent::ActionRejected {
                            feedback,
                            will_retry,
                        } => {
                            let observed = run.events.iter().any(|candidate| {
                                matches!(
                                    candidate,
                                    TaskEvent::Reobserved {
                                        revision,
                                        action_index,
                                        ..
                                    } if *revision == feedback.observed_revision
                                        && *action_index == feedback.action_index
                                )
                            });
                            if feedback.action_index > execution.next_action_index
                                || feedback.repair_attempt == 0
                                || feedback.repair_attempt > MAX_AUTOMATIC_REPAIR_ATTEMPTS
                                || feedback.intent.trim().is_empty()
                                || feedback.tool_name.trim().is_empty()
                                || feedback.message.trim().is_empty()
                                || !observed
                                || (!will_retry && run.status != AgentRunStatus::Failed)
                            {
                                return Err(WorkspaceError::InvalidWorkspace(format!(
                                    "agent run {} has an invalid rejected-action event",
                                    run.id
                                )));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(feedback) = last_failure {
                    if feedback.action_index != execution.next_action_index
                        || feedback.repair_attempt == 0
                        || feedback.repair_attempt > MAX_AUTOMATIC_REPAIR_ATTEMPTS
                        || feedback.intent.trim().is_empty()
                        || feedback.tool_name.trim().is_empty()
                        || feedback.message.trim().is_empty()
                        || !self
                            .store
                            .history()
                            .commits
                            .contains_key(&feedback.observed_revision)
                    {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "agent run {} has invalid iterative repair feedback",
                            run.id
                        )));
                    }
                    let persisted_feedback = run.events.iter().rev().find_map(|event| {
                        if let TaskEvent::ActionRejected { feedback, .. } = event {
                            Some(feedback)
                        } else {
                            None
                        }
                    });
                    if persisted_feedback != Some(feedback) {
                        return Err(WorkspaceError::InvalidWorkspace(format!(
                            "agent run {} repair feedback is not bound to its audit event",
                            run.id
                        )));
                    }
                }
            }
        }
        if run.action_commits.len() != execution.next_action_index {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "agent run {} output commits do not match its action checkpoint",
                run.id
            )));
        }
        let mut checkpoint = base_revision;
        let history = self.store.history();
        for (expected_index, action_commit) in run.action_commits.iter().enumerate() {
            if action_commit.action_index != expected_index {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "agent run {} has a misordered action commit",
                    run.id
                )));
            }
            let commit_id = action_commit.commit_id;
            if checkpoint == commit_id || !history.is_ancestor(checkpoint, commit_id)? {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "agent run {} output commits are not in ancestor order",
                    run.id
                )));
            }
            let commit = history
                .commits
                .get(&commit_id)
                .ok_or(HistoryError::CommitMissing(commit_id))?;
            if commit.action_source() != Some(ActionSource::for_run(task_id, change_set_id, run.id))
            {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "agent run {} references a commit from another task or run",
                    run.id
                )));
            }
            checkpoint = commit_id;
        }
        if expected_revision != checkpoint {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "task {task_id} expected revision does not match its action checkpoint"
            )));
        }
        let next_action = execution.actions.get(execution.next_action_index);
        match (next_action, &execution.next_action_preparation) {
            (Some(action), Some(preparation)) => {
                if !history.commits.contains_key(&preparation.base_revision())
                    || !history.is_ancestor(expected_revision, preparation.base_revision())?
                    || !history.is_ancestor(preparation.base_revision(), history.active_head()?)?
                {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "task {task_id} next-action preparation has an invalid base revision"
                    )));
                }
                let expected = PreparedAction::prepare_for_run(
                    &self.store.snapshot_at(preparation.base_revision())?,
                    task_id,
                    change_set_id,
                    run.id,
                    action.transaction.clone(),
                )?
                .record();
                if &expected != preparation {
                    return Err(WorkspaceError::InvalidWorkspace(format!(
                        "task {task_id} change set {change_set_id} run {} next-action preparation does not match its checkpoint",
                        run.id
                    )));
                }
            }
            (Some(_), None) if run.status == AgentRunStatus::Failed => {}
            (Some(_), None) => {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} next action is missing its preparation record"
                )));
            }
            (None, Some(_)) => {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "task {task_id} completed action plan retains a preparation record"
                )));
            }
            (None, None) => {}
        }
        Ok(())
    }

    fn recover_interrupted_tasks(&mut self) {
        for task in self.tasks.values_mut() {
            if task.status != TaskStatus::Running {
                continue;
            }
            let checkpoint = task.active_run().and_then(|run| {
                run.execution.as_ref().map(|execution| {
                    (
                        execution.is_complete(),
                        execution.next_action_index,
                        execution.remaining_actions(),
                    )
                })
            });
            match checkpoint {
                Some((true, _, _)) => {
                    task.status = TaskStatus::Completed;
                    if let Some(change_set) = task.active_change_set_mut() {
                        change_set.status = ChangeSetStatus::Completed;
                        if let Some(run) = change_set.active_run_mut() {
                            run.status = AgentRunStatus::Completed;
                        }
                    }
                }
                Some((false, completed_actions, remaining_actions)) => {
                    task.status = TaskStatus::Paused;
                    if let Some(change_set) = task.active_change_set_mut() {
                        change_set.status = ChangeSetStatus::Running;
                        if let Some(run) = change_set.active_run_mut() {
                            run.status = AgentRunStatus::Paused;
                            run.events.push(TaskEvent::Paused {
                                completed_actions,
                                remaining_actions,
                                reason: "Recovered after application interruption".into(),
                            });
                        }
                    }
                }
                None => {
                    task.status = TaskStatus::Failed;
                    if let Some(change_set) = task.active_change_set_mut() {
                        change_set.status = ChangeSetStatus::PartiallyFailed;
                        let diagnostic = if let Some(run) = change_set.active_run_mut() {
                            run.status = AgentRunStatus::Failed;
                            let message =
                                "Task was interrupted before a durable action plan was stored."
                                    .to_string();
                            run.events.push(TaskEvent::Failed {
                                message: message.clone(),
                            });
                            Some(ChangeSetDiagnostic {
                                run_id: run.id,
                                action_index: None,
                                message,
                            })
                        } else {
                            None
                        };
                        if let Some(diagnostic) = diagnostic {
                            change_set.diagnostics.push(diagnostic);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum RevertObjectValue {
    Layer(Option<crate::Layer>),
    Entity(Option<crate::Entity>),
    Parameter(Option<crate::Parameter>),
    Constraint(Option<crate::SketchConstraint>),
}

type RevertBaseline = BTreeMap<ObjectId, RevertObjectValue>;
type RevertTargetRevisions = BTreeMap<ObjectId, CommitId>;

fn object_value(document: &CadDocument, object: ObjectId) -> RevertObjectValue {
    match object {
        ObjectId::Layer(id) => RevertObjectValue::Layer(document.layers.get(&id).cloned()),
        ObjectId::Entity(id) => RevertObjectValue::Entity(document.entities.get(&id).cloned()),
        ObjectId::Parameter(id) => {
            RevertObjectValue::Parameter(document.parameters.get(&id).cloned())
        }
        ObjectId::Constraint(id) => {
            RevertObjectValue::Constraint(document.constraints.get(&id).cloned())
        }
    }
}

fn compensation_target_state(
    history: &History,
    target_commits: &[CommitId],
) -> Result<(RevertBaseline, RevertTargetRevisions), WorkspaceError> {
    let mut baseline = BTreeMap::new();
    let mut last_target_revision = BTreeMap::new();
    for commit_id in target_commits {
        let commit = history
            .commits
            .get(commit_id)
            .ok_or(HistoryError::CommitMissing(*commit_id))?;
        let parent = commit.parent.ok_or_else(|| {
            WorkspaceError::InvalidWorkspace(format!(
                "target commit {commit_id} does not have a parent"
            ))
        })?;
        let before = history.restore(parent)?;
        for object in transaction_writes(&commit.transaction) {
            baseline
                .entry(object)
                .or_insert_with(|| object_value(&before, object));
            last_target_revision.insert(object, *commit_id);
        }
    }
    Ok((baseline, last_target_revision))
}

fn compensation_object_order(
    objects: &BTreeSet<ObjectId>,
    baseline: &RevertBaseline,
) -> Vec<ObjectId> {
    let rank = |object: ObjectId| match baseline.get(&object) {
        Some(RevertObjectValue::Layer(Some(_))) => 0,
        Some(RevertObjectValue::Parameter(Some(_))) => 1,
        Some(RevertObjectValue::Constraint(None)) => 2,
        Some(RevertObjectValue::Entity(_)) => 3,
        Some(RevertObjectValue::Constraint(Some(_))) => 4,
        Some(RevertObjectValue::Parameter(None)) => 5,
        Some(RevertObjectValue::Layer(None)) => 6,
        None => 7,
    };
    let mut ordered = objects.iter().copied().collect::<Vec<_>>();
    ordered.sort_by_key(|object| (rank(*object), *object));
    ordered
}

fn build_compensation_transaction(
    current: &CadDocument,
    baseline: &RevertBaseline,
    selected: &BTreeSet<ObjectId>,
) -> CommandTransaction {
    let mut commands = Vec::new();

    // Restore or create referenced containers first. Locked target layers are
    // temporarily opened and returned to their exact baseline state last.
    for object in compensation_object_order(selected, baseline) {
        let ObjectId::Layer(id) = object else {
            continue;
        };
        match baseline.get(&object) {
            Some(RevertObjectValue::Layer(Some(desired))) => {
                let mut writable = desired.clone();
                writable.locked = false;
                match current.layers.get(&id) {
                    None => commands.push(CadCommand::CreateLayer { layer: writable }),
                    Some(existing) if existing != &writable => {
                        commands.push(CadCommand::UpdateLayer { layer: writable });
                    }
                    Some(_) => {}
                }
            }
            Some(RevertObjectValue::Layer(None)) => {
                if let Some(existing) = current.layers.get(&id).filter(|layer| layer.locked) {
                    commands.push(CadCommand::UpdateLayer {
                        layer: crate::Layer {
                            locked: false,
                            ..existing.clone()
                        },
                    });
                }
            }
            _ => {}
        }
    }
    for object in compensation_object_order(selected, baseline) {
        let ObjectId::Parameter(id) = object else {
            continue;
        };
        let Some(RevertObjectValue::Parameter(Some(desired))) = baseline.get(&object) else {
            continue;
        };
        if current.parameters.get(&id) != Some(desired) {
            commands.push(CadCommand::SetParameter {
                parameter: desired.clone(),
            });
        }
    }
    for object in compensation_object_order(selected, baseline) {
        let ObjectId::Constraint(id) = object else {
            continue;
        };
        if matches!(
            baseline.get(&object),
            Some(RevertObjectValue::Constraint(None))
        ) && current.constraints.contains_key(&id)
        {
            commands.push(CadCommand::DeleteConstraint { id });
        }
    }
    for object in compensation_object_order(selected, baseline) {
        let ObjectId::Entity(id) = object else {
            continue;
        };
        match baseline.get(&object) {
            Some(RevertObjectValue::Entity(Some(desired))) => match current.entities.get(&id) {
                None => commands.push(CadCommand::CreateEntity {
                    entity: desired.clone(),
                }),
                Some(existing) if existing != desired => {
                    commands.push(CadCommand::UpdateEntity {
                        entity: desired.clone(),
                    });
                }
                Some(_) => {}
            },
            Some(RevertObjectValue::Entity(None)) if current.entities.contains_key(&id) => {
                commands.push(CadCommand::DeleteEntity { id });
            }
            _ => {}
        }
    }
    for object in compensation_object_order(selected, baseline) {
        let ObjectId::Constraint(id) = object else {
            continue;
        };
        let Some(RevertObjectValue::Constraint(Some(desired))) = baseline.get(&object) else {
            continue;
        };
        match current.constraints.get(&id) {
            None => commands.push(CadCommand::CreateConstraint {
                constraint: desired.clone(),
            }),
            Some(existing) if existing != desired => {
                commands.push(CadCommand::UpdateConstraint {
                    constraint: desired.clone(),
                });
            }
            Some(_) => {}
        }
    }
    for object in compensation_object_order(selected, baseline) {
        let ObjectId::Parameter(id) = object else {
            continue;
        };
        if matches!(
            baseline.get(&object),
            Some(RevertObjectValue::Parameter(None))
        ) && current.parameters.contains_key(&id)
        {
            commands.push(CadCommand::DeleteParameter { id });
        }
    }
    for object in compensation_object_order(selected, baseline) {
        let ObjectId::Layer(id) = object else {
            continue;
        };
        if matches!(baseline.get(&object), Some(RevertObjectValue::Layer(None)))
            && current.layers.contains_key(&id)
        {
            commands.push(CadCommand::DeleteLayer { id });
        }
    }
    for object in compensation_object_order(selected, baseline) {
        let Some(RevertObjectValue::Layer(Some(desired))) = baseline.get(&object) else {
            continue;
        };
        if desired.locked {
            commands.push(CadCommand::UpdateLayer {
                layer: desired.clone(),
            });
        }
    }
    CommandTransaction::new(commands)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    History(HistoryError),
    Command(CommandError),
    TaskMissing(TaskId),
    ChangeSetMissing(PromptChangeSetId),
    ChangeSetNotRevertible(PromptChangeSetId),
    ChangeSetAlreadyReverted(PromptChangeSetId),
    ChangeSetNotOnActiveBranch {
        change_set_id: PromptChangeSetId,
        commit_id: CommitId,
        current: CommitId,
    },
    Unauthorized(TaskId),
    HistoryNavigationBlocked(TaskId),
    StaleRevision {
        expected: CommitId,
        actual: CommitId,
    },
    PreparedBaseNotAncestor {
        base: CommitId,
        current: CommitId,
    },
    PreparedInputMismatch {
        base: CommitId,
    },
    ObjectPreconditionFailed {
        expected: ObjectPrecondition,
        actual: ObjectPrecondition,
    },
    PreparedSourceMismatch,
    IdempotencyConflict {
        existing_commit: CommitId,
        current: CommitId,
    },
    Prepare(PrepareError),
    RemotePolicy(RemotePolicyError),
    InvalidTaskState {
        task_id: TaskId,
        expected: TaskStatus,
        actual: TaskStatus,
    },
    InvalidWorkspace(String),
}

impl From<HistoryError> for WorkspaceError {
    fn from(error: HistoryError) -> Self {
        Self::History(error)
    }
}

impl From<CommandError> for WorkspaceError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl From<PrepareError> for WorkspaceError {
    fn from(error: PrepareError) -> Self {
        Self::Prepare(error)
    }
}

impl From<RemotePolicyError> for WorkspaceError {
    fn from(error: RemotePolicyError) -> Self {
        Self::RemotePolicy(error)
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::History(error) => error.fmt(formatter),
            Self::Command(error) => error.fmt(formatter),
            Self::TaskMissing(id) => write!(formatter, "task {id} does not exist"),
            Self::ChangeSetMissing(id) => write!(formatter, "change set {id} does not exist"),
            Self::ChangeSetNotRevertible(id) => {
                write!(formatter, "change set {id} cannot be reverted")
            }
            Self::ChangeSetAlreadyReverted(id) => {
                write!(formatter, "change set {id} was already reverted")
            }
            Self::ChangeSetNotOnActiveBranch {
                change_set_id,
                commit_id,
                current,
            } => write!(
                formatter,
                "change set {change_set_id} commit {commit_id} is not an ancestor of active revision {current}"
            ),
            Self::Unauthorized(id) => write!(
                formatter,
                "task {id} is not authorized to write this change"
            ),
            Self::HistoryNavigationBlocked(id) => write!(
                formatter,
                "task {id} must finish or fail before its latest action can be undone"
            ),
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "operation expected revision {expected}, but the active revision is {actual}"
            ),
            Self::PreparedBaseNotAncestor { base, current } => write!(
                formatter,
                "prepared action base revision {base} is not an ancestor of current revision {current}"
            ),
            Self::PreparedInputMismatch { base } => write!(
                formatter,
                "prepared action input does not match revision {base} in this workspace"
            ),
            Self::ObjectPreconditionFailed { expected, actual } => write!(
                formatter,
                "object precondition for {:?} expected existence {} at revision {:?}, but found existence {} at revision {:?}",
                expected.object,
                expected.exists,
                expected.last_modified_revision,
                actual.exists,
                actual.last_modified_revision
            ),
            Self::PreparedSourceMismatch => {
                formatter.write_str("prepared action source does not match the commit request")
            }
            Self::IdempotencyConflict {
                existing_commit,
                current,
            } => write!(
                formatter,
                "action was already committed as {existing_commit}, which is not an ancestor of current revision {current}"
            ),
            Self::Prepare(error) => error.fmt(formatter),
            Self::RemotePolicy(error) => error.fmt(formatter),
            Self::InvalidTaskState {
                task_id,
                expected,
                actual,
            } => write!(
                formatter,
                "task {task_id} must be {expected:?}, but is currently {actual:?}"
            ),
            Self::InvalidWorkspace(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for WorkspaceError {}
