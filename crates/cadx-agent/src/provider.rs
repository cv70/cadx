use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Write as _;

use cadx_core::{
    ActionFailureFeedback, AgentRunId, Capability, CommitId, DesignTask, DocumentSnapshot,
    EntityId, MAX_REMOTE_CONTEXT_BYTES, MAX_REMOTE_SELECTED_ENTITY_IDS, ProjectId,
    PromptChangeSetId, REMOTE_CONTEXT_SCHEMA_VERSION, RemoteAccessCheck, RemoteAccessGrant,
    RemoteAccessGrantRequest, RemoteDataCategory, RemoteGrantId, RemoteObjectScope, TaskAction,
    TaskAuthority, TaskEvent, TaskId, TaskPlanningBudget,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::error::AgentError;
use crate::remote_plan::RemotePlanningDecision;

const REMOTE_CONTEXT_HASH_DOMAIN: &[u8] = b"CADX-REMOTE-CONTEXT\0json-v1\0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub endpoint: String,
    pub model: String,
    pub enabled_capabilities: BTreeSet<Capability>,
}

impl ProviderConfig {
    pub(crate) fn validate(&self) -> Result<(), AgentError> {
        if self.model.trim().is_empty() {
            return Err(AgentError::Provider("provider model is required".into()));
        }
        let endpoint = Url::parse(&self.endpoint).map_err(|_| {
            AgentError::Provider("provider endpoint must be an absolute URL".into())
        })?;
        if endpoint.host_str().is_none() {
            return Err(AgentError::Provider(
                "provider endpoint must include a host".into(),
            ));
        }
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(AgentError::Provider(
                "provider endpoint must not contain credentials or query data".into(),
            ));
        }
        let local_http = matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && local_http) {
            return Err(AgentError::Provider(
                "provider endpoint must use HTTPS, except for a loopback HTTP endpoint".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDisclosure {
    pub entity_count: usize,
    pub selected_entity_ids: Vec<u64>,
    pub includes_source_files: bool,
    pub includes_document_metadata: bool,
    pub includes_task_goal: bool,
    pub action_index: usize,
    pub remaining_action_budget: usize,
    pub remaining_decision_budget: usize,
    pub includes_failure_feedback: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteContextRequest {
    selected_entity_ids: Vec<EntityId>,
}

impl RemoteContextRequest {
    pub fn selected_entities(selected_entity_ids: impl IntoIterator<Item = EntityId>) -> Self {
        Self {
            selected_entity_ids: selected_entity_ids
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteContext {
    source_revision: CommitId,
    data_categories: BTreeSet<RemoteDataCategory>,
    payload_hash: String,
    payload_json: String,
}

impl RemoteContext {
    pub const fn context_schema_version(&self) -> u32 {
        REMOTE_CONTEXT_SCHEMA_VERSION
    }

    pub const fn source_revision(&self) -> CommitId {
        self.source_revision
    }

    pub fn data_categories(&self) -> &BTreeSet<RemoteDataCategory> {
        &self.data_categories
    }

    pub fn payload_hash(&self) -> &str {
        &self.payload_hash
    }

    pub fn payload_bytes(&self) -> usize {
        self.payload_json.len()
    }

    pub fn payload_json(&self) -> &str {
        &self.payload_json
    }
}

impl fmt::Debug for RemoteContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteContext")
            .field("context_schema_version", &REMOTE_CONTEXT_SCHEMA_VERSION)
            .field("source_revision", &self.source_revision)
            .field("data_categories", &self.data_categories)
            .field("payload_hash", &self.payload_hash)
            .field("payload_bytes", &self.payload_bytes())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDisclosure {
    pub config: ProviderConfig,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub change_set_id: PromptChangeSetId,
    pub run_id: AgentRunId,
    pub requested_capabilities: BTreeSet<Capability>,
    pub context: ContextDisclosure,
    pub payload_summary: String,
    pub context_schema_version: u32,
    pub source_revision: CommitId,
    pub data_categories: BTreeSet<RemoteDataCategory>,
    pub payload_bytes: usize,
    pub payload_hash: String,
}

impl ProviderDisclosure {
    pub fn granted_audit_event(
        &self,
        grant_id: RemoteGrantId,
        sent_at_unix_seconds: u64,
    ) -> TaskEvent {
        TaskEvent::ProviderDisclosure {
            endpoint: self.config.endpoint.clone(),
            model: self.config.model.clone(),
            project_id: Some(self.project_id),
            grant_id: Some(grant_id),
            sent_at_unix_seconds: Some(sent_at_unix_seconds),
            requested_capabilities: self.requested_capabilities.clone(),
            selected_entity_ids: self.context.selected_entity_ids.clone(),
            includes_source_files: self.context.includes_source_files,
            payload_summary: self.payload_summary.clone(),
            context_schema_version: self.context_schema_version,
            source_revision: self.source_revision,
            data_categories: self.data_categories.clone(),
            payload_bytes: self.payload_bytes,
            payload_hash: self.payload_hash.clone(),
        }
    }

    pub fn grant_request(
        &self,
        granted_at_unix_seconds: u64,
        expires_at_unix_seconds: Option<u64>,
    ) -> RemoteAccessGrantRequest {
        RemoteAccessGrantRequest {
            endpoint: self.config.endpoint.clone(),
            model: self.config.model.clone(),
            allowed_data_categories: self.data_categories.clone(),
            allowed_capabilities: self.requested_capabilities.clone(),
            object_scope: RemoteObjectScope::from_selected_entities(
                self.context.selected_entity_ids.iter().copied(),
            ),
            max_payload_bytes: MAX_REMOTE_CONTEXT_BYTES,
            granted_at_unix_seconds,
            expires_at_unix_seconds,
        }
    }

    pub fn is_authorized_by(&self, grant: &RemoteAccessGrant, unix_seconds: u64) -> bool {
        grant.authorizes(RemoteAccessCheck {
            project_id: self.project_id,
            endpoint: &self.config.endpoint,
            model: &self.config.model,
            data_categories: &self.data_categories,
            capabilities: &self.requested_capabilities,
            selected_entity_ids: &self.context.selected_entity_ids,
            payload_bytes: self.payload_bytes,
            unix_seconds,
        })
    }
}

fn payload_summary(context: &ContextDisclosure) -> String {
    format!(
        "Task goal, document metadata, and execution state; entity count: {}; {} selected entity identifier(s); action index: {}; {} action decision(s) and {} planning decision(s) remain; failure feedback: {}; geometry, attachments, and source files: {}.",
        context.entity_count,
        context.selected_entity_ids.len(),
        context.action_index,
        context.remaining_action_budget,
        context.remaining_decision_budget,
        if context.includes_failure_feedback {
            "included"
        } else {
            "not included"
        },
        if context.includes_source_files {
            "included"
        } else {
            "not included"
        }
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionBudget {
    pub max_planned_actions: usize,
    pub max_actions_per_run: usize,
}

impl Default for ExecutionBudget {
    fn default() -> Self {
        Self {
            max_planned_actions: 16,
            max_actions_per_run: 8,
        }
    }
}

impl ExecutionBudget {
    pub(crate) fn validate(self) -> Result<(), AgentError> {
        if self.max_actions_per_run == 0 {
            return Err(AgentError::Provider(
                "execution budgets must be greater than zero".into(),
            ));
        }
        self.planning_budget()?;
        Ok(())
    }

    pub(crate) fn planning_budget(self) -> Result<TaskPlanningBudget, AgentError> {
        TaskPlanningBudget::iterative(self.max_planned_actions).ok_or_else(|| {
            AgentError::Provider(format!(
                "remote action budget must be between 1 and {}",
                cadx_core::MAX_ITERATIVE_ACTIONS_PER_RUN
            ))
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentObservation {
    pub task: DesignTask,
    pub snapshot: DocumentSnapshot,
}

impl AgentObservation {
    pub fn action_index(&self) -> usize {
        self.task
            .execution()
            .map_or(0, cadx_core::TaskExecution::next_action_index)
    }

    pub fn last_failure(&self) -> Option<&ActionFailureFeedback> {
        self.task
            .execution()
            .and_then(cadx_core::TaskExecution::last_failure)
    }
}

pub type PlannedAction = TaskAction;

#[derive(Clone, Debug, PartialEq)]
pub enum PlanningDecision {
    Action(PlannedAction),
    Complete { summary: String },
}

pub trait TaskPlanner {
    fn plan_next(&self, observation: &AgentObservation) -> Result<PlanningDecision, AgentError>;
}

/// A remote planner receives only the exact immutable context approved by the
/// user. It cannot access the full local observation or document through this
/// contract.
pub trait RemoteTaskPlanner {
    fn config(&self) -> &ProviderConfig;

    /// Revalidates provider egress immediately before a remote call. Network
    /// implementations must not turn this into a cached decision.
    fn authorize_egress(&self) -> Result<(), AgentError>;

    fn context_request(&self) -> RemoteContextRequest {
        RemoteContextRequest::default()
    }

    fn plan_remote(&self, context: RemoteContext) -> Result<RemotePlanningDecision, AgentError>;
}

pub(crate) fn prepare_remote_context(
    config: ProviderConfig,
    request: RemoteContextRequest,
    project_id: ProjectId,
    observation: &AgentObservation,
) -> Result<(RemoteContext, ProviderDisclosure), AgentError> {
    config.validate()?;
    let source_revision = observation.snapshot.revision();
    let document = observation.snapshot.document();
    let change_set = observation.task.active_change_set().ok_or_else(|| {
        AgentError::Provider("task does not have an active prompt change set".into())
    })?;
    let run = change_set
        .active_run()
        .ok_or_else(|| AgentError::Provider("change set does not have an active run".into()))?;
    let task_capabilities = match &change_set.authorization {
        TaskAuthority::ReviewOnly => BTreeSet::new(),
        TaskAuthority::DirectWrite { capabilities } => capabilities.clone(),
    };
    let requested_capabilities = config
        .enabled_capabilities
        .intersection(&task_capabilities)
        .copied()
        .collect::<BTreeSet<_>>();
    if request.selected_entity_ids.len() > MAX_REMOTE_SELECTED_ENTITY_IDS {
        return Err(AgentError::Provider(
            "remote context selects too many entity identifiers".into(),
        ));
    }
    if let Some(id) = request
        .selected_entity_ids
        .iter()
        .find(|id| !document.entities.contains_key(id))
    {
        return Err(AgentError::Provider(format!(
            "selected entity {id} is not part of the observed document"
        )));
    }
    let data_categories = BTreeSet::from([
        RemoteDataCategory::TaskGoal,
        RemoteDataCategory::DocumentMetadata,
        RemoteDataCategory::DocumentStatistics,
        RemoteDataCategory::SelectionIdentifiers,
        RemoteDataCategory::GrantedCapabilities,
        RemoteDataCategory::ExecutionState,
    ]);
    let execution = remote_execution_context(observation)?;
    let payload = RemoteContextPayload {
        context_schema_version: REMOTE_CONTEXT_SCHEMA_VERSION,
        project_id,
        source_revision,
        task_id: observation.task.id,
        change_set_id: change_set.id,
        run_id: run.id,
        task_goal: observation
            .task
            .active_prompt()
            .unwrap_or(&observation.task.goal)
            .to_owned(),
        document: RemoteDocumentContext {
            title: document.metadata.title.clone(),
            description: document.metadata.description.clone(),
            schema_version: document.schema_version,
            units: document.units,
            entity_count: document.entities.len(),
            selected_entity_ids: request.selected_entity_ids.clone(),
        },
        allowed_capabilities: requested_capabilities
            .iter()
            .map(|capability| capability_name(*capability).into())
            .collect(),
        execution: execution.clone(),
        data_categories: data_categories.clone(),
        source_files_included: false,
        attachments_included: false,
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|_| AgentError::Provider("could not encode remote planning context".into()))?;
    if payload_json.len() > MAX_REMOTE_CONTEXT_BYTES {
        return Err(AgentError::Provider(format!(
            "remote context exceeds the {MAX_REMOTE_CONTEXT_BYTES}-byte limit"
        )));
    }
    let payload_hash = remote_context_hash(payload_json.as_bytes());
    let context = RemoteContext {
        source_revision,
        data_categories: data_categories.clone(),
        payload_hash: payload_hash.clone(),
        payload_json,
    };
    let context_disclosure = ContextDisclosure {
        entity_count: document.entities.len(),
        selected_entity_ids: request.selected_entity_ids,
        includes_source_files: false,
        includes_document_metadata: true,
        includes_task_goal: true,
        action_index: execution.action_index,
        remaining_action_budget: execution.remaining_actions,
        remaining_decision_budget: execution.remaining_decisions,
        includes_failure_feedback: execution.last_failure.is_some(),
    };
    let disclosure = ProviderDisclosure {
        config,
        project_id,
        task_id: observation.task.id,
        change_set_id: change_set.id,
        run_id: run.id,
        requested_capabilities,
        payload_summary: payload_summary(&context_disclosure),
        context: context_disclosure,
        context_schema_version: REMOTE_CONTEXT_SCHEMA_VERSION,
        source_revision,
        data_categories,
        payload_bytes: context.payload_bytes(),
        payload_hash,
    };
    Ok((context, disclosure))
}

fn remote_context_hash(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(REMOTE_CONTEXT_HASH_DOMAIN);
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing a digest to a String cannot fail");
    }
    encoded
}

const fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::Drafting => "drafting",
        Capability::Mechanical => "mechanical",
        Capability::Architecture => "architecture",
        Capability::Parameters => "parameters",
        Capability::Import => "import",
    }
}

#[derive(Serialize)]
struct RemoteContextPayload {
    context_schema_version: u32,
    project_id: ProjectId,
    source_revision: CommitId,
    task_id: TaskId,
    change_set_id: PromptChangeSetId,
    run_id: AgentRunId,
    task_goal: String,
    document: RemoteDocumentContext,
    allowed_capabilities: Vec<String>,
    execution: RemoteExecutionContext,
    data_categories: BTreeSet<RemoteDataCategory>,
    source_files_included: bool,
    attachments_included: bool,
}

#[derive(Clone, Serialize)]
struct RemoteExecutionContext {
    action_index: usize,
    max_actions: usize,
    remaining_actions: usize,
    max_decisions: usize,
    remaining_decisions: usize,
    last_failure: Option<ActionFailureFeedback>,
}

fn remote_execution_context(
    observation: &AgentObservation,
) -> Result<RemoteExecutionContext, AgentError> {
    let Some(execution) = observation.task.execution() else {
        let budget = ExecutionBudget::default().planning_budget()?;
        return Ok(RemoteExecutionContext {
            action_index: 0,
            max_actions: budget.max_actions(),
            remaining_actions: budget.max_actions(),
            max_decisions: budget.max_decisions(),
            remaining_decisions: budget.max_decisions(),
            last_failure: None,
        });
    };
    if !execution.is_iterative() {
        return Err(AgentError::Provider(
            "remote planning context requires iterative execution".into(),
        ));
    }
    let budget = execution.planning_budget();
    let decisions = observation
        .task
        .events()
        .iter()
        .filter(|event| matches!(event, TaskEvent::Reobserved { .. }))
        .count();
    Ok(RemoteExecutionContext {
        action_index: execution.next_action_index(),
        max_actions: budget.max_actions(),
        remaining_actions: budget
            .max_actions()
            .saturating_sub(execution.actions().len()),
        max_decisions: budget.max_decisions(),
        remaining_decisions: budget.max_decisions().saturating_sub(decisions),
        last_failure: execution.last_failure().cloned(),
    })
}

#[derive(Serialize)]
struct RemoteDocumentContext {
    title: String,
    description: String,
    schema_version: u32,
    units: cadx_core::Units,
    entity_count: usize,
    selected_entity_ids: Vec<EntityId>,
}
