use super::*;

pub(super) async fn add_recurring_series(
    app: &mut App,
    title: &str,
) -> (
    crate::ids::TaskId,
    aven_core::recurrence::RecurrenceSeriesId,
) {
    add_recurring_series_with_schedule(app, title, recurrence_test_schedule()).await
}

fn recurrence_test_schedule() -> RecurrenceSchedule {
    RecurrenceSchedule::new(
        RecurrenceRule::daily(),
        "UTC".parse::<TimeZoneId>().unwrap(),
        chrono::Utc::now().date_naive(),
        None,
        RecurrenceDuePolicy::SameDay,
    )
}

fn recurrence_test_draft(title: &str) -> aven_core::operations::RecurrenceSeriesDraft {
    crate::tui::store::recurrence_draft(
        title.to_string(),
        "Series detail".to_string(),
        None,
        "medium".to_string(),
        "todo".to_string(),
        Vec::new(),
        recurrence_test_schedule(),
    )
}

async fn add_stable_journey_series(
    app: &mut App,
    title: &str,
) -> (
    crate::ids::TaskId,
    aven_core::recurrence::RecurrenceSeriesId,
) {
    use chrono::Datelike;

    let today = chrono::Utc::now().date_naive();
    let schedule = RecurrenceSchedule::new(
        RecurrenceRule::weekly(today.weekday()),
        "UTC".parse::<TimeZoneId>().unwrap(),
        today,
        None,
        RecurrenceDuePolicy::SameDay,
    );
    add_recurring_series_with_schedule(app, title, schedule).await
}

