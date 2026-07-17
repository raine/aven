use crate::ids::WorkspaceId;
use crate::query::{TaskListItem, TaskSearchResult};
pub(super) use crate::test_support::test_conn;
use sqlx::SqliteConnection;

pub(super) async fn seed_default_project(conn: &mut SqliteConnection) {
    sqlx::query(
        "INSERT INTO projects(id, key, name, prefix, created_at, updated_at)
         VALUES ('0000000000000001', 'app', 'app', 'APP', 't', 't')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
}

pub(super) async fn insert_test_task(
    conn: &mut SqliteConnection,
    id: &str,
    title: &str,
    status: &str,
    priority: &str,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
         VALUES (?, ?, '', '0000000000000001', ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(title)
    .bind(status)
    .bind(priority)
    .bind(created_at)
    .bind(created_at)
    .bind(created_at)
    .execute(&mut *conn)
    .await
    .unwrap();
}

pub(super) async fn insert_test_label(conn: &mut SqliteConnection, task_id: &str, label: &str) {
    let workspace_id = crate::workspaces::default_workspace_id();
    sqlx::query("INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, 't')")
        .bind(&workspace_id)
        .bind(label)
        .execute(&mut *conn)
        .await
        .unwrap();

    sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
        .bind(&workspace_id)
        .bind(task_id)
        .bind(label)
        .execute(&mut *conn)
        .await
        .unwrap();
}

pub(super) async fn insert_test_conflict(
    conn: &mut SqliteConnection,
    task_id: &str,
    resolved: bool,
) {
    let resolved = if resolved { 1_i64 } else { 0_i64 };
    sqlx::query(
        "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, ?, 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, ?)",
    )
    .bind(crate::workspaces::DEFAULT_WORKSPACE_ID.to_string())
    .bind(task_id)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .bind(resolved)
    .execute(&mut *conn)
    .await
    .unwrap();
}

pub(super) fn listed_titles(items: &[TaskListItem]) -> Vec<&str> {
    items.iter().map(|item| item.task.title.as_str()).collect()
}

pub(super) fn listed_titles_from_search(items: &[TaskSearchResult]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item.item.task.title.as_str())
        .collect()
}

pub(super) async fn seed_workspace_project(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    key: &str,
    name: &str,
    prefix: &str,
) {
    sqlx::query(
        "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 't', 't')",
    )
    .bind(crate::ids::new_id())
    .bind(workspace_id)
    .bind(key)
    .bind(name)
    .bind(prefix)
    .execute(&mut *conn)
    .await
    .unwrap();
}

pub(super) async fn seed_workspace_label(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
) {
    sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, ?, 't')")
        .bind(workspace_id)
        .bind(name)
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn seed_workspace_task(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    id: &str,
    title: &str,
    project_key: &str,
    status: &str,
    priority: &str,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
         VALUES (?, ?, ?, '', (SELECT id FROM projects WHERE workspace_id = ? AND key = ?), ?, ?, ?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(id)
    .bind(title)
    .bind(workspace_id)
    .bind(project_key)
    .bind(status)
    .bind(priority)
    .bind(created_at)
    .bind(created_at)
    .bind(created_at)
    .execute(&mut *conn)
    .await
    .unwrap();
}

pub(super) async fn seed_workspace_task_label(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &str,
    label: &str,
) {
    sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
        .bind(workspace_id)
        .bind(task_id)
        .bind(label)
        .execute(&mut *conn)
        .await
        .unwrap();
}

pub(super) async fn seed_workspace_conflict(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &str,
) {
    sqlx::query(
        "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, ?, 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
}
