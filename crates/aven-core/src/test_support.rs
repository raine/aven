use anyhow::Result;
use sqlx::{Sqlite, SqliteConnection, pool::PoolConnection};

use crate::db::Database;
use crate::ids::{BASE32, TaskId, WorkspaceId};
use crate::operations::{
    DependencyOutcome, EpicLinkOutcome, LabelOutcome, TaskDraft, TaskOutcome, TaskUpdate,
    TaskUpdateOutcome,
};
use crate::types::Project;
use crate::workspaces::Workspace;

pub fn task_id(value: &str) -> TaskId {
    let mut encoded = value
        .bytes()
        .map(|byte| match byte.to_ascii_uppercase() {
            b'O' => '0',
            b'I' | b'L' => '1',
            byte if BASE32.contains(&byte) => char::from(byte),
            byte => char::from(BASE32[usize::from(byte) % BASE32.len()]),
        })
        .take(16)
        .collect::<String>();
    encoded.extend(std::iter::repeat_n('0', 16 - encoded.len()));
    encoded.parse().unwrap()
}

pub async fn acquire(database: &Database) -> Result<PoolConnection<Sqlite>> {
    database.acquire_reader().await
}

pub async fn ensure_default_workspace(conn: &mut SqliteConnection) -> Result<Workspace> {
    crate::workspaces::ensure_default_workspace(conn).await
}

pub async fn create_workspace(conn: &mut SqliteConnection, name: &str) -> Result<Workspace> {
    crate::workspaces::create_workspace(conn, name).await
}

pub async fn create_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
) -> Result<crate::projects::ProjectCreateOutcome> {
    crate::projects::create_project_in_workspace(conn, workspace_id, name).await
}

pub async fn resolve_or_create_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
) -> Result<Project> {
    crate::projects::resolve_or_create_project_in_workspace(conn, workspace_id, name).await
}

pub async fn create_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
) -> Result<TaskOutcome> {
    crate::operations::create_task(conn, workspace, draft).await
}

pub async fn update_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    update: TaskUpdate,
) -> Result<TaskUpdateOutcome> {
    crate::operations::update_task(conn, workspace, task_id, update).await
}

pub async fn set_task_deleted(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    deleted: bool,
) -> Result<TaskOutcome> {
    let outcome = crate::operations::update_task(
        conn,
        workspace,
        task_id,
        TaskUpdate {
            deleted: Some(deleted),
            ..TaskUpdate::default()
        },
    )
    .await?;
    Ok(TaskOutcome {
        task: outcome.task,
        create_change_id: None,
        attachment_change_ids: Vec::new(),
    })
}

pub async fn update_task_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    add: &[String],
    remove: &[String],
) -> Result<bool> {
    crate::operations::update_task_labels_in_workspace(conn, workspace_id, task_id, add, remove)
        .await
}

pub async fn add_task_dependency(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    depends_on_id: &TaskId,
) -> Result<DependencyOutcome> {
    crate::operations::add_task_dependency(conn, workspace, task_id, depends_on_id).await
}

pub async fn add_task_to_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &TaskId,
    epic_id: &TaskId,
) -> Result<EpicLinkOutcome> {
    crate::operations::add_task_to_epic(conn, workspace, child_id, epic_id).await
}

pub async fn create_label_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
) -> Result<LabelOutcome> {
    crate::operations::create_label_operation(conn, workspace, name).await
}

pub async fn list_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    search: Option<&str>,
) -> Result<Vec<String>> {
    crate::labels::list_labels_in_workspace(conn, workspace_id, search).await
}

pub async fn get_meta(conn: &mut SqliteConnection, key: &str) -> Result<Option<String>> {
    crate::db::get_meta(conn, key).await
}

pub async fn set_meta(conn: &mut SqliteConnection, key: &str, value: &str) -> Result<()> {
    crate::db::set_meta(conn, key, value).await
}

#[cfg(test)]
pub async fn test_conn() -> (tempfile::TempDir, PoolConnection<Sqlite>) {
    let temp = tempfile::tempdir().unwrap();
    let database = Database::open(&temp.path().join("test.sqlite"))
        .await
        .unwrap();
    let conn = database.acquire_reader().await.unwrap();
    (temp, conn)
}