async fn add_recurring_series_with_schedule(
    app: &mut App,
    title: &str,
    schedule: RecurrenceSchedule,
) -> (
    crate::ids::TaskId,
    aven_core::recurrence::RecurrenceSeriesId,
) {
    let (_, selected) = app
        .store
        .create_recurrence_series(
            crate::tui::store::recurrence_draft(
                title.to_string(),
                "Series detail".to_string(),
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
    let item = &app.store.tasks[selected.unwrap()];
    (
        item.task.id.clone(),
        item.recurrence.as_ref().unwrap().series_id.clone(),
    )
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct PersistedOccurrence {
    task_id: String,
    slot_on: String,
    task_status: Option<String>,
    outcome: String,
    projection_state: String,
    resolved_at: String,
    archived_at: String,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct PersistedPauseInterval {
    paused_at: String,
    resumed_at: String,
    suspended_slot_on: String,
    suspended_task_id: String,
}

#[derive(Debug, PartialEq, Eq)]
struct PersistedRecurrence {
    series_state: String,
    stopped_at: String,
    occurrences: Vec<PersistedOccurrence>,
    pause_intervals: Vec<PersistedPauseInterval>,
}

async fn persisted_recurrence(
    pool: &SqlitePool,
    workspace_id: &crate::ids::WorkspaceId,
    series_id: &aven_core::recurrence::RecurrenceSeriesId,
) -> PersistedRecurrence {
    let (series_state, stopped_at) = sqlx::query_as::<_, (String, String)>(
        "SELECT state, stopped_at FROM recurrence_series
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let occurrences = sqlx::query_as::<_, PersistedOccurrence>(
        "SELECT o.task_id, o.slot_on, t.status AS task_status, o.outcome,
                o.projection_state, o.resolved_at, o.archived_at
         FROM recurrence_occurrences o
         LEFT JOIN tasks t
           ON t.workspace_id = o.workspace_id AND t.id = o.task_id
         WHERE o.workspace_id = ? AND o.series_id = ? ORDER BY o.slot_on",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(pool)
    .await
    .unwrap();
    let pause_intervals = sqlx::query_as::<_, PersistedPauseInterval>(
        "SELECT paused_at, resumed_at, suspended_slot_on, suspended_task_id
         FROM recurrence_pause_intervals
         WHERE workspace_id = ? AND series_id = ? ORDER BY paused_at",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(pool)
    .await
    .unwrap();
    PersistedRecurrence {
        series_state,
        stopped_at,
        occurrences,
        pause_intervals,
    }
}

async fn dispatch_keys(app: &mut App, size: ratatui::layout::Size, codes: &[KeyCode]) {
    for code in codes {
        app.dispatch_key(key(*code), size).await.unwrap();
    }
}

async fn recurrence_history_test_app() -> (App, aven_core::db::Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history-test.db");
    let pool = crate::test_support::open_db(&path).await.unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&path).await.unwrap();
    let mut app = App::new_for_tests(database.clone()).await.unwrap();
    app._test_database_dir = Some(dir);
    (app, database)
}

async fn add_recurrence_history_fixture(
    app: &mut App,
    database: &aven_core::db::Database,
) -> (
    crate::ids::TaskId,
    aven_core::recurrence::RecurrenceSeriesId,
) {
    let at = chrono::Utc::now();
    let created_at = at - chrono::Duration::days(15);
    let schedule = RecurrenceSchedule::new(
        RecurrenceRule::daily(),
        "UTC".parse::<TimeZoneId>().unwrap(),
        created_at.date_naive(),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    app.store
        .create_project("History".to_string())
        .await
        .unwrap();
    let created = database
        .create_recurrence_series(
            &app.store.active_workspace,
            aven_core::operations::CreateRecurrenceSeriesParams::new(
                crate::tui::store::recurrence_draft(
                    "History fixture".to_string(),
                    "History detail".to_string(),
                    Some(app.store.projects[0].key.clone()),
                    "medium".to_string(),
                    "todo".to_string(),
                    Vec::new(),
                    schedule,
                ),
            )
            .at(created_at),
        )
        .await
        .unwrap();
    database
        .reconcile_recurrence_series(&app.store.active_workspace, &created.series.id, at)
        .await
        .unwrap();
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    (created.task.id, created.series.id)
}

async fn select_archived_occurrence_in_history(
    app: &mut App,
    archived_task_id: &aven_core::ids::TaskId,
) {
    app.execute_selected_recurrence_action(Action::ShowRecurrenceHistory)
        .await
        .unwrap();
    app.handle_overlay_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .await
        .unwrap();
    let Some(OverlayState::RecurrenceHistory(mut history)) = app.overlay.take() else {
        panic!("expected recurrence history");
    };
    history.selected = Some(
        history
            .page
            .items
            .iter()
            .position(|entry| entry.task_id.as_ref() == Some(archived_task_id))
            .expect("archived occurrence is present"),
    );
    app.overlay = Some(OverlayState::RecurrenceHistory(history));
}

#[tokio::test]
async fn recurrence_history_pagination_replaces_the_resident_page() {
    let (mut app, database) = recurrence_history_test_app().await;
    let (_, series_id) = add_recurrence_history_fixture(&mut app, &database).await;

    app.execute_selected_recurrence_action(Action::ShowRecurrenceHistory)
        .await
        .unwrap();
    let Some(OverlayState::RecurrenceHistory(first)) = app.overlay.as_ref() else {
        panic!("expected recurrence history");
    };
    assert_eq!(first.series_id, series_id);
    assert_eq!(first.page.offset, 0);
    assert_eq!(
        first.page.items.len(),
        crate::tui::overlay::RECURRENCE_HISTORY_PAGE_SIZE
    );
    assert!(first.page.has_more);
    let as_of = first.as_of;

    app.handle_overlay_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .await
        .unwrap();
    let Some(OverlayState::RecurrenceHistory(second)) = app.overlay.as_ref() else {
        panic!("expected second history page");
    };
    assert_eq!(second.page.offset, second.page.limit);
    assert_eq!(second.as_of, as_of);
    assert!(second.page.items.len() <= second.page.limit);

    app.handle_overlay_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .await
        .unwrap();
    let Some(OverlayState::RecurrenceHistory(first_again)) = app.overlay.as_ref() else {
        panic!("expected first history page");
    };
    assert_eq!(first_again.page.offset, 0);
    assert_eq!(first_again.as_of, as_of);
}

#[tokio::test]
async fn recurrence_history_enter_opens_the_linked_archived_task() {
    let (mut app, database) = recurrence_history_test_app().await;
    let (archived_task_id, _) = add_recurrence_history_fixture(&mut app, &database).await;
    select_archived_occurrence_in_history(&mut app, &archived_task_id).await;

    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();

    assert_eq!(app.store.view_state.view, TaskView::Search);
    assert_eq!(app.store.tasks[0].task.id, archived_task_id);
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn recurring_series_detail_opens_hidden_occurrence_and_returns() {
    let mut app = test_app().await;
    let (task_id, series_id) = add_recurring_series(&mut app, "Daily review").await;
    app.store
        .pause_recurrence(&series_id, Some(&task_id))
        .await
        .unwrap();
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());

    let terminal = ratatui::layout::Size::new(120, 40);
    app.dispatch_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), terminal)
        .await
        .unwrap();
    assert_eq!(
        app.store.recurrence_detail.as_ref().unwrap().series.id,
        series_id
    );
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());

    app.dispatch_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), terminal)
        .await
        .unwrap();
    assert_eq!(app.store.view_state.view, TaskView::Search);
    assert_eq!(app.store.tasks[0].task.id, task_id);
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());

    app.go_back().await.unwrap();
    assert_eq!(app.store.view_state.view, TaskView::Recurring);
    assert_eq!(
        app.store
            .selected_recurrence_series(app.list.selected_task())
            .unwrap()
            .series
            .id,
        series_id
    );
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn recurring_series_rows_route_task_commands_to_series_feedback() {
    let mut app = test_app().await;
    let (_, series_id) = add_recurring_series(&mut app, "Series command routing").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());

    for (keys, expected) in [
        (
            vec![KeyCode::Char('d')],
            "recurring rows select a series. press enter to open its occurrence before changing status.",
        ),
        (
            vec![KeyCode::Char('x')],
            "recurring rows select a series. press enter to open its occurrence before changing status.",
        ),
        (
            vec![KeyCode::Char('s')],
            "recurring rows select a series. press enter to open its occurrence before changing status.",
        ),
        (
            vec![KeyCode::Char(' ')],
            "recurring rows select series and cannot be marked. press enter to open an occurrence.",
        ),
    ] {
        app.notification = None;
        for code in keys {
            app.handle_normal_key(code).await.unwrap();
        }
        assert_eq!(toast_message(&app).as_deref(), Some(expected));
        assert!(app.overlay.is_none());
    }

    assert_eq!(
        app.store
            .recurrence_detail_for_series(&series_id)
            .await
            .unwrap()
            .series
            .state,
        aven_core::recurrence::RecurrenceSeriesState::Active
    );
}

#[tokio::test]
async fn recurring_series_rows_copy_series_ref_and_edit_template() {
    let mut app = test_app().await;
    add_recurring_series(&mut app, "Series copy and edit").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    let series_ref = app
        .store
        .selected_recurrence_series(app.list.selected_task())
        .unwrap()
        .series_ref
        .clone();

    app.handle_normal_key(KeyCode::Char('y')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
    assert_eq!(
        crate::tui::platform::clipboard_text_for_test(),
        Some(series_ref.clone())
    );
    assert_eq!(toast_message(&app), Some(format!("copied {series_ref}")));

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
        panic!("template edit did not open the recurrence composer");
    };
    assert!(state.template_schedule.is_some());
}

#[tokio::test]
async fn recurring_series_delete_guides_to_confirmed_stop() {
    let mut app = test_app().await;
    add_recurring_series(&mut app, "Series lifecycle routing").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(
            "recurring rows select a series. use t r s to stop future occurrences with confirmation."
        )
    );
    assert!(app.overlay.is_none());

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
    let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
        panic!("stop command did not open its confirmation picker");
    };
    assert!(matches!(picker.intent, PickerIntent::StopRecurrence { .. }));
}

