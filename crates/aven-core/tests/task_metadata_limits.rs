use aven_core::choices::TaskSource;
use aven_core::db::Database;
use aven_core::metadata::TaskMetadataInput;
use aven_core::operations::{TaskDraft, TaskUpdate};
use aven_core::sync::wire::{MAX_PULL_BATCH, MAX_PUSH_BATCH, SYNC_PROTOCOL_VERSION, SyncResponse};
use aven_core::sync::{ApplySyncPage, ServerSyncPage};

fn input(key: impl Into<String>, value: impl Into<String>) -> TaskMetadataInput {
    TaskMetadataInput {
        expected_field_id: None,
        key: key.into(),
        value: value.into(),
    }
}

fn draft(count: usize, value: &str) -> TaskDraft {
    TaskDraft {
        title: "metadata boundaries".into(),
        description: String::new(),
        project: Some("app".into()),
        status: "todo".into(),
        priority: "none".into(),
        source: TaskSource::Cli,
        labels: vec![],
        metadata: (0..count)
            .map(|i| input(format!("key{i}"), value))
            .collect(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

async fn snapshot(db: &Database) -> serde_json::Value {
    serde_json::to_value(db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap()).unwrap()
}

#[tokio::test]
async fn local_creation_and_addition_limits_are_atomic() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("local.sqlite"))
        .await
        .unwrap();
    let w = db.list_workspaces().await.unwrap().remove(0);
    for (count, value, accepted) in [
        (128, "x".into(), true),
        (129, "x".into(), false),
        (8, "x".repeat(4096), true),
        (9, "x".repeat(4096), false),
        (1, "x".repeat(4097), false),
        (1, "é".repeat(2048), true),
        (1, format!("{}x", "é".repeat(2048)), false),
    ] {
        let before = snapshot(&db).await;
        let result = db.create_task(&w, draft(count, &value)).await;
        if accepted {
            result.unwrap();
        } else {
            assert!(result.is_err());
            assert_eq!(snapshot(&db).await, before);
        }
    }
    for (count, value, error) in [
        (128, "x".into(), "too-many-metadata-values"),
        (8, "x".repeat(4096), "metadata-values-too-large"),
    ] {
        let task = db.create_task(&w, draft(count, &value)).await.unwrap().task;
        let before = snapshot(&db).await;
        let result = db
            .update_task(
                &w,
                &task.id,
                TaskUpdate {
                    title: Some("must roll back".into()),
                    set_metadata: vec![input("new-field", "x")],
                    ..Default::default()
                },
            )
            .await;
        assert!(result.err().unwrap().to_string().contains(error));
        assert_eq!(snapshot(&db).await, before);
    }
}

async fn drain(db: &Database, server: &Database) {
    for _ in 0..16 {
        let page = db
            .prepare_client_sync_page("https://sync.test".into(), MAX_PUSH_BATCH, MAX_PULL_BATCH)
            .await
            .unwrap();
        let result = server
            .persist_server_sync_page(ServerSyncPage {
                request: page.request.clone(),
            })
            .await
            .unwrap();
        let response = SyncResponse {
            protocol_version: SYNC_PROTOCOL_VERSION,
            cursor: result
                .changes
                .last()
                .and_then(|change| change.server_seq)
                .unwrap_or(page.request.after),
            has_more: result.has_more,
            push_acks: result.push_acks,
            changes: result.changes,
        };
        let has_more = response.has_more;
        db.apply_client_sync_page(ApplySyncPage {
            request: page.request,
            sync_generation: page.sync_generation,
            response,
            attempted_at: "2026-09-22T00:00:00Z".into(),
        })
        .await
        .unwrap();
        if !has_more && db.sync_persistence_status().await.unwrap().pending_changes == 0 {
            return;
        }
    }
    panic!("plaintext sync did not drain");
}

#[tokio::test]
async fn plaintext_merge_conflicts_and_capture_preserve_over_limit_metadata() {
    for (base_count, value) in [(127, "x".into()), (7, "x".repeat(4096))] {
        for reverse in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let a = Database::open(&root.path().join("a.sqlite")).await.unwrap();
            let b = Database::open(&root.path().join("b.sqlite")).await.unwrap();
            let server = Database::open(&root.path().join("server.sqlite"))
                .await
                .unwrap();
            let w = a.list_workspaces().await.unwrap().remove(0);
            let task = a
                .create_task(&w, draft(base_count, &value))
                .await
                .unwrap()
                .task;
            drain(&a, &server).await;
            drain(&b, &server).await;
            for (db, key) in [(&a, "extra-a"), (&b, "extra-b")] {
                db.update_task(
                    &w,
                    &task.id,
                    TaskUpdate {
                        set_metadata: vec![input(key, &value)],
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            }
            let devices = if reverse { [&b, &a] } else { [&a, &b] };
            for db in [devices[0], devices[1], devices[0]] {
                drain(db, &server).await;
            }
            let merged = a.task_metadata(&w.id, &task.id).await.unwrap();
            assert_eq!(merged.len(), base_count + 2);
            assert!(merged.iter().all(|v| v.value == value));
            assert_eq!(merged, b.task_metadata(&w.id, &task.id).await.unwrap());
            for db in [&a, &b] {
                let before = snapshot(db).await;
                let error = db
                    .update_task(
                        &w,
                        &task.id,
                        TaskUpdate {
                            title: Some("must roll back".into()),
                            set_metadata: vec![input("extra-growth", "x")],
                            ..Default::default()
                        },
                    )
                    .await
                    .err()
                    .unwrap();
                assert!(error.to_string().contains(if base_count == 127 {
                    "too-many-metadata-values"
                } else {
                    "metadata-values-too-large"
                }));
                assert_eq!(snapshot(db).await, before);
                let export = db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap();
                db.validate_import_data(&export).await.unwrap();
            }
            // Durable capture/resume/install of real plaintext merged state.
            let capture = a
                .capture_local_shared_state_never_dispatched()
                .await
                .unwrap();
            let target = Database::open(&root.path().join("target.sqlite"))
                .await
                .unwrap();
            target
                .install_shared_state(capture.shared_state())
                .await
                .unwrap();
            let resumed = a
                .resume_local_shared_state_never_dispatched()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(capture.candidate_id(), resumed.candidate_id());
            assert_eq!(merged, target.task_metadata(&w.id, &task.id).await.unwrap());
            a.cancel_local_shared_state_never_dispatched(capture.candidate_id())
                .await
                .unwrap();

            // Concurrent bounded replacements produce real conflicts while over-limit.
            for (db, letter) in [(&a, "a"), (&b, "b")] {
                db.update_task(
                    &w,
                    &task.id,
                    TaskUpdate {
                        set_metadata: vec![input("key0", letter.repeat(value.len()))],
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            }
            for db in [&a, &b, &a] {
                drain(db, &server).await;
            }
            let conflicts = a.task_conflicts(&w, &task.id, None).await.unwrap();
            assert_eq!(conflicts.len(), 1);
            a.resolve_conflict(
                &w,
                &task.id,
                &conflicts[0].field,
                &conflicts[0].remote_value,
            )
            .await
            .unwrap();
            for db in [&a, &b, &a] {
                drain(db, &server).await;
            }
            assert_eq!(
                a.task_metadata(&w.id, &task.id).await.unwrap(),
                b.task_metadata(&w.id, &task.id).await.unwrap()
            );

            // A removal variant resolves without requiring unrelated key removal.
            a.update_task(
                &w,
                &task.id,
                TaskUpdate {
                    remove_metadata: vec!["key1".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            b.update_task(
                &w,
                &task.id,
                TaskUpdate {
                    set_metadata: vec![input("key1", "b".repeat(value.len()))],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            for db in [&a, &b, &a] {
                drain(db, &server).await;
            }
            let conflict = b
                .task_conflicts(&w, &task.id, None)
                .await
                .unwrap()
                .remove(0);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&conflict.remote_value).unwrap()["present"],
                false
            );
            b.resolve_conflict(&w, &task.id, &conflict.field, &conflict.remote_value)
                .await
                .unwrap();
            for db in [&b, &a, &b] {
                drain(db, &server).await;
            }
            let final_values = a.task_metadata(&w.id, &task.id).await.unwrap();
            assert_eq!(final_values.len(), base_count + 1);
            assert_eq!(
                final_values,
                b.task_metadata(&w.id, &task.id).await.unwrap()
            );
            assert!(!final_values.iter().any(|v| v.key == "key1"));
            for db in [&a, &b] {
                drain(db, &server).await;
                assert_eq!(
                    db.sync_persistence_status().await.unwrap().pending_changes,
                    0
                );
                assert!(
                    db.task_conflicts(&w, &task.id, None)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
        }
    }
}

#[tokio::test]
async fn create_task_wire_metadata_bounds_remain_record_local() {
    use aven_core::ids::MetadataFieldId;
    use aven_core::sync::wire::{ChangeWire, validate_pushed_change};
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("wire.sqlite"))
        .await
        .unwrap();
    let w = db.list_workspaces().await.unwrap().remove(0);
    db.create_task(&w, draft(1, "x")).await.unwrap();
    let row = db
        .export_data("2026-09-22T00:00:00Z".into())
        .await
        .unwrap()
        .tables
        .changes
        .into_iter()
        .find(|row| row.op_type == "create_task")
        .unwrap();
    let mut encoded = serde_json::to_value(row).unwrap();
    encoded["payload"] = serde_json::from_str(encoded["payload"].as_str().unwrap()).unwrap();
    let mut change: ChangeWire = serde_json::from_value(encoded).unwrap();
    for (count, value, accepted) in [
        (128, "x".into(), true),
        (129, "x".into(), false),
        (8, "é".repeat(2048), true),
        (9, "é".repeat(2048), false),
        (1, "x".repeat(4097), false),
        (1, format!("{}x", "é".repeat(2048)), false),
    ] {
        change.payload["metadata"] =
            serde_json::json!((0..count).map(|i| serde_json::json!({
            "field_id": MetadataFieldId::new(), "key": format!("key{i}"), "value": value,
        })).collect::<Vec<_>>());
        assert_eq!(validate_pushed_change(&change).is_ok(), accepted);
    }
    let field = MetadataFieldId::new();
    change.op_type = "set_task_metadata".into();
    change.field = Some(format!("metadata:{field}"));
    change.payload = serde_json::json!({
        "workspace_id": w.id, "workspace_key": w.key,
        "field_id": field, "key": "key0", "value": "",
    });
    for (value, accepted) in [
        ("x".repeat(4096), true),
        ("x".repeat(4097), false),
        ("é".repeat(2048), true),
        (format!("{}x", "é".repeat(2048)), false),
    ] {
        change.payload["value"] = value.into();
        assert_eq!(validate_pushed_change(&change).is_ok(), accepted);
    }
}

#[tokio::test]
async fn portable_metadata_validation_keeps_value_identity_and_reference_checks() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("portable.sqlite"))
        .await
        .unwrap();
    let w = db.list_workspaces().await.unwrap().remove(0);
    db.create_task(&w, draft(1, "x")).await.unwrap();
    let valid = snapshot(&db).await;
    // Synthetic invalid exports test retained-state invariants, not sync acceptance.
    for defect in ["value", "task", "field", "identity", "key"] {
        let mut invalid = valid.clone();
        match defect {
            "value" => invalid["tables"]["task_metadata"][0]["value"] = "é".repeat(2049).into(),
            "task" => invalid["tables"]["task_metadata"][0]["task_id"] = "AAAAAAAAAAAAAAAA".into(),
            "field" => {
                invalid["tables"]["task_metadata"][0]["field_id"] = "AAAAAAAAAAAAAAAA".into()
            }
            "identity" => {
                let duplicate = invalid["tables"]["task_metadata"][0].clone();
                invalid["tables"]["task_metadata"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            "key" => invalid["tables"]["metadata_fields"][0]["key"] = "aven.private".into(),
            _ => unreachable!(),
        }
        let export = serde_json::from_value(invalid).unwrap();
        assert!(db.validate_import_data(&export).await.is_err(), "{defect}");
        assert!(db.import_data(&export).await.is_err(), "{defect}");
        assert_eq!(snapshot(&db).await, valid);
    }
}
