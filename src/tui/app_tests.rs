use super::*;
use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::operations::TaskDraft;
use crate::query::RecurrenceSeriesLifecycleFilter;
use crate::tui::app_conflicts::CONFLICT_CONFIRM_LOCAL_TITLE;
use crate::tui::app_edit::{
    EDIT_AVAILABILITY_TITLE, EDIT_DESCRIPTION_TITLE, EDIT_DUE_TITLE, EDIT_LABELS_TITLE,
    EDIT_PROJECT_TITLE, EDIT_TITLE_TITLE,
};
use crate::tui::app_filters::{SCOPE_PROJECT_TITLE, SWITCH_WORKSPACE_TITLE};
use crate::tui::app_intake::{IntakeWorkView, NaturalRetry};
use crate::tui::app_projects::{DELETE_PROJECT_TITLE, DELETE_TASK_TITLE};
use crate::tui::app_search::SearchControllerView;
use crate::tui::authoring::{ADD_NOTE_TITLE, AddTaskOrigin, AddTaskStep};
use crate::tui::config_overlay::{CONFIG_INFO_TITLE, CONFIG_INIT_TITLE, CONFIG_PATHS_TITLE};
use crate::tui::detail_session::DetailSnapshot;
use crate::tui::event::Action;
use crate::tui::overlay::{
    CommandState, ConfirmIntent, ConfirmState, LineEdit, MultilineInputMode, MultilineInputState,
    MultilineIntent, OverlayState, OverlayTarget, OverlayView, PickerIntent, PickerItem,
    PickerMode, PickerState, SearchIntent, SearchState, TagComboboxIntent, TextInputState,
    TextIntent, TextPanelState,
};
use crate::tui::store::{
    SidebarEntryTarget, TaskOrder, TaskScope, TaskScopeTarget, TaskView, TaskViewState,
};
use crate::tui::toast::ToastSeverity;
use crate::tui::ui::ViewSurface;
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule, TimeZoneId};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use sqlx::SqlitePool;

fn toast_message(app: &App) -> Option<String> {
    app.notification
        .as_ref()
        .map(|notification| notification.toast_view().message)
}

fn toast_severity(app: &App) -> Option<ToastSeverity> {
    app.notification
        .as_ref()
        .map(|notification| notification.toast_view().severity)
}

fn detail_scroll(app: &App) -> u16 {
    app.detail.state().map_or(0, |detail| detail.scroll())
}

async fn test_app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&dir.path().join("test.db"))
        .await
        .unwrap();
    App::new_for_tests(database).await.unwrap()
}

async fn add_recurring_series(
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
    let app = App::new_for_tests(database.clone()).await.unwrap();
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
        .create_recurrence_series_at(
            &app.store.active_workspace,
            crate::tui::store::recurrence_draft(
                "History fixture".to_string(),
                "History detail".to_string(),
                Some(app.store.projects[0].key.clone()),
                "medium".to_string(),
                "todo".to_string(),
                Vec::new(),
                schedule,
            ),
            created_at,
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
        "e edit",
        "p pause",
        "h history",
        "s stop",
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

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

async fn add_real_attachment(
    app: &mut App,
    _pool: &SqlitePool,
    db_path: &std::path::Path,
    selected: usize,
) -> String {
    app.set_add_task_db_path(db_path.to_path_buf());
    let blob_dir = crate::config::resolve_blob_dir(db_path, app.intake.config()).unwrap();
    let task_id = app.store.tasks[selected].task.id.clone();
    let database = aven_core::db::Database::open(db_path).await.unwrap();
    let outcome = database
        .add_task_attachment(
            &app.store.active_workspace,
            &blob_dir,
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            &task_id,
            crate::operations::AttachmentAddInput {
                filename: Some("viewer-test.png".to_string()),
                alt_text: None,
                declared_media_type: Some("image/png".to_string()),
                bytes: png_bytes(2, 2),
                optimization_policy: crate::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: true,
            },
        )
        .await
        .unwrap();
    app.store.refresh(Some(&task_id)).await.unwrap();
    app.list.select_task(Some(selected));
    outcome.outcome.attachment.attachment_id
}

fn test_task_intake_result(title: &str) -> crate::task_intake::TaskIntakeResult {
    crate::task_intake::TaskIntakeResult {
        task: test_task_draft(title),
        recurrence: None,
    }
}

fn test_task_draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: None,
        status: "inbox".to_string(),
        priority: "none".to_string(),
        source: TaskSource::Unknown,
        labels: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

fn test_attachment(
    attachment_id: &str,
    media_type: &str,
    has_blob: bool,
    dimensions: Option<(i64, i64)>,
) -> crate::task_render::AttachmentMetadataJson {
    crate::task_render::AttachmentMetadataJson {
        attachment_id: attachment_id.to_string(),
        task_id: "7KQ9A1X".to_string(),
        sha256: format!("{attachment_id:0<64}"),
        media_type: media_type.to_string(),
        byte_size: 4,
        filename: Some(format!("{attachment_id}.png")),
        alt_text: None,
        width: dimensions.map(|(width, _)| width),
        height: dimensions.map(|(_, height)| height),
        created_at: "2026-06-20T12:00:00Z".to_string(),
        deleted: false,
        deleted_at: None,
        bytes_state: if has_blob {
            crate::attachments::AttachmentBytesState::Present
        } else {
            crate::attachments::AttachmentBytesState::PendingDownload
        },
        has_blob,
    }
}

async fn create_and_select_task(app: &mut App, draft: TaskDraft) -> usize {
    let (_, selected) = app.store.create_task(draft, None).await.unwrap();
    let selected = selected.unwrap();
    app.list.select_task(Some(selected));
    selected
}

async fn test_app_with_pool() -> (tempfile::TempDir, SqlitePool, App) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&dir.path().join("test.db"))
        .await
        .unwrap();
    let app = App::new_for_tests(database).await.unwrap();
    (dir, pool, app)
}

async fn reset_default_workspace(pool: &SqlitePool) {
    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn detail_attachment_hit_id(
    item: &crate::query::TaskListItem,
    width: u16,
    height: u16,
    scroll: u16,
    context: &crate::tui::ui::DetailInlineImageContext,
) -> Option<String> {
    (0..height).find_map(|row| {
        (0..width).find_map(|column| {
            crate::tui::ui::detail_attachment_at_position(
                item, width, height, column, row, scroll, context,
            )
            .map(|hit| hit.attachment_id)
        })
    })
}

fn ctrl_s() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
}

fn ctrl_e() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)
}

fn ctrl_x() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn ctrl_a() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
}

fn ctrl_p() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
}

fn ctrl_r() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
}

fn ctrl_l() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)
}

fn ctrl_t() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)
}

fn ctrl_n() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)
}

fn ctrl_d() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)
}

fn ctrl_u() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)
}

fn header_click(column: u16) -> MouseEvent {
    click_at(column, 0)
}

fn click_at(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn mouse_wheel(kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }
}

fn task_row_click(column: u16, row: u16) -> MouseEvent {
    click_at(column, row)
}

fn left_click(column: u16, row: u16) -> MouseEvent {
    click_at(column, row)
}

fn left_drag(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn left_release(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn right_click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn task_list_area(size: (u16, u16)) -> ratatui::layout::Rect {
    let (width, height) = size;
    let body = ratatui::layout::Rect::new(0, 2, width, height.saturating_sub(4));
    if body.width < 100 {
        body
    } else {
        let sidebar_width = body.width.min(26);
        ratatui::layout::Rect::new(
            sidebar_width,
            body.y,
            body.width.saturating_sub(sidebar_width),
            body.height,
        )
    }
}

fn recurrence_right_click_event(app: &App, size: (u16, u16), series_index: usize) -> MouseEvent {
    let task_area = task_list_area(size);
    for row in task_area.y..task_area.y.saturating_add(task_area.height) {
        for column in task_area.x..task_area.x.saturating_add(task_area.width) {
            if crate::tui::ui::recurrence_series_at_position(
                &app.store,
                app.list.table_state(),
                task_area,
                column,
                row,
            )
            .is_some_and(|hit| hit.series_index == series_index)
            {
                return right_click(column, row);
            }
        }
    }
    panic!("expected recurrence series hit target");
}

fn wheel_down(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[track_caller]
fn picker_row_click(app: &App, visible_row: u16, size: ratatui::layout::Size) -> MouseEvent {
    let Some(overlay) = app.overlay.as_ref() else {
        panic!("expected overlay");
    };
    match OverlayView::from(overlay) {
        OverlayView::Picker(view) => {
            let layout = crate::tui::overlay::picker_layout(&view, size);
            left_click(
                layout.inner.x.saturating_add(2),
                layout
                    .inner
                    .y
                    .saturating_add(layout.list_start)
                    .saturating_add(visible_row),
            )
        }
        OverlayView::TagCombobox(view) => {
            let layout = crate::tui::overlay::tag_combobox_layout(&view, size);
            left_click(
                layout.inner.x.saturating_add(2),
                layout
                    .inner
                    .y
                    .saturating_add(layout.list_start)
                    .saturating_add(visible_row),
            )
        }
        _ => panic!("expected picker overlay"),
    }
}

fn detail_metadata_click(target: crate::tui::ui::DetailMetadataTarget) -> MouseEvent {
    let row = match target {
        crate::tui::ui::DetailMetadataTarget::Status => 8,
        crate::tui::ui::DetailMetadataTarget::Priority => 11,
    };
    left_click(88, row)
}

fn render_app_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let view = app.view();
    terminal
        .draw(|frame| {
            crate::tui::ui::render(frame, &app.store, &mut app.widgets, &mut app.list, &view)
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn render_app_text(app: &mut App, width: u16, height: u16) -> String {
    render_app_buffer(app, width, height)
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[tokio::test]
async fn sidebar_click_selects_project_scope_in_wide_layout() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.refresh().await.unwrap();

    let project_index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.target,
                Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(project)))
                    if project == "mobile-app"
            )
        })
        .expect("mobile-app sidebar entry");
    let terminal_size: ratatui::layout::Size = (140, 24).into();
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
        Focus::Tasks,
    )
    .expect("wide sidebar layout");
    assert!(project_index >= usize::from(layout.content.height));

    app.list.select_sidebar(Some(project_index));
    let _ = render_app_buffer(&mut app, terminal_size.width, terminal_size.height);
    let offset = app.list.sidebar_state().offset();
    assert!(offset > 0, "render must scroll the project into view");
    let visible_index = project_index
        .checked_sub(offset)
        .expect("sidebar offset must not exceed the selected project index");
    let visible_row = u16::try_from(visible_index).expect("visible row must fit in u16");
    assert!(visible_row < layout.content.height);
    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);

    app.dispatch_mouse(
        click_at(layout.content.x, layout.content.y + visible_row),
        terminal_size,
    )
    .await
    .unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert_eq!(app.list.selected_sidebar(), Some(project_index));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn sidebar_click_selects_saved_view_in_narrow_overlay() {
    let mut app = test_app().await;
    app.list.focus_sidebar();

    let view_row = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.target,
                Some(SidebarEntryTarget::View(TaskView::Open))
            )
        })
        .unwrap() as u16;
    let terminal_size: ratatui::layout::Size = (90, 24).into();
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
        Focus::Sidebar,
    )
    .unwrap();
    let row = layout.content.y + view_row;

    app.dispatch_mouse(click_at(layout.content.x, row), terminal_size)
        .await
        .unwrap();

    assert_eq!(app.store.view_state.view, TaskView::Open);
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert_eq!(app.list.selected_sidebar(), Some(view_row as usize));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn sidebar_click_uses_scroll_offset_in_wide_layout() {
    let mut app = test_app().await;
    for index in 0..25 {
        app.store
            .create_project(format!("Project {index}"))
            .await
            .unwrap();
    }
    app.refresh().await.unwrap();

    let project_index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.target,
                Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(project)))
                    if project == "project-24"
            )
        })
        .unwrap();
    app.list.focus_sidebar();
    app.list.select_sidebar(Some(project_index));

    let terminal_size: ratatui::layout::Size = (120, 24).into();
    let backend = ratatui::backend::TestBackend::new(terminal_size.width, terminal_size.height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let view = app.view();
    terminal
        .draw(|frame| {
            crate::tui::ui::render(frame, &app.store, &mut app.widgets, &mut app.list, &view)
        })
        .unwrap();

    let offset = app.list.sidebar_state().offset();
    assert!(offset > 0);
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
        Focus::Sidebar,
    )
    .unwrap();
    let visible_row = u16::try_from(project_index - offset).unwrap();

    app.dispatch_mouse(
        click_at(layout.content.x, layout.content.y + visible_row),
        terminal_size,
    )
    .await
    .unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("project-24".to_string())
    );
    assert_eq!(app.list.selected_sidebar(), Some(project_index));
}

async fn type_chars(app: &mut App, input: &str) {
    for ch in input.chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
}

async fn settle_search_preview(app: &mut App) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while app.search_preview_work_pending() {
            app.poll_search_preview().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("search preview settled");
}

fn assert_pending(app: &App, expected: &[&str]) {
    let expected = expected
        .iter()
        .map(|label| label.to_string())
        .collect::<Vec<_>>();
    assert_eq!(app.view().pending_shortcut, expected);
}

fn assert_pending_empty(app: &App) {
    assert!(app.view().pending_shortcut.is_empty());
}

async fn insert_title_conflict(
    pool: &SqlitePool,
    app: &mut App,
    selected: usize,
    local: &str,
    remote: &str,
) {
    let task_id = app.store.tasks[selected].task.id.clone();
    insert_title_conflict_for_task_id(pool, app, &task_id, local, remote).await;
}

async fn insert_title_conflict_for_task_id(
    pool: &SqlitePool,
    app: &mut App,
    task_id: &str,
    local: &str,
    remote: &str,
) {
    insert_conflict_for_task_id(pool, app, task_id, "title", local, remote).await;
}

async fn insert_conflict_for_task_id(
    pool: &SqlitePool,
    app: &mut App,
    task_id: &str,
    field: &str,
    local: &str,
    remote: &str,
) {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, ?, NULL, ?, ?, NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(task_id)
    .bind(field)
    .bind(local)
    .bind(remote)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    app.refresh().await.unwrap();
}

mod onboarding {
    use super::*;
    use crate::tui::event::{CommandLookup, lookup_command};
    use crate::tui::overlay::OverlayView;
    use crate::tui::store::OnboardingStatus;

    #[tokio::test]
    async fn automatic_welcome_completes_once() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;

        app.maybe_open_onboarding().await;
        assert!(app.onboarding_intro.is_some());
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::Onboarding {
                splash_underlay: true
            })
        ));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Onboarding {
                persist_on_exit: true
            })
        ));

        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(
            app.store.onboarding_status().await.unwrap(),
            OnboardingStatus::Complete
        );

        app.maybe_open_onboarding().await;
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn welcome_actions_open_the_first_use_flows() {
        let mut add_app = test_app().await;
        add_app.maybe_open_onboarding().await;
        add_app
            .dispatch_key(key(KeyCode::Char('a')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(add_app.overlay, Some(OverlayState::AddTask(_))));

        let mut help_app = test_app().await;
        help_app.maybe_open_onboarding().await;
        help_app
            .dispatch_key(shift_key(KeyCode::Char('?')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            help_app.overlay,
            Some(OverlayState::Help { scroll: 0 })
        ));
    }

    #[tokio::test]
    async fn forced_welcome_uses_automatic_intro() {
        let mut app = test_app().await;

        app.show_welcome_intro();
        assert!(app.onboarding_intro.is_some());
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::Onboarding {
                splash_underlay: true
            })
        ));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Onboarding {
                persist_on_exit: true
            })
        ));
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.store.onboarding_status().await.unwrap(),
            OnboardingStatus::Complete
        );
    }

    #[tokio::test]
    async fn welcome_replay_does_not_complete_automatic_onboarding() {
        let mut app = test_app().await;

        app.execute(Action::ShowWelcome).await.unwrap();
        assert!(app.onboarding_intro.is_none());
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::Onboarding {
                splash_underlay: false
            })
        ));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Onboarding {
                persist_on_exit: false
            })
        ));
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.store.onboarding_status().await.unwrap(),
            OnboardingStatus::Due
        );
    }

    #[tokio::test]
    async fn hidden_welcome_ignores_actions_and_modifiers() {
        let mut app = test_app().await;
        app.maybe_open_onboarding().await;

        app.dispatch_key(key(KeyCode::Enter), (69, 17).into())
            .await
            .unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Onboarding { .. })));

        app.dispatch_key(ctrl_a(), (80, 24).into()).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Onboarding { .. })));
        assert_eq!(
            app.store.onboarding_status().await.unwrap(),
            OnboardingStatus::Due
        );
    }

    #[tokio::test]
    async fn welcome_quit_completes_but_control_c_does_not() {
        let mut app = test_app().await;
        app.maybe_open_onboarding().await;
        app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
            .await
            .unwrap();
        assert!(app.should_quit);
        assert_eq!(
            app.store.onboarding_status().await.unwrap(),
            OnboardingStatus::Complete
        );

        let mut interrupted = test_app().await;
        interrupted.maybe_open_onboarding().await;
        interrupted
            .dispatch_key(ctrl_c(), (80, 24).into())
            .await
            .unwrap();
        assert!(interrupted.should_quit);
        assert_eq!(
            interrupted.store.onboarding_status().await.unwrap(),
            OnboardingStatus::Due
        );
    }

    #[test]
    fn welcome_command_is_discoverable() {
        assert_eq!(
            lookup_command("welcome"),
            CommandLookup::Found(Action::ShowWelcome)
        );
    }

    #[tokio::test]
    async fn update_review_supports_action_focus_and_later() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Update(
            crate::tui::overlay::UpdateOverlayState::Available {
                plan: crate::update::InstallPlan {
                    release: crate::update::Release {
                        version: semver::Version::new(99, 0, 0),
                        tag: "v99.0.0".to_string(),
                        archive_name: "aven-test.tar.gz".to_string(),
                        archive_url: "https://example.com/aven-test.tar.gz".to_string(),
                        checksum_url: "https://example.com/aven-test.sha256".to_string(),
                    },
                    method: crate::update::InstallMethod::Direct {
                        target: "/tmp/aven".into(),
                    },
                },
                notes: crate::tui::overlay::UpdateNotesState::Ready(
                    "## v99.0.0\n\n- release note".to_string(),
                ),
                scroll: 0,
                focus: crate::tui::overlay::UpdateActionFocus::Primary,
                cached: false,
            },
        ));

        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Update(
                crate::tui::overlay::UpdateOverlayState::Available {
                    focus: crate::tui::overlay::UpdateActionFocus::Later,
                    ..
                }
            ))
        ));

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn changelog_command_starts_loading_release_notes_from_github() {
        let mut app = test_app().await;

        app.execute(Action::ShowChangelog).await.unwrap();

        let Some(OverlayState::Changelog(state)) = app.overlay else {
            panic!("expected changelog overlay");
        };
        assert_eq!(state.markdown, "## Loading changelog…");
        assert!(app.changelog.work_pending());
    }

    #[tokio::test]
    async fn changelog_link_click_opens_documentation_in_browser() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Changelog(
            crate::tui::overlay::ChangelogState {
                markdown: "## Unreleased\n\n- [Read the guide](/guide/) for details.".to_string(),
                scroll: 0,
            },
        ));

        app.dispatch_mouse(click_at(10, 6), (100, 30).into())
            .await
            .unwrap();

        assert_eq!(
            crate::tui::platform::browser_url_for_test().as_deref(),
            Some("https://aven.raine.dev/guide/")
        );
        assert!(matches!(app.overlay, Some(OverlayState::Changelog(_))));
    }

    #[tokio::test]
    async fn changelog_reader_supports_less_style_paging_and_close() {
        let mut app = test_app().await;
        app.execute(Action::ShowChangelog).await.unwrap();
        app.overlay = Some(OverlayState::Changelog(
            crate::tui::overlay::ChangelogState {
                markdown: format!(
                    "## Release notes\n\n{}",
                    "- A changelog entry with enough content for paging.\n".repeat(40)
                ),
                scroll: 0,
            },
        ));

        app.handle_overlay_key(key(KeyCode::Char('d')))
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Changelog(ref state)) if state.scroll == 9
        ));
        app.handle_overlay_key(key(KeyCode::Char('u')))
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Changelog(ref state)) if state.scroll == 0
        ));
        app.handle_overlay_key(key(KeyCode::PageDown))
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Changelog(ref state)) if state.scroll == 17
        ));
        app.handle_overlay_key(key(KeyCode::PageUp)).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Changelog(ref state)) if state.scroll == 0
        ));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());
    }
}

mod theme_background {
    use super::*;
    use ratatui::style::Color;

    #[tokio::test]
    async fn tui_background_uses_terminal_background_for_main_surface() {
        let mut app = test_app().await;

        let buf = render_app_buffer(&mut app, 120, 30);

        assert_eq!(buf[(119, 10)].bg, Color::Reset);
    }
}

mod keyboard_dispatch {
    use super::*;
    use crate::tui::detail_session::DetailSession;

    #[tokio::test]
    async fn ctrl_c_quits_from_normal_mode() {
        let mut app = test_app().await;
        app.dispatch_key(ctrl_c(), (80, 24).into()).await.unwrap();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn ctrl_c_quits_while_overlay_captures_input() {
        let mut app = test_app().await;
        app.begin_search();
        app.dispatch_key(ctrl_c(), (80, 24).into()).await.unwrap();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn prefix_key_enters_prefix_mode() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        assert_pending(&app, &["t"]);
    }

    #[tokio::test]
    async fn add_task_alias_executes_immediately() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Title
        ));
    }

    #[tokio::test]
    async fn normal_dispatch_ignores_modified_shortcuts() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Priority target")).await;

        app.dispatch_key(ctrl_p(), (80, 24).into()).await.unwrap();

        assert!(app.overlay.is_none());
        assert_pending_empty(&app);

        app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('p')), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Priority)
        );
    }

    #[tokio::test]
    async fn prefix_is_inactive_while_overlay_captures_input() {
        let mut app = test_app().await;
        app.begin_search();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.input.as_str() == "t"
        ));
    }

    #[tokio::test]
    async fn esc_cancels_prefix_before_overlay() {
        let mut app = test_app().await;
        app.show_detail(0);
        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        assert_pending(&app, &["t"]);
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert_pending_empty(&app);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn invalid_continuation_shows_message() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
        assert_pending_empty(&app);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("invalid shortcut: t z")
        );
    }

    #[tokio::test]
    async fn valid_continuation_executes_and_clears() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_pending_empty(&app);
    }

    #[tokio::test]
    async fn order_shortcut_sets_sort() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        assert_eq!(app.store.view_state.order, TaskOrder::Priority);
        assert_eq!(toast_message(&app).as_deref(), Some("order priority asc"));
    }

    #[tokio::test]
    async fn created_order_shortcut_defaults_to_descending() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        assert_eq!(app.store.view_state.order, TaskOrder::Created);
        assert_eq!(app.store.sort_direction_label(), "desc");
        assert_eq!(toast_message(&app).as_deref(), Some("order created desc"));
    }

    #[tokio::test]
    async fn order_reverse_shortcut_toggles_direction() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
        assert_eq!(app.store.sort_direction_label(), "desc");
        assert_eq!(toast_message(&app).as_deref(), Some("order created desc"));
    }

    #[tokio::test]
    async fn due_order_shortcut_sets_sort() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(app.store.view_state.order, TaskOrder::DueOn);
        assert_eq!(toast_message(&app).as_deref(), Some("order due asc"));
    }

    #[tokio::test]
    async fn h_and_l_move_between_sidebar_and_tasks() {
        let mut app = test_app().await;
        app.list.focus_tasks();
        app.handle_normal_key(KeyCode::Char('h')).await.unwrap();
        assert_eq!(app.list.focus(), Focus::Sidebar);

        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert_eq!(app.list.focus(), Focus::Tasks);
    }

    #[tokio::test]
    async fn sidebar_selection_survives_focus_changes() {
        let mut app = test_app().await;
        app.list.focus_tasks();

        app.handle_normal_key(KeyCode::Char('h')).await.unwrap();
        let initial = app.list.selected_sidebar();
        app.handle_normal_key(KeyCode::Char('j')).await.unwrap();
        let selected = app.list.selected_sidebar();
        assert_ne!(selected, initial);

        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert_eq!(app.list.focus(), Focus::Tasks);
        app.handle_normal_key(KeyCode::Char('h')).await.unwrap();

        assert_eq!(app.list.focus(), Focus::Sidebar);
        assert_eq!(app.list.selected_sidebar(), selected);
    }

    #[tokio::test]
    async fn sidebar_toggle_shortcut_expands_task_list_and_restores_sidebar_focus() {
        let mut app = test_app().await;
        app.list.focus_sidebar();

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

        assert!(!app.list.sidebar_visible());
        assert_eq!(app.list.focus(), Focus::Tasks);
        assert_eq!(toast_message(&app).as_deref(), Some("task list expanded"));
        let hidden = app.view();
        assert!(!hidden.sidebar_visible);

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

        assert!(app.list.sidebar_visible());
        assert_eq!(app.list.focus(), Focus::Sidebar);
        assert_eq!(toast_message(&app).as_deref(), Some("sidebar visible"));
    }

    #[tokio::test]
    async fn command_panel_runs_sidebar_toggle() {
        let mut app = test_app().await;

        app.begin_command().await;
        type_chars(&mut app, "toggle-sidebar").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(!app.list.sidebar_visible());
        assert_eq!(app.list.focus(), Focus::Tasks);
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn hidden_sidebar_allows_wide_task_click_at_left_edge() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.list.select_task(Some(1));
        app.list.hide_sidebar();

        let area = ratatui::layout::Rect::new(0, 2, 140, 20);
        let mut click = None;
        'rows: for row in area.y..area.y.saturating_add(area.height) {
            for column in area.x..area.x.saturating_add(area.width) {
                if crate::tui::ui::task_at_position(
                    &app.store,
                    app.list.table_state(),
                    area,
                    column,
                    row,
                )
                .is_some_and(|hit| hit.task_index == 0)
                {
                    click = Some(task_row_click(column, row));
                    break 'rows;
                }
            }
        }
        let click = click.expect("expected task hit target");

        app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        assert_eq!(app.list.focus(), Focus::Tasks);
    }

    #[tokio::test]
    async fn hidden_sidebar_is_not_rendered_in_wide_layout() {
        let mut app = test_app().await;
        app.list.hide_sidebar();

        let text = render_app_text(&mut app, 140, 24);

        assert!(!text.contains("FILTERS"));
    }

    #[tokio::test]
    async fn h_reveals_sidebar() {
        let mut app = test_app().await;
        app.list.hide_sidebar();

        app.handle_normal_key(KeyCode::Char('h')).await.unwrap();

        assert!(app.list.sidebar_visible());
        assert_eq!(app.list.focus(), Focus::Sidebar);
    }

    #[tokio::test]
    async fn tab_reveals_sidebar_when_task_list_is_expanded() {
        let mut app = test_app().await;
        app.list.hide_sidebar();

        app.handle_normal_key(KeyCode::Tab).await.unwrap();

        assert!(app.list.sidebar_visible());
        assert_eq!(app.list.focus(), Focus::Sidebar);
        assert_eq!(toast_message(&app).as_deref(), Some("sidebar visible"));
    }

    #[tokio::test]
    async fn implemented_and_planned_commands_report_their_lifecycle() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(app.store.view_state.order, TaskOrder::DueOn);
        assert_eq!(toast_message(&app).as_deref(), Some("order due asc"));

        app.begin_command().await;
        type_chars(&mut app, "add-project-path").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(
                ":add-project-path is not yet implemented: requires a multi-step project/path picker flow"
            )
        );
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn esc_closes_every_overlay_variant() {
        let overlays = vec![
            OverlayState::Onboarding {
                persist_on_exit: false,
            },
            OverlayState::Help { scroll: 0 },
            OverlayState::Detail,
            OverlayState::DetailHelp { scroll: 0 },
            OverlayState::Search(SearchState {
                input: LineEdit::new("q".to_string()),
                results: Vec::new(),
                selected: 0,
                total_matches: 0,
                results_query: None,
                intent: SearchIntent::Navigate,
            }),
            OverlayState::Command {
                state: CommandState::new(LineEdit::new("ref".to_string())),
            },
            OverlayState::TextInput(TextInputState::new(
                TextIntent::AddProject,
                "T",
                "P",
                "x".to_string(),
            )),
            OverlayState::MultilineInput(MultilineInputState {
                intent: MultilineIntent::AddTaskNatural,
                title: "M".to_string(),
                prompt: "P".to_string(),
                lines: vec!["x".to_string()],
                row: 0,
                column: 1,
                mode: MultilineInputMode::Compose,
            }),
            OverlayState::Picker(PickerState {
                intent: PickerIntent::FilterLabel,
                title: "Pick".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                selected: 0,
                scroll: 0,
                multi: false,
                mode: PickerMode::Navigate,
            }),
            OverlayState::Confirm(ConfirmState {
                intent: ConfirmIntent::InitializeConfig {
                    path: std::path::PathBuf::from("/tmp/config.toml"),
                },
                title: "C".to_string(),
                prompt: "?".to_string(),
            }),
            OverlayState::TextPanel(TextPanelState {
                title: "Panel".to_string(),
                lines: vec!["line".to_string()],
                scroll: 0,
            }),
            OverlayState::SyncStatus(Box::default()),
        ];

        for overlay in overlays {
            let detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
            let mut app = test_app().await;
            if detail_help {
                app.detail = DetailSession::open(0);
            }
            app.overlay = Some(overlay);
            app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
                .await
                .unwrap();
            assert!(app.overlay.is_none());
            assert_eq!(app.detail.is_active(), detail_help);
            assert_pending_empty(&app);
        }
    }

    #[tokio::test]
    async fn mark_shortcuts_update_task_marks() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        create_and_select_task(&mut app, test_task_draft("second")).await;
        app.list.select_task(Some(first));
        let first_id = app.store.tasks[first].task.id.clone();
        app.notification = None;

        app.handle_normal_key(KeyCode::Char(' ')).await.unwrap();
        assert!(app.list.marked_task_ids().contains(&first_id));
        assert!(toast_message(&app).is_none());

        app.handle_normal_key(KeyCode::Char(' ')).await.unwrap();
        assert!(!app.list.marked_task_ids().contains(&first_id));
        assert!(toast_message(&app).is_none());

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('V')).await.unwrap();
        assert_eq!(app.list.marked_task_ids().len(), 2);
        assert!(toast_message(&app).is_none());

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('V')).await.unwrap();
        assert!(app.list.marked_task_ids().is_empty());
        assert!(toast_message(&app).is_none());

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('V')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        assert!(app.list.marked_task_ids().is_empty());
        assert!(toast_message(&app).is_none());
    }
}

