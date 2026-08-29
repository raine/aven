use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use sqlx::Connection;

use super::*;
use crate::operations::{CreateRecurrenceSeriesParams, RecurrenceSeriesDraft};
use crate::query::{
    SearchMatchedField, SortDirection, TaskFilters, TaskQueryMode, TaskSearchQuery, TaskSort,
};
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceRule, RecurrenceSchedule, TimeZoneId,
};
use crate::workspaces::Workspace;

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, day, hour, 0, 0)
        .single()
        .unwrap()
}

fn draft(title: &str, start_day: u32) -> RecurrenceSeriesDraft {
    RecurrenceSeriesDraft {
        metadata: Vec::new(),
        title: title.to_string(),
        description: "recurrence query fixture".to_string(),
        project: "recurrence".to_string(),
        priority: "high".to_string(),
        initial_status: "todo".to_string(),
        labels: Vec::new(),
        schedule: RecurrenceSchedule::new(
            RecurrenceRule::daily(),
            "UTC".parse::<TimeZoneId>().unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, start_day).unwrap(),
            None,
            RecurrenceDuePolicy::SameDay,
        ),
    }
}

async fn setup() -> (tempfile::TempDir, crate::db::Database, Workspace) {
    let temp = tempfile::tempdir().unwrap();
    let database = crate::db::Database::open(&temp.path().join("query.sqlite"))
        .await
        .unwrap();
    let workspace = {
        let mut conn = database.acquire_writer().await.unwrap();
        crate::test_support::ensure_default_workspace(&mut conn)
            .await
            .unwrap()
    };
    (temp, database, workspace)
}

async fn create(
    database: &crate::db::Database,
    workspace: &Workspace,
    title: &str,
    start_day: u32,
) -> crate::operations::RecurrenceCreateOutcome {
    database
        .create_recurrence_series(
            workspace,
            CreateRecurrenceSeriesParams::new(draft(title, start_day)).at(at(start_day, 12)),
        )
        .await
        .unwrap()
}

