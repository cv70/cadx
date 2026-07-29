use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};

use crate::command::{CadCommand, CommandTransaction};
use crate::document::{CadDocument, Domain};
use crate::{
    AgentRunId, CommitId, EntityId, ObjectId, PreparedActionRecord, ProjectId, PromptChangeSetId,
    RemoteGrantId, TaskId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    Drafting,
    Mechanical,
    Architecture,
    Parameters,
    Import,
}

pub const REMOTE_CONTEXT_SCHEMA_VERSION: u32 = 4;
pub(crate) const MIN_HASH_BOUND_REMOTE_CONTEXT_SCHEMA_VERSION: u32 = 1;
pub const MAX_REMOTE_CONTEXT_BYTES: usize = 64 * 1024;
pub const MAX_AUTOMATIC_REPAIR_ATTEMPTS: u8 = 3;
pub const MAX_ITERATIVE_ACTIONS_PER_RUN: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteDataCategory {
    TaskGoal,
    DocumentMetadata,
    DocumentStatistics,
    SelectionIdentifiers,
    GrantedCapabilities,
    ExecutionState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskAuthority {
    ReviewOnly,
    DirectWrite { capabilities: BTreeSet<Capability> },
}

impl TaskAuthority {
    pub fn all_direct() -> Self {
        Self::DirectWrite {
            capabilities: BTreeSet::from([
                Capability::Drafting,
                Capability::Mechanical,
                Capability::Architecture,
                Capability::Parameters,
                Capability::Import,
            ]),
        }
    }

    pub fn permits(&self, transaction: &CommandTransaction, document: &CadDocument) -> bool {
        let Self::DirectWrite { capabilities } = self else {
            return false;
        };
        // Authorization simulation applies commands to a temporary document.
        // Reject invalid external input first so it cannot reach internal
        // command application assumptions while authority is being checked.
        if transaction.preview(document).is_err() {
            return false;
        }
        let mut temporary = document.clone();
        transaction.commands.iter().all(|command| {
            let permitted = match command {
                CadCommand::CreateLayer { .. }
                | CadCommand::UpdateLayer { .. }
                | CadCommand::DeleteLayer { .. } => capabilities.contains(&Capability::Drafting),
                CadCommand::CreateEntity { entity } => {
                    permits_domain(capabilities, entity.kind.domain())
                }
                CadCommand::UpdateEntity { entity } => {
                    temporary.entities.get(&entity.id).is_some_and(|previous| {
                        permits_domain(capabilities, previous.kind.domain())
                            && permits_domain(capabilities, entity.kind.domain())
                    })
                }
                CadCommand::DeleteEntity { id } => temporary
                    .entities
                    .get(id)
                    .is_some_and(|entity| permits_domain(capabilities, entity.kind.domain())),
                CadCommand::SetParameter { .. } | CadCommand::DeleteParameter { .. } => {
                    capabilities.contains(&Capability::Parameters)
                }
                CadCommand::CreateConstraint { .. }
                | CadCommand::UpdateConstraint { .. }
                | CadCommand::DeleteConstraint { .. } => {
                    capabilities.contains(&Capability::Mechanical)
                }
            };
            if permitted {
                command.apply(&mut temporary);
            }
            permitted
        })
    }
}

fn permits_domain(capabilities: &BTreeSet<Capability>, domain: Domain) -> bool {
    capabilities.contains(&match domain {
        Domain::Drafting => Capability::Drafting,
        Domain::Mechanical => Capability::Mechanical,
        Domain::Architecture => Capability::Architecture,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEvent {
    Observed {
        entity_count: usize,
    },
    Reobserved {
        revision: CommitId,
        action_index: usize,
        entity_count: usize,
    },
    ProviderDisclosure {
        endpoint: String,
        model: String,
        #[serde(default)]
        project_id: Option<ProjectId>,
        #[serde(default)]
        grant_id: Option<RemoteGrantId>,
        #[serde(default)]
        sent_at_unix_seconds: Option<u64>,
        requested_capabilities: BTreeSet<Capability>,
        selected_entity_ids: Vec<EntityId>,
        includes_source_files: bool,
        payload_summary: String,
        #[serde(default)]
        context_schema_version: u32,
        #[serde(default)]
        source_revision: CommitId,
        #[serde(default)]
        data_categories: BTreeSet<RemoteDataCategory>,
        #[serde(default)]
        payload_bytes: usize,
        #[serde(default)]
        payload_hash: String,
    },
    Planned {
        action_count: usize,
    },
    PlanningCompleted {
        revision: CommitId,
        action_count: usize,
        summary: String,
    },
    ActionRejected {
        feedback: ActionFailureFeedback,
        will_retry: bool,
    },
    ToolCall {
        name: String,
        detail: String,
    },
    Committed {
        commit_id: CommitId,
        summary: String,
    },
    Validation {
        summary: String,
        passed: bool,
    },
    Validated {
        validator_id: String,
        validator_version: u32,
        candidate_state_hash: String,
        summary: String,
    },
    Paused {
        completed_actions: usize,
        remaining_actions: usize,
        reason: String,
    },
    Resumed {
        completed_actions: usize,
        remaining_actions: usize,
    },
    Failed {
        message: String,
    },
    Cancelled {
        reason: String,
    },
}

/// A planned, typed write action that can be persisted with a paused task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskAction {
    pub intent: String,
    pub tool_name: String,
    pub detail: String,
    pub transaction: CommandTransaction,
    /// Untrusted planner or caller claim retained for audit. The workspace
    /// generates independent local evidence before admitting the transaction.
    pub validation: ValidationReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionFailureKind {
    ToolRejected,
    ValidationFailed,
    StaleObservation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionFailureFeedback {
    pub action_index: usize,
    pub observed_revision: CommitId,
    pub repair_attempt: u8,
    pub kind: ActionFailureKind,
    pub intent: String,
    pub tool_name: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskExecutionStrategy {
    #[default]
    Batch,
    Iterative {
        planner_complete: bool,
        last_failure: Option<ActionFailureFeedback>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPlanningBudget {
    max_actions: usize,
    max_decisions: usize,
}

impl TaskPlanningBudget {
    pub fn iterative(max_actions: usize) -> Option<Self> {
        if max_actions == 0 || max_actions > MAX_ITERATIVE_ACTIONS_PER_RUN {
            return None;
        }
        Some(Self {
            max_actions,
            max_decisions: max_actions
                .checked_mul(usize::from(MAX_AUTOMATIC_REPAIR_ATTEMPTS) + 1)?
                .checked_add(1)?,
        })
    }

    pub const fn max_actions(self) -> usize {
        self.max_actions
    }

    pub const fn max_decisions(self) -> usize {
        self.max_decisions
    }

    pub(crate) const fn batch(action_count: usize) -> Self {
        Self {
            max_actions: action_count,
            max_decisions: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaskExecution {
    pub(crate) actions: Vec<TaskAction>,
    pub(crate) next_action_index: usize,
    pub(crate) base_revision: Option<CommitId>,
    pub(crate) expected_revision: Option<CommitId>,
    pub(crate) next_action_preparation: Option<PreparedActionRecord>,
    pub(crate) strategy: TaskExecutionStrategy,
    pub(crate) planning_budget: TaskPlanningBudget,
    #[serde(skip)]
    pub(crate) legacy_missing_strategy: bool,
    #[serde(skip)]
    pub(crate) legacy_missing_planning_budget: bool,
}

impl TaskExecution {
    pub(crate) fn new(
        actions: Vec<TaskAction>,
        base_revision: CommitId,
        next_action_preparation: Option<PreparedActionRecord>,
    ) -> Self {
        let planning_budget = TaskPlanningBudget::batch(actions.len());
        Self {
            actions,
            next_action_index: 0,
            base_revision: Some(base_revision),
            expected_revision: Some(base_revision),
            next_action_preparation,
            strategy: TaskExecutionStrategy::Batch,
            planning_budget,
            legacy_missing_strategy: false,
            legacy_missing_planning_budget: false,
        }
    }

    pub(crate) fn iterative(base_revision: CommitId, planning_budget: TaskPlanningBudget) -> Self {
        Self {
            actions: Vec::new(),
            next_action_index: 0,
            base_revision: Some(base_revision),
            expected_revision: Some(base_revision),
            next_action_preparation: None,
            strategy: TaskExecutionStrategy::Iterative {
                planner_complete: false,
                last_failure: None,
            },
            planning_budget,
            legacy_missing_strategy: false,
            legacy_missing_planning_budget: false,
        }
    }

    pub fn next_action_index(&self) -> usize {
        self.next_action_index
    }

    pub fn actions(&self) -> &[TaskAction] {
        &self.actions
    }

    pub const fn base_revision(&self) -> Option<CommitId> {
        self.base_revision
    }

    pub const fn expected_revision(&self) -> Option<CommitId> {
        self.expected_revision
    }

    pub fn next_action_preparation(&self) -> Option<&PreparedActionRecord> {
        self.next_action_preparation.as_ref()
    }

    pub fn remaining_actions(&self) -> usize {
        self.actions.len().saturating_sub(self.next_action_index)
    }

    pub fn is_complete(&self) -> bool {
        self.remaining_actions() == 0
            && match self.strategy {
                TaskExecutionStrategy::Batch => true,
                TaskExecutionStrategy::Iterative {
                    planner_complete, ..
                } => planner_complete,
            }
    }

    pub const fn strategy(&self) -> &TaskExecutionStrategy {
        &self.strategy
    }

    pub const fn planning_budget(&self) -> TaskPlanningBudget {
        self.planning_budget
    }

    pub fn is_iterative(&self) -> bool {
        matches!(self.strategy, TaskExecutionStrategy::Iterative { .. })
    }

    pub fn is_awaiting_planner(&self) -> bool {
        self.remaining_actions() == 0
            && matches!(
                self.strategy,
                TaskExecutionStrategy::Iterative {
                    planner_complete: false,
                    ..
                }
            )
    }

    pub fn last_failure(&self) -> Option<&ActionFailureFeedback> {
        match &self.strategy {
            TaskExecutionStrategy::Batch => None,
            TaskExecutionStrategy::Iterative { last_failure, .. } => last_failure.as_ref(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskExecutionWire {
    actions: Vec<TaskAction>,
    #[serde(default)]
    next_action_index: usize,
    #[serde(default)]
    base_revision: Option<CommitId>,
    #[serde(default)]
    expected_revision: Option<CommitId>,
    #[serde(default)]
    next_action_preparation: Option<PreparedActionRecord>,
    strategy: Option<TaskExecutionStrategy>,
    planning_budget: Option<TaskPlanningBudget>,
}

impl<'de> Deserialize<'de> for TaskExecution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = TaskExecutionWire::deserialize(deserializer)?;
        let legacy_missing_strategy = wire.strategy.is_none();
        let legacy_missing_planning_budget = wire.planning_budget.is_none();
        Ok(Self {
            actions: wire.actions,
            next_action_index: wire.next_action_index,
            base_revision: wire.base_revision,
            expected_revision: wire.expected_revision,
            next_action_preparation: wire.next_action_preparation,
            strategy: wire.strategy.unwrap_or_default(),
            planning_budget: wire.planning_budget.unwrap_or(TaskPlanningBudget {
                max_actions: 0,
                max_decisions: 0,
            }),
            legacy_missing_strategy,
            legacy_missing_planning_budget,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetStatus {
    Running,
    Completed,
    PartiallyFailed,
    Cancelled,
    Reverted,
    RevertedWithConflicts,
}

impl ChangeSetStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::PartiallyFailed
                | Self::Cancelled
                | Self::Reverted
                | Self::RevertedWithConflicts
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl AgentRunStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRunIdentity {
    pub kind: AgentKind,
    pub agent: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

impl AgentRunIdentity {
    pub fn local(agent: impl Into<String>) -> Self {
        Self {
            kind: AgentKind::Local,
            agent: agent.into(),
            provider: None,
            model: None,
        }
    }

    pub fn remote(
        agent: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            kind: AgentKind::Remote,
            agent: agent.into(),
            provider: Some(provider.into()),
            model: Some(model.into()),
        }
    }

    pub(crate) fn pending() -> Self {
        Self::local("pending")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredGoal {
    pub objective: String,
    pub constraints: Vec<String>,
    pub target_entity_ids: Vec<EntityId>,
}

impl StructuredGoal {
    pub fn from_prompt(prompt: impl Into<String>) -> Self {
        Self {
            objective: prompt.into(),
            constraints: Vec::new(),
            target_entity_ids: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeSetDiagnostic {
    pub run_id: AgentRunId,
    pub action_index: Option<usize>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeSetActionCommit {
    pub action_index: usize,
    pub commit_id: CommitId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSource {
    pub task_id: TaskId,
    pub change_set_id: Option<PromptChangeSetId>,
    pub agent_run_id: Option<AgentRunId>,
}

impl ActionSource {
    pub const fn for_run(
        task_id: TaskId,
        change_set_id: PromptChangeSetId,
        agent_run_id: AgentRunId,
    ) -> Self {
        Self {
            task_id,
            change_set_id: Some(change_set_id),
            agent_run_id: Some(agent_run_id),
        }
    }

    pub(crate) const fn legacy_task(task_id: TaskId) -> Self {
        Self {
            task_id,
            change_set_id: None,
            agent_run_id: None,
        }
    }

    pub const fn is_run_bound(self) -> bool {
        self.change_set_id.is_some() && self.agent_run_id.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevertConflictReason {
    ModifiedAfterTarget,
    DependencyValidationFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevertConflict {
    pub object: ObjectId,
    pub target_revision: CommitId,
    pub conflicting_revision: Option<CommitId>,
    pub reason: RevertConflictReason,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeSetCompensation {
    pub target_change_set_id: PromptChangeSetId,
    pub requested_at_revision: CommitId,
    pub reverted_objects: Vec<ObjectId>,
    pub conflicts: Vec<RevertConflict>,
    pub commit_id: Option<CommitId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSetRevertReport {
    pub target_change_set_id: PromptChangeSetId,
    pub compensation_change_set_id: PromptChangeSetId,
    pub commit_id: Option<CommitId>,
    pub reverted_objects: Vec<ObjectId>,
    pub conflicts: Vec<RevertConflict>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRun {
    pub id: AgentRunId,
    pub task_id: TaskId,
    pub change_set_id: PromptChangeSetId,
    pub attempt: u32,
    pub identity: AgentRunIdentity,
    pub status: AgentRunStatus,
    pub events: Vec<TaskEvent>,
    pub action_commits: Vec<ChangeSetActionCommit>,
    pub execution: Option<TaskExecution>,
}

impl AgentRun {
    pub fn output_commits(&self) -> impl Iterator<Item = CommitId> + '_ {
        self.action_commits.iter().map(|entry| entry.commit_id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptChangeSet {
    pub id: PromptChangeSetId,
    pub task_id: TaskId,
    pub prompt: String,
    pub structured_goal: StructuredGoal,
    pub authorization: TaskAuthority,
    pub status: ChangeSetStatus,
    pub runs: Vec<AgentRun>,
    pub active_run_id: AgentRunId,
    pub diagnostics: Vec<ChangeSetDiagnostic>,
    #[serde(default)]
    pub compensation: Option<ChangeSetCompensation>,
    #[serde(default)]
    pub reverted_by: Option<PromptChangeSetId>,
}

impl PromptChangeSet {
    pub fn active_run(&self) -> Option<&AgentRun> {
        self.runs.iter().find(|run| run.id == self.active_run_id)
    }

    pub(crate) fn active_run_mut(&mut self) -> Option<&mut AgentRun> {
        self.runs
            .iter_mut()
            .find(|run| run.id == self.active_run_id)
    }

    pub fn output_commits(&self) -> impl Iterator<Item = CommitId> + '_ {
        self.runs.iter().flat_map(AgentRun::output_commits)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DesignTask {
    pub id: TaskId,
    pub title: String,
    pub goal: String,
    pub authority: TaskAuthority,
    pub status: TaskStatus,
    pub change_sets: Vec<PromptChangeSet>,
    pub active_change_set_id: PromptChangeSetId,
    #[serde(skip)]
    pub(crate) legacy_layout: bool,
}

impl DesignTask {
    pub fn active_change_set(&self) -> Option<&PromptChangeSet> {
        self.change_sets
            .iter()
            .find(|change_set| change_set.id == self.active_change_set_id)
    }

    pub(crate) fn active_change_set_mut(&mut self) -> Option<&mut PromptChangeSet> {
        self.change_sets
            .iter_mut()
            .find(|change_set| change_set.id == self.active_change_set_id)
    }

    pub fn active_run(&self) -> Option<&AgentRun> {
        self.active_change_set()
            .and_then(PromptChangeSet::active_run)
    }

    pub(crate) fn active_run_mut(&mut self) -> Option<&mut AgentRun> {
        self.active_change_set_mut()
            .and_then(PromptChangeSet::active_run_mut)
    }

    pub fn active_prompt(&self) -> Option<&str> {
        self.active_change_set()
            .map(|change_set| change_set.prompt.as_str())
    }

    pub fn events(&self) -> &[TaskEvent] {
        self.active_run().map_or(&[], |run| run.events.as_slice())
    }

    pub fn execution(&self) -> Option<&TaskExecution> {
        self.active_run().and_then(|run| run.execution.as_ref())
    }

    pub fn output_commits(&self) -> impl Iterator<Item = CommitId> + '_ {
        self.change_sets
            .iter()
            .flat_map(PromptChangeSet::output_commits)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentDesignTaskWire {
    id: TaskId,
    title: String,
    goal: String,
    authority: TaskAuthority,
    status: TaskStatus,
    change_sets: Vec<PromptChangeSet>,
    active_change_set_id: PromptChangeSetId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyDesignTaskWire {
    id: TaskId,
    title: String,
    goal: String,
    authority: TaskAuthority,
    status: TaskStatus,
    events: Vec<TaskEvent>,
    output_commits: Vec<CommitId>,
    #[serde(default)]
    execution: Option<TaskExecution>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DesignTaskWire {
    Current(CurrentDesignTaskWire),
    Legacy(Box<LegacyDesignTaskWire>),
}

impl<'de> Deserialize<'de> for DesignTask {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        match DesignTaskWire::deserialize(deserializer)? {
            DesignTaskWire::Current(task) => Ok(Self {
                id: task.id,
                title: task.title,
                goal: task.goal,
                authority: task.authority,
                status: task.status,
                change_sets: task.change_sets,
                active_change_set_id: task.active_change_set_id,
                legacy_layout: false,
            }),
            DesignTaskWire::Legacy(task) => {
                let run_status = match task.status {
                    TaskStatus::Queued => AgentRunStatus::Queued,
                    TaskStatus::Running => AgentRunStatus::Running,
                    TaskStatus::Paused => AgentRunStatus::Paused,
                    TaskStatus::Completed => AgentRunStatus::Completed,
                    TaskStatus::Failed => AgentRunStatus::Failed,
                    TaskStatus::Cancelled => AgentRunStatus::Cancelled,
                };
                let change_set_status = match task.status {
                    TaskStatus::Completed => ChangeSetStatus::Completed,
                    TaskStatus::Failed => ChangeSetStatus::PartiallyFailed,
                    TaskStatus::Cancelled => ChangeSetStatus::Cancelled,
                    TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused => {
                        ChangeSetStatus::Running
                    }
                };
                let action_commits = task
                    .output_commits
                    .into_iter()
                    .enumerate()
                    .map(|(action_index, commit_id)| ChangeSetActionCommit {
                        action_index,
                        commit_id,
                    })
                    .collect();
                let change_set_id = task.id;
                let run_id = task.id;
                Ok(Self {
                    id: task.id,
                    title: task.title,
                    goal: task.goal.clone(),
                    authority: task.authority.clone(),
                    status: task.status,
                    change_sets: vec![PromptChangeSet {
                        id: change_set_id,
                        task_id: task.id,
                        prompt: task.goal.clone(),
                        structured_goal: StructuredGoal::from_prompt(task.goal),
                        authorization: task.authority,
                        status: change_set_status,
                        runs: vec![AgentRun {
                            id: run_id,
                            task_id: task.id,
                            change_set_id,
                            attempt: 1,
                            identity: AgentRunIdentity::local("legacy-agent"),
                            status: run_status,
                            events: task.events,
                            action_commits,
                            execution: task.execution,
                        }],
                        active_run_id: run_id,
                        diagnostics: Vec::new(),
                        compensation: None,
                        reverted_by: None,
                    }],
                    active_change_set_id: change_set_id,
                    legacy_layout: true,
                })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Passed,
    Warning,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub checks: Vec<CheckResult>,
}

impl ValidationReport {
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != CheckStatus::Failed)
    }

    pub fn summary(&self) -> String {
        let failures = self
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Failed)
            .count();
        if failures == 0 {
            format!("{} checks passed", self.checks.len())
        } else {
            format!("{failures} checks failed")
        }
    }
}
