use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ::dxf::entities::{
    Arc as DxfArc, Circle as DxfCircle, DimensionBase, Entity as DxfEntity, EntityType,
    Line as DxfLine, LwPolyline, RotatedDimension, Text as DxfText,
};
use ::dxf::enums::{AcadVersion, DimensionType, Units as DxfUnits};
use ::dxf::tables::Layer as DxfLayer;
use ::dxf::{Color, Drawing, LwPolylineVertex, Point as DxfPoint};
use cadx_core::{
    ActionFailureKind, AgentRunIdentity, CURRENT_SCHEMA_VERSION, CadCommand, CadDocument,
    Capability, CommandTransaction, ConstraintKind, Entity, EntityKind, Layer,
    MAX_REMOTE_CONTEXT_BYTES, Parameter, ParameterExpression, Point2, PointAnchor,
    REMOTE_CONTEXT_SCHEMA_VERSION, RemoteAccessGrantRequest, RemoteDataCategory, RemoteObjectScope,
    SketchConstraint, SketchPoint, SketchSegment, TaskAction, TaskAuthority, TaskEvent,
    TaskExecutionStrategy, TaskId, TaskPlanningBudget, TaskWorkspace, Units, ValidationReport,
    solve_constraints,
};
use serde_json::Value;

use crate::archive::encode_archive;
use crate::project::{ProjectManifest, WORKSPACE_ENTRY};

use super::*;

fn test_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "cadx-io-{label}-{}-{nonce}.{}",
        std::process::id(),
        PROJECT_EXTENSION
    ))
}

fn test_dxf_path(label: &str) -> PathBuf {
    test_path(label).with_extension(DXF_EXTENSION)
}

fn test_pdf_path(label: &str) -> PathBuf {
    test_path(label).with_extension(PDF_EXTENSION)
}

fn write_workspace_value(label: &str, value: &Value, format_version: u32) -> PathBuf {
    let bytes = serde_json::to_vec(value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.format_version = format_version;
    let path = test_path(label);
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();
    path
}

fn convert_task_to_format_six_layout(workspace: &mut Value, task_id: TaskId) {
    let task = workspace["tasks"][task_id.to_string()].take();
    let run = &task["change_sets"][0]["runs"][0];
    let output_commits = run["action_commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["commit_id"].clone())
        .collect::<Vec<_>>();
    workspace["tasks"][task_id.to_string()] = serde_json::json!({
        "id": task["id"],
        "title": task["title"],
        "goal": task["goal"],
        "authority": task["authority"],
        "status": task["status"],
        "events": run["events"],
        "output_commits": output_commits,
        "execution": run["execution"],
    });
    workspace
        .as_object_mut()
        .unwrap()
        .remove("next_change_set_id");
    workspace
        .as_object_mut()
        .unwrap()
        .remove("next_agent_run_id");
    for commit in workspace["history"]["commits"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        commit.as_object_mut().unwrap().remove("change_set_id");
        commit.as_object_mut().unwrap().remove("agent_run_id");
    }
}

fn sample_workspace() -> TaskWorkspace {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Persistent bracket"));
    let task = workspace.kernel().create_task(
        "Create base",
        "Create a base plate",
        TaskAuthority::all_direct(),
    );
    workspace.kernel().begin_task(task).unwrap();
    let base_revision = workspace.revision();
    workspace
        .kernel()
        .set_task_plan(
            task,
            base_revision,
            vec![TaskAction {
                intent: "Create editable base plate".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create a persistent editable base plate".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: Entity {
                        id: 1,
                        layer: 1,
                        name: "Base plate".into(),
                        visible: true,
                        kind: EntityKind::Rectangle {
                            origin: Point2::new(0.0, 0.0),
                            width: 80.0,
                            height: 50.0,
                        },
                        parameter_refs: BTreeSet::new(),
                    },
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    workspace
        .kernel()
        .apply_next_task_action(task)
        .unwrap()
        .unwrap();
    workspace.kernel().complete_task(task).unwrap();
    workspace
}

fn workspace_with_compensating_revert() -> (TaskWorkspace, TaskId, u64, u64) {
    let mut workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();
    let target_change_set_id = workspace.task(task_id).unwrap().active_change_set_id;
    let report = workspace
        .kernel()
        .revert_change_set(task_id, target_change_set_id)
        .unwrap();
    (
        workspace,
        task_id,
        target_change_set_id,
        report.compensation_change_set_id,
    )
}

fn planned_rectangle(id: u64) -> TaskAction {
    TaskAction {
        intent: format!("Create planned rectangle {id}"),
        tool_name: "drafting.create_rectangle".into(),
        detail: "Create an editable rectangle".into(),
        transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: Entity {
                id,
                layer: 1,
                name: format!("Planned rectangle {id}"),
                visible: true,
                kind: EntityKind::Rectangle {
                    origin: Point2::new((id - 1) as f64 * 20.0, 0.0),
                    width: 10.0,
                    height: 5.0,
                },
                parameter_refs: BTreeSet::new(),
            },
        }]),
        validation: ValidationReport::default(),
    }
}

fn workspace_with_persisted_plan(completed_actions: usize) -> (TaskWorkspace, TaskId) {
    assert!((1..=2).contains(&completed_actions));
    let mut workspace = TaskWorkspace::new(CadDocument::new("Persisted task plan"));
    let task_id = workspace.kernel().create_task(
        "Create two rectangles",
        "Create two editable rectangles",
        TaskAuthority::all_direct(),
    );
    workspace.kernel().begin_task(task_id).unwrap();
    let base_revision = workspace.revision();
    workspace
        .kernel()
        .set_task_plan(
            task_id,
            base_revision,
            vec![planned_rectangle(1), planned_rectangle(2)],
        )
        .unwrap();
    for _ in 0..completed_actions {
        workspace
            .kernel()
            .apply_next_task_action(task_id)
            .unwrap()
            .unwrap();
    }
    if completed_actions == 2 {
        workspace.kernel().complete_task(task_id).unwrap();
    } else {
        workspace
            .kernel()
            .pause_task(task_id, "Persisted checkpoint")
            .unwrap();
    }
    workspace.validate_integrity().unwrap();
    (workspace, task_id)
}

fn workspace_with_running_iterative_execution() -> (TaskWorkspace, TaskId) {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Iterative checkpoint"));
    let task_id = workspace.kernel().create_task(
        "Iterative task",
        "Create geometry one action at a time",
        TaskAuthority::all_direct(),
    );
    workspace
        .kernel()
        .begin_iterative_task_as(
            task_id,
            AgentRunIdentity::local("persisted-iterative-planner"),
        )
        .unwrap();
    let revision = workspace.revision();
    workspace
        .kernel()
        .record_iterative_observation(task_id, revision)
        .unwrap();
    workspace
        .kernel()
        .stage_iterative_action(task_id, revision, planned_rectangle(1))
        .unwrap();
    workspace
        .kernel()
        .apply_next_task_action(task_id)
        .unwrap()
        .unwrap();
    workspace.validate_integrity().unwrap();
    (workspace, task_id)
}

fn workspace_with_paused_iterative_execution() -> (TaskWorkspace, TaskId) {
    let (mut workspace, task_id) = workspace_with_running_iterative_execution();
    workspace
        .kernel()
        .pause_task(task_id, "Persist iterative checkpoint")
        .unwrap();
    workspace.validate_integrity().unwrap();
    (workspace, task_id)
}

fn workspace_with_iterative_failure_feedback() -> (TaskWorkspace, TaskId) {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Iterative feedback"));
    let task_id = workspace.kernel().create_task(
        "Repair rejected action",
        "Recover from a locally rejected tool call",
        TaskAuthority::all_direct(),
    );
    workspace
        .kernel()
        .begin_iterative_task_as(task_id, AgentRunIdentity::local("feedback-planner"))
        .unwrap();
    let revision = workspace.revision();
    workspace
        .kernel()
        .record_iterative_observation(task_id, revision)
        .unwrap();
    let rejected = TaskAction {
        intent: "Delete a missing entity".into(),
        tool_name: "drafting.delete_entity".into(),
        detail: "Exercise persisted repair feedback.".into(),
        transaction: CommandTransaction::new(vec![CadCommand::DeleteEntity { id: 99 }]),
        validation: ValidationReport::default(),
    };
    let error = workspace
        .kernel()
        .stage_iterative_action(task_id, revision, rejected.clone())
        .unwrap_err();
    assert!(
        workspace
            .kernel()
            .reject_iterative_action(
                task_id,
                revision,
                &rejected,
                ActionFailureKind::ToolRejected,
                error.to_string(),
            )
            .unwrap()
    );
    workspace
        .kernel()
        .pause_task(task_id, "Persist repair feedback")
        .unwrap();
    workspace.validate_integrity().unwrap();
    (workspace, task_id)
}

fn execution_json_mut(
    workspace: &mut Value,
    task_id: TaskId,
) -> &mut serde_json::Map<String, Value> {
    workspace["tasks"][task_id.to_string()]["change_sets"][0]["runs"][0]["execution"]
        .as_object_mut()
        .unwrap()
}

fn workspace_with_remote_audit() -> TaskWorkspace {
    let mut workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();
    let source_revision = workspace.revision();
    workspace
        .kernel()
        .record_event(
            task_id,
            TaskEvent::ProviderDisclosure {
                endpoint: "https://provider.example/v1".into(),
                model: "industrial-cad-planner".into(),
                project_id: None,
                grant_id: None,
                sent_at_unix_seconds: None,
                requested_capabilities: BTreeSet::from([
                    Capability::Drafting,
                    Capability::Mechanical,
                ]),
                selected_entity_ids: vec![1],
                includes_source_files: false,
                payload_summary: "Bounded remote planning context".into(),
                context_schema_version: REMOTE_CONTEXT_SCHEMA_VERSION,
                source_revision,
                data_categories: BTreeSet::from([
                    RemoteDataCategory::TaskGoal,
                    RemoteDataCategory::DocumentMetadata,
                    RemoteDataCategory::DocumentStatistics,
                    RemoteDataCategory::SelectionIdentifiers,
                    RemoteDataCategory::GrantedCapabilities,
                    RemoteDataCategory::ExecutionState,
                ]),
                payload_bytes: 512,
                payload_hash: "ab".repeat(32),
            },
        )
        .unwrap();
    workspace.validate_integrity().unwrap();
    workspace
}

fn workspace_with_project_grant_audit() -> (TaskWorkspace, u64) {
    let mut workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();
    let source_revision = workspace.revision();
    let project_id = workspace.project_id();
    let data_categories = BTreeSet::from([
        RemoteDataCategory::TaskGoal,
        RemoteDataCategory::DocumentMetadata,
        RemoteDataCategory::DocumentStatistics,
        RemoteDataCategory::SelectionIdentifiers,
        RemoteDataCategory::GrantedCapabilities,
        RemoteDataCategory::ExecutionState,
    ]);
    let capabilities = BTreeSet::from([Capability::Drafting, Capability::Mechanical]);
    let grant_id = workspace
        .kernel()
        .create_remote_access_grant(RemoteAccessGrantRequest {
            endpoint: "https://provider.example/v1".into(),
            model: "industrial-cad-planner".into(),
            allowed_data_categories: data_categories.clone(),
            allowed_capabilities: capabilities.clone(),
            object_scope: RemoteObjectScope::from_selected_entities([1]),
            max_payload_bytes: MAX_REMOTE_CONTEXT_BYTES,
            granted_at_unix_seconds: 100,
            expires_at_unix_seconds: Some(200),
        })
        .unwrap();
    workspace
        .kernel()
        .record_event(
            task_id,
            TaskEvent::ProviderDisclosure {
                endpoint: "https://provider.example/v1".into(),
                model: "industrial-cad-planner".into(),
                project_id: Some(project_id),
                grant_id: Some(grant_id),
                sent_at_unix_seconds: Some(150),
                requested_capabilities: capabilities,
                selected_entity_ids: vec![1],
                includes_source_files: false,
                payload_summary: "Project-granted remote planning context".into(),
                context_schema_version: REMOTE_CONTEXT_SCHEMA_VERSION,
                source_revision,
                data_categories,
                payload_bytes: 512,
                payload_hash: "cd".repeat(32),
            },
        )
        .unwrap();
    workspace.validate_integrity().unwrap();
    (workspace, grant_id)
}

fn remote_audit_event(workspace: &TaskWorkspace) -> &TaskEvent {
    workspace
        .tasks()
        .values()
        .flat_map(|task| task.events())
        .find(|event| matches!(event, TaskEvent::ProviderDisclosure { .. }))
        .unwrap()
}

fn remote_audit_json_mut(workspace: &mut Value) -> &mut serde_json::Map<String, Value> {
    workspace["tasks"]
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
        .unwrap()
}

#[derive(Clone, Copy)]
enum RemoteAuditCorruption {
    MissingRevision,
    OversizedPayload,
    InvalidHash,
}

impl RemoteAuditCorruption {
    const ALL: [Self; 3] = [
        Self::MissingRevision,
        Self::OversizedPayload,
        Self::InvalidHash,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::MissingRevision => "remote-audit-revision",
            Self::OversizedPayload => "remote-audit-bytes",
            Self::InvalidHash => "remote-audit-hash",
        }
    }

    fn apply(self, audit: &mut serde_json::Map<String, Value>) {
        match self {
            Self::MissingRevision => {
                audit.insert("source_revision".into(), Value::from(u64::MAX));
            }
            Self::OversizedPayload => {
                audit.insert(
                    "payload_bytes".into(),
                    Value::from((MAX_REMOTE_CONTEXT_BYTES + 1) as u64),
                );
            }
            Self::InvalidHash => {
                audit.insert(
                    "payload_hash".into(),
                    Value::String("not-a-sha256-digest".into()),
                );
            }
        }
    }
}

