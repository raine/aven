use chrono::{Duration, TimeZone, Utc};
use serde_json::Value;

use crate::sync::ApplySyncPage;
use crate::sync::wire::{ChangeWire, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse};

use super::*;
use crate::choices::TaskSource;
use crate::ids::TaskId;
use crate::operations::{
    CreateRecurrenceSeriesParams, RecurrenceSeriesDraft, TaskDraft, TaskUpdate,
};
use crate::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule};
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
    // same clock. Template metadata is intentionally absent: AVN-VWRD owns the
    // known metadata-version divergence and this test must not normalize it away.
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