#[tokio::test]
async fn recurring_series_list_and_detail_use_compact_natural_language() {
    let mut app = test_app().await;
    add_recurring_series(&mut app, "Natural recurring detail").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());

    let list = render_app_text(&mut app, 120, 30);
    assert!(list.contains("Every day"));
    assert!(!list.contains("Etc/UTC"));
    assert!(!list.contains("daily ·"));

    app.dispatch_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        (120, 30).into(),
    )
    .await
    .unwrap();
    let detail = render_app_text(&mut app, 120, 30);
    for expected in [
        "Every day",
        "Available    start of day",
        "Due          same day",
        "Enter open task",
        "t r p pause",
        "t r h history",
        "t r s stop",
        "Esc close",
    ] {
        assert!(detail.contains(expected), "missing detail text: {expected}");
    }
    assert!(!detail.contains("Time zone"));
    assert!(!detail.contains("fixed"));
    assert!(
        detail
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
            < 24
    );
}

#[tokio::test]
async fn recurrence_actions_do_not_create_projects_for_deleted_template_project() {
    let (mut app, database) = recurrence_history_test_app().await;
    let (_, series_id) = add_recurrence_history_fixture(&mut app, &database).await;
    let project_key = app.store.projects[0].key.clone();
    app.store.delete_project(&project_key).await.unwrap();
    assert!(
        database
            .list_projects(&app.store.active_workspace.id, None)
            .await
            .unwrap()
            .is_empty()
    );
    let target = crate::tui::app_recurrence::RecurrenceTargetId {
        workspace_id: app.store.active_workspace.id.clone(),
        series_id: series_id.clone(),
    };

    app.run_recurrence_action(
        target.clone(),
        crate::tui::app_recurrence::RecurrenceActionKind::History,
    )
    .await
    .unwrap();
    app.overlay = None;
    app.run_recurrence_action(
        target.clone(),
        crate::tui::app_recurrence::RecurrenceActionKind::Pause,
    )
    .await
    .unwrap();
    app.run_recurrence_action(
        target,
        crate::tui::app_recurrence::RecurrenceActionKind::EditTemplate,
    )
    .await
    .unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("recurring template project is unavailable")
    );
    assert!(
        database
            .list_projects(&app.store.active_workspace.id, None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn recurrence_template_update_restores_series_identity_after_reordering() {
    let mut app = test_app().await;
    add_recurring_series(&mut app, "Alpha series").await;
    let (_, edited_series_id) = add_recurring_series(&mut app, "Zulu series").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    let edited_index = app
        .store
        .recurrence_series
        .iter()
        .position(|item| item.series.id == edited_series_id)
        .unwrap();
    app.list.select_task(Some(edited_index));
    app.store
        .load_recurrence_series_detail(&edited_series_id)
        .await
        .unwrap();

    let message = app
        .store
        .update_recurrence_template(
            &edited_series_id,
            aven_core::operations::RecurrenceTemplateUpdate {
                title: Some("Aardvark series".to_string()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    app.list.select_task(message.selected);

    assert_eq!(
        app.store
            .selected_recurrence_series(app.list.selected_task())
            .unwrap()
            .series
            .id,
        edited_series_id
    );
    assert_eq!(
        app.store.recurrence_detail.as_ref().unwrap().series.title,
        "Aardvark series"
    );
}

#[tokio::test]
async fn recurrence_creation_in_recurring_view_selects_created_series() {
    let mut app = test_app().await;
    add_recurring_series(&mut app, "Anchor series").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());

    let (message, selected) = app
        .store
        .create_recurrence_series(
            recurrence_test_draft("Created series"),
            app.list.selected_task(),
        )
        .await
        .unwrap();
    app.list.select_task(selected);

    let selected = app
        .store
        .selected_recurrence_series(app.list.selected_task())
        .unwrap();
    assert_eq!(selected.series.title, "Created series");
    assert!(!message.contains("hidden by current filters"));
}

#[tokio::test]
async fn recurrence_creation_in_recurring_view_reports_filtered_series_and_preserves_selection() {
    let mut app = test_app().await;
    add_recurring_series(&mut app, "Keep Alpha").await;
    let (_, selected_series_id) = add_recurring_series(&mut app, "Keep Zulu").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    app.store.view_state.recurring.search = Some("Keep".to_string());
    let selected = app.store.refresh(None).await.unwrap();
    app.list.select_task(selected);
    let selected_index = app
        .store
        .recurrence_series
        .iter()
        .position(|item| item.series.id == selected_series_id)
        .unwrap();
    app.list.select_task(Some(selected_index));

    let (message, selected) = app
        .store
        .create_recurrence_series(
            recurrence_test_draft("Filtered series"),
            app.list.selected_task(),
        )
        .await
        .unwrap();
    app.list.select_task(selected);

    assert!(message.starts_with("Created recurring task RCR-"));
    assert!(!message.contains("hidden"));
    assert_eq!(
        app.store
            .selected_recurrence_series(app.list.selected_task())
            .unwrap()
            .series
            .id,
        selected_series_id
    );
    assert!(
        app.store
            .recurrence_series
            .iter()
            .all(|item| item.series.title != "Filtered series")
    );
}

#[tokio::test]
async fn recurrence_palette_skip_uses_live_projection_after_historical_navigation() {
    let (mut app, database) = recurrence_history_test_app().await;
    let (archived_task_id, series_id) = add_recurrence_history_fixture(&mut app, &database).await;
    select_archived_occurrence_in_history(&mut app, &archived_task_id).await;
    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();

    let live_task_id = database
        .recurrence_series_detail(&app.store.active_workspace.id, &series_id)
        .await
        .unwrap()
        .current_occurrence
        .unwrap()
        .task_id
        .unwrap();
    app.detail.close();
    app.overlay = None;
    app.dispatch_key(
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        ratatui::layout::Size::new(120, 24),
    )
    .await
    .unwrap();
    let Some(OverlayState::Command { state }) = app.overlay.take() else {
        panic!("expected command palette");
    };
    assert!(
        state
            .unavailable
            .iter()
            .all(|override_| override_.action != Action::SkipRecurrence)
    );
    app.execute_targeted_recurrence_action(state.target, Action::SkipRecurrence)
        .await
        .unwrap();

    let live_task = database
        .resolve_task_ref(&app.store.active_workspace, live_task_id.as_ref())
        .await
        .unwrap();
    assert_eq!(live_task.status.as_str(), "canceled");
    assert_eq!(app.store.tasks[0].task.id, archived_task_id);
}

#[tokio::test]
async fn recurrence_palette_disables_skip_for_historical_task_without_live_projection() {
    let (mut app, database) = recurrence_history_test_app().await;
    let (archived_task_id, series_id) = add_recurrence_history_fixture(&mut app, &database).await;
    select_archived_occurrence_in_history(&mut app, &archived_task_id).await;
    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();

    database
        .stop_recurrence_series(&app.store.active_workspace, &series_id, true)
        .await
        .unwrap();
    assert!(
        database
            .recurrence_series_detail(&app.store.active_workspace.id, &series_id)
            .await
            .unwrap()
            .current_occurrence
            .is_none()
    );

    app.detail.close();
    app.overlay = None;
    app.dispatch_key(
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        ratatui::layout::Size::new(120, 24),
    )
    .await
    .unwrap();
    let Some(OverlayState::Command { state }) = app.overlay.as_ref() else {
        panic!("expected command palette");
    };
    assert!(matches!(
        &state.target,
        Some(OverlayTarget::RecurrenceSeries {
            series_id: target_series_id,
            ..
        }) if target_series_id == &series_id
    ));
    assert!(state.unavailable.iter().any(|override_| {
        override_.action == Action::SkipRecurrence
            && override_.reason == "series has no current occurrence to skip"
    }));
}

#[tokio::test]
async fn recurrence_detail_restoration_accepts_matching_series_selection() {
    let mut app = test_app().await;
    let (_, series_id) = add_recurring_series(&mut app, "Detail restore").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    app.store
        .load_recurrence_series_detail(&series_id)
        .await
        .unwrap();
    app.detail.close();
    app.overlay = None;

    app.restore_detail_overlay_at_scroll(true, 7);

    assert_eq!(app.detail.state().map(|detail| detail.scroll()), Some(7));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn recurrence_series_detail_restores_after_lifecycle_history_and_stop_round_trips() {
    let mut app = test_app().await;
    let (_, series_id) = add_recurring_series(&mut app, "Detail round trips").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    let terminal = ratatui::layout::Size::new(120, 40);
    app.dispatch_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), terminal)
        .await
        .unwrap();

    dispatch_keys(
        &mut app,
        terminal,
        &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('p')],
    )
    .await;
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
    assert_eq!(
        app.store.recurrence_detail.as_ref().unwrap().series.state,
        aven_core::recurrence::RecurrenceSeriesState::Paused
    );

    dispatch_keys(
        &mut app,
        terminal,
        &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('h')],
    )
    .await;
    assert!(matches!(
        app.overlay,
        Some(OverlayState::RecurrenceHistory(_))
    ));
    app.dispatch_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), terminal)
        .await
        .unwrap();
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());

    dispatch_keys(
        &mut app,
        terminal,
        &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('s')],
    )
    .await;
    assert!(matches!(app.overlay, Some(OverlayState::Picker(_))));
    app.dispatch_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), terminal)
        .await
        .unwrap();
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
    assert_eq!(
        app.store.recurrence_detail.as_ref().unwrap().series.id,
        series_id
    );
}

