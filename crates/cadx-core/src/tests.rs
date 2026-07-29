use std::collections::BTreeSet;

use super::*;

fn rectangle(id: EntityId) -> Entity {
    Entity {
        id,
        layer: 1,
        name: "Base plate".into(),
        visible: true,
        kind: EntityKind::Rectangle {
            origin: Point2::new(0.0, 0.0),
            width: 80.0,
            height: 50.0,
        },
        parameter_refs: BTreeSet::new(),
    }
}

#[test]
fn transactions_are_atomic_when_later_commands_fail() {
    let mut document = CadDocument::new("Atomic");
    let transaction = CommandTransaction::new(vec![
        CadCommand::CreateEntity {
            entity: rectangle(1),
        },
        CadCommand::CreateEntity {
            entity: Entity {
                id: 2,
                layer: 1,
                name: "Invalid".into(),
                visible: true,
                kind: EntityKind::Circle {
                    center: Point2::new(0.0, 0.0),
                    radius: 0.0,
                },
                parameter_refs: BTreeSet::new(),
            },
        },
    ]);

    assert!(transaction.apply(&mut document).is_err());
    assert!(document.entities.is_empty());
}

#[test]
fn layer_locking_and_atomic_reassignment_protect_entity_edits() {
    let mut document = CadDocument::new("Managed layers");
    let secondary = Layer {
        id: 2,
        name: "Annotations".into(),
        visible: true,
        locked: false,
        color: [90, 160, 235, 255],
    };
    let mut annotation = rectangle(1);
    annotation.layer = 2;
    CommandTransaction::new(vec![
        CadCommand::CreateLayer {
            layer: secondary.clone(),
        },
        CadCommand::CreateEntity {
            entity: annotation.clone(),
        },
        CadCommand::UpdateLayer {
            layer: Layer {
                locked: true,
                ..secondary.clone()
            },
        },
    ])
    .apply(&mut document)
    .unwrap();

    let locked = document.clone();
    annotation.name = "Changed while locked".into();
    assert_eq!(
        CommandTransaction::new(vec![CadCommand::UpdateEntity {
            entity: annotation.clone(),
        }])
        .apply(&mut document),
        Err(CommandError::LayerLocked(2))
    );
    assert_eq!(document, locked);

    annotation.layer = 1;
    annotation.name = "Moved annotation".into();
    let diff = CommandTransaction::new(vec![
        CadCommand::UpdateLayer { layer: secondary },
        CadCommand::UpdateEntity { entity: annotation },
        CadCommand::DeleteLayer { id: 2 },
    ])
    .apply(&mut document)
    .unwrap();

    assert_eq!(diff.updated_layers, vec![2]);
    assert_eq!(diff.updated_entities, vec![1]);
    assert_eq!(diff.deleted_layers, vec![2]);
    assert_eq!(document.entities[&1].layer, 1);
    assert!(!document.layers.contains_key(&2));
    document.validate().unwrap();
}

#[test]
fn layers_require_unique_names_and_cannot_be_deleted_while_populated() {
    let mut document = CadDocument::new("Layer invariants");
    CommandTransaction::new(vec![CadCommand::CreateLayer {
        layer: Layer {
            id: 2,
            name: "Dimensions".into(),
            visible: true,
            locked: false,
            color: [240, 180, 70, 255],
        },
    }])
    .apply(&mut document)
    .unwrap();

    let duplicate = CommandTransaction::new(vec![CadCommand::CreateLayer {
        layer: Layer {
            id: 3,
            name: "dimensions".into(),
            visible: true,
            locked: false,
            color: [255, 255, 255, 255],
        },
    }])
    .apply(&mut document);
    assert!(matches!(duplicate, Err(CommandError::InvalidLayer(_))));

    let before = document.clone();
    let populated_delete = CommandTransaction::new(vec![
        CadCommand::CreateEntity {
            entity: rectangle(1),
        },
        CadCommand::DeleteLayer { id: 1 },
    ])
    .apply(&mut document);
    assert!(matches!(
        populated_delete,
        Err(CommandError::InvalidLayer(_))
    ));
    assert_eq!(document, before);
}

#[test]
fn snapshots_and_replay_restore_the_same_document() {
    let document = CadDocument::new("History");
    let mut history = History::new(document.clone());
    let mut current = document;
    for id in 1..=5 {
        let (next, _) = history
            .commit(
                &current,
                None,
                "Add geometry",
                CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(id),
                }]),
                ValidationReport::default(),
            )
            .unwrap();
        current = next;
    }

    assert!(history.snapshots.contains_key(&4));
    assert_eq!(history.restore(5).unwrap(), current);
    assert_eq!(history.restore(2).unwrap().entities.len(), 2);
}

