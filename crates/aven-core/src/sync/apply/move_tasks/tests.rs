use super::*;
use crate::choices::TaskSource;
use crate::db::{Database, begin_immediate, field_version};
use crate::ids::{new_id, now};
use crate::metadata::{TaskMetadataInput, resolve_or_create_metadata_field};
use crate::operations::{TaskDraft, TaskUpdate};
use crate::sync::wire::{MoveTaskMetadataSnapshot, MoveTaskSnapshot};
use serde_json::Value;

fn draft(keys: &[&str]) -> TaskDraft {
    TaskDraft {
        title: "Preserve provenance".into(),
        description: String::new(),
        project: Some("Source".into()),
        status: Some("todo".into()),
        priority: "none".into(),
        source: TaskSource::Cli,
        labels: Vec::new(),
        metadata: keys
            .iter()
            .copied()
            .map(|key| TaskMetadataInput {
                expected_field_id: None,
                key: key.into(),
                value: "original".into(),
            })
            .collect(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

async fn conflicts(conn: &mut SqliteConnection) -> Vec<Value> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT json_object('id',id,'workspace_id',workspace_id,'entity_type',entity_type,
         'entity_id',entity_id,'task_id',task_id,'field',field,'base_version',base_version,
         'local_value',local_value,'remote_value',remote_value,'local_change_id',local_change_id,
         'remote_change_id',remote_change_id,'variant_a',variant_a,'variant_b',variant_b,
         'created_at',created_at,'resolved',resolved) FROM conflicts ORDER BY id",
    )
    .fetch_all(conn)
    .await
    .unwrap();
    rows.into_iter()
        .map(|row| serde_json::from_str(&row).unwrap())
        .collect()
}

#[tokio::test]
async fn relocation_preserves_complete_conflicts_versions_and_metadata_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("move.sqlite"))
        .await
        .unwrap();
    let source = db.list_workspaces().await.unwrap().remove(0);
    let target = db.create_workspace("Target").await.unwrap();
    let source_project = db
        .create_project(&source, "Source", None)
        .await
        .unwrap()
        .project;
    let target_project = db
        .create_project(&target, "Target", None)
        .await
        .unwrap()
        .project;
    let task = db
        .create_task(&source, draft(&["owner", "removed"]))
        .await
        .unwrap()
        .task;
    db.update_task(
        &source,
        &task.id,
        TaskUpdate {
            title: Some("Updated provenance".into()),
            remove_metadata: vec!["removed".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut conn = db.acquire_writer().await.unwrap();
    let source_owner = resolve_or_create_metadata_field(&mut conn, &source, "owner")
        .await
        .unwrap();
    let target_owner = resolve_or_create_metadata_field(&mut conn, &target, "owner")
        .await
        .unwrap();
    assert_ne!(source_owner.id, target_owner.id);
    let removed = resolve_or_create_metadata_field(&mut conn, &source, "removed")
        .await
        .unwrap();
    let conflict_only = resolve_or_create_metadata_field(&mut conn, &source, "conflict_only")
        .await
        .unwrap();
    let owner_version = field_version(
        &mut conn,
        &task.id,
        &format!("metadata:{}", source_owner.id),
    )
    .await
    .unwrap();
    let removed_version = field_version(&mut conn, &task.id, &format!("metadata:{}", removed.id))
        .await
        .unwrap();
    let title_version = field_version(&mut conn, &task.id, "title").await.unwrap();
    assert!(title_version.is_some());
    assert!(owner_version.is_some());
    assert!(removed_version.is_some());
    for (field, resolved) in [
        ("title".to_string(), 1),
        (format!("metadata:{}", source_owner.id), 1),
        (format!("metadata:{}", conflict_only.id), 0),
    ] {
        let value = |value: &str| {
            if field.starts_with("metadata:") {
                crate::metadata::encode_metadata_conflict_value(Some(value)).unwrap()
            } else {
                value.into()
            }
        };
        let local_value = value("local provenance");
        let remote_value = value("remote provenance");
        sqlx::query("INSERT INTO conflicts(workspace_id,entity_type,entity_id,task_id,field,
            base_version,local_value,remote_value,local_change_id,remote_change_id,variant_a,variant_b,created_at,resolved)
            VALUES (?,'task',?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&source.id).bind(&task.id).bind(&task.id).bind(field)
            .bind(new_id()).bind(local_value).bind(remote_value)
            .bind(new_id()).bind(new_id()).bind("variant-a").bind("variant-b").bind(now()).bind(resolved)
            .execute(&mut *conn).await.unwrap();
    }
    let original = conflicts(&mut conn).await;
    let mut payload = MoveTasksPayload {
        source_workspace_id: source.id.clone(),
        target_workspace_id: target.id.clone(),
        project_id: target_project.id.clone(),
        project_key: target_project.key.clone(),
        project_name: target_project.name.clone(),
        project_prefix: target_project.prefix.clone(),
        tasks: vec![MoveTaskSnapshot {
            task_id: task.id.clone(),
            labels: Vec::new(),
            metadata: vec![
                MoveTaskMetadataSnapshot {
                    field_id: source_owner.id.clone(),
                    key: "owner".into(),
                    value: Some("original".into()),
                    version: owner_version.clone(),
                },
                MoveTaskMetadataSnapshot {
                    field_id: removed.id.clone(),
                    key: "removed".into(),
                    value: None,
                    version: removed_version.clone(),
                },
            ],
        }],
        dependencies: Vec::new(),
        related: Vec::new(),
        epics: Vec::new(),
    };
    for returning in [false, false, true] {
        if returning {
            payload.source_workspace_id = target.id.clone();
            payload.target_workspace_id = source.id.clone();
            payload.project_id = source_project.id.clone();
            payload.project_key = source_project.key.clone();
            payload.project_name = source_project.name.clone();
            payload.project_prefix = source_project.prefix.clone();
        }
        let change = ChangeWire {
            change_id: new_id(),
            client_id: "peer".into(),
            local_seq: 1,
            entity_type: "task_move".into(),
            entity_id: new_id(),
            field: None,
            op_type: "move_tasks".into(),
            payload: serde_json::to_value(&payload).unwrap(),
            base_version: None,
            created_at: now(),
            server_seq: Some(1),
        };
        let mut tx = begin_immediate(&mut conn).await.unwrap();
        apply_move_tasks(&mut tx, &change).await.unwrap();
        let mut expected = original.clone();
        for row in &mut expected {
            row["workspace_id"] = serde_json::to_value(&payload.target_workspace_id).unwrap();
            if let Some(field) = row["field"].as_str().unwrap().strip_prefix("metadata:") {
                let key: String = sqlx::query_scalar("SELECT key FROM metadata_fields WHERE id=?")
                    .bind(field)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                let id: String = sqlx::query_scalar(
                    "SELECT id FROM metadata_fields WHERE workspace_id=? AND key=?",
                )
                .bind(&payload.target_workspace_id)
                .bind(key)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                row["field"] = Value::String(format!("metadata:{id}"));
            }
        }
        assert_eq!(conflicts(&mut tx).await, expected);
        assert_eq!(
            field_version(&mut tx, &task.id, "title").await.unwrap(),
            title_version
        );
        assert_eq!(
            field_version(&mut tx, &task.id, "project").await.unwrap(),
            Some(change.change_id)
        );
        for (key, version) in [("owner", &owner_version), ("removed", &removed_version)] {
            let id: String =
                sqlx::query_scalar("SELECT id FROM metadata_fields WHERE workspace_id=? AND key=?")
                    .bind(&payload.target_workspace_id)
                    .bind(key)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            assert_eq!(
                field_version(&mut tx, &task.id, &format!("metadata:{id}"))
                    .await
                    .unwrap(),
                *version
            );
        }
        let live: i64 = sqlx::query_scalar("SELECT count(*) FROM task_metadata WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(live, 1);
        tx.commit().await.unwrap();
    }
    let alias: String = sqlx::query_scalar("SELECT local_field_id FROM metadata_field_id_aliases WHERE workspace_id=? AND remote_field_id=?")
        .bind(&target.id).bind(&source_owner.id).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(alias, target_owner.id.as_str());
}

#[tokio::test]
async fn unversioned_live_value_clears_an_earlier_aliased_tombstone_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("aliases.sqlite"))
        .await
        .unwrap();
    let source = db.list_workspaces().await.unwrap().remove(0);
    let target = db.create_workspace("Target").await.unwrap();
    db.create_project(&source, "Source", None).await.unwrap();
    let project = db
        .create_project(&target, "Target", None)
        .await
        .unwrap()
        .project;
    let task = db
        .create_task(&source, draft(&["a", "z"]))
        .await
        .unwrap()
        .task;
    db.update_task(
        &source,
        &task.id,
        TaskUpdate {
            remove_metadata: vec!["a".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut conn = db.acquire_writer().await.unwrap();
    let a = resolve_or_create_metadata_field(&mut conn, &source, "a")
        .await
        .unwrap();
    let z = resolve_or_create_metadata_field(&mut conn, &source, "z")
        .await
        .unwrap();
    let destination = resolve_or_create_metadata_field(&mut conn, &target, "a")
        .await
        .unwrap();
    super::super::metadata::ensure_remote_field(&mut conn, &target.id, &a.id, "a", &now())
        .await
        .unwrap();
    crate::metadata::rename_metadata_field(&mut conn, &target, "a", "z")
        .await
        .unwrap();
    let mapped_z =
        super::super::metadata::ensure_remote_field(&mut conn, &target.id, &z.id, "z", &now())
            .await
            .unwrap();
    assert_eq!(mapped_z.id, destination.id);
    let removed_version = field_version(&mut conn, &task.id, &format!("metadata:{}", a.id))
        .await
        .unwrap();
    assert!(removed_version.is_some());
    // Imported live metadata can have no field-version row.
    sqlx::query("DELETE FROM field_versions WHERE entity_type='task' AND entity_id=? AND field=?")
        .bind(&task.id)
        .bind(format!("metadata:{}", z.id))
        .execute(&mut *conn)
        .await
        .unwrap();
    let payload = MoveTasksPayload {
        source_workspace_id: source.id,
        target_workspace_id: target.id.clone(),
        project_id: project.id,
        project_key: project.key,
        project_name: project.name,
        project_prefix: project.prefix,
        tasks: vec![MoveTaskSnapshot {
            task_id: task.id.clone(),
            labels: Vec::new(),
            metadata: vec![
                MoveTaskMetadataSnapshot {
                    field_id: a.id,
                    key: "a".into(),
                    value: None,
                    version: removed_version,
                },
                MoveTaskMetadataSnapshot {
                    field_id: z.id,
                    key: "z".into(),
                    value: Some("original".into()),
                    version: None,
                },
            ],
        }],
        dependencies: Vec::new(),
        related: Vec::new(),
        epics: Vec::new(),
    };
    let change = ChangeWire {
        change_id: new_id(),
        client_id: "peer".into(),
        local_seq: 1,
        entity_type: "task_move".into(),
        entity_id: new_id(),
        field: None,
        op_type: "move_tasks".into(),
        payload: serde_json::to_value(payload).unwrap(),
        base_version: None,
        created_at: now(),
        server_seq: Some(1),
    };
    let mut tx = begin_immediate(&mut conn).await.unwrap();
    apply_move_tasks(&mut tx, &change).await.unwrap();
    assert_eq!(
        field_version(&mut tx, &task.id, &format!("metadata:{}", destination.id))
            .await
            .unwrap(),
        None
    );
    let update = ChangeWire {
        change_id: new_id(),
        entity_type: "task".into(),
        entity_id: task.id.to_string(),
        field: Some(format!("metadata:{}", destination.id)),
        op_type: "set_task_metadata".into(),
        payload: serde_json::json!({"workspace_id":target.id,"workspace_key":target.key,"field_id":destination.id,"key":"z","value":"edited"}),
        ..change
    };
    super::super::metadata::set_task_value(&mut tx, &update)
        .await
        .unwrap();
    assert!(conflicts(&mut tx).await.is_empty());
}
