use super::*;

use crate::choices::{PRIORITIES, TaskPriority, TaskStatus};
use crate::operations::{TaskDraft, TaskUpdate};
use crate::query::SortDirection;

async fn test_store() -> TuiStore {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    TuiStore::new(pool).await.unwrap()
}

async fn reset_default_workspace(pool: &SqlitePool) {
    let mut conn = pool.acquire().await.unwrap();
    let default = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    crate::workspaces::set_active_workspace(default);
}

async fn test_store_with_pool() -> (tempfile::TempDir, sqlx::SqlitePool, TuiStore) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let store = TuiStore::new(pool.clone()).await.unwrap();
    (dir, pool, store)
}

async fn create_selected_task(store: &mut TuiStore, title: &str) -> (String, usize) {
    let (_, selected) = store
        .create_task(
            TaskDraft {
                title: title.to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "none".to_string(),
                labels: Vec::new(),
                is_epic: false,
            },
            None,
        )
        .await
        .unwrap();
    let selected = selected.unwrap();
    let task_id = store.tasks[selected].task.id.clone();
    (task_id, selected)
}

async fn seed_title_conflict(pool: &SqlitePool, task_id: &str) {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, 'title', NULL, 'local title', 'remote title', NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(task_id)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
}

fn task_draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: None,
        status: "inbox".to_string(),
        priority: "none".to_string(),
        labels: Vec::new(),
        is_epic: false,
    }
}