mod attachment_paste {
    use super::*;

    async fn finish_attachment_work(app: &mut App) {
        for _ in 0..400 {
            app.poll_attachment_work().await.unwrap();
            if !app.attachment_controller.work_pending() {
                app.poll_attachment_work().await.unwrap();
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("attachment work did not finish");
    }

    async fn finish_task_intake_draft(app: &mut App) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                app.poll_pending_task_intake().await.unwrap();
                if matches!(app.overlay, Some(OverlayState::AddTask(_))) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("task intake should produce an add-task draft");
    }

    fn compressible_png_bytes() -> Vec<u8> {
        let width = 16u32;
        let height = 16u32;
        let mut raw = Vec::new();
        for _ in 0..height {
            raw.push(0);
            raw.extend(std::iter::repeat_n(255, width as usize * 4));
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, &raw).unwrap();
        let idat = encoder.finish().unwrap();

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend(width.to_be_bytes());
        ihdr.extend(height.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        append_png_chunk(&mut png, b"IHDR", &ihdr);
        append_png_chunk(&mut png, b"tEXt", &vec![b'a'; 4096]);
        append_png_chunk(&mut png, b"IDAT", &idat);
        append_png_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn jpeg_bytes() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 1))
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        bytes.into_inner()
    }

    fn append_png_chunk(png: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
        png.extend((data.len() as u32).to_be_bytes());
        png.extend(name);
        png.extend(data);
        let mut crc_data = Vec::with_capacity(name.len() + data.len());
        crc_data.extend(name);
        crc_data.extend(data);
        png.extend(crc32(&crc_data).to_be_bytes());
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    #[tokio::test]
    async fn detail_paste_image_path_attaches_to_selected_task() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let selected = create_and_select_task(&mut app, test_task_draft("image target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        let image = dir.path().join("photo.png");
        let original_bytes = compressible_png_bytes();
        std::fs::write(&image, &original_bytes).unwrap();
        app.show_detail(0);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        let pending = app.attachment_controller.views();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].task_id, task_id);
        assert_eq!(
            pending[0].status,
            crate::tui::attachment_controller::PendingAttachmentStatus::Preparing
        );
        let mut conn = pool.acquire().await.unwrap();
        let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(attachment_count, 0);
        drop(conn);
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("image already attached")
        );
        let item = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert!(item.task.description.is_empty());
        let mut conn = pool.acquire().await.unwrap();
        let attachments = crate::operations::attachment_read_items_by_task(
            &mut conn,
            app.store.active_workspace.id.as_str(),
            &task_id,
            false,
        )
        .await
        .unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0].attachment.filename.as_deref(),
            Some("photo.png")
        );
        assert_eq!(attachments[0].attachment.media_type, "image/png");
        assert_eq!(
            attachments[0].attachment.byte_size,
            original_bytes.len() as i64
        );
        assert!(attachments[0].has_blob);
    }

    #[tokio::test]
    async fn focused_detail_image_can_be_removed_after_confirmation() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        create_and_select_task(&mut app, test_task_draft("image target")).await;
        let image = dir.path().join("photo.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        app.show_detail(7);
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;
        let attachment_id = app.store.tasks[0].attachments[0].attachment_id.clone();
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target =
            Some(crate::tui::app::DetailTargetId::Attachment {
                attachment_id: attachment_id.clone(),
            });
        app.show_detail(7);

        app.dispatch_key(shift_key(KeyCode::Char('D')), (100, 30).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                intent: ConfirmIntent::DeleteAttachment { .. },
                ref title,
                ref prompt,
            })) if title == "Remove image" && prompt == "Remove photo.png?"
        ));

        app.dispatch_key(key(KeyCode::Char('y')), (100, 30).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
        assert!(app.store.tasks[0].attachments.is_empty());
        assert_eq!(toast_message(&app).as_deref(), Some("removed image"));
        let deleted: i64 =
            sqlx::query_scalar("SELECT deleted FROM task_attachments WHERE attachment_id = ?")
                .bind(&attachment_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(deleted, 1);
    }

    #[tokio::test]
    async fn removing_focused_attachment_selects_the_next_attachment() {
        let (dir, _pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        create_and_select_task(&mut app, test_task_draft("image target")).await;
        let first = dir.path().join("first.png");
        let second = dir.path().join("second.jpg");
        std::fs::write(&first, compressible_png_bytes()).unwrap();
        std::fs::write(&second, jpeg_bytes()).unwrap();
        app.show_detail(7);
        app.dispatch_paste(first.to_str().unwrap()).await.unwrap();
        app.dispatch_paste(second.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;
        let first_id = app.store.tasks[0].attachments[0].attachment_id.clone();
        let second_id = app.store.tasks[0].attachments[1].attachment_id.clone();
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target =
            Some(crate::tui::app::DetailTargetId::Attachment {
                attachment_id: first_id,
            });
        app.show_detail(7);

        app.dispatch_key(shift_key(KeyCode::Char('D')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('y')), (100, 30).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some(second_id.as_str())
        );
        assert_eq!(app.store.tasks[0].attachments.len(), 1);
    }

    #[tokio::test]
    async fn attachment_preview_remove_can_be_cancelled() {
        let (dir, _pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        create_and_select_task(&mut app, test_task_draft("image target")).await;
        let image = dir.path().join("photo.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        app.show_detail(3);
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;
        let attachment_id = app.store.tasks[0].attachments[0].attachment_id.clone();
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target =
            Some(crate::tui::app::DetailTargetId::Attachment {
                attachment_id: attachment_id.clone(),
            });
        app.overlay = Some(OverlayState::AttachmentPreview {
            attachment_id: attachment_id.clone(),
            scroll: 3,
        });

        app.dispatch_key(shift_key(KeyCode::Char('D')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('n')), (100, 30).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(app.store.tasks[0].attachments.len(), 1);
    }

    #[tokio::test]
    async fn detail_paste_image_path_obeys_optimization_config() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let mut config = crate::config::AppConfig::default();
        config.local.image_optimization = crate::config::ImageOptimizationConfig::Off;
        app.set_config(config);
        let selected = create_and_select_task(&mut app, test_task_draft("image target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        let image = dir.path().join("photo.png");
        let bytes = compressible_png_bytes();
        std::fs::write(&image, &bytes).unwrap();
        app.show_detail(0);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("image already attached")
        );
        let mut conn = pool.acquire().await.unwrap();
        let attachments = crate::operations::attachment_read_items_by_task(
            &mut conn,
            app.store.active_workspace.id.as_str(),
            &task_id,
            false,
        )
        .await
        .unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].attachment.byte_size, bytes.len() as i64);
    }

    #[tokio::test]
    async fn detail_paste_optimizes_asynchronously_when_enabled() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let mut config = crate::config::AppConfig::default();
        config.local.image_optimization = crate::config::ImageOptimizationConfig::Paste;
        app.set_config(config);
        let selected = create_and_select_task(&mut app, test_task_draft("optimized target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        let image = dir.path().join("optimized.png");
        let bytes = compressible_png_bytes();
        std::fs::write(&image, &bytes).unwrap();
        app.show_detail(0);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        assert_eq!(app.attachment_controller.views().len(), 1);
        finish_attachment_work(&mut app).await;

        let mut conn = pool.acquire().await.unwrap();
        let attachments = crate::operations::attachment_read_items_by_task(
            &mut conn,
            app.store.active_workspace.id.as_str(),
            &task_id,
            false,
        )
        .await
        .unwrap();
        assert_eq!(attachments.len(), 1);
        assert!(attachments[0].attachment.byte_size < bytes.len() as i64);
    }

    #[tokio::test]
    async fn detail_paste_failure_stays_visible_without_metadata() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        create_and_select_task(&mut app, test_task_draft("invalid image target")).await;
        let image = dir.path().join("invalid.png");
        std::fs::write(&image, b"not an image").unwrap();
        app.show_detail(0);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;

        let pending = app.attachment_controller.views();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].status,
            crate::tui::attachment_controller::PendingAttachmentStatus::Failed
        );
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("image attachment failed")
        );
        let mut conn = pool.acquire().await.unwrap();
        let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(attachment_count, 0);
    }

    #[tokio::test]
    async fn detail_paste_keeps_target_across_refresh_and_task_switch() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let first = create_and_select_task(&mut app, test_task_draft("first target")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second target")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        assert!(app.select_task_by_id(&first_id));
        let image = dir.path().join("switch.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        app.show_detail(0);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        assert!(app.select_task_by_id(&second_id));
        app.refresh().await.unwrap();
        assert_eq!(app.attachment_controller.views()[0].task_id, first_id);
        finish_attachment_work(&mut app).await;

        let selected_id = app
            .store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .id
            .clone();
        assert_eq!(selected_id, second_id);
        let mut conn = pool.acquire().await.unwrap();
        let first_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
                .bind(&first_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let second_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
                .bind(&second_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!((first_count, second_count), (1, 0));
    }

    #[tokio::test]
    async fn background_database_failure_cleans_staged_content() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        let db_path = dir.path().join("test.db");
        app.set_add_task_db_path(db_path.clone());
        create_and_select_task(&mut app, test_task_draft("failing target")).await;
        let image = dir.path().join("failure.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        app.show_detail(0);
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_background_attachment BEFORE INSERT ON task_attachments
             BEGIN SELECT RAISE(FAIL, 'injected attachment insert failure'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;

        assert_eq!(
            app.attachment_controller.views()[0].status,
            crate::tui::attachment_controller::PendingAttachmentStatus::Failed
        );
        let blob_dir = crate::config::resolve_blob_dir(&db_path, app.intake.config()).unwrap();
        let object_dir = blob_dir.join("objects").join("sha256");
        let object_count = std::fs::read_dir(object_dir)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(object_count, 0);
        let mut conn = pool.acquire().await.unwrap();
        let inventory_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_inventory")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(inventory_count, 0);
    }

    #[tokio::test]
    async fn concurrent_pastes_keep_paste_order() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let mut config = crate::config::AppConfig::default();
        config.local.image_optimization = crate::config::ImageOptimizationConfig::Paste;
        app.set_config(config);
        let selected = create_and_select_task(&mut app, test_task_draft("ordered target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        let first = dir.path().join("first.png");
        let second = dir.path().join("second.jpg");
        std::fs::write(&first, compressible_png_bytes()).unwrap();
        std::fs::write(&second, jpeg_bytes()).unwrap();
        app.show_detail(0);

        app.dispatch_paste(first.to_str().unwrap()).await.unwrap();
        app.dispatch_paste(second.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;

        let mut conn = pool.acquire().await.unwrap();
        let attachments = crate::operations::attachment_read_items_by_task(
            &mut conn,
            app.store.active_workspace.id.as_str(),
            &task_id,
            false,
        )
        .await
        .unwrap();
        let filenames = attachments
            .iter()
            .map(|item| item.attachment.filename.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(filenames, vec![Some("first.png"), Some("second.jpg")]);
    }

    #[tokio::test]
    async fn attachment_shutdown_finishes_commit_and_clears_pending_state() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        create_and_select_task(&mut app, test_task_draft("shutdown target")).await;
        let image = dir.path().join("shutdown.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        app.show_detail(0);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        app.attachment_controller.shutdown().await;

        assert!(app.attachment_controller.views().is_empty());
        let mut conn = pool.acquire().await.unwrap();
        let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(attachment_count, 1);
    }

    #[tokio::test]
    async fn detail_paste_image_path_ignores_existing_image() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let selected = create_and_select_task(&mut app, test_task_draft("image target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        let image = dir.path().join("photo.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        app.show_detail(0);

        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        finish_attachment_work(&mut app).await;

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("image already attached")
        );
        let item = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert!(item.task.description.is_empty());
        let mut conn = pool.acquire().await.unwrap();
        let attachments = crate::operations::attachment_read_items_by_task(
            &mut conn,
            app.store.active_workspace.id.as_str(),
            &task_id,
            false,
        )
        .await
        .unwrap();
        assert_eq!(attachments.len(), 1);
    }

    #[tokio::test]
    async fn add_task_paste_image_path_attaches_to_created_task_once() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let image = dir.path().join("composer.png");
        let image_bytes = compressible_png_bytes();
        std::fs::write(&image, &image_bytes).unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Include setup details").await;
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == crate::tui::authoring::AddTaskStep::Description
                    && state.description.lines.join("\n") == "Include setup details"
                    && state.attachments.len() == 1
                    && state.attachments[0].byte_size == image_bytes.len() as i64
                    && state.attachments[0].dimensions == Some((16, 16))
        ));
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(toast_message(&app).is_some_and(|message| message.starts_with("created task ")));
        let selected = app.list.selected_task().unwrap();
        let item = &app.store.tasks[selected];
        assert_eq!(item.task.title, "Write docs");
        assert_eq!(item.task.description, "Include setup details");
        let mut conn = pool.acquire().await.unwrap();
        let attachments = crate::operations::attachment_read_items_by_task(
            &mut conn,
            app.store.active_workspace.id.as_str(),
            &item.task.id,
            false,
        )
        .await
        .unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0].attachment.filename.as_deref(),
            Some("composer.png")
        );
        assert_eq!(
            attachments[0].attachment.alt_text.as_deref(),
            Some("pasted image")
        );
        assert!(attachments[0].has_blob);
    }

    #[tokio::test]
    async fn add_task_removing_draft_image_preserves_fields_without_database_or_sync_changes() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let image = dir.path().join("remove.png");
        let retained = dir.path().join("retained.jpg");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        std::fs::write(&retained, jpeg_bytes()).unwrap();
        let changes_before: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
            .fetch_one(&pool)
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Keep this title").await;
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        app.dispatch_paste(retained.to_str().unwrap())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Up)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Left)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('D')))
            .await
            .unwrap();

        assert_eq!(app.authoring.add_task_attachments().len(), 1);
        let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
            panic!("add task composer should remain open");
        };
        assert_eq!(state.title.as_str(), "Keep this title");
        assert_eq!(state.focus, crate::tui::authoring::AddTaskStep::Images);
        assert_eq!(
            state
                .attachments
                .iter()
                .map(|attachment| attachment.filename.as_str())
                .collect::<Vec<_>>(),
            vec!["retained.jpg"]
        );
        assert_eq!(state.selected_attachment, 0);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("removed draft image remove.png")
        );
        app.handle_overlay_key(key(KeyCode::Char('D')))
            .await
            .unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
            panic!("add task composer should remain open");
        };
        assert_eq!(state.focus, crate::tui::authoring::AddTaskStep::Title);
        assert!(state.attachments.is_empty());
        let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&pool)
            .await
            .unwrap();
        let changes_after: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(attachment_count, 0);
        assert_eq!(changes_after, changes_before);
    }

    #[tokio::test]
    async fn add_task_attachment_only_dismissal_confirms_and_discards_draft() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let image = dir.path().join("discard.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
            panic!("add task composer should remain open");
        };
        assert_eq!(state.attachments[0].filename, "discard.png");

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::AddTask(ref state))
                if state.mode == crate::tui::overlay::AddTaskMode::ConfirmDiscard
        ));
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.authoring.add_task_attachments().is_empty());
        let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(task_count, 0);
    }

    #[tokio::test]
    async fn failed_attachment_task_submission_preserves_composer_for_retry() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let image = dir.path().join("retry.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Retry attachment task").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Keep every field").await;
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_attachment_insert BEFORE INSERT ON task_attachments
             BEGIN SELECT RAISE(FAIL, 'injected attachment insert failure'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        assert!(app.handle_overlay_key(ctrl_s()).await.is_err());
        assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
        assert_eq!(app.authoring.add_task_attachments().len(), 1);
        let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
            panic!("add task composer should remain open");
        };
        assert_eq!(state.title.as_str(), "Retry attachment task");
        assert_eq!(state.description.lines.join("\n"), "Keep every field");
        let mut conn = pool.acquire().await.unwrap();
        let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(task_count, 0);
        sqlx::query("DROP TRIGGER fail_attachment_insert")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        app.handle_overlay_key(ctrl_s()).await.unwrap();
        let selected = app.list.selected_task().unwrap();
        let item = &app.store.tasks[selected];
        assert_eq!(item.task.title, "Retry attachment task");
        let mut conn = pool.acquire().await.unwrap();
        let attachment_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
                .bind(&item.task.id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(attachment_count, 1);
    }

    #[tokio::test]
    async fn failed_epic_child_attachment_submission_retains_owned_origin_for_retry() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let epic = crate::tui::store::EpicContext {
            epic_id: app.store.tasks[parent_index].task.id.clone(),
            display_ref: app.store.tasks[parent_index].display_ref.clone(),
            project_key: app.store.tasks[parent_index].task.project_key.clone(),
        };
        let return_search = SearchState::for_intent(SearchIntent::AddEpicChild {
            epic_id: epic.epic_id.clone(),
            display_ref: epic.display_ref.clone(),
            project_key: epic.project_key.clone(),
        });
        app.authoring
            .begin_epic_child(epic.clone(), return_search, "Retry child".to_string());
        app.begin_add_task_title();
        let image = dir.path().join("child-retry.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_child_attachment_insert BEFORE INSERT ON task_attachments
             BEGIN SELECT RAISE(FAIL, 'injected child attachment failure'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
        assert_eq!(app.authoring.add_task_attachments().len(), 1);
        assert!(matches!(
            app.authoring
                .submission_context()
                .map(|context| context.origin),
            Some(AddTaskOrigin::EpicChild { .. })
        ));
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("DROP TRIGGER fail_child_attachment_insert")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let parent = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == epic.epic_id)
            .unwrap();
        assert!(
            parent
                .epic_children
                .iter()
                .any(|child| child.title == "Retry child")
        );
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn rolled_back_attachment_task_is_retained_for_retry() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        let image = dir.path().join("committed.png");
        std::fs::write(&image, compressible_png_bytes()).unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Committed attachment task").await;
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_undo_insert BEFORE INSERT ON tui_undo_entries
             BEGIN SELECT RAISE(FAIL, 'injected undo failure'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        let error = app.handle_overlay_key(ctrl_s()).await.unwrap_err();
        assert!(!crate::tui::store::task_creation_committed(&error));
        assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
        assert_eq!(app.authoring.add_task_attachments().len(), 1);
        let mut conn = pool.acquire().await.unwrap();
        let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!((task_count, attachment_count), (0, 0));
    }

    #[tokio::test]
    async fn natural_add_paste_image_path_carries_attachment_into_created_task() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.set_add_task_db_path(dir.path().join("test.db"));
        configure_task_intake_success(&mut app, dir.path(), "parsed natural task", "model details");
        let image = dir.path().join("natural.webp");
        std::fs::write(&image, compressible_png_bytes()).unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.begin_add_task_natural();
        app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
        type_chars(&mut app, "make a task from this").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();
        finish_task_intake_draft(&mut app).await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        let item = &app.store.tasks[selected];
        assert_eq!(item.task.title, "parsed natural task");
        assert_eq!(item.task.description, "model details");
        let mut conn = pool.acquire().await.unwrap();
        let attachments = crate::operations::attachment_read_items_by_task(
            &mut conn,
            app.store.active_workspace.id.as_str(),
            &item.task.id,
            false,
        )
        .await
        .unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0].attachment.filename.as_deref(),
            Some("natural.webp")
        );
    }

    #[tokio::test]
    async fn natural_add_epic_child_uses_authoring_origin() {
        let (dir, _pool, mut app) = test_app_with_pool().await;
        configure_task_intake_success(&mut app, dir.path(), "Parsed child", "Model details");
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let epic = crate::tui::store::EpicContext {
            epic_id: app.store.tasks[parent_index].task.id.clone(),
            display_ref: app.store.tasks[parent_index].display_ref.clone(),
            project_key: app.store.tasks[parent_index].task.project_key.clone(),
        };
        let return_search = SearchState::for_intent(SearchIntent::AddEpicChild {
            epic_id: epic.epic_id.clone(),
            display_ref: epic.display_ref.clone(),
            project_key: epic.project_key.clone(),
        });
        app.authoring
            .begin_epic_child(epic.clone(), return_search, String::new());
        app.begin_add_task_title();
        app.begin_add_task_natural();
        type_chars(&mut app, "make a child task").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();
        finish_task_intake_draft(&mut app).await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let parent = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == epic.epic_id)
            .unwrap();
        assert!(
            parent
                .epic_children
                .iter()
                .any(|child| child.title == "Parsed child")
        );
        assert!(app.authoring.is_idle());
    }

    fn configure_task_intake_success(
        app: &mut App,
        dir: &std::path::Path,
        title: &str,
        description: &str,
    ) {
        let command = dir.join("task-intake-success.sh");
        std::fs::write(
            &command,
            format!(
                "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{{\"title\":\"{}\",\"description\":\"{}\",\"project\":null,\"priority\":\"none\",\"labels\":[]}}'\n",
                title, description
            ),
        )
        .unwrap();
        set_executable(&command);
        let mut config = app.intake.config().clone();
        config.agent.task_intake.command = Some(command.display().to_string());
        config.agent.task_intake.args = Vec::new();
        config.agent.task_intake.timeout_seconds = Some(5);
        app.set_config(config);
    }

    #[cfg(unix)]
    fn set_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn set_executable(_path: &std::path::Path) {}
}

mod command_and_config_overlays {
    use super::*;

    #[tokio::test]
    async fn command_overlay_executes_unique_lookup_and_keeps_overlay_on_errors() {
        let mut app = test_app().await;

        app.begin_command().await;
        for ch in "ref".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());

        app.begin_command().await;
        app.handle_overlay_key(key(KeyCode::Char('s')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
        assert_eq!(toast_message(&app).as_deref(), Some("ambiguous command: s"));

        app.begin_command().await;
        for ch in "zzzz".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("unknown command: zzzz")
        );
    }

