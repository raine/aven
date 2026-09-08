use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll, Waker};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use sqlx::SqliteConnection;

use super::*;
use crate::recurrence::{RecurrenceRule, TimeZoneId};

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, day, hour, 0, 0)
        .single()
        .unwrap()
}

fn daily_schedule(start_day: u32) -> RecurrenceSchedule {
    RecurrenceSchedule::new(
        RecurrenceRule::daily(),
        "UTC".parse::<TimeZoneId>().unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, start_day).unwrap(),
        None,
        RecurrenceDuePolicy::SameDay,
    )
}

fn draft(start_day: u32) -> RecurrenceSeriesDraft {
    RecurrenceSeriesDraft {
        metadata: Vec::new(),
        title: "daily journal".to_string(),
        description: "write one page".to_string(),
        project: "recurrence".to_string(),
        priority: "high".to_string(),
        initial_status: "todo".to_string(),
        labels: vec!["habit".to_string(), "writing".to_string()],
        schedule: daily_schedule(start_day),
    }
}

async fn setup() -> (
    tempfile::TempDir,
    sqlx::pool::PoolConnection<sqlx::Sqlite>,
    Workspace,
) {
    let (temp, mut conn) = crate::test_support::test_conn().await;
    let workspace = crate::test_support::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    for label in ["habit", "writing", "future"] {
        sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, ?, 't')")
            .bind(&workspace.id)
            .bind(label)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    (temp, conn, workspace)
}

async fn create_daily(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
) -> RecurrenceCreateOutcome {
    create_recurrence_series(
        conn,
        workspace,
        CreateRecurrenceSeriesParams::new(draft(20)).at(at(20, 12)),
    )
    .await
    .unwrap()
}

async fn resolve(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    outcome: RecurrenceOutcome,
    resolved_at: &str,
) -> RecurrenceResolveOutcome {
    let mut tx = begin_immediate(conn).await.unwrap();
    let result = resolve_recurrence_occurrence_in_transaction(
        &mut tx,
        workspace,
        task_id,
        outcome,
        resolved_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    result
}

async fn materialization_snapshot(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    task_id: &TaskId,
) -> Vec<String> {
    let task: String = sqlx::query_scalar(
        "SELECT json_object(
            'id', id, 'workspace_id', workspace_id, 'title', title,
            'description', description, 'project_id', project_id, 'status', status,
            'priority', priority, 'created_at', created_at, 'updated_at', updated_at,
            'queue_activity_at', queue_activity_at, 'available_at', available_at,
            'due_on', due_on, 'deleted', deleted, 'is_epic', is_epic)
         FROM tasks WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let labels: String = sqlx::query_scalar(
        "SELECT json_group_array(label) FROM (
            SELECT label FROM task_labels
            WHERE workspace_id = ? AND task_id = ? ORDER BY label
         )",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let occurrence: String = sqlx::query_scalar(
        "SELECT json_object(
            'workspace_id', workspace_id, 'series_id', series_id, 'slot_on', slot_on,
            'task_id', task_id, 'outcome', outcome, 'resolved_at', resolved_at,
            'outcome_change_id', outcome_change_id, 'projection_state', projection_state,
            'archived_at', archived_at)
         FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND task_id = ?",
    )
    .bind(workspace_id)
    .bind(series_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let versions: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object('field', field, 'version', version)) FROM (
            SELECT field, version FROM field_versions
            WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ?
            ORDER BY field
         )",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let changes: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object(
            'change_id', change_id, 'entity_type', entity_type, 'entity_id', entity_id,
            'field', field, 'op_type', op_type, 'payload', payload,
            'base_version', base_version, 'created_at', created_at)) FROM (
                SELECT change_id, entity_type, entity_id, field, op_type, payload,
                       base_version, created_at
                FROM changes WHERE change_id IN (
                    SELECT change_id FROM changes
                    WHERE (entity_type = 'task' AND entity_id = ?)
                       OR (entity_type = 'recurrence_series' AND entity_id = ?
                           AND field = 'projection')
                ) ORDER BY change_id
         )",
    )
    .bind(task_id)
    .bind(series_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    vec![task, labels, occurrence, versions, changes]
}

#[tokio::test]
async fn independent_replicas_materialize_byte_equal_occurrence_state() {
    let (_first_temp, mut first, workspace) = setup().await;
    let created = create_daily(&mut first, &workspace).await;
    let first_snapshot = materialization_snapshot(
        &mut first,
        &workspace.id,
        &created.series.id,
        &created.task.id,
    )
    .await;

    let (_second_temp, mut second, second_workspace) = setup().await;
    sqlx::query(
        "INSERT INTO projects(
            workspace_id, id, key, name, prefix, created_at, updated_at, deleted
         ) VALUES (?, ?, 'replica-project', 'Replica Project', 'RPL', 't', 't', 0)",
    )
    .bind(&second_workspace.id)
    .bind(&created.series.project_id)
    .execute(&mut *second)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO recurrence_series(
            workspace_id, id, title, description, project_id, priority, initial_status,
            frequency, interval, weekdays, timezone, start_on, available_local_time,
            due_policy, state, stopped_at, created_at, updated_at, deleted
         ) VALUES (?, ?, ?, ?, ?, ?, ?, 'daily', 1, '', 'UTC', '2026-07-20', '',
                   'same_day', 'active', '', ?, ?, 0)",
    )
    .bind(&second_workspace.id)
    .bind(&created.series.id)
    .bind(&created.series.title)
    .bind(&created.series.description)
    .bind(&created.series.project_id)
    .bind(created.series.priority.as_str())
    .bind(created.series.initial_status.as_str())
    .bind(&created.series.created_at)
    .bind(&created.series.updated_at)
    .execute(&mut *second)
    .await
    .unwrap();
    for label in ["habit", "writing"] {
        sqlx::query(
            "INSERT INTO recurrence_series_labels(workspace_id, series_id, label)
             VALUES (?, ?, ?)",
        )
        .bind(&second_workspace.id)
        .bind(&created.series.id)
        .bind(label)
        .execute(&mut *second)
        .await
        .unwrap();
    }
    let replica_occurrence = materialize_occurrence(
        &mut second,
        &second_workspace,
        &created.series,
        &["habit".to_string(), "writing".to_string()],
        created.occurrence.slot_on,
    )
    .await
    .unwrap();
    assert_eq!(replica_occurrence.task_id.as_ref(), Some(&created.task.id));
    let second_snapshot = materialization_snapshot(
        &mut second,
        &second_workspace.id,
        &created.series.id,
        &created.task.id,
    )
    .await;
    assert_eq!(first_snapshot, second_snapshot);
}