async fn set_task_timestamps(
    pool: &SqlitePool,
    workspace_id: &str,
    task_id: &str,
    queue_activity_at: &str,
    updated_at: Option<&str>,
) {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(queue_activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    if let Some(updated) = updated_at {
        sqlx::query("UPDATE tasks SET updated_at = ? WHERE workspace_id = ? AND id = ?")
            .bind(updated)
            .bind(workspace_id)
            .bind(task_id)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
}

async fn create_selected_task_with_stale_queue_activity(
    store: &mut TuiStore,
    pool: &SqlitePool,
    title: &str,
) -> (String, usize) {
    let (_, selected) = store.create_task(task_draft(title), None).await.unwrap();
    let selected = selected.unwrap();
    let task_id = store.tasks[selected].task.id.clone();
    let workspace_id = store.active_workspace.id.clone();
    set_task_timestamps(pool, &workspace_id, &task_id, "1970-01-01T00:00:00Z", None).await;
    store.refresh(Some(&task_id)).await.unwrap();
    (task_id, selected)
}

async fn pending_change_count(pool: &sqlx::SqlitePool) -> i64 {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

async fn pending_undo_count(pool: &sqlx::SqlitePool, workspace_id: &str) -> i64 {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query_scalar(
        "SELECT count(*) FROM tui_undo_entries WHERE workspace_id = ? AND undone_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}

async fn consumed_undo_count(pool: &sqlx::SqlitePool, workspace_id: &str) -> i64 {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query_scalar(
        "SELECT count(*) FROM tui_undo_entries WHERE workspace_id = ? AND undone_at IS NOT NULL",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}

async fn latest_payload(
    conn: &mut sqlx::SqliteConnection,
    entity_type: &str,
    op_type: &str,
) -> serde_json::Value {
    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM changes
         WHERE entity_type = ? AND op_type = ?
         ORDER BY local_seq DESC LIMIT 1",
    )
    .bind(entity_type)
    .bind(op_type)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    serde_json::from_str(&payload).unwrap()
}

fn assert_workspace_payload(payload: &serde_json::Value, workspace: &crate::workspaces::Workspace) {
    assert_eq!(
        payload["workspace_id"].as_str(),
        Some(workspace.id.as_str())
    );
    assert_eq!(
        payload["workspace_key"].as_str(),
        Some(workspace.key.as_str())
    );
}

const MOBILE_PROJECT_NAME: &str = "Mobile App";

async fn create_mobile_project(store: &mut TuiStore) {
    store
        .create_project(MOBILE_PROJECT_NAME.to_string())
        .await
        .unwrap();
}

async fn create_task_in_project(store: &mut TuiStore, title: &str, project_key: &str) -> usize {
    let (_, selected) = store
        .create_task(
            TaskDraft {
                title: title.to_string(),
                project: Some(project_key.to_string()),
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    selected.unwrap()
}

#[track_caller]
fn assert_project_hidden(store: &TuiStore, key: &str) {
    assert!(!store.projects.iter().any(|project| project.key == key));
    assert!(!store.sidebar_entries.iter().any(|entry| {
        entry.target
            == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                key.to_string(),
            )))
    }));
}

mod domain_mutations_and_pickers {
    use super::*;

    #[tokio::test]
    async fn create_project_refreshes_sidebar() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;

        assert!(
            store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
        assert!(
            store
                .sidebar_entries
                .iter()
                .any(|entry| entry.label.contains("Mobile App"))
        );
    }

    #[tokio::test]
    async fn delete_project_removes_unused_project() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;

        let outcome = store.delete_project("mobile-app").await.unwrap();

        assert_eq!(outcome.message, "deleted project mobile-app");
        assert_project_hidden(&store, "mobile-app");
    }

    #[tokio::test]
    async fn rename_project_updates_view_filters_and_tasks() {
        let mut store = test_store().await;
        store
            .create_project("Agent Offload".to_string())
            .await
            .unwrap();
        store
            .create_task(
                TaskDraft {
                    title: "Rename keeps task".to_string(),
                    project: Some("agent-offload".to_string()),
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        store
            .show_scope(TaskScopeTarget::Project("agent-offload".to_string()))
            .await
            .unwrap();

        let outcome = store
            .rename_project("agent-offload", "sideagent".to_string())
            .await
            .unwrap();

        assert_eq!(outcome.message, "renamed project sideagent prefix=SDG");
        assert_eq!(
            store.view_state.scope,
            TaskScope::Project("sideagent".to_string())
        );
        assert!(store.projects.iter().any(|project| {
            project.key == "sideagent" && project.name == "sideagent" && project.prefix == "SDG"
        }));
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.project_key, "sideagent");
    }

    #[tokio::test]
    async fn undo_project_rename_restores_view_filters_and_tasks() {
        let mut store = test_store().await;
        store
            .create_project("Agent Offload".to_string())
            .await
            .unwrap();
        store
            .create_task(
                TaskDraft {
                    title: "Undo rename keeps task".to_string(),
                    project: Some("agent-offload".to_string()),
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        store
            .show_scope(TaskScopeTarget::Project("agent-offload".to_string()))
            .await
            .unwrap();
        store
            .rename_project("agent-offload", "sideagent".to_string())
            .await
            .unwrap();

        store.undo_last(None).await.unwrap();

        assert_eq!(
            store.view_state.scope,
            TaskScope::Project("agent-offload".to_string())
        );
        assert!(store.projects.iter().any(|project| {
            project.key == "agent-offload"
                && project.name == "Agent Offload"
                && project.prefix == "AO"
        }));
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.project_key, "agent-offload");
    }

    #[tokio::test]
    async fn delete_project_hides_project_with_tasks() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        create_task_in_project(&mut store, "Keep project", "mobile-app").await;

        let outcome = store.delete_project("mobile-app").await.unwrap();

        assert_eq!(outcome.message, "deleted project mobile-app");
        assert_project_hidden(&store, "mobile-app");
    }

    #[tokio::test]
    async fn delete_project_hides_project_with_deleted_tasks() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        let selected =
            create_task_in_project(&mut store, "Deleted project task", "mobile-app").await;
        store.update_deleted(Some(selected), true).await.unwrap();

        let outcome = store.delete_project("mobile-app").await.unwrap();

        assert_eq!(outcome.message, "deleted project mobile-app");
        assert!(
            !store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
    }

    #[tokio::test]
    async fn create_label_refreshes_label_cache() {
        let mut store = test_store().await;
        store
            .create_label("Needs Review".to_string())
            .await
            .unwrap();

        assert!(store.labels.iter().any(|label| label == "needs-review"));
        assert!(
            store
                .label_picker_items()
                .iter()
                .any(|item| item.value == "needs-review")
        );
    }

    #[tokio::test]
    async fn project_picker_includes_infer_project_and_existing_projects() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;

        let items = store.project_picker_items(None);
        assert!(items[0].label.starts_with("Infer project"));
        assert!(items[0].selected);
        assert!(items.iter().any(|item| item.value == "mobile-app"));
    }

    #[tokio::test]
    async fn priority_picker_includes_all_priorities() {
        let store = test_store().await;
        let items = store.priority_picker_items("none");
        assert_eq!(items.len(), PRIORITIES.len());
        assert!(items[0].selected);
    }

    #[tokio::test]
    async fn existing_project_picker_items_excludes_infer_project() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;

        let items = store.existing_project_picker_items("mobile-app");
        assert!(!items.iter().any(|item| item.label == "Infer project"));
        assert!(items.iter().any(|item| item.value == "mobile-app"));
        assert!(items.iter().any(|item| item.selected));
    }
}

mod task_creation_and_updates {
    use super::*;

    #[tokio::test]
    async fn create_task_refreshes_and_selects_visible_task() {
        let mut store = test_store().await;
        store
            .create_label("needs-review".to_string())
            .await
            .unwrap();
        let (message, selected) = store
            .create_task(
                TaskDraft {
                    title: "Write docs".to_string(),
                    description: "details".to_string(),
                    priority: "high".to_string(),
                    labels: vec!["needs-review".to_string()],
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();

        let selected = selected.unwrap();
        assert_eq!(
            message,
            format!("created task {}", store.tasks[selected].display_ref)
        );
        let task = &store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.priority, TaskPriority::High);
        assert!(task.labels.iter().any(|label| label == "needs-review"));
    }

    #[tokio::test]
    async fn create_task_reports_hidden_by_filters() {
        let mut store = test_store().await;
        store.show_view(TaskView::Todo).await.unwrap();
        let (message, selected) = store
            .create_task(task_draft("Inbox task"), None)
            .await
            .unwrap();

        assert!(selected.is_none());
        assert!(message.contains("hidden by current filters"));
    }

    #[tokio::test]
    async fn create_task_preserves_previous_selection_when_hidden() {
        let mut store = test_store().await;
        let (_, first_selected) = store
            .create_task(task_draft("Todo task"), None)
            .await
            .unwrap();
        let first_selected = first_selected.unwrap();
        let task_id = store.tasks[first_selected].task.id.clone();
        store
            .update_status(Some(first_selected), "todo")
            .await
            .unwrap();
        store.show_view(TaskView::Todo).await.unwrap();
        let current_index = store.refresh(Some(&task_id)).await.unwrap();

        let (_, selected) = store
            .create_task(task_draft("Hidden inbox task"), current_index)
            .await
            .unwrap();

        assert_eq!(selected, current_index);
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Todo task");
    }

    #[tokio::test]
    async fn update_status_preserving_task_keeps_done_item_in_filtered_view() {
        let mut store = test_store().await;
        let _ = store
            .create_task(
                TaskDraft {
                    title: "Next target".to_string(),
                    status: "todo".to_string(),
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Done target".to_string(),
                    status: "todo".to_string(),
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();

        store.show_view(TaskView::Todo).await.unwrap();
        let selected = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();

        let result = store
            .update_status_preserving_task(Some(selected), "done")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.selected, Some(selected));
        assert_eq!(store.tasks[selected].task.id, task_id);
        assert_eq!(store.tasks[selected].task.status, TaskStatus::Done);
        assert_eq!(store.counts.done, 1);
    }

    #[tokio::test]
    async fn add_note_to_task_writes_note() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(task_draft("Note target"), None)
            .await
            .unwrap();
        let task_id = store.tasks[selected.unwrap()].task.id.clone();
        let note_id = store
            .add_note_to_task(&task_id, "hello note".to_string())
            .await
            .unwrap();
        assert!(!note_id.is_empty());
    }

    #[tokio::test]
    async fn update_task_fields_refresh_selected_task() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Old".to_string(),
                    description: "old body".to_string(),
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        let display_ref = store.tasks[selected].display_ref.clone();

        let title = store
            .update_title(Some(selected), "New".to_string())
            .await
            .unwrap()
            .unwrap();
        let description = store
            .update_description(Some(selected), "new body".to_string())
            .await
            .unwrap()
            .unwrap();
        let priority = store
            .set_exact_priority(Some(selected), "urgent")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(title.message, format!("set {display_ref} title"));
        assert_eq!(
            description.message,
            format!("set {display_ref} description")
        );
        assert_eq!(
            priority.message,
            format!("set {display_ref} priority=urgent")
        );
        let task = &store.tasks[selected].task;
        assert_eq!(task.title, "New");
        assert_eq!(task.description, "new body");
        assert_eq!(task.priority, TaskPriority::Urgent);
    }

    #[tokio::test]
    async fn title_edit_keeps_queue_activity_timestamp() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) =
            create_selected_task_with_stale_queue_activity(&mut store, &pool, "Old").await;
        let old_activity = "1970-01-01T00:00:00Z";
        let old_updated = "1970-01-02T00:00:00Z";
        set_task_timestamps(
            &pool,
            &store.active_workspace.id,
            &task_id,
            old_activity,
            Some(old_updated),
        )
        .await;
        store.refresh(Some(&task_id)).await.unwrap();

        store
            .update_title(Some(selected), "New".to_string())
            .await
            .unwrap();

        let task = &store.tasks[selected].task;
        assert_eq!(task.title, "New");
        assert_ne!(task.updated_at, old_updated);
        assert_eq!(task.queue_activity_at, old_activity);
    }

    #[tokio::test]
    async fn unchanged_title_edit_leaves_pending_change_count() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (_task_id, selected) = create_selected_task(&mut store, "Stable").await;
        let pending_before = pending_change_count(&pool).await;
        let display_ref = store.tasks[selected].display_ref.clone();

        let outcome = store
            .update_title(Some(selected), "Stable".to_string())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.message, format!("unchanged {display_ref} title"));
        assert_eq!(pending_change_count(&pool).await, pending_before);
        assert_eq!(store.tasks[selected].task.title, "Stable");
    }

    #[tokio::test]
    async fn unchanged_task_fields_leave_pending_change_count() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_project("Side".to_string()).await.unwrap();
        let (task_id, _selected) = create_selected_task(&mut store, "Stable").await;
        let pending_before = pending_change_count(&pool).await;
        let mut conn = pool.acquire().await.unwrap();

        let outcome = crate::operations::update_task(
            &mut conn,
            &task_id,
            TaskUpdate {
                title: Some("Stable".to_string()),
                description: Some(String::new()),
                project: Some("aven".to_string()),
                status: Some("inbox".to_string()),
                priority: Some("none".to_string()),
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        let deleted = crate::operations::set_task_deleted(&mut conn, &task_id, false)
            .await
            .unwrap();
        drop(conn);

        assert!(!outcome.changed);
        assert_eq!(deleted.task.title, "Stable");
        assert_eq!(pending_change_count(&pool).await, pending_before);
    }

    #[tokio::test]
    async fn unchanged_label_updates_leave_pending_change_count() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("missing".to_string()).await.unwrap();
        let (task_id, _selected) = create_selected_task(&mut store, "Labels").await;
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::update_task(
            &mut conn,
            &task_id,
            TaskUpdate {
                add_labels: vec!["bug".to_string()],
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        drop(conn);
        let pending_before = pending_change_count(&pool).await;
        let mut conn = pool.acquire().await.unwrap();

        let outcome = crate::operations::update_task(
            &mut conn,
            &task_id,
            TaskUpdate {
                add_labels: vec!["bug".to_string()],
                remove_labels: vec!["missing".to_string()],
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        drop(conn);

        assert!(!outcome.changed);
        assert_eq!(pending_change_count(&pool).await, pending_before);
    }

    #[tokio::test]
    async fn priority_edit_refreshes_queue_activity_timestamp() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (_task_id, selected) =
            create_selected_task_with_stale_queue_activity(&mut store, &pool, "Old").await;
        let old_activity = "1970-01-01T00:00:00Z";

        store
            .set_exact_priority(Some(selected), "high")
            .await
            .unwrap();

        let task = &store.tasks[selected].task;
        assert_eq!(task.priority, TaskPriority::High);
        assert_ne!(task.queue_activity_at, old_activity);
    }

    #[tokio::test]
    async fn update_labels_adds_and_removes_labels() {
        let mut store = test_store().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("docs".to_string()).await.unwrap();
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Labels".to_string(),
                    labels: vec!["bug".to_string()],
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        let display_ref = store.tasks[selected].display_ref.clone();

        let outcome = store
            .update_labels(Some(selected), vec!["docs".to_string()])
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.message, format!("set {display_ref} labels"));
        assert_eq!(store.tasks[selected].labels, vec!["docs".to_string()]);
    }
}

mod conflicts {
    use super::*;

    #[test]
    fn next_conflict_flag_index_wraps_forward() {
        let flags = vec![false, true, false, true];
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(1), 1),
            Some(3)
        );
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(3), 1),
            Some(1)
        );
    }

    #[test]
    fn next_conflict_flag_index_wraps_backward() {
        let flags = vec![false, true, false, true];
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(3), -1),
            Some(1)
        );
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(1), -1),
            Some(3)
        );
    }

    #[test]
    fn next_conflict_flag_index_returns_none_without_conflicts() {
        let flags = vec![false, false];
        assert!(TuiStore::next_conflict_flag_index(&flags, Some(0), 1).is_none());
    }

    #[test]
    fn next_conflict_flag_index_keeps_single_conflict() {
        let flags = vec![false, true, false];
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(1), 1),
            Some(1)
        );
    }

    #[tokio::test]
    async fn resolve_conflict_value_updates_task_and_clears_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
        let (_, selected) = store.create_task(task_draft("Before"), None).await.unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();
        let display_ref = store.tasks[selected].display_ref.clone();

        seed_title_conflict(&pool, &task_id).await;
        store.refresh(Some(&task_id)).await.unwrap();

        let outcome = store
            .resolve_conflict_value(
                ConflictTarget {
                    task_id,
                    display_ref: display_ref.clone(),
                    field: "title".to_string(),
                    variant_a: "a".to_string(),
                    local_value: "local title".to_string(),
                    variant_b: "b".to_string(),
                    remote_value: "remote title".to_string(),
                },
                "local title".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(
            outcome.message,
            format!("resolved {display_ref} conflict field=title")
        );
        assert_eq!(store.tasks[selected].task.title, "local title");
        assert!(!store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn resolve_missing_conflict_leaves_task_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
        let (_, selected) = store
            .create_task(task_draft("Stable title"), None)
            .await
            .unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();

        let error = store
            .resolve_conflict_value(
                ConflictTarget {
                    task_id,
                    display_ref: "APP-1".to_string(),
                    field: "title".to_string(),
                    variant_a: "a".to_string(),
                    local_value: "local".to_string(),
                    variant_b: "b".to_string(),
                    remote_value: "remote".to_string(),
                },
                "local".to_string(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("conflict-not-found"));
        assert_eq!(store.tasks[selected].task.title, "Stable title");
    }

    #[tokio::test]
    async fn update_title_returns_conflicted_field_error() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
        let (_, selected) = store
            .create_task(task_draft("Conflict"), None)
            .await
            .unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(&task_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        let error = store
            .update_title(Some(selected), "blocked".to_string())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("conflicted-field"));
    }
}

