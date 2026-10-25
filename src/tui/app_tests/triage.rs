use super::custom_commands::{
    capture_config, captured_json, external_database, poll_until_complete,
};
use super::*;
use crate::config::CustomTuiCommandTarget;
use crate::tui::triage::TriageOutcome;

async fn press(app: &mut App, codes: &[KeyCode]) {
    for code in codes {
        app.dispatch_key(key(*code), (100, 30).into())
            .await
            .unwrap();
    }
}

async fn task_status(app: &App, title: &str) -> TaskStatus {
    let task_id = app
        .triage
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.item.task.title == title)
        .unwrap()
        .item
        .task
        .id
        .clone();
    app.store
        .load_task_item(&task_id)
        .await
        .unwrap()
        .unwrap()
        .task
        .status
}

fn card_title(app: &App) -> Option<&str> {
    app.triage
        .as_ref()
        .unwrap()
        .current()
        .map(|entry| entry.item.task.title.as_str())
}

async fn app_with_inbox(titles: &[&str]) -> App {
    let mut app = test_app().await;
    for title in titles {
        create_and_select_task(&mut app, test_task_draft(title)).await;
    }
    app.open_triage().await.unwrap();
    app
}

#[tokio::test]
async fn triage_status_change_decides_and_advances() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let first = card_title(&app).unwrap().to_string();

    press(&mut app, &[KeyCode::Char('t'), KeyCode::Char('b')]).await;

    assert_eq!(task_status(&app, &first).await, TaskStatus::Backlog);
    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 1);
    assert_eq!(
        session.entries[0].outcome,
        Some(TriageOutcome::Status(TaskStatus::Backlog))
    );
}

#[tokio::test]
async fn triage_priority_on_inbox_promotes_and_stays() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let first = card_title(&app).unwrap().to_string();

    press(&mut app, &[KeyCode::Char('t'), KeyCode::Char('h')]).await;

    assert_eq!(task_status(&app, &first).await, TaskStatus::Todo);
    assert_eq!(card_title(&app), Some(first.as_str()));
    assert_eq!(app.triage.as_ref().unwrap().decided_count(), 0);

    press(&mut app, &[KeyCode::Char('t'), KeyCode::Char('t')]).await;

    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 1);
    assert_eq!(
        session.entries[0].outcome,
        Some(TriageOutcome::Status(TaskStatus::Todo))
    );
}

#[tokio::test]
async fn triage_current_status_on_stale_todo_keeps_it_without_writing() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let mut draft = test_task_draft("Stale todo");
    draft.status = "todo".to_string();
    let selected = create_and_select_task(&mut app, draft).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE id = ?")
        .bind("2000-01-01T00:00:00Z")
        .bind(&task_id)
        .execute(&pool)
        .await
        .unwrap();
    app.open_triage().await.unwrap();
    assert_eq!(card_title(&app), Some("Stale todo"));

    press(&mut app, &[KeyCode::Char('t'), KeyCode::Char('t')]).await;

    let session = app.triage.as_ref().unwrap();
    assert!(session.at_end());
    assert_eq!(
        session.entries[0].outcome,
        Some(TriageOutcome::Kept(TaskStatus::Todo))
    );
    let item = app.store.load_task_item(&task_id).await.unwrap().unwrap();
    assert_eq!(item.task.status, TaskStatus::Todo);
    assert_eq!(item.task.queue_activity_at, "2000-01-01T00:00:00Z");
    assert!(item.notes.is_empty());

    press(&mut app, &[KeyCode::Esc]).await;
    app.open_triage().await.unwrap();
    assert!(app.triage.is_none(), "a kept task stays out of triage");
}