#[test]
fn native_project_round_trip_preserves_editable_workspace_and_history() {
    let workspace = sample_workspace();
    let path = test_path("round-trip");

    let report = save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert_eq!(report.format_version, CURRENT_PROJECT_FORMAT_VERSION);
    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_round_trip_preserves_revision_bound_task_execution() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let path = test_path("revision-bound-plan-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    let execution = loaded.workspace.task(task_id).unwrap().execution().unwrap();
    assert_eq!(execution.base_revision(), Some(0));
    assert_eq!(execution.expected_revision(), Some(1));
    assert_eq!(execution.next_action_index(), 1);
    assert!(execution.next_action_preparation().is_some());
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_seven_workspace_migrates_additive_compensation_fields() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    for task in value["tasks"].as_object_mut().unwrap().values_mut() {
        for change_set in task["change_sets"].as_array_mut().unwrap() {
            change_set.as_object_mut().unwrap().remove("compensation");
            change_set.as_object_mut().unwrap().remove("reverted_by");
        }
    }
    for commit in value["history"]["commits"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        commit["diff"]
            .as_object_mut()
            .unwrap()
            .remove("deleted_parameters");
    }
    let path = write_workspace_value("format-seven-compensation-fields", &value, 7);

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.manifest.format_version,
        CURRENT_PROJECT_FORMAT_VERSION
    );
    assert_eq!(loaded.workspace, workspace);
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_eight_workspace_migrates_explicit_batch_execution_strategy() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id).remove("strategy");
    let path = write_workspace_value("format-eight-execution-strategy", &value, 8);

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    assert!(matches!(
        loaded.workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .strategy(),
        TaskExecutionStrategy::Batch
    ));
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_nine_workspace_migrates_project_identity_and_empty_remote_policy() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value.as_object_mut().unwrap().remove("project_id");
    value
        .as_object_mut()
        .unwrap()
        .remove("remote_access_policy");
    let path = write_workspace_value("format-nine-remote-policy", &value, 9);

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(loaded.workspace.document(), workspace.document());
    assert_eq!(loaded.workspace.history(), workspace.history());
    assert_eq!(loaded.workspace.tasks(), workspace.tasks());
    assert!(loaded.workspace.remote_access_grants().is_empty());
    assert!(loaded.workspace.remote_policy_events().is_empty());
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_ten_workspace_migrates_explicit_planning_budgets() {
    let (workspace, task_id) = workspace_with_paused_iterative_execution();
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id).remove("planning_budget");
    let path = write_workspace_value("format-ten-planning-budget", &value, 10);

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    assert_eq!(
        loaded.workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .planning_budget(),
        TaskPlanningBudget::iterative(cadx_core::MAX_ITERATIVE_ACTIONS_PER_RUN).unwrap()
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_eleven_workspace_migrates_empty_task_authorization_ledgers() {
    let workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["tasks"][task_id.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("authorization_revocations");
    let path = write_workspace_value("format-eleven-task-authorization", &value, 11);

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    assert!(
        loaded.workspace.tasks()[&task_id]
            .authorization_revocations()
            .is_empty()
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_eleven_workspace_rejects_current_task_authorization_events() {
    let mut workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();
    let change_set_id = workspace.tasks()[&task_id].active_change_set_id;
    workspace
        .kernel()
        .revoke_task_authorization(task_id, change_set_id, "Current-format revocation")
        .unwrap();
    let value = serde_json::to_value(&workspace).unwrap();
    let path = write_workspace_value("format-eleven-rejects-v12-ledger", &value, 11);

    assert!(matches!(
        load_workspace(&path).unwrap_err(),
        ProjectError::Workspace(_)
    ));
    fs::remove_file(path).unwrap();
}

#[test]
fn native_project_round_trip_preserves_task_authorization_revocation() {
    let mut workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();
    let change_set_id = workspace.tasks()[&task_id].active_change_set_id;
    workspace
        .kernel()
        .revoke_task_authorization(task_id, change_set_id, "Local operator revoked writes")
        .unwrap();
    let path = test_path("task-authorization-revocation-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    assert_eq!(
        loaded.workspace.tasks()[&task_id].authorization_revocations()[0].committed_action_count,
        1
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_missing_or_tampered_task_authorization_ledger() {
    let mut workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();
    let change_set_id = workspace.tasks()[&task_id].active_change_set_id;
    workspace
        .kernel()
        .revoke_task_authorization(task_id, change_set_id, "Local operator revoked writes")
        .unwrap();
    let mut corruptions = Vec::new();

    let mut missing = serde_json::to_value(&workspace).unwrap();
    missing["tasks"][task_id.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("authorization_revocations");
    corruptions.push(("missing-task-authorization-ledger", missing));

    let mut wrong_count = serde_json::to_value(&workspace).unwrap();
    wrong_count["tasks"][task_id.to_string()]["authorization_revocations"][0]["committed_action_count"] =
        Value::from(0);
    corruptions.push(("wrong-task-authorization-count", wrong_count));

    let mut missing_revision = serde_json::to_value(&workspace).unwrap();
    missing_revision["tasks"][task_id.to_string()]["authorization_revocations"][0]["revoked_at_revision"] =
        Value::from(999);
    corruptions.push(("missing-task-authorization-revision", missing_revision));

    let mut wrong_change_set = serde_json::to_value(&workspace).unwrap();
    wrong_change_set["tasks"][task_id.to_string()]["authorization_revocations"][0]["change_set_id"] =
        Value::from(999);
    corruptions.push(("wrong-task-authorization-change-set", wrong_change_set));

    let mut duplicate = serde_json::to_value(&workspace).unwrap();
    let event = duplicate["tasks"][task_id.to_string()]["authorization_revocations"][0].clone();
    duplicate["tasks"][task_id.to_string()]["authorization_revocations"]
        .as_array_mut()
        .unwrap()
        .push(event);
    corruptions.push(("duplicate-task-authorization-revocation", duplicate));

    for (label, value) in corruptions {
        let corrupted = serde_json::from_value::<TaskWorkspace>(value.clone()).unwrap();
        let save_path = test_path(&format!("save-{label}"));
        assert!(matches!(
            save_workspace(&corrupted, &save_path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        assert!(!save_path.exists());

        let load_path = write_workspace_value(label, &value, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(matches!(
            load_workspace(&load_path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        fs::remove_file(load_path).unwrap();
    }
}

#[test]
fn native_project_round_trip_preserves_paused_iterative_execution() {
    let (workspace, task_id) = workspace_with_paused_iterative_execution();
    let path = test_path("iterative-execution-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    let execution = loaded.workspace.tasks()[&task_id].execution().unwrap();
    assert!(execution.is_iterative());
    assert!(execution.is_awaiting_planner());
    assert_eq!(execution.next_action_index(), 1);
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn interrupted_iterative_execution_recovers_at_the_awaiting_planner_checkpoint() {
    let (workspace, task_id) = workspace_with_running_iterative_execution();
    let run_id = workspace.tasks()[&task_id].active_run().unwrap().id;
    let path = test_path("interrupted-iterative-execution");
    save_workspace(&workspace, &path).unwrap();

    let mut loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    let task = &loaded.workspace.tasks()[&task_id];
    assert_eq!(task.status, cadx_core::TaskStatus::Paused);
    assert_eq!(task.active_run().unwrap().id, run_id);
    assert!(task.execution().unwrap().is_awaiting_planner());
    loaded.workspace.kernel().resume_task(task_id).unwrap();
    assert_eq!(
        loaded.workspace.tasks()[&task_id].status,
        cadx_core::TaskStatus::Running
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn native_project_round_trip_preserves_iterative_failure_feedback() {
    let (workspace, task_id) = workspace_with_iterative_failure_feedback();
    let path = test_path("iterative-feedback-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert_eq!(loaded.workspace, workspace);
    let feedback = loaded.workspace.tasks()[&task_id]
        .execution()
        .unwrap()
        .last_failure()
        .unwrap();
    assert_eq!(feedback.kind, ActionFailureKind::ToolRejected);
    assert_eq!(feedback.repair_attempt, 1);
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn native_project_round_trip_preserves_compensating_revert_audit() {
    let (workspace, task_id, target_change_set_id, compensation_change_set_id) =
        workspace_with_compensating_revert();
    let path = test_path("compensating-revert-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    let task = loaded.workspace.task(task_id).unwrap();
    let target = task
        .change_sets
        .iter()
        .find(|change_set| change_set.id == target_change_set_id)
        .unwrap();
    let compensation = task
        .change_sets
        .iter()
        .find(|change_set| change_set.id == compensation_change_set_id)
        .unwrap();
    assert_eq!(target.reverted_by, Some(compensation_change_set_id));
    assert_eq!(
        compensation
            .compensation
            .as_ref()
            .unwrap()
            .target_change_set_id,
        target_change_set_id
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_format_rejects_tampered_compensation_audit() {
    type CompensationCorruption = (&'static str, Box<dyn Fn(&mut Value)>);

    let (workspace, task_id, _target_change_set_id, _compensation_change_set_id) =
        workspace_with_compensating_revert();
    let corruptions: [CompensationCorruption; 4] = [
        (
            "compensation-back-reference",
            Box::new(move |value| {
                value["tasks"][task_id.to_string()]["change_sets"][0]["reverted_by"] =
                    Value::from(999);
            }),
        ),
        (
            "compensation-request-revision",
            Box::new(move |value| {
                value["tasks"][task_id.to_string()]["change_sets"][1]["compensation"]["requested_at_revision"] =
                    Value::from(999);
            }),
        ),
        (
            "compensation-object-list",
            Box::new(move |value| {
                value["tasks"][task_id.to_string()]["change_sets"][1]["compensation"]["reverted_objects"] =
                    Value::Array(Vec::new());
            }),
        ),
        (
            "compensation-result-status",
            Box::new(move |value| {
                value["tasks"][task_id.to_string()]["change_sets"][0]["status"] =
                    Value::String("reverted_with_conflicts".into());
            }),
        ),
    ];

    for (label, corrupt) in corruptions {
        let mut value = serde_json::to_value(&workspace).unwrap();
        corrupt(&mut value);
        let path = write_workspace_value(label, &value, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(load_workspace(&path).is_err(), "accepted {label}");
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn format_six_task_layout_migrates_to_prompt_change_sets_and_agent_runs() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Legacy task hierarchy"));
    let task_id = workspace.kernel().create_task(
        "Create bracket",
        "Create the first bracket feature",
        TaskAuthority::all_direct(),
    );
    let mut value = serde_json::to_value(&workspace).unwrap();
    convert_task_to_format_six_layout(&mut value, task_id);
    let path = write_workspace_value("format-six-task-hierarchy", &value, 6);

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.manifest.format_version,
        CURRENT_PROJECT_FORMAT_VERSION
    );
    let task = loaded.workspace.task(task_id).unwrap();
    assert_eq!(task.change_sets.len(), 1);
    assert_eq!(task.active_change_set().unwrap().prompt, task.goal);
    assert_eq!(task.active_change_set().unwrap().runs.len(), 1);
    assert_eq!(
        task.active_change_set()
            .unwrap()
            .active_run()
            .unwrap()
            .attempt,
        1
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_format_rejects_the_legacy_task_execution_layout() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Reject legacy task layout"));
    let task_id = workspace.kernel().create_task(
        "Create bracket",
        "Create a bracket",
        TaskAuthority::all_direct(),
    );
    let mut value = serde_json::to_value(&workspace).unwrap();
    convert_task_to_format_six_layout(&mut value, task_id);
    let path = write_workspace_value(
        "current-rejects-legacy-task-layout",
        &value,
        CURRENT_PROJECT_FORMAT_VERSION,
    );

    assert!(load_workspace(&path).is_err());
    fs::remove_file(path).unwrap();
}

#[test]
fn current_format_rejects_missing_or_wrong_change_set_and_run_bindings() {
    let workspace = sample_workspace();
    let task_id = *workspace.tasks().keys().next().unwrap();

    let mut missing_change_set = serde_json::to_value(&workspace).unwrap();
    missing_change_set["tasks"][task_id.to_string()]["active_change_set_id"] = Value::from(999);
    let path = write_workspace_value(
        "current-missing-active-change-set",
        &missing_change_set,
        CURRENT_PROJECT_FORMAT_VERSION,
    );
    assert!(load_workspace(&path).is_err());
    fs::remove_file(path).unwrap();

    let mut wrong_run_binding = serde_json::to_value(&workspace).unwrap();
    wrong_run_binding["tasks"][task_id.to_string()]["change_sets"][0]["runs"][0]["change_set_id"] =
        Value::from(999);
    let path = write_workspace_value(
        "current-wrong-run-binding",
        &wrong_run_binding,
        CURRENT_PROJECT_FORMAT_VERSION,
    );
    assert!(load_workspace(&path).is_err());
    fs::remove_file(path).unwrap();

    let mut wrong_commit_binding = serde_json::to_value(&workspace).unwrap();
    wrong_commit_binding["history"]["commits"]["1"]["agent_run_id"] = Value::from(999);
    let path = write_workspace_value(
        "current-wrong-commit-run-binding",
        &wrong_commit_binding,
        CURRENT_PROJECT_FORMAT_VERSION,
    );
    assert!(load_workspace(&path).is_err());
    fs::remove_file(path).unwrap();

    let mut forged_authorization = serde_json::to_value(&workspace).unwrap();
    forged_authorization["tasks"][task_id.to_string()]["change_sets"][0]["authorization"] =
        Value::String("ReviewOnly".into());
    let path = write_workspace_value(
        "current-forged-change-set-authorization",
        &forged_authorization,
        CURRENT_PROJECT_FORMAT_VERSION,
    );
    assert!(load_workspace(&path).is_err());
    fs::remove_file(path).unwrap();
}

#[test]
fn current_format_rejects_duplicate_change_set_and_run_ids() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Duplicate hierarchy ids"));
    let first =
        workspace
            .kernel()
            .create_task("First", "First prompt", TaskAuthority::all_direct());
    let second =
        workspace
            .kernel()
            .create_task("Second", "Second prompt", TaskAuthority::all_direct());
    let first_change_set = workspace
        .task(first)
        .unwrap()
        .active_change_set()
        .unwrap()
        .id;
    let first_run = workspace.task(first).unwrap().active_run().unwrap().id;
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["tasks"][second.to_string()]["active_change_set_id"] = Value::from(first_change_set);
    value["tasks"][second.to_string()]["change_sets"][0]["id"] = Value::from(first_change_set);
    value["tasks"][second.to_string()]["change_sets"][0]["active_run_id"] = Value::from(first_run);
    value["tasks"][second.to_string()]["change_sets"][0]["runs"][0]["id"] = Value::from(first_run);
    value["tasks"][second.to_string()]["change_sets"][0]["runs"][0]["change_set_id"] =
        Value::from(first_change_set);
    let path = write_workspace_value(
        "current-duplicate-hierarchy-ids",
        &value,
        CURRENT_PROJECT_FORMAT_VERSION,
    );

    assert!(load_workspace(&path).is_err());
    fs::remove_file(path).unwrap();
}

#[test]
fn format_four_task_execution_infers_revision_preconditions_during_migration() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    let execution = execution_json_mut(&mut value, task_id);
    execution.remove("base_revision");
    execution.remove("expected_revision");
    execution.remove("next_action_preparation");
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.format_version = 4;
    let path = test_path("format-four-execution-revisions");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.manifest.format_version,
        CURRENT_PROJECT_FORMAT_VERSION
    );
    let execution = loaded.workspace.task(task_id).unwrap().execution().unwrap();
    assert_eq!(execution.base_revision(), Some(0));
    assert_eq!(execution.expected_revision(), Some(1));
    assert_eq!(execution.next_action_index(), 1);
    assert!(execution.next_action_preparation().is_some());
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_five_task_execution_infers_object_preconditions_during_migration() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id).remove("next_action_preparation");
    for (id, commit) in value["history"]["commits"].as_object_mut().unwrap() {
        if id != "0" {
            commit.as_object_mut().unwrap().remove("preparation");
        }
    }
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.format_version = 5;
    let path = test_path("format-five-object-preconditions");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.manifest.format_version,
        CURRENT_PROJECT_FORMAT_VERSION
    );
    let execution = loaded.workspace.task(task_id).unwrap().execution().unwrap();
    assert_eq!(execution.expected_revision(), Some(1));
    assert!(execution.next_action_preparation().is_some());
    assert!(
        loaded
            .workspace
            .history()
            .commits
            .values()
            .filter(|commit| commit.id != 0)
            .all(|commit| commit.preparation().is_some())
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_missing_execution_revision_preconditions() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id).remove("base_revision");
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("current-missing-execution-revision");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_missing_action_preparation() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id).remove("next_action_preparation");
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("current-missing-action-preparation");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_tampered_object_preconditions() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    let preconditions =
        execution_json_mut(&mut value, task_id)["next_action_preparation"]["preconditions"]
            .as_array_mut()
            .unwrap();
    preconditions[0]["last_modified_revision"] = Value::from(999);
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("current-tampered-object-preconditions");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_missing_commit_preparation() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["history"]["commits"]["1"]
        .as_object_mut()
        .unwrap()
        .remove("preparation");
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("current-missing-commit-preparation");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_tampered_commit_idempotency_key() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["history"]["commits"]["1"]["preparation"]["idempotency_key"][0] = Value::from(255);
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("current-tampered-commit-idempotency");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_tampered_execution_checkpoint_revision() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id).insert("expected_revision".into(), Value::from(0));
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("tampered-execution-revision");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_missing_execution_strategy() {
    let (workspace, task_id) = workspace_with_persisted_plan(1);
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id).remove("strategy");
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("missing-execution-strategy");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_missing_project_identity_or_policy_ledger() {
    for field in ["project_id", "remote_access_policy"] {
        let workspace = sample_workspace();
        let mut value = serde_json::to_value(&workspace).unwrap();
        value.as_object_mut().unwrap().remove(field);
        let corrupted = serde_json::from_value::<TaskWorkspace>(value.clone()).unwrap();
        let path = test_path(&format!("missing-{field}"));

        assert!(matches!(
            save_workspace(&corrupted, &path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        assert!(!path.exists());
        let path = write_workspace_value(
            &format!("load-missing-{field}"),
            &value,
            CURRENT_PROJECT_FORMAT_VERSION,
        );
        assert!(matches!(
            load_workspace(&path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn current_project_format_rejects_missing_or_tampered_planning_budget() {
    let (workspace, task_id) = workspace_with_paused_iterative_execution();
    let mut corruptions = Vec::new();

    let mut missing = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut missing, task_id).remove("planning_budget");
    corruptions.push(("missing-planning-budget", missing));

    let mut zero_actions = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut zero_actions, task_id)["planning_budget"]["max_actions"] =
        Value::from(0);
    corruptions.push(("zero-planning-action-budget", zero_actions));

    let mut widened_decisions = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut widened_decisions, task_id)["planning_budget"]["max_decisions"] =
        Value::from(usize::MAX);
    corruptions.push(("widened-planning-decision-budget", widened_decisions));

    for (label, value) in corruptions {
        let corrupted = serde_json::from_value::<TaskWorkspace>(value.clone()).unwrap();
        let save_path = test_path(&format!("save-{label}"));
        assert!(matches!(
            save_workspace(&corrupted, &save_path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        assert!(!save_path.exists());

        let load_path = write_workspace_value(label, &value, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(matches!(
            load_workspace(&load_path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        fs::remove_file(load_path).unwrap();
    }
}

#[test]
fn current_project_format_rejects_tampered_iterative_feedback() {
    let (workspace, task_id) = workspace_with_iterative_failure_feedback();
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id)["strategy"]["last_failure"]["repair_attempt"] =
        Value::from(0);
    let corrupted = serde_json::from_value::<TaskWorkspace>(value.clone()).unwrap();
    let path = test_path("tampered-iterative-feedback");

    let save_error = save_workspace(&corrupted, &path).unwrap_err();
    assert!(matches!(save_error, ProjectError::Workspace(_)));
    assert!(!path.exists());

    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();
    let load_error = load_workspace(&path).unwrap_err();
    assert!(matches!(load_error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_forged_iterative_completion() {
    let (workspace, task_id) = workspace_with_paused_iterative_execution();
    let mut value = serde_json::to_value(&workspace).unwrap();
    execution_json_mut(&mut value, task_id)["strategy"]["planner_complete"] = Value::Bool(true);
    let corrupted = serde_json::from_value::<TaskWorkspace>(value.clone()).unwrap();
    let path = test_path("forged-iterative-completion");

    assert!(matches!(
        save_workspace(&corrupted, &path).unwrap_err(),
        ProjectError::Workspace(_)
    ));
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();
    assert!(matches!(
        load_workspace(&path).unwrap_err(),
        ProjectError::Workspace(_)
    ));
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_noncontiguous_task_output_chain() {
    let (mut workspace, task_id) = workspace_with_persisted_plan(2);
    let expected_revision = workspace.revision();
    let user_commit = workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Create unrelated user rectangle",
            planned_rectangle(3).transaction,
            ValidationReport::default(),
        )
        .unwrap();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["tasks"][task_id.to_string()]["change_sets"][0]["runs"][0]["action_commits"][1]["commit_id"] =
        Value::from(user_commit);
    execution_json_mut(&mut value, task_id)
        .insert("expected_revision".into(), Value::from(user_commit));
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("noncontiguous-task-output");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn native_project_round_trip_preserves_hash_bound_remote_send_audit() {
    let workspace = workspace_with_remote_audit();
    let path = test_path("remote-audit-round-trip");

    let report = save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert_eq!(report.format_version, CURRENT_PROJECT_FORMAT_VERSION);
    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    let event = loaded
        .workspace
        .tasks()
        .values()
        .flat_map(|task| task.events())
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
    assert_eq!(*context_schema_version, REMOTE_CONTEXT_SCHEMA_VERSION);
    assert_eq!(*source_revision, workspace.history().head());
    assert_eq!(
        data_categories,
        &BTreeSet::from([
            RemoteDataCategory::TaskGoal,
            RemoteDataCategory::DocumentMetadata,
            RemoteDataCategory::DocumentStatistics,
            RemoteDataCategory::SelectionIdentifiers,
            RemoteDataCategory::GrantedCapabilities,
            RemoteDataCategory::ExecutionState,
        ])
    );
    assert_eq!(*payload_bytes, 512);
    assert_eq!(payload_hash, &"ab".repeat(32));
    fs::remove_file(path).unwrap();
}

#[test]
fn native_project_round_trip_preserves_project_grant_and_bound_send_audit() {
    let (workspace, grant_id) = workspace_with_project_grant_audit();
    let project_id = workspace.project_id();
    let path = test_path("project-grant-audit-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    assert_eq!(loaded.workspace.project_id(), project_id);
    assert_eq!(loaded.workspace.remote_access_grants().len(), 1);
    assert_eq!(loaded.workspace.remote_policy_events().len(), 1);
    let TaskEvent::ProviderDisclosure {
        project_id: event_project_id,
        grant_id: event_grant_id,
        sent_at_unix_seconds,
        ..
    } = remote_audit_event(&loaded.workspace)
    else {
        unreachable!();
    };
    assert_eq!(*event_project_id, Some(project_id));
    assert_eq!(*event_grant_id, Some(grant_id));
    assert_eq!(*sent_at_unix_seconds, Some(150));
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_tampered_grant_ledger_and_audit_binding() {
    for (label, tamper) in [
        ("grant-state", 0_u8),
        ("grant-event", 1_u8),
        ("grant-audit", 2_u8),
    ] {
        let (workspace, grant_id) = workspace_with_project_grant_audit();
        let mut value = serde_json::to_value(&workspace).unwrap();
        match tamper {
            0 => {
                value["remote_access_policy"]["grants"][grant_id.to_string()]["revoked_at_unix_seconds"] =
                    Value::from(160);
            }
            1 => {
                value["remote_access_policy"]["events"][0]["grant"]["endpoint"] =
                    Value::String("https://other.example/v1".into());
            }
            2 => {
                remote_audit_json_mut(&mut value)
                    .insert("grant_id".into(), Value::from(grant_id + 1));
            }
            _ => unreachable!(),
        }
        let corrupted = serde_json::from_value::<TaskWorkspace>(value.clone()).unwrap();
        let path = test_path(label);
        assert!(matches!(
            save_workspace(&corrupted, &path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        let path = write_workspace_value(label, &value, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(matches!(
            load_workspace(&path).unwrap_err(),
            ProjectError::Workspace(_)
        ));
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn format_three_remote_audit_without_hash_binding_is_migrated_as_legacy() {
    let workspace = workspace_with_remote_audit();
    let mut value = serde_json::to_value(workspace).unwrap();
    let audit = remote_audit_json_mut(&mut value);
    for field in [
        "context_schema_version",
        "source_revision",
        "data_categories",
        "payload_bytes",
        "payload_hash",
    ] {
        audit.remove(field);
    }
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.format_version = 3;
    let path = test_path("format-three-remote-audit");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.manifest.format_version,
        CURRENT_PROJECT_FORMAT_VERSION
    );
    let TaskEvent::ProviderDisclosure {
        context_schema_version,
        source_revision,
        data_categories,
        payload_bytes,
        payload_hash,
        ..
    } = remote_audit_event(&loaded.workspace)
    else {
        unreachable!();
    };
    assert_eq!(*context_schema_version, 0);
    assert_eq!(*source_revision, 0);
    assert!(data_categories.is_empty());
    assert_eq!(*payload_bytes, 0);
    assert!(payload_hash.is_empty());
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_invalid_remote_audit_on_save_and_load() {
    for corruption in RemoteAuditCorruption::ALL {
        let workspace = workspace_with_remote_audit();
        let mut value = serde_json::to_value(&workspace).unwrap();
        corruption.apply(remote_audit_json_mut(&mut value));
        let workspace = serde_json::from_value::<TaskWorkspace>(value.clone()).unwrap();
        let path = test_path(corruption.label());

        let save_error = save_workspace(&workspace, &path).unwrap_err();
        assert!(matches!(save_error, ProjectError::Workspace(_)));
        assert!(!path.exists());

        let bytes = serde_json::to_vec(&value).unwrap();
        let manifest = ProjectManifest::current(&bytes);
        fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();
        let load_error = load_workspace(&path).unwrap_err();
        assert!(matches!(load_error, ProjectError::Workspace(_)));
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn native_project_round_trip_preserves_exact_arc_semantics_and_history() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Arc persistence"));
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Create exact arc",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: Entity {
                    id: 1,
                    layer: 1,
                    name: "Arc".into(),
                    visible: true,
                    kind: EntityKind::Arc {
                        center: Point2::new(12.0, -4.0),
                        radius: 8.0,
                        start_angle: 5.8,
                        sweep_angle: 0.7,
                    },
                    parameter_refs: BTreeSet::new(),
                },
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let path = test_path("arc-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert_eq!(loaded.workspace, workspace);
    assert_eq!(
        loaded
            .workspace
            .history()
            .restore(loaded.workspace.history().head())
            .unwrap(),
        loaded.workspace.document().clone()
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn native_project_round_trip_preserves_exact_dimension_semantics_and_history() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Dimension persistence"));
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Create exact aligned dimension",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: Entity {
                    id: 1,
                    layer: 1,
                    name: "Overall width".into(),
                    visible: true,
                    kind: EntityKind::AlignedDimension {
                        start: Point2::new(1.0, 2.0),
                        end: Point2::new(41.0, 2.0),
                        offset: -9.5,
                        text_override: Some("TYP <>".into()),
                    },
                    parameter_refs: BTreeSet::new(),
                },
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let path = test_path("dimension-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert_eq!(loaded.workspace, workspace);
    assert_eq!(
        loaded
            .workspace
            .history()
            .restore(loaded.workspace.history().head())
            .unwrap(),
        loaded.workspace.document().clone()
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn pdf_export_is_parseable_vector_output_with_explicit_loss_reporting() {
    let path = test_pdf_path("pdf-export");
    let mut document = CadDocument::new("PDF drawing");
    let entities = vec![
        Entity {
            id: 1,
            layer: 1,
            name: "Line".into(),
            visible: true,
            kind: EntityKind::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(40.0, 0.0),
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 2,
            layer: 1,
            name: "Circle".into(),
            visible: true,
            kind: EntityKind::Circle {
                center: Point2::new(20.0, 15.0),
                radius: 5.0,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 3,
            layer: 1,
            name: "Arc".into(),
            visible: true,
            kind: EntityKind::Arc {
                center: Point2::new(10.0, 15.0),
                radius: 4.0,
                start_angle: 0.0,
                sweep_angle: std::f64::consts::PI,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 4,
            layer: 1,
            name: "Dimension".into(),
            visible: true,
            kind: EntityKind::AlignedDimension {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(40.0, 0.0),
                offset: -8.0,
                text_override: Some("TYP <>".into()),
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 5,
            layer: 1,
            name: "Label".into(),
            visible: true,
            kind: EntityKind::Text {
                position: Point2::new(0.0, 22.0),
                content: "CADX VECTOR".into(),
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 6,
            layer: 1,
            name: "Profile".into(),
            visible: true,
            kind: EntityKind::SketchProfile {
                points: vec![
                    Point2::new(25.0, 8.0),
                    Point2::new(35.0, 8.0),
                    Point2::new(30.0, 14.0),
                ],
                closed: true,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 7,
            layer: 1,
            name: "Solid".into(),
            visible: true,
            kind: EntityKind::Extrude {
                profile: 6,
                distance: 3.0,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 8,
            layer: 1,
            name: "Unicode label".into(),
            visible: true,
            kind: EntityKind::Text {
                position: Point2::new(0.0, 25.0),
                content: "\u{5c3a}\u{5bf8}".into(),
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 9,
            layer: 2,
            name: "Hidden".into(),
            visible: true,
            kind: EntityKind::Line {
                start: Point2::new(-100.0, -100.0),
                end: Point2::new(100.0, 100.0),
            },
            parameter_refs: BTreeSet::new(),
        },
    ];
    let mut commands = vec![CadCommand::CreateLayer {
        layer: Layer {
            id: 2,
            name: "Hidden".into(),
            visible: false,
            locked: false,
            color: [255, 0, 0, 255],
        },
    }];
    commands.extend(
        entities
            .into_iter()
            .map(|entity| CadCommand::CreateEntity { entity }),
    );
    CommandTransaction::new(commands)
        .apply(&mut document)
        .unwrap();
    let options = PdfExportOptions {
        page_size: PdfPageSize::A4,
        orientation: PdfOrientation::Landscape,
        margin_mm: 10.0,
        ..Default::default()
    };

    let report = export_pdf(&document, &path, options).unwrap();

    assert_eq!(report.exported_entities, 6);
    assert_eq!(report.skipped_entities, 3);
    assert_eq!(report.simplified_entities, 0);
    assert!(report.page_width_points > report.page_height_points);
    assert!(report.bytes > 0 && report.bytes <= MAX_PDF_BYTES);
    let parsed = lopdf::Document::load(&path).unwrap();
    assert_eq!(parsed.get_pages().len(), 1);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(
        bytes
            .windows(b"TYP 40.00".len())
            .any(|part| part == b"TYP 40.00")
    );
    assert!(
        bytes
            .windows(b"CADX VECTOR".len())
            .any(|part| part == b"CADX VECTOR")
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn invalid_pdf_page_setup_preserves_the_existing_destination() {
    let path = test_pdf_path("pdf-invalid-options");
    fs::write(&path, b"existing drawing").unwrap();
    let options = PdfExportOptions {
        margin_mm: 500.0,
        ..Default::default()
    };

    let error = export_pdf(&CadDocument::new("Invalid PDF"), &path, options).unwrap_err();

    assert!(matches!(error, PdfExportError::InvalidInput(_)));
    assert_eq!(fs::read(&path).unwrap(), b"existing drawing");
    fs::remove_file(path).unwrap();
}

#[test]
fn recovery_sidecar_round_trip_is_lossless_and_explicitly_discarded() {
    let workspace = sample_workspace();
    let project_path = test_path("recovery-project");
    let sidecar = recovery_path(&project_path).unwrap();

    assert!(!recovery_exists(&project_path).unwrap());
    let report = save_recovery(&workspace, &project_path).unwrap();
    assert_eq!(report.path, sidecar);
    assert!(recovery_exists(&project_path).unwrap());

    let loaded = load_recovery(&project_path).unwrap();
    assert_eq!(loaded.workspace, workspace);
    loaded.workspace.validate_integrity().unwrap();

    assert!(discard_recovery(&project_path).unwrap());
    assert!(!discard_recovery(&project_path).unwrap());
    assert!(!sidecar.exists());
}

#[cfg(unix)]
#[test]
fn automatically_discovered_recovery_sidecars_reject_symlinks() {
    let project_path = test_path("recovery-symlink-project");
    let recovery = recovery_path(&project_path).unwrap();
    let target = test_path("recovery-symlink-target");
    fs::write(&target, b"not a project").unwrap();
    std::os::unix::fs::symlink(&target, &recovery).unwrap();

    let error = recovery_exists(&project_path).unwrap_err();

    assert!(matches!(error, ProjectError::InvalidPath(path) if path == recovery));
    fs::remove_file(recovery).unwrap();
    fs::remove_file(target).unwrap();
}

#[test]
fn dxf_import_maps_units_layers_and_supported_entities_without_flattening_curves() {
    let path = test_dxf_path("dxf-import");
    let mut drawing = Drawing::new();
    drawing.header.version = AcadVersion::R2018;
    drawing.header.default_drawing_units = DxfUnits::Inches;
    drawing.add_layer(DxfLayer {
        name: "Parts".into(),
        color: Color::from_index(1),
        is_layer_on: false,
        ..Default::default()
    });

    let mut line = DxfEntity::new(EntityType::Line(DxfLine::new(
        DxfPoint::new(1.0, 2.0, 0.0),
        DxfPoint::new(3.0, 4.0, 0.0),
    )));
    line.common.layer = "Parts".into();
    drawing.add_entity(line);
    drawing.add_entity(DxfEntity::new(EntityType::Circle(DxfCircle::new(
        DxfPoint::new(2.0, 3.0, 0.0),
        0.5,
    ))));

    let mut polyline = LwPolyline {
        vertices: vec![
            LwPolylineVertex {
                x: 0.0,
                y: 0.0,
                ..Default::default()
            },
            LwPolylineVertex {
                x: 2.0,
                y: 0.0,
                ..Default::default()
            },
            LwPolylineVertex {
                x: 2.0,
                y: 1.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    polyline.set_is_closed(true);
    let mut polyline = DxfEntity::new(EntityType::LwPolyline(polyline));
    polyline.common.layer = "Parts".into();
    drawing.add_entity(polyline);

    let mut text = DxfEntity::new(EntityType::Text(DxfText {
        location: DxfPoint::new(4.0, 5.0, 0.0),
        value: "Part A".into(),
        ..Default::default()
    }));
    text.common.layer = "Parts".into();
    text.common.is_visible = false;
    drawing.add_entity(text);
    drawing.add_entity(DxfEntity::new(EntityType::Arc(DxfArc::new(
        DxfPoint::origin(),
        1.0,
        350.0,
        10.0,
    ))));

    let curved = LwPolyline {
        vertices: vec![
            LwPolylineVertex {
                x: 0.0,
                y: 0.0,
                bulge: 1.0,
                ..Default::default()
            },
            LwPolylineVertex {
                x: 1.0,
                y: 1.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    drawing.add_entity(DxfEntity::new(EntityType::LwPolyline(curved)));
    let mut paper_line = DxfEntity::new(EntityType::Line(DxfLine::default()));
    paper_line.common.is_in_paper_space = true;
    drawing.add_entity(paper_line);
    drawing.save_file(&path).unwrap();

    let source = CadDocument::new("DXF target");
    let plan = plan_dxf_import(&source, &path).unwrap();
    assert_eq!(plan.report.source_units, "inches");
    assert!((plan.report.scale_factor - 25.4).abs() < 1.0e-10);
    assert_eq!(plan.report.imported_entities, 5);
    assert_eq!(plan.report.skipped_entities, 2);
    assert_eq!(plan.report.created_layers, 2);

    let mut imported = source;
    plan.transaction.apply(&mut imported).unwrap();
    assert_eq!(imported.entities.len(), 5);
    let EntityKind::Line { start, end } = imported.entities[&1].kind else {
        panic!("expected imported line");
    };
    assert!((start.x - 25.4).abs() < 1.0e-10);
    assert!((start.y - 50.8).abs() < 1.0e-10);
    assert!((end.x - 76.2).abs() < 1.0e-10);
    let parts = imported
        .layers
        .values()
        .find(|layer| layer.name == "Parts")
        .unwrap();
    assert!(!parts.visible);
    assert_eq!(parts.color, [255, 0, 0, 255]);
    assert!(!imported.entities[&4].visible);
    let EntityKind::Arc {
        radius,
        start_angle,
        sweep_angle,
        ..
    } = imported.entities[&5].kind
    else {
        panic!("expected imported arc");
    };
    assert!((radius - 25.4).abs() < 1.0e-10);
    assert!((start_angle.to_degrees() - 350.0).abs() < 1.0e-10);
    assert!((sweep_angle.to_degrees() - 20.0).abs() < 1.0e-10);
    imported.validate().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn dxf_aligned_dimension_import_export_preserves_geometry_units_and_text() {
    let import_path = test_dxf_path("dxf-dimension-import");
    let export_path = test_dxf_path("dxf-dimension-export");
    let mut drawing = Drawing::new();
    drawing.header.version = AcadVersion::R2018;
    drawing.header.default_drawing_units = DxfUnits::Inches;
    drawing.add_entity(DxfEntity::new(EntityType::RotatedDimension(
        RotatedDimension {
            dimension_base: DimensionBase {
                definition_point_1: DxfPoint::new(1.0, -1.0, 0.0),
                text_mid_point: DxfPoint::new(3.0, -1.0, 0.0),
                dimension_type: DimensionType::Aligned,
                text: "TYP <>".into(),
                ..Default::default()
            },
            definition_point_2: DxfPoint::new(1.0, 2.0, 0.0),
            definition_point_3: DxfPoint::new(5.0, 2.0, 0.0),
            ..Default::default()
        },
    )));
    drawing.save_file(&import_path).unwrap();

    let source = CadDocument::new("DXF dimension target");
    let plan = plan_dxf_import(&source, &import_path).unwrap();
    assert_eq!(plan.report.imported_entities, 1);
    assert_eq!(plan.report.skipped_entities, 0);
    assert!((plan.report.scale_factor - 25.4).abs() < 1.0e-10);
    let mut imported = source;
    plan.transaction.apply(&mut imported).unwrap();
    let EntityKind::AlignedDimension {
        start,
        end,
        offset,
        text_override,
    } = &imported.entities[&1].kind
    else {
        panic!("expected imported aligned dimension");
    };
    assert_eq!(*start, Point2::new(25.4, 50.8));
    assert_eq!(*end, Point2::new(127.0, 50.8));
    assert!((*offset + 76.2).abs() < 1.0e-10);
    assert_eq!(text_override.as_deref(), Some("TYP <>"));

    let report = export_dxf(&imported, &export_path).unwrap();
    assert_eq!(report.exported_entities, 1);
    assert_eq!(report.simplified_entities, 0);
    let parsed = Drawing::load_file(&export_path).unwrap();
    let dimension = parsed
        .entities()
        .find_map(|entity| match &entity.specific {
            EntityType::RotatedDimension(dimension) => Some(dimension),
            _ => None,
        })
        .expect("exported DXF contains an aligned dimension");
    assert_eq!(
        dimension.dimension_base.dimension_type,
        DimensionType::Aligned
    );
    assert_eq!(dimension.dimension_base.text, "TYP <>");
    assert!((dimension.definition_point_2.x - 25.4).abs() < 1.0e-10);
    assert!((dimension.definition_point_3.x - 127.0).abs() < 1.0e-10);
    assert!((dimension.dimension_base.definition_point_1.y + 25.4).abs() < 1.0e-10);

    let round_trip = plan_dxf_import(&CadDocument::new("Round trip"), &export_path).unwrap();
    let mut round_trip_document = CadDocument::new("Round trip");
    round_trip
        .transaction
        .apply(&mut round_trip_document)
        .unwrap();
    assert_eq!(
        round_trip_document.entities[&1].kind,
        imported.entities[&1].kind
    );
    fs::remove_file(import_path).unwrap();
    fs::remove_file(export_path).unwrap();
}

#[test]
fn dxf_import_rejects_a_locked_layer_collision_before_any_workspace_write() {
    let path = test_dxf_path("dxf-locked-layer");
    let mut drawing = Drawing::new();
    drawing.add_layer(DxfLayer {
        name: "Concept".into(),
        ..Default::default()
    });
    let mut line = DxfEntity::new(EntityType::Line(DxfLine::new(
        DxfPoint::origin(),
        DxfPoint::new(10.0, 0.0, 0.0),
    )));
    line.common.layer = "Concept".into();
    drawing.add_entity(line);
    drawing.save_file(&path).unwrap();

    let mut document = CadDocument::new("Locked target");
    let mut layer = document.layers[&1].clone();
    layer.locked = true;
    CommandTransaction::new(vec![CadCommand::UpdateLayer { layer }])
        .apply(&mut document)
        .unwrap();
    let original = document.clone();

    assert!(matches!(
        plan_dxf_import(&document, &path),
        Err(DxfExchangeError::LockedLayer(name)) if name == "Concept"
    ));
    assert_eq!(document, original);
    fs::remove_file(path).unwrap();
}

#[test]
fn dxf_import_keeps_sanitized_source_layer_collisions_distinct() {
    let path = test_dxf_path("dxf-sanitized-layers");
    let mut drawing = Drawing::new();
    drawing.add_layer(DxfLayer {
        name: "Bad/Layer".into(),
        ..Default::default()
    });
    drawing.add_layer(DxfLayer {
        name: "DXF Layer 2".into(),
        ..Default::default()
    });
    for (layer_name, y) in [("Bad/Layer", 0.0), ("DXF Layer 2", 1.0)] {
        let mut line = DxfEntity::new(EntityType::Line(DxfLine::new(
            DxfPoint::new(0.0, y, 0.0),
            DxfPoint::new(10.0, y, 0.0),
        )));
        line.common.layer = layer_name.into();
        drawing.add_entity(line);
    }
    drawing.save_file(&path).unwrap();

    let source = CadDocument::new("Layer collision target");
    let plan = plan_dxf_import(&source, &path).unwrap();
    assert_eq!(plan.report.created_layers, 2);
    assert_eq!(plan.report.renamed_layers, 2);
    let mut imported = source;
    plan.transaction.apply(&mut imported).unwrap();

    let imported_layer_ids = imported
        .entities
        .values()
        .map(|entity| entity.layer)
        .collect::<BTreeSet<_>>();
    assert_eq!(imported_layer_ids.len(), 2);
    assert_eq!(
        imported
            .layers
            .values()
            .map(|layer| layer.name.to_ascii_lowercase())
            .collect::<BTreeSet<_>>()
            .len(),
        imported.layers.len()
    );
    imported.validate().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn dxf_export_is_parseable_reports_loss_and_round_trips_the_supported_projection() {
    let path = test_dxf_path("dxf-export");
    let mut document = CadDocument::new("Exchange model");
    let entities = vec![
        Entity {
            id: 1,
            layer: 1,
            name: "Axis".into(),
            visible: true,
            kind: EntityKind::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(40.0, 0.0),
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 2,
            layer: 1,
            name: "Hole".into(),
            visible: true,
            kind: EntityKind::Circle {
                center: Point2::new(10.0, 10.0),
                radius: 4.0,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 3,
            layer: 1,
            name: "Plate".into(),
            visible: true,
            kind: EntityKind::Rectangle {
                origin: Point2::new(0.0, 0.0),
                width: 40.0,
                height: 20.0,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 4,
            layer: 1,
            name: "Profile".into(),
            visible: true,
            kind: EntityKind::SketchProfile {
                points: vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(10.0, 0.0),
                    Point2::new(0.0, 10.0),
                ],
                closed: true,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 5,
            layer: 1,
            name: "Solid".into(),
            visible: true,
            kind: EntityKind::Extrude {
                profile: 4,
                distance: 5.0,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 6,
            layer: 1,
            name: "Wall".into(),
            visible: true,
            kind: EntityKind::Wall {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(20.0, 0.0),
                thickness: 0.2,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 7,
            layer: 1,
            name: "Room".into(),
            visible: true,
            kind: EntityKind::Room {
                boundary: vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(10.0, 0.0),
                    Point2::new(10.0, 10.0),
                    Point2::new(0.0, 10.0),
                ],
                area: 100.0,
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 8,
            layer: 1,
            name: "Label".into(),
            visible: true,
            kind: EntityKind::Text {
                position: Point2::new(3.0, 4.0),
                content: "CADX".into(),
            },
            parameter_refs: BTreeSet::new(),
        },
        Entity {
            id: 9,
            layer: 1,
            name: "Arc".into(),
            visible: true,
            kind: EntityKind::Arc {
                center: Point2::new(15.0, 10.0),
                radius: 6.0,
                start_angle: 350.0_f64.to_radians(),
                sweep_angle: 20.0_f64.to_radians(),
            },
            parameter_refs: BTreeSet::new(),
        },
    ];
    let mut commands = entities
        .into_iter()
        .map(|entity| CadCommand::CreateEntity { entity })
        .collect::<Vec<_>>();
    commands.push(CadCommand::SetParameter {
        parameter: Parameter::literal(1, "width", 40.0, Units::Millimeters),
    });
    let mut layer = document.layers[&1].clone();
    layer.locked = true;
    layer.color = [0, 255, 255, 255];
    commands.push(CadCommand::UpdateLayer { layer });
    CommandTransaction::new(commands)
        .apply(&mut document)
        .unwrap();

    let report = export_dxf(&document, &path).unwrap();
    assert_eq!(report.exported_entities, 8);
    assert_eq!(report.skipped_entities, 1);
    assert_eq!(report.simplified_entities, 4);
    assert_eq!(report.omitted_parameters, 1);
    assert_eq!(report.omitted_constraints, 0);
    assert_eq!(report.omitted_locked_layers, 1);
    assert!(report.bytes > 0);

    let parsed = Drawing::load_file(&path).unwrap();
    assert_eq!(parsed.header.default_drawing_units, DxfUnits::Millimeters);
    assert_eq!(parsed.entities().count(), 8);
    let concept = parsed
        .layers()
        .find(|layer| layer.name == "Concept")
        .unwrap();
    assert_eq!(concept.color.index(), Some(4));

    let target = CadDocument::new("Round trip target");
    let imported = plan_dxf_import(&target, &path).unwrap();
    assert_eq!(imported.report.imported_entities, 8);
    assert_eq!(imported.report.skipped_entities, 0);
    let mut target = target;
    imported.transaction.apply(&mut target).unwrap();
    assert_eq!(target.entities.len(), 8);
    let EntityKind::Arc {
        start_angle,
        sweep_angle,
        ..
    } = target.entities[&8].kind
    else {
        panic!("expected round-tripped arc");
    };
    assert!((start_angle.to_degrees() - 350.0).abs() < 1.0e-9);
    assert!((sweep_angle.to_degrees() - 20.0).abs() < 1.0e-9);
    target.validate().unwrap();
    fs::remove_file(path).unwrap();
}

#[cfg(unix)]
#[test]
fn dxf_import_rejects_symbolic_links() {
    use std::os::unix::fs::symlink;

    let target = test_dxf_path("dxf-symlink-target");
    let link = test_dxf_path("dxf-symlink-link");
    Drawing::new().save_file(&target).unwrap();
    symlink(&target, &link).unwrap();

    assert!(matches!(
        plan_dxf_import(&CadDocument::new("Target"), &link),
        Err(DxfExchangeError::InvalidPath(path)) if path == link
    ));

    fs::remove_file(link).unwrap();
    fs::remove_file(target).unwrap();
}

#[test]
fn native_project_round_trip_preserves_branch_local_redo_state() {
    let mut workspace = sample_workspace();
    let committed_head = workspace.history().head();
    workspace.kernel().undo().unwrap();
    assert!(workspace.can_redo());
    let path = test_path("redo-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let mut loaded = load_workspace(&path).unwrap().workspace;

    assert!(loaded.can_redo());
    assert_eq!(loaded.kernel().redo().unwrap(), committed_head);
    assert_eq!(loaded.document().entities.len(), 1);
    loaded.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_one_project_without_redo_state_is_migrated_and_revalidated() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["history"]
        .as_object_mut()
        .unwrap()
        .remove("redo_stacks");
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.format_version = 1;
    let path = test_path("format-one");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.manifest.format_version,
        CURRENT_PROJECT_FORMAT_VERSION
    );
    assert!(!loaded.workspace.can_redo());
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn format_two_project_regenerates_required_local_validation_evidence() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    for commit in value["history"]["commits"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        commit.as_object_mut().unwrap().remove("evidence");
    }
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.format_version = 2;
    let path = test_path("format-two-evidence");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert!(loaded.workspace.history().commits.values().all(|commit| {
        commit
            .validation_evidence()
            .is_some_and(|evidence| evidence.passed())
    }));
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn current_project_format_rejects_missing_local_validation_evidence() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["history"]["commits"]["1"]
        .as_object_mut()
        .unwrap()
        .remove("evidence");
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("current-missing-evidence");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn format_one_checksum_is_verified_before_migration() {
    let workspace = sample_workspace();
    let bytes = serde_json::to_vec(&workspace).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.format_version = 1;
    manifest.workspace_crc32 = manifest.workspace_crc32.wrapping_add(1);
    let path = test_path("format-one-checksum");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::IntegrityMismatch { .. }));
    fs::remove_file(path).unwrap();
}

#[test]
fn noncontiguous_persisted_redo_state_is_rejected() {
    let mut workspace = sample_workspace();
    workspace.kernel().undo().unwrap();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["history"]["redo_stacks"]["main"] = serde_json::json!([0]);
    let bytes = serde_json::to_vec(&value).unwrap();
    let manifest = ProjectManifest::current(&bytes);
    let path = test_path("invalid-redo");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::Workspace(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn checksum_mismatch_is_rejected_before_workspace_deserialization() {
    let workspace = sample_workspace();
    let bytes = serde_json::to_vec(&workspace).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.workspace_crc32 = manifest.workspace_crc32.wrapping_add(1);
    let path = test_path("checksum");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::IntegrityMismatch { .. }));
    fs::remove_file(path).unwrap();
}

#[test]
fn manifest_schema_must_match_the_workspace_payload() {
    let workspace = sample_workspace();
    let bytes = serde_json::to_vec(&workspace).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.document_schema_version = 0;
    let path = test_path("manifest-schema");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let error = load_workspace(&path).unwrap_err();

    assert!(matches!(error, ProjectError::InvalidManifest(_)));
    fs::remove_file(path).unwrap();
}

#[test]
fn format_zero_project_is_migrated_and_revalidated() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["document"]["schema_version"] = Value::from(0);
    for snapshot in value["history"]["snapshots"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        snapshot["document"]["schema_version"] = Value::from(0);
    }
    let bytes = serde_json::to_vec(&value).unwrap();
    let legacy_manifest = ProjectManifest {
        format_version: 0,
        document_schema_version: 0,
        workspace_entry: WORKSPACE_ENTRY.into(),
        workspace_bytes: 0,
        workspace_crc32: 0,
    };
    let path = test_path("migration");
    fs::write(&path, encode_archive(&legacy_manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.workspace.document().schema_version,
        CURRENT_SCHEMA_VERSION
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn schema_one_project_migrates_missing_parametric_fields_and_commit_diffs() {
    let mut workspace = sample_workspace();
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Add legacy parameter",
            CommandTransaction::new(vec![CadCommand::SetParameter {
                parameter: Parameter::literal(1, "legacy_width", 80.0, Units::Millimeters),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let mut value = serde_json::to_value(&workspace).unwrap();
    downgrade_workspace_to_schema_one(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.document_schema_version = 1;
    let path = test_path("schema-one");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.workspace.document().schema_version,
        CURRENT_SCHEMA_VERSION
    );
    assert!(loaded.workspace.document().constraints.is_empty());
    assert_eq!(loaded.workspace.document().next_constraint_id(), 1);
    assert_eq!(loaded.workspace.document().parameters[&1].expression, None);
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn schema_two_project_migrates_layer_locking_and_diff_fields() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    downgrade_workspace_to_schema_two(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.document_schema_version = 2;
    let path = test_path("schema-two");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.workspace.document().schema_version,
        CURRENT_SCHEMA_VERSION
    );
    assert!(
        loaded
            .workspace
            .document()
            .layers
            .values()
            .all(|layer| !layer.locked)
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn schema_three_project_migrates_to_the_arc_capable_schema_and_replays() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    downgrade_workspace_to_schema_three(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.document_schema_version = 3;
    let path = test_path("schema-three");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.workspace.document().schema_version,
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(loaded.workspace, workspace);
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn schema_four_project_migrates_to_the_dimension_capable_schema_and_replays() {
    let workspace = sample_workspace();
    let mut value = serde_json::to_value(&workspace).unwrap();
    downgrade_workspace_to_schema_four(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut manifest = ProjectManifest::current(&bytes);
    manifest.document_schema_version = 4;
    let path = test_path("schema-four");
    fs::write(&path, encode_archive(&manifest, &bytes).unwrap()).unwrap();

    let loaded = load_workspace(&path).unwrap();

    assert!(loaded.migrated);
    assert_eq!(
        loaded.workspace.document().schema_version,
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(loaded.workspace, workspace);
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

#[test]
fn parametric_history_round_trip_preserves_formulas_constraints_and_solve_replay() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Parametric persistence"));
    let start = SketchPoint::new(1, PointAnchor::Start);
    let end = SketchPoint::new(1, PointAnchor::End);
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Create constrained line",
            CommandTransaction::new(vec![
                CadCommand::SetParameter {
                    parameter: Parameter::literal(1, "target_length", 40.0, Units::Millimeters),
                },
                CadCommand::SetParameter {
                    parameter: Parameter::formula(
                        2,
                        "overall_length",
                        "target_length * 1.5",
                        Units::Millimeters,
                    )
                    .unwrap(),
                },
                CadCommand::CreateEntity {
                    entity: Entity {
                        id: 1,
                        layer: 1,
                        name: "Parametric line".into(),
                        visible: true,
                        kind: EntityKind::Line {
                            start: Point2::new(0.0, 0.0),
                            end: Point2::new(12.0, 4.0),
                        },
                        parameter_refs: BTreeSet::from([2]),
                    },
                },
                CadCommand::CreateConstraint {
                    constraint: SketchConstraint {
                        id: 1,
                        name: "Horizontal".into(),
                        driving: true,
                        kind: ConstraintKind::Horizontal {
                            segment: SketchSegment::new(start, end),
                        },
                    },
                },
                CadCommand::CreateConstraint {
                    constraint: SketchConstraint {
                        id: 2,
                        name: "Length".into(),
                        driving: true,
                        kind: ConstraintKind::Distance {
                            first: start,
                            second: end,
                            value: ParameterExpression::new("overall_length").unwrap(),
                        },
                    },
                },
            ]),
            ValidationReport::default(),
        )
        .unwrap();
    let solution = solve_constraints(workspace.document(), Default::default()).unwrap();
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Solve constrained line",
            solution.transaction().unwrap(),
            ValidationReport::default(),
        )
        .unwrap();
    let path = test_path("parametric-round-trip");

    save_workspace(&workspace, &path).unwrap();
    let loaded = load_workspace(&path).unwrap();

    assert!(!loaded.migrated);
    assert_eq!(loaded.workspace, workspace);
    assert_eq!(loaded.workspace.document().constraints.len(), 2);
    assert_eq!(
        loaded.workspace.document().parameters[&2]
            .expression
            .as_ref()
            .map(|expression| expression.source()),
        Some("target_length * 1.5")
    );
    loaded.workspace.validate_integrity().unwrap();
    fs::remove_file(path).unwrap();
}

fn downgrade_workspace_to_schema_one(workspace: &mut Value) {
    downgrade_document_to_schema_one(&mut workspace["document"]);
    for snapshot in workspace["history"]["snapshots"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        downgrade_document_to_schema_one(&mut snapshot["document"]);
    }
    for commit in workspace["history"]["commits"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        let diff = commit["diff"].as_object_mut().unwrap();
        diff.remove("updated_layers");
        diff.remove("deleted_layers");
        diff.remove("created_constraints");
        diff.remove("updated_constraints");
        diff.remove("deleted_constraints");
    }
}

fn downgrade_document_to_schema_one(document: &mut Value) {
    let document = document.as_object_mut().unwrap();
    document.insert("schema_version".into(), Value::from(1));
    document.remove("constraints");
    document.remove("next_constraint_id");
    for layer in document["layers"].as_object_mut().unwrap().values_mut() {
        layer.as_object_mut().unwrap().remove("locked");
    }
    for parameter in document["parameters"].as_object_mut().unwrap().values_mut() {
        parameter.as_object_mut().unwrap().remove("expression");
    }
}

fn downgrade_workspace_to_schema_two(workspace: &mut Value) {
    downgrade_document_to_schema_two(&mut workspace["document"]);
    for snapshot in workspace["history"]["snapshots"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        downgrade_document_to_schema_two(&mut snapshot["document"]);
    }
    for commit in workspace["history"]["commits"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        let diff = commit["diff"].as_object_mut().unwrap();
        diff.remove("updated_layers");
        diff.remove("deleted_layers");
    }
}

fn downgrade_document_to_schema_two(document: &mut Value) {
    let document = document.as_object_mut().unwrap();
    document.insert("schema_version".into(), Value::from(2));
    for layer in document["layers"].as_object_mut().unwrap().values_mut() {
        layer.as_object_mut().unwrap().remove("locked");
    }
}

fn downgrade_workspace_to_schema_three(workspace: &mut Value) {
    workspace["document"]["schema_version"] = Value::from(3);
    for snapshot in workspace["history"]["snapshots"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        snapshot["document"]["schema_version"] = Value::from(3);
    }
}

fn downgrade_workspace_to_schema_four(workspace: &mut Value) {
    workspace["document"]["schema_version"] = Value::from(4);
    for snapshot in workspace["history"]["snapshots"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        snapshot["document"]["schema_version"] = Value::from(4);
    }
}
