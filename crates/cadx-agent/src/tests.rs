use std::collections::BTreeSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use cadx_core::{
    ActionFailureFeedback, ActionFailureKind, AgentRunIdentity, CadCommand, CadDocument,
    Capability, CheckResult, CheckStatus, CommandTransaction, ConstraintKind, EntityKind, Layer,
    ParameterExpression, Point2, SketchConstraint, TaskAuthority, TaskEvent, TaskStatus,
    TaskWorkspace, ValidationReport, WorkspaceError,
};

use crate::heuristic::entity;

use super::*;

#[test]
fn direct_write_task_creates_a_replayable_mechanical_commit() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Bracket"));
    let task_id = workspace.kernel().create_task(
        "Create bracket",
        "Create a mechanical mounting bracket",
        TaskAuthority::all_direct(),
    );
    let report = TaskAgent::new(HeuristicPlanner)
        .run(&mut workspace, task_id)
        .unwrap();

    assert_eq!(report.commit_ids.len(), 1);
    assert_eq!(workspace.document().entities.len(), 3);
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Completed);
    assert_eq!(
        workspace.history().restore(report.commit_ids[0]).unwrap(),
        workspace.document().clone()
    );
}

#[test]
fn heuristic_planner_uses_a_visible_unlocked_layer() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Layer-aware planner"));
    let mut concept = workspace.document().layers[&1].clone();
    concept.locked = true;
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Prepare model layers",
            CommandTransaction::new(vec![
                CadCommand::UpdateLayer { layer: concept },
                CadCommand::CreateLayer {
                    layer: Layer {
                        id: 2,
                        name: "Agent output".into(),
                        visible: true,
                        locked: false,
                        color: [90, 160, 235, 255],
                    },
                },
            ]),
            ValidationReport::default(),
        )
        .unwrap();
    let task_id = workspace.kernel().create_task(
        "Draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );

    TaskAgent::new(HeuristicPlanner)
        .run(&mut workspace, task_id)
        .unwrap();

    assert!(
        workspace
            .document()
            .entities
            .values()
            .all(|entity| entity.layer == 2)
    );
    workspace.validate_integrity().unwrap();
}

#[test]
fn review_only_task_never_bypasses_workspace_authorization() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Review"));
    let task_id = workspace.kernel().create_task(
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
    assert!(workspace.document().entities.is_empty());
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Failed);
}

#[derive(Clone, Copy)]
struct ForgedValidationPlanner;

impl TaskPlanner for ForgedValidationPlanner {
    fn plan_next(&self, observation: &AgentObservation) -> Result<PlanningDecision, AgentError> {
        let entity_id = observation.snapshot.document().next_entity_id();
        let first_constraint = observation.snapshot.document().next_constraint_id();
        Ok(PlanningDecision::Action(PlannedAction {
            intent: "Create conflicting circle constraints".into(),
            tool_name: "mechanical.constrain_radius".into(),
            detail: "Apply two incompatible driving radii.".into(),
            transaction: CommandTransaction::new(vec![
                CadCommand::CreateEntity {
                    entity: entity(
                        entity_id,
                        "Conflicted circle",
                        EntityKind::Circle {
                            center: Point2::new(0.0, 0.0),
                            radius: 1.0,
                        },
                    ),
                },
                CadCommand::CreateConstraint {
                    constraint: SketchConstraint {
                        id: first_constraint,
                        name: "Small radius".into(),
                        driving: true,
                        kind: ConstraintKind::Radius {
                            entity_id,
                            value: ParameterExpression::new("5").unwrap(),
                        },
                    },
                },
                CadCommand::CreateConstraint {
                    constraint: SketchConstraint {
                        id: first_constraint + 1,
                        name: "Large radius".into(),
                        driving: true,
                        kind: ConstraintKind::Radius {
                            entity_id,
                            value: ParameterExpression::new("10").unwrap(),
                        },
                    },
                },
            ]),
            validation: ValidationReport {
                checks: vec![CheckResult {
                    name: "Planner supplied pass".into(),
                    status: CheckStatus::Passed,
                    detail: "The planner claims the constraints are valid.".into(),
                }],
            },
        }))
    }
}