#[tokio::test]
async fn create_series_atomically_materializes_complete_deterministic_snapshot() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;

    assert!(created.series_ref.starts_with("RCR-"));
    assert_eq!(created.occurrence.slot_on.to_string(), "2026-07-20");
    assert_eq!(created.task.title, "daily journal");
    assert_eq!(created.task.description, "write one page");
    assert_eq!(created.task.status, TaskStatus::Todo);
    assert_eq!(created.task.priority, TaskPriority::High);
    assert_eq!(created.task.created_at, "2026-07-20T00:00:00Z");
    assert_eq!(
        created.task.available_at.as_deref(),
        Some("2026-07-20T00:00:00Z")
    );
    assert_eq!(created.task.due_on.as_deref(), Some("2026-07-20"));

    let labels: Vec<String> = sqlx::query_scalar(
        "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = ? ORDER BY label",
    )
    .bind(&workspace.id)
    .bind(&created.task.id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(labels, vec!["habit", "writing"]);
    let versions: Vec<String> = sqlx::query_scalar(
        "SELECT version FROM field_versions
         WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ?",
    )
    .bind(&workspace.id)
    .bind(&created.task.id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(versions.len(), TaskField::VERSIONED.len());
    assert!(versions.iter().all(|version| version == &versions[0]));
    let change_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM changes
         WHERE entity_id IN (?, ?) AND op_type IN ('create_task', 'project_recurrence_occurrence')",
    )
    .bind(&created.task.id)
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(change_count, 2);

    let resolved = resolve_recurrence_ref(&mut conn, &workspace, &created.series_ref)
        .await
        .unwrap();
    assert_eq!(resolved.id, created.series.id);

    let labels = load_series_labels(&mut conn, &workspace.id, &created.series.id)
        .await
        .unwrap();
    materialize_occurrence(
        &mut conn,
        &workspace,
        &created.series,
        &labels,
        created.occurrence.slot_on,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE tasks SET title = 'divergent' WHERE workspace_id = ? AND id = ?")
        .bind(&workspace.id)
        .bind(&created.task.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let conflict = materialize_occurrence(
        &mut conn,
        &workspace,
        &created.series,
        &labels,
        created.occurrence.slot_on,
    )
    .await
    .unwrap_err();
    assert!(
        conflict
            .to_string()
            .contains("recurrence-generation-conflict")
    );
    assert_eq!(
        conflict
            .downcast_ref::<crate::error::CoreError>()
            .unwrap()
            .kind(),
        crate::error::ErrorKind::GenerationConflict
    );
}

#[tokio::test]
async fn recurrence_params_combine_clock_and_label_creation_policy() {
    let (_temp, mut conn, workspace) = setup().await;
    let mut input = draft(20);
    input.labels = vec!["created-with-series".to_string()];
    let created = create_recurrence_series(
        &mut conn,
        &workspace,
        CreateRecurrenceSeriesParams::new(input)
            .at(at(20, 12))
            .with_create_missing_labels(),
    )
    .await
    .unwrap();

    assert_eq!(created.series.created_at, "2026-07-20T12:00:00Z");
    assert_eq!(
        load_series_labels(&mut conn, &workspace.id, &created.series.id)
            .await
            .unwrap(),
        vec!["created-with-series"]
    );

    let updated = update_recurrence_template(
        &mut conn,
        &workspace,
        &created.series.id,
        UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
            set_metadata: Vec::new(),
            remove_metadata: Vec::new(),
            labels: Some(vec!["created-with-update".to_string()]),
            ..RecurrenceTemplateUpdate::default()
        })
        .with_create_missing_labels(),
    )
    .await
    .unwrap();

    assert!(updated.changed);
    assert_eq!(
        load_series_labels(&mut conn, &workspace.id, &created.series.id)
            .await
            .unwrap(),
        vec!["created-with-update"]
    );
}