#[test]
fn branch_heads_are_isolated() {
    let document = CadDocument::new("Branching");
    let mut history = History::new(document.clone());
    let (main_document, first) = history
        .commit(
            &document,
            None,
            "Add base",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    history.create_branch("alternative", first).unwrap();
    let (_, main_head) = history
        .commit(
            &main_document,
            None,
            "Main option",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(2),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let alternative_document = history.checkout_branch("alternative").unwrap();
    let (_, alternative_head) = history
        .commit(
            &alternative_document,
            None,
            "Alternative option",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(3),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    assert_ne!(main_head, alternative_head);
    assert_eq!(
        history
            .restore(main_head)
            .unwrap()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        history
            .restore(alternative_head)
            .unwrap()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
}

#[test]
fn review_only_tasks_cannot_mutate_the_document() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Protected"));
    let task = workspace.create_task("Inspect", "Review the plate", TaskAuthority::ReviewOnly);
    workspace.begin_task(task).unwrap();
    workspace
        .set_task_plan(
            task,
            workspace.revision(),
            vec![TaskAction {
                intent: "Add plate".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create a plate".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    let result = workspace.apply_next_task_action(task);
    assert_eq!(result, Err(WorkspaceError::Unauthorized(task)));
    assert!(workspace.document().entities.is_empty());
}

#[test]
fn scoped_authority_cannot_delete_another_domain_entity() {
    let mut document = CadDocument::new("Scoped");
    CommandTransaction::new(vec![CadCommand::CreateEntity {
        entity: Entity {
            id: 1,
            layer: 1,
            name: "Room".into(),
            visible: true,
            kind: EntityKind::Room {
                boundary: vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(10.0, 0.0),
                    Point2::new(0.0, 10.0),
                ],
                area: 50.0,
            },
            parameter_refs: BTreeSet::new(),
        },
    }])
    .apply(&mut document)
    .unwrap();
    let authority = TaskAuthority::DirectWrite {
        capabilities: BTreeSet::from([Capability::Mechanical]),
    };

    assert!(!authority.permits(
        &CommandTransaction::new(vec![CadCommand::DeleteEntity { id: 1 }]),
        &document
    ));
}

#[test]
fn transactions_cannot_leave_a_dangling_feature_reference() {
    let mut document = CadDocument::new("Feature integrity");
    CommandTransaction::new(vec![
        CadCommand::CreateEntity {
            entity: Entity {
                id: 1,
                layer: 1,
                name: "Closed profile".into(),
                visible: true,
                kind: EntityKind::SketchProfile {
                    points: vec![
                        Point2::new(0.0, 0.0),
                        Point2::new(20.0, 0.0),
                        Point2::new(0.0, 20.0),
                    ],
                    closed: true,
                },
                parameter_refs: BTreeSet::new(),
            },
        },
        CadCommand::CreateEntity {
            entity: Entity {
                id: 2,
                layer: 1,
                name: "Extrusion".into(),
                visible: true,
                kind: EntityKind::Extrude {
                    profile: 1,
                    distance: 5.0,
                },
                parameter_refs: BTreeSet::new(),
            },
        },
    ])
    .apply(&mut document)
    .unwrap();

    let before = document.clone();
    let error = CommandTransaction::new(vec![CadCommand::DeleteEntity { id: 1 }])
        .apply(&mut document)
        .unwrap_err();

    assert!(matches!(error, CommandError::EntityMissing(1)));
    assert_eq!(document, before);
}

#[test]
fn history_comparison_uses_full_branch_states() {
    let document = CadDocument::new("Compare");
    let mut history = History::new(document.clone());
    let (first_document, first) = history
        .commit(
            &document,
            None,
            "Base",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    history.create_branch("alternative", first).unwrap();
    let (_, main_head) = history
        .commit(
            &first_document,
            None,
            "Main change",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(2),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let alternative_document = history.checkout_branch("alternative").unwrap();
    let (_, alternative_head) = history
        .commit(
            &alternative_document,
            None,
            "Alternative change",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(3),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    let comparison = history.compare(main_head, alternative_head).unwrap();
    assert_eq!(comparison.added_entities, vec![3]);
    assert_eq!(comparison.removed_entities, vec![2]);
    assert!(comparison.modified_entities.is_empty());
    assert_eq!(comparison.summary(), "1 added, 1 removed, 0 modified");
    history.validate_integrity().unwrap();
}

#[test]
fn authority_preflight_rejects_invalid_transactions_without_applying_them() {
    let document = CadDocument::new("Authority preflight");
    let authority = TaskAuthority::all_direct();
    let invalid = CommandTransaction::new(vec![CadCommand::CreateEntity {
        entity: rectangle(EntityId::MAX),
    }]);

    assert!(!authority.permits(&invalid, &document));
    assert!(document.entities.is_empty());
}

#[test]
fn interrupted_running_task_with_durable_actions_recovers_as_paused() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Interrupted task"));
    let task_id = workspace.create_task(
        "Create plate",
        "Create an editable plate",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![TaskAction {
                intent: "Create plate".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create an editable rectangle.".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();

    workspace.migrate_to_current().unwrap();

    let task = &workspace.tasks()[&task_id];
    assert_eq!(task.status, TaskStatus::Paused);
    assert!(matches!(
        task.events().last(),
        Some(TaskEvent::Paused { .. })
    ));
    workspace.validate_integrity().unwrap();
}

#[test]
fn user_edits_are_semantic_commits_with_no_task_source() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("User edit"));

    let commit_id = workspace
        .apply_user_transaction(
            workspace.revision(),
            "Draw base plate",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    let commit = &workspace.history().commits[&commit_id];
    assert_eq!(commit.task_id, None);
    assert_eq!(
        workspace.history().restore(commit_id).unwrap(),
        workspace.document().clone()
    );
    workspace.validate_integrity().unwrap();
}

#[test]
fn private_document_store_preserves_workspace_json_layout_and_round_trip() {
    let workspace = TaskWorkspace::new(CadDocument::new("Stable workspace layout"));
    let encoded = serde_json::to_value(&workspace).unwrap();
    let keys = encoded
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        keys,
        BTreeSet::from([
            "document",
            "history",
            "next_agent_run_id",
            "next_change_set_id",
            "next_task_id",
            "project_id",
            "remote_access_policy",
            "tasks",
        ])
    );
    assert!(encoded.get("store").is_none());
    assert_eq!(
        serde_json::from_value::<TaskWorkspace>(encoded).unwrap(),
        workspace
    );
}

#[test]
fn kernel_facade_debug_does_not_expose_document_metadata() {
    let secret_title = "customer-confidential-model-name";
    let mut workspace = TaskWorkspace::new(CadDocument::new(secret_title));

    let debug = format!("{:?}", workspace.kernel());

    assert!(debug.contains("KernelFacade"));
    assert!(debug.contains("revision: 0"));
    assert!(!debug.contains(secret_title));
}

#[test]
fn local_evidence_is_bound_to_the_candidate_and_not_the_planner_claim() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Local evidence"));
    let task_id = workspace.create_task(
        "Create plate",
        "Create an editable plate",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    let claim = ValidationReport {
        checks: vec![CheckResult {
            name: "Planner assertion".into(),
            status: CheckStatus::Failed,
            detail: "The remote planner cannot establish validity.".into(),
        }],
    };

    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![TaskAction {
                intent: "Create plate".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create an editable plate".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: claim.clone(),
            }],
        )
        .unwrap();
    let commit_id = workspace.apply_next_task_action(task_id).unwrap();
    let commit_id = commit_id.unwrap();

    let commit = &workspace.history().commits[&commit_id];
    assert_eq!(commit.validation, claim);
    assert!(!commit.validation.passed());
    let evidence = commit.validation_evidence().unwrap();
    assert_eq!(evidence.validator_id(), CORE_VALIDATOR_ID);
    assert_eq!(evidence.validator_version(), CORE_VALIDATOR_VERSION);
    assert_eq!(evidence.candidate_state_hash_hex().len(), 64);
    assert_eq!(evidence.checks().len(), 2);
    assert!(evidence.passed());
    assert!(matches!(
        workspace.tasks()[&task_id].events().last(),
        Some(TaskEvent::Validated {
            validator_id,
            candidate_state_hash,
            ..
        }) if validator_id == CORE_VALIDATOR_ID
            && candidate_state_hash.as_str() == evidence.candidate_state_hash_hex()
    ));
    workspace.validate_integrity().unwrap();
}

#[test]
fn forged_planner_pass_cannot_commit_a_conflicting_constraint_system() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Conflicting candidate"));
    let task_id = workspace.create_task(
        "Create conflicting constraints",
        "Try to constrain one circle to two radii",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    let circle = Entity {
        id: 1,
        layer: 1,
        name: "Conflicted circle".into(),
        visible: true,
        kind: EntityKind::Circle {
            center: Point2::new(0.0, 0.0),
            radius: 1.0,
        },
        parameter_refs: BTreeSet::new(),
    };
    let constraints = [(1, "Small radius", "5"), (2, "Large radius", "10")]
        .into_iter()
        .map(|(id, name, value)| CadCommand::CreateConstraint {
            constraint: SketchConstraint {
                id,
                name: name.into(),
                driving: true,
                kind: ConstraintKind::Radius {
                    entity_id: 1,
                    value: ParameterExpression::new(value).unwrap(),
                },
            },
        });
    let transaction = CommandTransaction::new(
        std::iter::once(CadCommand::CreateEntity { entity: circle })
            .chain(constraints)
            .collect(),
    );
    let forged_claim = ValidationReport {
        checks: vec![CheckResult {
            name: "Remote validation".into(),
            status: CheckStatus::Passed,
            detail: "Everything passed.".into(),
        }],
    };

    let error = workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![TaskAction {
                intent: "Commit conflicts".into(),
                tool_name: "mechanical.constrain_radius".into(),
                detail: "Create conflicting constraints".into(),
                transaction,
                validation: forged_claim,
            }],
        )
        .unwrap_err();

    assert!(matches!(
        error,
        WorkspaceError::Prepare(PrepareError::ValidationFailed(_))
    ));
    assert_eq!(workspace.history().head(), 0);
    assert!(workspace.document().entities.is_empty());
    assert!(workspace.document().constraints.is_empty());
    workspace.validate_integrity().unwrap();
}

#[test]
fn replay_rejects_tampered_local_validation_evidence() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Tamper evidence"));
    workspace
        .apply_user_transaction(
            workspace.revision(),
            "Create plate",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let mut encoded = serde_json::to_value(&workspace).unwrap();
    let hash = encoded["history"]["commits"]["1"]["evidence"]["candidate_state_hash"]
        .as_array_mut()
        .unwrap();
    let first = hash[0].as_u64().unwrap();
    hash[0] = serde_json::Value::from((first + 1) % 256);
    let tampered = serde_json::from_value::<TaskWorkspace>(encoded).unwrap();

    let error = tampered.validate_integrity().unwrap_err();

    assert!(matches!(
        error,
        WorkspaceError::History(HistoryError::InvalidHistory(message))
            if message.contains("validation evidence does not match")
    ));
}

#[test]
fn undo_and_redo_restore_branch_heads_without_rewriting_history() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Undo and redo"));
    let first = workspace
        .apply_user_transaction(
            workspace.revision(),
            "First rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let second = workspace
        .apply_user_transaction(
            workspace.revision(),
            "Second rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(2),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    assert!(workspace.can_undo());
    assert!(!workspace.can_redo());
    assert_eq!(workspace.undo().unwrap(), first);
    assert_eq!(
        workspace
            .document()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert!(workspace.can_redo());
    assert_eq!(workspace.undo().unwrap(), 0);
    assert!(workspace.document().entities.is_empty());
    assert!(!workspace.can_undo());

    assert_eq!(workspace.redo().unwrap(), first);
    assert_eq!(workspace.redo().unwrap(), second);
    assert_eq!(
        workspace
            .document()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(!workspace.can_redo());
    assert_eq!(workspace.history().commits.len(), 3);
    workspace.validate_integrity().unwrap();
}

#[test]
fn a_new_edit_after_undo_starts_a_new_branch_path_and_clears_redo() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Divergent edit"));
    let first = workspace
        .apply_user_transaction(
            workspace.revision(),
            "First rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let abandoned = workspace
        .apply_user_transaction(
            workspace.revision(),
            "Abandoned rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(2),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    assert_eq!(workspace.undo().unwrap(), first);
    let replacement = workspace
        .apply_user_transaction(
            workspace.revision(),
            "Replacement rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(3),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    assert!(!workspace.can_redo());
    assert_eq!(
        workspace.history().commits[&replacement].parent,
        Some(first)
    );
    assert_eq!(
        workspace
            .document()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(
        workspace
            .history()
            .restore(abandoned)
            .unwrap()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    workspace.validate_integrity().unwrap();
}

#[test]
fn an_in_flight_task_action_cannot_be_undone_past_its_checkpoint() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Task checkpoint"));
    let task_id = workspace.create_task(
        "Create rectangle",
        "Create a rectangle",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![TaskAction {
                intent: "Create rectangle".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create one editable rectangle".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    workspace.apply_next_task_action(task_id).unwrap();

    assert!(!workspace.can_undo());
    assert_eq!(
        workspace.undo(),
        Err(WorkspaceError::HistoryNavigationBlocked(task_id))
    );
    assert!(workspace.document().entities.contains_key(&1));
    workspace.validate_integrity().unwrap();
}

#[test]
fn redo_paths_are_isolated_between_design_branches() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Branch redo isolation"));
    let first = workspace
        .apply_user_transaction(
            workspace.revision(),
            "First rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let main_second = workspace
        .apply_user_transaction(
            workspace.revision(),
            "Main rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(2),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    workspace.undo().unwrap();
    workspace.fork_at("alternative", first).unwrap();
    workspace.checkout_branch("alternative").unwrap();

    assert!(!workspace.can_redo());
    workspace
        .apply_user_transaction(
            workspace.revision(),
            "Alternative rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(3),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    workspace.checkout_branch("main").unwrap();
    assert!(workspace.can_redo());
    assert_eq!(workspace.redo().unwrap(), main_second);
    assert_eq!(
        workspace
            .document()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    workspace.validate_integrity().unwrap();
}

#[test]
fn document_snapshots_remain_bound_to_the_revision_that_created_them() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Snapshot isolation"));
    let original = workspace.snapshot();

    assert_eq!(original.revision(), 0);
    assert!(original.document().entities.is_empty());

    let committed = workspace
        .apply_user_transaction(
            original.revision(),
            "Create rectangle after snapshot",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let current = workspace.snapshot();

    assert_eq!(committed, 1);
    assert_eq!(original.revision(), 0);
    assert!(original.document().entities.is_empty());
    assert_eq!(current.revision(), committed);
    assert!(current.document().entities.contains_key(&1));
    workspace.validate_integrity().unwrap();
}

#[test]
fn stale_user_transaction_through_kernel_facade_is_rejected_atomically() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Stale user edit"));
    let stale = workspace.snapshot();
    let expected_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            expected_revision,
            "Create current rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let before_rejected_edit = workspace.clone();

    let error = workspace
        .kernel()
        .apply_user_transaction(
            stale.revision(),
            "Create stale rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(2),
            }]),
            ValidationReport::default(),
        )
        .unwrap_err();

    assert_eq!(
        error,
        WorkspaceError::StaleRevision {
            expected: stale.revision(),
            actual: before_rejected_edit.revision(),
        }
    );
    assert_eq!(workspace, before_rejected_edit);
    workspace.validate_integrity().unwrap();
}

#[test]
fn prepared_action_commits_after_an_unrelated_object_changes() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Object concurrency"));
    let initial_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            initial_revision,
            "Create independent rectangles",
            CommandTransaction::new(vec![
                CadCommand::CreateEntity {
                    entity: rectangle(1),
                },
                CadCommand::CreateEntity {
                    entity: rectangle(2),
                },
            ]),
            ValidationReport::default(),
        )
        .unwrap();

    let mut first_update = workspace.document().entities[&1].clone();
    first_update.name = "Prepared first rectangle".into();
    let prepared = workspace
        .kernel()
        .prepare_action(CommandTransaction::new(vec![CadCommand::UpdateEntity {
            entity: first_update,
        }]))
        .unwrap();

    let mut second_update = workspace.document().entities[&2].clone();
    second_update.name = "Intervening second rectangle".into();
    let intervening_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            intervening_revision,
            "Update unrelated rectangle",
            CommandTransaction::new(vec![CadCommand::UpdateEntity {
                entity: second_update,
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    let commit_id = workspace
        .kernel()
        .commit_prepared_user_action(
            "Install prepared first rectangle",
            prepared,
            ValidationReport::default(),
        )
        .unwrap();

    assert_eq!(commit_id, 3);
    assert_eq!(
        workspace.document().entities[&1].name,
        "Prepared first rectangle"
    );
    assert_eq!(
        workspace.document().entities[&2].name,
        "Intervening second rectangle"
    );
    let commit = &workspace.history().commits[&commit_id];
    assert_eq!(commit.parent, Some(2));
    assert_eq!(commit.preparation().unwrap().base_revision(), 1);
    workspace.validate_integrity().unwrap();
}

#[test]
fn prepared_action_retry_returns_the_original_commit_without_mutation() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Idempotent retry"));
    let prepared = workspace
        .kernel()
        .prepare_action(CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: rectangle(1),
        }]))
        .unwrap();
    let retry = prepared.clone();
    let first_commit = workspace
        .kernel()
        .commit_prepared_user_action("Create rectangle", prepared, ValidationReport::default())
        .unwrap();
    let after_first_commit = workspace.clone();

    let retry_commit = workspace
        .kernel()
        .commit_prepared_user_action(
            "Retry rectangle creation",
            retry,
            ValidationReport::default(),
        )
        .unwrap();

    assert_eq!(retry_commit, first_commit);
    assert_eq!(workspace, after_first_commit);
    assert_eq!(workspace.history().commits.len(), 2);
}

#[test]
fn prepared_action_retry_on_an_abandoned_branch_reports_conflict() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Idempotency branch conflict"));
    let prepared = workspace
        .kernel()
        .prepare_action(CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: rectangle(1),
        }]))
        .unwrap();
    let retry = prepared.clone();
    let committed = workspace
        .kernel()
        .commit_prepared_user_action("Create rectangle", prepared, ValidationReport::default())
        .unwrap();
    workspace.kernel().undo().unwrap();
    let before_rejection = workspace.clone();

    let error = workspace
        .kernel()
        .commit_prepared_user_action("Retry abandoned action", retry, ValidationReport::default())
        .unwrap_err();

    assert_eq!(
        error,
        WorkspaceError::IdempotencyConflict {
            existing_commit: committed,
            current: 0,
        }
    );
    assert_eq!(workspace, before_rejection);
}

#[test]
fn prepared_action_rejects_a_same_object_change_atomically() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Object conflict"));
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Create rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let mut prepared_update = workspace.document().entities[&1].clone();
    prepared_update.name = "Prepared name".into();
    let prepared = workspace
        .kernel()
        .prepare_action(CommandTransaction::new(vec![CadCommand::UpdateEntity {
            entity: prepared_update,
        }]))
        .unwrap();

    let mut intervening_update = workspace.document().entities[&1].clone();
    intervening_update.name = "Newer name".into();
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Update the same rectangle",
            CommandTransaction::new(vec![CadCommand::UpdateEntity {
                entity: intervening_update,
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let before_rejection = workspace.clone();

    let error = workspace
        .kernel()
        .commit_prepared_user_action(
            "Reject stale prepared update",
            prepared,
            ValidationReport::default(),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        WorkspaceError::ObjectPreconditionFailed {
            expected: ObjectPrecondition {
                object: ObjectId::Entity(1),
                last_modified_revision: Some(1),
                ..
            },
            actual: ObjectPrecondition {
                object: ObjectId::Entity(1),
                last_modified_revision: Some(2),
                ..
            }
        }
    ));
    assert_eq!(workspace, before_rejection);
}

#[test]
fn prepared_action_rejects_dependency_and_aba_changes() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Dependency conflict"));
    let prepared_create = workspace
        .kernel()
        .prepare_action(CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: rectangle(1),
        }]))
        .unwrap();
    let mut layer = workspace.document().layers[&1].clone();
    layer.color = [200, 80, 40, 255];
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Change target layer",
            CommandTransaction::new(vec![CadCommand::UpdateLayer { layer }]),
            ValidationReport::default(),
        )
        .unwrap();
    assert!(matches!(
        workspace.kernel().commit_prepared_user_action(
            "Create on stale layer",
            prepared_create,
            ValidationReport::default(),
        ),
        Err(WorkspaceError::ObjectPreconditionFailed {
            expected: ObjectPrecondition {
                object: ObjectId::Layer(1),
                ..
            },
            ..
        })
    ));

    let prepared_absent = workspace
        .kernel()
        .prepare_action(CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: rectangle(1),
        }]))
        .unwrap();
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Create then delete the same identity",
            CommandTransaction::new(vec![
                CadCommand::CreateEntity {
                    entity: rectangle(1),
                },
                CadCommand::DeleteEntity { id: 1 },
            ]),
            ValidationReport::default(),
        )
        .unwrap();
    assert!(matches!(
        workspace.kernel().commit_prepared_user_action(
            "Reject ABA identity reuse",
            prepared_absent,
            ValidationReport::default(),
        ),
        Err(WorkspaceError::ObjectPreconditionFailed {
            expected: ObjectPrecondition {
                object: ObjectId::Entity(1),
                exists: false,
                last_modified_revision: None,
            },
            actual: ObjectPrecondition {
                object: ObjectId::Entity(1),
                exists: false,
                last_modified_revision: Some(2),
            }
        })
    ));
}

