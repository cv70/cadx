//! The deterministic design document, command, task, and history contracts.
//!
//! This crate deliberately has no renderer, AI provider, or window-system
//! dependency. Every document mutation flows through [`CommandTransaction`].

mod command;
mod constraint;
mod document;
mod expression;
mod history;
mod kernel;
mod object;
mod prepared;
mod remote_policy;
mod snapshot;
mod store;
mod task;
mod validation;
mod workspace;

#[cfg(test)]
mod tests;

pub const CURRENT_SCHEMA_VERSION: u32 = 5;

pub type LayerId = u64;
pub type EntityId = u64;
pub type ParameterId = u64;
pub type ConstraintId = u64;
pub type TaskId = u64;
pub type PromptChangeSetId = u64;
pub type AgentRunId = u64;
pub type CommitId = u64;

pub const INITIAL_SNAPSHOT_INTERVAL: CommitId = 4;

pub use command::{CadCommand, CommandTransaction, DocumentDiff};
pub use constraint::{
    ConstraintDiagnostic, ConstraintError, ConstraintKind, ConstraintSolution,
    ConstraintSolverSettings, PointAnchor, SketchConstraint, SketchPoint, SketchSegment,
    solve_constraints,
};
pub use document::{
    CadDocument, CommandError, DocumentMetadata, DocumentSummary, Domain, Entity, EntityKind,
    Layer, Parameter, Point2, Units,
};
pub use expression::{ExpressionError, ParameterExpression, is_valid_parameter_name};
pub use history::{
    DesignBranch, History, HistoryComparison, HistoryError, SemanticCommit, Snapshot,
};
pub use kernel::KernelFacade;
pub use object::{ObjectId, ObjectPrecondition};
pub use prepared::{ActionIdempotencyKey, PrepareError, PreparedAction, PreparedActionRecord};
pub use remote_policy::{
    MAX_REMOTE_SELECTED_ENTITY_IDS, ProjectId, RemoteAccessCheck, RemoteAccessGrant,
    RemoteAccessGrantRequest, RemoteGrantId, RemoteObjectScope, RemotePolicyError,
    RemotePolicyEvent,
};
pub use snapshot::DocumentSnapshot;
pub use task::{
    ActionFailureFeedback, ActionFailureKind, ActionSource, AgentKind, AgentRun, AgentRunIdentity,
    AgentRunStatus, Capability, ChangeSetActionCommit, ChangeSetCompensation, ChangeSetDiagnostic,
    ChangeSetRevertReport, ChangeSetStatus, CheckResult, CheckStatus, DesignTask,
    MAX_AUTOMATIC_REPAIR_ATTEMPTS, MAX_ITERATIVE_ACTIONS_PER_RUN, MAX_REMOTE_CONTEXT_BYTES,
    PromptChangeSet, REMOTE_CONTEXT_SCHEMA_VERSION, RemoteDataCategory, RevertConflict,
    RevertConflictReason, StructuredGoal, TaskAction, TaskAuthority, TaskEvent, TaskExecution,
    TaskExecutionStrategy, TaskPlanningBudget, TaskStatus, ValidationReport,
};
pub use validation::{
    CORE_VALIDATOR_ID, CORE_VALIDATOR_VERSION, MAX_CANDIDATE_STATE_BYTES, ValidationEvidence,
};
pub use workspace::{TaskWorkspace, WorkspaceError};