mod views_filters_and_sort {
    use super::*;

    #[tokio::test]
    async fn sidebar_selection_prefers_project_scope_when_scoped() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();

        let selected = store.sidebar_selection().unwrap();

        assert_eq!(
            store.sidebar_entries[selected].target,
            Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                "mobile-app".to_string()
            )))
        );
    }

    #[tokio::test]
    async fn clear_filters_preserves_view_scope_and_order() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        store.show_view(TaskView::Todo).await.unwrap();
        store.view_state.order = TaskOrder::Priority;
        store.view_state.direction = SortDirection::Desc;
        store.view_state.filter_modifiers.label = Some("backend".to_string());
        store.view_state.filter_modifiers.task_ids = vec!["task-1".to_string()];

        store.clear_filters().await.unwrap();

        assert_eq!(
            store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
        assert_eq!(store.view_state.view, TaskView::Todo);
        assert_eq!(store.view_state.order, TaskOrder::Priority);
        assert_eq!(store.view_state.direction, SortDirection::Desc);
        assert!(store.view_state.filter_modifiers.label.is_none());
        assert!(store.view_state.filter_modifiers.task_ids.is_empty());
    }

    #[tokio::test]
    async fn show_conflicts_view_sets_conflicts_view() {
        let mut store = test_store().await;

        store.show_view(TaskView::Conflicts).await.unwrap();

        assert_eq!(store.view_state.view, TaskView::Conflicts);
        assert!(store.view_state.filters().conflicts_only);
    }

    #[tokio::test]
    async fn queue_view_hides_done_tasks() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(task_draft("Finished"), None)
            .await
            .unwrap();
        store.update_status(selected, "done").await.unwrap();

        store.show_view(TaskView::Queue).await.unwrap();

        assert!(
            store
                .tasks
                .iter()
                .all(|item| item.task.status != TaskStatus::Done)
        );
        assert_eq!(store.counts.done, 1);
        assert!(store.sidebar_entries.iter().any(|entry| {
            entry.target == Some(SidebarEntryTarget::View(TaskView::Done)) && entry.count == 1
        }));
    }

    #[tokio::test]
    async fn project_scope_hides_done_and_canceled_tasks_in_open_view() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        for (title, status) in [
            ("Open task", "todo"),
            ("Finished", "done"),
            ("Canceled", "canceled"),
        ] {
            let (_, selected) = store
                .create_task(
                    TaskDraft {
                        title: title.to_string(),
                        project: Some("mobile-app".to_string()),
                        ..task_draft("")
                    },
                    None,
                )
                .await
                .unwrap();
            let selected = selected.unwrap();
            store.update_status(Some(selected), status).await.unwrap();
        }

        store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        store.show_view(TaskView::Open).await.unwrap();

        let filters = store.view_state.filters();
        assert_eq!(filters.project.as_deref(), Some("mobile-app"));
        assert!(filters.hide_done);
        assert_eq!(
            store
                .tasks
                .iter()
                .map(|item| item.task.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Open task"]
        );
    }

    #[tokio::test]
    async fn done_view_preserves_project_scope() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        store.create_project("Ops".to_string()).await.unwrap();
        for (title, project) in [("Mobile done", "mobile-app"), ("Ops done", "ops")] {
            let (_, selected) = store
                .create_task(
                    TaskDraft {
                        title: title.to_string(),
                        project: Some(project.to_string()),
                        ..task_draft("")
                    },
                    None,
                )
                .await
                .unwrap();
            store.update_status(selected, "done").await.unwrap();
        }

        store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        store.show_view(TaskView::Done).await.unwrap();

        assert_eq!(
            store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
        assert_eq!(store.view_state.view, TaskView::Done);
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Mobile done");
    }

    #[tokio::test]
    async fn done_view_shows_done_tasks() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(task_draft("Finished"), None)
            .await
            .unwrap();
        let selected = selected.unwrap();
        store.update_status(Some(selected), "done").await.unwrap();

        store.show_view(TaskView::Done).await.unwrap();

        assert_eq!(
            store.view_state.filters().statuses,
            vec!["done", "canceled"]
        );
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Finished");
    }

    async fn create_search_task(store: &mut TuiStore, title: &str) -> String {
        let (_, selected) = store.create_task(task_draft(title), None).await.unwrap();
        store.tasks[selected.unwrap()].task.id.clone()
    }

    #[tokio::test]
    async fn search_view_preview_hides_deleted_ordinary_text_results() {
        let mut store = test_store().await;
        let live_id = create_search_task(&mut store, "Live needle").await;
        let deleted_id = create_search_task(&mut store, "Deleted needle").await;
        let deleted_index = store
            .tasks
            .iter()
            .position(|item| item.task.id == deleted_id)
            .unwrap();
        store
            .update_deleted(Some(deleted_index), true)
            .await
            .unwrap();

        let results = store.search_preview("needle", 10).await.unwrap();

        let ids = results
            .items
            .iter()
            .map(|result| result.task_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![live_id.as_str()]);
        assert!(!ids.contains(&deleted_id.as_str()));
        assert_eq!(results.total_matches, 1);
    }

    #[tokio::test]
    async fn search_view_submitted_search_hides_deleted_ordinary_text_results() {
        let mut store = test_store().await;
        let live_id = create_search_task(&mut store, "Live needle").await;
        let deleted_id = create_search_task(&mut store, "Deleted needle").await;
        let deleted_index = store
            .tasks
            .iter()
            .position(|item| item.task.id == deleted_id)
            .unwrap();
        store
            .update_deleted(Some(deleted_index), true)
            .await
            .unwrap();

        store.accept_search("needle").await.unwrap();

        let ids = store
            .tasks
            .iter()
            .map(|item| item.task.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![live_id.as_str()]);
        assert!(!ids.contains(&deleted_id.as_str()));
        assert_eq!(store.view_state.view, TaskView::Search);
    }

    #[tokio::test]
    async fn search_view_preview_returns_rendered_fields_without_full_hydration() {
        let mut store = test_store().await;
        let task_id = create_search_task(&mut store, "Preview needle").await;
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        store.set_exact_priority(Some(index), "high").await.unwrap();
        store.create_label("fast".to_string()).await.unwrap();
        store
            .update_labels(Some(index), vec!["fast".to_string()])
            .await
            .unwrap();
        store
            .add_note_to_task(&task_id, "needle note body".to_string())
            .await
            .unwrap();

        let results = store.search_preview("Preview", 10).await.unwrap();

        assert_eq!(results.total_matches, 1);
        let result = &results.items[0];
        assert_eq!(result.task_id, task_id);
        assert_eq!(result.title, "Preview needle");
        assert_eq!(result.priority, "high");
        assert_eq!(result.labels, vec!["fast"]);
        assert_eq!(
            result.matched_field,
            crate::query::SearchMatchedField::Title
        );
        assert!(!result.created_at.is_empty());
        assert!(!result.deleted);
    }

    #[tokio::test]
    async fn search_view_submitted_search_keeps_full_result_hydration() {
        let mut store = test_store().await;
        let blocker_id = create_search_task(&mut store, "Blocker task").await;
        let task_id = create_search_task(&mut store, "Hydrated needle").await;
        let dependent_id = create_search_task(&mut store, "Dependent task").await;

        let blocker_display_ref = store
            .tasks
            .iter()
            .find(|item| item.task.id == blocker_id)
            .map(|item| item.display_ref.clone())
            .unwrap();
        let dependent_display_ref = store
            .tasks
            .iter()
            .find(|item| item.task.id == dependent_id)
            .map(|item| item.display_ref.clone())
            .unwrap();

        let task_index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        store
            .create_label("needs-review".to_string())
            .await
            .unwrap();
        store
            .update_labels(Some(task_index), vec!["needs-review".to_string()])
            .await
            .unwrap();
        store
            .add_note_to_task(&task_id, "hydrated note".to_string())
            .await
            .unwrap();

        let pool = store.pool.clone();
        seed_title_conflict(&pool, &task_id).await;
        store.refresh(Some(&task_id)).await.unwrap();

        let mut conn = store.pool.acquire().await.unwrap();
        crate::operations::add_task_dependency(&mut conn, &task_id, &blocker_id)
            .await
            .unwrap();
        drop(conn);

        let mut conn = store.pool.acquire().await.unwrap();
        crate::operations::add_task_dependency(&mut conn, &dependent_id, &task_id)
            .await
            .unwrap();
        drop(conn);

        store.refresh(Some(&task_id)).await.unwrap();
        store.accept_search("Hydrated").await.unwrap();

        let item = store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(item.notes.len(), 1);
        assert_eq!(item.notes[0].body, "hydrated note");
        assert!(item.has_conflict);
        assert_eq!(item.unresolved_blocker_count, 1);
        assert_eq!(item.dependent_count, 1);
        assert_eq!(item.depends_on.len(), 1);
        assert_eq!(item.blocks.len(), 1);
        assert_eq!(item.depends_on[0].display_ref, blocker_display_ref);
        assert_eq!(item.blocks[0].display_ref, dependent_display_ref);
    }

    #[tokio::test]
    async fn search_view_finds_done_tasks_from_queue() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(task_draft("Finished spotlight needle"), None)
            .await
            .unwrap();
        store.update_status(selected, "done").await.unwrap();
        store.show_view(TaskView::Queue).await.unwrap();
        assert!(store.tasks.is_empty());

        store.accept_search("spotlight needle").await.unwrap();

        assert_eq!(store.view_state.scope, TaskScope::Workspace);
        assert_eq!(store.view_state.view, TaskView::Search);
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Finished spotlight needle");
    }

    #[tokio::test]
    async fn toggle_deleted_filter_switches_include_deleted() {
        let mut store = test_store().await;

        store.toggle_deleted_filter().await.unwrap();
        assert!(store.view_state.filter_modifiers.include_deleted);
        assert!(!store.view_state.filter_modifiers.deleted_only);

        store.toggle_deleted_filter().await.unwrap();
        assert!(store.view_state.filter_modifiers.include_deleted);
        assert!(store.view_state.filter_modifiers.deleted_only);

        store.toggle_deleted_filter().await.unwrap();
        assert!(!store.view_state.filter_modifiers.include_deleted);
        assert!(!store.view_state.filter_modifiers.deleted_only);
    }

    #[tokio::test]
    async fn deleted_filter_cycle_preserves_project_scope() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        create_task_in_project(&mut store, "Live project task", "mobile-app").await;
        let selected =
            create_task_in_project(&mut store, "Deleted project task", "mobile-app").await;
        store.update_deleted(Some(selected), true).await.unwrap();
        store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Live project task");

        store.toggle_deleted_filter().await.unwrap();

        assert_eq!(
            store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
        assert!(store.view_state.filter_modifiers.include_deleted);
        assert!(!store.view_state.filter_modifiers.deleted_only);
        assert_eq!(store.tasks.len(), 2);

        store.toggle_deleted_filter().await.unwrap();

        assert!(store.view_state.filter_modifiers.include_deleted);
        assert!(store.view_state.filter_modifiers.deleted_only);
        assert_eq!(store.tasks.len(), 1);
        assert!(store.tasks[0].task.deleted);
    }

    #[tokio::test]
    async fn ordering_from_queue_switches_to_open() {
        let mut store = test_store().await;

        store.set_order(TaskOrder::Priority).await.unwrap();
        assert_eq!(store.view_state.view, TaskView::Open);
        assert_eq!(store.view_state.order, TaskOrder::Priority);
        assert_eq!(store.view_state.direction, SortDirection::Asc);

        store.reverse_sort().await.unwrap();
        assert_eq!(store.view_state.view, TaskView::Open);
        assert_eq!(store.view_state.direction, SortDirection::Desc);
    }

    #[tokio::test]
    async fn created_order_defaults_to_descending_and_can_toggle() {
        let mut store = test_store().await;
        store.set_order(TaskOrder::Priority).await.unwrap();
        store.reverse_sort().await.unwrap();
        store.reverse_sort().await.unwrap();
        assert_eq!(store.view_state.direction, SortDirection::Asc);

        store.set_order(TaskOrder::Created).await.unwrap();
        assert_eq!(store.view_state.view, TaskView::Open);
        assert_eq!(store.view_state.order, TaskOrder::Created);
        assert_eq!(store.view_state.direction, SortDirection::Desc);

        store.reverse_sort().await.unwrap();
        assert_eq!(store.view_state.direction, SortDirection::Asc);
    }
}