    #[tokio::test]
    async fn command_overlay_tab_completes_unique_suffix_alias() {
        let mut app = test_app().await;

        app.begin_command().await;
        type_chars(&mut app, ":todo").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-todo" && state.input.cursor == "status-todo".len()
        ));
    }

    #[tokio::test]
    async fn command_overlay_tab_selects_first_partial_command_match() {
        let mut app = test_app().await;

        app.begin_command().await;
        type_chars(&mut app, ":delet").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "delete"
                    && state.input.cursor == "delete".len()
                    && state.highlighted.as_deref() == Some("delete")
        ));
    }

    #[tokio::test]
    async fn command_overlay_tab_cycles_from_exact_command_match() {
        let mut app = test_app().await;

        app.begin_command().await;
        type_chars(&mut app, ":delete").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "delete"
                    && state.input.cursor == "delete".len()
                    && state.highlighted.as_deref() == Some("delete")
        ));

        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "delete-project"
                    && state.highlighted.as_deref() == Some("delete-project")
        ));

        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "filter-deleted"
                    && state.highlighted.as_deref() == Some("filter-deleted")
        ));
    }

    #[tokio::test]
    async fn command_overlay_tab_cycles_ambiguous_matches() {
        let mut app = test_app().await;

        app.begin_command().await;
        type_chars(&mut app, "stat").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-picker"
                    && state.input.cursor == "status-picker".len()
                    && state.cycle_input.as_deref() == Some("stat")
                    && state.highlighted.as_deref() == Some("status-picker")
        ));

        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-inbox"
                    && state.input.cursor == "status-inbox".len()
                    && state.cycle_input.as_deref() == Some("stat")
                    && state.highlighted.as_deref() == Some("status-inbox")
        ));
    }

    #[tokio::test]
    async fn command_overlay_tab_cycles_from_exact_alias_match() {
        let mut app = test_app().await;

        app.begin_command().await;
        type_chars(&mut app, ":todo").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-todo"
                    && state.highlighted.as_deref() == Some("status-todo")
        ));

        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "view-todo"
                    && state.highlighted.as_deref() == Some("view-todo")
        ));
    }

    #[tokio::test]
    async fn command_overlay_edit_resets_completion_cycle() {
        let mut app = test_app().await;

        app.begin_command().await;
        type_chars(&mut app, "stat").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Backspace))
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Command { state })
                if state.cycle_input.is_none() && state.highlighted.is_none()
        ));
    }

    #[tokio::test]
    async fn search_overlay_shows_live_results_and_navigation() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;
        create_and_select_task(&mut app, test_task_draft("needle second")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;
        settle_search_preview(&mut app).await;

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.results.len(), 2);
        assert_eq!(state.selected, 0);
        assert!(
            state
                .results
                .iter()
                .any(|result| result.title == "needle first")
        );
        assert!(
            state
                .results
                .iter()
                .any(|result| result.title == "needle second")
        );

        app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.selected == 1
        ));
    }

    #[tokio::test]
    async fn search_overlay_allows_j_and_k_text_input() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("keyboard needle")).await;

        app.begin_search();
        type_chars(&mut app, "jk").await;

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.input.as_str() == "jk"
        ));
    }

    #[tokio::test]
    async fn search_overlay_ctrl_n_and_ctrl_p_select_results() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;
        create_and_select_task(&mut app, test_task_draft("needle second")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;
        settle_search_preview(&mut app).await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.selected == 1
        ));

        app.handle_overlay_key(ctrl_p()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.selected == 0
        ));
    }

    #[tokio::test]
    async fn search_overlay_refreshes_results_after_paste() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

        app.begin_search();
        app.dispatch_paste("needle").await.unwrap();
        settle_search_preview(&mut app).await;

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.input.as_str(), "needle");
        assert!(
            state
                .results
                .iter()
                .any(|result| result.title == "pasted needle")
        );
    }

    #[tokio::test]
    async fn search_overlay_enter_opens_selected_task_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("detail needle")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;
        settle_search_preview(&mut app).await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "detail needle");
    }

    #[tokio::test]
    async fn search_overlay_tab_opens_results_list() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("list needle")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;
        settle_search_preview(&mut app).await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.title, "list needle");
    }

    #[tokio::test]
    async fn search_overlay_keeps_input_immediate_while_preview_runs() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;

        app.begin_search();
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Char('e')))
            .await
            .unwrap();

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.input.as_str(), "ne");
        assert!(matches!(
            app.search.view(),
            SearchControllerView::Running { query: "ne" }
        ));
    }

    #[tokio::test]
    async fn search_overlay_marks_first_preview_as_stale_while_running() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;

        app.begin_search();
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.input.as_str(), "n");
        assert!(state.results.is_empty());
        assert!(state.results_query.is_none());
        assert!(!state.results_are_current());
        assert!(matches!(
            app.search.view(),
            SearchControllerView::Running { query: "n" }
        ));
    }

    #[tokio::test]
    async fn search_overlay_runs_latest_preview_after_input_changes() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;

        app.begin_search();
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Char('e')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Char('e')))
            .await
            .unwrap();

        assert!(matches!(
            app.search.view(),
            SearchControllerView::Running { query: "nee" }
        ));

        settle_search_preview(&mut app).await;

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.input.as_str(), "nee");
        assert_eq!(state.results_query.as_deref(), Some("nee"));
    }

    #[tokio::test]
    async fn search_overlay_ignores_stale_preview_results() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("old needle")).await;
        create_and_select_task(&mut app, test_task_draft("new needle")).await;

        app.begin_search();
        app.handle_overlay_key(key(KeyCode::Char('o')))
            .await
            .unwrap();

        if let Some(OverlayState::Search(state)) = &mut app.overlay {
            state.input.text = "new".to_string();
        }
        settle_search_preview(&mut app).await;

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_ne!(state.results_query.as_deref(), Some("o"));
    }

    #[tokio::test]
    async fn search_overlay_tab_submits_current_input_when_preview_lags() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("current needle")).await;

        app.begin_search();
        type_chars(&mut app, "current").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.store.tasks[0].task.title, "current needle");
        assert!(!app.search_preview_work_pending());
    }

    #[tokio::test]
    async fn search_overlay_keeps_stale_results_when_input_changes() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("alpha needle")).await;

        app.begin_search();
        type_chars(&mut app, "alpha").await;
        settle_search_preview(&mut app).await;

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.results_query.as_deref(), Some("alpha"));
        assert!(!state.results.is_empty());

        app.handle_overlay_key(key(KeyCode::Char('x')))
            .await
            .unwrap();

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.input.as_str(), "alphax");
        assert!(!state.results.is_empty());
        assert_eq!(state.results_query.as_deref(), Some("alpha"));
        assert!(!state.results_are_current());
        assert!(matches!(
            app.search.view(),
            SearchControllerView::Running { query: "alphax" }
        ));
    }

    #[tokio::test]
    async fn search_overlay_enter_does_not_open_stale_selected_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("alpha needle")).await;

        app.begin_search();
        type_chars(&mut app, "alpha").await;
        settle_search_preview(&mut app).await;
        app.handle_overlay_key(key(KeyCode::Char('x')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.search_preview_work_pending());
    }

    #[tokio::test]
    async fn search_overlay_cancel_aborts_active_preview() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;

        app.begin_search();
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();
        assert!(app.search_preview_work_pending());

        app.cancel_overlay();

        assert!(app.overlay.is_none());
        assert!(!app.search_preview_work_pending());
    }

    #[tokio::test]
    async fn search_overlay_paste_keeps_input_immediate_while_preview_runs() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

        app.begin_search();
        app.dispatch_paste("needle").await.unwrap();

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.input.as_str(), "needle");
        assert!(app.search_preview_work_pending());
    }

    #[tokio::test]
    async fn search_overlay_whitespace_paste_clears_results() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

        app.begin_search();
        app.dispatch_paste("needle").await.unwrap();
        settle_search_preview(&mut app).await;
        app.handle_overlay_key(ctrl_u()).await.unwrap();
        app.dispatch_paste("   ").await.unwrap();

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert!(state.results.is_empty());
        assert!(state.results_query.is_none());
        assert!(!app.search_preview_work_pending());
    }

    #[tokio::test]
    async fn search_replaces_existing_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help { scroll: 0 });
        app.begin_search();
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    }

    #[tokio::test]
    async fn toggle_help_closes_active_help_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help { scroll: 0 });
        app.toggle_help_at_height(24);
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn help_key_opens_help_overlay() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('?')).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Help { .. })));
    }

    #[tokio::test]
    async fn config_info_opens_text_panel() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        let Some(OverlayState::TextPanel(panel)) = app.overlay else {
            panic!("expected text panel");
        };
        assert_eq!(panel.title, CONFIG_INFO_TITLE);
        assert!(panel.lines.iter().any(|line| line.contains("config path:")));
    }

    #[tokio::test]
    async fn config_status_opens_sync_status() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

        let Some(OverlayState::SyncStatus(status)) = app.overlay else {
            panic!("expected sync status");
        };
        assert_eq!(*status, app.store.sync_status);
    }

    #[tokio::test]
    async fn config_paths_opens_text_panel() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        let Some(OverlayState::TextPanel(panel)) = app.overlay else {
            panic!("expected text panel");
        };
        assert_eq!(panel.title, CONFIG_PATHS_TITLE);
        assert!(
            panel
                .lines
                .iter()
                .any(|line| line.contains("effective database:"))
        );
    }

    #[tokio::test]
    async fn database_stats_opens_text_panel() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Stats target")).await;
        let selected = app.list.selected_task();
        app.store.update_status(selected, "done").await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                title: "Urgent task".to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "urgent".to_string(),
                source: TaskSource::Unknown,
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await;
        app.store
            .update_deleted(app.list.selected_task(), true)
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();

        let Some(OverlayState::DatabaseStats { stats, scroll }) = app.overlay else {
            panic!("expected database stats");
        };
        assert_eq!(scroll, 0);
        assert_eq!(stats.total_tasks, 2);
        assert_eq!(stats.open_tasks, 0);
        assert_eq!(stats.deleted_tasks, 1);
        assert_eq!(stats.statuses.done, 1);
        assert_eq!(stats.priorities.urgent, 0);
        assert_eq!(stats.notes, 0);
        assert!(stats.sqlite_page_size > 0);
        assert!(stats.sqlite_page_count > 0);
    }

    #[tokio::test]
    async fn command_panel_runs_database_stats() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Stats target")).await;

        app.begin_command().await;
        type_chars(&mut app, "database-stats").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::DatabaseStats { .. })
        ));
    }

    #[tokio::test]
    async fn config_init_requires_confirmation() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('i')).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                intent: ConfirmIntent::InitializeConfig { ref path },
                ref title,
                ..
            })) if title == CONFIG_INIT_TITLE && path == &crate::config::config_file_path().unwrap()
        ));
    }

    #[tokio::test]
    async fn config_init_cancel_does_not_set_success_message() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('i')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn command_panel_runs_config_show() {
        let mut app = test_app().await;
        app.begin_command().await;
        type_chars(&mut app, "config-show").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(TextPanelState { ref title, .. })) if title == CONFIG_INFO_TITLE
        ));
    }

    #[tokio::test]
    async fn command_panel_runs_workspace_switch() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        app.begin_command().await;
        type_chars(&mut app, "workspace-switch").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, items, .. }))
                if title == SWITCH_WORKSPACE_TITLE
                    && items.iter().any(|item| item.value == "client-work")
        ));

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn status_shortcut_uses_footer_chooser_for_unmarked_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("unmarked")).await;

        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
        assert!(app.overlay.is_none());

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Todo);
    }

    #[tokio::test]
    async fn status_shortcut_uses_footer_chooser_for_marked_tasks_with_undo() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());

        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
        assert!(app.overlay.is_none());

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();

        let status_for = |app: &App, task_id: &str| {
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id.as_str() == task_id)
                .unwrap()
                .task
                .status
        };
        assert_eq!(status_for(&app, &first_id), TaskStatus::Todo);
        assert_eq!(status_for(&app, &second_id), TaskStatus::Todo);
        assert_eq!(status_for(&app, &third_id), TaskStatus::Inbox);

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

        assert_eq!(status_for(&app, &first_id), TaskStatus::Inbox);
        assert_eq!(status_for(&app, &second_id), TaskStatus::Inbox);
        assert_eq!(status_for(&app, &third_id), TaskStatus::Inbox);
    }

    #[tokio::test]
    async fn submit_edit_project_updates_only_marked_tasks() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        let original_project = app.store.tasks[third].task.project_key.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());
        app.begin_edit_project();

        let (selection, mixed) = match &app.overlay {
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::EditProject { selection, mixed },
                ..
            })) => (selection.clone(), *mixed),
            overlay => panic!("expected project edit intent, got {overlay:?}"),
        };
        app.submit_edit_project(selection, mixed, "mobile-app".to_string())
            .await
            .unwrap();

        let project_for = |app: &App, task_id: &str| {
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id.as_str() == task_id)
                .unwrap()
                .task
                .project_key
                .clone()
        };
        assert_eq!(project_for(&app, &first_id), "mobile-app");
        assert_eq!(project_for(&app, &second_id), "mobile-app");
        assert_eq!(project_for(&app, &third_id), original_project);
    }

    #[tokio::test]
    async fn project_edit_records_moved_task_for_detail_recall() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let selected = create_and_select_task(&mut app, test_task_draft("moved task")).await;
        let changed_id = app.store.tasks[selected].task.id.clone();
        let original_project = app.store.tasks[selected].task.project_key.clone();
        app.show_scope(TaskScopeTarget::Project(original_project))
            .await
            .unwrap();
        app.list.select_task(Some(0));
        app.show_detail(0);
        app.begin_edit_project();
        let (selection, mixed) = match &app.overlay {
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::EditProject { selection, mixed },
                ..
            })) => (selection.clone(), *mixed),
            overlay => panic!("expected project edit intent, got {overlay:?}"),
        };

        app.submit_edit_project(selection, mixed, "mobile-app".to_string())
            .await
            .unwrap();

        assert_eq!(app.list.last_changed_task_id(), Some(&changed_id));
        assert!(toast_message(&app).unwrap().contains("g . return"));
        app.execute(Action::ReturnToLastChange).await.unwrap();
        assert!(app.detail.is_active());
        assert_eq!(app.store.view_state.view, TaskView::Search);
        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.id, changed_id);
    }

    #[tokio::test]
    async fn project_edit_keeps_captured_targets_and_anchor_across_refresh() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());
        app.begin_edit_project();

        app.list.clear_marks();
        app.list.mark(third_id.clone());
        app.list.select_task(Some(first));
        app.store.refresh(None).await.unwrap();
        let (selection, mixed) = match &app.overlay {
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::EditProject { selection, mixed },
                ..
            })) => (selection.clone(), *mixed),
            overlay => panic!("expected project edit intent, got {overlay:?}"),
        };
        app.submit_edit_project(selection, mixed, "mobile-app".to_string())
            .await
            .unwrap();

        let project_for = |app: &App, task_id: &crate::ids::TaskId| {
            app.store
                .tasks
                .iter()
                .find(|item| &item.task.id == task_id)
                .unwrap()
                .task
                .project_key
                .clone()
        };
        assert_eq!(project_for(&app, &first_id), "mobile-app");
        assert_eq!(project_for(&app, &second_id), "mobile-app");
        assert_ne!(project_for(&app, &third_id), "mobile-app");
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&third_id)
        );
    }

    #[tokio::test]
    async fn empty_project_retry_keeps_captured_targets() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());
        app.begin_edit_project();
        let (selection, mixed) = match &app.overlay {
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::EditProject { selection, mixed },
                ..
            })) => (selection.clone(), *mixed),
            overlay => panic!("expected project edit intent, got {overlay:?}"),
        };

        let expected_ids = selection.ids().cloned().collect::<Vec<_>>();
        app.list.clear_marks();
        app.list.mark(third_id);
        app.handle_overlay_submit(crate::tui::overlay::OverlaySubmit::Picker {
            intent: PickerIntent::EditProject { selection, mixed },
            values: Vec::new(),
            partial_values: Vec::new(),
        })
        .await
        .unwrap();

        let retried_ids = match &app.overlay {
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::EditProject { selection, .. },
                ..
            })) => selection.ids().cloned().collect::<Vec<_>>(),
            overlay => panic!("expected retried project edit intent, got {overlay:?}"),
        };
        assert_eq!(retried_ids, expected_ids);
    }

    #[tokio::test]
    async fn submit_edit_priority_updates_only_marked_tasks() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());
        app.begin_edit_priority();

        let (selection, mixed) = match &app.overlay {
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::EditPriority { selection, mixed },
                ..
            })) => (selection.clone(), *mixed),
            overlay => panic!("expected priority edit intent, got {overlay:?}"),
        };
        app.submit_edit_priority(selection, mixed, "high".to_string())
            .await
            .unwrap();

        let priority_for = |app: &App, task_id: &str| {
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id.as_str() == task_id)
                .unwrap()
                .task
                .priority
        };
        assert_eq!(priority_for(&app, &first_id), TaskPriority::High);
        assert_eq!(priority_for(&app, &second_id), TaskPriority::High);
        assert_eq!(priority_for(&app, &third_id), TaskPriority::None);
    }

    #[tokio::test]
    async fn mixed_marked_due_dates_keep_then_set_and_undo_as_one_batch() {
        let mut app = test_app().await;
        let first = create_and_select_task(
            &mut app,
            TaskDraft {
                due_on: Some("2099-01-01".to_string()),
                ..test_task_draft("first")
            },
        )
        .await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());

        app.begin_edit_due();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if state.title == "Edit due date · 2 marked tasks"
                    && state.prompt.contains("Current: varies")
                    && state.input.as_str().is_empty()
        ));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id == first_id)
                .unwrap()
                .task
                .due_on
                .as_deref(),
            Some("2099-01-01")
        );

        app.begin_edit_due();
        type_chars(&mut app, "2099-02-02").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        for task_id in [&first_id, &second_id] {
            assert_eq!(
                app.store
                    .tasks
                    .iter()
                    .find(|item| &item.task.id == task_id)
                    .unwrap()
                    .task
                    .due_on
                    .as_deref(),
                Some("2099-02-02")
            );
        }
        assert!(
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id == third_id)
                .unwrap()
                .task
                .due_on
                .is_none()
        );
        assert_eq!(app.list.marked_task_ids().len(), 2);

        app.undo_last().await.unwrap();
        assert_eq!(
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id == first_id)
                .unwrap()
                .task
                .due_on
                .as_deref(),
            Some("2099-01-01")
        );
        assert!(
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id == second_id)
                .unwrap()
                .task
                .due_on
                .is_none()
        );
    }

    #[tokio::test]
    async fn clearing_marked_due_dates_requires_confirmation() {
        let mut app = test_app().await;
        let first = create_and_select_task(
            &mut app,
            TaskDraft {
                due_on: Some("2099-01-01".to_string()),
                ..test_task_draft("first clear")
            },
        )
        .await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(
            &mut app,
            TaskDraft {
                due_on: Some("2099-02-02".to_string()),
                ..test_task_draft("second clear")
            },
        )
        .await;
        let second_id = app.store.tasks[second].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());

        app.begin_edit_due();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Confirm(state))
                if matches!(state.intent, ConfirmIntent::ClearDue { .. })
                    && state.prompt == "Clear due date on 2 marked tasks?"
        ));
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        for task_id in [first_id, second_id] {
            assert!(
                app.store
                    .tasks
                    .iter()
                    .find(|item| item.task.id == task_id)
                    .unwrap()
                    .task
                    .due_on
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn begin_delete_task_confirms_marked_tasks_when_tasks_are_marked() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        app.list.mark(first_id);
        app.list.mark(second_id);

        app.begin_delete_task();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Confirm(ConfirmState { prompt, .. }))
                if prompt == "Delete 2 marked tasks?"
        ));
    }

    #[tokio::test]
    async fn update_deleted_updates_only_marked_tasks() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());

        app.update_deleted(true).await.unwrap();

        assert!(!app.store.tasks.iter().any(|item| item.task.id == first_id));
        assert!(!app.store.tasks.iter().any(|item| item.task.id == second_id));
        assert!(app.store.tasks.iter().any(|item| item.task.id == third_id));
    }

    #[tokio::test]
    async fn edit_labels_shortcut_uses_one_mark_as_single_task_scope() {
        let mut app = test_app().await;
        let index = create_and_select_task(&mut app, test_task_draft("marked")).await;
        let id = app.store.tasks[index].task.id.clone();
        app.list.mark(id);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TagCombobox(state))
                if matches!(state.intent, TagComboboxIntent::EditLabels { .. })
                    && state.title == EDIT_LABELS_TITLE
        ));
    }

    #[tokio::test]
    async fn edit_labels_shortcut_uses_selected_task_without_marks() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("selected")).await;

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TagCombobox(state))
                if matches!(state.intent, TagComboboxIntent::EditLabels { .. })
        ));
    }

    #[tokio::test]
    async fn submit_edit_labels_multi_updates_only_marked_tasks() {
        let mut app = test_app().await;
        app.store.create_label("batch".to_string()).await.unwrap();
        let first = create_and_select_task(&mut app, test_task_draft("first")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        let third = create_and_select_task(&mut app, test_task_draft("third")).await;
        let third_id = app.store.tasks[third].task.id.clone();
        app.list.mark(first_id.clone());
        app.list.mark(second_id.clone());
        app.begin_edit_labels();

        let selection = match &app.overlay {
            Some(OverlayState::TagCombobox(state)) => match &state.intent {
                TagComboboxIntent::EditLabelsMulti { selection } => selection.clone(),
                intent => panic!("expected multi-label edit intent, got {intent:?}"),
            },
            overlay => panic!("expected label editor, got {overlay:?}"),
        };
        app.submit_edit_labels_multi(selection, vec!["batch".to_string()], Vec::new())
            .await
            .unwrap();

        let labels_for = |app: &App, task_id: &str| {
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id.as_str() == task_id)
                .unwrap()
                .labels
                .clone()
        };
        assert_eq!(labels_for(&app, &first_id), vec!["batch".to_string()]);
        assert_eq!(labels_for(&app, &second_id), vec!["batch".to_string()]);
        assert!(labels_for(&app, &third_id).is_empty());
    }
}

mod filters_and_workspaces {
    use super::*;

    #[tokio::test]
    async fn back_shortcut_restores_filter_and_project_navigation() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.submit_filter_priority(vec!["urgent".to_string()])
            .await
            .unwrap();

        assert_eq!(
            app.store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
        assert_eq!(
            app.store.view_state.filter_modifiers.priority.as_deref(),
            Some("urgent")
        );

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('[')).await.unwrap();

        assert_eq!(
            app.store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
        assert_eq!(app.store.view_state.filter_modifiers.priority, None);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("returned to previous navigation state")
        );

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('[')).await.unwrap();

        assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
    }

    #[tokio::test]
    async fn back_shortcut_reports_empty_history() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('[')).await.unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no previous navigation state")
        );
    }

    #[tokio::test]
    async fn scope_project_shortcut_opens_project_picker() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, .. })) if title == SCOPE_PROJECT_TITLE
        ));
    }

    #[tokio::test]
    async fn done_view_shortcut_keeps_project_scope() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.store.create_project("Ops".to_string()).await.unwrap();
        for (title, project) in [("Mobile done", "mobile-app"), ("Ops done", "ops")] {
            let (_, selected) = app
                .store
                .create_task(
                    TaskDraft {
                        title: title.to_string(),
                        description: String::new(),
                        project: Some(project.to_string()),
                        status: "inbox".to_string(),
                        priority: "none".to_string(),
                        source: TaskSource::Unknown,
                        labels: Vec::new(),
                        available_at: None,
                        due_on: None,
                        is_epic: false,
                    },
                    None,
                )
                .await
                .unwrap();
            app.store.update_status(selected, "done").await.unwrap();
        }
        let selected = app
            .store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.apply_filter_selection(selected);

        app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        assert_eq!(
            app.store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
        assert_eq!(app.store.view_state.view, TaskView::Done);
        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.title, "Mobile done");
        assert_eq!(toast_message(&app).as_deref(), Some("view updated"));
    }

    #[tokio::test]
    async fn filter_shortcuts_apply_label_status_priority_and_deleted() {
        let mut app = test_app().await;
        app.store.create_label("backend".to_string()).await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                title: "Filtered task".to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "urgent".to_string(),
                source: TaskSource::Unknown,
                labels: vec!["backend".to_string()],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await;

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('/')))
            .await
            .unwrap();
        type_chars(&mut app, "backend").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            app.store.view_state.filter_modifiers.label.as_deref(),
            Some("backend")
        );
        assert_eq!(toast_message(&app).as_deref(), Some("label filter applied"));
        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('/')))
            .await
            .unwrap();
        type_chars(&mut app, "urgent").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            app.store.view_state.filter_modifiers.priority.as_deref(),
            Some("urgent")
        );

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        assert!(app.store.view_state.filter_modifiers.include_deleted);
        assert!(!app.store.view_state.filter_modifiers.deleted_only);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("showing deleted tasks")
        );

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        assert!(app.store.view_state.filter_modifiers.include_deleted);
        assert!(app.store.view_state.filter_modifiers.deleted_only);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("showing deleted tasks only")
        );
    }

    #[tokio::test]
    async fn switch_workspace_shortcut_opens_picker() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('w')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, .. })) if title == SWITCH_WORKSPACE_TITLE
        ));

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn refresh_reports_invalid_project_scope_fallback() {
        let mut app = test_app().await;
        app.store.view_state.scope = TaskScope::Project("missing".to_string());
        app.store.view_state.view = TaskView::Todo;

        app.refresh().await.unwrap();

        assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
        assert_eq!(app.store.view_state.view, TaskView::Todo);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("project scope missing is no longer available")
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    }

    #[tokio::test]
    async fn switch_workspace_changes_active_workspace() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("Default only")).await;

        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);
        app.refresh().await.unwrap();

        app.store.view_state.view = TaskView::Todo;

        let (message, selected) = app
            .store
            .switch_workspace("client-work".to_string())
            .await
            .unwrap();
        app.apply_filter_selection(selected);
        app.set_success(message);

        assert_eq!(app.store.active_workspace.key, "client-work");
        assert_eq!(app.store.view_state.view, TaskView::Todo);
        assert!(app.store.tasks.is_empty());
        assert!(app.overlay.is_none());
        assert!(
            toast_message(&app)
                .is_some_and(|message| message.contains("switched workspace to client-work"))
        );

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn clear_filters_shortcut_resets_default_view() {
        let mut app = test_app().await;
        app.store.view_state.view = TaskView::Todo;

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Todo);
        assert_eq!(toast_message(&app).as_deref(), Some("filters cleared"));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
    }

    #[tokio::test]
    async fn go_conflicts_shortcut_sets_conflicts_view() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
    }

    #[tokio::test]
    async fn header_click_opens_scope_menu_and_selects_scope() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let selected = app
            .store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.apply_filter_selection(selected);

        let (scope_column, menu_column) = (0..140)
            .find_map(|column| {
                match crate::tui::ui::header_target_at(
                    &app.store,
                    None,
                    ratatui::layout::Rect::new(0, 0, 140, 2),
                    column,
                    0,
                ) {
                    Some(crate::tui::ui::HeaderTarget::Scope {
                        column: menu_column,
                    }) => Some((column, menu_column)),
                    _ => None,
                }
            })
            .unwrap();
        app.dispatch_mouse(header_click(scope_column), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.column == menu_column
                    && state.row == 0
                    && state.items.iter().any(|item| item.label == "workspace")
                    && state.items.iter().any(|item| item.label.contains("Mobile App"))
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: menu_column.saturating_add(2),
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();
        assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
        assert_eq!(toast_message(&app).as_deref(), Some("scope updated"));
    }

    #[tokio::test]
    async fn header_click_opens_view_menu_and_selects_view() {
        let mut app = test_app().await;

        let (view_column, menu_column) = (0..140)
            .find_map(|column| {
                match crate::tui::ui::header_target_at(
                    &app.store,
                    None,
                    ratatui::layout::Rect::new(0, 0, 140, 2),
                    column,
                    0,
                ) {
                    Some(crate::tui::ui::HeaderTarget::View {
                        column: menu_column,
                    }) => Some((column, menu_column)),
                    _ => None,
                }
            })
            .unwrap();
        app.dispatch_mouse(header_click(view_column), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.column == menu_column
                    && state.row == 0
                    && state.items.iter().any(|item| item.label == "inbox")
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: menu_column.saturating_add(2),
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Inbox);
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("view updated"));
    }

    #[tokio::test]
    async fn header_click_opens_workspace_menu_and_switches_workspace() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        let (workspace_column, menu_column) = (0..140)
            .find_map(|column| {
                match crate::tui::ui::header_target_at(
                    &app.store,
                    None,
                    ratatui::layout::Rect::new(0, 0, 140, 2),
                    column,
                    0,
                ) {
                    Some(crate::tui::ui::HeaderTarget::Workspace {
                        column: menu_column,
                    }) => Some((column, menu_column)),
                    _ => None,
                }
            })
            .unwrap();
        app.dispatch_mouse(header_click(workspace_column), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.column == menu_column
                    && state.row == 0
                    && state.items.iter().any(|item| item.label.contains("Client Work"))
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: menu_column.saturating_add(2),
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();
        assert_eq!(app.store.active_workspace.key, "client-work");
        assert!(app.overlay.is_none());
        assert!(
            toast_message(&app)
                .is_some_and(|message| message.contains("switched workspace to client-work"))
        );

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn header_metric_click_still_selects_view_directly() {
        let mut app = test_app().await;

        let queue_column = (0..140)
            .find(|column| {
                matches!(
                    crate::tui::ui::header_target_at(
                        &app.store,
                        None,
                        ratatui::layout::Rect::new(0, 0, 140, 2),
                        *column,
                        0,
                    ),
                    Some(crate::tui::ui::HeaderTarget::MetricView(TaskView::Queue))
                )
            })
            .unwrap();
        app.dispatch_mouse(header_click(queue_column), (140, 24).into())
            .await
            .unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Queue);
        assert_eq!(toast_message(&app).as_deref(), Some("view updated"));

        let mut app = test_app().await;
        let inbox_column = (0..140)
            .find(|column| {
                matches!(
                    crate::tui::ui::header_target_at(
                        &app.store,
                        None,
                        ratatui::layout::Rect::new(0, 0, 140, 2),
                        *column,
                        0,
                    ),
                    Some(crate::tui::ui::HeaderTarget::MetricView(TaskView::Inbox))
                )
            })
            .unwrap();
        app.dispatch_mouse(header_click(inbox_column), (140, 24).into())
            .await
            .unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Inbox);
    }

    #[tokio::test]
    async fn header_click_opens_sync_status() {
        let mut app = test_app().await;

        app.dispatch_mouse(header_click(135), (140, 24).into())
            .await
            .unwrap();

        let Some(OverlayState::SyncStatus(status)) = app.overlay else {
            panic!("expected sync status");
        };
        assert_eq!(*status, app.store.sync_status);
    }

    #[tokio::test]
    async fn header_click_opens_order_menu_and_selects_order() {
        let mut app = test_app().await;

        let (order_column, menu_column) = (0..140)
            .find_map(|column| {
                match crate::tui::ui::header_target_at(
                    &app.store,
                    None,
                    ratatui::layout::Rect::new(0, 0, 140, 2),
                    column,
                    0,
                ) {
                    Some(crate::tui::ui::HeaderTarget::Order {
                        column: menu_column,
                    }) => Some((column, menu_column)),
                    _ => None,
                }
            })
            .unwrap();
        app.dispatch_mouse(header_click(order_column), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::OrderMenu(state))
                if state.column == menu_column
                    && state.row == 0
                    && state.selected == TaskOrder::Created
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: order_column,
                row: 6,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Open);
        assert_eq!(app.store.view_state.order, TaskOrder::Project);
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("order project asc"));
    }

    #[tokio::test]
    async fn header_click_ignores_capturing_overlay() {
        let mut app = test_app().await;
        app.begin_search();

        app.dispatch_mouse(header_click(45), (140, 24).into())
            .await
            .unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Queue);
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    }

    #[tokio::test]
    async fn header_home_click_closes_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(3);

        app.dispatch_mouse(header_click(2), (140, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.detail.is_active());
        assert_eq!(app.list.selected_task(), Some(0));
    }

    #[tokio::test]
    async fn header_home_click_closes_detail_underlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.detail = crate::tui::detail_session::DetailSession::open(0);
        app.overlay = Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::FilterLabel,
            title: "Pick".to_string(),
            filter: LineEdit::blank(),
            items: vec![PickerItem {
                label: "One".to_string(),
                value: "one".to_string(),
                selected: false,
            }],
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        }));

        app.dispatch_mouse(header_click(2), (140, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.detail.is_active());
        assert_eq!(app.list.selected_task(), Some(0));
    }

    #[tokio::test]
    async fn mouse_movement_without_hover_state_skips_redraw() {
        let mut app = test_app().await;

        let changed = app
            .dispatch_mouse(
                MouseEvent {
                    kind: MouseEventKind::Moved,
                    column: 10,
                    row: 6,
                    modifiers: KeyModifiers::NONE,
                },
                (80, 24).into(),
            )
            .await
            .unwrap();

        assert!(!changed);
    }

    #[tokio::test]
    async fn mouse_wheel_moves_task_selection_down_and_up() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("first")).await;
        create_and_select_task(&mut app, test_task_draft("second")).await;
        app.list.select_task(Some(0));

        let changed = app
            .dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert!(changed);
        assert_eq!(app.list.selected_task(), Some(1));

        let changed = app
            .dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert!(changed);
        assert_eq!(app.list.selected_task(), Some(0));
    }

    #[tokio::test]
    async fn mouse_wheel_stops_at_task_list_edges() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("first")).await;
        create_and_select_task(&mut app, test_task_draft("second")).await;
        app.list.select_task(Some(0));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.list.selected_task(), Some(0));

        app.list.select_task(Some(1));
        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.list.selected_task(), Some(1));
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_with_overlay() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;
        app.begin_search();
        let selected = app.list.selected_task();

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.list.selected_task(), selected);
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_in_sidebar_focus() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;
        app.list.focus_sidebar();

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        assert_eq!(app.list.focus(), Focus::Sidebar);
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_with_detail_underlay() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;
        app.detail = crate::tui::detail_session::DetailSession::open(0);

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_for_small_terminal() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (69, 24).into())
            .await
            .unwrap();
        assert_eq!(app.list.selected_task(), Some(0));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 17).into())
            .await
            .unwrap();
        assert_eq!(app.list.selected_task(), Some(0));
    }

    #[tokio::test]
    async fn picker_row_click_submits_clicked_row() {
        let mut app = test_app().await;
        let size = (100, 24).into();
        app.begin_filter_priority();

        app.dispatch_mouse(picker_row_click(&app, 2, size), size)
            .await
            .unwrap();

        assert_eq!(
            app.store.view_state.filter_modifiers.priority.as_deref(),
            Some("medium")
        );
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn picker_row_click_toggles_multi_select_row() {
        let mut app = test_app().await;
        let size = (100, 24).into();
        app.store.create_label("bug".to_string()).await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                labels: vec!["bug".to_string()],
                ..test_task_draft("Labeled target")
            },
        )
        .await;
        app.begin_edit_labels();

        app.dispatch_mouse(picker_row_click(&app, 0, size), size)
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TagCombobox(state))
                if state.options.iter().any(|item| item == "bug")
                    && state.selected.iter().any(|item| item == "bug")
        ));
    }

    #[tokio::test]
    async fn text_panel_mouse_scrolls_and_closes_outside() {
        let mut app = test_app().await;
        let size = (100, 24).into();
        app.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            "Panel",
            (0..20).map(|index| format!("line {index}")).collect(),
        )));

        app.dispatch_mouse(wheel_down(50, 12), size).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(TextPanelState { scroll: 1, .. }))
        ));

        app.dispatch_mouse(left_click(0, 0), size).await.unwrap();

        assert!(app.overlay.is_none());
    }
}