async fn resolve_at(
    database: &crate::db::Database,
    workspace: &Workspace,
    task_id: &TaskId,
    outcome: RecurrenceOutcome,
    resolved_at: DateTime<Utc>,
) -> crate::operations::RecurrenceResolveOutcome {
    let mut conn = database.acquire_writer().await.unwrap();
    let mut tx = crate::db::begin_immediate(&mut conn).await.unwrap();
    let result = crate::operations::recurrence::resolve_recurrence_occurrence_in_transaction(
        &mut tx,
        workspace,
        task_id,
        outcome,
        &resolved_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    result
}

async fn stop_at(
    database: &crate::db::Database,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    stopped_at: DateTime<Utc>,
) {
    let mut conn = database.acquire_writer().await.unwrap();
    crate::operations::recurrence::stop_recurrence_series(
        &mut conn,
        workspace,
        series_id,
        false,
        &stopped_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
    .await
    .unwrap();
}

async fn list(
    database: &crate::db::Database,
    workspace: &Workspace,
    filters: TaskFilters,
    mode: TaskQueryMode,
) -> Vec<crate::query::TaskListItem> {
    let mut conn = database.acquire_writer().await.unwrap();
    super::super::tasks::list_task_items_in_workspace(
        &mut conn,
        &workspace.id,
        filters,
        mode,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn recurring_series_list_defaults_to_active_and_paused() {
    let (_temp, database, workspace) = setup().await;
    let active = create(&database, &workspace, "Active", 20).await;
    let paused = create(&database, &workspace, "Paused", 20).await;
    database
        .pause_recurrence_series(&workspace, &paused.series.id)
        .await
        .unwrap();
    let stopped = create(&database, &workspace, "Stopped", 20).await;
    database
        .stop_recurrence_series(&workspace, &stopped.series.id, false)
        .await
        .unwrap();

    let items = database
        .list_recurrence_series_view_at(
            &workspace.id,
            at(24, 12),
            crate::query::RecurrenceSeriesListQuery::default(),
        )
        .await
        .unwrap();
    let ids = items
        .iter()
        .map(|item| item.series.id.clone())
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(
        ids,
        [active.series.id, paused.series.id].into_iter().collect()
    );
    assert!(items.iter().all(|item| item.current_occurrence.is_some()));
}

#[tokio::test]
async fn recurring_series_list_filters_lifecycle_and_searches_title_or_ref() {
    let (_temp, database, workspace) = setup().await;
    let active = create(&database, &workspace, "Quarterly Review", 20).await;
    let stopped = create(&database, &workspace, "Retired Review", 20).await;
    database
        .stop_recurrence_series(&workspace, &stopped.series.id, false)
        .await
        .unwrap();

    let stopped_items = database
        .list_recurrence_series_view_at(
            &workspace.id,
            at(24, 12),
            crate::query::RecurrenceSeriesListQuery {
                lifecycle: crate::query::RecurrenceSeriesLifecycleFilter::Stopped,
                ..crate::query::RecurrenceSeriesListQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(stopped_items.len(), 1);
    assert_eq!(stopped_items[0].series.id, stopped.series.id);

    let all = database
        .list_recurrence_series_view_at(
            &workspace.id,
            at(24, 12),
            crate::query::RecurrenceSeriesListQuery {
                lifecycle: crate::query::RecurrenceSeriesLifecycleFilter::All,
                ..crate::query::RecurrenceSeriesListQuery::default()
            },
        )
        .await
        .unwrap();
    let active_ref = all
        .iter()
        .find(|item| item.series.id == active.series.id)
        .unwrap()
        .series_ref
        .to_lowercase();
    for search in ["quarterly".to_string(), active_ref] {
        let matches = database
            .list_recurrence_series_view_at(
                &workspace.id,
                at(24, 12),
                crate::query::RecurrenceSeriesListQuery {
                    lifecycle: crate::query::RecurrenceSeriesLifecycleFilter::All,
                    search: Some(search),
                    ..crate::query::RecurrenceSeriesListQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].series.id, active.series.id);
    }
}

#[tokio::test]
async fn recurring_series_list_respects_project_scope() {
    let (_temp, database, workspace) = setup().await;
    let mobile = database
        .create_project(&workspace, "Mobile")
        .await
        .unwrap()
        .project;
    let default = create(&database, &workspace, "Default review", 20).await;
    let mut mobile_draft = draft("Mobile review", 20);
    mobile_draft.project = mobile.key.clone();
    let mobile_series = database
        .create_recurrence_series(
            &workspace,
            CreateRecurrenceSeriesParams::new(mobile_draft).at(at(20, 12)),
        )
        .await
        .unwrap();

    let items = database
        .list_recurrence_series_view_at(
            &workspace.id,
            at(24, 12),
            crate::query::RecurrenceSeriesListQuery {
                project: Some(mobile.key.clone()),
                ..crate::query::RecurrenceSeriesListQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].series.id, mobile_series.series.id);
    assert_eq!(items[0].project_key, mobile.key);
    assert_ne!(items[0].series.id, default.series.id);
}

#[tokio::test]
async fn recurring_series_list_statement_shapes_stay_bounded_with_history() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "Bounded review", 1).await;
    let mut task_id = created.task.id;
    let mut conn = database.acquire_writer().await.unwrap();
    for day in 1..=24 {
        let mut tx = crate::db::begin_immediate(&mut conn).await.unwrap();
        let outcome = crate::operations::recurrence::resolve_recurrence_occurrence_in_transaction(
            &mut tx,
            &workspace,
            &task_id,
            RecurrenceOutcome::Completed,
            &format!("2026-07-{day:02}T12:00:00Z"),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        task_id = outcome.successor.unwrap().id;
    }

    conn.clear_cached_statements().await.unwrap();
    let items = super::list_recurrence_series_view(
        &mut conn,
        &workspace.id,
        &crate::query::RecurrenceSeriesListQuery::default(),
    )
    .await
    .unwrap();

    assert_eq!(items.len(), 1);
    assert!(items[0].current_occurrence.is_some());
    assert!(conn.cached_statements_size() <= 8);
}

#[tokio::test]
async fn bounded_reconciliation_progresses_past_current_series() {
    let (_temp, database, workspace) = setup().await;
    for index in 0..=REPORT_RECONCILE_LIMIT {
        create(&database, &workspace, &format!("daily report {index}"), 20).await;
    }

    let first = database
        .reconcile_recurrence_reports_at(&workspace.id, at(24, 12))
        .await
        .unwrap();
    let second = database
        .reconcile_recurrence_reports_at(&workspace.id, at(24, 12))
        .await
        .unwrap();

    assert_eq!(first.examined, REPORT_RECONCILE_LIMIT);
    assert_eq!(first.changed, REPORT_RECONCILE_LIMIT);
    assert!(first.incomplete);
    assert_eq!(second.examined, 1);
    assert_eq!(second.changed, 1);
    assert!(!second.incomplete);
}

#[tokio::test]
async fn reconciliation_keeps_one_active_row_and_hydrates_in_batch() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "daily report", 20).await;

    let first = database
        .reconcile_recurrence_reports_at(&workspace.id, at(24, 12))
        .await
        .unwrap();
    let second = database
        .reconcile_recurrence_reports_at(&workspace.id, at(24, 12))
        .await
        .unwrap();
    assert_eq!(first.examined, 1);
    assert_eq!(first.changed, 1);
    assert_eq!(second.changed, 0);
    assert!(!first.incomplete);

    let mut conn = database.acquire_writer().await.unwrap();
    let projected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND projection_state = 'projected'",
    )
    .bind(&workspace.id)
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(projected, 1);
    let items = super::super::tasks::list_task_items_in_workspace(
        &mut conn,
        &workspace.id,
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await
    .unwrap();
    assert_eq!(items.len(), 1);
    let recurrence = items[0].recurrence.as_ref().unwrap();
    assert_eq!(recurrence.series_id, created.series.id);
    assert_eq!(recurrence.slot_on, "2026-07-24");
    assert_eq!(recurrence.series_ref, created.series_ref);
}

#[tokio::test]
async fn paused_and_archived_tasks_leave_all_open_paths_but_stopped_final_stays_visible() {
    let (_temp, database, workspace) = setup().await;
    let paused = create(&database, &workspace, "paused fixture", 20).await;
    database
        .pause_recurrence_series(&workspace, &paused.series.id)
        .await
        .unwrap();

    assert!(
        list(
            &database,
            &workspace,
            TaskFilters {
                hide_done: true,
                ..TaskFilters::default()
            },
            TaskQueryMode::Flat,
        )
        .await
        .is_empty()
    );
    assert!(
        list(
            &database,
            &workspace,
            TaskFilters {
                hide_done: true,
                ..TaskFilters::default()
            },
            TaskQueryMode::RankedQueue,
        )
        .await
        .is_empty()
    );
    let mut conn = database.acquire_writer().await.unwrap();
    let search = super::super::search::search_task_items_in_workspace(
        &mut conn,
        &workspace.id,
        TaskSearchQuery {
            metadata: Vec::new(),
            has_metadata: Vec::new(),
            missing_metadata: Vec::new(),
            text: "paused fixture".to_string(),
            project: None,
            include_deleted: false,
            limit: 20,
        },
    )
    .await
    .unwrap();
    assert!(search.is_empty());
    let counts = super::super::sidebar::sidebar_counts_for_scope_in_workspace(
        &mut conn,
        &workspace.id,
        None,
    )
    .await
    .unwrap();
    assert_eq!(counts.open, 0);
    drop(conn);
    let paused_history = database
        .recurrence_history_at(&workspace.id, &paused.series.id, at(24, 12), 0, 20)
        .await
        .unwrap();
    assert!(
        paused_history
            .items
            .iter()
            .any(|item| item.kind == RecurrenceHistoryKind::Paused)
    );
    assert_eq!(
        database
            .resolve_task_ref(&workspace, paused.task.id.as_ref())
            .await
            .unwrap()
            .id,
        paused.task.id
    );

    let archived = create(&database, &workspace, "archived fixture", 20).await;
    database
        .reconcile_recurrence_series(&workspace, &archived.series.id, at(22, 12))
        .await
        .unwrap();
    let visible = list(
        &database,
        &workspace,
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
    )
    .await;
    assert_eq!(
        visible
            .iter()
            .filter(|item| item
                .recurrence
                .as_ref()
                .is_some_and(|value| { value.series_id == archived.series.id }))
            .count(),
        1
    );
    assert_eq!(
        database
            .resolve_task_ref(&workspace, archived.task.id.as_ref())
            .await
            .unwrap()
            .id,
        archived.task.id
    );

    let stopped = create(&database, &workspace, "stopped fixture", 20).await;
    stop_at(&database, &workspace, &stopped.series.id, at(20, 13)).await;
    let visible = list(
        &database,
        &workspace,
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
    )
    .await;
    assert!(visible.iter().any(|item| item.task.id == stopped.task.id));
}

#[tokio::test]
async fn recurrence_history_pages_preserve_metadata_and_boundaries() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "paged history", 20).await;
    database
        .reconcile_recurrence_series(&workspace, &created.series.id, at(24, 12))
        .await
        .unwrap();
    let first = database
        .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 0, 2)
        .await
        .unwrap();
    let second = database
        .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 2, 2)
        .await
        .unwrap();

    let boundary = database
        .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 0, 500)
        .await
        .unwrap();

    assert_eq!(boundary.limit, 500);
    for limit in [0, 501, 900] {
        let error = database
            .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 0, limit)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "error recurrence-history-limit-invalid limit={limit} min=1 max=500 hint=\"pass a limit between 1 and 500\""
            )
        );
    }

    assert_eq!(first.items.len(), 2);
    assert_eq!(first.offset, 0);
    assert_eq!(first.limit, 2);
    assert!(first.has_more);
    assert_eq!(second.items.len(), 2);
    assert_eq!(second.offset, 2);
    assert!(!second.has_more);
    assert_eq!(first.total, second.total);
    for entry in first.items.iter().chain(&second.items) {
        assert_ne!(entry.slot_on.is_some(), entry.interval_started_at.is_some());
        assert_eq!(entry.openable, entry.task_id.is_some());
    }
}