#[tokio::test]
async fn database_creation_samples_implicit_time_before_waiting_for_writer() {
    let temp = tempfile::tempdir().unwrap();
    let database = Database::open(&temp.path().join("boundary.sqlite"))
        .await
        .unwrap();
    let workspace = database.resolve_workspace("default").await.unwrap();
    {
        let mut writer = database.acquire_writer().await.unwrap();
        for label in ["habit", "writing"] {
            sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, ?, 't')")
                .bind(&workspace.id)
                .bind(label)
                .execute(&mut *writer)
                .await
                .unwrap();
        }
    }

    let held_writer = database.acquire_writer().await.unwrap();
    let after_boundary = Arc::new(AtomicBool::new(false));
    let clock_calls = Arc::new(AtomicUsize::new(0));
    let mut creation = Box::pin(database.create_recurrence_series_with_clock(
        &workspace,
        CreateRecurrenceSeriesParams::new(draft(20)),
        {
            let after_boundary = Arc::clone(&after_boundary);
            let clock_calls = Arc::clone(&clock_calls);
            move || {
                clock_calls.fetch_add(1, Ordering::SeqCst);
                Ok(if after_boundary.load(Ordering::SeqCst) {
                    at(21, 0)
                } else {
                    at(20, 23)
                })
            }
        },
    ));
    let mut context = TaskContext::from_waker(Waker::noop());

    assert!(matches!(
        creation.as_mut().poll(&mut context),
        Poll::Pending
    ));
    assert_eq!(clock_calls.load(Ordering::SeqCst), 1);
    after_boundary.store(true, Ordering::SeqCst);
    drop(held_writer);

    let created = creation.await.unwrap();
    assert_eq!(created.series.created_at, "2026-07-20T23:00:00Z");
    assert_eq!(created.occurrence.slot_on.to_string(), "2026-07-20");

    let explicit = database
        .create_recurrence_series_with_clock(
            &workspace,
            CreateRecurrenceSeriesParams::new(draft(20)).at(at(22, 12)),
            || panic!("explicit creation time must bypass the default clock"),
        )
        .await
        .unwrap();
    assert_eq!(explicit.series.created_at, "2026-07-22T12:00:00Z");
    assert_eq!(explicit.occurrence.slot_on.to_string(), "2026-07-22");
}