#[test]
fn prepared_action_is_bound_to_its_originating_workspace_state() {
    let mut source = TaskWorkspace::new(CadDocument::new("Source workspace"));
    let prepared = source
        .kernel()
        .prepare_action(CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: rectangle(1),
        }]))
        .unwrap();
    let mut destination = TaskWorkspace::new(CadDocument::new("Different workspace"));
    let before_rejection = destination.clone();

    let error = destination
        .kernel()
        .commit_prepared_user_action(
            "Reject foreign action",
            prepared,
            ValidationReport::default(),
        )
        .unwrap_err();

    assert_eq!(error, WorkspaceError::PreparedInputMismatch { base: 0 });
    assert_eq!(destination, before_rejection);
}

#[test]
fn object_version_snapshot_tracks_tombstones_on_the_active_branch() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Version snapshot"));
    assert_eq!(
        workspace.snapshot().object_precondition(ObjectId::Layer(1)),
        ObjectPrecondition {
            object: ObjectId::Layer(1),
            exists: true,
            last_modified_revision: Some(0),
        }
    );
    assert_eq!(
        workspace
            .snapshot()
            .object_precondition(ObjectId::Entity(1)),
        ObjectPrecondition {
            object: ObjectId::Entity(1),
            exists: false,
            last_modified_revision: None,
        }
    );
    let revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            revision,
            "Create and delete identity",
            CommandTransaction::new(vec![
                CadCommand::CreateEntity {
                    entity: rectangle(1),
                },
                CadCommand::DeleteEntity { id: 1 },
            ]),
            ValidationReport::default(),
        )
        .unwrap();

    assert_eq!(
        workspace
            .snapshot()
            .object_precondition(ObjectId::Entity(1)),
        ObjectPrecondition {
            object: ObjectId::Entity(1),
            exists: false,
            last_modified_revision: Some(1),
        }
    );
}