#[tokio::test]
async fn history_pagination_seeks_over_an_ancient_schedule() {
    let (_temp, database, workspace) = setup().await;
    let mut long_draft = draft("ancient history", 1);
    long_draft.schedule.start_on = NaiveDate::from_ymd_opt(1900, 1, 1).unwrap();
    let created = database
        .create_recurrence_series(
            &workspace,
            CreateRecurrenceSeriesParams::new(long_draft).at(at(24, 12)),
        )
        .await
        .unwrap();

    let history = database
        .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 0, 2)
        .await
        .unwrap();

    assert_eq!(
        history
            .items
            .iter()
            .filter_map(|entry| entry.slot_on.as_deref())
            .collect::<Vec<_>>(),
        ["2026-07-23", "2026-07-22"]
    );
    assert!(history.total > 40_000);
    assert!(history.has_more);
}

#[tokio::test]
async fn history_includes_lattice_slots_before_series_creation() {
    let (_temp, database, workspace) = setup().await;
    let created = database
        .create_recurrence_series(
            &workspace,
            CreateRecurrenceSeriesParams::new(draft("past start", 20)).at(at(24, 12)),
        )
        .await
        .unwrap();

    let history = database
        .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 0, 20)
        .await
        .unwrap();
    assert_eq!(history.total, 4);
    assert!(
        history
            .items
            .iter()
            .all(|entry| { entry.kind == RecurrenceHistoryKind::Missed && !entry.openable })
    );
    assert_eq!(
        history
            .items
            .iter()
            .filter_map(|entry| entry.slot_on.as_deref())
            .collect::<Vec<_>>(),
        ["2026-07-23", "2026-07-22", "2026-07-21", "2026-07-20"]
    );
}