mod sync_workspace_payloads {
    use super::*;

    #[tokio::test]
    async fn explicit_workspace_payloads_pair_id_and_key_when_active_differs() {
        let (_dir, pool, _store) = test_store_with_pool().await;
        let default = crate::workspaces::active_workspace();
        let mut conn = pool.acquire().await.unwrap();
        let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        crate::workspaces::set_active_workspace(default);

        crate::operations::create_label_operation_in_workspace(
            &mut conn,
            &other.id,
            "Needs Review",
        )
        .await
        .unwrap();
        assert_workspace_payload(
            &latest_payload(&mut conn, "label", "create_label").await,
            &other,
        );

        let task = crate::operations::create_task_in_workspace(
            &mut conn,
            &other.id,
            TaskDraft {
                title: "Scoped task".to_string(),
                project: Some("Mobile App".to_string()),
                labels: vec!["Needs Review".to_string()],
                ..task_draft("")
            },
        )
        .await
        .unwrap()
        .task;
        assert_workspace_payload(
            &latest_payload(&mut conn, "project", "create_project").await,
            &other,
        );
        assert_workspace_payload(
            &latest_payload(&mut conn, "task", "create_task").await,
            &other,
        );

        crate::operations::create_label_operation_in_workspace(&mut conn, &other.id, "Docs")
            .await
            .unwrap();
        crate::operations::update_task_labels_in_workspace(
            &mut conn,
            &other.id,
            &task.id,
            &[String::from("Docs")],
            &[String::from("Needs Review")],
        )
        .await
        .unwrap();
        assert_workspace_payload(
            &latest_payload(&mut conn, "task", "label_add").await,
            &other,
        );
        assert_workspace_payload(
            &latest_payload(&mut conn, "task", "label_remove").await,
            &other,
        );
    }
}

