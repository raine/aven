use std::io::Cursor;
use std::time::Duration as StdDuration;

use chrono::{Duration, TimeZone, Utc};
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde_json::Value;

use crate::sync::ApplySyncPage;
use crate::sync::wire::{ChangeWire, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse};

use super::*;
use crate::choices::TaskSource;
use crate::ids::TaskId;
use crate::operations::{
    CreateRecurrenceSeriesParams, RecurrenceSeriesDraft, TaskCreationUndo, TaskDraft, TaskUpdate,
};
use crate::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule};
use crate::undo::UndoContext;
use crate::workspaces::Workspace;

async fn fresh() -> (tempfile::TempDir, Database, Workspace) {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(&dir.path().join("db.sqlite")).await.unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    (dir, database, workspace)
}

fn png_bytes_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::new(width, height))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn png_bytes() -> Vec<u8> {
    png_bytes_with_dimensions(2, 1)
}

fn draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.into(),
        description: "description".into(),
        project: Some("app".into()),
        status: "todo".into(),
        priority: "none".into(),
        source: TaskSource::Cli,
        labels: vec![],
        metadata: vec![],
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

async fn task(database: &Database, workspace: &Workspace, title: &str) -> TaskId {
    database
        .create_task(workspace, draft(title))
        .await
        .unwrap()
        .task
        .id
}

fn wire(row: &crate::data_safety::ChangeRow) -> ChangeWire {
    let mut value = serde_json::to_value(row).unwrap();
    value["payload"] = serde_json::from_str(&row.payload).unwrap();
    serde_json::from_value(value).unwrap()
}

async fn apply_remote_attachment_change(
    database: &Database,
    workspace: &Workspace,
    task_id: &TaskId,
    attachment_id: &str,
    sha256: &str,
    delete: bool,
) {
    let export = database
        .export_data("2026-09-21T12:00:00Z".into())
        .await
        .unwrap();
    let mut change = wire(&export.tables.changes[0]);
    change.change_id = crate::ids::new_id();
    change.entity_type = "task".into();
    change.entity_id = task_id.to_string();
    change.field = Some("attachments".into());
    change.op_type = if delete {
        "attachment_delete".into()
    } else {
        "attachment_add".into()
    };
    change.base_version = None;
    change.created_at = "2026-09-21T12:00:00Z".into();
    change.payload = if delete {
        serde_json::json!({
            "workspace_id": workspace.id,
            "workspace_key": workspace.key,
            "attachment_id": attachment_id,
            "deleted_at": "2026-09-21T12:01:00Z"
        })
    } else {
        serde_json::json!({
            "workspace_id": workspace.id,
            "workspace_key": workspace.key,
            "attachment_id": attachment_id,
            "sha256": sha256,
            "byte_size": 7,
            "media_type": "image/png",
            "filename": "remote.png",
            "alt_text": null,
            "width": 1,
            "height": 1,
            "created_at": "2026-09-21T12:00:00Z"
        })
    };
    let after = database
        .meta("sync_cursor")
        .await
        .unwrap()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    change.server_seq = Some(after + 1);
    database
        .apply_client_sync_page(ApplySyncPage {
            sync_generation: 0,
            request: SyncRequest {
                protocol_version: Some(SYNC_PROTOCOL_VERSION),
                client_id: "remote".into(),
                after,
                pull_limit: Some(512),
                changes: vec![],
            },
            response: SyncResponse {
                protocol_version: SYNC_PROTOCOL_VERSION,
                changes: vec![change],
                push_acks: vec![],
                cursor: after + 1,
                has_more: false,
            },
            attempted_at: "2026-09-21T12:00:00Z".into(),
            previous_pushed: 0,
            previous_pulled: 0,
        })
        .await
        .unwrap();
}

struct TestStream {
    next: i64,
}

impl TestStream {
    async fn send(&mut self, from: &Database, replicas: &[&Database], attempted_at: &str) {
        let export = from.export_data(attempted_at.into()).await.unwrap();
        let mut pending = export
            .tables
            .changes
            .iter()
            .filter(|row| row.server_seq.is_none())
            .collect::<Vec<_>>();
        pending.sort_by_key(|row| (row.local_seq, &row.created_at, &row.change_id));
        let changes = pending
            .into_iter()
            .map(|row| {
                self.next += 1;
                let mut change = wire(row);
                change.server_seq = Some(self.next);
                change
            })
            .collect::<Vec<_>>();
        if changes.is_empty() {
            return;
        }
        for replica in replicas {
            let after = replica
                .meta("sync_cursor")
                .await
                .unwrap()
                .unwrap()
                .parse()
                .unwrap();
            replica
                .apply_client_sync_page(ApplySyncPage {
                    sync_generation: 0,
                    request: SyncRequest {
                        protocol_version: Some(SYNC_PROTOCOL_VERSION),
                        client_id: "ZZZZZZZZZZZZZZZZ".into(),
                        after,
                        pull_limit: Some(512),
                        changes: vec![],
                    },
                    response: SyncResponse {
                        protocol_version: SYNC_PROTOCOL_VERSION,
                        changes: changes.clone(),
                        push_acks: vec![],
                        cursor: self.next,
                        has_more: false,
                    },
                    attempted_at: attempted_at.into(),
                    previous_pushed: 0,
                    previous_pulled: 0,
                })
                .await
                .unwrap();
        }
    }
}