mod task_row_mouse {
    use super::*;
    use ratatui::layout::Rect;

    fn row_column_task_click_event(size: (u16, u16), viewport_row: u16) -> MouseEvent {
        let task_area = task_list_area(size);
        task_row_click(task_area.x + 1, task_area.y + 1 + viewport_row)
    }

    fn status_right_click_event(app: &App, size: (u16, u16), task_index: usize) -> MouseEvent {
        let task_area = task_list_area(size);
        let table = app.list.table_state();
        for row in task_area.y..task_area.y.saturating_add(task_area.height) {
            for column in task_area.x..task_area.x.saturating_add(task_area.width) {
                if crate::tui::ui::task_status_at_position(
                    &app.store, table, task_area, column, row,
                )
                .is_some_and(|hit| hit.task_index == task_index)
                {
                    return right_click(column, row);
                }
            }
        }
        panic!("expected status hit target");
    }

    #[tokio::test]
    async fn task_row_click_selects_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;

        let click = row_column_task_click_event((80, 24), 1);
        app.dispatch_mouse(click, (80, 24).into()).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        assert_eq!(app.list.focus(), Focus::Tasks);
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn status_right_click_opens_status_menu_for_clicked_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.list.select_task(Some(1));

        let click = status_right_click_event(&app, (140, 24), 0);
        app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        assert_eq!(app.list.focus(), Focus::Tasks);
        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn recurrence_context_menu_matches_lifecycle() {
        let mut app = test_app().await;
        let (task_id, series_id) = add_recurring_series(&mut app, "Context series").await;
        app.list
            .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
        let size = (140, 24);

        let click = recurrence_right_click_event(&app, size, 0);
        app.dispatch_mouse(click, size.into()).await.unwrap();
        let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
            panic!("expected recurrence context menu");
        };
        assert!(matches!(
            picker.intent,
            PickerIntent::RecurrenceActions { .. }
        ));
        let values = picker
            .items
            .iter()
            .map(|item| item.value.as_str())
            .collect::<Vec<_>>();
        assert!(values.contains(&"pause"));
        assert!(values.contains(&"stop"));
        assert!(!values.contains(&"resume"));

        app.cancel_overlay();
        app.store
            .pause_recurrence(&series_id, Some(&task_id))
            .await
            .unwrap();
        let click = recurrence_right_click_event(&app, size, 0);
        app.dispatch_mouse(click, size.into()).await.unwrap();
        let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
            panic!("expected paused recurrence context menu");
        };
        let values = picker
            .items
            .iter()
            .map(|item| item.value.as_str())
            .collect::<Vec<_>>();
        assert!(values.contains(&"resume"));
        assert!(values.contains(&"stop"));
        assert!(!values.contains(&"pause"));

        app.cancel_overlay();
        let mut app = test_app().await;
        add_recurring_series(&mut app, "Stopped context").await;
        app.list
            .select_task(app.store.show_view(TaskView::Recurring).await.unwrap());
        app.execute_selected_recurrence_action(Action::StopRecurrence)
            .await
            .unwrap();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .await
            .unwrap();
        let click = recurrence_right_click_event(&app, size, 0);
        app.dispatch_mouse(click, size.into()).await.unwrap();
        let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
            panic!("expected stopped recurrence context menu");
        };
        let values = picker
            .items
            .iter()
            .map(|item| item.value.as_str())
            .collect::<Vec<_>>();
        assert!(!values.contains(&"pause"));
        assert!(!values.contains(&"resume"));
        assert!(!values.contains(&"stop"));
        assert!(values.contains(&"history"));
        assert!(values.contains(&"edit-template"));
    }

    #[tokio::test]
    async fn status_right_click_ignores_non_status_columns() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.list.select_task(Some(1));

        let click = row_column_task_click_event((140, 24), 1);
        app.dispatch_mouse(right_click(click.column, click.row), (140, 24).into())
            .await
            .unwrap();

        assert_eq!(app.list.selected_task(), Some(1));
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn status_right_click_reuses_status_update_and_undo() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;

        let size = (140, 24).into();
        let click = status_right_click_event(&app, (140, 24), 0);
        app.dispatch_mouse(click, size).await.unwrap();
        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
        app.dispatch_key(key(KeyCode::Char('a')), size)
            .await
            .unwrap();

        assert_eq!(app.store.tasks[0].task.status, TaskStatus::Active);
        assert!(toast_message(&app).is_some_and(|message| message.ends_with("status=active")));

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        assert_eq!(app.store.tasks[0].task.status, TaskStatus::Inbox);
        assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
    }

    #[tokio::test]
    async fn task_row_click_opens_detail_on_double_click() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;

        let click = row_column_task_click_event((80, 24), 1);
        app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
        assert_eq!(app.list.selected_task(), Some(0));
        assert!(app.overlay.is_none());
        assert!(app.list.has_task_click());

        app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
        assert!(!app.list.has_task_click());
        assert_eq!(app.list.selected_task(), Some(0));
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn task_row_click_wide_layout_respects_sidebar_offset() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.list.select_task(Some(1));
        app.list.focus_tasks();

        let sidebar = crate::tui::ui::sidebar_layout(Rect::new(0, 0, 140, 24), Focus::Tasks)
            .unwrap()
            .sidebar;
        let sidebar_click = task_row_click(
            sidebar.x.saturating_add(sidebar.width).saturating_sub(1),
            sidebar.y + 2,
        );
        app.dispatch_mouse(sidebar_click, (140, 24).into())
            .await
            .unwrap();
        assert_eq!(app.list.selected_task(), Some(1));
        assert_eq!(app.list.focus(), Focus::Tasks);

        let click = row_column_task_click_event((140, 24), 1);
        app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        assert_eq!(app.list.focus(), Focus::Tasks);
    }

    #[tokio::test]
    async fn task_row_click_preview_area_miss_is_ignored() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.list.select_task(Some(1));

        let task_area = task_list_area((140, 40));
        let preview_row = task_area.y + task_area.height.saturating_sub(3);
        let click = task_row_click(task_area.x + 1, preview_row);
        app.dispatch_mouse(click, (140, 40).into()).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(1));
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn task_row_click_stale_state_is_reset_after_non_task_hit() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;

        let row_click = row_column_task_click_event((80, 24), 1);
        app.dispatch_mouse(row_click, (80, 24).into())
            .await
            .unwrap();
        app.dispatch_mouse(task_row_click(10, 23), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_mouse(row_click, (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn task_row_click_ignores_narrow_sidebar_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        app.list.focus_sidebar();

        let overlay = crate::tui::ui::sidebar_layout(Rect::new(0, 0, 80, 40), Focus::Sidebar)
            .expect("sidebar overlay should exist in narrow layout")
            .sidebar;
        let click = task_row_click(overlay.x + 1, overlay.y + 1);
        app.dispatch_mouse(click, (80, 40).into()).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        assert!(app.overlay.is_none());
        assert_eq!(app.list.focus(), Focus::Sidebar);
    }
}

mod authoring {
    use super::*;

    #[tokio::test]
    async fn add_task_shortcut_opens_title_prompt() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Title && state.title.as_str().is_empty()
        ));
    }

    #[tokio::test]
    async fn add_task_alias_creates_task_after_title() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.list.selected_task().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.status, TaskStatus::Inbox);
        assert_eq!(task.task.priority, TaskPriority::None);
        assert_eq!(task.task.description, "");
        assert!(task.labels.is_empty());
    }

    #[tokio::test]
    async fn add_task_every_four_weeks_uses_local_zone_and_recurring_default() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let (expected_zone, expected_start) = {
            let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
                panic!("expected composer");
            };
            state.title = LineEdit::new("Four week planning".to_string());
            state.set_repeat_rule("every 4 weeks on Monday and Thursday".to_string());
            (state.time_zone.clone(), state.repeat_start_on.text.clone())
        };
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let row = sqlx::query_as::<_, (String, i64, String, String, String)>(
            "SELECT initial_status, interval, weekdays, timezone, start_on FROM recurrence_series WHERE title = ?",
        )
        .bind("Four week planning")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "todo");
        assert_eq!(row.1, 4);
        assert_eq!(row.2, "mon,thu");
        assert_eq!(row.3, expected_zone);
        assert_eq!(row.4, expected_start);
        let message = toast_message(&app).unwrap();
        assert!(message.starts_with("Created recurring task RCR-"));
        assert!(!message.contains(&expected_zone));
        assert!(!message.contains('('));
    }

    #[tokio::test]
    async fn add_task_monthly_previews_and_persists_the_rule() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.title = LineEdit::new("Monthly planning".to_string());
        state.set_repeat_rule("monthly".to_string());
        state.refresh_recurrence_preview();
        assert_eq!(state.recurrence_preview.len(), 3);

        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let row = sqlx::query_as::<_, (String, i64, String)>(
            "SELECT frequency, interval, weekdays FROM recurrence_series WHERE title = ?",
        )
        .bind("Monthly planning")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, ("monthly".to_string(), 1, String::new()));
    }

    #[tokio::test]
    async fn add_task_invalid_repeat_time_focuses_available_time() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.title = LineEdit::new("Daily planning".to_string());
        state.set_repeat_rule("daily".to_string());
        state.repeat_at = LineEdit::new("daily".to_string());

        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::RepeatAt
                    && state.recurrence_error.as_deref().is_some_and(|error| error.contains("invalid-repeat-at"))
        ));
    }

    #[tokio::test]
    async fn add_task_recurring_preserves_each_explicit_open_status() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        for status in ["inbox", "backlog", "todo", "active"] {
            app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
            let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
                panic!("expected composer");
            };
            state.title = LineEdit::new(format!("Explicit {status}"));
            state.status = status.to_string();
            state.status_origin = crate::tui::authoring::InitialStatusOrigin::Explicit;
            state.set_repeat_rule("daily".to_string());
            app.handle_overlay_key(ctrl_s()).await.unwrap();

            let stored: String =
                sqlx::query_scalar("SELECT initial_status FROM recurrence_series WHERE title = ?")
                    .bind(format!("Explicit {status}"))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(stored, status);
        }
    }

    #[tokio::test]
    async fn add_task_recurring_terminal_status_persists_nothing() {
        for status in ["done", "canceled"] {
            let (_dir, pool, mut app) = test_app_with_pool().await;
            let labels_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM labels")
                .fetch_one(&pool)
                .await
                .unwrap();
            app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
            let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
                panic!("expected composer");
            };
            state.title = LineEdit::new(format!("Terminal {status}"));
            state.status = status.to_string();
            state.status_origin = crate::tui::authoring::InitialStatusOrigin::Explicit;
            state.labels = vec![format!("unpersisted-{status}")];
            state.set_repeat_rule("daily".to_string());
            app.handle_overlay_key(ctrl_s()).await.unwrap();

            assert!(matches!(
                &app.overlay,
                Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Status
            ));
            assert_eq!(
                toast_message(&app).as_deref(),
                Some(
                    "recurring tasks require an open initial status: inbox, backlog, todo, or active"
                )
            );
            let labels_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM labels")
                .fetch_one(&pool)
                .await
                .unwrap();
            let series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recurrence_series")
                .fetch_one(&pool)
                .await
                .unwrap();
            let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(labels_after, labels_before);
            assert_eq!(series, 0);
            assert_eq!(tasks, 0);
        }
    }

    #[tokio::test]
    async fn recurring_creation_without_inferred_project_preserves_the_draft() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.title = LineEdit::new("Keep this recurring draft".to_string());
        state.selected_project = None;
        state.inferred_project = None;
        state.project = "no project".to_string();
        state.set_repeat_rule("daily".to_string());

        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
            panic!("expected preserved composer");
        };
        assert_eq!(state.focus, AddTaskStep::Project);
        assert_eq!(state.title.text, "Keep this recurring draft");
        assert_eq!(state.repeat_rule.text, "daily");
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("Choose a project for this recurring task")
        );
        let series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recurrence_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series, 0);
        assert_eq!(tasks, 0);
    }

    #[tokio::test]
    async fn add_task_template_preserves_frozen_schedule_identity() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.title = LineEdit::new("Frozen schedule".to_string());
        state.set_repeat_rule("every 4 weeks on Monday and Thursday".to_string());
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let before = sqlx::query_as::<_, (String, i64, String, String, String)>(
            "SELECT id, interval, weekdays, timezone, start_on FROM recurrence_series WHERE title = ?",
        )
        .bind("Frozen schedule")
        .fetch_one(&pool)
        .await
        .unwrap();
        let database = aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap();
        let series_id = before
            .0
            .parse::<aven_core::recurrence::RecurrenceSeriesId>()
            .unwrap();
        let detail = database
            .recurrence_series_detail(&app.store.active_workspace.id, &series_id)
            .await
            .unwrap();
        app.authoring
            .begin_edit_recurrence_template(&detail, String::new());
        app.begin_add_task_step();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected template composer");
        };
        assert!(state.schedule_expanded);
        state.title = LineEdit::new("Updated template".to_string());
        state.selected_project = None;
        state.repeat_rule = LineEdit::new("daily".to_string());
        state.time_zone = "UTC".to_string();
        state.repeat_start_on = LineEdit::new("2030-01-01".to_string());
        state.repeat_at = LineEdit::new("09:00".to_string());
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let after = sqlx::query_as::<_, (String, i64, String, String, String, String)>(
            "SELECT id, interval, weekdays, timezone, start_on, available_local_time FROM recurrence_series WHERE title = ?",
        )
        .bind("Updated template")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (&after.0, after.1, &after.2, &after.3, &after.4),
            (&before.0, before.1, &before.2, &before.3, &before.4)
        );
        assert_eq!(after.5, "09:00:00");
    }

    #[tokio::test]
    async fn add_task_status_hotkey_selects_direct_status() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;

        app.handle_overlay_key(ctrl_t()).await.unwrap();

        assert_pending(&app, &["t"]);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.status == "inbox"
        ));
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::AddTask(state)) if state.status_prefix_active
        ));
        assert_eq!(toast_message(&app), None);

        app.handle_overlay_key(key(KeyCode::Char('a')))
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.status == "active"
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("add task status=active")
        );

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Write docs");
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Active);
    }

    #[tokio::test]
    async fn add_task_priority_hotkey_selects_direct_priority() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Fix release").await;

        app.handle_overlay_key(ctrl_r()).await.unwrap();

        assert_pending(&app, &["r"]);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.priority == "none"
        ));
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::AddTask(state)) if state.priority_prefix_active
        ));
        assert_eq!(toast_message(&app), None);

        app.handle_overlay_key(key(KeyCode::Char('h')))
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.priority == "high"
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("add task priority=high")
        );

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Fix release");
        assert_eq!(app.store.tasks[selected].task.priority, TaskPriority::High);
    }

    #[tokio::test]
    async fn add_task_labels_picker_sets_created_task_labels() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;

        app.handle_overlay_key(ctrl_l()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Labels(labels)
                        if labels.intent == TagComboboxIntent::AddTaskLabels
                )
        ));
        type_chars(&mut app, "feature").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Labels(labels)
                        if labels.input.as_str().is_empty()
                            && labels.selected == vec!["feature".to_string()]
                )
        ));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.title.as_str() == "Write docs"
                    && state.labels == vec!["feature".to_string()]
        ));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.list.selected_task().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.labels, vec!["feature".to_string()]);
    }

    #[tokio::test]
    async fn add_task_labels_escape_returns_to_add_task_only_dialog() {
        let mut app = test_app().await;
        app.intake.enter_add_task_only(AppConfig::default());
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(ctrl_l()).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(!app.should_quit);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.title.as_str() == "Write docs"
        ));
    }

    #[tokio::test]
    async fn direct_task_start_opens_selected_detail_without_history() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("first task")).await;
        let target = create_and_select_task(&mut app, test_task_draft("target task")).await;
        let task_id = app.store.tasks[target].task.id.clone();
        drop(app);
        sqlx::query("UPDATE tasks SET status = 'done', deleted = 1 WHERE id = ?")
            .bind(&task_id)
            .execute(&pool)
            .await
            .unwrap();

        let view_state = TaskViewState {
            view: TaskView::Search,
            filter_modifiers: crate::tui::store::TaskFilterModifiers {
                task_ids: vec![task_id.clone()],
                ..crate::tui::store::TaskFilterModifiers::default()
            },
            ..TaskViewState::default()
        };
        let database = aven_core::db::Database::open(&_dir.path().join("test.db"))
            .await
            .unwrap();
        let mut app = App::new_with_view_state(
            database,
            crate::workspaces::Workspace::default(),
            view_state,
        )
        .await
        .unwrap();

        app.open_task_on_start(&task_id).unwrap();

        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.list.selected_task(), Some(0));
        assert_eq!(app.store.tasks[0].task.id, task_id);
        assert!(app.store.tasks[0].task.deleted);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.list.pop_navigation().is_none());
        assert!(app.detail.state_mut().unwrap().history.pop().is_none());

        app.handle_normal_key(KeyCode::Esc).await.unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.store.view_state.view, TaskView::Search);
        assert_eq!(app.store.tasks.len(), 1);
    }

    #[tokio::test]
    async fn direct_task_start_rejects_a_missing_loaded_target() {
        let mut app = test_app().await;
        let missing: crate::ids::TaskId = "ABCD000000000000".parse().unwrap();

        let error = app.open_task_on_start(&missing).unwrap_err();

        assert_eq!(
            error.to_string(),
            "task target disappeared before the TUI loaded"
        );
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn recent_actions_start_selects_the_first_action() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("recorded task")).await;
        drop(app);
        let view_state = TaskViewState {
            view: TaskView::RecentActions,
            ..TaskViewState::default()
        };

        let database = aven_core::db::Database::open(&_dir.path().join("test.db"))
            .await
            .unwrap();
        let app = App::new_with_view_state(
            database,
            crate::workspaces::Workspace::default(),
            view_state,
        )
        .await
        .unwrap();

        assert!(!app.store.recent_actions.is_empty());
        assert!(app.store.tasks.is_empty());
        assert_eq!(app.list.selected_task(), Some(0));
    }

    #[tokio::test]
    async fn add_task_start_view_keeps_main_surface() {
        let mut app = test_app().await;
        app.open_add_task_on_start(false).await.unwrap();

        let view = app.view();

        assert_eq!(view.surface, ViewSurface::Main);
        assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
    }

    #[tokio::test]
    async fn add_task_only_view_uses_popup_surface() {
        let mut app = test_app().await;
        app.intake.enter_add_task_only(AppConfig::default());
        app.begin_add_task().await.unwrap();

        let view = app.view();

        assert_eq!(view.surface, ViewSurface::AddTask);
    }

    #[tokio::test]
    async fn add_task_only_render_skips_normal_tui() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Existing queue task")).await;
        app.intake.enter_add_task_only(AppConfig::default());
        app.begin_add_task().await.unwrap();

        let rendered = render_app_text(&mut app, 50, 12);

        assert!(rendered.contains("Add task"));
        assert!(rendered.contains("Enter title here"));
        assert!(!rendered.contains("terminal too small for aven tui"));
        assert!(!rendered.contains("Existing queue task"));
    }

    #[tokio::test]
    async fn add_task_only_natural_render_uses_popup_surface() {
        let mut app = test_app().await;
        app.intake.enter_add_task_only(AppConfig::default());
        app.begin_add_task().await.unwrap();
        app.begin_add_task_natural();

        let rendered = render_app_text(&mut app, 50, 12);

        assert!(rendered.contains("Add task: natural language"));
        assert!(rendered.contains("Describe the task in natural language"));
        assert!(rendered.contains("Ctrl-Enter / ^S parse"));
        assert!(!rendered.contains("terminal too small for aven tui"));
    }

    #[tokio::test]
    async fn add_task_uses_active_project_view() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let selected = app
            .store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.apply_filter_selection(selected);

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(
            app.store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
    }

    #[tokio::test]
    async fn add_task_flow_configures_project_and_priority_from_title() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(ctrl_p()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Picker { state: picker, .. }
                        if picker.intent == PickerIntent::AddTaskProject
                )
        ));
        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        app.handle_overlay_key(ctrl_r()).await.unwrap();

        assert_pending(&app, &["r"]);
        app.handle_overlay_key(key(KeyCode::Char('h')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.list.selected_task().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(task.task.priority, TaskPriority::High);
        assert_eq!(task.task.description, "");
        assert!(task.labels.is_empty());
    }

    #[tokio::test]
    async fn add_task_tab_opens_description_step() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.title.as_str() == "Write docs"
        ));
    }

    #[tokio::test]
    async fn add_task_picker_escape_returns_to_add_task_only_dialog() {
        let mut app = test_app().await;
        app.intake.enter_add_task_only(AppConfig::default());
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(ctrl_p()).await.unwrap();

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(!app.should_quit);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.title.as_str() == "Write docs"
        ));
    }

    #[tokio::test]
    async fn add_task_description_flow_creates_task_with_description() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Include setup details").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.list.selected_task().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.description, "Include setup details");
    }

    #[tokio::test]
    async fn add_task_description_ctrl_x_ctrl_e_opens_external_editor_and_returns_to_composer() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Details").await;
        app.handle_overlay_key(ctrl_x()).await.unwrap();
        app.handle_overlay_key(ctrl_e()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.title.as_str() == "Write docs"
                    && state.description.lines == vec!["Details from editor".to_string()]
        ));
    }

    #[tokio::test]
    async fn add_task_description_ctrl_x_non_editor_key_clears_prefix_and_edits_text() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        app.handle_overlay_key(ctrl_x()).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('z')))
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.description.lines == vec!["z".to_string()]
        ));
    }

    #[tokio::test]
    async fn add_task_description_ctrl_e_moves_to_line_end() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Details").await;
        app.handle_overlay_key(ctrl_e()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.description.column == "Details".len()
                    && state.description.lines == vec!["Details".to_string()]
        ));
    }

    #[tokio::test]
    async fn add_task_project_and_priority_return_to_description_step() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Details").await;
        app.handle_overlay_key(ctrl_p()).await.unwrap();
        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.description.lines == vec!["Details".to_string()]
        ));

        app.handle_overlay_key(ctrl_r()).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('h')))
            .await
            .unwrap();
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(task.task.priority, TaskPriority::High);
        assert_eq!(task.task.description, "Details");
    }

    #[tokio::test]
    async fn refresh_preserves_recent_action_selection() {
        let mut app = test_app().await;
        app.store
            .create_task(test_task_draft("first task"), None)
            .await
            .unwrap();
        app.store
            .create_task(test_task_draft("second task"), None)
            .await
            .unwrap();
        app.store.view_state.view = TaskView::RecentActions;
        app.refresh().await.unwrap();
        app.list.select_task(Some(1));
        let selected_change_id = app.store.recent_actions[1].change_id.clone();

        app.refresh().await.unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(
            app.store.recent_actions[selected].change_id,
            selected_change_id
        );
    }

    #[tokio::test]
    async fn recent_actions_mouse_click_selects_row() {
        let mut app = test_app().await;
        app.store
            .create_task(test_task_draft("first task"), None)
            .await
            .unwrap();
        app.store
            .create_task(test_task_draft("second task"), None)
            .await
            .unwrap();
        app.store.view_state.view = TaskView::RecentActions;
        app.refresh().await.unwrap();
        let expected_change_id = app.store.recent_actions[1].change_id.clone();

        app.dispatch_mouse(left_click(1, 4), (80, 24).into())
            .await
            .unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.list.focus(), Focus::Tasks);
        assert_eq!(
            app.store.recent_actions[selected].change_id,
            expected_change_id
        );
    }

    #[tokio::test]
    async fn idle_poll_timeout_uses_refresh_deadline() {
        let app = test_app().await;

        let timeout = app.next_poll_timeout();

        assert!(timeout <= std::time::Duration::from_secs(5));
        assert!(timeout > std::time::Duration::from_secs(4));
        assert!(!app.has_time_based_redraw());
    }

    #[tokio::test]
    async fn automatic_refresh_surfaces_task_when_availability_arrives() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let (message, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Scheduled refresh task".to_string(),
                    available_at: Some("2999-11-01T04:00:00Z".to_string()),
                    due_on: None,
                    ..test_task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        assert!(selected.is_none());
        assert!(message.contains("hidden by current filters"));
        assert!(app.store.tasks.is_empty());
        assert_eq!(app.store.counts.upcoming, 1);

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("UPDATE tasks SET available_at = ? WHERE title = ?")
            .bind("2026-03-08T05:00:00Z")
            .bind("Scheduled refresh task")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        app.next_refresh_at = std::time::Instant::now() - std::time::Duration::from_secs(1);

        assert!(app.refresh_if_due().await.unwrap());

        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.title, "Scheduled refresh task");
        assert_eq!(
            app.store.tasks[0].queue.band,
            crate::queue::QueueBand::Available
        );
        assert_eq!(app.store.counts.open, 1);
        assert_eq!(app.store.counts.inbox, 1);
        assert_eq!(app.store.counts.upcoming, 0);
        assert!(!app.refresh_is_due());
    }

    #[tokio::test]
    async fn automatic_refresh_preserves_visible_deleted_task_until_user_refresh() {
        let mut app = test_app().await;
        let (_, selected) = app
            .store
            .create_task(test_task_draft("Deleted task stays visible"), None)
            .await
            .unwrap();
        app.list.select_task(selected);

        app.update_deleted(true).await.unwrap();
        let deleted_id = app.store.tasks[0].task.id.clone();
        assert!(app.store.tasks[0].task.deleted);

        app.next_refresh_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
        assert!(app.refresh_if_due().await.unwrap());

        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.id, deleted_id);
        assert!(app.store.tasks[0].task.deleted);
        assert_eq!(app.list.selected_task(), Some(0));

        app.refresh().await.unwrap();

        assert!(app.store.tasks.is_empty());
        assert_eq!(app.list.selected_task(), None);
    }

    #[tokio::test]
    async fn refresh_attempt_schedules_next_deadline() {
        let mut app = test_app().await;
        app.next_refresh_at = std::time::Instant::now() - std::time::Duration::from_secs(1);

        assert!(app.refresh_is_due());
        app.schedule_next_refresh();

        assert!(!app.refresh_is_due());
        assert!(app.refresh_timeout() <= std::time::Duration::from_secs(5));
        assert!(app.refresh_timeout() > std::time::Duration::from_secs(4));
    }

    #[tokio::test]
    async fn loading_poll_timeout_uses_spinner_cadence() {
        let mut app = test_app().await;
        app.notification = Some(Notification::loading("parsing task with LLM"));

        assert_eq!(
            app.next_poll_timeout(),
            std::time::Duration::from_millis(120)
        );
        assert!(app.has_time_based_redraw());
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
        assert!(toast_message(&app).is_some_and(|message| {
            message.contains("parsing task with LLM") && !message.contains("•")
        }));
    }

    #[tokio::test]
    async fn toast_expiry_clears_message_once() {
        let mut app = test_app().await;
        app.set_success("created task APP-TEST");
        let first_timeout = app.next_poll_timeout();
        app.notification = Some(Notification::Toast {
            toast: crate::tui::toast::Toast::new("created task APP-TEST", ToastSeverity::Success),
            created_at: std::time::Instant::now() - std::time::Duration::from_secs(5),
        });

        assert!(first_timeout <= std::time::Duration::from_secs(4));
        assert!(app.clear_expired_notification());
        assert!(app.notification.is_none());
        assert!(!app.clear_expired_notification());
    }

    #[tokio::test]
    async fn unfinished_task_intake_poll_does_not_request_redraw() {
        let mut app = test_app().await;
        let handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(test_task_intake_result("pending task"))
        });
        app.notification = Some(Notification::loading("adding task with LLM"));
        app.intake.start_handle(
            handle,
            NaturalRetry::AddTask,
            "pending task".to_string(),
            true,
        );

        assert!(!app.poll_pending_task_intake().await.unwrap());
        app.intake.cancel();
    }

    #[tokio::test]
    async fn canceling_authoring_aborts_pending_task_intake() {
        let mut app = test_app().await;
        app.intake.start_handle(
            tokio::spawn(async {
                std::future::pending::<Result<crate::task_intake::TaskIntakeResult>>().await
            }),
            NaturalRetry::Dialog,
            "pending task".to_string(),
            false,
        );

        app.cancel_authoring_overlay();

        assert!(!app.intake.work_pending());
    }

    #[tokio::test]
    async fn finished_task_intake_poll_requests_redraw() {
        let mut app = test_app().await;
        app.authoring.begin_add_task(None, None);
        let handle = tokio::spawn(async { Ok(test_task_intake_result("ready task")) });
        app.notification = Some(Notification::loading("adding task with LLM"));
        app.intake.start_handle(
            handle,
            NaturalRetry::AddTask,
            "ready task".to_string(),
            true,
        );

        for _ in 0..100 {
            if app.poll_pending_task_intake().await.unwrap() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        assert_eq!(app.intake.view().work, IntakeWorkView::Ready);
        assert!(matches!(
            app.notification.as_ref(),
            Some(Notification::Loading { message, .. }) if message == "adding task with LLM"
        ));

        assert!(app.poll_pending_task_intake().await.unwrap());
        assert_eq!(app.intake.view().work, IntakeWorkView::Idle);
        assert!(toast_message(&app).is_some_and(|message| message.starts_with("created task ")));
    }

    #[tokio::test]
    async fn finished_attachment_task_intake_closes_composer() {
        let (dir, _pool, mut app) = test_app_with_pool().await;
        let db_path = dir.path().join("test.db");
        app.set_add_task_db_path(db_path);
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "task with image").await;
        let image_path = dir.path().join("intake.png");
        std::fs::write(&image_path, png_bytes(2, 1)).unwrap();
        assert!(
            app.paste_add_task_image_from_text(image_path.to_str().unwrap())
                .unwrap()
        );
        assert!(app.authoring.add_task_has_pending_attachments());
        app.intake.start_handle(
            tokio::spawn(async {
                Ok(crate::task_intake::TaskIntakeResult {
                    task: test_task_draft("created with image"),
                    recurrence: None,
                })
            }),
            NaturalRetry::AddTask,
            "task with image".to_string(),
            true,
        );

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while app.intake.work_pending() {
                app.poll_pending_task_intake().await.unwrap();
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("task intake should finish");

        assert!(app.overlay.is_none());
        assert!(app.authoring.is_idle());
        let created = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == "created with image")
            .unwrap();
        assert_eq!(created.attachments.len(), 1);
    }

    #[tokio::test]
    async fn add_task_ctrl_n_creates_task_in_background_in_full_tui() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "in slack-agent fix dispatch").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        assert!(!app.intake.work_pending());
        assert!(app.overlay.is_none());
        assert!(app.authoring.is_idle());
        assert!(
            toast_message(&app).is_some_and(|message| { message == "adding task in background" })
        );
    }

    #[tokio::test]
    async fn add_task_ctrl_n_from_description_runs_in_background() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Include setup details").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        assert!(!app.intake.work_pending());
        assert!(app.overlay.is_none());
        assert!(app.authoring.is_idle());
        assert!(
            toast_message(&app).is_some_and(|message| { message == "adding task in background" })
        );
    }

    #[tokio::test]
    async fn add_task_only_ctrl_n_exits_immediately() {
        let mut app = test_app().await;
        app.intake.enter_add_task_only(AppConfig::default());

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "add from popup").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        assert!(app.should_quit);
        assert!(!app.intake.work_pending());
        assert!(app.overlay.is_none());
        assert_eq!(app.intake.view().message, Some("adding task in background"));
    }

    #[tokio::test]
    async fn add_task_only_natural_dialog_submit_exits_immediately() {
        let mut app = test_app().await;
        app.intake.enter_add_task_only(AppConfig::default());

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.begin_add_task_natural();
        type_chars(&mut app, "dialog add from popup").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.should_quit);
        assert!(!app.intake.work_pending());
        assert!(app.overlay.is_none());
        assert_eq!(app.intake.view().message, Some("adding task in background"));
    }

    #[tokio::test]
    async fn add_task_natural_dialog_error_reopens_natural_dialog() {
        let mut app = test_app().await;
        configure_task_intake_failure(&mut app, "parse-title-fail.sh");

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.begin_add_task_natural();
        type_chars(&mut app, "raw natural title").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                app.poll_pending_task_intake().await.unwrap();
                if toast_message(&app).is_some_and(|message| {
                    message.contains("task intake failed") && message.contains("logged to")
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("task intake failure should finish");

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.lines.join("\n") == "raw natural title"
        ));
        assert!(toast_message(&app).is_some_and(|message| {
            message.contains("task intake failed") && message.contains("logged to")
        }));
    }

    fn configure_task_intake_failure(app: &mut App, script_name: &str) {
        let dir = tempfile::tempdir().unwrap().keep();
        let command = dir.join(script_name);
        std::fs::write(&command, "#!/bin/sh\ncat >/dev/null\nexit 1\n").unwrap();
        set_executable(&command);
        let mut config = AppConfig::default();
        config.agent.task_intake.command = Some(command.display().to_string());
        config.agent.task_intake.args = Vec::new();
        config.agent.task_intake.timeout_seconds = Some(5);
        app.set_config(config);
    }

    #[cfg(unix)]
    fn set_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn set_executable(_path: &std::path::Path) {}

    #[tokio::test]
    async fn add_task_flow_cancels_at_title_step() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn add_task_blank_title_is_rejected() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("task title is required")
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Title && state.title_error
        ));
    }

    #[tokio::test]
    async fn add_task_enter_opens_each_metadata_control() {
        for field in [
            AddTaskStep::Project,
            AddTaskStep::Status,
            AddTaskStep::Priority,
            AddTaskStep::Labels,
        ] {
            let mut app = test_app().await;
            app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
            let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
                panic!("expected composer");
            };
            state.focus = field;
            app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
            assert!(matches!(
                &app.overlay,
                Some(OverlayState::AddTask(state))
                    if !matches!(state.mode, crate::tui::overlay::AddTaskMode::Compose)
            ));
        }
    }

    #[tokio::test]
    async fn cancelling_schedule_editor_restores_compact_composer_height() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let dialog_height = |app: &mut App| {
            let buffer = render_app_buffer(app, 120, 30);
            let row = |row: u16| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            };
            let top = (0..buffer.area.height)
                .find(|row_index| row(*row_index).contains("╭─ Add task "))
                .expect("add task top border");
            let bottom = (top..buffer.area.height)
                .rev()
                .find(|row_index| row(*row_index).contains('╰'))
                .expect("add task bottom border");
            bottom - top + 1
        };
        let compact_height = dialog_height(&mut app);

        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.focus = AddTaskStep::Schedule;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.mode == crate::tui::overlay::AddTaskMode::Compose
                    && !state.mode.expands_composer()
        ));
        assert_eq!(dialog_height(&mut app), compact_height);
    }

    #[tokio::test]
    async fn recurrence_due_control_cycles_each_policy() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.set_repeat_rule("daily".to_string());
        state.mode = crate::tui::overlay::AddTaskMode::Schedule(
            state.schedule_editor(crate::tui::overlay::ScheduleEditorField::DuePolicy),
        );

        app.handle_overlay_key(key(KeyCode::Right)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Schedule(editor)
                        if editor.repeat_due == "none"
                )
        ));

        app.handle_overlay_key(key(KeyCode::Left)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Schedule(editor)
                        if editor.repeat_due == "same-day"
                )
        ));
    }

    #[tokio::test]
    async fn add_task_ctrl_a_opens_once_schedule_at_availability() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();

        app.handle_overlay_key(ctrl_a()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Schedule(editor)
                        if editor.mode == crate::tui::overlay::ScheduleEditorMode::Once
                            && editor.focus == crate::tui::overlay::ScheduleEditorField::Available
                )
        ));
    }

    #[tokio::test]
    async fn add_task_ctrl_u_opens_once_schedule_at_due() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();

        app.handle_overlay_key(ctrl_u()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Schedule(editor)
                        if editor.mode == crate::tui::overlay::ScheduleEditorMode::Once
                            && editor.focus == crate::tui::overlay::ScheduleEditorField::Due
                )
        ));
    }

    #[tokio::test]
    async fn add_task_arrow_keys_navigate_visible_fields() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();

        app.handle_overlay_key(key(KeyCode::Up)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Project
        ));
        for expected in [
            AddTaskStep::Status,
            AddTaskStep::Priority,
            AddTaskStep::Labels,
            AddTaskStep::Schedule,
        ] {
            app.handle_overlay_key(key(KeyCode::Right)).await.unwrap();
            assert!(matches!(
                &app.overlay,
                Some(OverlayState::AddTask(state)) if state.focus == expected
            ));
        }
        app.handle_overlay_key(key(KeyCode::Right)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Schedule
        ));
        app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Title
        ));
        app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Description
        ));
        app.handle_overlay_key(key(KeyCode::Up)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Title
        ));
    }

    #[tokio::test]
    async fn add_task_schedule_enter_opens_structured_editor() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.focus = AddTaskStep::Schedule;

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    state.mode,
                    crate::tui::overlay::AddTaskMode::Schedule(ref editor)
                        if editor.mode == crate::tui::overlay::ScheduleEditorMode::Once
                )
        ));
    }

    #[tokio::test]
    async fn add_task_mouse_opens_metadata_and_focuses_text_fields() {
        for (column, row, expected) in [
            (3, 6, AddTaskStep::Project),
            (29, 6, AddTaskStep::Status),
            (55, 6, AddTaskStep::Priority),
            (3, 7, AddTaskStep::Labels),
        ] {
            let mut app = test_app().await;
            app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
            app.dispatch_mouse(task_row_click(column, row), (80, 24).into())
                .await
                .unwrap();
            assert!(
                matches!(
                    &app.overlay,
                    Some(OverlayState::AddTask(state))
                        if state.focus == expected
                            && !matches!(state.mode, crate::tui::overlay::AddTaskMode::Compose)
                ),
                "click at ({column}, {row}) should open {expected:?}"
            );
        }

        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.dispatch_mouse(task_row_click(29, 7), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Schedule
                    && matches!(state.mode, crate::tui::overlay::AddTaskMode::Schedule(_))
        ));
    }

    #[tokio::test]
    async fn add_task_composer_sets_fuzzy_availability() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Test rollout").await;
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.available_at = crate::tui::overlay::LineEdit::new("next monday at 9am".to_string());

        app.handle_overlay_key(ctrl_s()).await.unwrap();
        app.store.show_view(TaskView::Upcoming).await.unwrap();

        assert_eq!(app.store.tasks.len(), 1);
        assert!(app.store.tasks[0].task.available_at.is_some());
    }

    #[tokio::test]
    async fn add_task_composer_preserves_availability_and_due_date() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Ship rollout").await;
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.available_at = crate::tui::overlay::LineEdit::new("in 2 weeks".to_string());
        state.due_on = crate::tui::overlay::LineEdit::new("in 3 weeks".to_string());

        app.handle_overlay_key(ctrl_s()).await.unwrap();
        app.store.show_view(TaskView::Upcoming).await.unwrap();

        assert_eq!(app.store.tasks.len(), 1);
        let task = &app.store.tasks[0].task;
        let available_at = task.available_at.as_deref().unwrap();
        let due_on = task.due_on.as_deref().unwrap();
        assert_eq!(due_on.len(), 10);
        assert!(due_on > &available_at[..10]);
    }

    #[tokio::test]
    async fn add_task_composer_preserves_ambiguous_availability() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Test rollout").await;
        let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
            panic!("expected composer");
        };
        state.available_at = crate::tui::overlay::LineEdit::new("monday".to_string());

        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::AvailableAt
                    && state.available_at.text == "monday"
        ));
        assert!(toast_message(&app).is_some_and(|message| message.contains("use next monday")));
    }

    #[tokio::test]
    async fn add_note_requires_selected_task() {
        let mut app = test_app().await;
        app.list.select_task(None);
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('N')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task for note")
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
    }

    #[tokio::test]
    async fn add_note_alias_requires_selected_task() {
        let mut app = test_app().await;
        app.list.select_task(None);
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task for note")
        );
    }

    #[tokio::test]
    async fn add_note_discard_confirmation_preserves_and_discards_draft() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('N')).await.unwrap();
        type_chars(&mut app, "draft note").await;
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if matches!(state.intent, MultilineIntent::AddNote { .. })
                    && state.mode == MultilineInputMode::ConfirmDiscard
                    && state.lines == ["draft note"]
                    && state.row == 0
                    && state.column == 10
        ));

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.mode == MultilineInputMode::Compose
                    && state.lines == ["draft note"]
                    && state.row == 0
                    && state.column == 10
        ));

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('N')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.mode == MultilineInputMode::Compose
                    && state.lines == [""]
                    && state.row == 0
                    && state.column == 0
        ));
    }

    #[tokio::test]
    async fn add_note_flow_creates_note_for_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('N')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state)) if state.title == ADD_NOTE_TITLE
        ));

        type_chars(&mut app, "Important detail").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(toast_message(&app).is_some_and(|message| message.starts_with("added note ")));
    }
}

