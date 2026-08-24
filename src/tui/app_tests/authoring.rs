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
    assert!(toast_message(&app).is_some_and(|message| message.ends_with(" · u undo")));
}

#[tokio::test]
async fn create_more_reopens_blank_composer_with_retained_defaults() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    state.title = LineEdit::new("First rapid task".to_string());
    state.description.lines = vec!["First details".to_string()];
    state.apply_status_choice("todo");
    state.apply_priority_choice("high");
    state.labels = vec!["rapid".to_string()];
    state.is_epic = true;

    app.handle_overlay_key(ctrl_g()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.focus == AddTaskStep::Title
                && state.title.text.is_empty()
                && state.description.lines == vec![String::new()]
                && state.effective_status() == "todo"
                && state.priority.value() == "high"
                && state.labels == vec!["rapid".to_string()]
                && !state.is_epic
                && !state.create_more
    ));
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected repeated composer");
    };
    state.title = LineEdit::new("Second rapid task".to_string());
    app.handle_overlay_key(ctrl_g()).await.unwrap();

    let rows = sqlx::query_as::<_, (String, String, String, String, i64)>(
        "SELECT t.title, t.description, t.status, t.priority, t.is_epic
         FROM tasks t WHERE t.title IN ('First rapid task', 'Second rapid task')
         ORDER BY t.title",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        (
            "First rapid task".to_string(),
            "First details".to_string(),
            "todo".to_string(),
            "high".to_string(),
            1,
        )
    );
    assert_eq!(
        rows[1],
        (
            "Second rapid task".to_string(),
            String::new(),
            "todo".to_string(),
            "high".to_string(),
            0,
        )
    );
    for title in ["First rapid task", "Second rapid task"] {
        let task = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == title)
            .expect("rapid task should be visible");
        assert_eq!(task.labels, vec!["rapid".to_string()]);
    }
    assert!(toast_message(&app).is_some_and(|message| message.starts_with("created task ")));

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.mode == crate::tui::overlay::AddTaskMode::ConfirmDiscard
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn create_more_preserves_derived_status_semantics() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    state.title = LineEdit::new("Derived todo".to_string());
    state.apply_priority_choice("high");
    app.handle_overlay_key(ctrl_g()).await.unwrap();

    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected repeated composer");
    };
    assert!(state.status_is_automatic());
    assert_eq!(state.effective_status(), "todo");
    state.title = LineEdit::new("Derived inbox".to_string());
    state.apply_priority_choice("none");
    app.handle_overlay_key(ctrl_g()).await.unwrap();

    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT title, status, priority FROM tasks
         WHERE title IN ('Derived todo', 'Derived inbox') ORDER BY title",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "Derived inbox".to_string(),
                "inbox".to_string(),
                "none".to_string(),
            ),
            (
                "Derived todo".to_string(),
                "todo".to_string(),
                "high".to_string(),
            ),
        ]
    );
}

#[tokio::test]
async fn create_more_refresh_failure_resets_committed_draft_without_duplication() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    state.title = LineEdit::new("Committed once".to_string());
    state.description.lines = vec!["Do not retry".to_string()];
    app.store.fail_next_refresh();

    app.handle_overlay_key(ctrl_g()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.focus == AddTaskStep::Title
                && state.title.text.is_empty()
                && state.description.lines == vec![String::new()]
                && !state.create_more
    ));
    assert!(toast_message(&app).is_some_and(|message| {
        message.starts_with("created task ") && message.contains("list refresh failed")
    }));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'Committed once'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn create_more_precommit_failure_preserves_the_exact_draft() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    sqlx::query(
        "CREATE TRIGGER reject_repeat_undo BEFORE INSERT ON tui_undo_entries
         BEGIN SELECT RAISE(FAIL, 'injected undo failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    state.title = LineEdit::new("Retry safely".to_string());
    state.description.lines = vec!["Preserve this".to_string()];
    state.apply_priority_choice("urgent");

    let error = app.handle_overlay_key(ctrl_g()).await.unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.title.text == "Retry safely"
                && state.description.lines == vec!["Preserve this".to_string()]
                && state.priority.value() == "urgent"
                && !state.create_more
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'Retry safely'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
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
    assert!(matches!(
        app.view().overlay,
        Some(OverlayView::AddTask(state)) if !state.create_more_available
    ));
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
        state.apply_status_choice(status);
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
        state.apply_status_choice(status);
        state.labels = vec![format!("unpersisted-{status}")];
        state.set_repeat_rule("daily".to_string());
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Status
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("recurring tasks require an open initial status: inbox, backlog, todo, or active")
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
        Some(OverlayState::AddTask(state)) if state.effective_status() == "inbox"
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
        Some(OverlayState::AddTask(state)) if state.effective_status() == "active"
    ));
    assert_eq!(toast_message(&app), None);

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
        Some(OverlayState::AddTask(state)) if state.priority.value() == "none"
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
        Some(OverlayState::AddTask(state)) if state.priority.value() == "high"
    ));
    assert_eq!(toast_message(&app), None);

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.title, "Fix release");
    assert_eq!(app.store.tasks[selected].task.priority, TaskPriority::High);
}