#[tokio::test]
async fn recurrence_series_detail_keeps_status_and_stop_shortcuts_distinct() {
    let mut app = test_app().await;
    let (_, series_id) = add_recurring_series(&mut app, "Shortcut distinction").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    let terminal = ratatui::layout::Size::new(120, 40);
    app.dispatch_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), terminal)
        .await
        .unwrap();
    let rendered = render_app_text(&mut app, terminal.width, terminal.height);
    assert!(rendered.contains("t r p") && rendered.contains("pause"));
    assert!(rendered.contains("t r s") && rendered.contains("stop"));

    app.dispatch_key(
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        terminal,
    )
    .await
    .unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("Status applies to occurrence tasks. Press Enter to open the current occurrence")
    );
    assert_eq!(
        app.store.recurrence_detail.as_ref().unwrap().series.state,
        aven_core::recurrence::RecurrenceSeriesState::Active
    );

    dispatch_keys(
        &mut app,
        terminal,
        &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('s')],
    )
    .await;

    let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
        panic!("explicit stop shortcut did not open confirmation");
    };
    assert!(matches!(picker.intent, PickerIntent::StopRecurrence { .. }));
    assert_eq!(
        app.store.recurrence_detail.as_ref().unwrap().series.id,
        series_id
    );
}

#[tokio::test]
async fn recurrence_series_detail_return_survives_intermediate_back_navigation() {
    let mut app = test_app().await;
    let (_, series_id) = add_recurring_series(&mut app, "Multi-hop return").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    app.activate_or_toggle_detail().await.unwrap();
    app.detail = crate::tui::detail_session::DetailSession::open(5);
    app.open_recurrence_occurrence().await.unwrap();
    app.show_view(TaskView::Open).await.unwrap();

    app.go_back().await.unwrap();
    assert_eq!(app.store.view_state.view, TaskView::Search);
    assert!(app.series_detail_return.is_some());

    app.go_back().await.unwrap();
    assert_eq!(app.store.view_state.view, TaskView::Recurring);
    assert_eq!(
        app.store
            .selected_recurrence_series(app.list.selected_task())
            .unwrap()
            .series
            .id,
        series_id
    );
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
    assert_eq!(app.detail.state().map(|detail| detail.scroll()), Some(5));
    assert!(app.series_detail_return.is_none());
}