async fn establish_test_prefix(
    source: &Database,
    installed: &Database,
    capture: &SharedStateCapture,
) -> TestStream {
    let count = i64::try_from(capture.snapshot.tables.changes.len()).unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    let mut tx = db::begin_immediate(&mut conn).await.unwrap();
    sqlx::query("UPDATE changes SET server_seq = NULL")
        .execute(&mut *tx)
        .await
        .unwrap();
    for row in &capture.snapshot.tables.changes {
        sqlx::query("UPDATE changes SET server_seq = ? WHERE change_id = ?")
            .bind(row.server_seq)
            .bind(&row.change_id)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM shared_history_provenance")
        .execute(&mut *tx)
        .await
        .unwrap();
    tables::import_shared_history_provenance(
        &mut tx,
        &capture.snapshot.tables.shared_history_provenance,
    )
    .await
    .unwrap();
    crate::epic_membership::recover(&mut tx, true)
        .await
        .unwrap();
    db::set_meta(&mut tx, "sync_cursor", &count.to_string())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut conn = installed.acquire_writer().await.unwrap();
    db::set_meta(&mut conn, "sync_cursor", &count.to_string())
        .await
        .unwrap();
    TestStream { next: count }
}

fn normalized_shared(capture: SharedStateCapture) -> Value {
    let mut value = serde_json::to_value(capture.snapshot.tables).unwrap();
    for task in value["tasks"].as_array_mut().unwrap() {
        task.as_object_mut().unwrap().remove("updated_at");
        task.as_object_mut().unwrap().remove("queue_activity_at");
    }
    for change in value["changes"].as_array_mut().unwrap() {
        let payload: Value = serde_json::from_str(change["payload"].as_str().unwrap()).unwrap();
        if matches!(
            change["op_type"].as_str(),
            Some("create_task" | "project_recurrence_occurrence")
        ) && payload.get("series_id").is_some()
        {
            // Deterministic recurrence changes can be generated independently.
            // Existing duplicate equality preserves their canonical meaning while
            // allowing each replica's author and local counter to differ.
            change.as_object_mut().unwrap().remove("client_id");
            change.as_object_mut().unwrap().remove("local_seq");
        }
    }
    for rows in value.as_object_mut().unwrap().values_mut() {
        rows.as_array_mut()
            .unwrap()
            .sort_by_key(|row| row.to_string());
    }
    value
}

async fn exported(database: &Database) -> Value {
    serde_json::to_value(
        database
            .export_data("2026-09-21T12:00:00Z".into())
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn capture_install_preserves_boundary_identity_and_private_exclusions() {
    let (_source_dir, source, workspace) = fresh().await;
    let (_target_dir, target, _) = fresh().await;
    let captured_task = task(&source, &workspace, "captured").await;
    let later_task = task(&source, &workspace, "later").await;
    {
        let mut conn = source.acquire_writer().await.unwrap();
        let accepted_id: String =
            sqlx::query_scalar("SELECT change_id FROM changes ORDER BY local_seq LIMIT 1")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        sqlx::query("UPDATE changes SET server_seq = 10 WHERE change_id = ?")
            .bind(accepted_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        db::set_meta(&mut conn, "private_test_credential", "do-not-copy")
            .await
            .unwrap();
        db::set_meta(&mut conn, "sync_cursor", "987").await.unwrap();
        sqlx::query(
            "INSERT INTO project_paths(workspace_id, project_id, path)
             SELECT workspace_id, id, '/private/source/path' FROM projects LIMIT 1",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let source_before = exported(&source).await;
    let capture = source.capture_shared_state().await.unwrap();
    assert_eq!(source_before, exported(&source).await);
    source
        .update_task(
            &workspace,
            &later_task,
            TaskUpdate {
                title: Some("post-capture source edit".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        source
            .export_data("2026-09-21T12:01:00Z".into())
            .await
            .unwrap()
            .tables
            .changes
            .len(),
        capture.snapshot.tables.changes.len() + 1
    );
    assert_eq!(
        capture
            .snapshot
            .tables
            .changes
            .iter()
            .filter_map(|row| row.server_seq)
            .collect::<Vec<_>>(),
        (1..=capture.snapshot.tables.changes.len() as i64).collect::<Vec<_>>()
    );
    assert!(
        capture
            .snapshot
            .tables
            .shared_history_provenance
            .iter()
            .any(|row| row.source_server_seq == Some(10))
    );
    assert!(
        capture
            .snapshot
            .tables
            .shared_history_provenance
            .iter()
            .any(|row| row.source_pending_rank.is_some())
    );

    let target_identity = target.meta("client_id").await.unwrap().unwrap();
    target.install_shared_state(&capture).await.unwrap();
    assert_eq!(
        target.meta("client_id").await.unwrap().unwrap(),
        target_identity
    );
    assert_eq!(
        target.meta("sync_cursor").await.unwrap().as_deref(),
        Some("0")
    );
    assert_eq!(target.meta("private_test_credential").await.unwrap(), None);
    let installed = target
        .export_data("2026-09-21T12:01:00Z".into())
        .await
        .unwrap();
    assert!(installed.tables.project_paths.is_empty());
    assert_eq!(
        installed
            .tables
            .tasks
            .iter()
            .find(|row| row.id == captured_task)
            .unwrap()
            .title,
        "captured"
    );
    let recaptured = target.capture_shared_state().await.unwrap();
    assert_eq!(
        serde_json::to_value(&recaptured.snapshot.tables).unwrap(),
        serde_json::to_value(&capture.snapshot.tables).unwrap()
    );
    assert!(target.install_shared_state(&capture).await.is_err());
}

#[tokio::test]
async fn failed_install_rolls_back_the_fresh_target_and_never_changes_source() {
    let (_source_dir, source, workspace) = fresh().await;
    task(&source, &workspace, "source task").await;
    let source_before = exported(&source).await;
    let mut capture = source.capture_shared_state().await.unwrap();
    let duplicate_workspace = serde_json::from_value(
        serde_json::to_value(&capture.snapshot.tables.workspaces[0]).unwrap(),
    )
    .unwrap();
    capture.snapshot.tables.workspaces.push(duplicate_workspace);
    let (_target_dir, target, _) = fresh().await;
    let target_before = exported(&target).await;

    assert!(target.install_shared_state(&capture).await.is_err());
    assert_eq!(target_before, exported(&target).await);
    assert_eq!(source_before, exported(&source).await);
}

#[tokio::test]
async fn installed_shared_state_round_trips_through_portable_export() {
    let (_source_dir, source, workspace) = fresh().await;
    task(&source, &workspace, "portable").await;
    let capture = source.capture_shared_state().await.unwrap();
    let (_installed_dir, installed, _) = fresh().await;
    installed.install_shared_state(&capture).await.unwrap();

    let portable = installed
        .export_data("2026-09-21T14:00:00Z".into())
        .await
        .unwrap();
    assert_eq!(portable.version, EXPORT_VERSION);
    assert!(
        portable
            .tables
            .meta
            .iter()
            .all(|row| row.key != "sync_server_url")
    );
    assert_eq!(
        portable.tables.shared_history_provenance.len(),
        portable.tables.changes.len()
    );

    let (_restored_dir, restored, _) = fresh().await;
    restored.validate_import_data(&portable).await.unwrap();
    restored.import_data(&portable).await.unwrap();
    let restored_capture = restored.capture_shared_state().await.unwrap();
    let installed_capture = installed.capture_shared_state().await.unwrap();
    assert_eq!(
        normalized_shared(restored_capture),
        normalized_shared(installed_capture)
    );

    let mut legacy_without_server: crate::data_safety::AvenExport =
        serde_json::from_value(serde_json::to_value(&portable).unwrap()).unwrap();
    legacy_without_server.version = RELATED_LINKS_EXPORT_VERSION;
    legacy_without_server
        .tables
        .shared_history_provenance
        .clear();
    let (_legacy_target_dir, legacy_target, _) = fresh().await;
    let error = legacy_target
        .validate_import_data(&legacy_without_server)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("missing its server identity"));
}

#[tokio::test]
async fn imported_baseline_and_subsequent_operations_converge_after_install() {
    let (_original_dir, original, workspace) = fresh().await;
    let epic = task(&original, &workspace, "epic").await;
    let child = task(&original, &workspace, "child").await;
    let other = task(&original, &workspace, "other").await;
    original
        .add_task_to_epic(&workspace, &child, &epic)
        .await
        .unwrap();
    original
        .add_task_related_link(&workspace, &child, &other)
        .await
        .unwrap();
    original
        .update_task(
            &workspace,
            &child,
            TaskUpdate {
                title: Some("alpha".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    original
        .update_task(
            &workspace,
            &child,
            TaskUpdate {
                title: Some("beta".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let at = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
    let series = original
        .create_recurrence_series(
            &workspace,
            CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                title: "daily".into(),
                description: "template".into(),
                project: "app".into(),
                priority: "none".into(),
                initial_status: "todo".into(),
                labels: vec![],
                metadata: vec![],
                schedule: RecurrenceSchedule::new(
                    RecurrenceRule::daily(),
                    "UTC".parse().unwrap(),
                    at.date_naive(),
                    None,
                    RecurrenceDuePolicy::SameDay,
                ),
            })
            .at(at),
        )
        .await
        .unwrap();

    let attachment_id = crate::ids::new_id();
    let hash = "22".repeat(32);
    {
        let mut conn = original.acquire_writer().await.unwrap();
        sqlx::query(
            "INSERT INTO blob_inventory(
                 sha256, byte_size, media_type, available, first_seen_at, last_verified_at
             ) VALUES (?, 9, 'image/png', 0, '2026-09-21T12:00:00Z', NULL)",
        )
        .bind(&hash)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_attachments(
                 workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                 filename, alt_text, width, height, created_at, created_by_change_id,
                 deleted, deleted_at, deleted_by_change_id
             ) VALUES (?, ?, ?, ?, 9, 'image/png', 'unavailable.png', NULL, 1, 1,
                 '2026-09-21T12:00:00Z', NULL, 0, NULL, NULL)",
        )
        .bind(&workspace.id)
        .bind(&attachment_id)
        .bind(&child)
        .bind(&hash)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let mut portable = original
        .export_data("2026-09-21T12:00:00Z".into())
        .await
        .unwrap();
    portable
        .tables
        .changes
        .retain(|row| row.op_type != "epic_link_add");
    let (_source_dir, source, _) = fresh().await;
    source.import_data(&portable).await.unwrap();
    let imported = source
        .export_data("2026-09-21T12:00:00Z".into())
        .await
        .unwrap();
    assert!(
        imported
            .tables
            .task_epic_links
            .iter()
            .any(|row| { row.child_task_id == child && row.epic_task_id == epic })
    );
    assert!(
        imported
            .tables
            .changes
            .iter()
            .all(|row| row.op_type != "epic_link_add")
    );
    assert_eq!(
        imported
            .tables
            .meta
            .iter()
            .filter(|row| row.key.starts_with("epic_membership_baseline:"))
            .count(),
        1
    );

    {
        let mut conn = source.acquire_writer().await.unwrap();
        let title_changes: Vec<String> = sqlx::query_scalar(
            "SELECT change_id FROM changes
             WHERE entity_id = ? AND field = 'title' ORDER BY local_seq DESC LIMIT 2",
        )
        .bind(&child)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        let current_version: String = sqlx::query_scalar(
            "SELECT version FROM field_versions
             WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ? AND field = 'title'",
        )
        .bind(&workspace.id)
        .bind(&child)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conflicts(
                 workspace_id, entity_type, entity_id, task_id, field, base_version,
                 local_value, remote_value, local_change_id, remote_change_id,
                 variant_a, variant_b, created_at, resolved
             ) VALUES (?, 'task', ?, ?, 'title', NULL, 'beta', 'remote', ?, ?,
                 'variant-a', 'variant-b', '2026-09-21T12:30:00Z', 0)",
        )
        .bind(&workspace.id)
        .bind(&child)
        .bind(&child)
        .bind(&current_version)
        .bind(&title_changes[1])
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let capture = source.capture_shared_state().await.unwrap();
    let (_installed_dir, installed, _) = fresh().await;
    installed.install_shared_state(&capture).await.unwrap();
    let mut stream = establish_test_prefix(&source, &installed, &capture).await;

    installed
        .resolve_conflict(&workspace, &child, "title", "resolved")
        .await
        .unwrap();
    stream
        .send(&installed, &[&source, &installed], "2026-09-21T13:00:00Z")
        .await;
    source
        .remove_task_from_epic(&workspace, &child, &epic)
        .await
        .unwrap();
    stream
        .send(&source, &[&source, &installed], "2026-09-21T13:01:00Z")
        .await;
    installed
        .remove_task_related_link(&workspace, &child, &other)
        .await
        .unwrap();
    stream
        .send(&installed, &[&source, &installed], "2026-09-21T13:02:00Z")
        .await;
    source
        .delete_task_attachment(&workspace, &attachment_id)
        .await
        .unwrap();
    stream
        .send(&source, &[&source, &installed], "2026-09-21T13:03:00Z")
        .await;
    // Both replicas generate the same deterministic recurrence identities at the
    // same clock.
    source
        .reconcile_recurrence_series(&workspace, &series.series.id, at + Duration::days(1))
        .await
        .unwrap();
    installed
        .reconcile_recurrence_series(&workspace, &series.series.id, at + Duration::days(1))
        .await
        .unwrap();
    stream
        .send(&source, &[&source, &installed], "2026-09-22T12:00:00Z")
        .await;

    let source_capture = source.capture_shared_state().await.unwrap();
    let installed_capture = installed.capture_shared_state().await.unwrap();
    assert_eq!(
        serde_json::to_value(&source_capture.snapshot.tables.field_versions).unwrap(),
        serde_json::to_value(&installed_capture.snapshot.tables.field_versions).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&source_capture.snapshot.tables.conflicts).unwrap(),
        serde_json::to_value(&installed_capture.snapshot.tables.conflicts).unwrap()
    );
    let source_state = normalized_shared(source_capture);
    let installed_state = normalized_shared(installed_capture);
    for (table, source_rows) in source_state.as_object().unwrap() {
        assert_eq!(
            source_rows, &installed_state[table],
            "shared table diverged: {table}"
        );
    }

    let final_state = installed
        .export_data("2026-09-22T12:00:00Z".into())
        .await
        .unwrap();
    assert!(final_state.tables.task_epic_links.is_empty());
    assert!(
        final_state
            .tables
            .task_related_links
            .iter()
            .any(|row| row.linked == 0)
    );
    assert!(
        final_state
            .tables
            .task_attachments
            .iter()
            .any(|row| row.attachment_id == attachment_id && row.deleted == 1)
    );
    assert!(
        final_state
            .tables
            .recurrence_occurrences
            .iter()
            .any(|row| row.slot_on == "2026-09-22")
    );
    assert!(
        final_state
            .tables
            .conflicts
            .iter()
            .all(|row| row.resolved == 1)
    );
}

#[tokio::test]
async fn durable_capture_reopens_original_snapshot_after_later_edits() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("db.sqlite");
    let database = Database::open(&db_path).await.unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let captured_task = task(&database, &workspace, "captured title").await;
    let before_cursor = database.meta("sync_cursor").await.unwrap();
    let before_sequences: Vec<(String, Option<i64>)> = {
        let mut conn = database.acquire_reader().await.unwrap();
        sqlx::query_as("SELECT change_id, server_seq FROM changes ORDER BY local_seq")
            .fetch_all(&mut *conn)
            .await
            .unwrap()
    };

    let capture = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let candidate_id = capture.candidate_id().to_string();
    let stream_id = capture.stream_id().to_string();
    database
        .update_task(
            &workspace,
            &captured_task,
            TaskUpdate {
                title: Some("later title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    drop(database);

    let reopened = Database::open(&db_path).await.unwrap();
    let resumed = reopened
        .resume_local_shared_state_never_dispatched()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.candidate_id(), candidate_id);
    assert_eq!(resumed.stream_id(), stream_id);
    assert_eq!(
        resumed
            .shared_state()
            .snapshot
            .tables
            .tasks
            .iter()
            .find(|row| row.id == captured_task)
            .unwrap()
            .title,
        "captured title"
    );
    assert_eq!(reopened.meta("sync_cursor").await.unwrap(), before_cursor);
    let after_sequences: Vec<(String, Option<i64>)> = {
        let mut conn = reopened.acquire_reader().await.unwrap();
        sqlx::query_as(
            "SELECT change_id, server_seq FROM changes
             WHERE change_id IN (SELECT change_id FROM local_shared_capture_changes)
             ORDER BY local_seq",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap()
    };
    assert_eq!(after_sequences, before_sequences);
    reopened
        .cancel_local_shared_state_never_dispatched(&candidate_id)
        .await
        .unwrap();
    let current = reopened
        .export_data("2026-09-21T12:02:00Z".into())
        .await
        .unwrap();
    assert_eq!(
        current
            .tables
            .tasks
            .iter()
            .find(|row| row.id == captured_task)
            .unwrap()
            .title,
        "later title"
    );
}

#[tokio::test]
async fn failed_durable_capture_rolls_back_journal_floor_and_ownership() {
    let (_temp, database, _workspace) = fresh().await;
    let hash = "ab".repeat(32);
    {
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query(
            "INSERT INTO blob_inventory(
                 sha256, byte_size, media_type, available, first_seen_at, last_verified_at
             ) VALUES (?, 10, 'image/png', 1, '2026-09-21T12:00:00Z',
                 '2026-09-21T12:00:00Z')",
        )
        .bind(&hash)
        .execute(&mut *conn)
        .await
        .unwrap();
        db::set_meta(&mut conn, "local_seq", "41").await.unwrap();
        // Fail after the journal, counter floor, generation and image
        // classification writes, so the whole capture must roll back.
        sqlx::query(
            "CREATE TRIGGER fail_capture_pin BEFORE INSERT ON local_shared_capture_pins
             BEGIN SELECT RAISE(ABORT, 'injected capture pin failure'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let error = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("injected capture pin failure"),
        "{error:#}"
    );
    let mut conn = database.acquire_reader().await.unwrap();
    let journals: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_journal")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let images: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_images")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_pins")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!((journals, images, pins), (0, 0, 0));
    assert_eq!(
        db::get_meta(&mut conn, "local_seq")
            .await
            .unwrap()
            .as_deref(),
        Some("41")
    );
    assert_eq!(
        db::get_meta(&mut conn, "sync_generation")
            .await
            .unwrap()
            .as_deref(),
        Some("0")
    );
}

#[tokio::test]
async fn captured_history_blocks_general_and_recurrence_undo_atomically() {
    let (_temp, database, workspace) = fresh().await;
    let created = database
        .create_task_with_undo(
            &workspace,
            draft("undo protected"),
            TaskCreationUndo::TuiTask,
        )
        .await
        .unwrap();
    database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let error = database
        .apply_latest_tui_undo(&workspace.id)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("protected-history"));
    assert!(
        database
            .task_undo_snapshot(&workspace.id, &created.task.id)
            .await
            .is_ok()
    );
    assert!(
        database
            .latest_tui_undo_presentation(&workspace.id)
            .await
            .unwrap()
            .is_some(),
        "the failed owning transaction must not consume undo"
    );

    database
        .cancel_local_shared_state_never_dispatched(
            database
                .resume_local_shared_state_never_dispatched()
                .await
                .unwrap()
                .unwrap()
                .candidate_id(),
        )
        .await
        .unwrap();
    database.clear_pending_tui_undo_entries().await.unwrap();
    let at = Utc::now();
    let series = database
        .create_recurrence_series(
            &workspace,
            CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                title: "protected recurrence".into(),
                description: String::new(),
                project: "app".into(),
                priority: "none".into(),
                initial_status: "todo".into(),
                labels: vec![],
                metadata: vec![],
                schedule: RecurrenceSchedule::new(
                    RecurrenceRule::daily(),
                    "UTC".parse().unwrap(),
                    at.date_naive(),
                    None,
                    RecurrenceDuePolicy::SameDay,
                ),
            })
            .at(at),
        )
        .await
        .unwrap();
    database
        .mutate_tasks(
            &workspace,
            vec![(
                series.task.id.clone(),
                TaskUpdate {
                    status: Some("done".into()),
                    ..Default::default()
                },
            )],
            UndoContext::tui("complete recurrence"),
        )
        .await
        .unwrap();
    database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let before = database
        .export_data("2026-09-21T18:00:00Z".into())
        .await
        .unwrap();
    let error = database
        .apply_latest_tui_undo(&workspace.id)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("protected-history"));
    let after = database
        .export_data("2026-09-21T18:00:00Z".into())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(before.tables).unwrap(),
        serde_json::to_value(after.tables).unwrap()
    );
}

#[tokio::test]
async fn durable_pin_survives_cleanup_until_idempotent_local_cancellation() {
    let (temp, database, workspace) = fresh().await;
    let blob_dir = temp.path().join("blobs");
    let task_id = task(&database, &workspace, "image owner").await;
    let bytes = png_bytes();
    let hash = crate::attachments::storage::sha256_hex(&bytes);
    let object = crate::attachments::storage::object_path(&blob_dir, &hash).unwrap();
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, &bytes).unwrap();
    let extra_bytes = png_bytes_with_dimensions(1, 2);
    let extra_hash = crate::attachments::storage::sha256_hex(&extra_bytes);
    let extra_object = crate::attachments::storage::object_path(&blob_dir, &extra_hash).unwrap();
    std::fs::write(&extra_object, &extra_bytes).unwrap();
    let unavailable_hash = "cd".repeat(32);
    let attachment_id = crate::ids::new_id();
    {
        let mut conn = database.acquire_writer().await.unwrap();
        crate::attachments::storage::upsert_inventory_available(
            &mut conn,
            &hash,
            i64::try_from(bytes.len()).unwrap(),
            "image/png",
        )
        .await
        .unwrap();
        crate::attachments::storage::upsert_inventory_available(
            &mut conn,
            &extra_hash,
            i64::try_from(extra_bytes.len()).unwrap(),
            "image/png",
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO blob_inventory(
                 sha256, byte_size, media_type, available, first_seen_at, last_verified_at
             ) VALUES (?, 9, 'image/png', 0, '2026-09-21T12:00:00Z', NULL)",
        )
        .bind(&unavailable_hash)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_attachments(
                 workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                 filename, alt_text, width, height, created_at, created_by_change_id,
                 deleted, deleted_at, deleted_by_change_id
             ) VALUES (?, ?, ?, ?, ?, 'image/png', 'selected.png', NULL, 2, 1,
                 '2026-09-21T12:00:00Z', NULL, 0, NULL, NULL)",
        )
        .bind(&workspace.id)
        .bind(&attachment_id)
        .bind(&task_id)
        .bind(&hash)
        .bind(i64::try_from(bytes.len()).unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    let capture = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let candidate_id = capture.candidate_id().to_string();
    {
        let mut conn = database.acquire_reader().await.unwrap();
        let classes: Vec<(String, String)> = sqlx::query_as(
            "SELECT sha256, classification FROM local_shared_capture_images ORDER BY sha256",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert!(classes.contains(&(hash.clone(), "current_selected".into())));
        assert!(classes.contains(&(extra_hash.clone(), "extra_selected".into())));
        assert!(classes.contains(&(unavailable_hash, "unavailable".into())));
        let pins: Vec<String> =
            sqlx::query_scalar("SELECT sha256 FROM local_shared_capture_pins ORDER BY sha256")
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        assert_eq!(pins.len(), 2);
        assert!(pins.contains(&hash));
        assert!(pins.contains(&extra_hash));
    }
    database
        .delete_task_attachment(&workspace, &attachment_id)
        .await
        .unwrap();
    let policy = crate::attachments::lifecycle::LifecyclePolicy {
        grace: StdDuration::ZERO,
        ..Default::default()
    };
    let pinned = database
        .prune_attachments(&blob_dir, policy, true)
        .await
        .unwrap();
    assert_eq!(pinned.pruned.count, 0);
    assert!(object.exists());

    assert!(
        database
            .cancel_local_shared_state_never_dispatched(&candidate_id)
            .await
            .unwrap()
    );
    assert!(
        !database
            .cancel_local_shared_state_never_dispatched(&candidate_id)
            .await
            .unwrap()
    );
    let released = database
        .prune_attachments(&blob_dir, policy, true)
        .await
        .unwrap();
    assert_eq!(released.pruned.count, 2);
    assert!(!object.exists());
    assert!(!extra_object.exists());
}

#[tokio::test]
async fn active_local_capture_fences_sync_backup_import_and_restore() {
    let (temp, database, workspace) = fresh().await;
    task(&database, &workspace, "fenced").await;
    let prepared = database
        .prepare_client_sync_page("https://sync.test".into(), 0, 10)
        .await
        .unwrap();
    let stale_request = prepared.request.clone();
    let stale_generation = prepared.sync_generation;
    let portable = database
        .export_data("2026-09-21T12:00:00Z".into())
        .await
        .unwrap();
    let capture = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let candidate_id = capture.candidate_id().to_string();

    let response = SyncResponse {
        protocol_version: SYNC_PROTOCOL_VERSION,
        changes: vec![],
        push_acks: vec![],
        cursor: prepared.request.after,
        has_more: false,
    };
    let error = database
        .apply_client_sync_page(ApplySyncPage {
            request: prepared.request,
            sync_generation: prepared.sync_generation,
            response,
            attempted_at: "2026-09-21T12:01:00Z".into(),
            previous_pushed: 0,
            previous_pulled: 0,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("local-shared-capture-active"));
    assert!(
        database
            .prepare_client_sync_page("https://sync.test".into(), 1, 10)
            .await
            .unwrap_err()
            .to_string()
            .contains("local-shared-capture-active")
    );
    assert!(
        database
            .import_data(&portable)
            .await
            .unwrap_err()
            .to_string()
            .contains("local-shared-capture-active")
    );
    let backup = temp.path().join("backup.sqlite");
    assert!(
        db::backup_database(database.path(), &backup)
            .await
            .unwrap_err()
            .to_string()
            .contains("local-shared-capture-active")
    );
    let archive = temp.path().join("backup.tar.zst");
    assert!(
        database
            .create_backup_archive(&temp.path().join("blobs"), &archive)
            .await
            .unwrap_err()
            .to_string()
            .contains("local-shared-capture-active")
    );

    let restore_source = temp.path().join("restore-source.sqlite");
    let inactive = Database::open(&restore_source).await.unwrap();
    drop(inactive);
    assert!(
        db::restore_database_file(database.path(), &restore_source)
            .await
            .unwrap_err()
            .to_string()
            .contains("local-shared-capture-active")
    );

    database
        .cancel_local_shared_state_never_dispatched(&candidate_id)
        .await
        .unwrap();
    let stale_response = SyncResponse {
        protocol_version: SYNC_PROTOCOL_VERSION,
        changes: vec![],
        push_acks: vec![],
        cursor: stale_request.after,
        has_more: false,
    };
    let error = database
        .apply_client_sync_page(ApplySyncPage {
            request: stale_request,
            sync_generation: stale_generation,
            response: stale_response,
            attempted_at: "2026-09-21T12:02:00Z".into(),
            previous_pushed: 0,
            previous_pulled: 0,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("sync-generation-changed"));
}

#[tokio::test]
async fn remote_current_attachment_without_inventory_fails_capture() {
    let (_temp, database, workspace) = fresh().await;
    let task_id = task(&database, &workspace, "remote image owner").await;
    let attachment_id = crate::ids::new_id();
    let hash = "ab".repeat(32);
    apply_remote_attachment_change(
        &database,
        &workspace,
        &task_id,
        &attachment_id,
        &hash,
        false,
    )
    .await;

    let error = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("attachment inventory missing"));
    assert!(
        database
            .resume_local_shared_state_never_dispatched()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn unavailable_current_image_fails_but_deleted_history_is_permitted() {
    let (_temp, database, workspace) = fresh().await;
    let task_id = task(&database, &workspace, "remote image owner").await;
    let attachment_id = crate::ids::new_id();
    let hash = "cd".repeat(32);
    apply_remote_attachment_change(
        &database,
        &workspace,
        &task_id,
        &attachment_id,
        &hash,
        false,
    )
    .await;
    {
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query(
            "INSERT INTO blob_inventory(
                 sha256, byte_size, media_type, available, first_seen_at
             ) VALUES (?, 7, 'image/png', 0, '2026-09-21T12:00:00Z')",
        )
        .bind(&hash)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let error = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("required-image-unavailable"));
    assert!(error.to_string().contains("complete image download"));

    apply_remote_attachment_change(&database, &workspace, &task_id, &attachment_id, &hash, true)
        .await;
    let capture = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let mut conn = database.acquire_reader().await.unwrap();
    let classification: String = sqlx::query_scalar(
        "SELECT classification FROM local_shared_capture_images
         WHERE candidate_id = ? AND sha256 = ?",
    )
    .bind(capture.candidate_id())
    .bind(&hash)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let pins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM local_shared_capture_pins
         WHERE candidate_id = ? AND sha256 = ?",
    )
    .bind(capture.candidate_id())
    .bind(&hash)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(classification, "unavailable");
    assert_eq!(pins, 0);
}

#[tokio::test]
async fn malformed_persisted_capture_fails_closed() {
    let (_temp, database, workspace) = fresh().await;
    task(&database, &workspace, "malformed").await;
    database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    {
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query("UPDATE local_shared_capture_journal SET snapshot_json = '{\"version\":999}'")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert!(
        database
            .resume_local_shared_state_never_dispatched()
            .await
            .unwrap_err()
            .to_string()
            .contains("malformed")
    );
    assert!(
        database
            .prepare_client_sync_page("https://sync.test".into(), 1, 10)
            .await
            .unwrap_err()
            .to_string()
            .contains("local-shared-capture-active")
    );
}

#[tokio::test]
async fn recurrence_reconciles_and_resolves_after_install_at_fixed_clocks() {
    recurrence_continuation(false).await;
}

#[tokio::test]
async fn recurrence_metadata_successor_converges_after_install() {
    recurrence_continuation(true).await;
}

async fn recurrence_continuation(with_metadata: bool) {
    use crate::operations::{CreateRecurrenceSeriesParams, RecurrenceSeriesDraft};
    use crate::recurrence::{
        RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceRule, RecurrenceSchedule,
    };
    let (_a_dir, a, ws) = fresh().await;
    let (_b_dir, b, _) = fresh().await;
    // Public note and metadata mutations consult wall time, so keep the fixed
    // occurrence clocks in the future to avoid archiving them during mutation.
    let at = Utc.with_ymd_and_hms(2100, 9, 21, 12, 0, 0).unwrap();
    let series = a
        .create_recurrence_series(
            &ws,
            CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                title: "daily".into(),
                description: "template".into(),
                project: "app".into(),
                priority: "none".into(),
                initial_status: "todo".into(),
                labels: vec!["habit".into()],
                metadata: if with_metadata {
                    vec![crate::metadata::TaskMetadataInput {
                        expected_field_id: None,
                        key: "ticket".into(),
                        value: "42".into(),
                    }]
                } else {
                    vec![]
                },
                schedule: RecurrenceSchedule::new(
                    RecurrenceRule::daily(),
                    "UTC".parse().unwrap(),
                    at.date_naive(),
                    None,
                    RecurrenceDuePolicy::SameDay,
                ),
            })
            .at(at)
            .with_create_missing_labels(),
        )
        .await
        .unwrap();
    a.add_note(&ws, &series.task.id, "occurrence-local note".into())
        .await
        .unwrap();
    let snapshot = a.capture_shared_state().await.unwrap();
    b.install_shared_state(&snapshot).await.unwrap();
    assert_recurrence_replicas_equal(&a, &b).await;
    let mut stream = establish_test_prefix(&a, &b, &snapshot).await;
    let tomorrow = at + chrono::Duration::days(1);
    // Both replicas independently generate the same deterministic projection.
    a.reconcile_recurrence_series(&ws, &series.series.id, tomorrow)
        .await
        .unwrap();
    b.reconcile_recurrence_series(&ws, &series.series.id, tomorrow)
        .await
        .unwrap();
    stream.send(&a, &[&a, &b], "2100-09-22T12:00:00Z").await;
    stream.send(&b, &[&a, &b], "2100-09-22T12:00:00Z").await;
    assert_recurrence_replicas_equal(&a, &b).await;
    let current = b
        .reconcile_recurrence_series(&ws, &series.series.id, tomorrow)
        .await
        .unwrap()
        .occurrence
        .unwrap();
    {
        let mut conn = b.acquire_writer().await.unwrap();
        let mut tx = db::begin_immediate(&mut conn).await.unwrap();
        crate::operations::recurrence::resolve_recurrence_occurrence_in_transaction(
            &mut tx,
            &ws,
            current.task_id.as_ref().unwrap(),
            RecurrenceOutcome::Completed,
            "2100-09-22T13:00:00Z",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    stream.send(&b, &[&a, &b], "2100-09-22T13:00:00Z").await;
    assert_recurrence_replicas_equal(&a, &b).await;
    if with_metadata {
        let successor: TaskId = b
            .export_data("2100-09-22T14:00:00Z".into())
            .await
            .unwrap()
            .tables
            .recurrence_occurrences
            .into_iter()
            .find(|o| o.slot_on == "2100-09-23")
            .unwrap()
            .task_id
            .parse()
            .unwrap();
        b.update_task(
            &ws,
            &successor,
            TaskUpdate {
                set_metadata: vec![crate::metadata::TaskMetadataInput {
                    expected_field_id: None,
                    key: "ticket".into(),
                    value: "edited".into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        stream.send(&b, &[&a, &b], "2100-09-22T14:00:00Z").await;
        assert_recurrence_replicas_equal(&a, &b).await;
    }
    let data = b.export_data("2100-09-22T14:00:00Z".into()).await.unwrap();
    assert!(data.tables.recurrence_occurrences.len() >= 3);
    assert_eq!(data.tables.notes.len(), 1);
    assert_eq!(
        !data.tables.recurrence_series_metadata.is_empty(),
        with_metadata
    );
    assert!(!data.tables.recurrence_series_labels.is_empty());
    assert!(data.tables.conflicts.is_empty());
}

async fn assert_recurrence_replicas_equal(a: &Database, b: &Database) {
    assert_eq!(
        normalized_shared(a.capture_shared_state().await.unwrap()),
        normalized_shared(b.capture_shared_state().await.unwrap())
    );
}
