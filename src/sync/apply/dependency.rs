use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::operations::dependency_path_exists;
use crate::sync::wire::ChangeWire;

use super::shared::{str_payload, workspace_id_payload};

pub(super) async fn add_dependency(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let depends_on_task_id = str_payload(&change.payload, "depends_on_task_id")?;
    ensure_dependency_tasks_exist(conn, &workspace_id, &change.entity_id, &depends_on_task_id)
        .await?;
    if dependency_path_exists(conn, &workspace_id, &depends_on_task_id, &change.entity_id).await? {
        if !remote_dependency_wins(&change.entity_id, &depends_on_task_id) {
            return Ok(());
        }
        sqlx::query(
            "DELETE FROM task_dependencies
             WHERE workspace_id = ? AND task_id = ? AND depends_on_task_id = ?",
        )
        .bind(&workspace_id)
        .bind(&depends_on_task_id)
        .bind(&change.entity_id)
        .execute(&mut *conn)
        .await?;
        if dependency_path_exists(conn, &workspace_id, &depends_on_task_id, &change.entity_id)
            .await?
        {
            return Ok(());
        }
    }
    sqlx::query(
        "INSERT OR IGNORE INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(&depends_on_task_id)
    .bind(&change.created_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(super) async fn remove_dependency(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let depends_on_task_id = str_payload(&change.payload, "depends_on_task_id")?;
    sqlx::query(
        "DELETE FROM task_dependencies WHERE workspace_id = ? AND task_id = ? AND depends_on_task_id = ?",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(&depends_on_task_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn ensure_dependency_tasks_exist(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    depends_on_task_id: &str,
) -> Result<()> {
    let existing: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tasks WHERE workspace_id = ? AND id IN (?, ?)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(depends_on_task_id)
    .fetch_one(&mut *conn)
    .await?;
    if existing != 2 {
        bail!(
            "error dependency-missing-task task_id={task_id} depends_on_task_id={depends_on_task_id}"
        );
    }
    Ok(())
}

fn remote_dependency_wins(task_id: &str, depends_on_task_id: &str) -> bool {
    task_id < depends_on_task_id
}