#[test]
fn planner_forged_pass_cannot_bypass_local_candidate_validation() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Local validation gate"));
    let task_id = workspace.kernel().create_task(
        "Conflicting constraints",
        "Create an impossible radius system",
        TaskAuthority::all_direct(),
    );

    let error = TaskAgent::new(ForgedValidationPlanner)
        .run(&mut workspace, task_id)
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::AutomaticRepairExhausted { attempts: 3, .. }
    ));
    let task = &workspace.tasks()[&task_id];
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(
        task.execution()
            .and_then(cadx_core::TaskExecution::last_failure)
            .map(|feedback| feedback.repair_attempt),
        Some(3)
    );
    assert_eq!(
        task.events()
            .iter()
            .filter(|event| matches!(event, TaskEvent::ActionRejected { .. }))
            .count(),
        4
    );
    assert!(matches!(
        task.events().last(),
        Some(TaskEvent::Failed { .. })
    ));
    assert_eq!(workspace.history().head(), 0);
    assert!(workspace.document().entities.is_empty());
    assert!(workspace.document().constraints.is_empty());
    workspace.validate_integrity().unwrap();
}

#[derive(Clone)]
struct TwoStepPlanner {
    calls: Arc<AtomicUsize>,
}

impl TaskPlanner for TwoStepPlanner {
    fn plan_next(&self, observation: &AgentObservation) -> Result<PlanningDecision, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let first_id = observation.snapshot.document().next_entity_id();
        let decision = match observation.action_index() {
            0 => PlanningDecision::Action(PlannedAction {
                intent: "Create first drafting entity".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Created the first editable rectangle.".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: entity(
                        first_id,
                        "First rectangle",
                        EntityKind::Rectangle {
                            origin: Point2::new(0.0, 0.0),
                            width: 20.0,
                            height: 10.0,
                        },
                    ),
                }]),
                validation: ValidationReport::default(),
            }),
            1 => PlanningDecision::Action(PlannedAction {
                intent: "Create second drafting entity".into(),
                tool_name: "drafting.create_circle".into(),
                detail: "Created the second editable circle.".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: entity(
                        first_id,
                        "Second circle",
                        EntityKind::Circle {
                            center: Point2::new(10.0, 10.0),
                            radius: 4.0,
                        },
                    ),
                }]),
                validation: ValidationReport::default(),
            }),
            _ => PlanningDecision::Complete {
                summary: "Both drafting entities were re-observed.".into(),
            },
        };
        Ok(decision)
    }
}

#[test]
fn action_budget_pauses_and_resumes_iterative_planning_at_an_action_boundary() {
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = TaskAgent::new(TwoStepPlanner {
        calls: Arc::clone(&calls),
    });
    let mut workspace = TaskWorkspace::new(CadDocument::new("Resume"));
    let task_id = workspace.kernel().create_task(
        "Two steps",
        "Create two drafting entities",
        TaskAuthority::all_direct(),
    );

    let first_report = agent
        .run_with_action_budget(&mut workspace, task_id, Some(1))
        .unwrap();
    assert_eq!(first_report.status, TaskStatus::Paused);
    assert_eq!(first_report.commit_ids.len(), 1);
    assert_eq!(workspace.document().entities.len(), 1);
    assert_eq!(
        workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .next_action_index(),
        1
    );
    workspace.validate_integrity().unwrap();

    let second_report = agent.run(&mut workspace, task_id).unwrap();
    assert_eq!(second_report.status, TaskStatus::Completed);
    assert_eq!(second_report.commit_ids.len(), 1);
    assert_eq!(workspace.document().entities.len(), 2);
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    workspace.validate_integrity().unwrap();
}

#[derive(Clone)]
struct RepairingValidationPlanner {
    feedback: Arc<Mutex<Vec<ActionFailureFeedback>>>,
}

impl TaskPlanner for RepairingValidationPlanner {
    fn plan_next(&self, observation: &AgentObservation) -> Result<PlanningDecision, AgentError> {
        if observation.action_index() > 0 {
            return Ok(PlanningDecision::Complete {
                summary: "The repaired action was observed in the model.".into(),
            });
        }
        let Some(feedback) = observation.last_failure() else {
            return ForgedValidationPlanner.plan_next(observation);
        };
        self.feedback.lock().unwrap().push(feedback.clone());
        let entity_id = observation.snapshot.document().next_entity_id();
        Ok(PlanningDecision::Action(PlannedAction {
            intent: "Create a locally valid replacement".into(),
            tool_name: "drafting.create_rectangle".into(),
            detail: "Replace the rejected constraint system with editable geometry.".into(),
            transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: entity(
                    entity_id,
                    "Repaired rectangle",
                    EntityKind::Rectangle {
                        origin: Point2::new(0.0, 0.0),
                        width: 20.0,
                        height: 10.0,
                    },
                ),
            }]),
            validation: ValidationReport::default(),
        }))
    }
}