#[tokio::test]
async fn add_task_priority_picker_and_shortcut_share_status_derivation() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    state.focus = AddTaskStep::Priority;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer priority picker");
    };
    let crate::tui::overlay::AddTaskMode::Picker { state: picker, .. } = &mut state.mode else {
        panic!("expected priority picker");
    };
    picker.selected = picker
        .items
        .iter()
        .position(|item| item.value == "high")
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.priority.value() == "high" && state.effective_status() == "todo"
    ));

    app.handle_overlay_key(ctrl_r()).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.priority.value() == "none" && state.effective_status() == "inbox"
    ));
}

#[tokio::test]
async fn add_task_status_picker_auto_restores_derived_status() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    state.apply_priority_choice("high");
    state.apply_status_choice("inbox");
    state.focus = AddTaskStep::Status;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer status picker");
    };
    let crate::tui::overlay::AddTaskMode::Picker { state: picker, .. } = &mut state.mode else {
        panic!("expected status picker");
    };
    assert!(picker.items.iter().any(|item| item.label == "Auto (todo)"));
    picker.selected = picker
        .items
        .iter()
        .position(|item| item.value == crate::tui::store::ADD_TASK_STATUS_AUTO_VALUE)
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.status_is_automatic() && state.effective_status() == "todo"
    ));
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
async fn add_task_label_draft_does_not_persist_before_task_creation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "Write docs").await;
    app.handle_overlay_key(ctrl_l()).await.unwrap();
    type_chars(&mut app, "Orphan Label").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let before_cancel: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM labels WHERE workspace_id = (SELECT id FROM workspaces LIMIT 1) AND name = 'orphan-label'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before_cancel, 0);

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    let after_cancel: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM labels WHERE workspace_id = (SELECT id FROM workspaces LIMIT 1) AND name = 'orphan-label'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_cancel, 0);
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn add_task_labels_escape_returns_to_add_task_only_dialog() {
    let mut app = test_app().await;
    app.intake.enter_add_task_only();
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
        query: TaskQuery::Search,
        projection_origin: crate::tui::store::TaskProjectionOrigin::ExactTasks(vec![
            task_id.clone(),
        ]),
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
    assert!(app.list.navigation_is_empty());
    assert!(app.detail.state_mut().unwrap().history.pop().is_none());

    app.handle_normal_key(KeyCode::Esc).await.unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.store.view_state.query, TaskQuery::Search);
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
        query: TaskQuery::RecentActions,
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
async fn recent_action_opens_its_task_detail() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("recent action task")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.store.view_state.query = TaskQuery::RecentActions;
    app.refresh().await.unwrap();
    let action_index = app
        .store
        .recent_actions
        .iter()
        .position(|action| action.entity_type == "task" && action.entity_id == task_id.to_string())
        .unwrap();
    app.list.select_task(Some(action_index));

    app.handle_normal_key(KeyCode::Enter).await.unwrap();

    assert!(app.detail.is_active());
    assert_eq!(app.store.view_state.query, TaskQuery::Search);
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.id, task_id);
    assert_eq!(app.list.selected_task(), Some(0));
}

#[tokio::test]
async fn recent_action_detail_returns_to_the_selected_action() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("first recent task")).await;
    create_and_select_task(&mut app, test_task_draft("second recent task")).await;
    app.store.view_state.query = TaskQuery::RecentActions;
    app.refresh().await.unwrap();
    let selected_index = 1;
    let change_id = app.store.recent_actions[selected_index].change_id.clone();
    app.list.select_task(Some(selected_index));
    app.list.set_task_offset(1);

    app.handle_normal_key(KeyCode::Enter).await.unwrap();
    app.handle_normal_key(KeyCode::Esc).await.unwrap();

    assert!(!app.detail.is_active());
    assert_eq!(app.store.view_state.query, TaskQuery::RecentActions);
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.recent_actions[selected].change_id, change_id);
    assert_eq!(app.list.task_offset(), 1);
}

#[tokio::test]
async fn unavailable_recent_action_task_reports_feedback() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("available task")).await;
    app.store.view_state.query = TaskQuery::RecentActions;
    app.refresh().await.unwrap();
    let mut action = app.store.recent_actions[0].clone();
    action.entity_type = "task".to_string();
    action.entity_id = crate::ids::TaskId::new().to_string();
    app.store.recent_actions = vec![action];
    app.list.select_task(Some(0));

    app.handle_normal_key(KeyCode::Enter).await.unwrap();

    assert!(!app.detail.is_active());
    assert_eq!(app.store.view_state.query, TaskQuery::RecentActions);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("recent action task is unavailable")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
}

