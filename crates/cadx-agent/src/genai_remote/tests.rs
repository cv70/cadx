use std::collections::BTreeSet;
use std::fs;

use cadx_core::{
    CadCommand, CadDocument, Capability, CommandTransaction, Entity, EntityKind, Layer, Point2,
    TaskAuthority, TaskWorkspace, solve_constraints,
};

use super::*;
use crate::provider::{
    AgentObservation, PlanningDecision, RemoteContextRequest, prepare_remote_context,
};
use crate::remote_plan::{decode_decision, materialize_decision};

fn config() -> ProviderConfig {
    ProviderConfig {
        endpoint: "https://provider.example/v1".into(),
        model: "test-model".into(),
        enabled_capabilities: BTreeSet::from([
            Capability::Drafting,
            Capability::Mechanical,
            Capability::Architecture,
        ]),
    }
}

#[test]
fn endpoint_normalization_preserves_the_v1_path_for_responses() {
    assert_eq!(
        normalized_endpoint("https://provider.example/v1"),
        "https://provider.example/v1/"
    );
    assert_eq!(
        normalized_endpoint("https://provider.example/v1///"),
        "https://provider.example/v1/"
    );
}

#[test]
fn prompt_contains_disclosed_metadata_but_not_entity_geometry_or_attachments() {
    let mut document = CadDocument::new("Remote context");
    document.entities.insert(
        1,
        Entity {
            id: 1,
            layer: 1,
            name: "Private geometry name".into(),
            visible: true,
            kind: EntityKind::Line {
                start: Point2::new(123.25, 456.5),
                end: Point2::new(789.75, 1_001.0),
            },
            parameter_refs: BTreeSet::new(),
        },
    );
    let mut workspace = TaskWorkspace::new(document);
    let task_id = workspace.kernel().create_task(
        "Draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let observation = AgentObservation {
        task: workspace.task(task_id).unwrap().clone(),
        snapshot: workspace.snapshot(),
    };
    let (context, disclosure) = prepare_remote_context(
        config(),
        RemoteContextRequest::default(),
        workspace.project_id(),
        &observation,
    )
    .unwrap();

    let prompt = build_prompt(&context);

    assert!(prompt.contains("\"entity_count\":1"));
    assert!(prompt.contains("\"source_files_included\":false"));
    assert!(prompt.contains("\"attachments_included\":false"));
    assert!(!prompt.contains("Private geometry name"));
    assert!(!prompt.contains("123.25"));
    assert!(!prompt.contains("456.5"));
    assert_eq!(disclosure.payload_hash, context.payload_hash());
    assert_eq!(disclosure.payload_bytes, prompt.len());
}

#[test]
fn remote_json_is_materialized_as_a_local_typed_transaction() {
    let decision = decode_decision(
        r#"{
                "decision": "action",
                "action": {
                    "intent": "Create base rectangle",
                    "detail": "Create an editable 80 by 40 mm base.",
                    "operation": {
                        "kind": "create_rectangle",
                        "name": "Base plate",
                        "origin": [0.0, 0.0],
                        "width": 80.0,
                        "height": 40.0
                    },
                    "validation": [{
                        "name": "Positive dimensions",
                        "detail": "Both dimensions are positive.",
                        "status": "passed"
                    }]
                }
            }"#,
    )
    .unwrap();
    let document = CadDocument::new("Remote plan");
    let PlanningDecision::Action(action) =
        materialize_decision(decision, &document, &BTreeSet::from([Capability::Drafting])).unwrap()
    else {
        panic!("expected one remote action");
    };

    assert_eq!(action.tool_name, "remote.create_rectangle");
    assert!(matches!(
        action.transaction.commands.as_slice(),
        [CadCommand::CreateEntity {
            entity: Entity {
                kind: EntityKind::Rectangle { width, height, .. },
                ..
            }
        }] if *width == 80.0 && *height == 40.0
    ));
}

#[test]
fn remote_plan_materialization_avoids_hidden_and_locked_layers() {
    let mut document = CadDocument::new("Remote layer selection");
    let mut concept = document.layers[&1].clone();
    concept.locked = true;
    CommandTransaction::new(vec![
        CadCommand::UpdateLayer { layer: concept },
        CadCommand::CreateLayer {
            layer: Layer {
                id: 2,
                name: "Remote output".into(),
                visible: true,
                locked: false,
                color: [90, 160, 235, 255],
            },
        },
    ])
    .apply(&mut document)
    .unwrap();
    let decision = decode_decision(
        r#"{
            "decision": "action",
            "action": {
                "intent": "Create circle",
                "detail": "Create editable output.",
                "operation": {
                    "kind": "create_circle",
                    "name": "Output circle",
                    "center": [0.0, 0.0],
                    "radius": 5.0
                }
            }
        }"#,
    )
    .unwrap();

    let PlanningDecision::Action(action) =
        materialize_decision(decision, &document, &BTreeSet::from([Capability::Drafting])).unwrap()
    else {
        panic!("expected one remote action");
    };

    assert!(matches!(
        action.transaction.commands.as_slice(),
        [CadCommand::CreateEntity { entity }] if entity.layer == 2
    ));
}

