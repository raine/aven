use super::*;

#[tokio::test]
async fn default_startup_opens_all_projects() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    create_task_in_project(&mut store, "mobile task", "mobile-app").await;

    let reopened = TuiStore::new(store.database.clone(), store.active_workspace.clone())
        .await
        .unwrap();

    assert_eq!(reopened.view_state.view, TaskView::Queue);
    assert_eq!(reopened.view_state.scope, TaskScope::Workspace);
    assert_eq!(reopened.tasks.len(), 1);
}

#[tokio::test]
async fn initial_project_opens_project_view() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    store.create_project("Ops".to_string()).await.unwrap();
    create_task_in_project(&mut store, "mobile task", "mobile-app").await;
    store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "ops task".to_string(),
                project: Some("ops".to_string()),
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let reopened = TuiStore::new_with_view_state(
        store.database.clone(),
        store.active_workspace.clone(),
        TaskViewState {
            scope: TaskScope::Project("mobile-app".to_string()),
            ..TaskViewState::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        reopened.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(reopened.view_state.view, TaskView::Queue);
    assert_eq!(reopened.tasks.len(), 1);
    assert_eq!(reopened.tasks[0].task.title, "mobile task");
}

#[tokio::test]
async fn delete_project_ignores_tasks_in_other_workspace() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    crate::projects::create_project_in_workspace(&mut conn, &other.id, "Mobile App")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at)
         VALUES (?, 'other-task', 'Other task', '', (SELECT id FROM projects WHERE workspace_id = ? AND key = 'mobile-app'), 'todo', 'none', 't', 't')",
    )
    .bind(&other.id)
    .bind(&other.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    store.delete_project("mobile-app").await.unwrap();

    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    let other_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM projects WHERE workspace_id = ? AND key = 'mobile-app'",
    )
    .bind(&other.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(other_count, 1);
}

#[tokio::test]
async fn delete_project_uses_store_workspace() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    create_task_in_project(&mut store, "Default task", "mobile-app").await;
    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    crate::projects::create_project_in_workspace(&mut conn, &other.id, "Mobile App")
        .await
        .unwrap();
    drop(conn);
    store
        .switch_workspace("client-work".to_string())
        .await
        .unwrap();

    store.delete_project("mobile-app").await.unwrap();

    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    let default_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM projects WHERE workspace_id = ? AND key = 'mobile-app'",
    )
    .bind(crate::workspaces::DEFAULT_WORKSPACE_ID)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(default_count, 1);
}

#[tokio::test]
async fn deferred_task_intake_retains_spawned_workspace() {
    let mut store = test_store().await;
    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    crate::projects::create_project_in_workspace(
        &mut conn,
        &store.active_workspace.id,
        "Default Only",
    )
    .await
    .unwrap();
    let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    drop(conn);

    let intake = store.spawn_task_intake(
        crate::config::TaskIntakeConfig {
            command: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "sleep 0.1; printf '%s' '{\"title\":\"Deferred task\",\"project\":\"default-only\"}'"
                    .to_string(),
            ],
            timeout_seconds: Some(5),
            system_prompt: None,
        },
        "deferred task".to_string(),
        None,
    );
    store.switch_workspace(other.key).await.unwrap();

    let draft = intake.await.unwrap().unwrap();

    assert_eq!(store.active_workspace.id, other.id);
    assert_eq!(draft.task.project.as_deref(), Some("default-only"));
}

#[tokio::test]
async fn workspace_administration_preserves_active_workspace_and_task_ownership() {
    let mut store = test_store().await;
    let (_, selected) = store
        .create_task(task_draft("Default workspace task"), None)
        .await
        .unwrap();
    assert!(selected.is_some());
    let task_id = store.tasks[0].task.id.clone();
    let active_id = store.active_workspace.id.clone();

    let create_message = store
        .create_workspace("Client Work".to_string())
        .await
        .unwrap();

    assert_eq!(store.active_workspace.id, active_id);
    assert_eq!(store.active_workspace.key, "default");
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.id, task_id);
    assert_eq!(
        create_message,
        "created workspace Client Work (client-work)"
    );
    assert_eq!(
        store
            .create_workspace("hello-world".to_string())
            .await
            .unwrap(),
        "created workspace hello-world"
    );

    let inactive_message = store
        .rename_workspace("client-work".to_string(), "Consulting".to_string())
        .await
        .unwrap();

    assert_eq!(store.active_workspace.id, active_id);
    assert_eq!(store.active_workspace.key, "default");
    assert_eq!(store.tasks[0].task.id, task_id);
    assert_eq!(
        inactive_message,
        "renamed workspace to consulting (Consulting); active workspace remains default"
    );

    let active_message = store
        .rename_workspace("default".to_string(), "Personal".to_string())
        .await
        .unwrap();

    assert_eq!(store.active_workspace.id, active_id);
    assert_eq!(store.active_workspace.key, "personal");
    assert_eq!(store.active_workspace.name, "Personal");
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.id, task_id);
    assert_eq!(
        active_message,
        "renamed active workspace to personal (Personal)"
    );
    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    let task_workspace: crate::ids::WorkspaceId =
        sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(task_workspace, active_id);
}