#[tokio::test]
async fn triage_undo_of_keep_clears_the_review_mark() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let mut draft = test_task_draft("Stale active");
    draft.status = "active".to_string();
    let selected = create_and_select_task(&mut app, draft).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE id = ?")
        .bind("2000-01-01T00:00:00Z")
        .bind(&task_id)
        .execute(&pool)
        .await
        .unwrap();
    app.open_triage().await.unwrap();

    press(&mut app, &[KeyCode::Char('t'), KeyCode::Char('a')]).await;
    assert!(app.triage.as_ref().unwrap().at_end());
    press(&mut app, &[KeyCode::Char('u')]).await;

    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 0);
    assert_eq!(session.entries[0].outcome, None);
    assert!(
        app.store
            .database()
            .triage_reviews()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn triage_navigation_leaves_task_undecided() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let first = card_title(&app).unwrap().to_string();

    press(&mut app, &[KeyCode::Char('j')]).await;

    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 1);
    assert_eq!(session.decided_count(), 0);
    assert!(session.entries[0].passed);
    assert_eq!(task_status(&app, &first).await, TaskStatus::Inbox);
}

#[tokio::test]
async fn triage_undo_restores_task_and_card() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let first = card_title(&app).unwrap().to_string();

    press(&mut app, &[KeyCode::Char('x')]).await;
    assert_eq!(task_status(&app, &first).await, TaskStatus::Canceled);

    press(&mut app, &[KeyCode::Char('u')]).await;

    assert_eq!(task_status(&app, &first).await, TaskStatus::Inbox);
    let session = app.triage.as_ref().unwrap();
    assert_eq!(card_title(&app), Some(first.as_str()));
    assert_eq!(session.entries[0].outcome, None);
}

#[tokio::test]
async fn triage_repeated_decision_key_cancels_only_one_task() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let first = card_title(&app).unwrap().to_string();

    press(&mut app, &[KeyCode::Char('x'), KeyCode::Char('x')]).await;

    let second = card_title(&app).unwrap().to_string();
    assert_ne!(first, second);
    assert_eq!(task_status(&app, &first).await, TaskStatus::Canceled);
    assert_eq!(task_status(&app, &second).await, TaskStatus::Inbox);
}

#[tokio::test]
async fn leaving_triage_restores_previous_view() {
    let mut app = app_with_inbox(&["First"]).await;
    assert_eq!(app.store.view_state.query, TaskQuery::All);

    press(&mut app, &[KeyCode::Esc]).await;

    assert!(app.triage.is_none());
    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
}

#[tokio::test]
async fn triage_priority_choice_promotes_and_stays() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let first = card_title(&app).unwrap().to_string();

    press(&mut app, &[KeyCode::Char('!'), KeyCode::Char('h')]).await;

    assert!(app.footer_choice.is_none());
    assert_eq!(task_status(&app, &first).await, TaskStatus::Todo);
    assert_eq!(card_title(&app), Some(first.as_str()));
    assert_eq!(app.triage.as_ref().unwrap().decided_count(), 0);
}

#[tokio::test]
async fn triage_status_choice_decides_and_advances() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let first = card_title(&app).unwrap().to_string();

    press(&mut app, &[KeyCode::Char('s'), KeyCode::Char('d')]).await;

    assert_eq!(task_status(&app, &first).await, TaskStatus::Done);
    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 1);
    assert_eq!(
        session.entries[0].outcome,
        Some(TriageOutcome::Status(TaskStatus::Done))
    );
}

#[tokio::test]
async fn triage_arrow_keys_move_between_cards() {
    let mut app = app_with_inbox(&["First", "Second"]).await;

    press(&mut app, &[KeyCode::Right]).await;
    assert_eq!(app.triage.as_ref().unwrap().cursor, 1);

    press(&mut app, &[KeyCode::Left]).await;
    assert_eq!(app.triage.as_ref().unwrap().cursor, 0);
}

