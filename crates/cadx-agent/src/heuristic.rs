use std::collections::BTreeSet;

use cadx_core::{
    CadCommand, CheckResult, CheckStatus, CommandTransaction, Entity, EntityKind, Point2,
    ValidationReport,
};

use crate::error::AgentError;
use crate::provider::{AgentObservation, PlannedAction, PlanningDecision, TaskPlanner};

/// A deterministic stand-in for a model-backed planner.
///
/// It makes the desktop prototype useful without giving heuristic code a
/// special mutation path. Production planners implement [`TaskPlanner`] and
/// return the same typed actions after model/tool orchestration.
#[derive(Clone, Debug, Default)]
pub struct HeuristicPlanner;

impl TaskPlanner for HeuristicPlanner {
    fn plan_next(&self, observation: &AgentObservation) -> Result<PlanningDecision, AgentError> {
        if observation.action_index() > 0 {
            return Ok(PlanningDecision::Complete {
                summary: "The requested editable concept has been created and re-observed.".into(),
            });
        }
        let prompt = observation
            .task
            .active_prompt()
            .unwrap_or(&observation.task.goal);
        if prompt.trim().is_empty() {
            return Err(AgentError::Planning("a task goal is required".into()));
        }
        let goal = prompt.to_ascii_lowercase();
        let document = observation.snapshot.document();
        let start_id = document.next_entity_id();
        let layer = observation
            .snapshot
            .document()
            .layers
            .values()
            .find(|layer| layer.visible && !layer.locked)
            .map(|layer| layer.id)
            .ok_or_else(|| AgentError::Planning("no visible unlocked layer is available".into()))?;
        let action = if contains_any(
            &goal,
            &["room", "floor", "wall", "building", "architecture"],
        ) {
            architecture_action(start_id, layer)
        } else if contains_any(
            &goal,
            &["bracket", "mechanical", "mount", "part", "extrude"],
        ) {
            mechanical_action(start_id, layer)
        } else {
            drafting_action_on_layer(start_id, layer)
        };
        Ok(PlanningDecision::Action(action))
    }
}

fn contains_any(goal: &str, words: &[&str]) -> bool {
    words.iter().any(|word| goal.contains(word))
}

#[cfg(test)]
pub(crate) fn entity(id: u64, name: &str, kind: EntityKind) -> Entity {
    entity_on_layer(id, 1, name, kind)
}

fn entity_on_layer(id: u64, layer: u64, name: &str, kind: EntityKind) -> Entity {
    Entity {
        id,
        layer,
        name: name.into(),
        visible: true,
        kind,
        parameter_refs: BTreeSet::new(),
    }
}

fn mechanical_action(id: u64, layer: u64) -> PlannedAction {
    let profile = entity_on_layer(
        id,
        layer,
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
    let hole = entity_on_layer(
        id + 1,
        layer,
        "Mounting hole",
        EntityKind::Circle {
            center: Point2::new(0.0, 0.0),
            radius: 8.0,
        },
    );
    let solid = entity_on_layer(
        id + 2,
        layer,
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

fn architecture_action(id: u64, layer: u64) -> PlannedAction {
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
            entity: entity_on_layer(
                id + index as u64,
                layer,
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
        entity: entity_on_layer(
            id + 4,
            layer,
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

fn drafting_action_on_layer(id: u64, layer: u64) -> PlannedAction {
    PlannedAction {
        intent: "Create an editable drafting concept".into(),
        tool_name: "drafting.create_geometry".into(),
        detail: "Created a base rectangle and center annotation.".into(),
        transaction: CommandTransaction::new(vec![
            CadCommand::CreateEntity {
                entity: entity_on_layer(
                    id,
                    layer,
                    "Draft rectangle",
                    EntityKind::Rectangle {
                        origin: Point2::new(-50.0, -30.0),
                        width: 100.0,
                        height: 60.0,
                    },
                ),
            },
            CadCommand::CreateEntity {
                entity: entity_on_layer(
                    id + 1,
                    layer,
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