mod undo {
    use super::*;

    #[tokio::test]
    async fn undo_returns_none_when_empty() {
        let mut store = test_store().await;
        assert!(store.undo_last(None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn undo_title_edit_expires_on_store_restart() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Before").await;
        store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();
        assert_eq!(store.tasks[selected].task.title, "After");

        let mut restarted = TuiStore::new(pool).await.unwrap();
        assert!(restarted.undo_last(None).await.unwrap().is_none());
        let index = restarted
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(restarted.tasks[index].task.title, "After");
    }

    #[tokio::test]
    async fn store_startup_clears_pending_undo_but_preserves_consumed_entries() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (_, selected) = create_selected_task(&mut store, "Before").await;
        let workspace_id = store.active_workspace.id.clone();

        store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();
        store.undo_last(None).await.unwrap().unwrap();

        let consumed_before = consumed_undo_count(&pool, &workspace_id).await;
        assert_eq!(consumed_before, 1);
        assert_eq!(pending_undo_count(&pool, &workspace_id).await, 1);

        drop(store);
        let _restarted = TuiStore::new(pool.clone()).await.unwrap();

        assert_eq!(pending_undo_count(&pool, &workspace_id).await, 0);
        assert_eq!(
            consumed_undo_count(&pool, &workspace_id).await,
            consumed_before
        );
    }

    #[tokio::test]
    async fn undo_guard_blocks_stale_task_field() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Before").await;
        store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("UPDATE tasks SET title = ? WHERE id = ?")
            .bind("Changed")
            .bind(&task_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let error = store.undo_last(None).await.unwrap_err();
        assert!(error.to_string().contains("undo-state-changed"));
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(store.tasks[index].task.title, "Changed");
    }

    #[tokio::test]
    async fn undo_delete_restores_task() {
        let mut store = test_store().await;
        let (task_id, selected) = create_selected_task(&mut store, "Keep").await;
        let display_ref = store.tasks[selected].display_ref.clone();
        let delete = store
            .update_deleted(Some(selected), true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(delete.message, format!("deleted {display_ref}"));
        assert!(!store.view_state.filter_modifiers.include_deleted);
        store.refresh(Some(&task_id)).await.unwrap();
        assert!(store.tasks.iter().all(|item| item.task.id != task_id));
        store.view_state.filter_modifiers.include_deleted = true;
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert!(store.tasks[index].task.deleted);

        store.undo_last(None).await.unwrap();
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert!(!store.tasks[index].task.deleted);
    }

    #[tokio::test]
    async fn repeated_delete_does_not_add_noop_undo_entry() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Keep once").await;
        store.update_deleted(Some(selected), true).await.unwrap();
        let workspace_id = store.active_workspace.id.clone();
        let undo_count_after_delete = pending_undo_count(&pool, &workspace_id).await;
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        store.update_deleted(Some(index), true).await.unwrap();

        assert_eq!(
            pending_undo_count(&pool, &workspace_id).await,
            undo_count_after_delete
        );
        store.undo_last(None).await.unwrap().unwrap();
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert!(!store.tasks[index].task.deleted);
    }

    #[tokio::test]
    async fn noop_task_field_updates_do_not_add_undo_entries() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_project("Side".to_string()).await.unwrap();
        store.create_label("bug".to_string()).await.unwrap();
        let (task_id, selected) = create_selected_task(&mut store, "Noop fields").await;
        store
            .update_title(Some(selected), "Changed".to_string())
            .await
            .unwrap();
        store
            .update_description(Some(selected), "details".to_string())
            .await
            .unwrap();
        store
            .set_exact_priority(Some(selected), "high")
            .await
            .unwrap();
        store
            .update_project(Some(selected), "side".to_string())
            .await
            .unwrap();
        store
            .update_labels(Some(selected), vec!["bug".to_string()])
            .await
            .unwrap();
        store.update_status(Some(selected), "todo").await.unwrap();
        let workspace_id = store.active_workspace.id.clone();
        let undo_count_after_changes = pending_undo_count(&pool, &workspace_id).await;
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();

        store.update_status(Some(index), "todo").await.unwrap();
        store.set_exact_priority(Some(index), "high").await.unwrap();
        store
            .update_title(Some(index), "Changed".to_string())
            .await
            .unwrap();
        store
            .update_description(Some(index), "details".to_string())
            .await
            .unwrap();
        store
            .update_project(Some(index), "side".to_string())
            .await
            .unwrap();
        store
            .update_labels(Some(index), vec!["bug".to_string()])
            .await
            .unwrap();

        assert_eq!(
            pending_undo_count(&pool, &workspace_id).await,
            undo_count_after_changes
        );
    }