#[tokio::test]
async fn history_combines_task_outcomes_archived_and_derived_rows() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "history fixture", 20).await;
    resolve_at(
        &database,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Completed,
        at(20, 18),
    )
    .await;
    database
        .reconcile_recurrence_series(&workspace, &created.series.id, at(24, 12))
        .await
        .unwrap();
    stop_at(&database, &workspace, &created.series.id, at(24, 12)).await;
    let history = database
        .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 0, 20)
        .await
        .unwrap();
    assert_eq!(history.total, 4);
    let derived_22 = history
        .items
        .iter()
        .find(|item| item.slot_on.as_deref() == Some("2026-07-22"))
        .unwrap();
    assert_eq!(derived_22.kind, RecurrenceHistoryKind::Missed);
    assert!(!derived_22.openable);
    let archived = history
        .items
        .iter()
        .find(|item| item.slot_on.as_deref() == Some("2026-07-21"))
        .unwrap();
    assert_eq!(archived.kind, RecurrenceHistoryKind::Missed);
    assert!(archived.archived_projection);
    assert!(archived.openable);
    assert!(archived.task_ref.is_some());
    let derived = history
        .items
        .iter()
        .find(|item| item.slot_on.as_deref() == Some("2026-07-23"))
        .unwrap();
    assert_eq!(derived.kind, RecurrenceHistoryKind::Missed);
    assert!(!derived.openable);

    let grouped = list(
        &database,
        &workspace,
        TaskFilters {
            status: Some("done".to_string()),
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
    )
    .await;
    assert_eq!(grouped.len(), 1);
    let group = grouped[0].recurrence_group.as_ref().unwrap();
    assert_eq!(group.counts.completed, 1);
    assert_eq!(group.counts.missed, 3);

    let expanded = list(
        &database,
        &workspace,
        TaskFilters {
            status: Some("done".to_string()),
            expand_recurring: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
    )
    .await;
    assert_eq!(expanded.len(), 1);
    assert!(expanded[0].recurrence_group.is_none());

    let mut conn = database.acquire_writer().await.unwrap();
    let sidebar = super::super::sidebar::sidebar_counts_for_scope_in_workspace(
        &mut conn,
        &workspace.id,
        None,
    )
    .await
    .unwrap();
    assert_eq!(sidebar.done, 1);
    let grouped_search = super::super::search::search_task_items_in_workspace(
        &mut conn,
        &workspace.id,
        TaskSearchQuery {
            metadata: Vec::new(),
            has_metadata: Vec::new(),
            missing_metadata: Vec::new(),
            text: "history fixture".to_string(),
            project: None,
            include_deleted: false,
            limit: 20,
        },
    )
    .await
    .unwrap();
    assert_eq!(grouped_search.len(), 1);
    assert!(grouped_search[0].item.recurrence_group.is_some());
    let expanded_search = super::super::search::search_task_occurrence_items_in_workspace(
        &mut conn,
        &workspace.id,
        TaskSearchQuery {
            metadata: Vec::new(),
            has_metadata: Vec::new(),
            missing_metadata: Vec::new(),
            text: "history fixture".to_string(),
            project: None,
            include_deleted: false,
            limit: 20,
        },
    )
    .await
    .unwrap();
    assert_eq!(expanded_search.len(), 2);
}

#[tokio::test]
async fn refs_and_recent_actions_resolve_and_group_successor_projection() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "recent fixture", 20).await;
    assert_eq!(
        database
            .resolve_recurrence_ref(&workspace, &created.series_ref)
            .await
            .unwrap()
            .id,
        created.series.id
    );
    assert_eq!(
        database
            .resolve_recurrence_ref(&workspace, created.task.id.as_ref())
            .await
            .unwrap()
            .id,
        created.series.id
    );

    let resolved = resolve_at(
        &database,
        &workspace,
        &created.task.id,
        RecurrenceOutcome::Skipped,
        at(20, 18),
    )
    .await;
    let successor = resolved.successor.unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let status_change_id: String = sqlx::query_scalar(
        "SELECT json_extract(payload, '$.task_status_change_id')
         FROM changes
         WHERE entity_type = 'recurrence_series'
           AND entity_id = ?
           AND op_type = 'resolve_recurrence_occurrence'",
    )
    .bind(&created.series.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let successor_create_id: String = sqlx::query_scalar(
        "SELECT change_id FROM changes
         WHERE entity_type = 'task' AND entity_id = ? AND op_type = 'create_task'",
    )
    .bind(&successor.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let successor_projection_id: String = sqlx::query_scalar(
        "SELECT change_id FROM changes
         WHERE entity_type = 'recurrence_series'
           AND entity_id = ?
           AND op_type = 'project_recurrence_occurrence'
           AND json_extract(payload, '$.task_id') = ?",
    )
    .bind(&created.series.id)
    .bind(&successor.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let actions = super::super::recent_actions::list_recent_actions_in_workspace(
        &mut conn,
        &workspace.id,
        None,
    )
    .await
    .unwrap();
    let resolution = actions
        .iter()
        .find(|item| item.op_type == crate::change_log::op_type::RESOLVE_RECURRENCE_OCCURRENCE)
        .unwrap();
    assert_eq!(
        resolution.target.display_ref.as_deref(),
        Some(created.series_ref.as_str())
    );
    assert_eq!(resolution.grouped_change_count, 4);
    for suppressed_change_id in [
        status_change_id,
        successor_create_id,
        successor_projection_id,
    ] {
        assert!(
            !actions
                .iter()
                .any(|item| item.change_id == suppressed_change_id)
        );
    }
}

#[tokio::test]
async fn recurrence_hydration_statement_shapes_stay_bounded() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "batch fixture", 1).await;
    let mut task_id = created.task.id;
    let mut conn = database.acquire_writer().await.unwrap();
    for day in 1..=24 {
        let mut tx = crate::db::begin_immediate(&mut conn).await.unwrap();
        let outcome = crate::operations::recurrence::resolve_recurrence_occurrence_in_transaction(
            &mut tx,
            &workspace,
            &task_id,
            RecurrenceOutcome::Completed,
            &format!("2026-07-{day:02}T12:00:00Z"),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        task_id = outcome.successor.unwrap().id;
    }

    conn.clear_cached_statements().await.unwrap();
    let items = super::super::tasks::list_task_items_in_workspace(
        &mut conn,
        &workspace.id,
        TaskFilters {
            status: Some("done".to_string()),
            expand_recurring: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await
    .unwrap();
    assert_eq!(items.len(), 24);
    assert!(items.iter().all(|item| item.recurrence.is_some()));
    let statement_count = conn.cached_statements_size();
    assert!(statement_count <= 18, "statement count: {statement_count}");
}

#[tokio::test]
async fn search_resolves_recurrence_series_refs_to_their_occurrences() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "series ref fixture", 6).await;
    let series_ref = created.series_ref.clone();
    let suffix = series_ref.split_once('-').unwrap().1.to_string();

    let search = |text: String| {
        let database = database.clone();
        let workspace_id = workspace.id.clone();
        async move {
            let mut conn = database.acquire_writer().await.unwrap();
            super::super::search::search_task_items_in_workspace(
                &mut conn,
                &workspace_id,
                TaskSearchQuery {
                    metadata: Vec::new(),
                    has_metadata: Vec::new(),
                    missing_metadata: Vec::new(),
                    text,
                    project: None,
                    include_deleted: false,
                    limit: 20,
                },
            )
            .await
            .unwrap()
        }
    };

    // The series ref is what the UI shows for a recurring row, so searching it
    // has to land on the occurrence the series currently projects.
    let qualified = search(series_ref).await;
    assert_eq!(qualified.len(), 1);
    assert_eq!(qualified[0].item.task.id, created.task.id);
    assert_eq!(qualified[0].matched_field, SearchMatchedField::Ref);

    let bare = search(suffix.clone()).await;
    assert_eq!(bare.len(), 1);
    assert_eq!(bare[0].item.task.id, created.task.id);
    assert_eq!(bare[0].matched_field, SearchMatchedField::Ref);

    let wrong_prefix = search(format!("/APP-{suffix}")).await;
    assert!(wrong_prefix.is_empty());
}