mod detail_mode {
    use super::*;

    fn detail_navigation_state(
        app: &App,
        task_id: crate::ids::TaskId,
        scroll: u16,
    ) -> DetailSnapshot {
        DetailSnapshot {
            task_id,
            scroll,
            focused_target: None,
            expanded_sections: std::collections::BTreeSet::new(),
            view_state: app.store.view_state.clone(),
        }
    }

    #[tokio::test]
    async fn q_closes_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.should_quit);
    }

    #[tokio::test]
    async fn back_shortcut_closes_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.detail.is_active());
        assert!(app.pending_shortcut.is_empty());
    }

    #[tokio::test]
    async fn help_key_opens_detail_help_from_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('?')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(app.overlay, Some(OverlayState::DetailHelp { .. })));
        assert_eq!(app.list.focus(), Focus::Tasks);
        assert!(app.list.selected_task().is_some());
    }

    #[tokio::test]
    async fn closing_detail_help_returns_to_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
        app.show_detail(0);
        app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn second_help_key_returns_from_detail_help_to_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
        app.show_detail(0);
        app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('?')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_tab_jumps_to_notes_below_viewport() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("Hidden notes target");
        draft.description = (0..40)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let selected = create_and_select_task(&mut app, draft).await;
        let expected = crate::tui::ui::detail_section_scroll_target(
            &app.store.tasks[selected],
            0,
            80,
            24,
            false,
        );
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(detail_scroll(&app), expected);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(app.list.focus(), Focus::Tasks);

        app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::BackTab), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(detail_scroll(&app), expected);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_tab_remains_unchanged_when_notes_are_visible() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Visible notes target")).await;
        assert_eq!(
            crate::tui::ui::detail_section_scroll_target(
                &app.store.tasks[selected],
                0,
                80,
                24,
                false,
            ),
            0
        );
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(app.list.focus(), Focus::Tasks);
    }

    #[tokio::test]
    async fn detail_scroll_keys_update_detail_offset() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("Scroll target");
        draft.description = (0..100)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        create_and_select_task(&mut app, draft).await;
        app.show_detail(0);

        app.dispatch_key(ctrl_d(), (80, 24).into()).await.unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::PageDown), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(ctrl_u(), (80, 24).into()).await.unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::PageUp), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_scroll_resists_down_input_at_bottom() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Short detail")).await;
        app.show_detail(0);

        for _ in 0..10 {
            app.dispatch_key(key(KeyCode::Char('j')), (80, 24).into())
                .await
                .unwrap();
        }
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn mouse_wheel_scrolls_prefix_hint_overlay() {
        let mut app = test_app().await;
        app.dispatch_key(key(KeyCode::Char('t')), (80, 10).into())
            .await
            .unwrap();

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 10).into())
            .await
            .unwrap();

        assert_eq!(app.pending_shortcut.labels(), vec!["t".to_string()]);
        assert_eq!(app.pending_shortcut_scroll, 1);
    }

    #[tokio::test]
    async fn arrows_scroll_prefix_hint_overlay() {
        let mut app = test_app().await;
        app.dispatch_key(key(KeyCode::Char('t')), (80, 10).into())
            .await
            .unwrap();

        app.dispatch_key(key(KeyCode::Down), (80, 10).into())
            .await
            .unwrap();
        assert_eq!(app.pending_shortcut.labels(), vec!["t".to_string()]);
        assert_eq!(app.pending_shortcut_scroll, 1);

        app.dispatch_key(key(KeyCode::Up), (80, 10).into())
            .await
            .unwrap();
        assert_eq!(app.pending_shortcut.labels(), vec!["t".to_string()]);
        assert_eq!(app.pending_shortcut_scroll, 0);
    }

    #[tokio::test]
    async fn mouse_wheel_scrolls_help_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help { scroll: 0 });

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Help { scroll: 1 })
        ));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Help { scroll: 0 })
        ));
    }

    #[tokio::test]
    async fn mouse_wheel_clamps_help_overlay() {
        let mut app = test_app().await;
        let expected = crate::tui::ui::help_scroll_cap(24);
        app.overlay = Some(OverlayState::Help { scroll: 0 });

        for _ in 0..200 {
            app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
                .await
                .unwrap();
        }

        assert!(matches!(app.overlay, Some(OverlayState::Help { scroll }) if scroll == expected));
    }

    #[tokio::test]
    async fn mouse_wheel_scrolls_detail_help_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
        app.show_detail(0);
        app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 10).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::DetailHelp { scroll: 1 })
        ));
    }

    #[tokio::test]
    async fn stable_frame_mouse_movement_reuses_detail_document() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("Stable detail projection");
        draft.description =
            "**wrapped Markdown** remains stable across pointer movement".to_string();
        create_and_select_task(&mut app, draft).await;
        app.show_detail(0);
        let size: ratatui::layout::Size = (80, 24).into();
        render_app_buffer(&mut app, size.width, size.height);
        let document = std::rc::Rc::clone(
            app.widgets
                .detail_document
                .as_ref()
                .expect("rendered detail document"),
        );
        let projection_id = document.projection_id();

        for (column, row) in [(4, 7), (18, 9)] {
            app.dispatch_mouse(
                MouseEvent {
                    kind: MouseEventKind::Moved,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                size,
            )
            .await
            .unwrap();
            render_app_buffer(&mut app, size.width, size.height);
            let cached = app
                .widgets
                .detail_document
                .as_ref()
                .expect("stable detail document");
            assert_eq!(cached.projection_id(), projection_id);
            assert!(std::rc::Rc::ptr_eq(cached, &document));
        }
    }

    #[tokio::test]
    async fn rendered_disclosure_rebuilds_focus_and_scroll_from_expanded_document() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("Expanded projection");
        draft.description = (0..30)
            .map(|index| format!("description line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let selected = create_and_select_task(&mut app, draft).await;
        let child_ids = (0..6)
            .map(|index| crate::test_support::task_id(&format!("expanded-child-{index}")))
            .collect::<Vec<_>>();
        app.store.tasks[selected].depends_on = child_ids
            .iter()
            .enumerate()
            .map(|(index, task_id)| crate::query::TaskDependencyLink {
                task_id: task_id.clone(),
                display_ref: format!("APP-{index}"),
                title: format!("Child {index}"),
                status: "todo".to_string(),
                priority: "none".to_string(),
                unresolved: true,
            })
            .collect();
        app.show_detail(0);
        app.detail.state_mut().unwrap().set_focused_target(Some(
            crate::tui::app::DetailTargetId::Expand {
                section: crate::tui::app::DetailSection::DependsOn,
            },
        ));
        let size: ratatui::layout::Size = (80, 24).into();
        render_app_buffer(&mut app, size.width, size.height);

        app.dispatch_key(key(KeyCode::Enter), size).await.unwrap();

        let focused = app
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
            .cloned()
            .expect("first revealed dependency focused");
        assert_eq!(focused.task_id(), Some(&child_ids[3]));
        assert!(
            app.detail
                .state()
                .unwrap()
                .expanded_sections()
                .contains(&crate::tui::app::DetailSection::DependsOn)
        );
        let expected_scroll = app
            .detail_document_for_query(size)
            .and_then(|document| document.target_scroll_target(&focused, 0))
            .unwrap();
        assert_eq!(detail_scroll(&app), expected_scroll);
    }

    #[tokio::test]
    async fn detail_drag_selects_rendered_text_and_coexists_with_scrolling() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("Select this title");
        draft.description = (0..40)
            .map(|index| format!("description line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        create_and_select_task(&mut app, draft).await;
        app.show_detail(0);
        let size = (80, 24).into();

        app.dispatch_mouse(left_click(0, 3), size).await.unwrap();
        app.dispatch_mouse(left_drag(7, 3), size).await.unwrap();
        app.dispatch_mouse(left_release(7, 3), size).await.unwrap();

        let selection = app
            .detail
            .state_mut()
            .unwrap()
            .text_selection
            .clone()
            .unwrap();
        let selected_text = {
            let item = app.store.selected_task(app.list.selected_task()).unwrap();
            crate::tui::ui::detail_selected_text(item, &selection)
        };
        assert_eq!(selected_text.as_deref(), Some("Select"));
        assert!(!app.detail.state_mut().unwrap().text_dragging);
        assert!(render_app_text(&mut app, 80, 24).contains("copy selection"));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), size)
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        let item = app.store.selected_task(app.list.selected_task()).unwrap();
        assert_eq!(
            crate::tui::ui::detail_selected_text(item, &selection).as_deref(),
            Some("Select")
        );

        app.dispatch_key(key(KeyCode::Esc), size).await.unwrap();
        assert!(app.detail.state_mut().unwrap().text_selection.is_none());
        assert!(!render_app_text(&mut app, 80, 24).contains("copy selection"));
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_mouse_wheel_updates_detail_offset() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("Mouse detail scroll target");
        draft.description = (0..100)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        create_and_select_task(&mut app, draft).await;
        app.show_detail(0);

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_mouse_wheel_clamps_detail_offset() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("Mouse detail clamp target");
        draft.description = (0..100)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let selected = create_and_select_task(&mut app, draft).await;
        let expected = crate::tui::ui::detail_scroll_cap(&app.store.tasks[selected], 80, 24);
        app.show_detail(0);

        for _ in 0..200 {
            app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
                .await
                .unwrap();
        }

        assert_eq!(detail_scroll(&app), expected);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_mouse_wheel_scrolls_conflict_text_panel() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Conflict mouse scroll target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        for index in 0..20 {
            insert_conflict_for_task_id(
                &pool,
                &mut app,
                &task_id,
                &format!("field-{index}"),
                &format!("local value {index}"),
                &format!("remote value {index}"),
            )
            .await;
        }

        app.show_conflict_details().await.unwrap();
        let expected = match app.overlay.as_ref() {
            Some(OverlayState::TextPanel(panel)) => {
                crate::tui::ui::text_panel_scroll_cap(&panel.lines)
            }
            Some(overlay) => panic!("unexpected overlay for conflict details: {overlay:?}"),
            None => panic!("expected conflict details overlay"),
        };

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(ref panel)) if panel.scroll == 1
        ));

        for _ in 0..200 {
            app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
                .await
                .unwrap();
        }
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(ref panel)) if panel.scroll == expected
        ));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(ref panel)) if panel.scroll == expected.saturating_sub(1)
        ));
    }

    #[tokio::test]
    async fn detail_next_and_previous_task_stay_in_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        let first = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.title == "First")
            .unwrap();
        let second = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.title == "Second")
            .unwrap();
        app.list.select_task(Some(first));
        app.show_detail(7);

        app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.list.selected_task(), Some(second));
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(toast_message(&app).as_deref(), Some("selected next task"));

        app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.list.selected_task(), Some(first));
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("selected previous task")
        );
    }

    #[tokio::test]
    async fn detail_tab_focuses_epic_children_and_j_k_selects_and_opens() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let first_index = create_and_select_task(&mut app, test_task_draft("First child")).await;
        let first_id = app.store.tasks[first_index].task.id.clone();
        let second_index = create_and_select_task(&mut app, test_task_draft("Second child")).await;
        let second_id = app.store.tasks[second_index].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            &first_id,
            &parent_id,
        )
        .await
        .unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            &second_id,
            &parent_id,
        )
        .await
        .unwrap();
        drop(conn);
        app.store.refresh(Some(&parent_id)).await.unwrap();
        let parent_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.list.select_task(Some(parent_index));
        app.show_detail(4);
        let child_ids = app.store.tasks[parent_index]
            .epic_children
            .iter()
            .map(|child| child.task_id.clone())
            .collect::<Vec<_>>();

        app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id)
                .map(crate::ids::TaskId::as_str),
            Some(child_ids[0].as_str())
        );
        assert_eq!(
            app.view()
                .detail_focus
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id)
                .map(crate::ids::TaskId::as_str),
            Some(child_ids[0].as_str())
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('j')), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id)
                .map(crate::ids::TaskId::as_str),
            Some(child_ids[1].as_str())
        );

        app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id)
                .map(crate::ids::TaskId::as_str),
            Some(child_ids[0].as_str())
        );

        app.dispatch_key(key(KeyCode::Char('j')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, child_ids[1]);
        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, parent_id);
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id),
            Some(&child_ids[1])
        );
    }

    #[tokio::test]
    async fn missing_linked_task_keeps_current_detail_open() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Current task")).await;
        let current_id = app.store.tasks[selected].task.id.clone();
        let missing_id = crate::test_support::task_id("missing-linked-task");
        app.store.tasks[selected].depends_on = vec![crate::query::TaskDependencyLink {
            task_id: missing_id,
            display_ref: "APP-MISS".to_string(),
            title: "Unavailable blocker".to_string(),
            status: "todo".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        }];
        app.show_detail(2);

        app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.store.tasks[selected].task.id, current_id);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("linked task is unavailable")
        );
        assert!(app.detail.state_mut().unwrap().history.is_empty());
    }

    #[tokio::test]
    async fn linked_task_opens_outside_active_list_filter() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let child_index = create_and_select_task(&mut app, test_task_draft("Filtered child")).await;
        let child_id = app.store.tasks[child_index].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            &child_id,
            &parent_id,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE tasks SET status = 'todo' WHERE id = ?")
            .bind(&child_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        app.store.view_state.view = TaskView::Inbox;
        app.store.refresh(Some(&parent_id)).await.unwrap();
        assert_eq!(app.store.tasks.len(), 1);
        app.list.select_task(Some(0));
        app.show_detail(3);
        app.detail
            .state_mut()
            .unwrap()
            .expanded_sections
            .insert(crate::tui::app::DetailSection::Blocks);

        app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Search);
        assert_eq!(app.store.tasks[0].task.id, child_id);
        assert!(app.detail.state_mut().unwrap().expanded_sections.is_empty());
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Inbox);
        assert_eq!(app.store.tasks[0].task.id, parent_id);
        assert!(
            app.detail
                .state_mut()
                .unwrap()
                .expanded_sections
                .contains(&crate::tui::app::DetailSection::Blocks)
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id),
            Some(&child_id)
        );
    }

    #[tokio::test]
    async fn unavailable_detail_history_keeps_current_task_open() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Current linked task")).await;
        let current = app.store.tasks[selected].clone();
        let current_id = current.task.id.clone();
        app.store.show_exact_task(current);
        app.list.select_task(Some(0));
        app.show_detail(4);
        let current_view = app.store.view_state.clone();
        let missing_id = crate::test_support::task_id("missing-history-task");
        app.push_detail_navigation_state(crate::tui::detail_session::DetailSnapshot {
            task_id: missing_id.clone(),
            scroll: 2,
            focused_target: None,
            expanded_sections: Default::default(),
            view_state: crate::tui::store::TaskViewState::for_exact_task(missing_id),
        });

        assert!(!app.go_back_in_detail().await.unwrap());
        assert_eq!(app.store.view_state, current_view);
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&current_id)
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_lifecycle_preserves_parent_state_and_reopens_cleanly() {
        let mut app = test_app().await;
        let parent = create_and_select_task(&mut app, test_task_draft("Selectable parent")).await;
        let parent_id = app.store.tasks[parent].task.id.clone();
        let child = create_and_select_task(&mut app, test_task_draft("Linked child")).await;
        let child_id = app.store.tasks[child].task.id.clone();
        let parent = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.list.select_task(Some(parent));
        app.show_detail(3);
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Task {
            section: DetailSection::Blocks,
            task_id: child_id.clone(),
        });
        app.detail
            .state_mut()
            .unwrap()
            .expanded_sections
            .insert(DetailSection::Blocks);
        let size = (80, 24).into();
        app.dispatch_mouse(left_click(0, 3), size).await.unwrap();
        app.dispatch_mouse(left_drag(10, 3), size).await.unwrap();
        app.dispatch_mouse(left_release(10, 3), size).await.unwrap();
        let parent_selection = app
            .detail
            .state_mut()
            .unwrap()
            .text_selection
            .clone()
            .unwrap();
        let parent_text = crate::tui::ui::detail_selected_text(
            app.store.selected_task(Some(parent)).unwrap(),
            &parent_selection,
        )
        .unwrap();

        app.open_detail_task(&child_id, 3).await;

        assert_eq!(app.store.tasks[0].task.id, child_id);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
        assert!(app.detail.state_mut().unwrap().expanded_sections.is_empty());
        assert!(app.detail.state_mut().unwrap().text_selection.is_some());
        assert!(
            crate::tui::ui::detail_selected_text(&app.store.tasks[0], &parent_selection).is_none()
        );
        assert!(app.detail_has_parent());

        app.dispatch_key(key(KeyCode::Char('g')), size)
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('[')), size)
            .await
            .unwrap();

        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&parent_id)
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(DetailTargetId::task_id),
            Some(&child_id)
        );
        assert!(
            app.detail
                .state_mut()
                .unwrap()
                .expanded_sections
                .contains(&DetailSection::Blocks)
        );
        assert_eq!(
            crate::tui::ui::detail_selected_text(
                app.store.selected_task(app.list.selected_task()).unwrap(),
                &parent_selection,
            )
            .as_deref(),
            Some(parent_text.as_str())
        );
        assert!(!app.detail_has_parent());

        app.dispatch_key(key(KeyCode::Char('q')), size)
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_inactive());

        app.activate_or_toggle_detail().await.unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
        assert!(app.detail.state_mut().unwrap().expanded_sections.is_empty());
        assert!(app.detail.state_mut().unwrap().text_selection.is_none());
        assert!(app.detail.state_mut().unwrap().history.is_empty());
    }

    #[tokio::test]
    async fn epic_detail_add_child_shortcut_opens_contextual_search() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let display_ref = app.store.tasks[parent_index].display_ref.clone();
        app.show_detail(3);

        for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
        }

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected add-child search");
        };
        assert!(matches!(
            &state.intent,
            SearchIntent::AddEpicChild {
                epic_id,
                display_ref: purpose_ref,
                ..
            } if epic_id == &parent_id && purpose_ref == &display_ref
        ));
        assert!(state.results[0].create_new);
    }

    #[tokio::test]
    async fn canceling_new_epic_child_authoring_preserves_list_origin() {
        let mut app = test_app().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();

        for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
            app.handle_normal_key(code).await.unwrap();
        }
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
        assert!(matches!(
            app.authoring
                .submission_context()
                .map(|context| context.origin),
            Some(AddTaskOrigin::EpicChild { .. })
        ));

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
        assert!(!app.detail.is_active());

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(!app.detail.is_active());
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&parent_id)
        );
    }

    #[tokio::test]
    async fn submitting_new_epic_child_uses_owned_origin_and_focuses_child() {
        let mut app = test_app().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();

        for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
            app.handle_normal_key(code).await.unwrap();
        }
        type_chars(&mut app, "Owned child").await;
        settle_search_preview(&mut app).await;
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.authoring.is_idle());
        assert!(app.detail.is_active());
        let parent = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == parent_id)
            .unwrap();
        let child = parent
            .epic_children
            .iter()
            .find(|child| child.title == "Owned child")
            .unwrap();
        assert_eq!(
            app.detail
                .state()
                .and_then(|detail| detail.focused_target())
                .and_then(DetailTargetId::task_id),
            Some(&child.task_id)
        );
    }

    #[tokio::test]
    async fn submitting_new_epic_child_from_linked_detail_returns_to_epic() {
        let mut app = test_app().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let parent_ref = app.store.tasks[parent_index].display_ref.clone();
        let project_key = app.store.tasks[parent_index].task.project_key.clone();
        let child_index = create_and_select_task(&mut app, test_task_draft("Existing child")).await;
        let child_id = app.store.tasks[child_index].task.id.clone();
        app.store
            .add_epic_child(
                crate::tui::store::EpicContext {
                    epic_id: parent_id.clone(),
                    display_ref: parent_ref,
                    project_key,
                },
                child_id.clone(),
            )
            .await
            .unwrap();
        let parent_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.list.select_task(Some(parent_index));
        app.show_detail(0);
        app.open_detail_task(&child_id, 0).await;

        for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
        }
        type_chars(&mut app, "Linked detail child").await;
        settle_search_preview(&mut app).await;
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&parent_id)
        );
        assert!(app.detail.is_active());
        assert!(matches!(
            app.detail
                .state()
                .and_then(|detail| detail.focused_target()),
            Some(DetailTargetId::Task {
                section: DetailSection::EpicChildren,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn canceling_new_epic_child_authoring_returns_to_detail_origin() {
        let mut app = test_app().await;
        create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        app.show_detail(3);

        for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
        }
        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
        assert!(matches!(
            app.authoring
                .submission_context()
                .map(|context| context.origin),
            Some(AddTaskOrigin::EpicChild { .. })
        ));

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn epic_detail_add_child_search_hides_other_projects() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent needle")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let parent_project = app.store.tasks[parent_index].task.project_key.clone();
        create_and_select_task(&mut app, test_task_draft("Same project needle")).await;
        create_and_select_task(
            &mut app,
            TaskDraft {
                project: Some("other".to_string()),
                ..test_task_draft("Other project needle")
            },
        )
        .await;
        let parent_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.list.select_task(Some(parent_index));
        app.show_detail(0);

        for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
        }
        type_chars(&mut app, "needle").await;
        settle_search_preview(&mut app).await;

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected add-child search");
        };
        assert!(
            state
                .results
                .iter()
                .filter(|result| !result.create_new)
                .all(|result| result.project_key == parent_project)
        );
        assert!(
            state
                .results
                .iter()
                .any(|result| result.title == "Same project needle")
        );
        assert!(
            state
                .results
                .iter()
                .all(|result| result.title != "Other project needle")
        );
    }

    #[tokio::test]
    async fn right_navigation_toggles_selected_epic() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let child_index = create_and_select_task(&mut app, test_task_draft("Child task")).await;
        let child_id = app.store.tasks[child_index].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            &child_id,
            &parent_id,
        )
        .await
        .unwrap();
        drop(conn);
        app.store.show_view(TaskView::Epics).await.unwrap();

        for code in [KeyCode::Char('l'), KeyCode::Right] {
            let parent_index = app
                .store
                .tasks
                .iter()
                .position(|item| item.task.id == parent_id)
                .unwrap();
            app.list.select_task(Some(parent_index));

            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();

            assert!(app.store.view_state.expanded_epic_ids.contains(&parent_id));
            assert!(app.store.tasks.iter().any(|item| item.task.id == child_id));

            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();

            assert!(app.store.view_state.collapsed_epic_ids.contains(&parent_id));
            assert!(!app.store.tasks.iter().any(|item| item.task.id == child_id));
        }
    }

    #[tokio::test]
    async fn focused_detail_child_removes_and_undo_restores_relationship() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let child_index = create_and_select_task(&mut app, test_task_draft("Child task")).await;
        let child_id = app.store.tasks[child_index].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            &child_id,
            &parent_id,
        )
        .await
        .unwrap();
        drop(conn);
        app.store.view_state.view = crate::tui::store::TaskView::Epics;
        app.store.refresh(Some(&parent_id)).await.unwrap();
        let parent_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.store
            .toggle_selected_epic(Some(parent_index))
            .await
            .unwrap()
            .unwrap();
        app.list.select_task(Some(parent_index));
        app.move_selection(1).await.unwrap();
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&child_id)
        );
        app.list.select_task(Some(parent_index));
        app.show_detail(0);
        app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
            .await
            .unwrap();

        for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('r')] {
            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
        }

        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .removed_epic_child
                .as_ref()
                .map(|removed| &removed.child.task_id),
            Some(&child_id)
        );
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(DetailTargetId::task_id),
            Some(&child_id)
        );
        let linked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM task_epic_links WHERE epic_task_id = ? AND child_task_id = ?",
        )
        .bind(&parent_id)
        .bind(&child_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, 0);

        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&child_id)
        );

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .map(|item| &item.task.id),
            Some(&parent_id)
        );
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(DetailTargetId::task_id),
            Some(&child_id)
        );
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .removed_epic_child
                .as_ref()
                .map(|removed| &removed.child.task_id),
            Some(&child_id)
        );

        app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.detail.state_mut().unwrap().removed_epic_child.is_none());
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(DetailTargetId::task_id),
            Some(&child_id)
        );
        let linked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM task_epic_links WHERE epic_task_id = ? AND child_task_id = ?",
        )
        .bind(&parent_id)
        .bind(&child_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, 1);
    }

    #[tokio::test]
    async fn detail_tab_focuses_locally_available_images_independent_of_preview_state() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let suppressed = test_attachment("SUPPRESSED", "image/png", true, Some((640, 480)));
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext {
                unavailable_hashes: [suppressed.sha256.clone()].into_iter().collect(),
                ..crate::tui::ui::DetailInlineImageContext::default()
            });
        let mut unavailable = test_attachment("UNAVAILABLE", "image/png", true, Some((640, 480)));
        unavailable.bytes_state = crate::attachments::AttachmentBytesState::Unavailable;
        app.store.tasks[selected].attachments = vec![
            test_attachment("PENDING", "image/png", false, Some((640, 480))),
            unavailable,
            suppressed,
            test_attachment("DOCUMENT", "application/pdf", true, Some((640, 480))),
            test_attachment("INVALID", "image/png", true, Some((0, 480))),
            test_attachment("FIRSTIMAGE", "image/png", true, Some((640, 480))),
            test_attachment("LASTIMAGE", "image/jpeg", true, Some((800, 600))),
        ];
        app.show_detail(3);

        app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some("SUPPRESSED")
        );
        assert_eq!(
            app.view()
                .detail_focus
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some("SUPPRESSED")
        );
        let forward_scroll = detail_scroll(&app);
        let context = app.inline_images.context_override().unwrap().clone();
        assert_eq!(
            detail_attachment_hit_id(
                &app.store.tasks[selected],
                100,
                30,
                forward_scroll,
                &context,
            )
            .as_deref(),
            Some("SUPPRESSED")
        );

        app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(shift_key(KeyCode::Tab), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some("LASTIMAGE")
        );
        let reverse_scroll = detail_scroll(&app);
        assert!(reverse_scroll >= forward_scroll);
    }

    fn fail_image_viewer(_path: &std::path::Path) -> anyhow::Result<()> {
        anyhow::bail!("viewer unavailable")
    }

    #[tokio::test]
    async fn external_viewer_notification_keeps_inline_previews_enabled() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
        create_and_select_task(&mut app, test_task_draft("Image task")).await;
        app.show_detail(0);

        app.set_success("opened attachment in default image viewer");

        assert!(
            app.inline_image_context()
                .is_some_and(|context| context.previews_enabled)
        );
    }

    #[tokio::test]
    async fn unsupported_terminal_focus_opens_highlighted_image_externally() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment_id =
            add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext {
                previews_enabled: false,
                ..crate::tui::ui::DetailInlineImageContext::default()
            });
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some(attachment_id.as_str())
        );
        app.dispatch_key(key(KeyCode::Char('o')), (100, 30).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(app.inline_images.export_count(), 1);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("opened attachment in default image viewer")
        );
    }

    #[tokio::test]
    async fn unsupported_terminal_enter_and_mouse_use_external_viewer() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment_id =
            add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
        let context = crate::tui::ui::DetailInlineImageContext {
            previews_enabled: false,
            ..crate::tui::ui::DetailInlineImageContext::default()
        };
        app.inline_images.set_context_override(context.clone());
        app.show_detail(0);
        app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(app.inline_images.export_count(), 1);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        let item = app.store.selected_task(app.list.selected_task()).unwrap();
        let (column, row) = (0..30)
            .flat_map(|row| (0..100).map(move |column| (column, row)))
            .find(|(column, row)| {
                crate::tui::ui::detail_attachment_at_position(
                    item, 100, 30, *column, *row, 0, &context,
                )
                .is_some_and(|hit| hit.attachment_id == attachment_id)
            })
            .expect("attachment label hit target");
        app.dispatch_mouse(left_click(column, row), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(app.inline_images.export_count(), 1);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn full_screen_preview_opens_current_image_externally() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment_id =
            add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
        app.overlay = Some(OverlayState::AttachmentPreview {
            attachment_id: attachment_id.clone(),
            scroll: 3,
        });

        app.dispatch_key(key(KeyCode::Char('o')), (100, 30).into())
            .await
            .unwrap();

        assert_eq!(app.inline_images.export_count(), 1);
        assert!(matches!(
            app.overlay,
            Some(OverlayState::AttachmentPreview {
                attachment_id: ref current,
                scroll: 3,
            }) if current == &attachment_id
        ));
    }

    #[tokio::test]
    async fn external_viewer_reports_unavailable_bytes_and_launch_failures() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment_id =
            add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("UPDATE blob_inventory SET available = 0")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        app.open_attachment_externally(&attachment_id).await;
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("attachment bytes are unavailable")
        );
        assert!(app.inline_images.export_count() == 0);

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("UPDATE blob_inventory SET available = 1")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        app.inline_images
            .set_image_viewer_launcher(fail_image_viewer);
        app.open_attachment_externally(&attachment_id).await;
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("could not start the default image viewer")
        );
        assert!(app.inline_images.export_count() == 0);
        let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(lease_count, 0);

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("UPDATE task_attachments SET deleted = 1, deleted_at = '2026-07-19T00:00:00Z'")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        app.open_attachment_externally(&attachment_id).await;
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("attachment is no longer available")
        );
    }

    #[tokio::test]
    async fn successful_external_export_holds_temp_file_until_app_cleanup() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment_id =
            add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;

        app.open_attachment_externally(&attachment_id).await;

        let export_dir = app.inline_images.export_directory(0).to_path_buf();
        assert!(export_dir.join("attachment.png").is_file());
        let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(lease_count, 0);
        app.inline_images.clear_exports();
        assert!(!export_dir.exists());
    }

    #[tokio::test]
    async fn dropped_external_export_releases_read_lease() {
        let (dir, pool, mut app) = test_app_with_pool().await;
        let db_path = dir.path().join("test.db");
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment_id = add_real_attachment(&mut app, &pool, &db_path, selected).await;
        let blob_dir = crate::config::resolve_blob_dir(&db_path, app.intake.config()).unwrap();
        let export = app
            .store
            .lease_image_export(&blob_dir, &attachment_id)
            .await
            .unwrap();
        let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(lease_count, 1);

        drop(export);
        for _ in 0..20 {
            let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
                .fetch_one(&pool)
                .await
                .unwrap();
            if lease_count == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("dropped image export retained its read lease");
    }

    #[tokio::test]
    async fn detail_keyboard_scroll_includes_framed_preview_rows() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let context = crate::tui::ui::DetailInlineImageContext::default();
        app.inline_images.set_context_override(context.clone());
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        app.store.tasks[selected].attachments = vec![test_attachment(
            "OVERFLOWIMAGE",
            "image/png",
            true,
            Some((640, 480)),
        )];
        app.show_detail(0);
        let text_only_cap = crate::tui::ui::detail_scroll_cap(&app.store.tasks[selected], 80, 26);
        let preview_cap = crate::tui::ui::detail_scroll_cap_with_images(
            &app.store.tasks[selected],
            80,
            26,
            Some(&context),
        );
        assert!(preview_cap > text_only_cap);

        app.dispatch_key(key(KeyCode::PageDown), (80, 26).into())
            .await
            .unwrap();

        assert_eq!(detail_scroll(&app), preview_cap);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_child_focus_order_includes_images_and_enter_opens_preview() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let child_index = create_and_select_task(&mut app, test_task_draft("Child")).await;
        let child_id = app.store.tasks[child_index].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            &child_id,
            &parent_id,
        )
        .await
        .unwrap();
        drop(conn);
        app.store.refresh(Some(&parent_id)).await.unwrap();
        let parent_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.list.select_task(Some(parent_index));
        app.store.tasks[parent_index].attachments = vec![test_attachment(
            "FOCUSIMAGE",
            "image/png",
            true,
            Some((640, 480)),
        )];
        app.show_detail(5);

        app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id),
            Some(&child_id)
        );
        app.dispatch_key(key(KeyCode::Char('j')), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some("FOCUSIMAGE")
        );
        let image_scroll = detail_scroll(&app);
        assert_eq!(
            detail_attachment_hit_id(
                &app.store.tasks[parent_index],
                100,
                30,
                image_scroll,
                app.inline_images.context_override().unwrap(),
            )
            .as_deref(),
            Some("FOCUSIMAGE")
        );
        app.dispatch_key(key(KeyCode::Char('k')), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::task_id),
            Some(&child_id)
        );
        app.dispatch_key(key(KeyCode::Char('j')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::AttachmentPreview {
                ref attachment_id,
                scroll,
            }) if attachment_id == "FOCUSIMAGE" && scroll == image_scroll
        ));
    }

    #[tokio::test]
    async fn full_screen_attachment_preview_switches_between_images() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let suppressed = test_attachment("SUPPRESSEDIMAGE", "image/png", true, Some((640, 480)));
        let suppressed_hash = suppressed.sha256.clone();
        app.store.tasks[selected].attachments = vec![
            test_attachment("FIRSTIMAGE", "image/png", true, Some((640, 480))),
            test_attachment("MISSINGIMAGE", "image/png", false, Some((640, 480))),
            test_attachment("DOCUMENT", "application/pdf", true, None),
            suppressed,
            test_attachment("SECONDIMAGE", "image/jpeg", true, Some((800, 600))),
        ];
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext {
                unavailable_hashes: [suppressed_hash].into_iter().collect(),
                ..crate::tui::ui::DetailInlineImageContext::default()
            });
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target =
            Some(crate::tui::app::DetailTargetId::Attachment {
                attachment_id: "FIRSTIMAGE".to_string(),
            });
        app.overlay = Some(OverlayState::AttachmentPreview {
            attachment_id: "FIRSTIMAGE".to_string(),
            scroll: 4,
        });

        app.dispatch_key(key(KeyCode::Char('j')), (100, 30).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::AttachmentPreview {
                ref attachment_id,
                scroll: 4,
            }) if attachment_id == "SECONDIMAGE"
        ));
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some("SECONDIMAGE")
        );

        app.dispatch_key(key(KeyCode::Down), (100, 30).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::AttachmentPreview {
                ref attachment_id,
                scroll: 4,
            }) if attachment_id == "FIRSTIMAGE"
        ));

        app.dispatch_key(key(KeyCode::Char('k')), (100, 30).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::AttachmentPreview {
                ref attachment_id,
                scroll: 4,
            }) if attachment_id == "SECONDIMAGE"
        ));
    }

    #[tokio::test]
    async fn detail_image_click_opens_preview_while_payload_is_loading() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment = test_attachment("CLICKIMAGE", "image/png", true, Some((640, 480)));
        let source_hash = attachment.sha256.clone();
        app.store.tasks[selected].attachments = vec![attachment];
        app.show_detail(0);
        app.widgets.inline_image_placements = vec![crate::tui::ui::DetailInlineImagePlacement {
            attachment_id: "CLICKIMAGE".to_string(),
            source_hash,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }];
        let item = &app.store.tasks[selected];
        let context = crate::tui::ui::DetailInlineImageContext::default();
        let (column, row) = (0..30)
            .flat_map(|row| (0..100).map(move |column| (column, row)))
            .find(|(column, row)| {
                crate::tui::ui::detail_attachment_at_position(
                    item, 100, 30, *column, *row, 0, &context,
                )
                .is_some()
            })
            .expect("image hit target");

        app.dispatch_mouse(left_click(column, row), (100, 30).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::AttachmentPreview {
                ref attachment_id,
                scroll: 0,
            }) if attachment_id == "CLICKIMAGE"
        ));
    }

    #[tokio::test]
    async fn duplicate_hash_click_requires_the_visible_attachment_identity() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let context = crate::tui::ui::DetailInlineImageContext::default();
        app.inline_images.set_context_override(context.clone());
        let selected = create_and_select_task(&mut app, test_task_draft("Duplicate images")).await;
        let first = test_attachment("FIRSTDUPLICATE", "image/png", true, Some((640, 480)));
        let mut second = test_attachment("SECONDDUPLICATE", "image/png", true, Some((640, 480)));
        second.sha256 = first.sha256.clone();
        let source_hash = first.sha256.clone();
        app.store.tasks[selected].attachments = vec![first, second];
        let scroll = crate::tui::ui::detail_attachment_scroll_target(
            &app.store.tasks[selected],
            "SECONDDUPLICATE",
            0,
            80,
            30,
            &context,
        )
        .expect("second attachment scroll target");
        app.show_detail(scroll);
        let (column, row) = (0..30)
            .flat_map(|row| (0..80).map(move |column| (column, row)))
            .find(|(column, row)| {
                crate::tui::ui::detail_attachment_at_position(
                    &app.store.tasks[selected],
                    80,
                    30,
                    *column,
                    *row,
                    scroll,
                    &context,
                )
                .is_some_and(|hit| hit.attachment_id == "SECONDDUPLICATE")
            })
            .expect("second attachment hit");
        app.widgets.inline_image_placements = vec![crate::tui::ui::DetailInlineImagePlacement {
            attachment_id: "FIRSTDUPLICATE".to_string(),
            source_hash: source_hash.clone(),
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }];

        app.dispatch_mouse(left_click(column, row), (80, 30).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(detail_scroll(&app), scroll);

        app.widgets.inline_image_placements[0].attachment_id = "SECONDDUPLICATE".to_string();
        app.dispatch_mouse(left_click(column, row), (80, 30).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::AttachmentPreview {
                ref attachment_id,
                scroll: current,
            }) if attachment_id == "SECONDDUPLICATE" && current == scroll
        ));
    }

    #[tokio::test]
    async fn changing_detail_task_clears_image_focus() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        let first = create_and_select_task(&mut app, test_task_draft("First")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        let first = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == first_id)
            .unwrap();
        app.store.tasks[first].attachments = vec![test_attachment(
            "FIRSTIMAGE",
            "image/png",
            true,
            Some((640, 480)),
        )];
        app.list.select_task(Some(first));
        app.show_detail(0);
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target =
            Some(crate::tui::app::DetailTargetId::Attachment {
                attachment_id: "FIRSTIMAGE".to_string(),
            });

        let previous = app.list.selected_task();
        app.select_detail_task(1);

        assert_ne!(app.list.selected_task(), previous);
        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
    }

    #[tokio::test]
    async fn invalidated_image_focus_is_cleared_without_closing_detail() {
        for key_code in [KeyCode::Enter, KeyCode::Esc] {
            let (_dir, _pool, mut app) = test_app_with_pool().await;
            app.inline_images
                .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
            let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
            let attachment =
                test_attachment("INVALIDATEDIMAGE", "image/png", true, Some((640, 480)));
            app.store.tasks[selected].attachments = vec![attachment];
            app.show_detail(4);
            app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
                .await
                .unwrap();
            let focused_scroll = detail_scroll(&app);
            assert_eq!(
                app.detail
                    .state_mut()
                    .unwrap()
                    .focused_target
                    .as_ref()
                    .and_then(crate::tui::app::DetailTargetId::attachment_id),
                Some("INVALIDATEDIMAGE")
            );

            app.store.tasks[selected].attachments[0].has_blob = false;
            app.store.tasks[selected].attachments[0].bytes_state =
                crate::attachments::AttachmentBytesState::Unavailable;
            assert!(app.view().detail_focus.is_none());

            app.dispatch_key(key(key_code), (100, 30).into())
                .await
                .unwrap();

            assert!(app.detail.state_mut().unwrap().focused_target.is_none());
            assert!(app.overlay.is_none());
            assert!(app.detail.is_active());
            assert_eq!(detail_scroll(&app), focused_scroll);
        }
    }

    #[tokio::test]
    async fn detail_image_escape_returns_and_preserves_focus() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        app.store.tasks[selected].attachments = vec![test_attachment(
            "ATTACHMENT000001",
            "image/png",
            true,
            Some((640, 480)),
        )];
        app.show_detail(4);
        app.detail.state_mut().unwrap().focused_target =
            Some(crate::tui::app::DetailTargetId::Attachment {
                attachment_id: "ATTACHMENT000001".to_string(),
            });
        app.overlay = Some(OverlayState::AttachmentPreview {
            attachment_id: "ATTACHMENT000001".to_string(),
            scroll: 4,
        });
        assert!(!app.detail_underlay());

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some("ATTACHMENT000001")
        );
    }

    #[tokio::test]
    async fn attachment_preview_refresh_preserves_owning_task_across_query_change() {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
        let selected = create_and_select_task(&mut app, test_task_draft("Preview owner")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        app.store.tasks[selected].attachments = vec![test_attachment(
            "OWNEDIMAGE",
            "image/png",
            true,
            Some((640, 480)),
        )];
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target =
            Some(crate::tui::app::DetailTargetId::Attachment {
                attachment_id: "OWNEDIMAGE".to_string(),
            });
        app.overlay = Some(OverlayState::AttachmentPreview {
            attachment_id: "OWNEDIMAGE".to_string(),
            scroll: 6,
        });
        app.store.view_state.view = TaskView::Done;

        app.refresh().await.unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, task_id);
        assert_eq!(
            app.store.tasks[selected].attachments[0].attachment_id,
            "OWNEDIMAGE"
        );
        app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, task_id);
    }

    #[tokio::test]
    async fn detail_back_returns_from_epic_child_to_parent_detail() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let child_index = create_and_select_task(&mut app, test_task_draft("Child task")).await;
        let child_id = app.store.tasks[child_index].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &crate::workspaces::Workspace::default(),
            &child_id,
            &parent_id,
        )
        .await
        .unwrap();
        drop(conn);
        app.store.refresh(Some(&parent_id)).await.unwrap();
        let parent_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.list.select_task(Some(parent_index));
        app.show_detail(3);

        let parent_item = app.store.selected_task(app.list.selected_task()).unwrap();
        let click = (0..24)
            .flat_map(|row| (0..80).map(move |column| (column, row)))
            .find(|(column, row)| {
                crate::tui::ui::detail_child_task_at_position(parent_item, 80, 24, *column, *row, 3)
                    .is_some_and(|hit| hit.task_id == child_id)
            })
            .map(|(column, row)| left_click(column, row))
            .expect("expected child task hit target");

        app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, child_id);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
            .await
            .unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, parent_id);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_back_escape_restores_parent_scroll_and_child_focus() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let parent_index = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: true,
                ..test_task_draft("Parent epic")
            },
        )
        .await;
        let parent_id = app.store.tasks[parent_index].task.id.clone();
        let child_index = create_and_select_task(&mut app, test_task_draft("Child task")).await;
        let child_id = app.store.tasks[child_index].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            &child_id,
            &parent_id,
        )
        .await
        .unwrap();
        drop(conn);
        app.store.refresh(Some(&parent_id)).await.unwrap();
        app.list.select_task(Some(
            app.store
                .tasks
                .iter()
                .position(|item| item.task.id == parent_id)
                .unwrap(),
        ));
        app.show_detail(7);
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: child_id.clone(),
        });

        app.open_detail_task(&child_id, 7).await;
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, parent_id);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(DetailTargetId::task_id),
            Some(&child_id)
        );
    }

    #[tokio::test]
    async fn detail_back_cycle_unwinds_one_frame_per_escape() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("First")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("Second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        app.push_detail_navigation_state(detail_navigation_state(&app, first_id.clone(), 2));
        app.push_detail_navigation_state(detail_navigation_state(&app, second_id.clone(), 4));
        app.list.select_task(Some(first));
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            second_id
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            first_id
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_inactive());
    }

    #[tokio::test]
    async fn detail_back_escape_leaves_focus_before_popping_history() {
        let mut app = test_app().await;
        let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
        let parent_id = app.store.tasks[parent].task.id.clone();
        let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
        app.store.tasks[child].attachments = vec![test_attachment(
            "FOCUSEDIMAGE",
            "image/png",
            true,
            Some((640, 480)),
        )];
        app.list.select_task(Some(child));
        app.show_detail(5);
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
            attachment_id: "FOCUSEDIMAGE".to_string(),
        });
        app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 3));

        app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
            .await
            .unwrap();
        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()]
                .task
                .title,
            "Child"
        );
        assert!(app.detail_has_parent());

        app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            parent_id
        );
    }

    #[tokio::test]
    async fn detail_back_shortcut_works_while_attachment_is_focused() {
        let mut app = test_app().await;
        let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
        let parent_id = app.store.tasks[parent].task.id.clone();
        let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
        app.store.tasks[child].attachments = vec![test_attachment(
            "FOCUSEDIMAGE",
            "image/png",
            true,
            Some((640, 480)),
        )];
        app.list.select_task(Some(child));
        app.show_detail(0);
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
            attachment_id: "FOCUSEDIMAGE".to_string(),
        });
        app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 6));

        app.dispatch_key(key(KeyCode::Char('g')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('[')), (100, 30).into())
            .await
            .unwrap();

        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            parent_id
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_back_skips_unavailable_history_entries() {
        let mut app = test_app().await;
        let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
        let parent_id = app.store.tasks[parent].task.id.clone();
        create_and_select_task(&mut app, test_task_draft("Child")).await;
        app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 8));
        app.push_detail_navigation_state(detail_navigation_state(
            &app,
            crate::ids::TaskId::new(),
            4,
        ));
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            parent_id
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    }

    #[tokio::test]
    async fn detail_back_exhausted_unavailable_history_closes_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Child")).await;
        app.push_detail_navigation_state(detail_navigation_state(
            &app,
            crate::ids::TaskId::new(),
            4,
        ));
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_inactive());
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    }

    #[tokio::test]
    async fn detail_back_q_closes_session_and_enter_keeps_root_open() {
        let mut app = test_app().await;
        let root = create_and_select_task(&mut app, test_task_draft("Root")).await;
        let root_id = app.store.tasks[root].task.id.clone();
        create_and_select_task(&mut app, test_task_draft("Child")).await;
        app.push_detail_navigation_state(detail_navigation_state(&app, root_id, 3));
        app.show_detail(5);

        app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_inactive());
    }

    #[tokio::test]
    async fn detail_back_attachment_preview_escape_precedes_history() {
        let mut app = test_app().await;
        let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
        let parent_id = app.store.tasks[parent].task.id.clone();
        create_and_select_task(&mut app, test_task_draft("Child")).await;
        app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 9));
        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
            attachment_id: "PREVIEWIMAGE".to_string(),
        });
        app.overlay = Some(OverlayState::AttachmentPreview {
            attachment_id: "PREVIEWIMAGE".to_string(),
            scroll: 4,
        });

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.detail_has_parent());

        if app.detail.is_inactive() {
            app.show_detail(0);
        }
        app.detail.state_mut().unwrap().focused_target = None;
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            parent_id
        );
    }

    #[tokio::test]
    async fn detail_back_refresh_preserves_usable_parent_history() {
        let mut app = test_app().await;
        let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
        let parent_id = app.store.tasks[parent].task.id.clone();
        let child = create_and_select_task(&mut app, test_task_draft("Child")).await;
        let child_id = app.store.tasks[child].task.id.clone();
        app.push_detail_navigation_state(detail_navigation_state(&app, parent_id.clone(), 5));
        app.show_detail(0);

        app.refresh().await.unwrap();
        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            child_id
        );
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.store.tasks[app.list.selected_task().unwrap()].task.id,
            parent_id
        );
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_back_root_escape_closes_direct_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Root")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_inactive());
    }

    #[tokio::test]
    async fn detail_back_q_prevents_history_leaking_into_later_session() {
        let mut app = test_app().await;
        let parent = create_and_select_task(&mut app, test_task_draft("Parent")).await;
        let parent_id = app.store.tasks[parent].task.id.clone();
        create_and_select_task(&mut app, test_task_draft("Child")).await;
        app.push_detail_navigation_state(detail_navigation_state(&app, parent_id, 3));
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
            .await
            .unwrap();
        app.activate_or_toggle_detail().await.unwrap();
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_inactive());
    }

    #[tokio::test]
    async fn add_note_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('n')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state)) if state.title == ADD_NOTE_TITLE
        ));
        assert!(app.view().detail_underlay);

        type_chars(&mut app, "Important detail").await;
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].notes.len(), 1);
    }

    #[tokio::test]
    async fn availability_editor_sets_task_from_task_list() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Defer from list")).await;
        let task_id = app.store.tasks[selected].task.id.clone();

        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
            .await
            .unwrap();

        let rendered = render_app_text(&mut app, 100, 30);
        assert!(rendered.contains(EDIT_AVAILABILITY_TITLE));
        assert!(rendered.contains("Try tomorrow · in 2 weeks · next monday at 9am"));
        assert!(rendered.contains("Local dates/times · empty or now = immediate"));
        type_chars(&mut app, "tomorrow").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        app.show_view(TaskView::Upcoming).await.unwrap();
        let task = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert!(task.task.available_at.is_some());
    }

    #[tokio::test]
    async fn availability_editor_keeps_invalid_input_open() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Invalid availability")).await;
        app.begin_edit_availability();
        type_chars(&mut app, "someday maybe").await;

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if matches!(state.intent, TextIntent::EditAvailability { .. })
                    && state.input.text == "someday maybe"
        ));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
        assert!(toast_message(&app).is_some_and(|message| message.contains("use today, tomorrow")));
    }

    #[tokio::test]
    async fn due_editor_sets_clears_and_undoes_deadline() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Due from list")).await;

        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('u')), (100, 30).into())
            .await
            .unwrap();

        let rendered = render_app_text(&mut app, 100, 30);
        assert!(rendered.contains(EDIT_DUE_TITLE));
        assert!(rendered.contains("Try today · tomorrow · in 2 weeks · next monday"));
        type_chars(&mut app, "2099-01-01").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.due_on.as_deref(),
            Some("2099-01-01")
        );

        app.begin_edit_due();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        let selected = app.list.selected_task().unwrap();
        assert!(app.store.tasks[selected].task.due_on.is_none());

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        let selected = app.list.selected_task().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.due_on.as_deref(),
            Some("2099-01-01")
        );
    }

    #[tokio::test]
    async fn detail_availability_editor_reschedules_and_clears_task() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Detail availability")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        app.show_detail(2);

        for value in ["2099-01-01", "2099-02-01"] {
            let expected = crate::time_input::parse_available_at_input(value).unwrap();
            app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
                .await
                .unwrap();
            app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
                .await
                .unwrap();
            app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
                .await
                .unwrap();
            type_chars(&mut app, value).await;
            app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

            assert!(app.overlay.is_none());
            assert!(app.detail.is_active());
            let selected = app.list.selected_task().unwrap();
            assert_eq!(
                app.store.tasks[selected].task.available_at.as_deref(),
                Some(expected.as_str())
            );

            app.refresh().await.unwrap();
            let selected = app.list.selected_task().unwrap();
            assert_eq!(app.store.tasks[selected].task.id, task_id);
            assert!(app.overlay.is_none());
            assert!(app.detail.is_active());
        }

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
            .await
            .unwrap();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        type_chars(&mut app, "now").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        let selected = app.list.selected_task().unwrap();
        assert!(app.store.tasks[selected].task.available_at.is_none());
    }

    #[tokio::test]
    async fn leaving_detail_reconciles_deferred_task_with_active_view() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Leave detail")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
            .await
            .unwrap();
        type_chars(&mut app, "2099-01-01").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.store.tasks.iter().any(|item| item.task.id == task_id));

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.store.tasks.iter().all(|item| item.task.id != task_id));
    }

    #[tokio::test]
    async fn task_list_availability_editor_clears_with_empty_input() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Clear from list")).await;
        app.store
            .update_availability(
                app.list.selected_task(),
                "2099-01-01T00:00:00Z".to_string(),
                true,
            )
            .await
            .unwrap();
        app.show_view(TaskView::Upcoming).await.unwrap();
        app.list.select_task(Some(0));

        app.begin_edit_availability();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        app.show_view(TaskView::Queue).await.unwrap();
        assert!(
            app.store.tasks.iter().any(
                |item| item.task.title == "Clear from list" && item.task.available_at.is_none()
            )
        );
    }

    #[tokio::test]
    async fn detail_shortcuts_do_not_leave_detail_before_opening_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('p')), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Priority)
        );
        assert!(app.view().detail_underlay);
    }

    #[tokio::test]
    async fn edit_title_from_detail_renders_inline_cursor() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
        app.show_detail(5);

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if matches!(state.intent, TextIntent::EditTitle { .. })
        ));
        assert!(app.view().detail_underlay);
        assert_eq!(app.view().detail_underlay_scroll, 0);

        let rendered = render_app_text(&mut app, 100, 30);

        assert!(rendered.contains("Detail title target"));
        assert!(!rendered.contains("Edit title"));
        assert!(!rendered.contains("Enter submit"));
    }

    #[tokio::test]
    async fn cancel_edit_title_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn committed_title_edit_refresh_failure_does_not_reopen_editor() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Original title")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        app.begin_edit_title();
        app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
        type_chars(&mut app, " changed").await;
        app.store.fail_next_refresh();

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(!matches!(app.overlay, Some(OverlayState::TextInput(_))));
        assert!(toast_message(&app).is_some_and(|message| message.contains("mutation committed")));
        let persisted = app.store.load_task_item(&task_id).await.unwrap().unwrap();
        assert_eq!(persisted.task.title, "Original title changed");
    }

    #[tokio::test]
    async fn submit_edit_title_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.store.tasks[selected].task.title,
            "Detail title target updated"
        );
    }

    #[tokio::test]
    async fn detail_edit_chords_open_advertised_editors() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.store.create_label("Bug".to_string()).await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                title: "Detail target".to_string(),
                description: "existing description".to_string(),
                project: None,
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: TaskSource::Unknown,
                labels: vec!["bug".to_string()],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await;

        for (events, expected_intent) in [
            (
                vec![key(KeyCode::Char('e')), key(KeyCode::Char('l'))],
                "labels",
            ),
            (vec![shift_key(KeyCode::Char('N'))], "note"),
            (vec![shift_key(KeyCode::Char('D'))], "delete"),
        ] {
            app.show_detail(4);
            app.footer_choice = None;
            app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
                .await
                .unwrap();
            assert_pending(&app, &["t"]);
            assert!(app.overlay.is_none());
            assert!(app.detail.is_active());

            for event in events {
                app.dispatch_key(event, (80, 24).into()).await.unwrap();
            }
            let matches_intent = match (&app.overlay, expected_intent) {
                (Some(OverlayState::TagCombobox(state)), "labels") => {
                    matches!(state.intent, TagComboboxIntent::EditLabels { .. })
                }
                (Some(OverlayState::MultilineInput(state)), "note") => {
                    matches!(state.intent, MultilineIntent::AddNote { .. })
                }
                (Some(OverlayState::Confirm(state)), "delete") => {
                    matches!(state.intent, ConfirmIntent::DeleteTasks { .. })
                }
                _ => false,
            };
            assert!(
                matches_intent,
                "expected {expected_intent}, got {:?}",
                app.overlay
            );
            assert_pending_empty(&app);
            assert!(app.view().detail_underlay);
            assert_eq!(app.view().detail_underlay_scroll, 4);
        }

        for (events, expected_mode) in [
            (vec![key(KeyCode::Char('s'))], FooterChoiceMode::Status),
            (
                vec![key(KeyCode::Char('e')), key(KeyCode::Char('p'))],
                FooterChoiceMode::Priority,
            ),
        ] {
            app.show_detail(4);
            app.footer_choice = None;
            app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
                .await
                .unwrap();
            assert_pending(&app, &["t"]);
            assert!(app.overlay.is_none());
            assert!(app.detail.is_active());

            for event in events {
                app.dispatch_key(event, (80, 24).into()).await.unwrap();
            }
            assert_eq!(
                app.footer_choice.as_ref().map(|choice| choice.mode),
                Some(expected_mode)
            );
            assert_pending_empty(&app);
            assert!(app.view().detail_underlay);
            assert_eq!(app.view().detail_underlay_scroll, 4);
        }
    }

    #[tokio::test]
    async fn detail_single_key_edit_shortcuts_still_work() {
        let mut app = test_app().await;
        app.store.create_label("Bug".to_string()).await.unwrap();
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(3);

        app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('l')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TagCombobox(state)) if matches!(state.intent, TagComboboxIntent::EditLabels { .. })
        ));
        assert_pending_empty(&app);
        assert!(app.view().detail_underlay);
    }

    #[tokio::test]
    async fn invalid_detail_prefix_stays_in_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(5);

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('z')), (80, 24).into())
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("invalid shortcut: t z")
        );
    }

    #[tokio::test]
    async fn detail_prefix_hints_render_above_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();

        let rendered = render_app_text(&mut app, 100, 30);

        assert!(rendered.contains("t …"));
        assert!(rendered.contains(":detail-edit-title"));
    }

    #[tokio::test]
    async fn ignored_keys_stay_in_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('a')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn detail_copy_clicks_copy_displayed_values_and_show_toasts() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Copy target")).await;
        app.show_detail(4);
        let terminal_size: ratatui::layout::Size = (120, 30).into();

        for (column, row) in [(2, 5), (88, 17), (88, 20), (88, 23)] {
            let value = crate::tui::ui::detail_copy_target_at(
                app.store.selected_task(app.list.selected_task()).unwrap(),
                terminal_size.width,
                terminal_size.height,
                column,
                row,
            )
            .unwrap()
            .value;

            app.dispatch_mouse(left_click(column, row), terminal_size)
                .await
                .unwrap();

            assert_eq!(
                crate::tui::platform::clipboard_text_for_test(),
                Some(value.clone())
            );
            assert_eq!(
                toast_message(&app).as_deref(),
                Some(format!("copied {value}").as_str())
            );
            assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
            assert!(app.overlay.is_none());
            assert!(app.detail.is_active());
        }
    }

    #[tokio::test]
    async fn detail_toast_renders_above_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Toast target")).await;
        app.show_detail(0);
        app.set_success("set APP-TEST status=done");

        let rendered = render_app_text(&mut app, 100, 30);

        assert!(rendered.contains("set APP-TEST status=done"));
    }

    #[tokio::test]
    async fn detail_done_shortcut_keeps_detail_and_sets_message() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Next target")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Done target")).await;
        let selected_task_id = app.store.tasks[selected].task.id.clone();
        let display_ref = app.store.tasks[selected].display_ref.clone();
        app.show_detail(7);

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(format!("set {display_ref} status=done").as_str())
        );
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, selected_task_id);
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Done);
    }

    #[tokio::test]
    async fn queue_status_change_preserves_viewport_row_and_recalls_changed_task() {
        let mut app = test_app().await;
        for title in ["First", "Changed", "Third"] {
            create_and_select_task(&mut app, test_task_draft(title)).await;
        }
        app.show_view(TaskView::Queue).await.unwrap();
        let selected = 1;
        let changed_id = app.store.tasks[selected].task.id.clone();
        app.list.select_task(Some(selected));
        app.list.set_task_offset(1);

        app.update_status(TaskStatus::Active).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(selected));
        assert_ne!(app.store.tasks[selected].task.id, changed_id);
        assert_eq!(app.list.task_offset(), 2);
        assert_eq!(app.list.last_changed_task_id(), Some(&changed_id));
        assert!(toast_message(&app).unwrap().contains("g . return"));

        app.execute(Action::ReturnToLastChange).await.unwrap();

        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .unwrap()
                .task
                .id,
            changed_id
        );
    }

    #[tokio::test]
    async fn recalling_filtered_status_change_returns_from_detail_to_queue_anchor() {
        let mut app = test_app().await;
        for title in ["First", "Changed", "Third"] {
            create_and_select_task(&mut app, test_task_draft(title)).await;
        }
        app.show_view(TaskView::Queue).await.unwrap();
        let selected = 1;
        let changed_id = app.store.tasks[selected].task.id.clone();
        app.list.select_task(Some(selected));
        app.list.set_task_offset(1);

        app.update_status(TaskStatus::Done).await.unwrap();
        let replacement_id = app.store.tasks[selected].task.id.clone();
        let return_offset = app.list.task_offset();
        app.execute(Action::ReturnToLastChange).await.unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Search);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.id, changed_id);

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Queue);
        assert!(app.overlay.is_none());
        assert_eq!(app.list.task_offset(), return_offset);
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .unwrap()
                .task
                .id,
            replacement_id
        );
    }

    #[tokio::test]
    async fn detail_status_picker_done_keeps_same_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Next target")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Done target")).await;
        let selected_task_id = app.store.tasks[selected].task.id.clone();
        app.show_detail(4);

        app.dispatch_key(key(KeyCode::Char('s')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, selected_task_id);
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Done);
    }

    #[tokio::test]
    async fn detail_status_mouse_click_opens_menu_and_returns_to_detail() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Status click target")).await;
        app.show_detail(3);

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
            (120, 30).into(),
        )
        .await
        .unwrap();

        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
        assert!(app.view().detail_underlay);

        app.dispatch_key(key(KeyCode::Char('a')), (120, 30).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Active);
    }

    #[tokio::test]
    async fn detail_status_menu_empty_click_returns_to_detail_without_selecting_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Hidden task")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Visible task")).await;
        app.show_detail(0);

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
            (120, 30).into(),
        )
        .await
        .unwrap();
        app.dispatch_mouse(left_click(110, 20), (120, 30).into())
            .await
            .unwrap();

        assert_eq!(app.list.selected_task(), Some(selected));
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn detail_priority_mouse_click_opens_menu_and_returns_to_detail() {
        let mut app = test_app().await;
        let selected = create_and_select_task(
            &mut app,
            TaskDraft {
                priority: "medium".to_string(),
                ..test_task_draft("Priority click target")
            },
        )
        .await;
        app.show_detail(3);

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Priority),
            (120, 30).into(),
        )
        .await
        .unwrap();

        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Priority)
        );
        assert!(app.view().detail_underlay);

        app.dispatch_key(key(KeyCode::Char('u')), (120, 30).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            app.store.tasks[selected].task.priority,
            TaskPriority::Urgent
        );
    }

    #[tokio::test]
    async fn detail_undo_shortcut_reverts_last_mutation() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        app.store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();
        app.show_detail(5);

        app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(app.store.tasks[selected].task.title, "Before");
        assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
    }

    #[tokio::test]
    async fn detail_undo_after_status_menu_keeps_task_identity() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Other task")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Undo target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        app.show_detail(0);

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
            (120, 30).into(),
        )
        .await
        .unwrap();
        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
        app.dispatch_key(key(KeyCode::Char('a')), (120, 30).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        app.dispatch_key(key(KeyCode::Char('u')), (120, 30).into())
            .await
            .unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, task_id);
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    #[tokio::test]
    async fn populated_add_note_from_detail_returns_after_confirmed_discard() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
            .await
            .unwrap();
        type_chars(&mut app, "detail draft").await;
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.mode == MultilineInputMode::ConfirmDiscard
                    && state.lines == ["detail draft"]
        ));
        assert!(app.detail.is_active());

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.mode == MultilineInputMode::Compose
                    && state.lines == ["detail draft"]
        ));
        assert!(app.detail.is_active());

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('Y')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn cancel_add_note_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn add_note_blank_body_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;
        app.show_detail(0);

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
            .await
            .unwrap();
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("note body is required")
        );
    }
}

