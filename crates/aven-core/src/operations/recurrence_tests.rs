use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll, Wake, Waker};

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
    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

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
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = TaskContext::from_waker(&waker);

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

#[tokio::test]
async fn stopped_series_keeps_final_task_and_skip_current_creates_no_successor() {
    let (_temp, mut conn, workspace) = setup().await;
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