#[tokio::test]
async fn switch_workspace_refreshes_workspace_scoped_state() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let mut store = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();
    let (_, selected) = store
        .create_task(task_draft("Default workspace task"), None)
        .await
        .unwrap();
    assert!(selected.is_some());
    assert_eq!(store.tasks.len(), 1);

    let mut conn = pool.acquire().await.unwrap();
    let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    drop(conn);

    store.view_state.scope = TaskScope::Project("missing".to_string());
    store.show_view(TaskView::Todo).await.unwrap();
    store.view_state.filter_modifiers.label = Some("default-label".to_string());
    store.view_state.filter_modifiers.priority = Some("urgent".to_string());
    store.view_state.projection_origin =
        super::super::TaskProjectionOrigin::ExactTasks(vec![crate::test_support::task_id(
            "task-1",
        )]);
    store.view_state.filter_modifiers.include_deleted = true;

    let selected = store.switch_workspace(other.key.clone()).await.unwrap();

    assert!(selected.is_none());
    assert_eq!(store.active_workspace.key, "client-work");
    assert_eq!(store.view_state.scope, TaskScope::Workspace);
    assert_eq!(store.view_state.view, TaskView::Todo);
    assert_eq!(
        store.view_state.projection_origin,
        super::super::TaskProjectionOrigin::NamedView
    );
    assert_eq!(
        store.view_state.filter_modifiers,
        TaskFilterModifiers::default()
    );
    assert!(store.tasks.is_empty());
    assert!(
        store
            .workspaces
            .iter()
            .any(|workspace| workspace.key == "client-work")
    );

    reset_default_workspace(&pool).await;
}

#[tokio::test]
async fn workspace_picker_order_is_stable_when_active_workspace_changes() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let mut store = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    crate::workspaces::create_workspace(&mut conn, "Team Space")
        .await
        .unwrap();
    drop(conn);
    store.refresh(None).await.unwrap();

    let before = store
        .workspace_picker_items()
        .into_iter()
        .map(|item| item.value)
        .collect::<Vec<_>>();
    store
        .switch_workspace("client-work".to_string())
        .await
        .unwrap();
    let after = store
        .workspace_picker_items()
        .into_iter()
        .map(|item| item.value)
        .collect::<Vec<_>>();

    assert_eq!(before, vec!["client-work", "default", "team-space"]);
    assert_eq!(after, before);

    reset_default_workspace(&pool).await;
}

#[tokio::test]
async fn refresh_reads_store_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let mut store = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();
    let (_, selected) = store
        .create_task(task_draft("Default workspace task"), None)
        .await
        .unwrap();
    assert!(selected.is_some());

    let mut conn = pool.acquire().await.unwrap();
    let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
         VALUES (?, ?, 'client', 'Client', 'CLI', 't', 't')",
    )
    .bind(crate::ids::new_id())
    .bind(&other.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO labels(workspace_id, name, created_at) VALUES (?, 'client-label', 't')",
    )
    .bind(&other.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at)
         VALUES (?, ?, 'Client workspace task', '', (SELECT id FROM projects WHERE workspace_id = ? AND key = 'client'), 'todo', 'none', 't', 't')",
    )
    .bind(&other.id)
    .bind(crate::ids::new_id())
    .bind(&other.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    store.active_workspace = other;
    store.refresh(None).await.unwrap();

    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Client workspace task");
    assert!(store.projects.iter().any(|project| project.key == "client"));
    assert_eq!(store.labels, vec!["client-label".to_string()]);
    assert_eq!(store.counts.open, 1);
    assert_eq!(store.counts.todo, 1);

    reset_default_workspace(&pool).await;
}
