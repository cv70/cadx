use std::fmt;

use cadx_core::{RemoteGrantId, TaskId, WorkspaceError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    Planning(String),
    Provider(String),
    DisclosureDoesNotMatch(TaskId),
    RemoteGrantDoesNotAuthorize(RemoteGrantId),
    BudgetExceeded {
        planned_actions: usize,
        limit: usize,
    },
    AutomaticRepairExhausted {
        attempts: u8,
        last_error: String,
    },
    Workspace(WorkspaceError),
}

impl From<WorkspaceError> for AgentError {
    fn from(error: WorkspaceError) -> Self {
        Self::Workspace(error)
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Planning(message) => formatter.write_str(message),
            Self::Provider(message) => formatter.write_str(message),
            Self::DisclosureDoesNotMatch(task_id) => write!(
                formatter,
                "reviewed remote disclosure no longer matches task {task_id}"
            ),
            Self::RemoteGrantDoesNotAuthorize(grant_id) => write!(
                formatter,
                "project remote access grant {grant_id} does not authorize this request"
            ),
            Self::BudgetExceeded {
                planned_actions,
                limit,
            } => write!(
                formatter,
                "provider planned {planned_actions} actions, exceeding the task limit of {limit}"
            ),
            Self::AutomaticRepairExhausted {
                attempts,
                last_error,
            } => write!(
                formatter,
                "automatic action repair failed after {attempts} attempt(s): {last_error}"
            ),
            Self::Workspace(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AgentError {}