#[tokio::test]
async fn recurring_series_lifecycle_filter_reveals_stopped_series() {
    let mut app = test_app().await;
    let (task_id, series_id) = add_recurring_series(&mut app, "Finite review").await;
    app.store
        .stop_recurrence(&series_id, Some(&task_id), false)
        .await
        .unwrap();
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    assert!(app.store.recurrence_series.is_empty());

    app.store.view_state.recurring.lifecycle = RecurrenceSeriesLifecycleFilter::Stopped;
    app.refresh().await.unwrap();
    assert_eq!(app.store.recurrence_series.len(), 1);
    assert_eq!(app.store.recurrence_series[0].series.id, series_id);
}

#[tokio::test]
async fn recurrence_pause_resume_journey_preserves_selection_and_occurrence() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (_task_id, series_id) = add_stable_journey_series(&mut app, "Pause resume journey").await;
    let size: ratatui::layout::Size = (140, 24).into();
    dispatch_keys(&mut app, size, &[KeyCode::Char('v'), KeyCode::Char('u')]).await;
    assert_eq!(app.store.view_state.view, TaskView::Recurring);
    let workspace_id = app.store.active_workspace.id.clone();
    let before = persisted_recurrence(&pool, &workspace_id, &series_id).await;
    assert_eq!(before.series_state, "active");
    assert_eq!(before.occurrences.len(), 1);
    let rendered = render_app_text(&mut app, size.width, size.height);
    assert!(
        rendered.contains("Recurring Tasks")
            && rendered.contains("Pause resume journey")
            && rendered.contains("active"),
        "pause/resume journey: expected the active series in Recurring Tasks"
    );

    dispatch_keys(
        &mut app,
        size,
        &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('p')],
    )
    .await;

    let paused = persisted_recurrence(&pool, &workspace_id, &series_id).await;
    assert_eq!(
        paused.series_state, "paused",
        "pause/resume journey: keyboard Pause did not persist"
    );
    assert_eq!(
        paused.occurrences, before.occurrences,
        "pause/resume journey: Pause changed the live occurrence"
    );
    assert_eq!(paused.pause_intervals.len(), 1);
    assert!(paused.pause_intervals[0].resumed_at.is_empty());
    assert_eq!(
        paused.pause_intervals[0].suspended_task_id,
        before.occurrences[0].task_id
    );
    assert_eq!(
        app.store
            .selected_recurrence_series(app.list.selected_task())
            .unwrap()
            .series
            .id,
        series_id,
        "pause/resume journey: Pause lost the selected series"
    );
    let rendered = render_app_text(&mut app, size.width, size.height);
    assert!(
        rendered.contains("Pause resume journey") && rendered.contains("paused"),
        "pause/resume journey: paused series disappeared from Recurring Tasks"
    );

    let click = recurrence_right_click_event(&app, (size.width, size.height), 0);
    app.dispatch_mouse(click, size).await.unwrap();
    let resume_row = match app.overlay.as_ref() {
        Some(OverlayState::Picker(picker)) => {
            assert!(matches!(
                picker.intent,
                PickerIntent::RecurrenceActions { .. }
            ));
            picker
                .items
                .iter()
                .position(|item| item.value == "resume")
                .expect("pause/resume journey: paused series has no Resume action")
                as u16
        }
        _ => panic!("pause/resume journey: right-click did not open recurrence actions"),
    };
    let click = picker_row_click(&app, resume_row, size);
    app.dispatch_mouse(click, size).await.unwrap();

    let resumed = persisted_recurrence(&pool, &workspace_id, &series_id).await;
    assert_eq!(
        resumed.series_state, "active",
        "pause/resume journey: mouse Resume did not persist"
    );
    assert_eq!(
        resumed.occurrences, before.occurrences,
        "pause/resume journey: Resume changed the live occurrence"
    );
    assert_eq!(resumed.pause_intervals.len(), 1);
    assert!(!resumed.pause_intervals[0].resumed_at.is_empty());
    assert_eq!(
        app.store
            .selected_recurrence_series(app.list.selected_task())
            .unwrap()
            .series
            .id,
        series_id,
        "pause/resume journey: Resume lost the selected series"
    );
    assert!(
        app.overlay.is_none(),
        "pause/resume journey: Resume left the context menu open"
    );
    let rendered = render_app_text(&mut app, size.width, size.height);
    assert!(
        rendered.contains("Pause resume journey") && rendered.contains("active"),
        "pause/resume journey: resumed series did not render as active"
    );
}