#[test]
fn paused_plan_resumes_after_an_unrelated_user_revision() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Concurrent paused plan"));
    let task_id = workspace.create_task(
        "Create two rectangles",
        "Create a pair of editable rectangles",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![
                TaskAction {
                    intent: "Create first rectangle".into(),
                    tool_name: "drafting.create_rectangle".into(),
                    detail: "Create the first rectangle".into(),
                    transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                        entity: rectangle(1),
                    }]),
                    validation: ValidationReport::default(),
                },
                TaskAction {
                    intent: "Create second rectangle".into(),
                    tool_name: "drafting.create_rectangle".into(),
                    detail: "Create the second rectangle".into(),
                    transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                        entity: rectangle(2),
                    }]),
                    validation: ValidationReport::default(),
                },
            ],
        )
        .unwrap();
    let first_task_revision = workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.pause_task(task_id, "Awaiting review").unwrap();
    workspace
        .apply_user_transaction(
            workspace.revision(),
            "Create intervening user rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(3),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let user_revision = workspace.revision();

    workspace.resume_task(task_id).unwrap();
    let second_task_revision = workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.complete_task(task_id).unwrap();

    let task = workspace.task(task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    assert_eq!(
        task.output_commits().collect::<Vec<_>>(),
        vec![first_task_revision, second_task_revision]
    );
    assert_eq!(
        workspace.history().commits[&second_task_revision].parent,
        Some(user_revision)
    );
    assert_eq!(task.execution().unwrap().next_action_index(), 2);
    assert!(workspace.document().entities.contains_key(&1));
    assert!(workspace.document().entities.contains_key(&2));
    assert!(workspace.document().entities.contains_key(&3));
    workspace.validate_integrity().unwrap();
}

#[test]
fn plan_from_an_ancestor_snapshot_installs_after_an_unrelated_edit() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Asynchronous planning"));
    let task_id = workspace.create_task(
        "Create rectangle",
        "Create an editable rectangle",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    let planning_revision = workspace.revision();
    workspace
        .apply_user_transaction(
            workspace.revision(),
            "Create unrelated human rectangle",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(3),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let user_revision = workspace.revision();

    workspace
        .set_task_plan(
            task_id,
            planning_revision,
            vec![TaskAction {
                intent: "Create planned rectangle".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create an editable rectangle".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    let commit_id = workspace.apply_next_task_action(task_id).unwrap().unwrap();

    assert_eq!(
        workspace.history().commits[&commit_id].parent,
        Some(user_revision)
    );
    assert_eq!(
        workspace.history().commits[&commit_id]
            .preparation()
            .unwrap()
            .base_revision(),
        planning_revision
    );
    assert!(workspace.document().entities.contains_key(&1));
    assert!(workspace.document().entities.contains_key(&3));
    workspace.validate_integrity().unwrap();
}

#[test]
fn paused_plan_rejects_an_intervening_change_to_its_next_action_object() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Conflicting paused plan"));
    let task_id = workspace.create_task(
        "Create and rename rectangle",
        "Create one editable rectangle and rename it",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    let mut renamed = rectangle(1);
    renamed.name = "Task rename".into();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![
                TaskAction {
                    intent: "Create rectangle".into(),
                    tool_name: "drafting.create_rectangle".into(),
                    detail: "Create the rectangle".into(),
                    transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                        entity: rectangle(1),
                    }]),
                    validation: ValidationReport::default(),
                },
                TaskAction {
                    intent: "Rename rectangle".into(),
                    tool_name: "drafting.update_rectangle".into(),
                    detail: "Rename the rectangle".into(),
                    transaction: CommandTransaction::new(vec![CadCommand::UpdateEntity {
                        entity: renamed,
                    }]),
                    validation: ValidationReport::default(),
                },
            ],
        )
        .unwrap();
    workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.pause_task(task_id, "Awaiting review").unwrap();
    let mut human_update = workspace.document().entities[&1].clone();
    human_update.name = "Human rename".into();
    workspace
        .apply_user_transaction(
            workspace.revision(),
            "Rename rectangle manually",
            CommandTransaction::new(vec![CadCommand::UpdateEntity {
                entity: human_update,
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let before_rejection = workspace.clone();

    let error = workspace.resume_task(task_id).unwrap_err();

    assert!(matches!(
        error,
        WorkspaceError::ObjectPreconditionFailed {
            expected: ObjectPrecondition {
                object: ObjectId::Entity(1),
                last_modified_revision: Some(1),
                ..
            },
            actual: ObjectPrecondition {
                object: ObjectId::Entity(1),
                last_modified_revision: Some(2),
                ..
            }
        }
    ));
    assert_eq!(workspace, before_rejection);
    workspace.validate_integrity().unwrap();
}

#[test]
fn design_task_groups_prompts_runs_and_action_commits_without_losing_partial_work() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Prompt hierarchy"));
    let task_id = workspace.create_task(
        "Develop bracket",
        "Create the bracket base",
        TaskAuthority::all_direct(),
    );
    workspace
        .begin_task_as(task_id, AgentRunIdentity::local("planner-v1"))
        .unwrap();
    workspace.set_task_plan(task_id, 0, Vec::new()).unwrap();
    workspace.complete_task(task_id).unwrap();

    let change_set_id = workspace
        .add_prompt(task_id, "Add one mounting pad", TaskAuthority::all_direct())
        .unwrap();
    workspace
        .begin_task_as(task_id, AgentRunIdentity::local("planner-v2"))
        .unwrap();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![TaskAction {
                intent: "Add mounting pad".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create one editable mounting pad".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    let commit_id = workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace
        .fail_task(task_id, "Downstream feature planning failed")
        .unwrap();

    let failed_run_id = workspace.task(task_id).unwrap().active_run().unwrap().id;
    let retry_run_id = workspace.retry_active_change_set(task_id).unwrap();
    assert_ne!(retry_run_id, failed_run_id);
    workspace
        .begin_task_as(task_id, AgentRunIdentity::local("planner-v2"))
        .unwrap();
    workspace
        .set_task_plan(task_id, workspace.revision(), Vec::new())
        .unwrap();
    workspace.complete_task(task_id).unwrap();

    let task = workspace.task(task_id).unwrap();
    assert_eq!(task.change_sets.len(), 2);
    let change_set = task.active_change_set().unwrap();
    assert_eq!(change_set.id, change_set_id);
    assert_eq!(change_set.status, ChangeSetStatus::Completed);
    assert_eq!(change_set.runs.len(), 2);
    assert_eq!(change_set.runs[0].status, AgentRunStatus::Failed);
    assert_eq!(
        change_set.runs[0].output_commits().collect::<Vec<_>>(),
        vec![commit_id]
    );
    assert_eq!(change_set.runs[1].status, AgentRunStatus::Completed);
    assert_eq!(
        workspace.history().commits[&commit_id].action_source(),
        Some(ActionSource::for_run(task_id, change_set_id, failed_run_id))
    );
    assert_eq!(task.output_commits().collect::<Vec<_>>(), vec![commit_id]);
    workspace.validate_integrity().unwrap();
}

#[test]
fn pause_and_resume_continue_the_same_agent_run() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Resume run identity"));
    let task_id = workspace.create_task(
        "Create two pads",
        "Create two mounting pads",
        TaskAuthority::all_direct(),
    );
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            0,
            vec![TaskAction {
                intent: "Create pad".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create pad".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    let run_id = workspace.task(task_id).unwrap().active_run().unwrap().id;
    workspace.pause_task(task_id, "Yield").unwrap();
    workspace.resume_task(task_id).unwrap();

    assert_eq!(
        workspace.task(task_id).unwrap().active_run().unwrap().id,
        run_id
    );
    assert_eq!(
        workspace
            .task(task_id)
            .unwrap()
            .active_change_set()
            .unwrap()
            .runs
            .len(),
        1
    );
    workspace.validate_integrity().unwrap();
}

#[test]
fn cancelling_a_prompt_preserves_a_retryable_diagnostic() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Cancel prompt"));
    let task_id = workspace.create_task(
        "Create concept",
        "Create a concept",
        TaskAuthority::all_direct(),
    );

    workspace
        .cancel_task(task_id, "User changed direction")
        .unwrap();

    let task = workspace.task(task_id).unwrap();
    let change_set = task.active_change_set().unwrap();
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert_eq!(change_set.status, ChangeSetStatus::Cancelled);
    assert_eq!(change_set.diagnostics.len(), 1);
    assert!(matches!(
        change_set.active_run().unwrap().events.last(),
        Some(TaskEvent::Cancelled { .. })
    ));
    workspace.validate_integrity().unwrap();
    workspace.retry_active_change_set(task_id).unwrap();
    assert_eq!(workspace.task(task_id).unwrap().status, TaskStatus::Queued);
    workspace.validate_integrity().unwrap();
}

#[test]
fn compensating_revert_appends_history_and_preserves_unrelated_user_work() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Compensating revert"));
    let task_id = workspace.create_task(
        "Create pad",
        "Create one mounting pad",
        TaskAuthority::all_direct(),
    );
    let target_change_set_id = workspace.task(task_id).unwrap().active_change_set_id;
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![TaskAction {
                intent: "Create pad".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create pad".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    let target_commit = workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.complete_task(task_id).unwrap();
    let unrelated_commit = workspace
        .apply_user_transaction(
            workspace.revision(),
            "Create unrelated annotation",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(2),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    let report = workspace
        .revert_change_set(task_id, target_change_set_id)
        .unwrap();

    let compensation_commit = report.commit_id.unwrap();
    assert!(compensation_commit > unrelated_commit);
    assert_eq!(
        workspace.history().commits[&compensation_commit].parent,
        Some(unrelated_commit)
    );
    assert_eq!(
        workspace.history().commits[&compensation_commit].action_source(),
        Some(ActionSource::for_run(
            task_id,
            report.compensation_change_set_id,
            workspace.task(task_id).unwrap().active_run().unwrap().id,
        ))
    );
    assert_eq!(report.reverted_objects, vec![ObjectId::Entity(1)]);
    assert!(report.conflicts.is_empty());
    assert!(!workspace.document().entities.contains_key(&1));
    assert!(workspace.document().entities.contains_key(&2));
    assert!(workspace.history().commits.contains_key(&target_commit));
    let task = workspace.task(task_id).unwrap();
    assert_eq!(task.change_sets[0].status, ChangeSetStatus::Reverted);
    assert_eq!(
        task.change_sets[0].reverted_by,
        Some(report.compensation_change_set_id)
    );
    assert_eq!(task.status, TaskStatus::Completed);
    workspace.validate_integrity().unwrap();
}

#[test]
fn compensating_revert_keeps_modified_objects_and_restores_independent_ones() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Partial compensation"));
    let task_id = workspace.create_task(
        "Create pads",
        "Create two mounting pads",
        TaskAuthority::all_direct(),
    );
    let target_change_set_id = workspace.task(task_id).unwrap().active_change_set_id;
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![
                TaskAction {
                    intent: "Create first pad".into(),
                    tool_name: "drafting.create_rectangle".into(),
                    detail: "Create first pad".into(),
                    transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                        entity: rectangle(1),
                    }]),
                    validation: ValidationReport::default(),
                },
                TaskAction {
                    intent: "Create second pad".into(),
                    tool_name: "drafting.create_rectangle".into(),
                    detail: "Create second pad".into(),
                    transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                        entity: rectangle(2),
                    }]),
                    validation: ValidationReport::default(),
                },
            ],
        )
        .unwrap();
    workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.complete_task(task_id).unwrap();
    let mut human_version = workspace.document().entities[&1].clone();
    human_version.name = "Reviewed by human".into();
    let human_commit = workspace
        .apply_user_transaction(
            workspace.revision(),
            "Review first pad",
            CommandTransaction::new(vec![CadCommand::UpdateEntity {
                entity: human_version.clone(),
            }]),
            ValidationReport::default(),
        )
        .unwrap();

    let report = workspace
        .revert_change_set(task_id, target_change_set_id)
        .unwrap();

    assert_eq!(report.reverted_objects, vec![ObjectId::Entity(2)]);
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].object, ObjectId::Entity(1));
    assert_eq!(
        report.conflicts[0].reason,
        RevertConflictReason::ModifiedAfterTarget
    );
    assert_eq!(report.conflicts[0].conflicting_revision, Some(human_commit));
    assert_eq!(workspace.document().entities[&1], human_version);
    assert!(!workspace.document().entities.contains_key(&2));
    assert_eq!(
        workspace.task(task_id).unwrap().change_sets[0].status,
        ChangeSetStatus::RevertedWithConflicts
    );
    workspace.validate_integrity().unwrap();
}