#[tokio::test]
async fn create_rolls_back_series_task_and_changes_on_materialization_failure() {
    let (_temp, mut conn, workspace) = setup().await;
    sqlx::query(
        "CREATE TRIGGER fail_recurrence_occurrence
         BEFORE INSERT ON recurrence_occurrences
         BEGIN SELECT RAISE(FAIL, 'injected recurrence occurrence failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();

    assert!(
        create_recurrence_series(
            &mut conn,
            &workspace,
            CreateRecurrenceSeriesParams::new(draft(20)).at(at(20, 12)),
        )
        .await
        .is_err()
    );
    for (table, query) in [
        (
            "recurrence_series",
            "SELECT count(*) FROM recurrence_series",
        ),
        (
            "recurrence_occurrences",
            "SELECT count(*) FROM recurrence_occurrences",
        ),
        ("tasks", "SELECT count(*) FROM tasks"),
        ("changes", "SELECT count(*) FROM changes"),
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} should roll back");
    }
}

#[tokio::test]
async fn template_edits_apply_only_to_future_occurrences() {
    let (_temp, mut conn, workspace) = setup().await;
    let mut series_draft = draft(20);
    series_draft.metadata = vec![crate::metadata::TaskMetadataInput {
        expected_field_id: None,
        key: "legacy-id".to_string(),
        value: "old".to_string(),
    }];
    let created = create_recurrence_series(
        &mut conn,
        &workspace,
        CreateRecurrenceSeriesParams::new(series_draft).at(at(20, 12)),
    )
    .await
    .unwrap();
    let original_task_id = created.task.id.clone();
    let update = RecurrenceTemplateUpdate {
        set_metadata: vec![crate::metadata::TaskMetadataInput {
            expected_field_id: None,
            key: "legacy-id".to_string(),
            value: "new".to_string(),
        }],
        remove_metadata: Vec::new(),
        title: Some("future journal".to_string()),
        priority: Some("urgent".to_string()),
        labels: Some(vec!["future".to_string()]),
        ..Default::default()
    };
    let updated = update_recurrence_template(
        &mut conn,
        &workspace,
        &created.series.id,
        UpdateRecurrenceTemplateParams::new(update),
    )
    .await
    .unwrap();
    assert!(updated.changed);
    let old_task = get_task_in_workspace(&mut conn, &workspace, &original_task_id)
        .await
        .unwrap();
    assert_eq!(old_task.title, "daily journal");
    assert_eq!(old_task.priority, TaskPriority::High);
    let old_metadata: (String, String) = sqlx::query_as(
        "SELECT f.key, m.value FROM task_metadata m
         JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id
         WHERE m.workspace_id = ? AND m.task_id = ?",
    )
    .bind(&workspace.id)
    .bind(&original_task_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(old_metadata, ("legacy-id".to_string(), "old".to_string()));
    crate::metadata::rename_metadata_field(&mut conn, &workspace, "legacy-id", "external-id")
        .await
        .unwrap();

    let resolved = resolve(
        &mut conn,
        &workspace,
        &original_task_id,
        RecurrenceOutcome::Completed,
        "2026-07-20T18:00:00Z",
    )
    .await;
    let successor = resolved.successor.unwrap();
    assert_eq!(successor.title, "future journal");
    assert_eq!(successor.priority, TaskPriority::Urgent);
    let labels: Vec<String> =
        sqlx::query_scalar("SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = ?")
            .bind(&workspace.id)
            .bind(&successor.id)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(labels, vec!["future"]);
    let successor_metadata: (String, String) = sqlx::query_as(
        "SELECT f.key, m.value FROM task_metadata m
         JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id
         WHERE m.workspace_id = ? AND m.task_id = ?",
    )
    .bind(&workspace.id)
    .bind(&successor.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        successor_metadata,
        ("external-id".to_string(), "new".to_string())
    );
}

#[tokio::test]
async fn six_days_late_archives_old_projection_and_materializes_only_live_slot() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    sqlx::query("UPDATE tasks SET description = ? WHERE workspace_id = ? AND id = ?")
        .bind("local notes remain")
        .bind(&workspace.id)
        .bind(&created.task.id)
        .execute(&mut *conn)
        .await
        .unwrap();

    let reconciled =
        reconcile_recurrence_series_once(&mut conn, &workspace, &created.series.id, at(26, 10))
            .await
            .unwrap();
    assert!(reconciled.changed);
    assert_eq!(
        reconciled.occurrence.unwrap().slot_on.to_string(),
        "2026-07-26"
    );
    let archived = load_occurrence(
        &mut conn,
        &workspace.id,
        &created.series.id,
        NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        archived.projection_state,
        RecurrenceProjectionState::Archived
    );
    let archived_task = get_task_in_workspace(&mut conn, &workspace, &created.task.id)
        .await
        .unwrap();
    assert_eq!(archived_task.description, "local notes remain");
    assert!(!archived_task.deleted);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ?",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(count, 2);

    let second =
        reconcile_recurrence_series_once(&mut conn, &workspace, &created.series.id, at(26, 10))
            .await
            .unwrap();
    assert!(!second.changed);
}

#[tokio::test]
async fn lifecycle_conflict_returns_before_archival_or_creation() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    sqlx::query(
        "INSERT INTO conflicts(
            workspace_id, entity_type, entity_id, task_id, field, base_version,
            local_value, remote_value, local_change_id, remote_change_id,
            variant_a, variant_b, created_at, resolved
         ) VALUES (?, 'recurrence_series', ?, '', 'state', NULL,
            'active', 'paused', NULL, 'REMOTECHANGE0001',
            'active', 'paused', '2026-07-21T00:00:00Z', 0)",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .execute(&mut *conn)
    .await
    .unwrap();

    let result =
        reconcile_recurrence_series_once(&mut conn, &workspace, &created.series.id, at(26, 10))
            .await
            .unwrap();
    assert!(result.lifecycle_blocked);
    assert!(!result.changed);
    assert_eq!(result.occurrence.unwrap().slot_on.to_string(), "2026-07-20");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ?",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn resolve_failure_rolls_back_task_status_outcome_and_successor() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    sqlx::query(
        "CREATE TRIGGER fail_recurrence_resolution
         BEFORE UPDATE OF outcome ON recurrence_occurrences
         WHEN NEW.outcome != ''
         BEGIN SELECT RAISE(FAIL, 'injected recurrence resolution failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();

    let mut tx = begin_immediate(&mut conn).await.unwrap();
    let result = resolve_recurrence_occurrence_in_transaction(
        &mut tx,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Completed,
        "2026-07-20T18:00:00Z",
    )
    .await;
    assert!(result.is_err());
    tx.rollback().await.unwrap();

    let task = get_task_in_workspace(&mut conn, &workspace, &created.task.id)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Todo);
    let occurrence = load_projected_occurrence(&mut conn, &workspace.id, &created.series.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(occurrence.slot_on.to_string(), "2026-07-20");
    assert!(occurrence.outcome.is_none());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ?",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn pause_resume_omits_suspended_and_pause_boundary_slots() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    pause_recurrence_series(
        &mut conn,
        &workspace,
        &created.series.id,
        "2026-07-20T12:00:00Z",
    )
    .await
    .unwrap();

    let resumed = resume_recurrence_series(&mut conn, &workspace, &created.series.id, at(23, 12))
        .await
        .unwrap();
    assert_eq!(resumed.series.state, RecurrenceSeriesState::Active);
    assert_eq!(
        resumed.occurrence.unwrap().slot_on.to_string(),
        "2026-07-24"
    );
    let old = load_occurrence(
        &mut conn,
        &workspace.id,
        &created.series.id,
        NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(old.projection_state, RecurrenceProjectionState::Archived);
    let dates: Vec<String> = sqlx::query_scalar(
        "SELECT slot_on FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? ORDER BY slot_on",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(dates, vec!["2026-07-20", "2026-07-24"]);
}

#[tokio::test]
async fn stop_paused_series_advances_equal_lifecycle_timestamp() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    let timestamp = "2026-07-20T12:00:00Z";
    pause_recurrence_series(&mut conn, &workspace, &created.series.id, timestamp)
        .await
        .unwrap();

    let stopped =
        stop_recurrence_series(&mut conn, &workspace, &created.series.id, false, timestamp)
            .await
            .unwrap();

    assert_eq!(stopped.series.state, RecurrenceSeriesState::Stopped);
    assert_eq!(
        stopped.series.stopped_at.as_deref(),
        Some("2026-07-20T12:00:01Z")
    );
    let resumed_at: String = sqlx::query_scalar(
        "SELECT resumed_at FROM recurrence_pause_intervals
         WHERE workspace_id = ? AND series_id = ?",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(resumed_at, "2026-07-20T12:00:01Z");
}

async fn assert_recurrence_roundtrip(temp: &tempfile::TempDir) {
    let database = crate::db::Database::open(&temp.path().join("test.sqlite"))
        .await
        .unwrap();
    let export = database
        .export_data("2026-07-24T18:00:00Z".into())
        .await
        .unwrap();
    let report = database.database_integrity_report().await.unwrap();
    let target_temp = tempfile::tempdir().unwrap();
    let target = crate::db::Database::open(&target_temp.path().join("import.sqlite"))
        .await
        .unwrap();
    let validation = target.validate_import_data(&export).await;
    assert!(
        report.checks.iter().all(|check| check.ok) && validation.is_ok(),
        "integrity: {report:#?}, import validation: {validation:?}"
    );
    let imported_report = target.import_data(&export).await.unwrap();
    assert!(imported_report.quick_check_ok);
    assert!(imported_report.checks.iter().all(|check| check.ok));
    let imported = target
        .export_data(export.exported_at.clone())
        .await
        .unwrap();
    for (before, after) in [
        (
            serde_json::to_value(&export.tables.recurrence_series).unwrap(),
            serde_json::to_value(&imported.tables.recurrence_series).unwrap(),
        ),
        (
            serde_json::to_value(&export.tables.recurrence_occurrences).unwrap(),
            serde_json::to_value(&imported.tables.recurrence_occurrences).unwrap(),
        ),
        (
            serde_json::to_value(&export.tables.tasks).unwrap(),
            serde_json::to_value(&imported.tables.tasks).unwrap(),
        ),
    ] {
        assert_eq!(before, after);
    }
}

#[tokio::test]
async fn stopped_final_outcomes_pass_integrity_and_portable_import() {
    for start_day in [20, 22] {
        for outcome in [RecurrenceOutcome::Completed, RecurrenceOutcome::Skipped] {
            let (temp, mut conn, workspace) = setup().await;
            let created = create_recurrence_series(
                &mut conn,
                &workspace,
                CreateRecurrenceSeriesParams::new(draft(start_day)).at(at(20, 12)),
            )
            .await
            .unwrap();
            let stopped = stop_recurrence_series(
                &mut conn,
                &workspace,
                &created.series.id,
                false,
                "2026-07-20T15:00:00Z",
            )
            .await
            .unwrap();
            assert_eq!(
                stopped.series.stopped_at.as_deref(),
                Some("2026-07-20T15:00:00Z")
            );
            assert_recurrence_roundtrip(&temp).await;
            let resolved = resolve(
                &mut conn,
                &workspace,
                &created.task.id,
                outcome,
                "2026-07-24T18:00:00Z",
            )
            .await;
            assert!(resolved.successor.is_none());
            assert_eq!(
                resolved.resolved.resolved_at.as_deref(),
                Some("2026-07-24T18:00:00Z")
            );
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM recurrence_occurrences")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(count, 1);
            let series = load_series(&mut conn, &workspace.id, &created.series.id)
                .await
                .unwrap();
            assert_eq!(series.stopped_at.as_deref(), Some("2026-07-20T15:00:00Z"));
            assert_recurrence_roundtrip(&temp).await;
        }
    }
}

#[tokio::test]
async fn stopped_history_rejects_post_stop_archival_and_successors() {
    for corruption in [
        "archive",
        "earlier_outcome",
        "successor",
        "invalid_timestamp",
    ] {
        let (temp, mut conn, workspace) = setup().await;
        let created = create_daily(&mut conn, &workspace).await;
        let first = resolve(
            &mut conn,
            &workspace,
            &created.task.id,
            RecurrenceOutcome::Completed,
            "2026-07-20T13:00:00Z",
        )
        .await;
        let final_task = first.successor.unwrap().id;
        stop_recurrence_series(
            &mut conn,
            &workspace,
            &created.series.id,
            false,
            "2026-07-20T15:00:00Z",
        )
        .await
        .unwrap();
        let final_outcome = resolve(
            &mut conn,
            &workspace,
            &final_task,
            RecurrenceOutcome::Skipped,
            "2026-07-22T18:00:00Z",
        )
        .await;
        assert!(final_outcome.successor.is_none());
        assert_recurrence_roundtrip(&temp).await;
        match corruption {
            "archive" => {
                sqlx::query(
                    "UPDATE recurrence_occurrences SET projection_state = 'archived',
                     outcome = '', resolved_at = '', outcome_change_id = '',
                     archived_at = '2026-07-22T18:00:00Z' WHERE task_id = ?",
                )
                .bind(&final_task)
                .execute(&mut *conn)
                .await
                .unwrap();
            }
            "earlier_outcome" => {
                sqlx::query(
                    "UPDATE recurrence_occurrences SET resolved_at = '2026-07-20T18:00:00Z'
                     WHERE task_id = ?",
                )
                .bind(&created.task.id)
                .execute(&mut *conn)
                .await
                .unwrap();
                // An earlier outcome cannot be the retained final occurrence, even
                // when its change claims it generated no successor.
                sqlx::query(
                    "UPDATE changes SET payload = json_set(payload, '$.resolved_at',
                     '2026-07-20T18:00:00Z', '$.successor_task_id', '') WHERE change_id = ?",
                )
                .bind(&first.resolved.outcome_change_id)
                .execute(&mut *conn)
                .await
                .unwrap();
            }
            "successor" => {
                sqlx::query(
                    "UPDATE changes SET payload = json_set(payload, '$.successor_task_id', ?)
                     WHERE change_id = ?",
                )
                .bind(&created.task.id)
                .bind(&final_outcome.resolved.outcome_change_id)
                .execute(&mut *conn)
                .await
                .unwrap();
            }
            "invalid_timestamp" => {
                sqlx::query(
                    "UPDATE recurrence_occurrences SET resolved_at = 'invalid' WHERE task_id = ?",
                )
                .bind(&final_task)
                .execute(&mut *conn)
                .await
                .unwrap();
            }
            _ => unreachable!(),
        }
        let database = crate::db::Database::open(&temp.path().join("test.sqlite"))
            .await
            .unwrap();
        let report = database.database_integrity_report().await.unwrap();
        assert!(
            !report
                .checks
                .iter()
                .find(|check| check.label == "recurrence stop boundaries")
                .unwrap()
                .ok,
            "{corruption}: {report:#?}"
        );
        let export = database
            .export_data("2026-07-24T18:00:00Z".into())
            .await
            .unwrap();
        let error = database
            .validate_import_data(&export)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("stop boundary") || error.contains("occurrence timestamp"),
            "{corruption}: {error}"
        );
        assert!(database.import_data(&export).await.is_err());
    }
}

