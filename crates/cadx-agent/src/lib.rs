//! Task-oriented agent orchestration for CADX.
//!
//! The planner may be backed by a local model, a cloud provider, or recorded
//! responses. It never receives a mutable document. The runner is the only
//! bridge to the workspace and writes through its authorization checks.

use std::collections::BTreeSet;
use std::fmt;

use cadx_core::{
    CadCommand, CadDocument, Capability, CheckResult, CheckStatus, CommandTransaction, DesignTask,
    Entity, EntityKind, Point2, TaskEvent, TaskId, TaskStatus, TaskWorkspace, ValidationReport,
    WorkspaceError,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub endpoint: String,
    pub model: String,
    pub enabled_capabilities: BTreeSet<Capability>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDisclosure {
    pub entity_count: usize,
    pub selected_entity_ids: Vec<u64>,
    pub includes_source_files: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentObservation {
    pub task: DesignTask,
    pub document: CadDocument,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedAction {
    pub intent: String,
    pub tool_name: String,
    pub detail: String,
    pub transaction: CommandTransaction,
    pub validation: ValidationReport,
}

pub trait TaskPlanner {
    fn plan(&self, observation: &AgentObservation) -> Result<Vec<PlannedAction>, AgentError>;
}

#[derive(Clone, Debug, Default)]
pub struct TaskAgent<P> {
    planner: P,
}

impl<P> TaskAgent<P>
where
    P: TaskPlanner,
{
    pub fn new(planner: P) -> Self {
        Self { planner }
    }

    pub fn run(
        &self,
        workspace: &mut TaskWorkspace,
        task_id: TaskId,
    ) -> Result<AgentRunReport, AgentError> {
        workspace.begin_task(task_id)?;
        let task = workspace
            .tasks
            .get(&task_id)
            .cloned()
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        let observation = AgentObservation {
            task,
            document: workspace.document.clone(),
        };
        let actions = match self.planner.plan(&observation) {
            Ok(actions) => actions,
            Err(error) => {
                workspace.fail_task(task_id, error.to_string())?;
                return Err(error);
            }
        };
        workspace.record_event(
            task_id,
            TaskEvent::Planned {
                action_count: actions.len(),
            },
        )?;

        let mut commit_ids = Vec::new();
        for action in actions {
            workspace.record_event(
                task_id,
                TaskEvent::ToolCall {
                    name: action.tool_name.clone(),
                    detail: action.detail.clone(),
                },
            )?;
            let commit_id = match workspace.apply_task_transaction(
                task_id,
                action.intent,
                action.transaction,
                action.validation,
            ) {
                Ok(commit_id) => commit_id,
                Err(error) => {
                    workspace.fail_task(task_id, error.to_string())?;
                    return Err(error.into());
                }
            };
            commit_ids.push(commit_id);
        }
        workspace.complete_task(task_id)?;
        Ok(AgentRunReport {
            task_id,
            status: TaskStatus::Completed,
            commit_ids,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRunReport {
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub commit_ids: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    Planning(String),
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
            Self::Workspace(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AgentError {}

/// A deterministic stand-in for a model-backed planner.
///
/// It makes the desktop prototype useful without giving heuristic code a
/// special mutation path. Production planners implement [`TaskPlanner`] and
/// return the same typed actions after model/tool orchestration.
#[derive(Clone, Debug, Default)]
pub struct HeuristicPlanner;

impl TaskPlanner for HeuristicPlanner {
    fn plan(&self, observation: &AgentObservation) -> Result<Vec<PlannedAction>, AgentError> {
        if observation.task.goal.trim().is_empty() {
            return Err(AgentError::Planning("a task goal is required".into()));
        }
        let goal = observation.task.goal.to_ascii_lowercase();
        let start_id = observation.document.next_entity_id();
        let action = if contains_any(
            &goal,
            &["room", "floor", "wall", "building", "architecture"],
        ) {
            architecture_action(start_id)
        } else if contains_any(
            &goal,
            &["bracket", "mechanical", "mount", "part", "extrude"],
        ) {
            mechanical_action(start_id)
        } else {
            drafting_action(start_id)
        };
        Ok(vec![action])
    }
}

fn contains_any(goal: &str, words: &[&str]) -> bool {
    words.iter().any(|word| goal.contains(word))
}

fn entity(id: u64, name: &str, kind: EntityKind) -> Entity {
    Entity {
        id,
        layer: 1,
        name: name.into(),
        visible: true,
        kind,
        parameter_refs: BTreeSet::new(),
    }
}

fn mechanical_action(id: u64) -> PlannedAction {
    let profile = entity(
        id,
        "Mounting bracket profile",
        EntityKind::SketchProfile {
            points: vec![
                Point2::new(-45.0, -30.0),
                Point2::new(45.0, -30.0),
                Point2::new(45.0, 30.0),
                Point2::new(-45.0, 30.0),
            ],
            closed: true,
        },
    );
    let hole = entity(
        id + 1,
        "Mounting hole",
        EntityKind::Circle {
            center: Point2::new(0.0, 0.0),
            radius: 8.0,
        },
    );
    let solid = entity(
        id + 2,
        "Bracket extrusion",
        EntityKind::Extrude {
            profile: id,
            distance: 12.0,
        },
    );
    PlannedAction {
        intent: "Create an editable mounting bracket concept".into(),
        tool_name: "mechanical.create_feature".into(),
        detail: "Created a closed profile, mounting hole reference, and 12 mm extrusion.".into(),
        transaction: CommandTransaction::new(vec![
            CadCommand::CreateEntity { entity: profile },
            CadCommand::CreateEntity { entity: hole },
            CadCommand::CreateEntity { entity: solid },
        ]),
        validation: ValidationReport {
            checks: vec![
                pass(
                    "Closed profile",
                    "The extrusion input is a closed sketch profile.",
                ),
                pass("Positive extrusion", "Feature depth is 12 mm."),
            ],
        },
    }
}

fn architecture_action(id: u64) -> PlannedAction {
    let points = vec![
        Point2::new(-60.0, -40.0),
        Point2::new(60.0, -40.0),
        Point2::new(60.0, 40.0),
        Point2::new(-60.0, 40.0),
    ];
    let mut commands = Vec::new();
    for index in 0..4 {
        let start = points[index];
        let end = points[(index + 1) % 4];
        commands.push(CadCommand::CreateEntity {
            entity: entity(
                id + index as u64,
                &format!("Perimeter wall {}", index + 1),
                EntityKind::Wall {
                    start,
                    end,
                    thickness: 180.0,
                },
            ),
        });
    }
    commands.push(CadCommand::CreateEntity {
        entity: entity(
            id + 4,
            "Concept room",
            EntityKind::Room {
                boundary: points,
                area: 9_600.0,
            },
        ),
    });
    PlannedAction {
        intent: "Create a semantic room perimeter".into(),
        tool_name: "architecture.create_room".into(),
        detail: "Created four connected walls and a room object with an editable boundary.".into(),
        transaction: CommandTransaction::new(commands),
        validation: ValidationReport {
            checks: vec![
                pass(
                    "Closed perimeter",
                    "Four wall segments form a closed room boundary.",
                ),
                pass("Room area", "Initial area is 9,600 square model units."),
            ],
        },
    }
}

fn drafting_action(id: u64) -> PlannedAction {
    PlannedAction {
        intent: "Create an editable drafting concept".into(),
        tool_name: "drafting.create_geometry".into(),
        detail: "Created a base rectangle and center annotation.".into(),
        transaction: CommandTransaction::new(vec![
            CadCommand::CreateEntity {
                entity: entity(
                    id,
                    "Draft rectangle",
                    EntityKind::Rectangle {
                        origin: Point2::new(-50.0, -30.0),
                        width: 100.0,
                        height: 60.0,
                    },
                ),
            },
            CadCommand::CreateEntity {
                entity: entity(
                    id + 1,
                    "Design note",
                    EntityKind::Text {
                        position: Point2::new(-35.0, 0.0),
                        content: "Concept".into(),
                    },
                ),
            },
        ]),
        validation: ValidationReport {
            checks: vec![pass("Geometry", "Draft entities are finite and editable.")],
        },
    }
}

fn pass(name: &str, detail: &str) -> CheckResult {
    CheckResult {
        name: name.into(),
        status: CheckStatus::Passed,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use cadx_core::{CadDocument, TaskAuthority};

    use super::*;

    #[test]
    fn direct_write_task_creates_a_replayable_mechanical_commit() {
        let mut workspace = TaskWorkspace::new(CadDocument::new("Bracket"));
        let task_id = workspace.create_task(
            "Create bracket",
            "Create a mechanical mounting bracket",
            TaskAuthority::all_direct(),
        );
        let report = TaskAgent::new(HeuristicPlanner)
            .run(&mut workspace, task_id)
            .unwrap();

        assert_eq!(report.commit_ids.len(), 1);
        assert_eq!(workspace.document.entities.len(), 3);
        assert_eq!(workspace.tasks[&task_id].status, TaskStatus::Completed);
        assert_eq!(
            workspace.history.restore(report.commit_ids[0]).unwrap(),
            workspace.document
        );
    }

    #[test]
    fn review_only_task_never_bypasses_workspace_authorization() {
        let mut workspace = TaskWorkspace::new(CadDocument::new("Review"));
        let task_id = workspace.create_task(
            "Review",
            "Create a bracket",
            cadx_core::TaskAuthority::ReviewOnly,
        );
        let error = TaskAgent::new(HeuristicPlanner)
            .run(&mut workspace, task_id)
            .unwrap_err();

        assert!(matches!(
            error,
            AgentError::Workspace(WorkspaceError::Unauthorized(_))
        ));
        assert!(workspace.document.entities.is_empty());
        assert_eq!(workspace.tasks[&task_id].status, TaskStatus::Failed);
    }
}