mod task_editing {
    use super::*;

    #[tokio::test]
    async fn add_note_blank_body_is_rejected() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('N')).await.unwrap();
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("note body is required")
        );
    }

    #[tokio::test]
    async fn no_selected_mutating_shortcuts_report_failure() {
        let mut app = test_app().await;
        app.list.select_task(None);

        for sequence in [
            [KeyCode::Char('t'), KeyCode::Char('i')],
            [KeyCode::Char('t'), KeyCode::Char('h')],
            [KeyCode::Char('t'), KeyCode::Char('D')],
            [KeyCode::Char('t'), KeyCode::Char('R')],
        ] {
            app.notification = None;
            app.handle_normal_key(sequence[0]).await.unwrap();
            app.handle_normal_key(sequence[1]).await.unwrap();
            assert_eq!(
                toast_message(&app).as_deref(),
                Some("no selected task to edit")
            );
        }
    }

    #[tokio::test]
    async fn add_project_shortcut_opens_prompt_and_creates_project() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.prompt == "project name:"
        ));

        for ch in "Mobile App".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("created project mobile-app")
        );
        assert!(
            app.store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
        assert!(
            app.store
                .sidebar_entries
                .iter()
                .any(|entry| entry.label.contains("Mobile App"))
        );
    }

    #[tokio::test]
    async fn add_label_shortcut_opens_prompt_and_creates_label() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('L')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.prompt == "label name:"
        ));

        for ch in "Needs Review".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("created label needs-review")
        );
        assert!(app.store.labels.iter().any(|label| label == "needs-review"));
        assert!(
            app.store
                .label_picker_items()
                .iter()
                .any(|item| item.value == "needs-review")
        );
    }

    #[tokio::test]
    async fn edit_title_shortcut_prefills_and_updates_title() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Old title")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if state.title == EDIT_TITLE_TITLE
                    && state.prompt.is_empty()
                    && state.input.as_str() == "Old title"
        ));

        app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Old title updated");
    }

    #[tokio::test]
    async fn one_mark_edits_the_marked_title_and_preserves_cursor_selection() {
        let mut app = test_app().await;
        let marked = create_and_select_task(&mut app, test_task_draft("Marked title")).await;
        let marked_id = app.store.tasks[marked].task.id.clone();
        let selected = create_and_select_task(&mut app, test_task_draft("Cursor title")).await;
        let selected_id = app.store.tasks[selected].task.id.clone();
        app.list.mark(marked_id.clone());

        app.begin_edit_title();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.input.as_str() == "Marked title"
        ));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        type_chars(&mut app, "Updated marked title").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let marked = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == marked_id)
            .unwrap();
        assert_eq!(marked.task.title, "Updated marked title");
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, selected_id);
        assert!(app.list.marked_task_ids().contains(&marked_id));
    }

    #[tokio::test]
    async fn multiple_marks_disable_title_editing() {
        let mut app = test_app().await;
        let first = create_and_select_task(&mut app, test_task_draft("First")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("Second")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        app.list.mark(first_id);
        app.list.mark(second_id);

        app.begin_edit_title();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("title requires one task · 2 tasks marked")
        );
    }

    #[tokio::test]
    async fn edit_description_prefills_and_ctrl_s_updates() {
        let mut app = test_app().await;
        create_and_select_task(
            &mut app,
            TaskDraft {
                description: "first\nsecond".to_string(),
                ..test_task_draft("Description target")
            },
        )
        .await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.title == EDIT_DESCRIPTION_TITLE
                    && state.prompt.is_empty()
                    && state.lines == vec!["first".to_string(), "second".to_string()]
        ));

        app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.description,
            "first\nsecond updated"
        );
    }

    #[tokio::test]
    async fn edit_project_picker_uses_existing_projects_only() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        create_and_select_task(&mut app, test_task_draft("Project target")).await;

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('j')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state))
                if state.title == EDIT_PROJECT_TITLE
                    && !state.items.iter().any(|item| item.label == "Infer project")
        ));

        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.project_key, "mobile-app");
    }

    #[tokio::test]
    async fn edit_priority_picker_prefills_current_priority() {
        let mut app = test_app().await;
        create_and_select_task(
            &mut app,
            TaskDraft {
                priority: "high".to_string(),
                ..test_task_draft("Priority target")
            },
        )
        .await;

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Priority)
        );

        app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
            .await
            .unwrap();
        let selected = app.list.selected_task().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.priority,
            TaskPriority::Urgent
        );
    }

    #[tokio::test]
    async fn edit_labels_picker_prefills_current_labels_and_removes_unselected() {
        let mut app = test_app().await;
        app.store.create_label("Bug".to_string()).await.unwrap();
        app.store.create_label("Docs".to_string()).await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                labels: vec!["bug".to_string()],
                ..test_task_draft("Label target")
            },
        )
        .await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TagCombobox(state))
                if state.title == EDIT_LABELS_TITLE
                    && state.options.iter().any(|item| item == "bug")
                    && state.selected.iter().any(|item| item == "bug")
        ));

        type_chars(&mut app, "bug").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].labels, vec!["docs".to_string()]);
    }

    #[tokio::test]
    async fn status_picker_alias_updates_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Status alias")).await;

        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Todo);
    }

    #[tokio::test]
    async fn done_alias_keeps_selected_row_position() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        create_and_select_task(&mut app, test_task_draft("Third")).await;
        let selected = 1;
        let next_title = app.store.tasks[selected + 1].task.title.clone();
        app.list.select_task(Some(selected));

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(selected));
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, next_title);
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
    }

    #[tokio::test]
    async fn done_alias_clamps_selection_when_last_row_is_done() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        let selected = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.title == "Second")
            .unwrap();
        app.list.select_task(Some(selected));

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(0));
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "First");
    }

    #[tokio::test]
    async fn done_and_cancel_aliases_update_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Status alias")).await;

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        let selected = app.store.show_view(TaskView::Done).await.unwrap();
        app.list.select_task(selected);
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Done);

        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        let selected = app.store.show_view(TaskView::Done).await.unwrap();
        app.list.select_task(selected);
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Canceled);
    }

    #[tokio::test]
    async fn exact_priority_shortcut_updates_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Priority shortcut")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

        let selected = app.list.selected_task().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.priority,
            TaskPriority::Urgent
        );
    }

    #[tokio::test]
    async fn edit_shortcuts_require_selected_task() {
        let mut app = test_app().await;
        app.list.select_task(None);

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task to edit")
        );
    }

    #[tokio::test]
    async fn edit_description_conflict_preserves_overlay() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(
            &mut app,
            TaskDraft {
                description: "old".to_string(),
                ..test_task_draft("Conflict target")
            },
        )
        .await;
        let task_id = app.store.tasks[selected].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, 'description', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(&task_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(toast_message(&app).is_some_and(|message| message.contains("conflicted-field")));
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.lines.join("\n") == "old updated"
        ));
    }

    #[tokio::test]
    async fn detail_copy_hotkeys_copy_task_text_and_show_feedback() {
        let mut app = test_app().await;
        let selected = create_and_select_task(
            &mut app,
            TaskDraft {
                description: "First paragraph.\n\n- item".to_string(),
                ..test_task_draft("Copy target")
            },
        )
        .await;
        app.store.tasks[selected]
            .notes
            .push(crate::query::TaskNote {
                body: "Note body".to_string(),
                created_at: crate::ids::now(),
            });
        assert!(app.view().copy_description_available);
        assert!(app.view().copy_notes_available);

        for (key_code, expected_message) in [
            ('t', "copied task title"),
            ('d', "copied task description"),
            ('a', "copied task title and description"),
            ('n', "copied task notes"),
        ] {
            app.show_detail(3);
            app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
                .await
                .unwrap();
            app.dispatch_key(key(KeyCode::Char(key_code)), (80, 24).into())
                .await
                .unwrap();

            assert_eq!(toast_message(&app).as_deref(), Some(expected_message));
            assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
            assert!(app.overlay.is_none());
            assert!(app.detail.is_active());
        }
    }

    #[tokio::test]
    async fn copying_empty_task_description_shows_info() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Title only")).await;
        app.show_detail(0);
        assert!(!app.view().copy_description_available);

        app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("task description is empty")
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
    }

    #[tokio::test]
    async fn copying_task_without_notes_shows_info() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("No notes")).await;
        assert!(!app.view().copy_notes_available);

        app.handle_normal_key(KeyCode::Char('y')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert_eq!(toast_message(&app).as_deref(), Some("task has no notes"));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
    }

    #[tokio::test]
    async fn table_copy_hotkeys_copy_task_text_and_show_feedback() {
        let mut app = test_app().await;
        let selected = create_and_select_task(
            &mut app,
            TaskDraft {
                description: "First paragraph.\n\n- item".to_string(),
                ..test_task_draft("Copy target")
            },
        )
        .await;
        app.store.tasks[selected]
            .notes
            .push(crate::query::TaskNote {
                body: "Note body".to_string(),
                created_at: crate::ids::now(),
            });
        assert!(app.view().copy_description_available);
        assert!(app.view().copy_notes_available);

        for (key_code, expected_message) in [
            ('t', "copied task title"),
            ('d', "copied task description"),
            ('a', "copied task title and description"),
            ('n', "copied task notes"),
        ] {
            app.handle_normal_key(KeyCode::Char('y')).await.unwrap();
            app.handle_normal_key(KeyCode::Char(key_code))
                .await
                .unwrap();

            assert_eq!(toast_message(&app).as_deref(), Some(expected_message));
            assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
            assert!(app.overlay.is_none());
        }
    }

    #[tokio::test]
    async fn table_copy_menu_copies_display_ref_and_id() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Copy target")).await;
        let display_ref = app.store.tasks[selected].display_ref.clone();

        for key_code in ['r', 'i'] {
            app.handle_normal_key(KeyCode::Char('y')).await.unwrap();
            app.handle_normal_key(KeyCode::Char(key_code))
                .await
                .unwrap();

            assert_eq!(
                toast_message(&app).as_deref(),
                Some(format!("copied {display_ref}").as_str())
            );
            assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
        }
    }

    #[tokio::test]
    async fn copy_requires_selected_task() {
        let mut app = test_app().await;
        app.list.select_task(None);

        app.copy_selected_ref(TaskRefKind::Short);

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task to copy")
        );
    }

    #[tokio::test]
    async fn undo_shortcut_reverts_last_mutation() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        app.store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "After");

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Before");
        assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
    }

    #[tokio::test]
    async fn undo_shortcut_keeps_selected_row_position() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        create_and_select_task(&mut app, test_task_draft("Third")).await;
        let selected = 1;
        app.list.select_task(Some(selected));

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(app.list.selected_task(), Some(selected));

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

        assert_eq!(app.list.selected_task(), Some(selected));
        assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
    }

    #[tokio::test]
    async fn undo_command_reverts_last_mutation() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        app.store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();

        app.begin_command().await;
        for ch in "undo".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "Before");
    }

    #[tokio::test]
    async fn undo_reports_nothing_to_undo() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        assert_eq!(toast_message(&app).as_deref(), Some("nothing to undo"));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
    }
}