    #[tokio::test]
    async fn duplicate_project_and_label_do_not_add_undo_entries() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_project("Side".to_string()).await.unwrap();
        store.create_label("bug".to_string()).await.unwrap();
        let workspace_id = store.active_workspace.id.clone();
        let undo_count_after_creates = pending_undo_count(&pool, &workspace_id).await;

        store.create_project("Side".to_string()).await.unwrap();
        store.create_label("bug".to_string()).await.unwrap();

        assert_eq!(
            pending_undo_count(&pool, &workspace_id).await,
            undo_count_after_creates
        );
    }

    #[tokio::test]
    async fn undo_restore_redeletes_task() {
        let mut store = test_store().await;
        let (task_id, selected) = create_selected_task(&mut store, "Gone").await;
        store.update_deleted(Some(selected), true).await.unwrap();
        store.view_state.filter_modifiers.include_deleted = true;
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        let display_ref = store.tasks[index].display_ref.clone();
        let restore = store
            .update_deleted(Some(index), false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restore.message, format!("restored {display_ref}"));

        store.undo_last(None).await.unwrap();
        store.view_state.filter_modifiers.include_deleted = true;
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert!(store.tasks[index].task.deleted);
    }

    #[tokio::test]
    async fn undo_create_task_removes_local_unsynced_task() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, _) = create_selected_task(&mut store, "Temporary").await;
        store.undo_last(None).await.unwrap();

        let mut conn = pool.acquire().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn undo_labels_uses_set_comparison() {
        let mut store = test_store().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("docs".to_string()).await.unwrap();
        let (task_id, selected) = create_selected_task(&mut store, "Labels").await;
        store
            .update_labels(Some(selected), vec!["bug".to_string()])
            .await
            .unwrap();
        store
            .update_labels(Some(selected), vec!["docs".to_string()])
            .await
            .unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(store.tasks[index].labels, vec!["docs".to_string()]);

        store.undo_last(None).await.unwrap();
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(store.tasks[index].labels, vec!["bug".to_string()]);
    }

    #[tokio::test]
    async fn undo_note_create_deletes_only_unsynced_note() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Notes").await;
        let note_id = store
            .add_note_to_task(&task_id, "hello".to_string())
            .await
            .unwrap();
        store.undo_last(None).await.unwrap();

        let mut conn = pool.acquire().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE id = ?")
            .bind(&note_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 0);
        drop(conn);
        store.refresh(Some(&task_id)).await.unwrap();
        assert_eq!(store.tasks[selected].task.title, "Notes");
    }

    #[tokio::test]
    async fn undo_project_create_fails_when_referenced_or_synced() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_project("Side".to_string()).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let workspace_id = store.active_workspace.id.clone();
        let project_key = store
            .projects
            .iter()
            .find(|project| project.key == "side")
            .unwrap()
            .key
            .clone();
        sqlx::query(
            "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES (?, ?, 'Uses project', '', (SELECT id FROM projects WHERE workspace_id = ? AND key = ?), 'inbox', 'none', ?, ?)",
        )
        .bind(&workspace_id)
        .bind(crate::ids::new_id())
        .bind(&workspace_id)
        .bind(&project_key)
        .bind(crate::ids::now())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        let error = store.undo_last(None).await.unwrap_err();
        assert!(error.to_string().contains("undo-state-changed"));
        store.refresh(None).await.unwrap();
        assert!(store.projects.iter().any(|project| project.key == "side"));
    }

    #[tokio::test]
    async fn undo_label_create_fails_when_referenced_or_synced() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_label("shared".to_string()).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let workspace_id = store.active_workspace.id.clone();
        sqlx::query(
            "INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, 'shared')",
        )
        .bind(&workspace_id)
        .bind(crate::ids::new_id())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        let error = store.undo_last(None).await.unwrap_err();
        assert!(error.to_string().contains("undo-state-changed"));
        let mut conn = pool.acquire().await.unwrap();
        store.labels = list_labels_in_workspace(&mut conn, &store.active_workspace.id, None)
            .await
            .unwrap();
        assert!(store.labels.iter().any(|label| label == "shared"));
    }

    #[tokio::test]
    async fn undo_conflict_resolution_restores_unresolved_conflict() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Before").await;
        let display_ref = store.tasks[selected].display_ref.clone();

        seed_title_conflict(&pool, &task_id).await;
        store.refresh(Some(&task_id)).await.unwrap();

        store
            .resolve_conflict_value(
                ConflictTarget {
                    task_id: task_id.clone(),
                    display_ref,
                    field: "title".to_string(),
                    variant_a: "a".to_string(),
                    local_value: "local title".to_string(),
                    variant_b: "b".to_string(),
                    remote_value: "remote title".to_string(),
                },
                "local title".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(store.tasks[selected].task.title, "local title");
        assert!(!store.tasks[selected].has_conflict);

        store.undo_last(None).await.unwrap();
        store.refresh(Some(&task_id)).await.unwrap();
        assert_eq!(store.tasks[selected].task.title, "Before");
        assert!(store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn undo_project_conflict_resolution_uses_project_ids() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_project("Ops".to_string()).await.unwrap();
        let (task_id, selected) = create_selected_task(&mut store, "Before").await;
        let display_ref = store.tasks[selected].display_ref.clone();
        let workspace_id = store.active_workspace.id.clone();

        let mut conn = pool.acquire().await.unwrap();
        let app_id: String =
            sqlx::query_scalar("SELECT id FROM projects WHERE workspace_id = ? AND key = 'aven'")
                .bind(&workspace_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let ops_id: String =
            sqlx::query_scalar("SELECT id FROM projects WHERE workspace_id = ? AND key = 'ops'")
                .bind(&workspace_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value,
             remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at,
             resolved)
             VALUES (?, ?, 'project', NULL, ?, ?, NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(&workspace_id)
        .bind(&task_id)
        .bind(&app_id)
        .bind(&ops_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        store.refresh(Some(&task_id)).await.unwrap();

        store
            .resolve_conflict_value(
                ConflictTarget {
                    task_id: task_id.clone(),
                    display_ref,
                    field: "project".to_string(),
                    variant_a: "a".to_string(),
                    local_value: app_id,
                    variant_b: "b".to_string(),
                    remote_value: ops_id,
                },
                "ops".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(store.tasks[selected].task.project_key, "ops");
        assert!(!store.tasks[selected].has_conflict);

        store.undo_last(None).await.unwrap();
        store.refresh(Some(&task_id)).await.unwrap();
        assert_eq!(store.tasks[selected].task.project_key, "aven");
        assert!(store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn undo_is_workspace_scoped_within_running_store() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Scoped").await;
        store
            .update_title(Some(selected), "Changed".to_string())
            .await
            .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        let other = crate::workspaces::create_workspace(&mut conn, "other")
            .await
            .unwrap();
        drop(conn);
        store.switch_workspace(other.key.clone()).await.unwrap();
        assert!(store.undo_last(None).await.unwrap().is_none());

        store.switch_workspace("default".to_string()).await.unwrap();
        store.undo_last(None).await.unwrap().unwrap();
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(store.tasks[index].task.title, "Scoped");
    }

    #[tokio::test]
    async fn undo_consumes_entry_once() {
        let mut store = test_store().await;
        let (_, selected) = create_selected_task(&mut store, "Once").await;
        store
            .update_title(Some(selected), "Changed".to_string())
            .await
            .unwrap();
        store.undo_last(None).await.unwrap().unwrap();
        store.undo_last(None).await.unwrap().unwrap();
        assert!(store.undo_last(None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn undo_skips_noop_status_before_previous_mutation() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Noop status").await;
        store.update_status(Some(selected), "todo").await.unwrap();
        let workspace_id = store.active_workspace.id.clone();
        let undo_count_after_change = pending_undo_count(&pool, &workspace_id).await;
        store.update_status(Some(selected), "todo").await.unwrap();
        assert_eq!(
            pending_undo_count(&pool, &workspace_id).await,
            undo_count_after_change
        );

        store.undo_last(None).await.unwrap().unwrap();
        store.refresh(Some(&task_id)).await.unwrap();
        let index = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(store.tasks[index].task.status, TaskStatus::Inbox);
        assert_eq!(pending_undo_count(&pool, &workspace_id).await, 1);
    }
}

mod workspace_scoping {
    use super::*;

    #[tokio::test]
    async fn default_startup_opens_all_projects() {
        let mut store = test_store().await;
        create_mobile_project(&mut store).await;
        create_task_in_project(&mut store, "mobile task", "mobile-app").await;

        let reopened = TuiStore::new(store.pool.clone()).await.unwrap();

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
                    title: "ops task".to_string(),
                    project: Some("ops".to_string()),
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();

        crate::workspaces::set_active_workspace(store.active_workspace.clone());
        let reopened =
            TuiStore::new_with_initial_project(store.pool.clone(), Some("mobile-app".to_string()))
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
        let mut conn = store.pool.acquire().await.unwrap();
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

        let mut conn = store.pool.acquire().await.unwrap();
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
        let mut conn = store.pool.acquire().await.unwrap();
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

        let mut conn = store.pool.acquire().await.unwrap();
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
    async fn switch_workspace_refreshes_workspace_scoped_state() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        reset_default_workspace(&pool).await;
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
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
        store.view_state.filter_modifiers.task_ids = vec!["task-1".to_string()];
        store.view_state.filter_modifiers.include_deleted = true;

        let (message, selected) = store.switch_workspace(other.key.clone()).await.unwrap();

        assert_eq!(message, "switched workspace to client-work (Client Work)");
        assert!(selected.is_none());
        assert_eq!(store.active_workspace.key, "client-work");
        assert_eq!(store.view_state.scope, TaskScope::Workspace);
        assert_eq!(store.view_state.view, TaskView::Todo);
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
    async fn workspace_picker_selects_first_inactive_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        reset_default_workspace(&pool).await;
        let mut store = TuiStore::new(pool.clone()).await.unwrap();

        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);
        store.refresh(None).await.unwrap();

        let items = store.workspace_picker_items();
        assert_eq!(items[0].label, "default");
        assert_eq!(items[0].value, "default");
        assert!(!items[0].selected);
        assert!(
            items
                .iter()
                .find(|item| item.value == "client-work")
                .is_some_and(|item| item.label == "Client Work (client-work)" && item.selected)
        );

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn refresh_reads_store_workspace_without_mutating_global_active_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        reset_default_workspace(&pool).await;
        let default = crate::workspaces::active_workspace();
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
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
        crate::workspaces::set_active_workspace(default.clone());
        store.refresh(None).await.unwrap();

        assert_eq!(crate::workspaces::active_workspace_id(), default.id);
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Client workspace task");
        assert!(store.projects.iter().any(|project| project.key == "client"));
        assert_eq!(store.labels, vec!["client-label".to_string()]);
        assert_eq!(store.counts.open, 1);
        assert_eq!(store.counts.todo, 1);

        reset_default_workspace(&pool).await;
    }
}

mod epics {
    use super::*;

    async fn create_epic_child_pair(store: &mut TuiStore) -> (String, String, usize) {
        let (parent_id, _parent_index) = create_selected_task(store, "epic parent").await;
        let child_title = format!("child of {}", &parent_id[..4]);
        let (child_id, _) = create_selected_task(store, &child_title).await;

        let mut conn = store.pool.acquire().await.unwrap();
        crate::operations::add_task_to_epic(&mut conn, &child_id, &parent_id)
            .await
            .unwrap();
        drop(conn);

        store.view_state.view = TaskView::Epics;
        store.refresh(Some(&parent_id)).await.unwrap();
        let parent_index = store
            .tasks
            .iter()
            .position(|t| t.task.id == parent_id)
            .unwrap();
        assert!(store.view_state.expanded_epic_ids.contains(&parent_id));
        assert!(store.tasks.iter().any(|task| task.task.id == child_id));
        (parent_id, child_id, parent_index)
    }

    #[tokio::test]
    async fn epics_view_expands_parent_by_default() {
        let mut store = test_store().await;
        let (parent_id, child_id, _) = create_epic_child_pair(&mut store).await;

        assert!(store.view_state.expanded_epic_ids.contains(&parent_id));
        assert!(store.tasks.iter().any(|task| task.task.id == child_id));
    }

    #[tokio::test]
    async fn toggle_epic_collapses_and_expands_parent() {
        let mut store = test_store().await;
        let (parent_task_id, child_id, parent_index) = create_epic_child_pair(&mut store).await;

        assert!(store.view_state.expanded_epic_ids.contains(&parent_task_id));

        store
            .toggle_selected_epic(Some(parent_index))
            .await
            .unwrap()
            .unwrap();
        assert!(!store.view_state.expanded_epic_ids.contains(&parent_task_id));
        assert!(
            store
                .view_state
                .collapsed_epic_ids
                .contains(&parent_task_id)
        );
        assert!(!store.tasks.iter().any(|task| task.task.id == child_id));

        let parent_index = store
            .tasks
            .iter()
            .position(|task| task.task.id == parent_task_id)
            .unwrap();
        store
            .toggle_selected_epic(Some(parent_index))
            .await
            .unwrap()
            .unwrap();
        assert!(store.view_state.expanded_epic_ids.contains(&parent_task_id));
        assert!(
            !store
                .view_state
                .collapsed_epic_ids
                .contains(&parent_task_id)
        );
        assert!(store.tasks.iter().any(|task| task.task.id == child_id));
    }

    #[tokio::test]
    async fn toggle_epic_noop_when_no_selection() {
        let mut store = test_store().await;
        assert!(store.toggle_selected_epic(None).await.unwrap().is_none());
        assert!(
            store
                .toggle_selected_epic(Some(99))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn detach_epic_child_removes_link() {
        let mut store = test_store().await;
        let (parent_id, child_id, _parent_index) = create_epic_child_pair(&mut store).await;

        let parent_index = store
            .tasks
            .iter()
            .position(|t| t.task.id == parent_id)
            .unwrap();
        let child_index = store
            .tasks
            .iter()
            .position(|t| t.task.id == child_id)
            .unwrap();

        assert!(
            store.tasks[parent_index]
                .epic_children
                .iter()
                .any(|l| l.task_id == child_id)
        );

        let outcome = store
            .detach_selected_epic_child(Some(child_index))
            .await
            .unwrap()
            .unwrap();

        assert!(outcome.message.contains("detached"));

        if let Some(refreshed_parent) = store.tasks.iter().position(|t| t.task.id == parent_id) {
            assert!(
                !store.tasks[refreshed_parent]
                    .epic_children
                    .iter()
                    .any(|l| l.task_id == child_id)
            );
        }
    }

    #[tokio::test]
    async fn promote_epic_child_removes_link_and_selects_child() {
        let mut store = test_store().await;
        let (_parent_id, child_id, _parent_index) = create_epic_child_pair(&mut store).await;

        let child_index = store
            .tasks
            .iter()
            .position(|t| t.task.id == child_id)
            .unwrap();

        let outcome = store
            .promote_selected_epic_child(Some(child_index))
            .await
            .unwrap()
            .unwrap();

        assert!(outcome.message.contains("promoted"));
    }
}
mod dependency_actions {
    use super::*;

    #[tokio::test]
    async fn dependency_actions_add_remove_and_undo() {
        let mut store = test_store().await;
        let (blocker_id, _) = create_selected_task(&mut store, "Blocker").await;
        let (task_id, selected) = create_selected_task(&mut store, "Blocked").await;

        let add = store
            .add_dependency(Some(selected), &blocker_id)
            .await
            .unwrap()
            .unwrap();
        assert!(add.message.contains("added dependency"));
        let selected = add.selected.unwrap();
        assert_eq!(store.tasks[selected].depends_on.len(), 1);
        assert_eq!(store.tasks[selected].depends_on[0].task_id, blocker_id);
        assert_eq!(store.tasks[selected].unresolved_blocker_count, 1);

        store.undo_last(Some(selected)).await.unwrap().unwrap();
        let selected = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert!(store.tasks[selected].depends_on.is_empty());

        let add2 = store
            .add_dependency(Some(selected), &blocker_id)
            .await
            .unwrap()
            .unwrap();
        let selected = add2.selected.unwrap();
        let remove = store
            .remove_dependency(Some(selected), &blocker_id)
            .await
            .unwrap()
            .unwrap();
        assert!(remove.message.contains("removed dependency"));
        let selected = remove.selected.unwrap();
        assert!(store.tasks[selected].depends_on.is_empty());

        store.undo_last(Some(selected)).await.unwrap().unwrap();
        let selected = store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(store.tasks[selected].depends_on[0].task_id, blocker_id);
        assert_eq!(store.tasks[selected].unresolved_blocker_count, 1);
    }
}