#[tokio::test]
async fn non_task_recent_action_reports_missing_task_identity() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task action")).await;
    app.store.view_state.query = TaskQuery::RecentActions;
    app.refresh().await.unwrap();
    let mut action = app.store.recent_actions[0].clone();
    action.entity_type = "project".to_string();
    app.store.recent_actions = vec![action];
    app.list.select_task(Some(0));

    app.handle_normal_key(KeyCode::Enter).await.unwrap();

    assert!(!app.detail.is_active());
    assert_eq!(app.store.view_state.query, TaskQuery::RecentActions);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("recent action has no task identity")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
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
    app.intake.enter_add_task_only();
    app.begin_add_task().await.unwrap();

    let view = app.view();

    assert_eq!(view.surface, ViewSurface::AddTask);
}

#[tokio::test]
async fn add_task_only_render_skips_normal_tui() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Existing queue task")).await;
    app.intake.enter_add_task_only();
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
    app.intake.enter_add_task_only();
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
async fn add_task_project_picker_creates_and_selects_project() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "Write docs").await;
    app.handle_overlay_key(ctrl_p()).await.unwrap();
    type_chars(&mut app, "Mobile App").await;

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if matches!(
                &state.mode,
                AddTaskMode::Picker { state: picker, .. }
                    if picker.items.iter().any(|item| {
                        item.label == "+ Create project \"Mobile App\""
                            && crate::tui::store::create_project_picker_name(&item.value)
                                == Some("Mobile App")
                    })
            )
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.selected_project.as_deref() == Some("mobile-app")
                && state.project == "mobile-app"
    ));
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.project_key, "mobile-app");
}

#[tokio::test]
async fn add_task_project_creation_cancel_restores_inference_selection() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = &mut app.overlay else {
        panic!("expected add task overlay");
    };
    state.project = "inferred-project".to_string();
    state.inferred_project = Some("inferred-project".to_string());

    app.handle_overlay_key(ctrl_p()).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if matches!(
                &state.mode,
                AddTaskMode::Picker { state: picker, .. }
                    if picker.items.iter().any(|item| item.value.is_empty() && item.selected)
            )
    ));
    type_chars(&mut app, "New Project").await;
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.selected_project.is_none()
                && state.inferred_project.as_deref() == Some("inferred-project")
                && state.project == "inferred-project"
                && state.mode == AddTaskMode::Compose
    ));
}

#[tokio::test]
async fn add_task_project_creation_error_keeps_picker_filter_open() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    app.handle_overlay_key(ctrl_p()).await.unwrap();
    type_chars(&mut app, "---").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if matches!(
                &state.mode,
                AddTaskMode::Picker { state: picker, .. }
                    if picker.filter.as_str() == "---"
                        && picker.items.iter().any(|item| {
                            item.label == "+ Create project \"---\""
                                && crate::tui::store::create_project_picker_name(&item.value)
                                    == Some("---")
                        })
            )
    ));
    assert!(toast_message(&app).is_some_and(|message| message.contains("invalid-project")));
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
    app.intake.enter_add_task_only();
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
    app.store.view_state.query = TaskQuery::RecentActions;
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
    app.store.view_state.query = TaskQuery::RecentActions;
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
                metadata: Vec::new(),
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
async fn successful_mutation_adds_undo_toast_action() {
    let mut app = test_app().await;
    let result = app
        .store
        .create_task(test_task_draft("Toast undo"), None)
        .await
        .unwrap();

    app.apply_mutation_result(crate::tui::store::MutationMessage::new(result.0, result.1));

    assert!(toast_message(&app).is_some_and(|message| message.ends_with(" · u undo")));
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
    assert!(toast_message(&app).is_some_and(|message| { message == "adding task in background" }));
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
    assert!(toast_message(&app).is_some_and(|message| { message == "adding task in background" }));
}

#[tokio::test]
async fn add_task_only_ctrl_n_exits_immediately() {
    let mut app = test_app().await;
    app.intake.enter_add_task_only();

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
    app.intake.enter_add_task_only();

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
async fn add_task_composer_creates_epic_container() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    state.title = crate::tui::overlay::LineEdit::new("Plan release".to_string());
    state.focus = AddTaskStep::Epic;

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let Some(OverlayState::AddTask(state)) = app.overlay.as_mut() else {
        panic!("expected composer");
    };
    assert!(state.is_epic);
    assert_eq!(state.focus, AddTaskStep::Epic);
    assert!(matches!(
        state.mode,
        crate::tui::overlay::AddTaskMode::Compose
    ));
    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .await
        .unwrap();

    let created = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.title == "Plan release")
        .expect("created task");
    assert!(created.task.is_epic);
}

