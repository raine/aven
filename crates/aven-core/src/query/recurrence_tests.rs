use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use sqlx::Connection;

use super::*;
use crate::operations::RecurrenceSeriesDraft;
use crate::query::{SortDirection, TaskFilters, TaskQueryMode, TaskSearchQuery, TaskSort};
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
        let mut conn = database.acquire().await.unwrap();
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
        .create_recurrence_series_at(workspace, draft(title, start_day), at(start_day, 12))
        .await
        .unwrap()
}

async fn list(
    database: &crate::db::Database,
    workspace: &Workspace,
    filters: TaskFilters,
    mode: TaskQueryMode,
) -> Vec<crate::query::TaskListItem> {
    let mut conn = database.acquire().await.unwrap();
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

    let mut conn = database.acquire().await.unwrap();
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
    let mut conn = database.acquire().await.unwrap();
    let search = super::super::search::search_task_items_in_workspace(
        &mut conn,
        &workspace.id,
        TaskSearchQuery {
            text: "paused fixture".to_string(),
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
    database
        .stop_recurrence_series(&workspace, &stopped.series.id, false)
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
    assert!(visible.iter().any(|item| item.task.id == stopped.task.id));
}

#[tokio::test]
async fn history_combines_grouped_explicit_corrected_archived_and_derived_rows() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "history fixture", 20).await;
    database
        .resolve_recurrence_occurrence(&workspace, &created.task.id, RecurrenceOutcome::Completed)
        .await
        .unwrap();
    database
        .reconcile_recurrence_series(&workspace, &created.series.id, at(24, 12))
        .await
        .unwrap();
    database
        .record_recurrence_outcome(
            &workspace,
            &created.series.id,
            NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
            RecurrenceOutcome::Completed,
            "2026-07-22T18:00:00Z".to_string(),
            at(24, 12),
        )
        .await
        .unwrap();

    let history = database
        .recurrence_history_at(&workspace.id, &created.series.id, at(24, 12), 0, 20)
        .await
        .unwrap();
    assert_eq!(history.total, 4);
    let corrected = history
        .items
        .iter()
        .find(|item| item.slot_on.as_deref() == Some("2026-07-22"))
        .unwrap();
    assert_eq!(corrected.kind, RecurrenceHistoryKind::Completed);
    assert!(corrected.corrected);
    assert!(!corrected.openable);
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
    assert_eq!(group.counts.completed, 2);
    assert_eq!(group.counts.missed, 2);

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

    let mut conn = database.acquire().await.unwrap();
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
            text: "history fixture".to_string(),
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
            text: "history fixture".to_string(),
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

    let resolved = database
        .resolve_recurrence_occurrence(&workspace, &created.task.id, RecurrenceOutcome::Skipped)
        .await
        .unwrap();
    let successor = resolved.successor.unwrap();
    let mut conn = database.acquire().await.unwrap();
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
    assert!(!actions.iter().any(|item| {
        item.entity_id == successor.id.as_str()
            && item.op_type == crate::change_log::op_type::CREATE_TASK
    }));
}

#[tokio::test]
async fn recurrence_hydration_statement_shapes_stay_bounded() {
    let (_temp, database, workspace) = setup().await;
    let created = create(&database, &workspace, "batch fixture", 1).await;
    let mut task_id = created.task.id;
    let mut conn = database.acquire().await.unwrap();
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
    assert!(conn.cached_statements_size() <= 16);
}