#[test]
fn validation_failure_is_structured_repaired_and_reobserved() {
    let feedback = Arc::new(Mutex::new(Vec::new()));
    let planner = RepairingValidationPlanner {
        feedback: Arc::clone(&feedback),
    };
    let mut workspace = TaskWorkspace::new(CadDocument::new("Iterative repair"));
    let task_id = workspace.kernel().create_task(
        "Repair invalid action",
        "Create valid editable geometry",
        TaskAuthority::all_direct(),
    );

    let report = TaskAgent::new(planner)
        .run(&mut workspace, task_id)
        .unwrap();

    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(report.commit_ids.len(), 1);
    assert_eq!(workspace.document().entities.len(), 1);
    assert!(workspace.document().constraints.is_empty());
    let feedback = feedback.lock().unwrap();
    assert_eq!(feedback.len(), 1);
    assert_eq!(feedback[0].kind, ActionFailureKind::ValidationFailed);
    assert_eq!(feedback[0].repair_attempt, 1);
    let observations = workspace.tasks()[&task_id]
        .events()
        .iter()
        .filter_map(|event| match event {
            TaskEvent::Reobserved {
                revision,
                action_index,
                entity_count,
            } => Some((*revision, *action_index, *entity_count)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(observations, vec![(0, 0, 0), (0, 0, 0), (1, 1, 1)]);
    workspace.validate_integrity().unwrap();
}

#[derive(Clone)]
struct ConflictRepairPlanner {
    feedback: Arc<Mutex<Vec<ActionFailureFeedback>>>,
}

impl TaskPlanner for ConflictRepairPlanner {
    fn plan_next(&self, observation: &AgentObservation) -> Result<PlanningDecision, AgentError> {
        if observation.action_index() > 0 {
            return Ok(PlanningDecision::Complete {
                summary: "The conflict-aware replacement was re-observed.".into(),
            });
        }
        let feedback = observation
            .last_failure()
            .ok_or_else(|| AgentError::Planning("expected stale-action feedback".into()))?;
        self.feedback.lock().unwrap().push(feedback.clone());
        let mut entity = observation.snapshot.document().entities[&1].clone();
        entity.name = "Agent edit after re-observe".into();
        Ok(PlanningDecision::Action(PlannedAction {
            intent: "Update the newer human geometry".into(),
            tool_name: "drafting.update_entity".into(),
            detail: "Preserve the re-observed geometry while applying the requested semantic edit."
                .into(),
            transaction: CommandTransaction::new(vec![CadCommand::UpdateEntity { entity }]),
            validation: ValidationReport::default(),
        }))
    }
}

#[test]
fn stale_action_is_replanned_from_the_new_revision_without_losing_human_geometry() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Conflict repair"));
    let base_entity = entity(
        1,
        "Base rectangle",
        EntityKind::Rectangle {
            origin: Point2::new(0.0, 0.0),
            width: 10.0,
            height: 5.0,
        },
    );
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Create base rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: base_entity,
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let task_id = workspace.kernel().create_task(
        "Update rectangle",
        "Rename the rectangle without changing newer dimensions",
        TaskAuthority::all_direct(),
    );
    workspace
        .kernel()
        .begin_iterative_task_as(task_id, AgentRunIdentity::local("staged-test-planner"))
        .unwrap();
    let observed_revision = workspace.revision();
    workspace
        .kernel()
        .record_iterative_observation(task_id, observed_revision)
        .unwrap();
    let mut stale_entity = workspace.document().entities[&1].clone();
    stale_entity.name = "Stale agent edit".into();
    workspace
        .kernel()
        .stage_iterative_action(
            task_id,
            observed_revision,
            PlannedAction {
                intent: "Apply stale rename".into(),
                tool_name: "drafting.update_entity".into(),
                detail: "This proposal will be invalidated by a human edit.".into(),
                transaction: CommandTransaction::new(vec![CadCommand::UpdateEntity {
                    entity: stale_entity,
                }]),
                validation: ValidationReport::default(),
            },
        )
        .unwrap();
    let mut human_entity = workspace.document().entities[&1].clone();
    human_entity.name = "Human edit".into();
    human_entity.kind = EntityKind::Rectangle {
        origin: Point2::new(0.0, 0.0),
        width: 42.0,
        height: 9.0,
    };
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Human changes dimensions",
            CommandTransaction::new(vec![CadCommand::UpdateEntity {
                entity: human_entity,
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let feedback = Arc::new(Mutex::new(Vec::new()));

    let report = TaskAgent::new(ConflictRepairPlanner {
        feedback: Arc::clone(&feedback),
    })
    .run(&mut workspace, task_id)
    .unwrap();

    assert_eq!(report.commit_ids.len(), 1);
    assert_eq!(
        feedback.lock().unwrap()[0].kind,
        ActionFailureKind::StaleObservation
    );
    let final_entity = &workspace.document().entities[&1];
    assert_eq!(final_entity.name, "Agent edit after re-observe");
    assert!(matches!(
        final_entity.kind,
        EntityKind::Rectangle {
            width: 42.0,
            height: 9.0,
            ..
        }
    ));
    assert_eq!(
        workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .actions()
            .len(),
        1
    );
    workspace.validate_integrity().unwrap();
}

#[derive(Clone)]
struct RecordedRemotePlanner {
    config: ProviderConfig,
    action_count: usize,
    egress_allowed: Arc<AtomicBool>,
    plan_calls: Arc<AtomicUsize>,
    payloads: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl RemoteTaskPlanner for RecordedRemotePlanner {
    fn config(&self) -> &ProviderConfig {
        &self.config
    }

    fn authorize_egress(&self) -> Result<(), AgentError> {
        if self.egress_allowed.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(AgentError::Provider("test egress policy denied".into()))
        }
    }

    fn plan_remote(&self, context: RemoteContext) -> Result<RemotePlanningDecision, AgentError> {
        self.plan_calls.fetch_add(1, Ordering::SeqCst);
        let payload = serde_json::from_str::<serde_json::Value>(context.payload_json()).unwrap();
        self.payloads.lock().unwrap().push(payload.clone());
        let action_index = payload["execution"]["action_index"].as_u64().unwrap() as usize;
        let decision = if action_index < self.action_count {
            serde_json::json!({
                "decision": "action",
                "action": {
                    "intent": format!("Create remote rectangle {action_index}"),
                    "detail": "Create bounded editable drafting geometry.",
                    "operation": {
                        "kind": "create_rectangle",
                        "name": format!("Remote rectangle {action_index}"),
                        "origin": [action_index as f64 * 20.0, 0.0],
                        "width": 10.0,
                        "height": 5.0
                    }
                }
            })
        } else {
            serde_json::json!({
                "decision": "complete",
                "summary": "All requested remote actions are present."
            })
        };
        RemotePlanningDecision::decode_json(&decision.to_string())
    }
}

fn recorded_remote_planner(action_count: usize) -> RecordedRemotePlanner {
    RecordedRemotePlanner {
        config: ProviderConfig {
            endpoint: "https://provider.example/v1".into(),
            model: "recorded-cad-model".into(),
            enabled_capabilities: BTreeSet::from([Capability::Drafting]),
        },
        action_count,
        egress_allowed: Arc::new(AtomicBool::new(true)),
        plan_calls: Arc::new(AtomicUsize::new(0)),
        payloads: Arc::new(Mutex::new(Vec::new())),
    }
}

#[test]
fn remote_disclosure_preflight_is_side_effect_free_and_does_not_call_the_provider() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote preflight"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let before = workspace.clone();
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();

    assert_eq!(disclosure.project_id, workspace.project_id());
    assert_eq!(workspace, before);
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Queued);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn egress_revocation_rejects_a_round_before_audit_and_provider_call() {
    let planner = recorded_remote_planner(1);
    let allowed = Arc::clone(&planner.egress_allowed);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Egress pre-audit gate"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    let before = workspace.clone();
    allowed.store(false, Ordering::SeqCst);

    let error = agent
        .prepare_authorized_remote_round(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget::default(),
        )
        .unwrap_err();

    assert_eq!(
        error,
        AgentError::Provider("test egress policy denied".into())
    );
    assert_eq!(workspace, before);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn task_authorization_revocation_blocks_remote_round_before_audit_and_provider_call() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Task authorization gate"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let change_set_id = workspace.tasks()[&task_id].active_change_set_id;
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    workspace
        .kernel()
        .revoke_task_authorization(task_id, change_set_id, "Operator revoked model writes")
        .unwrap();

    let error = agent
        .prepare_authorized_remote_round(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget::default(),
        )
        .unwrap_err();

    assert_eq!(
        error,
        AgentError::Workspace(WorkspaceError::AuthorizationRevoked {
            task_id,
            change_set_id,
        })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Queued);
    assert!(
        workspace.tasks()[&task_id]
            .events()
            .iter()
            .all(|event| !matches!(event, TaskEvent::ProviderDisclosure { .. }))
    );
    workspace.validate_integrity().unwrap();
}

#[test]
fn task_authorization_revocation_blocks_an_already_audited_remote_output() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("In-flight task authorization"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let change_set_id = workspace.tasks()[&task_id].active_change_set_id;
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    let round = agent
        .prepare_authorized_remote_round(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget::default(),
        )
        .unwrap();
    workspace
        .kernel()
        .revoke_task_authorization(task_id, change_set_id, "Operator revoked model writes")
        .unwrap();

    let output = agent.plan_authorized_remote_round(round).unwrap();
    let error = agent
        .apply_remote_round_output(&mut workspace, output)
        .unwrap_err();

    assert_eq!(
        error,
        AgentError::Workspace(WorkspaceError::AuthorizationRevoked {
            task_id,
            change_set_id,
        })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(workspace.document().entities.is_empty());
    assert!(
        workspace
            .history()
            .commits
            .keys()
            .all(|commit_id| *commit_id == 0)
    );
    workspace
        .kernel()
        .fail_task(task_id, error.to_string())
        .unwrap();
    workspace.validate_integrity().unwrap();
}

#[test]
fn egress_revocation_after_audit_still_blocks_the_provider_call() {
    let planner = recorded_remote_planner(1);
    let allowed = Arc::clone(&planner.egress_allowed);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Egress send gate"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    let round = agent
        .prepare_authorized_remote_round(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget::default(),
        )
        .unwrap();
    allowed.store(false, Ordering::SeqCst);

    let error = match agent.plan_authorized_remote_round(round) {
        Ok(_) => panic!("revoked egress unexpectedly reached the provider"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        AgentError::Provider("test egress policy denied".into())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn project_grant_survives_reobservation_but_each_send_keeps_an_exact_audit() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Project grant"));
    let project_id = workspace.project_id();
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let reviewed = agent.remote_disclosure(&workspace, task_id).unwrap();
    let reviewed_hash = reviewed.payload_hash.clone();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &reviewed, 100, Some(200))
        .unwrap();

    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Unrelated human edit",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: entity(
                    1,
                    "Human line",
                    EntityKind::Line {
                        start: Point2::new(0.0, 0.0),
                        end: Point2::new(4.0, 0.0),
                    },
                ),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let current = agent
        .validate_remote_access_grant(&workspace, task_id, grant_id, 101)
        .unwrap();
    assert_ne!(current.payload_hash, reviewed_hash);
    assert_eq!(current.source_revision, 1);

    let report = agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            grant_id,
            102,
            ExecutionBudget::default(),
        )
        .unwrap();

    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(workspace.document().entities.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let event = workspace.tasks()[&task_id]
        .events()
        .iter()
        .find(|event| matches!(event, TaskEvent::ProviderDisclosure { .. }))
        .unwrap();
    let TaskEvent::ProviderDisclosure {
        project_id: event_project_id,
        grant_id: event_grant_id,
        sent_at_unix_seconds,
        payload_hash,
        source_revision,
        ..
    } = event
    else {
        unreachable!();
    };
    assert_eq!(*event_project_id, Some(project_id));
    assert_eq!(*event_grant_id, Some(grant_id));
    assert_eq!(*sent_at_unix_seconds, Some(102));
    assert_eq!(*source_revision, 1);
    assert_eq!(payload_hash, &current.payload_hash);
    workspace.validate_integrity().unwrap();
}

#[test]
fn expired_and_revoked_project_grants_block_provider_calls() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Grant lifecycle"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let expired = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, Some(110))
        .unwrap();
    assert_eq!(
        agent
            .validate_remote_access_grant(&workspace, task_id, expired, 110)
            .unwrap_err(),
        AgentError::RemoteGrantDoesNotAuthorize(expired)
    );

    let revoked = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    workspace
        .kernel()
        .revoke_remote_access_grant(revoked, 105)
        .unwrap();
    let error = agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            revoked,
            105,
            ExecutionBudget::default(),
        )
        .unwrap_err();

    assert_eq!(error, AgentError::RemoteGrantDoesNotAuthorize(revoked));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Queued);
    workspace.validate_integrity().unwrap();
}

#[test]
fn project_grant_creation_rejects_a_stale_or_forged_review() {
    let agent = TaskAgent::new(recorded_remote_planner(1));
    let mut workspace = TaskWorkspace::new(CadDocument::new("Grant review binding"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let mut disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    disclosure.payload_hash = "0".repeat(64);

    let error = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, Some(200))
        .unwrap_err();

    assert_eq!(error, AgentError::DisclosureDoesNotMatch(task_id));
    assert!(workspace.remote_access_grants().is_empty());
}

#[test]
fn remote_planner_requires_a_project_grant_before_receiving_a_plan_request() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote grant"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let error = agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            u64::MAX,
            100,
            ExecutionBudget::default(),
        )
        .unwrap_err();

    assert_eq!(error, AgentError::RemoteGrantDoesNotAuthorize(u64::MAX));
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Queued);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(workspace.document().entities.is_empty());
}