#[test]
fn all_conflict_compensation_records_result_without_an_empty_commit() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("All conflicts"));
    let task_id =
        workspace.create_task("Create pad", "Create one pad", TaskAuthority::all_direct());
    let target_change_set_id = workspace.task(task_id).unwrap().active_change_set_id;
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            0,
            vec![TaskAction {
                intent: "Create pad".into(),
                tool_name: "drafting.create_rectangle".into(),
                detail: "Create pad".into(),
                transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.complete_task(task_id).unwrap();
    let mut reviewed = workspace.document().entities[&1].clone();
    reviewed.name = "Keep this pad".into();
    workspace
        .apply_user_transaction(
            workspace.revision(),
            "Keep pad",
            CommandTransaction::new(vec![CadCommand::UpdateEntity {
                entity: reviewed.clone(),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let head_before_revert = workspace.revision();

    let report = workspace
        .revert_change_set(task_id, target_change_set_id)
        .unwrap();

    assert_eq!(workspace.revision(), head_before_revert);
    assert_eq!(report.commit_id, None);
    assert!(report.reverted_objects.is_empty());
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(workspace.document().entities[&1], reviewed);
    let compensation = workspace
        .task(task_id)
        .unwrap()
        .active_change_set()
        .unwrap();
    assert_eq!(compensation.status, ChangeSetStatus::Completed);
    assert!(compensation.output_commits().next().is_none());
    assert_eq!(compensation.compensation.as_ref().unwrap().commit_id, None);
    workspace.validate_integrity().unwrap();
}

#[test]
fn compensating_revert_orders_cross_object_dependencies_and_locked_layers() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Dependent compensation"));
    let task_id = workspace.create_task(
        "Create constrained feature",
        "Create a parameterized line on a managed layer",
        TaskAuthority::all_direct(),
    );
    let target_change_set_id = workspace.task(task_id).unwrap().active_change_set_id;
    let layer = Layer {
        id: 2,
        name: "Generated".into(),
        visible: true,
        locked: false,
        color: [80, 170, 220, 255],
    };
    let parameter = Parameter {
        id: 1,
        name: "feature_length".into(),
        value: 10.0,
        unit: Units::Millimeters,
        expression: None,
    };
    let entity = Entity {
        id: 1,
        layer: 2,
        name: "Parameterized line".into(),
        visible: true,
        kind: EntityKind::Line {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(10.0, 0.0),
        },
        parameter_refs: BTreeSet::from([1]),
    };
    let constraint = SketchConstraint {
        id: 1,
        name: "Horizontal line".into(),
        driving: true,
        kind: ConstraintKind::Horizontal {
            segment: SketchSegment::new(
                SketchPoint::new(1, PointAnchor::Start),
                SketchPoint::new(1, PointAnchor::End),
            ),
        },
    };
    workspace.begin_task(task_id).unwrap();
    workspace
        .set_task_plan(
            task_id,
            workspace.revision(),
            vec![TaskAction {
                intent: "Create dependent feature".into(),
                tool_name: "mechanical.create_feature".into(),
                detail: "Create layer, parameter, entity, and constraint".into(),
                transaction: CommandTransaction::new(vec![
                    CadCommand::CreateLayer {
                        layer: layer.clone(),
                    },
                    CadCommand::SetParameter { parameter },
                    CadCommand::CreateEntity { entity },
                    CadCommand::CreateConstraint { constraint },
                    CadCommand::UpdateLayer {
                        layer: Layer {
                            locked: true,
                            ..layer
                        },
                    },
                ]),
                validation: ValidationReport::default(),
            }],
        )
        .unwrap();
    workspace.apply_next_task_action(task_id).unwrap().unwrap();
    workspace.complete_task(task_id).unwrap();

    let report = workspace
        .revert_change_set(task_id, target_change_set_id)
        .unwrap();

    assert!(report.conflicts.is_empty());
    assert_eq!(report.reverted_objects.len(), 4);
    assert!(!workspace.document().layers.contains_key(&2));
    assert!(workspace.document().parameters.is_empty());
    assert!(workspace.document().entities.is_empty());
    assert!(workspace.document().constraints.is_empty());
    workspace.validate_integrity().unwrap();
}

#[test]
fn delete_parameter_is_replayable_and_rejects_missing_ids() {
    let mut document = CadDocument::new("Delete parameter");
    let parameter = Parameter {
        id: 1,
        name: "width".into(),
        value: 12.0,
        unit: Units::Millimeters,
        expression: None,
    };
    CommandTransaction::new(vec![CadCommand::SetParameter {
        parameter: parameter.clone(),
    }])
    .apply(&mut document)
    .unwrap();

    let diff = CommandTransaction::new(vec![CadCommand::DeleteParameter { id: 1 }])
        .apply(&mut document)
        .unwrap();

    assert_eq!(diff.deleted_parameters, vec![1]);
    assert!(document.parameters.is_empty());
    assert!(matches!(
        CommandTransaction::new(vec![CadCommand::DeleteParameter { id: 1 }]).apply(&mut document),
        Err(CommandError::InvalidParameter(_))
    ));
}

fn remote_grant_request(
    selected_entity_ids: impl IntoIterator<Item = EntityId>,
) -> RemoteAccessGrantRequest {
    RemoteAccessGrantRequest {
        endpoint: "https://provider.example/v1".into(),
        model: "cad-model".into(),
        allowed_data_categories: BTreeSet::from([
            RemoteDataCategory::TaskGoal,
            RemoteDataCategory::DocumentStatistics,
            RemoteDataCategory::SelectionIdentifiers,
        ]),
        allowed_capabilities: BTreeSet::from([Capability::Drafting]),
        object_scope: RemoteObjectScope::from_selected_entities(selected_entity_ids),
        max_payload_bytes: MAX_REMOTE_CONTEXT_BYTES,
        granted_at_unix_seconds: 100,
        expires_at_unix_seconds: Some(200),
    }
}

#[test]
fn project_remote_grants_are_scoped_timed_and_append_only() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote policy"));
    let project_id = workspace.project_id();
    let grant_id = workspace
        .kernel()
        .create_remote_access_grant(remote_grant_request([7, 9]))
        .unwrap();
    let grant = &workspace.remote_access_grants()[&grant_id];
    let data_categories = BTreeSet::from([RemoteDataCategory::TaskGoal]);
    let capabilities = BTreeSet::from([Capability::Drafting]);
    assert!(grant.authorizes(RemoteAccessCheck {
        project_id,
        endpoint: "https://provider.example/v1",
        model: "cad-model",
        data_categories: &data_categories,
        capabilities: &capabilities,
        selected_entity_ids: &[7],
        payload_bytes: 512,
        unix_seconds: 100,
    }));
    assert!(!grant.authorizes(RemoteAccessCheck {
        project_id,
        endpoint: "https://provider.example/v1",
        model: "cad-model",
        data_categories: &data_categories,
        capabilities: &capabilities,
        selected_entity_ids: &[8],
        payload_bytes: 512,
        unix_seconds: 100,
    }));
    assert!(!grant.is_active_at(200));

    workspace
        .kernel()
        .revoke_remote_access_grant(grant_id, 150)
        .unwrap();
    assert!(!workspace.remote_access_grants()[&grant_id].is_active_at(150));
    assert_eq!(workspace.remote_policy_events().len(), 2);
    workspace.validate_integrity().unwrap();
}

#[test]
fn remote_policy_ledger_tampering_is_rejected() {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Remote policy integrity"));
    let grant_id = workspace
        .kernel()
        .create_remote_access_grant(remote_grant_request([]))
        .unwrap();
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["remote_access_policy"]["grants"][grant_id.to_string()]["revoked_at_unix_seconds"] =
        serde_json::Value::from(120);
    let tampered = serde_json::from_value::<TaskWorkspace>(value).unwrap();

    assert!(matches!(
        tampered.validate_integrity(),
        Err(WorkspaceError::RemotePolicy(
            RemotePolicyError::InvalidLedger(_)
        ))
    ));
}

#[test]
fn workspace_project_identity_round_trips_without_entering_document_history() {
    let workspace = TaskWorkspace::new(CadDocument::new("Project identity"));
    let project_id = workspace.project_id();
    let encoded = serde_json::to_vec(&workspace).unwrap();
    let decoded = serde_json::from_slice::<TaskWorkspace>(&encoded).unwrap();

    assert_eq!(decoded.project_id(), project_id);
    assert_eq!(decoded.document(), workspace.document());
    assert_eq!(decoded.history(), workspace.history());
    decoded.validate_integrity().unwrap();
}

fn iterative_rectangle_action(id: EntityId) -> TaskAction {
    TaskAction {
        intent: format!("Create rectangle {id}"),
        tool_name: "drafting.create_rectangle".into(),
        detail: format!("Create editable rectangle {id}"),
        transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: rectangle(id),
        }]),
        validation: ValidationReport::default(),
    }
}