#[tokio::test]
async fn stopped_series_keeps_final_task_and_skip_current_creates_no_successor() {
    let (temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    let stopped = stop_recurrence_series(
        &mut conn,
        &workspace,
        &created.series.id,
        false,
        "2026-07-20T15:00:00Z",
    )
    .await
    .unwrap();
    assert_eq!(stopped.series.state, RecurrenceSeriesState::Stopped);
    assert_eq!(
        stopped.occurrence.as_ref().unwrap().task_id.as_ref(),
        Some(&created.task.id)
    );
    let resolved = resolve(
        &mut conn,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Completed,
        "2026-07-20T18:00:00Z",
    )
    .await;
    assert!(resolved.successor.is_none());
    assert_recurrence_roundtrip(&temp).await;

    let second = create_recurrence_series(
        &mut conn,
        &workspace,
        CreateRecurrenceSeriesParams::new(draft(20)).at(at(20, 12)),
    )
    .await
    .unwrap();
    let stopped = stop_recurrence_series(
        &mut conn,
        &workspace,
        &second.series.id,
        true,
        "2026-07-20T16:00:00Z",
    )
    .await
    .unwrap();
    assert_eq!(
        stopped.occurrence.unwrap().outcome,
        Some(RecurrenceOutcome::Skipped)
    );
    let projected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND projection_state = 'projected'",
    )
    .bind(&workspace.id)
    .bind(&second.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(projected, 0);
}

#[tokio::test]
async fn task_mutation_routing_rejects_delete_reopen_and_archived_edits() {
    let (_temp, mut conn, workspace) = setup().await;
    let current_at = Utc::now();
    let mut current_draft = draft(20);
    current_draft.schedule = RecurrenceSchedule::new(
        RecurrenceRule::daily(),
        "UTC".parse::<TimeZoneId>().unwrap(),
        current_at.date_naive(),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    let created = create_recurrence_series(
        &mut conn,
        &workspace,
        CreateRecurrenceSeriesParams::new(current_draft).at(current_at),
    )
    .await
    .unwrap();
    let delete_error =
        crate::mutation::set_task_field(&mut conn, &workspace, &created.task.id, "deleted", "1")
            .await
            .unwrap_err();
    assert!(
        delete_error
            .to_string()
            .contains("recurrence-current-delete")
    );

    let resolved_at = format_utc(current_at + chrono::Duration::hours(1));
    let resolved = resolve(
        &mut conn,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Completed,
        &resolved_at,
    )
    .await;
    let reopen_error =
        crate::mutation::set_task_field(&mut conn, &workspace, &created.task.id, "status", "todo")
            .await
            .unwrap_err();
    assert!(reopen_error.to_string().contains("terminal-reopen"));
    crate::mutation::set_task_field(&mut conn, &workspace, &created.task.id, "deleted", "1")
        .await
        .unwrap();
    crate::mutation::set_task_field(&mut conn, &workspace, &created.task.id, "deleted", "0")
        .await
        .unwrap();

    let successor = resolved.successor.unwrap();
    reconcile_recurrence_series_once(
        &mut conn,
        &workspace,
        &created.series.id,
        current_at + chrono::Duration::days(6),
    )
    .await
    .unwrap();
    let archived_error = crate::mutation::set_task_field(
        &mut conn,
        &workspace,
        &successor.id,
        "title",
        "should fail",
    )
    .await
    .unwrap_err();
    assert!(archived_error.to_string().contains("occurrence-archived"));

    let note_error = crate::operations::tasks::add_note_operation(
        &mut conn,
        &workspace,
        &successor.id,
        "should fail".to_string(),
        false,
    )
    .await
    .err()
    .unwrap();
    assert!(note_error.to_string().contains("occurrence-archived"));

    let current = load_projected_occurrence(&mut conn, &workspace.id, &created.series.id)
        .await
        .unwrap()
        .unwrap()
        .task_id
        .unwrap();
    let dependency_error =
        crate::operations::add_task_dependency(&mut conn, &workspace, &current, &successor.id)
            .await
            .err()
            .unwrap();
    assert!(dependency_error.to_string().contains("occurrence-archived"));
    let epic_error =
        crate::operations::add_task_to_epic(&mut conn, &workspace, &successor.id, &current)
            .await
            .err()
            .unwrap();
    assert!(epic_error.to_string().contains("occurrence-archived"));
}

#[tokio::test]
async fn stale_occurrence_mutations_reconcile_before_resolving_or_pausing() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;

    let mut tx = begin_immediate(&mut conn).await.unwrap();
    let error = resolve_recurrence_occurrence_in_transaction(
        &mut tx,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Completed,
        "2026-07-26T10:00:00Z",
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("occurrence-not-current"));
    tx.rollback().await.unwrap();

    let paused = pause_recurrence_series(
        &mut conn,
        &workspace,
        &created.series.id,
        "2026-07-26T11:00:00Z",
    )
    .await
    .unwrap();
    let projected = paused.occurrence.unwrap();
    assert_eq!(
        projected.slot_on,
        NaiveDate::from_ymd_opt(2026, 7, 26).unwrap()
    );
    assert_ne!(projected.task_id.as_ref(), Some(&created.task.id));
}

#[tokio::test]
async fn immediate_undo_removes_untouched_active_successor_and_restores_tip() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    let resolved = resolve(
        &mut conn,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Completed,
        "2026-07-20T18:00:00Z",
    )
    .await;
    let successor_id = resolved.successor.unwrap().id;

    let mut tx = begin_immediate(&mut conn).await.unwrap();
    assert!(
        undo_recurrence_resolution(&mut tx, &workspace.id, &created.task.id, "todo", "done",)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    let restored = get_task_in_workspace(&mut conn, &workspace, &created.task.id)
        .await
        .unwrap();
    assert_eq!(restored.status, TaskStatus::Todo);
    let occurrence = load_projected_occurrence(&mut conn, &workspace.id, &created.series.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(occurrence.task_id.as_ref(), Some(&created.task.id));
    let successor_exists: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND id = ?")
            .bind(&workspace.id)
            .bind(&successor_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(successor_exists, 0);
}

#[tokio::test]
async fn immediate_undo_keeps_successor_referenced_by_either_related_endpoint() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    let resolved = resolve(
        &mut conn,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Completed,
        "2026-07-20T18:00:00Z",
    )
    .await;
    let successor_id = resolved.successor.unwrap().id;
    let project_id: crate::ids::ProjectId =
        sqlx::query_scalar("SELECT project_id FROM tasks WHERE id = ?")
            .bind(&successor_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let endpoint_ids = [
        "0000000000000001".parse::<TaskId>().unwrap(),
        "7ZZZZZZZZZZZZZZZ".parse::<TaskId>().unwrap(),
    ];
    for endpoint_id in &endpoint_ids {
        sqlx::query(
            "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES (?, ?, 'related endpoint', '', ?, 'todo', 'none', 't', 't')",
        )
        .bind(endpoint_id)
        .bind(&workspace.id)
        .bind(&project_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        crate::operations::set_task_related_link_in_transaction(
            &mut conn,
            &workspace,
            endpoint_id,
            &successor_id,
            true,
        )
        .await
        .unwrap();
    }

    let mut tx = begin_immediate(&mut conn).await.unwrap();
    let error =
        undo_recurrence_resolution(&mut tx, &workspace.id, &created.task.id, "todo", "done")
            .await
            .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("recurrence-undo-successor-touched")
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn immediate_undo_restores_paused_tip_without_creating_successor() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    pause_recurrence_series(
        &mut conn,
        &workspace,
        &created.series.id,
        "2026-07-20T12:30:00Z",
    )
    .await
    .unwrap();
    resolve(
        &mut conn,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Skipped,
        "2026-07-20T13:00:00Z",
    )
    .await;

    let mut tx = begin_immediate(&mut conn).await.unwrap();
    assert!(
        undo_recurrence_resolution(&mut tx, &workspace.id, &created.task.id, "todo", "canceled",)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let occurrence = load_projected_occurrence(&mut conn, &workspace.id, &created.series.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(occurrence.task_id.as_ref(), Some(&created.task.id));
    assert!(occurrence.outcome.is_none());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ?",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn project_delete_stops_active_and_paused_series_atomically() {
    let (_temp, mut conn, workspace) = setup().await;
    let active = create_daily(&mut conn, &workspace).await;
    let paused = create_recurrence_series(
        &mut conn,
        &workspace,
        CreateRecurrenceSeriesParams::new(draft(20)).at(at(20, 13)),
    )
    .await
    .unwrap();
    pause_recurrence_series(
        &mut conn,
        &workspace,
        &paused.series.id,
        "2026-07-20T14:00:00Z",
    )
    .await
    .unwrap();

    let outcome =
        crate::operations::projects::delete_project_operation(&mut conn, &workspace, "recurrence")
            .await
            .unwrap();

    assert_eq!(outcome.stopped_series_count, 2);
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT state FROM recurrence_series WHERE workspace_id = ? ORDER BY id",
    )
    .bind(&workspace.id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(states, vec!["stopped", "stopped"]);
    let stopped_changes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM changes WHERE op_type = 'stop_recurrence_series'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(stopped_changes, 2);
    let open_pauses: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_pause_intervals
         WHERE workspace_id = ? AND resumed_at = ''",
    )
    .bind(&workspace.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(open_pauses, 0);
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted FROM projects WHERE workspace_id = ? AND id = ?")
            .bind(&workspace.id)
            .bind(&active.series.project_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(deleted);
    let projected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences
         WHERE workspace_id = ? AND projection_state = 'projected'",
    )
    .bind(&workspace.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(projected, 2);
}

#[tokio::test]
async fn typed_structural_requests_preserve_occurrence_write_policy() {
    let (_temp, mut conn, workspace) = setup().await;
    let ordinary = crate::operations::create_task(
        &mut conn,
        &workspace,
        crate::operations::TaskDraft {
            title: "ordinary".to_string(),
            description: String::new(),
            project: Some("recurrence".to_string()),
            status: "todo".to_string(),
            priority: "none".to_string(),
            source: crate::choices::TaskSource::Unknown,
            labels: Vec::new(),
            metadata: Vec::new(),
            available_at: None,
            due_on: None,
            is_epic: false,
        },
    )
    .await
    .unwrap();
    let active = create_daily(&mut conn, &workspace).await;
    let paused = create_daily(&mut conn, &workspace).await;
    pause_recurrence_series(
        &mut conn,
        &workspace,
        &paused.series.id,
        "2026-07-20T13:00:00Z",
    )
    .await
    .unwrap();
    let stopped = create_daily(&mut conn, &workspace).await;
    stop_recurrence_series(
        &mut conn,
        &workspace,
        &stopped.series.id,
        false,
        "2026-07-20T13:00:00Z",
    )
    .await
    .unwrap();
    let resolved = create_daily(&mut conn, &workspace).await;
    resolve(
        &mut conn,
        &workspace,
        &resolved.task.id,
        RecurrenceOutcome::Completed,
        "2026-07-20T13:00:00Z",
    )
    .await;
    let archived = create_daily(&mut conn, &workspace).await;
    reconcile_recurrence_series_once(&mut conn, &workspace, &archived.series.id, at(21, 12))
        .await
        .unwrap();

    for intent in [
        RecurrenceStructuralMutation::Labels,
        RecurrenceStructuralMutation::Notes,
        RecurrenceStructuralMutation::Attachments,
        RecurrenceStructuralMutation::Dependencies,
        RecurrenceStructuralMutation::EpicMembership,
    ] {
        let mut tx = begin_immediate(&mut conn).await.unwrap();
        for task in [
            &ordinary.task,
            &active.task,
            &paused.task,
            &stopped.task,
            &resolved.task,
        ] {
            assert_eq!(
                route_recurrence_task_mutation(
                    &mut tx,
                    &workspace,
                    &task.id,
                    RecurrenceTaskMutation::Structural(intent),
                    "2026-07-20T14:00:00Z",
                )
                .await
                .unwrap(),
                RecurrenceMutationOutcome::Proceed,
                "{intent:?} for {}",
                task.id,
            );
        }
        let error = route_recurrence_task_mutation(
            &mut tx,
            &workspace,
            &archived.task.id,
            RecurrenceTaskMutation::Structural(intent),
            "2026-07-21T14:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("recurrence-occurrence-archived"));
        let boundary_error = route_recurrence_task_mutation(
            &mut tx,
            &workspace,
            &active.task.id,
            RecurrenceTaskMutation::Structural(intent),
            "2026-07-21T00:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(
            boundary_error
                .to_string()
                .contains("recurrence-occurrence-archived")
        );
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn typed_gate_uses_owner_clock_at_slot_boundary_and_rolls_back_reconciliation() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    let before = materialization_snapshot(
        &mut conn,
        &workspace.id,
        &created.series.id,
        &created.task.id,
    )
    .await;
    let mut tx = begin_immediate(&mut conn).await.unwrap();
    assert_eq!(
        route_recurrence_task_mutation(
            &mut tx,
            &workspace,
            &created.task.id,
            RecurrenceTaskMutation::Scalar {
                field: TaskField::Status,
                value: "done"
            },
            "2026-07-20T23:59:59Z",
        )
        .await
        .unwrap(),
        RecurrenceMutationOutcome::Handled,
    );
    let occurrence = load_occurrence_for_task(&mut tx, &workspace.id, &created.task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(occurrence.outcome, Some(RecurrenceOutcome::Completed));
    assert_eq!(
        occurrence.resolved_at.as_deref(),
        Some("2026-07-20T23:59:59Z")
    );
    let task = get_task_in_workspace(&mut tx, &workspace, &created.task.id)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.updated_at, "2026-07-20T23:59:59Z");
    tx.rollback().await.unwrap();
    assert_eq!(
        before,
        materialization_snapshot(
            &mut conn,
            &workspace.id,
            &created.series.id,
            &created.task.id
        )
        .await
    );

    let mut tx = begin_immediate(&mut conn).await.unwrap();
    let error = route_recurrence_task_mutation(
        &mut tx,
        &workspace,
        &created.task.id,
        RecurrenceTaskMutation::Scalar {
            field: TaskField::Status,
            value: "done",
        },
        "2026-07-21T00:00:00Z",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("recurrence-occurrence-archived"));
    let current = load_projected_occurrence(&mut tx, &workspace.id, &created.series.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.slot_on, at(21, 0).date_naive());
    tx.rollback().await.unwrap();
    assert_eq!(
        before,
        materialization_snapshot(
            &mut conn,
            &workspace.id,
            &created.series.id,
            &created.task.id
        )
        .await
    );
}

#[tokio::test]
async fn typed_scalar_gate_preserves_outcomes_and_stopped_final_writes() {
    let (_temp, mut conn, workspace) = setup().await;
    let created = create_daily(&mut conn, &workspace).await;
    stop_recurrence_series(
        &mut conn,
        &workspace,
        &created.series.id,
        false,
        "2026-07-20T13:00:00Z",
    )
    .await
    .unwrap();
    let mut tx = begin_immediate(&mut conn).await.unwrap();
    for (field, value, expected) in [
        (
            TaskField::Title,
            "final task",
            RecurrenceMutationOutcome::Proceed,
        ),
        (
            TaskField::Status,
            "todo",
            RecurrenceMutationOutcome::NoChange,
        ),
        (
            TaskField::Status,
            "active",
            RecurrenceMutationOutcome::Proceed,
        ),
        (TaskField::Deleted, "0", RecurrenceMutationOutcome::Proceed),
    ] {
        assert_eq!(
            route_recurrence_task_mutation(
                &mut tx,
                &workspace,
                &created.task.id,
                RecurrenceTaskMutation::Scalar { field, value },
                "2026-07-22T12:00:00Z"
            )
            .await
            .unwrap(),
            expected
        );
    }
    let error = route_recurrence_task_mutation(
        &mut tx,
        &workspace,
        &created.task.id,
        RecurrenceTaskMutation::Scalar {
            field: TaskField::Deleted,
            value: "1",
        },
        "2026-07-22T12:00:00Z",
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("complete or cancel the final occurrence")
    );
    assert_eq!(
        route_recurrence_task_mutation(
            &mut tx,
            &workspace,
            &created.task.id,
            RecurrenceTaskMutation::Scalar {
                field: TaskField::Status,
                value: "canceled"
            },
            "2026-07-22T12:00:00Z"
        )
        .await
        .unwrap(),
        RecurrenceMutationOutcome::Handled
    );
    assert!(
        load_projected_occurrence(&mut tx, &workspace.id, &created.series.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        load_occurrence_for_task(&mut tx, &workspace.id, &created.task.id)
            .await
            .unwrap()
            .unwrap()
            .outcome,
        Some(RecurrenceOutcome::Skipped)
    );
    assert_eq!(
        route_recurrence_task_mutation(
            &mut tx,
            &workspace,
            &created.task.id,
            RecurrenceTaskMutation::Scalar {
                field: TaskField::Status,
                value: "canceled"
            },
            "2026-07-22T12:00:00Z"
        )
        .await
        .unwrap(),
        RecurrenceMutationOutcome::NoChange
    );
    for value in ["todo", "done"] {
        assert!(
            route_recurrence_task_mutation(
                &mut tx,
                &workspace,
                &created.task.id,
                RecurrenceTaskMutation::Scalar {
                    field: TaskField::Status,
                    value
                },
                "2026-07-22T12:00:00Z"
            )
            .await
            .is_err()
        );
    }
    tx.commit().await.unwrap();
}