mod delete_and_restore {
    use super::*;

    #[tokio::test]
    async fn delete_task_opens_confirmation_with_task_context() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Delete target")).await;
        let display_ref = app.store.tasks[selected].display_ref.clone();

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                intent: ConfirmIntent::DeleteTasks { .. },
                ref title,
                ref prompt,
            })) if title == DELETE_TASK_TITLE
                && prompt.contains(&display_ref)
                && prompt.contains("Delete target")
        ));
        assert!(!app.store.tasks[selected].task.deleted);
    }

    #[tokio::test]
    async fn cancel_delete_task_leaves_task_unchanged() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Keep target")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.store.tasks[selected].task.deleted);
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn confirm_delete_task_soft_deletes_selected_task() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Delete target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        let display_ref = app.store.tasks[selected].display_ref.clone();

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(app.list.selected_task(), Some(selected));
        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[selected].task.id, task_id);
        assert!(app.store.tasks[selected].task.deleted);
        assert!(
            app.store
                .load_task_item(&task_id)
                .await
                .unwrap()
                .unwrap()
                .task
                .deleted
        );
        assert!(!app.store.view_state.filter_modifiers.include_deleted);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(format!("deleted {display_ref}").as_str())
        );
    }

    #[tokio::test]
    async fn delete_task_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Detail delete target")).await;
        app.show_detail(7);

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('D')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                intent: ConfirmIntent::DeleteTasks { .. },
                ..
            }))
        ));
        assert!(app.view().detail_underlay);

        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert!(app.store.tasks[selected].task.deleted);
    }

    #[tokio::test]
    async fn rename_project_opens_project_picker_from_task_focus() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginRenameProject).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::RenameProject,
                ..
            }))
        ));
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn delete_project_opens_project_picker_from_task_focus() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::DeleteProject,
                ..
            }))
        ));
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn delete_project_picker_preselects_sidebar_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.list.focus_sidebar();
        let project_index = app
            .store
            .sidebar_entries
            .iter()
            .position(|entry| {
                entry.target
                    == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                        "mobile-app".to_string(),
                    )))
            })
            .unwrap();
        app.list.select_sidebar(Some(project_index));

        app.execute(Action::BeginDeleteProject).await.unwrap();

        let Some(OverlayState::Picker(state)) = &app.overlay else {
            panic!("expected project picker");
        };
        assert_eq!(state.items[state.selected].value, "mobile-app");
    }

    #[tokio::test]
    async fn rename_project_submission_updates_selected_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Agent Offload".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginRenameProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                intent: TextIntent::RenameProject { .. },
                ..
            }))
        ));
        app.submit_rename_project("agent-offload".to_string(), "sideagent".to_string())
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("renamed project sideagent prefix=SDG")
        );
        assert!(
            app.store
                .projects
                .iter()
                .any(|project| project.key == "sideagent")
        );
    }

    #[tokio::test]
    async fn rename_project_cancel_returns_to_browse() {
        let mut app = test_app().await;
        app.store
            .create_project("Agent Offload".to_string())
            .await
            .unwrap();
        let selected = app.list.selected_task();

        app.execute(Action::BeginRenameProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.view().detail_underlay);
        assert_eq!(app.list.selected_task(), selected);
    }

    #[tokio::test]
    async fn delete_project_confirmation_removes_selected_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                intent: TextIntent::ConfirmDeleteProject { .. },
                ..
            }))
        ));
        app.submit_delete_project_name("mobile-app".to_string(), "mobile-app".to_string())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                intent: ConfirmIntent::DeleteProject { .. },
                ..
            }))
        ));
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("deleted project mobile-app")
        );
        assert!(
            !app.store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
    }

    #[tokio::test]
    async fn delete_project_cancel_discards_intent() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.list.focus_sidebar();
        let project_index = app
            .store
            .sidebar_entries
            .iter()
            .position(|entry| {
                entry.target
                    == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                        "mobile-app".to_string(),
                    )))
            })
            .unwrap();
        app.list.select_sidebar(Some(project_index));

        app.execute(Action::BeginDeleteProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                intent: TextIntent::ConfirmDeleteProject { .. },
                ..
            }))
        ));
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    }
}