#[tokio::test]
async fn recurrence_stop_journey_persists_keep_skip_and_cancel() {
    for choice in ["cancel", "keep", "skip"] {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let (_task_id, series_id) = add_stable_journey_series(&mut app, "Stop journey").await;
        let size: ratatui::layout::Size = (140, 24).into();
        dispatch_keys(&mut app, size, &[KeyCode::Char('v'), KeyCode::Char('u')]).await;
        assert_eq!(app.store.view_state.view, TaskView::Recurring);
        let workspace_id = app.store.active_workspace.id.clone();
        let before = persisted_recurrence(&pool, &workspace_id, &series_id).await;

        dispatch_keys(
            &mut app,
            size,
            &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('s')],
        )
        .await;
        let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
            panic!("stop journey: keyboard Stop did not open a picker for {choice}");
        };
        assert!(matches!(picker.intent, PickerIntent::StopRecurrence { .. }));
        assert_eq!(
            picker
                .items
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            vec!["keep", "skip"]
        );
        assert_eq!(
            picker.selected, 0,
            "stop journey: Keep was not the safe default for {choice}"
        );
        assert_eq!(
            persisted_recurrence(&pool, &workspace_id, &series_id).await,
            before,
            "stop journey: opening the picker mutated persistence for {choice}"
        );

        match choice {
            "cancel" => app.dispatch_key(key(KeyCode::Esc), size).await.unwrap(),
            "keep" => app.dispatch_key(key(KeyCode::Enter), size).await.unwrap(),
            "skip" => {
                app.dispatch_key(key(KeyCode::Down), size).await.unwrap();
                app.dispatch_key(key(KeyCode::Enter), size).await.unwrap();
            }
            _ => unreachable!(),
        }

        let after = persisted_recurrence(&pool, &workspace_id, &series_id).await;
        if choice == "cancel" {
            assert_eq!(
                after, before,
                "stop journey: Escape mutated recurrence persistence"
            );
            assert!(
                app.overlay.is_none(),
                "stop journey: Escape left the picker open"
            );
            continue;
        }

        assert_eq!(
            after.series_state, "stopped",
            "stop journey: {choice} did not stop the series"
        );
        assert_eq!(
            after.occurrences.len(),
            1,
            "stop journey: {choice} created a successor"
        );
        let occurrence = &after.occurrences[0];
        assert_eq!(occurrence.task_id, before.occurrences[0].task_id);
        assert_eq!(occurrence.slot_on, before.occurrences[0].slot_on);
        assert_eq!(
            occurrence.task_status.as_deref(),
            Some(if choice == "skip" { "canceled" } else { "todo" })
        );
        assert_eq!(
            occurrence.outcome,
            if choice == "skip" { "skipped" } else { "" }
        );
        assert_eq!(
            occurrence.projection_state,
            if choice == "skip" {
                "resolved"
            } else {
                "projected"
            }
        );
        assert_eq!(
            app.store.view_state.recurring.lifecycle,
            RecurrenceSeriesLifecycleFilter::All,
            "stop journey: {choice} did not reveal the stopped series"
        );
        assert_eq!(
            app.store
                .selected_recurrence_series(app.list.selected_task())
                .unwrap()
                .series
                .id,
            series_id,
            "stop journey: {choice} lost the selected series"
        );
        assert!(
            app.overlay.is_none(),
            "stop journey: {choice} left the picker open"
        );
        assert!(
            toast_message(&app).is_some_and(|message| message.contains(if choice == "skip" {
                "skipped current occurrence"
            } else {
                "kept current occurrence"
            })),
            "stop journey: {choice} produced the wrong success message"
        );
    }
}

