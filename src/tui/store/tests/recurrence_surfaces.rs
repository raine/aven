use super::*;
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule, TimeZoneId};
use chrono::Utc;

async fn create_daily(store: &mut TuiStore) -> (TaskId, usize) {
    create_daily_named(store, "Daily journal").await
}

async fn create_daily_named(store: &mut TuiStore, title: &str) -> (TaskId, usize) {
    let schedule = RecurrenceSchedule::new(
        RecurrenceRule::daily(),
        "UTC".parse::<TimeZoneId>().unwrap(),
        Utc::now().date_naive(),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    let (_, selected) = store
        .create_recurrence_series(
            recurrence_draft(
                title.to_string(),
                "Write one entry".to_string(),
                None,
                "medium".to_string(),
                "todo".to_string(),
                Vec::new(),
                schedule,
            ),
            None,
        )
        .await
        .unwrap();
    let selected = selected.unwrap();
    (store.tasks[selected].task.id.clone(), selected)
}

#[tokio::test]
async fn recurring_series_view_includes_paused_and_filters_stopped() {
    let mut store = test_store().await;
    let (_, selected) = create_daily(&mut store).await;
    let series_id = store.tasks[selected]
        .recurrence
        .as_ref()
        .unwrap()
        .series_id
        .clone();
    let (_, stopped_selected) = create_daily_named(&mut store, "Stopped journal").await;
    let stopped_id = store.tasks[stopped_selected]
        .recurrence
        .as_ref()
        .unwrap()
        .series_id
        .clone();
    store
        .database
        .stop_recurrence_series(&store.active_workspace, &stopped_id, false)
        .await
        .unwrap();
    store
        .database
        .pause_recurrence_series(&store.active_workspace, &series_id)
        .await
        .unwrap();

    store.show_view(TaskQuery::Recurring).await.unwrap();
    assert_eq!(store.recurrence_series.len(), 1);
    assert_eq!(store.recurrence_series[0].series.id, series_id);
    assert!(store.tasks.is_empty());

    store.view_state.recurring.lifecycle = crate::query::RecurrenceSeriesLifecycleFilter::Stopped;
    store.refresh(None).await.unwrap();
    assert_eq!(store.recurrence_series.len(), 1);
    assert_eq!(store.recurrence_series[0].series.id, stopped_id);
    assert_eq!(
        store.recurrence_series[0].series.state,
        aven_core::recurrence::RecurrenceSeriesState::Stopped
    );
}

#[tokio::test]
async fn recurring_series_search_and_refresh_restore_series_identity() {
    let mut store = test_store().await;
    create_daily_named(&mut store, "Alpha journal").await;
    create_daily_named(&mut store, "Beta journal").await;
    store.show_view(TaskQuery::Recurring).await.unwrap();
    let beta = store
        .recurrence_series
        .iter()
        .find(|item| item.series.title == "Beta journal")
        .unwrap()
        .series
        .id
        .clone();

    let selected = store
        .set_recurring_search("beta".to_string(), Some(&beta))
        .await
        .unwrap();
    assert_eq!(store.recurrence_series.len(), 1);
    assert_eq!(
        store
            .selected_recurrence_series(selected)
            .unwrap()
            .series
            .id,
        beta
    );

    store.load_recurrence_series_detail(&beta).await.unwrap();
    store.view_state.recurring.search = None;
    let selection = MainRowSelection::RecurrenceSeries(beta.clone());
    let selected = store
        .refresh_with_scope_fallback(Some(&selection))
        .await
        .unwrap()
        .selected;
    assert_eq!(
        store
            .selected_recurrence_series(selected)
            .unwrap()
            .series
            .id,
        beta
    );
    assert_eq!(store.recurrence_detail.as_ref().unwrap().series.id, beta);
}

#[tokio::test]
async fn recurrence_store_creation_hydrates_series_and_slot_metadata() {
    let mut store = test_store().await;
    let (_, selected) = create_daily(&mut store).await;

    let recurrence = store.tasks[selected].recurrence.as_ref().unwrap();
    assert!(recurrence.series_ref.starts_with("RCR-"));
    assert_eq!(recurrence.rule_label, "daily");
    assert_eq!(recurrence.timezone, "UTC");
    assert_eq!(recurrence.slot_on, Utc::now().date_naive().to_string());
}

#[tokio::test]
async fn paused_projection_leaves_ordinary_views_but_direct_access_remains() {
    let mut store = test_store().await;
    let (task_id, selected) = create_daily(&mut store).await;

    let series_id = store.tasks[selected]
        .recurrence
        .as_ref()
        .unwrap()
        .series_id
        .clone();
    store
        .pause_recurrence(&series_id, Some(&task_id))
        .await
        .unwrap();
    assert!(!store.tasks.iter().any(|item| item.task.id == task_id));

    store.view_state.query = TaskQuery::Search;
    store.view_state.projection_origin =
        super::super::TaskProjectionOrigin::ExactTasks(vec![task_id.clone()]);
    store.refresh(Some(&task_id)).await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.id, task_id);
    assert_eq!(
        store.tasks[0].recurrence.as_ref().unwrap().lifecycle,
        aven_core::recurrence::RecurrenceSeriesState::Paused
    );
}

#[tokio::test]
async fn done_view_uses_one_grouped_series_history_row() {
    let mut store = test_store().await;
    let (_, selected) = create_daily(&mut store).await;
    store.update_status(Some(selected), "done").await.unwrap();

    store.show_view(TaskQuery::Done).await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    let group = store.tasks[0].recurrence_group.as_ref().unwrap();
    assert_eq!(group.counts.completed, 1);
    assert_eq!(group.counts.skipped, 0);
}

#[tokio::test]
async fn lifecycle_conflict_appears_in_needs_action_with_series_target() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (_, selected) = create_daily(&mut store).await;
    let recurrence = store.tasks[selected].recurrence.clone().unwrap();
    let workspace_id = store.active_workspace.id.clone();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(
            workspace_id, entity_type, entity_id, task_id, field, base_version,
            local_value, remote_value, local_change_id, remote_change_id,
            variant_a, variant_b, created_at, resolved
         ) VALUES (?, 'recurrence_series', ?, '', 'state', NULL,
            'active', 'paused', NULL, ?, 'local', 'remote', ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&recurrence.series_id)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    store.show_view(TaskQuery::Conflicts).await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert!(store.tasks[0].has_conflict);
    let targets = store.conflict_targets(Some(0)).await.unwrap().unwrap();
    assert!(targets.iter().any(|target| {
        target.field == "state"
            && target.recurrence_series_id.as_ref() == Some(&recurrence.series_id)
            && target.display_ref == recurrence.series_ref
    }));
}