#[test]
fn project_grant_reauthorizes_each_new_revision_without_calling_the_provider() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Revision-aware grant"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Human edit after approval",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: entity(
                    1,
                    "Newer human line",
                    EntityKind::Line {
                        start: Point2::new(0.0, 0.0),
                        end: Point2::new(10.0, 0.0),
                    },
                ),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    let current = agent
        .validate_remote_access_grant(&workspace, task_id, grant_id, 101)
        .unwrap();

    assert_eq!(current.source_revision, 1);
    assert_ne!(current.payload_hash, disclosure.payload_hash);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Queued);
    assert_eq!(workspace.document().entities.len(), 1);
}

#[test]
fn project_grant_can_cover_a_later_prompt_while_disclosure_is_rebuilt() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Project-scoped grant"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    workspace.kernel().begin_task(task_id).unwrap();
    let revision = workspace.revision();
    workspace
        .kernel()
        .set_task_plan(task_id, revision, Vec::new())
        .unwrap();
    workspace.kernel().complete_task(task_id).unwrap();
    workspace
        .kernel()
        .add_prompt(
            task_id,
            "Create a drafting concept",
            TaskAuthority::all_direct(),
        )
        .unwrap();

    let current = agent
        .validate_remote_access_grant(&workspace, task_id, grant_id, 101)
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_ne!(disclosure.change_set_id, current.change_set_id);
    assert_ne!(disclosure.run_id, current.run_id);
    workspace.validate_integrity().unwrap();
}