#[tokio::test]
async fn edit_epic_picker_toggles_container_state() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Plan release")).await;
    app.list.select_task(Some(selected));

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Up)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let task = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.title == "Plan release")
        .expect("edited task");
    assert!(task.task.is_epic);

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let task = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.title == "Plan release")
        .expect("edited task");
    assert!(!task.task.is_epic);
}

#[tokio::test]
async fn edit_epic_picker_keeps_container_with_children() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, _child_ids) =
        create_epic_with_children(&mut app, &pool, "Plan release", &["Ship build"]).await;
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .expect("parent epic");
    app.list.select_task(Some(parent));

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == parent_id)
            .expect("parent epic")
            .task
            .is_epic
    );
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state))
            if matches!(state.intent, crate::tui::overlay::PickerIntent::EditEpic { .. })
    ));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("remove all epic children before turning the container off")
    );
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
async fn add_task_ctrl_a_moves_title_cursor_to_start() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "Write docs").await;

    app.handle_overlay_key(ctrl_a()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.mode == crate::tui::overlay::AddTaskMode::Compose
                && state.title.cursor == 0
                && state.title.text == "Write docs"
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
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Epic
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
async fn ctrl_g_creates_and_reopens_the_composer() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "First task").await;

    app.dispatch_key(ctrl_g(), (100, 30).into()).await.unwrap();

    assert_eq!(app.store.tasks.len(), 1);
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.focus == AddTaskStep::Title
                && state.title.as_str().is_empty()
                && !state.create_more
    ));

    type_chars(&mut app, "Second task").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(app.store.tasks.len(), 2);
    assert!(app.overlay.is_none());
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
    app.store.show_view(TaskQuery::Upcoming).await.unwrap();

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
    app.store.show_view(TaskQuery::Upcoming).await.unwrap();

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
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
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
async fn add_note_targets_one_marked_task_instead_of_cursor() {
    let mut app = test_app().await;
    let marked = create_and_select_task(&mut app, test_task_draft("Marked note target")).await;
    let marked_id = app.store.tasks[marked].task.id.clone();
    let cursor = create_and_select_task(&mut app, test_task_draft("Cursor note target")).await;
    let cursor_id = app.store.tasks[cursor].task.id.clone();
    assert_ne!(marked_id, cursor_id);
    app.list.mark(marked_id.clone());

    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

    let task_id = match &app.overlay {
        Some(OverlayState::MultilineInput(state)) => match &state.intent {
            MultilineIntent::AddNote { task_id, .. } => task_id.clone(),
            intent => panic!("expected add-note intent, got {intent:?}"),
        },
        overlay => panic!("expected add-note overlay, got {overlay:?}"),
    };
    assert_eq!(task_id, marked_id);

    type_chars(&mut app, "Marked detail").await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    let marked = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == marked_id)
        .unwrap();
    let cursor = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id != marked_id)
        .unwrap();
    assert_eq!(marked.notes.len(), 1);
    assert!(cursor.notes.is_empty());
}

#[tokio::test]
async fn add_note_rejects_multiple_marked_tasks() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("First note target")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("Second note target")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id);
    app.list.mark(second_id);

    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("note requires one task · 2 tasks marked")
    );
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

#[tokio::test]
async fn add_task_reports_title_caret_to_terminal_cursor() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "ab").await;

    let buffer = render_app_buffer(&mut app, 120, 30);
    let cursor = app.widgets.text_cursor.expect("caret reported");

    assert_eq!(buffer[(cursor.x, cursor.y)].symbol(), " ");
    assert_eq!(buffer[(cursor.x - 2, cursor.y)].symbol(), "a");
    assert_eq!(buffer[(cursor.x - 1, cursor.y)].symbol(), "b");
}

#[tokio::test]
async fn add_task_caret_follows_wide_characters() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "한글").await;

    let buffer = render_app_buffer(&mut app, 120, 30);
    let cursor = app.widgets.text_cursor.expect("caret reported");

    assert_eq!(buffer[(cursor.x - 4, cursor.y)].symbol(), "한");
    assert_eq!(buffer[(cursor.x - 2, cursor.y)].symbol(), "글");
}

#[tokio::test]
async fn add_task_title_scrolls_wide_characters_within_the_dialog() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, &"한".repeat(80)).await;

    let buffer = render_app_buffer(&mut app, 100, 30);
    let cursor = app.widgets.text_cursor.expect("caret reported");
    let caret_row = (0..buffer.area.width)
        .map(|column| buffer[(column, cursor.y)].symbol())
        .collect::<String>();

    assert!(cursor.x < buffer.area.width, "caret stayed on screen");
    assert_eq!(buffer[(cursor.x - 2, cursor.y)].symbol(), "한");
    assert!(
        caret_row.matches('한').count() < 80,
        "title scrolled instead of overflowing: {caret_row:?}"
    );
}
