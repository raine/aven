#![allow(unused_variables)]

use super::*;
use crate::ids::{TaskId, WorkspaceId};

use crate::choices::{PRIORITIES, TaskPriority, TaskSource, TaskStatus};
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
    let mut store = TuiStore::new(database, crate::workspaces::Workspace::default())
        .await
        .unwrap();
    store._test_database_dir = Some(std::sync::Arc::new(dir));
    store
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
                metadata: Vec::new(),
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
        metadata: Vec::new(),
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
                metadata: Vec::new(),
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

mod mutation_wakeup;

mod detail_loading;

mod domain_mutations_and_pickers;

mod task_creation_and_updates;

mod conflicts;

mod views_filters_and_sort;

mod sync_workspace_payloads;

mod undo;

mod onboarding;

mod workspace_scoping;

mod dependency_actions;
mod epics;

mod label_administration;

mod recurrence_surfaces;