#[test]
fn project_grant_is_bound_to_the_approved_provider_endpoint_and_model() {
    let approved_agent = TaskAgent::new(recorded_remote_planner(1));
    let mut workspace = TaskWorkspace::new(CadDocument::new("Provider-bound grant"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = approved_agent
        .remote_disclosure(&workspace, task_id)
        .unwrap();
    let grant_id = approved_agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    let mut changed_endpoint = disclosure.config.clone();
    changed_endpoint.endpoint = "https://other-provider.example/v1".into();
    let mut changed_model = disclosure.config.clone();
    changed_model.model = "different-cad-model".into();

    for config in [changed_endpoint, changed_model] {
        let planner = RecordedRemotePlanner {
            config,
            action_count: 1,
            egress_allowed: Arc::new(AtomicBool::new(true)),
            plan_calls: Arc::new(AtomicUsize::new(0)),
            payloads: Arc::new(Mutex::new(Vec::new())),
        };
        let calls = Arc::clone(&planner.plan_calls);
        let agent = TaskAgent::new(planner);
        let mut candidate = workspace.clone();

        let error = agent
            .run_remote_with_grant(
                &mut candidate,
                task_id,
                grant_id,
                101,
                ExecutionBudget::default(),
            )
            .unwrap_err();

        assert_eq!(error, AgentError::RemoteGrantDoesNotAuthorize(grant_id));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(candidate.tasks()[&task_id].status, TaskStatus::Queued);
        assert!(candidate.document().entities.is_empty());
    }
}

#[test]
fn remote_context_is_bounded_and_redacted_from_debug_output() {
    let planner = recorded_remote_planner(1);
    let agent = TaskAgent::new(planner.clone());
    let mut workspace = TaskWorkspace::new(CadDocument::new("Private project metadata"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Confidential remote task goal",
        TaskAuthority::all_direct(),
    );
    let observation = AgentObservation {
        task: workspace.task(task_id).unwrap().clone(),
        snapshot: workspace.snapshot(),
    };
    let (context, _) = crate::provider::prepare_remote_context(
        planner.config,
        RemoteContextRequest::default(),
        workspace.project_id(),
        &observation,
    )
    .unwrap();
    let debug = format!("{context:?}");
    assert!(!debug.contains("Confidential remote task goal"));
    assert!(!debug.contains("Private project metadata"));
    assert!(context.payload_bytes() <= cadx_core::MAX_REMOTE_CONTEXT_BYTES);

    let mut oversized_workspace = TaskWorkspace::new(CadDocument::new("Oversized context"));
    let oversized_task_id = oversized_workspace.kernel().create_task(
        "Oversized remote task",
        "x".repeat(cadx_core::MAX_REMOTE_CONTEXT_BYTES),
        TaskAuthority::all_direct(),
    );
    let error = agent
        .remote_disclosure(&oversized_workspace, oversized_task_id)
        .unwrap_err();
    assert!(matches!(error, AgentError::Provider(_)));
}

#[test]
fn granted_remote_plan_is_audited_and_still_uses_local_transactions() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote plan"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();

    let report = agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget::default(),
        )
        .unwrap();

    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(workspace.document().entities.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let event = workspace.tasks()[&task_id]
        .events()
        .iter()
        .find(|event| matches!(event, TaskEvent::ProviderDisclosure { .. }))
        .unwrap();
    let TaskEvent::ProviderDisclosure {
        context_schema_version,
        source_revision,
        data_categories,
        payload_bytes,
        payload_hash,
        ..
    } = event
    else {
        unreachable!();
    };
    assert_eq!(
        *context_schema_version,
        cadx_core::REMOTE_CONTEXT_SCHEMA_VERSION
    );
    assert_eq!(*source_revision, 0);
    assert!(!data_categories.is_empty());
    assert!(*payload_bytes > 0);
    assert_eq!(payload_hash.len(), 64);
    workspace.validate_integrity().unwrap();
}

#[test]
fn every_remote_decision_is_reobserved_audited_and_receives_latest_feedback() {
    let planner = recorded_remote_planner(1);
    let calls = Arc::clone(&planner.plan_calls);
    let payloads = Arc::clone(&planner.payloads);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote round contract"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create one drafting entity",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    let budget = ExecutionBudget::default();

    let first_round = agent
        .prepare_authorized_remote_round(&mut workspace, task_id, grant_id, 101, budget)
        .unwrap();
    let first_output = agent.plan_authorized_remote_round(first_round).unwrap();
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Human creates the observed entity ID",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: entity(
                    1,
                    "Human line",
                    EntityKind::Line {
                        start: Point2::new(0.0, 0.0),
                        end: Point2::new(10.0, 0.0),
                    },
                ),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let RemoteRoundApply::ActionRejected { feedback } = agent
        .apply_remote_round_output(&mut workspace, first_output)
        .unwrap()
    else {
        panic!("the stale first-round action must be rejected");
    };
    assert_eq!(feedback.kind, ActionFailureKind::StaleObservation);
    assert_eq!(feedback.observed_revision, 0);

    let report = agent
        .run_remote_with_grant(&mut workspace, task_id, grant_id, 102, budget)
        .unwrap();

    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(report.commit_ids.len(), 1);
    assert_eq!(workspace.document().entities.len(), 2);
    let audits = workspace.tasks()[&task_id]
        .events()
        .iter()
        .filter_map(|event| match event {
            TaskEvent::ProviderDisclosure {
                source_revision,
                payload_hash,
                ..
            } => Some((*source_revision, payload_hash.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(audits.len(), calls.load(Ordering::SeqCst));
    assert_eq!(
        audits
            .iter()
            .map(|(revision, _)| *revision)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        audits
            .iter()
            .map(|(_, hash)| hash)
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    let payloads = payloads.lock().unwrap();
    assert_eq!(payloads.len(), audits.len());
    assert_eq!(payloads[0]["execution"]["action_index"], 0);
    assert!(payloads[0]["execution"]["last_failure"].is_null());
    assert_eq!(payloads[1]["source_revision"], 1);
    assert_eq!(
        payloads[1]["execution"]["last_failure"]["kind"],
        "stale_observation"
    );
    assert_eq!(
        payloads[1]["execution"]["last_failure"]["repair_attempt"],
        1
    );
    assert_eq!(payloads[2]["source_revision"], 2);
    assert_eq!(payloads[2]["execution"]["action_index"], 1);
    assert!(payloads[2]["execution"]["last_failure"].is_null());
    workspace.validate_integrity().unwrap();
}

#[test]
fn missing_iterative_remote_send_audit_is_rejected_by_workspace_integrity() {
    let agent = TaskAgent::new(recorded_remote_planner(1));
    let mut workspace = TaskWorkspace::new(CadDocument::new("Missing round audit"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget::default(),
        )
        .unwrap();
    let mut serialized = serde_json::to_value(&workspace).unwrap();
    let events = serialized["tasks"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["change_sets"][0]["runs"][0]["events"]
        .as_array_mut()
        .unwrap();
    let audit_index = events
        .iter()
        .position(|event| event.get("ProviderDisclosure").is_some())
        .unwrap();
    events.remove(audit_index);
    let workspace = serde_json::from_value::<TaskWorkspace>(serialized).unwrap();

    assert!(matches!(
        workspace.validate_integrity(),
        Err(WorkspaceError::InvalidWorkspace(_))
    ));
}

#[test]
fn tampered_remote_send_audit_is_rejected_by_workspace_integrity() {
    let agent = TaskAgent::new(recorded_remote_planner(1));
    let mut workspace = TaskWorkspace::new(CadDocument::new("Audit integrity"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget::default(),
        )
        .unwrap();
    let mut serialized = serde_json::to_value(&workspace).unwrap();
    let disclosure = serialized["tasks"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .flat_map(|task| {
            task["change_sets"][0]["runs"][0]["events"]
                .as_array_mut()
                .unwrap()
        })
        .find_map(|event| event.get_mut("ProviderDisclosure"))
        .unwrap()
        .as_object_mut()
        .unwrap();
    disclosure.insert(
        "payload_hash".into(),
        serde_json::Value::String("not-a-valid-sha256".into()),
    );
    let workspace = serde_json::from_value::<TaskWorkspace>(serialized).unwrap();

    assert!(matches!(
        workspace.validate_integrity(),
        Err(WorkspaceError::InvalidWorkspace(_))
    ));
}

#[test]
fn persisted_remote_action_budget_blocks_later_rounds_without_being_widened() {
    let planner = recorded_remote_planner(2);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote budget"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();

    let first = agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            grant_id,
            101,
            ExecutionBudget {
                max_planned_actions: 1,
                max_actions_per_run: 1,
            },
        )
        .unwrap();
    assert_eq!(first.status, TaskStatus::Paused);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(workspace.document().entities.len(), 1);

    let error = agent
        .run_remote_with_grant(
            &mut workspace,
            task_id,
            grant_id,
            102,
            ExecutionBudget {
                max_planned_actions: 16,
                max_actions_per_run: 8,
            },
        )
        .unwrap_err();

    assert!(matches!(error, AgentError::Workspace(_)));
    assert_eq!(workspace.tasks()[&task_id].status, TaskStatus::Failed);
    assert_eq!(
        workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .planning_budget()
            .max_actions(),
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(workspace.document().entities.len(), 1);
}

#[test]
fn provider_endpoint_cannot_embed_a_secret() {
    let config = ProviderConfig {
        endpoint: "https://api-key@example.invalid/v1?token=secret".into(),
        model: "recorded-cad-model".into(),
        enabled_capabilities: BTreeSet::new(),
    };

    let error = config.validate().unwrap_err();

    assert!(matches!(error, AgentError::Provider(_)));
}

#[test]
fn remote_task_resumes_with_a_new_audited_decision_without_widening_its_budget() {
    let planner = recorded_remote_planner(2);
    let calls = Arc::clone(&planner.plan_calls);
    let agent = TaskAgent::new(planner);
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote resume"));
    let task_id = workspace.kernel().create_task(
        "Remote draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let disclosure = agent.remote_disclosure(&workspace, task_id).unwrap();
    let grant_id = agent
        .create_remote_access_grant(&mut workspace, task_id, &disclosure, 100, None)
        .unwrap();
    let budget = ExecutionBudget {
        max_planned_actions: 2,
        max_actions_per_run: 1,
    };

    let first = agent
        .run_remote_with_grant(&mut workspace, task_id, grant_id, 101, budget)
        .unwrap();
    assert_eq!(first.status, TaskStatus::Paused);
    assert_eq!(workspace.document().entities.len(), 1);

    let second = agent
        .run_remote_with_grant(&mut workspace, task_id, grant_id, 102, budget)
        .unwrap();
    assert_eq!(second.status, TaskStatus::Paused);
    assert_eq!(workspace.document().entities.len(), 2);
    let widened = ExecutionBudget {
        max_planned_actions: 16,
        max_actions_per_run: 1,
    };
    let third = agent
        .run_remote_with_grant(&mut workspace, task_id, grant_id, 103, widened)
        .unwrap();
    assert_eq!(third.status, TaskStatus::Completed);
    assert_eq!(
        workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .planning_budget()
            .max_actions(),
        2
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    workspace.validate_integrity().unwrap();
}
