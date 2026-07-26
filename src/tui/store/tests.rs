#![allow(unused_variables)]

use super::*;
use crate::ids::{TaskId, WorkspaceId};

use crate::choices::{PRIORITIES, TaskPriority, TaskStatus};
use crate::operations::{TaskDraft, TaskUpdate};
use crate::query::SortDirection;
use aven_core::test_support::list_labels_in_workspace;
use sqlx::SqlitePool;

async fn test_store() -> TuiStore {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = crate::test_support::open_db(&db_path).await.unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&db_path).await.unwrap();
    TuiStore::new(database, crate::workspaces::Workspace::default())
        .await
        .unwrap()
}

async fn reset_default_workspace(pool: &SqlitePool) {
    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
}

async fn test_store_with_pool() -> (tempfile::TempDir, sqlx::SqlitePool, TuiStore) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = crate::test_support::open_db(&db_path).await.unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&db_path).await.unwrap();
    let store = TuiStore::new(database, crate::workspaces::Workspace::default())
        .await
        .unwrap();
    (dir, pool, store)
}

async fn create_selected_task(store: &mut TuiStore, title: &str) -> (TaskId, usize) {
    let (_, selected) = store
        .create_task(
            TaskDraft {
                title: title.to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "none".to_string(),
                labels: Vec::new(),
                available_at: None,
                due_on: None,
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

async fn seed_title_conflict(pool: &SqlitePool, task_id: &TaskId) {
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

async fn seed_title_conflict_database(database: &crate::db::Database, task_id: &TaskId) {
    let mut conn = aven_core::test_support::acquire(database).await.unwrap();
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
}

async fn seed_field_conflict_database(
    database: &crate::db::Database,
    task_id: &TaskId,
    field: &str,
) {
    let mut conn = aven_core::test_support::acquire(database).await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, ?, NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(task_id)
    .bind(field)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
}

fn task_draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: None,
        status: "inbox".to_string(),
        priority: "none".to_string(),
        labels: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

async fn set_task_timestamps(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
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
) -> (TaskId, usize) {
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

async fn pending_undo_count(pool: &sqlx::SqlitePool, workspace_id: &WorkspaceId) -> i64 {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query_scalar(
        "SELECT count(*) FROM tui_undo_entries WHERE workspace_id = ? AND undone_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}

async fn consumed_undo_count(pool: &sqlx::SqlitePool, workspace_id: &WorkspaceId) -> i64 {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query_scalar(
        "SELECT count(*) FROM tui_undo_entries WHERE workspace_id = ? AND undone_at IS NOT NULL",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}

async fn reject_undo_inserts(pool: &sqlx::SqlitePool) {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_undo_insert BEFORE INSERT ON tui_undo_entries
         BEGIN SELECT RAISE(FAIL, 'injected undo failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
}

#[track_caller]
fn assert_selected_task(store: &TuiStore, outcome: &MutationMessage, expected_task_id: &TaskId) {
    let selected = outcome.selected.expect("mutation should restore selection");
    assert_eq!(&store.tasks[selected].task.id, expected_task_id);
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

mod detail_loading {
    use super::*;

    #[tokio::test]
    async fn exact_task_load_ignores_active_view_filters() {
        let mut store = test_store().await;
        let (task_id, index) = create_selected_task(&mut store, "Filtered detail").await;
        store.update_status(Some(index), "todo").await.unwrap();
        store.view_state.view = TaskView::Inbox;
        store.refresh(None).await.unwrap();
        assert!(store.tasks.iter().all(|item| item.task.id != task_id));

        let item = store.load_task_item(&task_id).await.unwrap().unwrap();

        assert_eq!(item.task.id, task_id);
        assert_eq!(item.task.status.as_str(), "todo");
    }

    #[tokio::test]
    async fn exact_task_load_returns_none_for_missing_id() {
        let store = test_store().await;
        let missing = crate::test_support::task_id("missing-detail-task");

        assert!(store.load_task_item(&missing).await.unwrap().is_none());
    }
}

mod domain_mutations_and_pickers {
    use super::*;

    #[tokio::test]
    async fn project_creation_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        reject_undo_inserts(&pool).await;

        let error = store
            .create_project("Atomic Domain".to_string())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM projects WHERE key = ?")
            .bind("atomic-domain")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, 0);
    }

    #[tokio::test]
    async fn label_creation_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        reject_undo_inserts(&pool).await;

        let error = store
            .create_label("atomic-domain".to_string())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE name = ?")
            .bind("atomic-domain")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, 0);
    }

    #[tokio::test]
    async fn project_rename_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store
            .create_project("Before Atomic Rename".to_string())
            .await
            .unwrap();
        reject_undo_inserts(&pool).await;

        let error = store
            .rename_project("before-atomic-rename", "After Atomic Rename".to_string())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let key: String = sqlx::query_scalar("SELECT key FROM projects WHERE name = ?")
            .bind("Before Atomic Rename")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(key, "before-atomic-rename");
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
    async fn queue_status_change_keeps_selection_at_ranked_position() {
        let mut store = test_store().await;
        for title in ["First", "Selected", "Third"] {
            create_selected_task(&mut store, title).await;
        }
        store.show_view(TaskView::Queue).await.unwrap();
        let selected = 1;
        let selected_id = store.tasks[selected].task.id.clone();

        let result = store
            .update_status(Some(selected), "active")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.selected, Some(selected));
        assert_ne!(store.tasks[selected].task.id, selected_id);
        assert_eq!(
            store
                .tasks
                .iter()
                .position(|item| item.task.id == selected_id),
            Some(0)
        );
    }

    #[tokio::test]
    async fn filtered_status_change_selects_successor_then_previous_at_end() {
        let mut store = test_store().await;
        for title in ["First", "Second", "Third"] {
            let (_, selected) = create_selected_task(&mut store, title).await;
            store.update_status(Some(selected), "todo").await.unwrap();
        }
        store.show_view(TaskView::Todo).await.unwrap();
        let second = 1;
        let predecessor_id = store.tasks[second - 1].task.id.clone();
        let successor_id = store.tasks[second + 1].task.id.clone();

        let result = store
            .update_status(Some(second), "done")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.selected, Some(second));
        assert_eq!(store.tasks[second].task.id, successor_id);

        let result = store
            .update_status(result.selected, "done")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.selected, Some(0));
        assert_eq!(store.tasks[0].task.id, predecessor_id);
    }

    #[tokio::test]
    async fn unchanged_status_keeps_selected_task_at_its_position() {
        let mut store = test_store().await;
        for title in ["First", "Selected", "Third"] {
            create_selected_task(&mut store, title).await;
        }
        store.show_view(TaskView::Queue).await.unwrap();
        let selected = 1;
        let selected_id = store.tasks[selected].task.id.clone();

        let result = store
            .update_status(Some(selected), "inbox")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.selected, Some(selected));
        assert_eq!(store.tasks[selected].task.id, selected_id);
    }

    #[tokio::test]
    async fn marked_status_change_keeps_selection_at_ranked_position() {
        let mut store = test_store().await;
        for title in ["First", "Selected", "Third", "Fourth"] {
            create_selected_task(&mut store, title).await;
        }
        store.show_view(TaskView::Queue).await.unwrap();
        let selected = 2;
        let targets = vec![
            store.tasks[0].task.id.clone(),
            store.tasks[selected].task.id.clone(),
        ];

        let result = store
            .update_status_for_tasks(Some(selected), &targets, "active")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.selected, Some(selected));
        assert_eq!(store.tasks[selected].task.status, TaskStatus::Inbox);
    }

    #[tokio::test]
    async fn preserving_status_change_follows_task_after_queue_reorder() {
        let mut store = test_store().await;
        for title in ["First", "Selected", "Third"] {
            create_selected_task(&mut store, title).await;
        }
        store.show_view(TaskView::Queue).await.unwrap();
        let selected = 1;
        let selected_id = store.tasks[selected].task.id.clone();

        let result = store
            .update_status_preserving_task(Some(selected), "active")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.selected, Some(0));
        assert_eq!(store.tasks[0].task.id, selected_id);
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
    async fn note_creation_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, _) = create_selected_task(&mut store, "Note undo failure").await;
        reject_undo_inserts(&pool).await;

        let error = store
            .add_note_to_task(&task_id, "atomic note".to_string())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, 0);
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
    async fn availability_edit_refreshes_task_and_sidebar_counts() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(task_draft("Availability target"), None)
            .await
            .unwrap();
        let selected = selected.unwrap();

        let set = store
            .update_availability(Some(selected), "2099-01-01T00:00:00Z".to_string(), true)
            .await
            .unwrap()
            .unwrap();

        assert!(set.message.contains("set"));
        assert_eq!(store.counts.inbox, 0);
        assert_eq!(store.counts.upcoming, 1);
        let selected = set.selected.unwrap();
        assert_eq!(
            store.tasks[selected].task.available_at.as_deref(),
            Some("2099-01-01T00:00:00Z")
        );

        let cleared = store
            .update_availability(Some(selected), String::new(), false)
            .await
            .unwrap()
            .unwrap();

        assert!(cleared.message.contains("cleared"));
        assert_eq!(store.counts.inbox, 1);
        assert_eq!(store.counts.upcoming, 0);
        let selected = cleared.selected.unwrap();
        assert!(store.tasks[selected].task.available_at.is_none());
    }

    #[tokio::test]
    async fn title_edit_keeps_queue_activity_timestamp() {
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
        store.create_project("Side".to_string()).await.unwrap();
        let (task_id, _selected) = create_selected_task(&mut store, "Stable").await;
        let pending_before = pending_change_count(&pool).await;
        let mut conn = pool.acquire().await.unwrap();

        let outcome = crate::operations::update_task(
            &mut conn,
            &crate::workspaces::Workspace::default(),
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
        let deleted = crate::operations::set_task_deleted(
            &mut conn,
            &crate::workspaces::Workspace::default(),
            &task_id,
            false,
        )
        .await
        .unwrap();
        drop(conn);

        assert!(!outcome.changed);
        assert_eq!(deleted.task.title, "Stable");
        assert_eq!(pending_change_count(&pool).await, pending_before);
    }

    #[tokio::test]
    async fn unchanged_label_updates_leave_pending_change_count() {
        let (dir, pool, mut store) = test_store_with_pool().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("missing".to_string()).await.unwrap();
        let (task_id, _selected) = create_selected_task(&mut store, "Labels").await;
        let mut conn = pool.acquire().await.unwrap();
        crate::operations::update_task(
            &mut conn,
            &crate::workspaces::Workspace::default(),
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
            &crate::workspaces::Workspace::default(),
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
    async fn text_mutation_rejects_multiple_targets() {
        let mut store = test_store().await;
        let (first_id, _) = create_selected_task(&mut store, "First").await;
        let (second_id, second) = create_selected_task(&mut store, "Second").await;
        let selection = crate::tui::task_selection::TaskSelection::from_ids(
            &store.tasks,
            &[first_id, second_id],
            Some(second),
        )
        .unwrap();

        let error = store
            .mutate_text_selection(&selection, TaskTextField::Title, "Unexpected".to_string())
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "text mutation requires exactly one task");
    }

    #[tokio::test]
    async fn priority_cycle_uses_authoritative_value_after_selection_capture() {
        let mut store = test_store().await;
        let (task_id, selected) = create_selected_task(&mut store, "Priority race").await;
        let selection = crate::tui::task_selection::TaskSelection::from_ids(
            &store.tasks,
            std::slice::from_ref(&task_id),
            Some(selected),
        )
        .unwrap();
        store
            .database
            .update_task(
                &store.active_workspace,
                &task_id,
                TaskUpdate {
                    priority: Some("high".to_string()),
                    ..TaskUpdate::default()
                },
            )
            .await
            .unwrap();

        store
            .mutate_priority_selection(&selection, PriorityMutation::Cycle { reverse: false })
            .await
            .unwrap();

        assert_eq!(store.tasks[selected].task.priority, TaskPriority::Urgent);
    }

    #[tokio::test]
    async fn label_selection_uses_authoritative_labels_after_capture() {
        let mut store = test_store().await;
        for label in ["old", "docs", "bug"] {
            store.create_label(label.to_string()).await.unwrap();
        }
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Label race".to_string(),
                    labels: vec!["old".to_string()],
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();
        let selection = crate::tui::task_selection::TaskSelection::from_ids(
            &store.tasks,
            std::slice::from_ref(&task_id),
            Some(selected),
        )
        .unwrap();
        store
            .database
            .update_task(
                &store.active_workspace,
                &task_id,
                TaskUpdate {
                    add_labels: vec!["docs".to_string()],
                    remove_labels: vec!["old".to_string()],
                    ..TaskUpdate::default()
                },
            )
            .await
            .unwrap();

        store
            .mutate_labels_selection(&selection, vec!["bug".to_string()], Vec::new())
            .await
            .unwrap();

        assert_eq!(store.tasks[selected].labels, vec!["bug".to_string()]);
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

    #[tokio::test]
    async fn update_status_for_tasks_sets_status_on_each_marked_task() {
        let mut store = test_store().await;
        let (first_id, _) = create_selected_task(&mut store, "First").await;
        let (second_id, _) = create_selected_task(&mut store, "Second").await;
        let task_ids = vec![first_id.clone(), second_id.clone()];

        let outcome = store
            .update_status_for_tasks(None, &task_ids, "todo")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.message, "set status on 2 tasks");
        for task_id in task_ids {
            let item = store
                .tasks
                .iter()
                .find(|item| item.task.id == task_id)
                .unwrap();
            assert_eq!(item.task.status, TaskStatus::Todo);
        }
    }

    #[tokio::test]
    async fn set_exact_priority_for_tasks_sets_priority_on_each_marked_task() {
        let mut store = test_store().await;
        let (first_id, _) = create_selected_task(&mut store, "First").await;
        let (second_id, _) = create_selected_task(&mut store, "Second").await;
        let task_ids = vec![first_id.clone(), second_id.clone()];

        let outcome = store
            .set_exact_priority_for_tasks(None, &task_ids, "high")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.message, "set priority on 2 tasks");
        for task_id in task_ids {
            let item = store
                .tasks
                .iter()
                .find(|item| item.task.id == task_id)
                .unwrap();
            assert_eq!(item.task.priority, TaskPriority::High);
        }
    }

    #[tokio::test]
    async fn set_exact_priority_for_tasks_rolls_back_on_later_conflict() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (first_id, _) = create_selected_task(&mut store, "First atomic").await;
        let (second_id, _) = create_selected_task(&mut store, "Second atomic").await;
        seed_field_conflict_database(&store.database, &second_id, "priority").await;
        let undo_before = pending_undo_count(&pool, &store.active_workspace.id).await;

        let error = store
            .set_exact_priority_for_tasks(None, &[first_id.clone(), second_id], "high")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("conflicted-field"));
        store.refresh(None).await.unwrap();
        let first = store
            .tasks
            .iter()
            .find(|item| item.task.id == first_id)
            .unwrap();
        assert_eq!(first.task.priority, TaskPriority::None);
        assert_eq!(
            pending_undo_count(&pool, &store.active_workspace.id).await,
            undo_before
        );
    }

    #[tokio::test]
    async fn update_project_for_tasks_sets_project_on_each_marked_task() {
        let mut store = test_store().await;
        store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let (first_id, _) = create_selected_task(&mut store, "First").await;
        let (second_id, _) = create_selected_task(&mut store, "Second").await;
        let task_ids = vec![first_id.clone(), second_id.clone()];

        let outcome = store
            .update_project_for_tasks(None, &task_ids, "mobile-app".to_string())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.message, "set project on 2 tasks");
        for task_id in task_ids {
            let item = store
                .tasks
                .iter()
                .find(|item| item.task.id == task_id)
                .unwrap();
            assert_eq!(item.task.project_key, "mobile-app");
        }
    }

    #[tokio::test]
    async fn update_deleted_for_tasks_deletes_each_marked_task() {
        let mut store = test_store().await;
        let (first_id, _) = create_selected_task(&mut store, "First").await;
        let (second_id, _) = create_selected_task(&mut store, "Second").await;
        let task_ids = vec![first_id, second_id];

        let outcome = store
            .update_deleted_for_tasks(None, &task_ids, true)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.message, "deleted 2 tasks");
        assert!(store.tasks.is_empty());
    }

    #[tokio::test]
    async fn single_delete_refreshes_counts_and_clamps_stale_anchor() {
        let mut store = test_store().await;
        let (_first_id, _) = create_selected_task(&mut store, "First").await;
        let (second_id, second) = create_selected_task(&mut store, "Second").await;
        let selection = crate::tui::task_selection::TaskSelection::from_ids(
            &store.tasks,
            std::slice::from_ref(&second_id),
            Some(second),
        )
        .unwrap();
        store.tasks.remove(0);

        let outcome = store
            .mutate_deleted_selection(&selection, true, false)
            .await
            .unwrap();

        assert_eq!(store.counts.inbox, 1);
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(outcome.selected, Some(0));
    }

    #[tokio::test]
    async fn update_labels_for_tasks_sets_labels_on_each_marked_task() {
        let mut store = test_store().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("docs".to_string()).await.unwrap();
        let (_, first_selected) = store
            .create_task(
                TaskDraft {
                    title: "First".to_string(),
                    labels: vec!["bug".to_string()],
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let first_id = store.tasks[first_selected.unwrap()].task.id.clone();
        let (_, second_selected) = store
            .create_task(
                TaskDraft {
                    title: "Second".to_string(),
                    labels: vec!["docs".to_string()],
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let second_id = store.tasks[second_selected.unwrap()].task.id.clone();
        let task_ids = vec![first_id.clone(), second_id.clone()];

        let outcome = store
            .update_labels_for_tasks(
                None,
                &task_ids,
                vec!["bug".to_string(), "docs".to_string()],
                Vec::new(),
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.message, "set labels on 2 tasks");
        for task_id in task_ids {
            let item = store
                .tasks
                .iter()
                .find(|item| item.task.id == task_id)
                .unwrap();
            assert_eq!(item.labels, vec!["bug".to_string(), "docs".to_string()]);
        }
    }

    #[tokio::test]
    async fn update_labels_for_tasks_preserves_partial_membership() {
        let mut store = test_store().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("docs".to_string()).await.unwrap();
        let (first_id, first) = create_selected_task(&mut store, "First labels").await;
        store
            .update_labels(Some(first), vec!["bug".to_string()])
            .await
            .unwrap();
        let (second_id, second) = create_selected_task(&mut store, "Second labels").await;
        store
            .update_labels(Some(second), vec!["docs".to_string()])
            .await
            .unwrap();

        store
            .update_labels_for_tasks(
                None,
                &[first_id.clone(), second_id.clone()],
                vec!["bug".to_string()],
                vec!["docs".to_string()],
            )
            .await
            .unwrap();

        let labels_for = |store: &TuiStore, task_id: &TaskId| {
            store
                .tasks
                .iter()
                .find(|item| &item.task.id == task_id)
                .unwrap()
                .labels
                .clone()
        };
        assert_eq!(labels_for(&store, &first_id), vec!["bug".to_string()]);
        assert_eq!(
            labels_for(&store, &second_id),
            vec!["bug".to_string(), "docs".to_string()]
        );
    }

    #[tokio::test]
    async fn update_labels_for_tasks_reports_unchanged_batch() {
        let mut store = test_store().await;
        store.create_label("bug".to_string()).await.unwrap();
        let (task_id, selected) = create_selected_task(&mut store, "Stable labels").await;
        store
            .update_labels(Some(selected), vec!["bug".to_string()])
            .await
            .unwrap();

        let outcome = store
            .update_labels_for_tasks(None, &[task_id], vec!["bug".to_string()], Vec::new())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            outcome.message,
            format!("unchanged {} labels", store.tasks[selected].display_ref)
        );
    }

    #[tokio::test]
    async fn committed_mutation_reports_refresh_failure_without_rolling_back() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Refresh failure").await;
        store.fail_next_refresh();

        let error = store
            .update_status(Some(selected), "todo")
            .await
            .unwrap_err();

        assert!(mutation_committed(&error));
        assert!(error.to_string().contains("injected refresh failure"));
        let persisted: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, "todo");
        assert_eq!(store.tasks[selected].task.status, TaskStatus::Inbox);
    }

    #[tokio::test]
    async fn single_mutation_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Single undo failure").await;
        let workspace_id = store.active_workspace.id.clone();
        let undo_before = pending_undo_count(&pool, &workspace_id).await;
        reject_undo_inserts(&pool).await;

        let error = store
            .update_status(Some(selected), "todo")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, "inbox");
        assert_eq!(store.tasks[selected].task.status, TaskStatus::Inbox);
        assert_eq!(pending_undo_count(&pool, &workspace_id).await, undo_before);
    }

    #[tokio::test]
    async fn batch_mutation_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (first_id, _) = create_selected_task(&mut store, "First undo failure").await;
        let (second_id, _) = create_selected_task(&mut store, "Second undo failure").await;
        let workspace_id = store.active_workspace.id.clone();
        let undo_before = pending_undo_count(&pool, &workspace_id).await;
        reject_undo_inserts(&pool).await;

        let error = store
            .set_exact_priority_for_tasks(None, &[first_id.clone(), second_id.clone()], "high")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        for task_id in [&first_id, &second_id] {
            let persisted: String = sqlx::query_scalar("SELECT priority FROM tasks WHERE id = ?")
                .bind(task_id)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(persisted, "none");
            let cached = store
                .tasks
                .iter()
                .find(|item| &item.task.id == task_id)
                .unwrap();
            assert_eq!(cached.task.priority, TaskPriority::None);
        }
        assert_eq!(pending_undo_count(&pool, &workspace_id).await, undo_before);
    }

    #[tokio::test]
    async fn deletion_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Delete undo failure").await;
        reject_undo_inserts(&pool).await;

        let error = store
            .update_deleted(Some(selected), true)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: i64 = sqlx::query_scalar("SELECT deleted FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, 0);
    }

    #[tokio::test]
    async fn label_assignment_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store.create_label("atomic".to_string()).await.unwrap();
        let (task_id, selected) = create_selected_task(&mut store, "Label undo failure").await;
        reject_undo_inserts(&pool).await;

        let error = store
            .update_labels_for_tasks(
                Some(selected),
                std::slice::from_ref(&task_id),
                vec!["atomic".to_string()],
                Vec::new(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(persisted, 0);
    }

    #[tokio::test]
    async fn project_assignment_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        store
            .create_project("Atomic Project".to_string())
            .await
            .unwrap();
        let (task_id, selected) = create_selected_task(&mut store, "Project undo failure").await;
        let before: String = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        reject_undo_inserts(&pool).await;

        let error = store
            .update_project_for_tasks(
                Some(selected),
                std::slice::from_ref(&task_id),
                "atomic-project".to_string(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: String = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, before);
    }

    #[tokio::test]
    async fn date_mutation_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Date undo failure").await;
        reject_undo_inserts(&pool).await;

        let error = store
            .update_due_for_tasks(
                Some(selected),
                std::slice::from_ref(&task_id),
                "2026-08-01".to_string(),
                false,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: String = sqlx::query_scalar("SELECT due_on FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, "");
    }

    #[tokio::test]
    async fn unchanged_single_and_batch_edits_preserve_state_without_undo() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Unchanged edits").await;
        let workspace_id = store.active_workspace.id.clone();
        let undo_before = pending_undo_count(&pool, &workspace_id).await;
        let changes_before = pending_change_count(&pool).await;

        let availability = store
            .update_availability(Some(selected), String::new(), false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            availability.message,
            format!(
                "unchanged {} availability",
                store.tasks[selected].display_ref
            )
        );
        assert_selected_task(&store, &availability, &task_id);

        let status = store
            .update_status_for_tasks(Some(selected), std::slice::from_ref(&task_id), "inbox")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            status.message,
            format!(
                "unchanged {} status=inbox",
                store.tasks[selected].display_ref
            )
        );
        assert_selected_task(&store, &status, &task_id);

        let priority = store
            .set_exact_priority_for_tasks(status.selected, std::slice::from_ref(&task_id), "none")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            priority.message,
            format!(
                "unchanged {} priority=none",
                store.tasks[selected].display_ref
            )
        );
        assert_selected_task(&store, &priority, &task_id);

        let project = store
            .update_project_for_tasks(
                priority.selected,
                std::slice::from_ref(&task_id),
                "aven".to_string(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            project.message,
            format!("unchanged {} project", store.tasks[selected].display_ref)
        );
        assert_selected_task(&store, &project, &task_id);

        let labels = store
            .update_labels_for_tasks(
                project.selected,
                std::slice::from_ref(&task_id),
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            labels.message,
            format!("unchanged {} labels", store.tasks[selected].display_ref)
        );
        assert_selected_task(&store, &labels, &task_id);

        let availability = store
            .update_availability_for_tasks(
                labels.selected,
                std::slice::from_ref(&task_id),
                String::new(),
                false,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            availability.message,
            format!(
                "unchanged {} availability",
                store.tasks[selected].display_ref
            )
        );
        assert_selected_task(&store, &availability, &task_id);

        let due = store
            .update_due_for_tasks(
                availability.selected,
                std::slice::from_ref(&task_id),
                String::new(),
                false,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            due.message,
            format!("unchanged {} due date", store.tasks[selected].display_ref)
        );
        assert_selected_task(&store, &due, &task_id);

        let deleted = store
            .update_deleted_for_tasks(due.selected, std::slice::from_ref(&task_id), false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            deleted.message,
            format!("already restored {}", store.tasks[selected].display_ref)
        );
        assert_selected_task(&store, &deleted, &task_id);

        assert_eq!(pending_change_count(&pool).await, changes_before);
        assert_eq!(pending_undo_count(&pool, &workspace_id).await, undo_before);
    }

    #[tokio::test]
    async fn mutation_families_restore_selection_after_refresh() {
        let mut store = test_store().await;
        store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        store.create_label("bug".to_string()).await.unwrap();
        let (target_id, _) = create_selected_task(&mut store, "Mutation target").await;
        let (selected_id, _) = create_selected_task(&mut store, "Selection anchor").await;
        let selected = store
            .tasks
            .iter()
            .position(|item| item.task.id == selected_id)
            .unwrap();

        let status = store
            .update_status_for_tasks(Some(selected), std::slice::from_ref(&target_id), "todo")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.selected, Some(selected));
        let selected = store
            .tasks
            .iter()
            .position(|item| item.task.id == selected_id)
            .unwrap();

        let priority = store
            .set_exact_priority_for_tasks(Some(selected), std::slice::from_ref(&target_id), "high")
            .await
            .unwrap()
            .unwrap();
        assert_selected_task(&store, &priority, &selected_id);

        let project = store
            .update_project_for_tasks(
                priority.selected,
                std::slice::from_ref(&target_id),
                "mobile-app".to_string(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_selected_task(&store, &project, &selected_id);

        let labels = store
            .update_labels_for_tasks(
                project.selected,
                std::slice::from_ref(&target_id),
                vec!["bug".to_string()],
                Vec::new(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_selected_task(&store, &labels, &selected_id);

        let availability = store
            .update_availability_for_tasks(
                labels.selected,
                std::slice::from_ref(&target_id),
                "2020-01-01T00:00:00Z".to_string(),
                false,
            )
            .await
            .unwrap()
            .unwrap();
        assert_selected_task(&store, &availability, &selected_id);

        let due = store
            .update_due_for_tasks(
                availability.selected,
                std::slice::from_ref(&target_id),
                "2099-01-01".to_string(),
                false,
            )
            .await
            .unwrap()
            .unwrap();
        assert_selected_task(&store, &due, &selected_id);

        let deleted = store
            .update_deleted_for_tasks(due.selected, std::slice::from_ref(&target_id), true)
            .await
            .unwrap()
            .unwrap();
        assert_selected_task(&store, &deleted, &selected_id);
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
        let pool = crate::test_support::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(
            aven_core::db::Database::open(&dir.path().join("test.db"))
                .await
                .unwrap(),
            crate::workspaces::Workspace::default(),
        )
        .await
        .unwrap();
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
    async fn conflict_resolution_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Conflict undo failure").await;
        let display_ref = store.tasks[selected].display_ref.clone();
        seed_title_conflict(&pool, &task_id).await;
        reject_undo_inserts(&pool).await;

        let error = store
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
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let title: String = sqlx::query_scalar("SELECT title FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let resolved: i64 = sqlx::query_scalar("SELECT resolved FROM conflicts WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(title, "Conflict undo failure");
        assert_eq!(resolved, 0);
    }

    #[tokio::test]
    async fn resolve_missing_conflict_leaves_task_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::test_support::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(
            aven_core::db::Database::open(&dir.path().join("test.db"))
                .await
                .unwrap(),
            crate::workspaces::Workspace::default(),
        )
        .await
        .unwrap();
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
        let pool = crate::test_support::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(
            aven_core::db::Database::open(&dir.path().join("test.db"))
                .await
                .unwrap(),
            crate::workspaces::Workspace::default(),
        )
        .await
        .unwrap();
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
    async fn availability_transition_refreshes_tasks_sidebar_and_project_counts() {
        let (dir, pool, mut store) = test_store_with_pool().await;
        let (message, selected) = store
            .create_task(
                TaskDraft {
                    title: "Scheduled store task".to_string(),
                    available_at: Some("2999-03-08T05:00:00Z".to_string()),
                    due_on: None,
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();

        assert!(selected.is_none());
        assert!(message.contains("hidden by current filters"));
        assert!(store.tasks.is_empty());
        assert_eq!(store.counts.open, 0);
        assert_eq!(store.counts.inbox, 0);
        assert_eq!(store.counts.upcoming, 1);
        let project = store
            .projects
            .iter()
            .find(|project| project.key == "aven")
            .unwrap();
        assert_eq!(project.open_count, 0);
        assert_eq!(project.inbox_count, 0);

        store.show_view(TaskView::Upcoming).await.unwrap();
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Scheduled store task");
        let task_id = store.tasks[0].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("UPDATE tasks SET available_at = ? WHERE id = ?")
            .bind("2026-03-08T05:00:00Z")
            .bind(&task_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        store.show_view(TaskView::Queue).await.unwrap();

        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.id, task_id);
        assert_eq!(
            store.tasks[0].queue.band,
            crate::queue::QueueBand::Available
        );
        assert_eq!(store.counts.open, 1);
        assert_eq!(store.counts.inbox, 1);
        assert_eq!(store.counts.upcoming, 0);
        let project = store
            .projects
            .iter()
            .find(|project| project.key == "aven")
            .unwrap();
        assert_eq!(project.open_count, 1);
        assert_eq!(project.inbox_count, 1);
    }

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
        store.view_state.filter_modifiers.task_ids = vec![crate::test_support::task_id("task-1")];

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

    async fn create_search_task(store: &mut TuiStore, title: &str) -> TaskId {
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
        let mut draft = task_draft("Preview needle");
        draft.is_epic = true;
        let (_, selected) = store.create_task(draft, None).await.unwrap();
        let task_id = store.tasks[selected.unwrap()].task.id.clone();
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
        assert!(result.is_epic);
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

        let pool = store.database.clone();
        seed_title_conflict_database(&pool, &task_id).await;
        store.refresh(Some(&task_id)).await.unwrap();

        let mut conn = aven_core::test_support::acquire(&store.database)
            .await
            .unwrap();
        crate::operations::add_task_dependency(
            &mut conn,
            &crate::workspaces::Workspace::default(),
            &task_id,
            &blocker_id,
        )
        .await
        .unwrap();
        drop(conn);

        let mut conn = aven_core::test_support::acquire(&store.database)
            .await
            .unwrap();
        crate::operations::add_task_dependency(
            &mut conn,
            &crate::workspaces::Workspace::default(),
            &dependent_id,
            &task_id,
        )
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
    async fn upcoming_keeps_availability_as_effective_order() {
        let mut store = test_store().await;
        store.show_view(TaskView::Upcoming).await.unwrap();
        store.set_order(TaskOrder::DueOn).await.unwrap();
        store.reverse_sort().await.unwrap();

        assert_eq!(store.view_state.view, TaskView::Upcoming);
        assert_eq!(store.view_state.sort(), crate::query::TaskSort::AvailableAt);
        assert_eq!(store.view_state.sort_direction(), SortDirection::Asc);
        assert_eq!(store.sort_label(), "available");
        assert_eq!(store.sort_direction_label(), "asc");
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
    async fn explicit_workspace_payloads_pair_id_and_key() {
        let (_dir, pool, _store) = test_store_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let other = crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();

        crate::operations::create_label_operation(&mut conn, &other, "Needs Review")
            .await
            .unwrap();
        assert_workspace_payload(
            &latest_payload(&mut conn, "label", "create_label").await,
            &other,
        );

        let task = crate::operations::create_task(
            &mut conn,
            &other,
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

        crate::operations::create_label_operation(&mut conn, &other, "Docs")
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
        let (dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Before").await;
        store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();
        assert_eq!(store.tasks[selected].task.title, "After");

        let mut restarted = TuiStore::new(
            aven_core::db::Database::open(&dir.path().join("test.db"))
                .await
                .unwrap(),
            crate::workspaces::Workspace::default(),
        )
        .await
        .unwrap();
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let _restarted = TuiStore::new(
            aven_core::db::Database::open(&dir.path().join("test.db"))
                .await
                .unwrap(),
            crate::workspaces::Workspace::default(),
        )
        .await
        .unwrap();

        assert_eq!(pending_undo_count(&pool, &workspace_id).await, 0);
        assert_eq!(
            consumed_undo_count(&pool, &workspace_id).await,
            consumed_before
        );
    }

    #[tokio::test]
    async fn undo_guard_blocks_stale_task_field() {
        let (dir, pool, mut store) = test_store_with_pool().await;
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
    async fn single_task_undo_summary_keeps_display_ref() {
        let mut store = test_store().await;
        let (_task_id, selected) = create_selected_task(&mut store, "Single summary").await;
        let display_ref = store.tasks[selected].display_ref.clone();

        store.update_status(Some(selected), "todo").await.unwrap();
        let undo = store.undo_last(None).await.unwrap().unwrap();

        assert_eq!(undo.message, format!("undid status {display_ref}"));
    }

    #[tokio::test]
    async fn partially_unchanged_batch_undo_summary_uses_changed_count() {
        let mut store = test_store().await;
        let (first_id, first) = create_selected_task(&mut store, "Already changed").await;
        let (second_id, _) = create_selected_task(&mut store, "Needs change").await;
        store.update_status(Some(first), "todo").await.unwrap();

        store
            .update_status_for_tasks(None, &[first_id, second_id], "todo")
            .await
            .unwrap();
        let undo = store.undo_last(None).await.unwrap().unwrap();

        assert_eq!(undo.message, "undid status 1 task");
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
        let (dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, selected) = create_selected_task(&mut store, "Keep once").await;
        store.view_state.filter_modifiers.include_deleted = true;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
    async fn undo_atomic_attachment_task_removes_all_database_rows() {
        let (dir, pool, mut store) = test_store_with_pool().await;
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 1))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let pending = crate::tui::authoring::PendingTaskAttachment::new(
            "ATTACHMENT000001".to_string(),
            crate::operations::AttachmentAddInput {
                filename: Some("image.png".to_string()),
                alt_text: None,
                declared_media_type: Some("image/png".to_string()),
                bytes: bytes.into_inner(),
                optimization_policy:
                    crate::attachments::optimization::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        );
        store
            .create_task_with_attachments(
                task_draft("Temporary attachment task"),
                None,
                &dir.path().join("blobs"),
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                vec![pending],
            )
            .await
            .unwrap();
        let task_id = store.tasks[0].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        let undo_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tui_undo_entries")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(undo_count, 1);
        drop(conn);

        store.undo_last(None).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let attachment_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let change_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM changes WHERE entity_id = ?")
                .bind(&task_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!((task_count, attachment_count, change_count), (0, 0, 0));
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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
        let (dir, pool, mut store) = test_store_with_pool().await;
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

    #[tokio::test]
    async fn update_labels_for_tasks_records_single_undo_payload() {
        let mut store = test_store().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("docs".to_string()).await.unwrap();
        let (first_id, first) = create_selected_task(&mut store, "First").await;
        store
            .update_labels(Some(first), vec!["bug".to_string()])
            .await
            .unwrap();
        let (second_id, second) = create_selected_task(&mut store, "Second").await;
        store
            .update_labels(Some(second), vec!["docs".to_string()])
            .await
            .unwrap();

        store
            .update_labels_for_tasks(
                None,
                &[first_id.clone(), second_id.clone()],
                vec!["bug".to_string(), "docs".to_string()],
                Vec::new(),
            )
            .await
            .unwrap();

        store.undo_last(None).await.unwrap().unwrap();
        store.refresh(None).await.unwrap();
        let first = store
            .tasks
            .iter()
            .find(|item| item.task.id == first_id)
            .unwrap();
        let second = store
            .tasks
            .iter()
            .find(|item| item.task.id == second_id)
            .unwrap();
        assert_eq!(first.labels, vec!["bug".to_string()]);
        assert_eq!(second.labels, vec!["docs".to_string()]);
    }
}

mod onboarding {
    use super::*;

    async fn set_marker(store: &TuiStore, value: &str) {
        let mut conn = aven_core::test_support::acquire(&store.database)
            .await
            .unwrap();
        aven_core::test_support::set_meta(&mut conn, "tui_onboarding_version", value)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn fresh_database_is_due_until_completed() {
        let store = test_store().await;
        assert_eq!(
            store.onboarding_status().await.unwrap(),
            OnboardingStatus::Due
        );

        store.complete_onboarding().await.unwrap();

        assert_eq!(
            store.onboarding_status().await.unwrap(),
            OnboardingStatus::Complete
        );
    }

    #[tokio::test]
    async fn established_database_is_not_treated_as_first_launch() {
        let mut store = test_store().await;
        create_selected_task(&mut store, "Existing task").await;

        assert_eq!(
            store.onboarding_status().await.unwrap(),
            OnboardingStatus::Established
        );
    }

    #[tokio::test]
    async fn marker_parsing_is_trimmed_and_downgrade_safe() {
        let store = test_store().await;
        for value in [" 1 ", "2", "4294967295"] {
            set_marker(&store, value).await;
            assert_eq!(
                store.onboarding_status().await.unwrap(),
                OnboardingStatus::Complete,
                "marker {value:?}"
            );
        }
    }

    #[tokio::test]
    async fn invalid_or_older_markers_are_due_for_empty_database() {
        let store = test_store().await;
        for value in ["0", "-1", "invalid", "4294967296"] {
            set_marker(&store, value).await;
            assert_eq!(
                store.onboarding_status().await.unwrap(),
                OnboardingStatus::Due,
                "marker {value:?}"
            );
        }
    }

    #[tokio::test]
    async fn completion_preserves_future_marker() {
        let store = test_store().await;
        set_marker(&store, "2").await;

        store.complete_onboarding().await.unwrap();

        let mut conn = aven_core::test_support::acquire(&store.database)
            .await
            .unwrap();
        assert_eq!(
            aven_core::test_support::get_meta(&mut conn, "tui_onboarding_version")
                .await
                .unwrap()
                .as_deref(),
            Some("2")
        );
    }
}

mod workspace_scoping {
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
        assert_eq!(draft.project.as_deref(), Some("default-only"));
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
        store.view_state.filter_modifiers.task_ids = vec![crate::test_support::task_id("task-1")];
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
}

mod epics {
    use super::*;

    async fn create_epic_child_pair(store: &mut TuiStore) -> (TaskId, TaskId, usize) {
        let (parent_id, _parent_index) = create_selected_task(store, "epic parent").await;
        let child_title = format!("child of {}", &parent_id[..4]);
        let (child_id, _) = create_selected_task(store, &child_title).await;

        let mut conn = aven_core::test_support::acquire(&store.database)
            .await
            .unwrap();
        crate::operations::add_task_to_epic(
            &mut conn,
            &crate::workspaces::Workspace::default(),
            &child_id,
            &parent_id,
        )
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
    async fn status_change_from_collapsed_epic_selects_remaining_row() {
        let mut store = test_store().await;
        let (first_parent_id, _, _) = create_epic_child_pair(&mut store).await;
        store.show_view(TaskView::Queue).await.unwrap();
        let (second_parent_id, _, _) = create_epic_child_pair(&mut store).await;
        let first_parent = store
            .tasks
            .iter()
            .position(|task| task.task.id == first_parent_id)
            .unwrap();
        store
            .toggle_selected_epic(Some(first_parent))
            .await
            .unwrap()
            .unwrap();
        let first_parent = store
            .tasks
            .iter()
            .position(|task| task.task.id == first_parent_id)
            .unwrap();

        let result = store
            .update_status(Some(first_parent), "done")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.selected, Some(0));
        assert_eq!(store.tasks[0].task.id, second_parent_id);
        assert!(
            !store
                .view_state
                .collapsed_epic_ids
                .contains(&first_parent_id)
        );
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
    async fn remove_epic_child_uses_resolved_ids_and_records_undo() {
        let mut store = test_store().await;
        let (parent_id, child_id, _parent_index) = create_epic_child_pair(&mut store).await;
        let child_index = store
            .tasks
            .iter()
            .position(|task| task.task.id == child_id)
            .unwrap();
        let target = store
            .resolve_epic_child_target(Some(child_index), None)
            .unwrap();

        let outcome = store.remove_epic_child(target).await.unwrap();

        assert!(outcome.changed);
        assert!(outcome.message.message.contains("Removed"));
        let parent_index = store
            .tasks
            .iter()
            .position(|task| task.task.id == parent_id)
            .unwrap();
        assert!(
            store.tasks[parent_index]
                .epic_children
                .iter()
                .all(|child| child.task_id != child_id)
        );

        store.undo_last(None).await.unwrap().unwrap();
        let parent = store
            .tasks
            .iter()
            .find(|task| task.task.id == parent_id)
            .unwrap();
        assert!(
            parent
                .epic_children
                .iter()
                .any(|child| child.task_id == child_id)
        );
    }

    #[tokio::test]
    async fn epic_membership_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (parent_id, parent_index) = create_selected_task(&mut store, "Atomic epic").await;
        let (child_id, _) = create_selected_task(&mut store, "Atomic child").await;
        let epic = EpicContext {
            epic_id: parent_id.clone(),
            display_ref: store.tasks[parent_index].display_ref.clone(),
            project_key: store.tasks[parent_index].task.project_key.clone(),
        };
        reject_undo_inserts(&pool).await;

        let error = store
            .add_epic_child(epic, child_id.clone())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let linked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM task_epic_links
             WHERE epic_task_id = ? AND child_task_id = ?",
        )
        .bind(&parent_id)
        .bind(&child_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, 0);
        let is_epic: i64 = sqlx::query_scalar("SELECT is_epic FROM tasks WHERE id = ?")
            .bind(&parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(is_epic, 0);
    }

    #[tokio::test]
    async fn add_epic_child_and_undo_remove_relationship() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (parent_id, parent_index) = create_selected_task(&mut store, "Parent epic").await;
        let (child_id, _) = create_selected_task(&mut store, "Child task").await;
        let epic = store
            .resolve_epic_context(Some(parent_index), false)
            .unwrap_or_else(|| EpicContext {
                epic_id: parent_id.clone(),
                display_ref: store.tasks[parent_index].display_ref.clone(),
                project_key: store.tasks[parent_index].task.project_key.clone(),
            });

        let outcome = store.add_epic_child(epic, child_id.clone()).await.unwrap();

        assert!(outcome.changed);
        store.undo_last(None).await.unwrap().unwrap();
        let linked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM task_epic_links WHERE epic_task_id = ? AND child_task_id = ?",
        )
        .bind(&parent_id)
        .bind(&child_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, 0);
    }

    #[tokio::test]
    async fn epic_child_creation_rolls_back_task_when_link_validation_fails() {
        let (_dir, pool, store) = test_store_with_pool().await;
        let parent = store
            .database
            .create_task(
                &store.active_workspace,
                TaskDraft {
                    project: Some("parent-project".to_string()),
                    is_epic: true,
                    ..task_draft("Parent epic")
                },
            )
            .await
            .unwrap();

        let error = store
            .database
            .create_task_for_epic(
                &store.active_workspace,
                TaskDraft {
                    project: Some("child-project".to_string()),
                    ..task_draft("Unexpected standalone child")
                },
                &parent.task.id,
            )
            .await
            .unwrap_err();

        assert!(format!("{error:#}").contains("epic-cross-project"));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = ?")
            .bind("Unexpected standalone child")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn remove_completed_epic_child() {
        let mut store = test_store().await;
        let (_parent_id, child_id, _parent_index) = create_epic_child_pair(&mut store).await;
        let child_index = store
            .tasks
            .iter()
            .position(|task| task.task.id == child_id)
            .unwrap();
        store
            .update_status(Some(child_index), "done")
            .await
            .unwrap();
        store.view_state.view = TaskView::Epics;
        store.refresh(None).await.unwrap();
        let child_index = store
            .tasks
            .iter()
            .position(|task| task.task.id == child_id)
            .unwrap();
        let target = store
            .resolve_epic_child_target(Some(child_index), None)
            .unwrap();

        let outcome = store.remove_epic_child(target).await.unwrap();

        assert!(outcome.changed);
    }
}
mod dependency_actions {
    use super::*;

    #[tokio::test]
    async fn dependency_add_rolls_back_when_undo_recording_fails() {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (blocker_id, _) = create_selected_task(&mut store, "Atomic blocker").await;
        let (task_id, selected) = create_selected_task(&mut store, "Atomic blocked").await;
        reject_undo_inserts(&pool).await;

        let error = store
            .add_dependency(Some(selected), &blocker_id)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("injected undo failure"));
        let persisted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM task_dependencies
             WHERE task_id = ? AND depends_on_task_id = ?",
        )
        .bind(&task_id)
        .bind(&blocker_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(persisted, 0);
    }

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
