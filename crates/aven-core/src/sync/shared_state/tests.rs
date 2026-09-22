use chrono::{Duration, TimeZone, Utc};
use serde_json::Value;

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
async fn capture_install_and_domain_continuation_preserve_shared_semantics() {
    let (_source_dir, source, workspace) = fresh().await;
    let (_target_dir, target, _) = fresh().await;
    let epic = task(&source, &workspace, "epic").await;
    let child = task(&source, &workspace, "child").await;
    let other = task(&source, &workspace, "other").await;
    source
        .add_task_to_epic(&workspace, &child, &epic)
        .await
        .unwrap();
    source
        .add_task_dependency(&workspace, &child, &other)
        .await
        .unwrap();
    source
        .add_task_related_link(&workspace, &child, &other)
        .await
        .unwrap();

    source
        .update_task(
            &workspace,
            &child,
            TaskUpdate {
                title: Some("local title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    source
        .update_task(
            &workspace,
            &child,
            TaskUpdate {
                title: Some("second title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let at = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
    let series = source
        .create_recurrence_series(
            &workspace,
            CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                title: "daily".into(),
                description: "template".into(),
                project: "app".into(),
                priority: "none".into(),
                initial_status: "todo".into(),
                labels: vec!["habit".into()],
                metadata: vec![],
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

    let unavailable_attachment_id = crate::ids::new_id();
    let unavailable_hash = "11".repeat(32);
    {
        let mut conn = source.acquire_writer().await.unwrap();
        sqlx::query(
            "INSERT INTO blob_inventory(
                 sha256, byte_size, media_type, available, first_seen_at, last_verified_at
             ) VALUES (?, 7, 'image/png', 0, '2026-09-21T12:00:00Z', NULL)",
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
             ) VALUES (?, ?, ?, ?, 7, 'image/png', 'missing.png', 'missing', 1, 1,
                 '2026-09-21T12:00:00Z', NULL, 0, NULL, NULL)",
        )
        .bind(&workspace.id)
        .bind(&unavailable_attachment_id)
        .bind(&child)
        .bind(&unavailable_hash)
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

        let title_changes: Vec<(String,)> = sqlx::query_as(
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
             ) VALUES (?, 'task', ?, ?, 'title', NULL, 'second title', 'remote title',
                 ?, ?, 'variant-a', 'variant-b', '2026-09-21T12:00:00Z', 0)",
        )
        .bind(&workspace.id)
        .bind(&child)
        .bind(&child)
        .bind(&current_version)
        .bind(&title_changes[1].0)
        .execute(&mut *conn)
        .await
        .unwrap();

        let change_ids: Vec<String> = sqlx::query_scalar(
            "SELECT change_id FROM changes ORDER BY local_seq, created_at, change_id",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        for (index, change_id) in change_ids.iter().take(3).enumerate() {
            sqlx::query("UPDATE changes SET server_seq = ? WHERE change_id = ?")
                .bind(i64::try_from((index + 1) * 10).unwrap())
                .bind(change_id)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
    }

    let source_before = exported(&source).await;
    let capture = source.capture_shared_state().await.unwrap();
    assert_eq!(source_before, exported(&source).await);
    source
        .update_task(
            &workspace,
            &other,
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
            .provenance
            .iter()
            .any(|row| row.source_server_seq == Some(10))
    );
    assert!(
        capture
            .provenance
            .iter()
            .any(|row| row.source_pending_rank.is_some())
    );

    let target_identity = target.meta("client_id").await.unwrap().unwrap();
    let report = target.install_shared_state(&capture).await.unwrap();
    assert_eq!(
        report.prefix_count as usize,
        capture.snapshot.tables.changes.len()
    );
    assert_eq!(report.attachment_count, 1);
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
    assert_eq!(
        installed
            .tables
            .tasks
            .iter()
            .find(|row| row.id == other)
            .unwrap()
            .title,
        "other"
    );

    let recaptured = target.capture_shared_state().await.unwrap();
    assert_eq!(
        serde_json::to_value(&recaptured.snapshot.tables).unwrap(),
        serde_json::to_value(&capture.snapshot.tables).unwrap()
    );
    assert_eq!(recaptured.provenance, capture.provenance);

    target
        .resolve_conflict(&workspace, &child, "title", "resolved title")
        .await
        .unwrap();
    target
        .remove_task_related_link(&workspace, &child, &other)
        .await
        .unwrap();
    target
        .remove_task_from_epic(&workspace, &child, &epic)
        .await
        .unwrap();
    let deleted_attachment = target
        .delete_task_attachment(&workspace, &unavailable_attachment_id)
        .await
        .unwrap();
    assert!(deleted_attachment.attachment.deleted);
    assert!(!deleted_attachment.has_blob);

    let reconciled = target
        .reconcile_recurrence_series(&workspace, &series.series.id, at + Duration::days(1))
        .await
        .unwrap();
    assert!(reconciled.changed);
    assert_eq!(
        reconciled.occurrence.unwrap().slot_on.to_string(),
        "2026-09-22"
    );

    let result = target
        .export_data("2026-09-22T12:00:00Z".into())
        .await
        .unwrap();
    assert!(result.tables.task_related_links.iter().any(|row| {
        ((row.task_a_id == child && row.task_b_id == other)
            || (row.task_a_id == other && row.task_b_id == child))
            && row.linked == 0
    }));
    assert!(result.tables.task_epic_links.is_empty());
    assert!(result.tables.conflicts.iter().all(|row| row.resolved == 1));
    assert!(
        result
            .tables
            .field_versions
            .iter()
            .any(|row| { row.entity_id == child.as_str() && row.field == "title" })
    );
    assert!(result.tables.recurrence_occurrences.len() >= 2);
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