fn iterative_workspace_with_budget(max_actions: usize) -> (TaskWorkspace, TaskId) {
    let mut workspace = TaskWorkspace::new(CadDocument::new("Bounded iterative planning"));
    let task_id = workspace.kernel().create_task(
        "Bounded task",
        "Create geometry one decision at a time",
        TaskAuthority::all_direct(),
    );
    workspace
        .kernel()
        .begin_iterative_task_as_with_budget(
            task_id,
            AgentRunIdentity::local("bounded-test-planner"),
            TaskPlanningBudget::iterative(max_actions).unwrap(),
        )
        .unwrap();
    (workspace, task_id)
}

#[test]
fn iterative_planning_budget_round_trips_and_caps_actions_and_decisions() {
    let (mut workspace, task_id) = iterative_workspace_with_budget(1);
    let budget = TaskPlanningBudget::iterative(1).unwrap();
    assert_eq!(
        workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .planning_budget(),
        budget
    );

    let revision = workspace.revision();
    workspace
        .kernel()
        .record_iterative_observation(task_id, revision)
        .unwrap();
    assert!(
        workspace
            .kernel()
            .record_iterative_observation(task_id, revision)
            .is_err()
    );

    for rejected_id in 10..13 {
        let rejected = iterative_rectangle_action(rejected_id);
        assert!(
            workspace
                .kernel()
                .reject_iterative_action(
                    task_id,
                    revision,
                    &rejected,
                    ActionFailureKind::ToolRejected,
                    "synthetic rejection",
                )
                .unwrap()
        );
        workspace
            .kernel()
            .record_iterative_observation(task_id, revision)
            .unwrap();
    }

    workspace
        .kernel()
        .stage_iterative_action(task_id, revision, iterative_rectangle_action(1))
        .unwrap();
    workspace
        .kernel()
        .apply_next_task_action(task_id)
        .unwrap()
        .unwrap();

    let revision = workspace.revision();
    workspace
        .kernel()
        .record_iterative_observation(task_id, revision)
        .unwrap();
    let over_budget = iterative_rectangle_action(2);
    let error = workspace
        .kernel()
        .stage_iterative_action(task_id, revision, over_budget.clone())
        .unwrap_err();
    assert!(error.to_string().contains("action budget"));
    assert!(
        workspace
            .kernel()
            .reject_iterative_action(
                task_id,
                revision,
                &over_budget,
                ActionFailureKind::ToolRejected,
                error.to_string(),
            )
            .unwrap()
    );
    let error = workspace
        .kernel()
        .record_iterative_observation(task_id, revision)
        .unwrap_err();
    assert!(error.to_string().contains("planning-decision budget"));

    workspace.validate_integrity().unwrap();
    let encoded = serde_json::to_vec(&workspace).unwrap();
    let decoded = serde_json::from_slice::<TaskWorkspace>(&encoded).unwrap();
    assert_eq!(
        decoded.tasks()[&task_id]
            .execution()
            .unwrap()
            .planning_budget(),
        budget
    );
    decoded.validate_integrity().unwrap();
}

