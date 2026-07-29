//! Task-oriented agent orchestration for CADX.
//!
//! The planner may be backed by a local model, a cloud provider, or recorded
//! responses. It never receives a mutable document. The runner is the only
//! bridge to the workspace and writes through its authorization checks.

mod error;
mod genai_remote;
mod heuristic;
mod provider;
mod remote_plan;
mod runtime;

#[cfg(test)]
mod tests;

pub use error::AgentError;
pub use genai_remote::GenAiRemotePlanner;
pub use heuristic::HeuristicPlanner;
pub use provider::{
    AgentObservation, ContextDisclosure, ExecutionBudget, PlannedAction, PlanningDecision,
    ProviderConfig, ProviderDisclosure, RemoteContext, RemoteContextRequest, RemoteTaskPlanner,
    TaskPlanner,
};
pub use remote_plan::RemotePlanningDecision;
pub use runtime::{
    AgentRunReport, AuthorizedRemoteRound, RemoteRoundApply, RemoteRoundOutput, TaskAgent,
};