#[test]
fn remote_parametric_decisions_are_reobserved_and_materialized_one_at_a_time() {
    let parameter_decision = decode_decision(
        r#"{
                "decision": "action",
                "action": {
                        "intent": "Set target length",
                        "detail": "Create an editable line length parameter.",
                        "operation": {
                            "kind": "create_parameter",
                            "name": "target_length",
                            "value": 40.0
                        }
                }
            }"#,
    )
    .unwrap();
    let mut document = CadDocument::new("Remote parametric plan");
    let capabilities = BTreeSet::from([
        Capability::Drafting,
        Capability::Mechanical,
        Capability::Parameters,
    ]);
    let PlanningDecision::Action(parameter_action) =
        materialize_decision(parameter_decision, &document, &capabilities).unwrap()
    else {
        panic!("expected parameter action");
    };
    assert!(matches!(
        parameter_action.transaction.commands.as_slice(),
        [CadCommand::SetParameter { parameter }] if parameter.name == "target_length"
    ));
    parameter_action.transaction.apply(&mut document).unwrap();

    let line_decision = decode_decision(
        r#"{
                "decision": "action",
                "action": {
                        "intent": "Create constrained line",
                        "detail": "Create a horizontal line driven by the target length.",
                        "operation": {
                            "kind": "create_constrained_line",
                            "name": "Driven line",
                            "start": [0.0, 0.0],
                            "end": [12.0, 7.0],
                            "horizontal": true,
                            "length": "target_length"
                        }
                }
            }"#,
    )
    .unwrap();
    let PlanningDecision::Action(line_action) =
        materialize_decision(line_decision, &document, &capabilities).unwrap()
    else {
        panic!("expected constrained-line action");
    };
    assert!(matches!(
        line_action.transaction.commands.as_slice(),
        [
            CadCommand::CreateEntity { .. },
            CadCommand::CreateConstraint { .. },
            CadCommand::CreateConstraint { .. }
        ]
    ));
    line_action.transaction.apply(&mut document).unwrap();
    assert_eq!(document.parameters.len(), 1);
    assert_eq!(document.constraints.len(), 2);
    assert_eq!(document.entities[&1].parameter_refs, BTreeSet::from([1]));
    assert!(
        solve_constraints(&document, Default::default())
            .unwrap()
            .converged
    );
}

#[test]
fn remote_parametric_operations_require_capability_and_prior_dependencies() {
    let constrained_line = decode_decision(
        r#"{
                "decision": "action",
                "action": {
                    "intent": "Create constrained line",
                    "detail": "Create a horizontal line.",
                    "operation": {
                        "kind": "create_constrained_line",
                        "name": "Line",
                        "start": [0.0, 0.0],
                        "end": [1.0, 1.0],
                        "horizontal": true
                    }
                }
            }"#,
    )
    .unwrap();
    let document = CadDocument::new("Remote capabilities");
    let error = materialize_decision(
        constrained_line,
        &document,
        &BTreeSet::from([Capability::Drafting]),
    )
    .unwrap_err();
    assert!(matches!(error, AgentError::Provider(_)));

    let future_formula = decode_decision(
        r#"{
                "decision": "action",
                "action": {
                        "intent": "Create formula",
                        "detail": "Use a parameter defined later.",
                        "operation": {
                            "kind": "create_parameter",
                            "name": "derived",
                            "formula": "future * 2"
                        }
                }
            }"#,
    )
    .unwrap();
    let error = materialize_decision(
        future_formula,
        &document,
        &BTreeSet::from([Capability::Parameters]),
    )
    .unwrap_err();
    assert!(matches!(error, AgentError::Provider(_)));
}

#[test]
fn unknown_model_fields_are_rejected_before_any_workspace_write() {
    let error = decode_decision(
        r#"{
                "decision": "complete",
                "summary": "Done",
                "untrusted_extra": true
            }"#,
    )
    .unwrap_err();

    assert_eq!(
        error,
        AgentError::Provider("remote model returned an invalid planning decision".into())
    );
}

#[test]
fn batch_shaped_or_ambiguous_remote_responses_are_rejected() {
    for body in [
        r#"{"actions":[]}"#,
        r#"{"decision":"complete","summary":""}"#,
        r#"{"decision":"action","action":{"intent":"x","detail":"x","operation":{"kind":"create_rectangle","name":"x","origin":[0,0],"width":1,"height":1}},"summary":"also complete"}"#,
    ] {
        assert!(decode_decision(body).is_err(), "accepted {body}");
    }
}

#[test]
fn planner_requires_a_nonempty_configured_key() {
    let error = GenAiRemotePlanner::new(config(), "   ").unwrap_err();

    assert_eq!(
        error,
        AgentError::Provider("provider API key is required".into())
    );
}

#[test]
fn planner_debug_output_redacts_the_configured_key() {
    let planner = GenAiRemotePlanner::new(config(), "test-provider-key").unwrap();

    assert!(!format!("{planner:?}").contains("test-provider-key"));
}

#[test]
fn planner_enforces_egress_policy_before_attempting_a_network_request() {
    let directory = tempfile::tempdir().unwrap();
    let policy_path = directory.path().join("egress-policy.yaml");
    fs::write(&policy_path, b"version: 1\nallowed_providers: []\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&policy_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let planner = GenAiRemotePlanner::new_with_egress_policy(
        config(),
        "test-provider-key",
        EgressPolicyEnforcer::at(&policy_path),
    )
    .unwrap();
    let mut workspace = TaskWorkspace::new(CadDocument::new("Denied remote context"));
    let task_id = workspace.kernel().create_task(
        "Draft",
        "Create a drafting concept",
        TaskAuthority::all_direct(),
    );
    let observation = AgentObservation {
        task: workspace.task(task_id).unwrap().clone(),
        snapshot: workspace.snapshot(),
    };
    let (context, _) = prepare_remote_context(
        config(),
        RemoteContextRequest::default(),
        workspace.project_id(),
        &observation,
    )
    .unwrap();

    let error = match planner.plan_remote(context) {
        Ok(_) => panic!("denied egress unexpectedly attempted a provider request"),
        Err(error) => error,
    };

    assert!(matches!(error, AgentError::Provider(message) if message.contains("egress denied")));
}