#[test]
fn iterative_action_from_ancestor_observation_merges_after_unrelated_user_edit() {
    let (mut workspace, task_id) = iterative_workspace_with_budget(1);
    let observed_revision = workspace.revision();
    workspace
        .kernel()
        .record_iterative_observation(task_id, observed_revision)
        .unwrap();
    let current_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            current_revision,
            "Create unrelated human geometry",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(3),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let human_revision = workspace.revision();

    workspace
        .kernel()
        .stage_iterative_action(task_id, observed_revision, iterative_rectangle_action(1))
        .unwrap();
    let task_revision = workspace
        .kernel()
        .apply_next_task_action(task_id)
        .unwrap()
        .unwrap();

    assert_eq!(
        workspace.history().commits[&task_revision].parent,
        Some(human_revision)
    );
    assert_eq!(
        workspace.history().commits[&task_revision]
            .preparation()
            .unwrap()
            .base_revision(),
        observed_revision
    );
    assert!(workspace.document().entities.contains_key(&1));
    assert!(workspace.document().entities.contains_key(&3));
    workspace.validate_integrity().unwrap();
}

#[test]
fn iterative_action_from_ancestor_observation_rejects_same_object_change() {
    let (mut workspace, task_id) = iterative_workspace_with_budget(1);
    let observed_revision = workspace.revision();
    workspace
        .kernel()
        .record_iterative_observation(task_id, observed_revision)
        .unwrap();
    let current_revision = workspace.revision();
    workspace
        .kernel()
        .apply_user_transaction(
            current_revision,
            "Human claims planned identity",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        )
        .unwrap();
    let action = iterative_rectangle_action(1);
    workspace
        .kernel()
        .stage_iterative_action(task_id, observed_revision, action.clone())
        .unwrap();

    let error = workspace
        .kernel()
        .apply_next_task_action(task_id)
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::ObjectPreconditionFailed {
            expected: ObjectPrecondition {
                object: ObjectId::Entity(1),
                exists: false,
                ..
            },
            ..
        }
    ));
    assert!(
        workspace
            .kernel()
            .reject_iterative_action(
                task_id,
                observed_revision,
                &action,
                ActionFailureKind::StaleObservation,
                error.to_string(),
            )
            .unwrap()
    );
    assert_eq!(
        workspace.tasks()[&task_id]
            .execution()
            .unwrap()
            .last_failure()
            .unwrap()
            .observed_revision,
        observed_revision
    );
    workspace.validate_integrity().unwrap();
}