#[tokio::test]
async fn recurrence_stop_rejects_invalid_picker_value() {
    let mut app = test_app().await;
    let (_task_id, series_id) = add_recurring_series(&mut app, "Invalid stop").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    let target = Some(OverlayTarget::RecurrenceSeries {
        workspace_id: app.store.active_workspace.id.clone(),
        series_id: series_id.clone(),
    });

    app.submit_stop_recurrence(target, Some("unknown"))
        .await
        .unwrap();

    assert_eq!(toast_message(&app).as_deref(), Some("invalid stop outcome"));
    assert_eq!(
        app.store
            .recurrence_detail_for_series(&series_id)
            .await
            .unwrap()
            .series
            .state,
        aven_core::recurrence::RecurrenceSeriesState::Active
    );
}

#[tokio::test]
async fn recurrence_overlay_retains_series_identity_across_selection_change() {
    let mut app = test_app().await;
    let (_, first_id) = add_recurring_series(&mut app, "Alpha series").await;
    let (_, second_id) = add_recurring_series(&mut app, "Beta series").await;
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    let first_index = app
        .store
        .recurrence_series
        .iter()
        .position(|item| item.series.id == first_id)
        .unwrap();
    let second_index = app
        .store
        .recurrence_series
        .iter()
        .position(|item| item.series.id == second_id)
        .unwrap();
    app.list.select_task(Some(first_index));
    app.execute_selected_recurrence_action(Action::StopRecurrence)
        .await
        .unwrap();
    app.list.select_task(Some(second_index));

    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();

    assert_eq!(
        app.store
            .recurrence_detail_for_series(&first_id)
            .await
            .unwrap()
            .series
            .state,
        aven_core::recurrence::RecurrenceSeriesState::Stopped
    );
    assert_eq!(
        app.store
            .recurrence_detail_for_series(&second_id)
            .await
            .unwrap()
            .series
            .state,
        aven_core::recurrence::RecurrenceSeriesState::Active
    );
}

#[tokio::test]
async fn recurrence_palette_disables_invalid_lifecycle_action_with_reason() {
    let mut app = test_app().await;
    let (task_id, series_id) = add_recurring_series(&mut app, "Paused command").await;
    app.store
        .pause_recurrence(&series_id, Some(&task_id))
        .await
        .unwrap();
    app.list
        .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
    app.begin_command().await;
    let Some(OverlayState::Command { state }) = app.overlay.as_mut() else {
        panic!("expected command palette");
    };
    state.input = LineEdit::new("recurrence-pause".to_string());

    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();

    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("series is already paused")
    );

    app.notification = None;
    app.execute_selected_recurrence_action(Action::PauseRecurrence)
        .await
        .unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("series is already paused")
    );
}