mod conflicts {
    use super::*;

    #[tokio::test]
    async fn conflict_list_shortcut_applies_conflicts_view() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no unresolved conflicts")
        );
    }

    #[tokio::test]
    async fn conflict_show_opens_text_panel_and_esc_closes() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Conflict show")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextPanel(state))
                if state.lines.iter().any(|line| line.contains("field=title"))
        ));

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn conflict_next_selects_next_conflicted_task() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        let first_id = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == "First")
            .unwrap()
            .task
            .id
            .clone();
        let second_id = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == "Second")
            .unwrap()
            .task
            .id
            .clone();
        insert_title_conflict_for_task_id(&pool, &mut app, &first_id, "local one", "remote one")
            .await;
        insert_title_conflict_for_task_id(&pool, &mut app, &second_id, "local two", "remote two")
            .await;
        let first = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == first_id)
            .unwrap();
        let second = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == second_id)
            .unwrap();
        app.list.select_task(Some(first));

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        assert_eq!(app.list.selected_task(), Some(second));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("selected next conflict")
        );
    }

    #[tokio::test]
    async fn accept_local_conflict_resolves_after_confirmation() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Confirm(state)) if state.title == CONFLICT_CONFIRM_LOCAL_TITLE
        ));

        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.store.tasks[selected].task.title, "local title");
        assert!(!app.store.tasks[selected].has_conflict);
        assert!(
            toast_message(&app).is_some_and(
                |message| message.contains("resolved") && message.contains("field=title")
            )
        );
    }

    #[tokio::test]
    async fn accept_remote_conflict_resolves_after_confirmation() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "remote title");
        assert!(!app.store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn manual_conflict_merge_resolves_with_submitted_value() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        type_chars(&mut app, " merged").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "local title merged");
        assert!(!app.store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn manual_conflict_retry_preserves_submitted_text_after_error() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        type_chars(&mut app, " merged").await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("DELETE FROM conflicts WHERE task_id = ? AND field = 'title'")
            .bind(&task_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(toast_message(&app).is_some_and(|message| message.contains("conflict-not-found")));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if matches!(state.intent, TextIntent::ResolveConflictManually { .. })
                    && state.input.as_str() == "local title merged"
        ));
    }

    #[tokio::test]
    async fn conflict_resolution_without_selected_task_reports_message() {
        let mut app = test_app().await;
        app.list.select_task(None);

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task for conflict resolution")
        );
    }

    #[tokio::test]
    async fn cancel_discards_conflict_intent() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Conflict")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                intent: ConfirmIntent::ResolveConflict { .. },
                ..
            }))
        ));
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }
}

mod typed_overlay_submissions {
    use super::*;

    #[tokio::test]
    async fn add_project_submit_uses_intent_independent_of_title() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::TextInput(TextInputState::new(
            TextIntent::AddProject,
            "Renamed copy",
            "project name:",
            "Mobile App".to_string(),
        )));

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("created project mobile-app")
        );
    }

    #[tokio::test]
    async fn add_task_project_shortcut_uses_intent_independent_of_title_prefix() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = &mut app.overlay else {
            panic!("expected add task overlay");
        };
        state.project = "Create item".to_string();

        app.handle_overlay_key(ctrl_p()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if matches!(
                    &state.mode,
                    crate::tui::overlay::AddTaskMode::Picker { state: picker, .. }
                        if picker.intent == PickerIntent::AddTaskProject
                )
        ));
    }

    #[tokio::test]
    async fn add_task_priority_shortcut_uses_intent_independent_of_title_prefix() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = &mut app.overlay else {
            panic!("expected add task overlay");
        };
        state.project = "Create item".to_string();

        app.handle_overlay_key(ctrl_r()).await.unwrap();

        assert_pending(&app, &["r"]);
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::AddTask(state)) if state.priority_prefix_active
        ));
    }

    #[tokio::test]
    async fn delete_project_picker_and_name_confirm_use_distinct_intents() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Picker(PickerState {
                intent: PickerIntent::DeleteProject,
                ref title,
                ..
            })) if title == DELETE_PROJECT_TITLE
        ));

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                intent: TextIntent::ConfirmDeleteProject { .. },
                ref title,
                ..
            })) if title == DELETE_PROJECT_TITLE
        ));
    }

    #[tokio::test]
    async fn delete_project_name_mismatch_keeps_confirmation_open() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        app.submit_delete_project_name("mobile-app".to_string(), "mobile".to_string())
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("project name does not match")
        );
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                intent: TextIntent::ConfirmDeleteProject { .. },
                ref title,
                ..
            })) if title == DELETE_PROJECT_TITLE
        ));
        assert!(
            app.store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
    }

    #[tokio::test]
    async fn search_selected_blocker_adds_dependency() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Blocker needle")).await;
        let blocked_index = create_and_select_task(&mut app, test_task_draft("Blocked")).await;
        let blocked_id = app.store.tasks[blocked_index].task.id.clone();

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('B')).await.unwrap();
        type_chars(&mut app, "needle").await;
        settle_search_preview(&mut app).await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let blocked_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == blocked_id)
            .unwrap();
        assert_eq!(app.store.tasks[blocked_index].depends_on.len(), 1);
        let toast = toast_message(&app);
        assert!(
            toast.is_some() && toast.as_deref().unwrap().contains("added dependency"),
            "expected success message, got: {:?}",
            toast,
        );
    }

    #[tokio::test]
    async fn add_dependency_search_tab_keeps_picker_context() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Blocked")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('B')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state))
                if matches!(state.intent, SearchIntent::AddDependency { .. })
        ));
    }

    #[tokio::test]
    async fn remove_shortcut_opens_current_dependency_picker() {
        let mut app = test_app().await;
        let blocker_index = create_and_select_task(&mut app, test_task_draft("Blocker")).await;
        let blocker_id = app.store.tasks[blocker_index].task.id.clone();
        let blocked_index = create_and_select_task(&mut app, test_task_draft("Blocked")).await;
        let blocked_id = app.store.tasks[blocked_index].task.id.clone();
        app.store
            .add_dependency(Some(blocked_index), &blocker_id)
            .await
            .unwrap()
            .unwrap();
        let blocked_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == blocked_id)
            .unwrap();
        app.list.select_task(Some(blocked_index));

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('U')).await.unwrap();

        let Some(OverlayState::Picker(state)) = &app.overlay else {
            panic!("expected dependency picker");
        };
        assert!(matches!(
            &state.intent,
            PickerIntent::RemoveDependency { selection }
                if selection.single_id() == Some(&blocked_id)
        ));
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].value, blocker_id.to_string());
    }

    #[tokio::test]
    async fn submitting_dependency_removal_removes_dependency() {
        let mut app = test_app().await;
        let blocker_index = create_and_select_task(&mut app, test_task_draft("Blocker")).await;
        let blocker_id = app.store.tasks[blocker_index].task.id.clone();
        let blocked_index = create_and_select_task(&mut app, test_task_draft("Blocked")).await;
        let blocked_id = app.store.tasks[blocked_index].task.id.clone();
        app.store
            .add_dependency(Some(blocked_index), &blocker_id)
            .await
            .unwrap()
            .unwrap();
        let blocked_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == blocked_id)
            .unwrap();
        app.list.select_task(Some(blocked_index));

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('U')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let blocked_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == blocked_id)
            .unwrap();
        assert!(app.store.tasks[blocked_index].depends_on.is_empty());
        assert!(
            toast_message(&app).is_some_and(|message| { message.contains("removed dependency") })
        );
    }

    #[tokio::test]
    async fn no_selected_task_shows_info() {
        let mut app = test_app().await;
        app.list.select_task(None);

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('B')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task to edit")
        );
    }

    #[tokio::test]
    async fn column_view_installs_custom_configuration() {
        let mut app = test_app().await;
        let mut config = crate::config::AppConfig::default();
        config.tui.columns.reverse();

        app.set_config(config);

        assert_eq!(app.store.task_columns[0].name, "Done");
    }

    #[tokio::test]
    async fn column_view_shortcut_selects_columns() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Columns);
    }

    #[tokio::test]
    async fn column_view_preview_shortcut_toggles_session_visibility() {
        let mut app = test_app().await;
        app.store.show_view(TaskView::Columns).await.unwrap();
        assert!(app.store.columns_preview_visible);

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert!(!app.store.columns_preview_visible);

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert!(app.store.columns_preview_visible);
    }

    #[tokio::test]
    async fn column_move_shortcut_updates_status_and_undo_restores_it() {
        let mut app = test_app().await;
        let index = create_and_select_task(&mut app, test_task_draft("move me")).await;
        let task_id = app.store.tasks[index].task.id.clone();
        app.store.show_view(TaskView::Columns).await.unwrap();
        app.list.select_task(
            app.store
                .tasks
                .iter()
                .position(|item| item.task.id == task_id),
        );

        app.handle_normal_key(KeyCode::Char('>')).await.unwrap();

        let moved = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(moved.task.status, TaskStatus::Backlog);
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .unwrap()
                .task
                .id,
            task_id
        );

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        assert_eq!(
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id == task_id)
                .unwrap()
                .task
                .status,
            TaskStatus::Inbox
        );
    }

    #[tokio::test]
    async fn column_move_shortcut_advances_marked_tasks_from_their_own_lanes() {
        let mut app = test_app().await;
        let inbox = create_and_select_task(&mut app, test_task_draft("inbox task")).await;
        let inbox_id = app.store.tasks[inbox].task.id.clone();
        let mut todo_draft = test_task_draft("ready task");
        todo_draft.status = "todo".to_string();
        let todo = create_and_select_task(&mut app, todo_draft).await;
        let todo_id = app.store.tasks[todo].task.id.clone();
        app.list.mark(inbox_id.clone());
        app.list.mark(todo_id.clone());
        app.store.show_view(TaskView::Columns).await.unwrap();

        app.handle_normal_key(KeyCode::Char('>')).await.unwrap();

        let status_for = |task_id: &str| {
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id.as_str() == task_id)
                .unwrap()
                .task
                .status
        };
        assert_eq!(status_for(&inbox_id), TaskStatus::Backlog);
        assert_eq!(status_for(&todo_id), TaskStatus::Active);

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        let status_for = |task_id: &str| {
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id.as_str() == task_id)
                .unwrap()
                .task
                .status
        };
        assert_eq!(status_for(&inbox_id), TaskStatus::Inbox);
        assert_eq!(status_for(&todo_id), TaskStatus::Todo);
    }

    #[tokio::test]
    async fn column_move_picker_uses_lane_names_and_first_statuses() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("move with picker")).await;
        app.store.show_view(TaskView::Columns).await.unwrap();

        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { intent: PickerIntent::MoveToColumn { .. }, title, items, .. }))
                if title == "Move to column"
                    && items.iter().any(|item| item.label == "Done" && item.value == "done")
                    && items.iter().all(|item| !item.label.contains('→'))
        ));
    }

    #[tokio::test]
    async fn column_relative_move_keeps_marked_batch_unchanged_at_edge() {
        let mut app = test_app().await;
        let inbox = create_and_select_task(&mut app, test_task_draft("inbox edge")).await;
        let inbox_id = app.store.tasks[inbox].task.id.clone();
        let mut backlog_draft = test_task_draft("backlog beside edge");
        backlog_draft.status = "backlog".to_string();
        let backlog = create_and_select_task(&mut app, backlog_draft).await;
        let backlog_id = app.store.tasks[backlog].task.id.clone();
        app.list.mark(inbox_id.clone());
        app.list.mark(backlog_id.clone());
        app.store.show_view(TaskView::Columns).await.unwrap();

        app.handle_normal_key(KeyCode::Char('<')).await.unwrap();

        let status_for = |task_id: &str| {
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id.as_str() == task_id)
                .unwrap()
                .task
                .status
        };
        assert_eq!(status_for(&inbox_id), TaskStatus::Inbox);
        assert_eq!(status_for(&backlog_id), TaskStatus::Backlog);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("already at first column")
        );
    }

    #[tokio::test]
    async fn choosing_current_column_preserves_grouped_status() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("stay canceled")).await;
        app.store.show_view(TaskView::Columns).await.unwrap();
        app.update_status(TaskStatus::Canceled).await.unwrap();

        let selection = app.resolve_task_selection().unwrap();
        app.move_tasks_to_column(selection, "done".to_string())
            .await
            .unwrap();

        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .unwrap()
                .task
                .status,
            TaskStatus::Canceled
        );
    }

    #[tokio::test]
    async fn column_lane_header_click_moves_selected_task() {
        let mut app = test_app().await;
        let index = create_and_select_task(&mut app, test_task_draft("mouse move")).await;
        let task_id = app.store.tasks[index].task.id.clone();
        app.store.show_view(TaskView::Columns).await.unwrap();
        app.list.select_task(
            app.store
                .tasks
                .iter()
                .position(|item| item.task.id == task_id),
        );

        app.dispatch_mouse(left_click(16, 2), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id == task_id)
                .unwrap()
                .task
                .status,
            TaskStatus::Backlog
        );
    }

    #[tokio::test]
    async fn column_card_right_click_opens_status_choices() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("mouse status")).await;
        app.store.show_view(TaskView::Columns).await.unwrap();

        app.dispatch_mouse(right_click(1, 4), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(FooterChoiceMode::Status)
        );
    }

    #[tokio::test]
    async fn column_view_navigates_within_and_between_lanes() {
        let mut app = test_app().await;
        for (title, status) in [
            ("active one", "active"),
            ("active two", "active"),
            ("todo", "todo"),
        ] {
            let mut draft = test_task_draft(title);
            draft.status = status.to_string();
            app.store.create_task(draft, None).await.unwrap();
        }
        app.store.show_view(TaskView::Columns).await.unwrap();
        let active = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.title == "active one")
            .unwrap();
        let todo = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.title == "todo")
            .unwrap();
        app.list.select_task(Some(active));

        app.move_selection(1).await.unwrap();
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .unwrap()
                .task
                .status,
            TaskStatus::Active
        );
        app.move_left();
        assert_eq!(app.list.selected_task(), Some(todo));
        app.move_right().await.unwrap();
        assert_eq!(
            app.store
                .selected_task(app.list.selected_task())
                .unwrap()
                .task
                .status,
            TaskStatus::Active
        );
    }

    #[tokio::test]
    async fn column_view_keeps_task_selected_when_status_becomes_done() {
        let mut app = test_app().await;
        let mut draft = test_task_draft("finish me");
        draft.status = "active".to_string();
        create_and_select_task(&mut app, draft).await;
        app.show_view(TaskView::Columns).await.unwrap();
        let selected_id = app
            .store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .id
            .clone();

        app.update_status(TaskStatus::Done).await.unwrap();

        let selected = app.store.selected_task(app.list.selected_task()).unwrap();
        assert_eq!(selected.task.id, selected_id);
        assert_eq!(selected.task.status, TaskStatus::Done);
    }
}