#[tokio::test]
async fn triage_custom_command_targets_card_task_and_stays() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("capture.json");
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    app.set_config(capture_config(
        &capture,
        CustomTuiCommandTarget::Focused,
        vec!["z d".to_string()],
    ));
    app.open_triage().await.unwrap();
    press(&mut app, &[KeyCode::Char('j')]).await;
    let card = card_title(&app).unwrap().to_string();
    let card_id = app
        .triage
        .as_ref()
        .unwrap()
        .current()
        .unwrap()
        .item
        .task
        .id
        .clone();
    let other = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id != card_id)
        .unwrap();
    app.list.select_task(Some(other));

    press(&mut app, &[KeyCode::Char('z'), KeyCode::Char('d')]).await;
    poll_until_complete(&mut app).await;

    let json = captured_json(&capture);
    assert_eq!(json["selection"]["primary"]["task"]["id"], card_id.as_str());
    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 1);
    assert_eq!(session.decided_count(), 0);
    assert_eq!(card_title(&app), Some(card.as_str()));
}

#[tokio::test]
async fn triage_custom_command_status_change_decides_and_advances() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    app.set_config(capture_config(
        &dir.path().join("capture.json"),
        CustomTuiCommandTarget::Focused,
        vec!["z d".to_string()],
    ));
    app.open_triage().await.unwrap();
    let card_id = app
        .triage
        .as_ref()
        .unwrap()
        .current()
        .unwrap()
        .item
        .task
        .id
        .clone();

    press(&mut app, &[KeyCode::Char('z'), KeyCode::Char('d')]).await;
    external_database(&app)
        .await
        .update_task(
            &app.store.active_workspace,
            &card_id,
            crate::operations::TaskUpdate {
                status: Some("todo".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    poll_until_complete(&mut app).await;

    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 1);
    assert_eq!(
        session.entries[0].outcome,
        Some(TriageOutcome::Status(TaskStatus::Todo))
    );
}

#[tokio::test]
async fn triage_paging_keys_scroll_the_card_and_reset_on_the_next_card() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let session = app.triage.as_ref().unwrap();
    session.scroll_max.set(30);
    session.scroll_viewport.set(10);

    app.dispatch_key(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        (100, 30).into(),
    )
    .await
    .unwrap();
    press(&mut app, &[KeyCode::PageDown]).await;

    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.cursor, 0);
    assert_eq!(session.card_scroll(), 14);

    press(&mut app, &[KeyCode::Char('j')]).await;
    assert_eq!(app.triage.as_ref().unwrap().card_scroll(), 0);
}

#[tokio::test]
async fn triage_opened_in_a_project_scope_covers_only_that_project() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    create_and_select_task(&mut app, test_task_draft("Elsewhere")).await;
    let mut draft = test_task_draft("In project");
    draft.project = Some("mobile-app".to_string());
    create_and_select_task(&mut app, draft).await;
    app.show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();

    app.open_triage().await.unwrap();

    let session = app.triage.as_ref().unwrap();
    assert_eq!(session.project.as_deref(), Some("mobile-app"));
    assert_eq!(card_title(&app), Some("In project"));
    assert_eq!(session.entries.len(), 1);
    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );

    press(&mut app, &[KeyCode::Esc]).await;
    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
}

#[tokio::test]
async fn triage_owns_wheel_over_sidebar_and_paging() {
    let mut app = app_with_inbox(&["First", "Second"]).await;
    let session = app.triage.as_ref().unwrap();
    session.scroll_max.set(100);
    session.scroll_viewport.set(10);
    let sidebar = app.list.selected_sidebar();
    let task = app.list.selected_task();
    let focus = app.list.focus();
    app.dispatch_mouse(wheel_down(3, 5), (140, 24).into())
        .await
        .unwrap();
    assert_eq!(app.triage.as_ref().unwrap().card_scroll(), 3);
    app.dispatch_key(key(KeyCode::PageDown), (140, 24).into())
        .await
        .unwrap();
    assert!(app.triage.as_ref().unwrap().card_scroll() > 3);
    assert_eq!(app.triage.as_ref().unwrap().cursor, 0);
    assert_eq!(app.list.selected_sidebar(), sidebar);
    assert_eq!(app.list.selected_task(), task);
    assert_eq!(app.list.focus(), focus);
}
