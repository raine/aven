use std::collections::HashSet;

use anyhow::{Result, bail};
use serde_json::json;
use sqlx::SqliteConnection;

use crate::db::{begin_immediate, insert_change};
use crate::ids::now;
use crate::refs::get_task;
use crate::workspaces::workspace_for_id;

pub(crate) struct DependencyOutcome {
    pub(crate) task: crate::types::Task,
    pub(crate) depends_on: crate::types::Task,
    pub(crate) changed: bool,
}

pub(crate) async fn add_task_dependency(
    conn: &mut SqliteConnection,
    task_id: &str,
    depends_on_id: &str,
) -> Result<DependencyOutcome> {
    if task_id == depends_on_id {
        bail!("error dependency-self task_id={task_id}");
    }

    let mut tx = begin_immediate(conn).await?;
    let task = get_task(&mut tx, task_id).await?;
    let depends_on = get_task(&mut tx, depends_on_id).await?;

    if task.workspace_id != depends_on.workspace_id {
        bail!(
            "error dependency-cross-workspace task_id={task_id} depends_on_task_id={depends_on_id}"
        );
    }

    if dependency_path_exists(&mut tx, &task.workspace_id, &depends_on.id, &task.id).await? {
        bail!("error dependency-cycle task_id={task_id} depends_on_task_id={depends_on_id}");
    }

    let workspace_id = task.workspace_id.clone();
    let created_at = now();
    let changed = sqlx::query(
        "INSERT OR IGNORE INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&task.id)
    .bind(&depends_on.id)
    .bind(&created_at)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;

    if changed {
        let workspace = workspace_for_id(&mut tx, &workspace_id).await?;
        insert_change(
            &mut tx,
            "task",
            &task.id,
            Some("dependencies"),
            "dependency_add",
            json!({
                "workspace_id": &workspace.id,
                "workspace_key": &workspace.key,
                "depends_on_task_id": &depends_on.id,
            }),
            None,
        )
        .await?;
    }

    tx.commit().await?;
    Ok(DependencyOutcome {
        task,
        depends_on,
        changed,
    })
}

pub(crate) async fn remove_task_dependency(
    conn: &mut SqliteConnection,
    task_id: &str,
    depends_on_id: &str,
) -> Result<DependencyOutcome> {
    if task_id == depends_on_id {
        bail!("error dependency-self task_id={task_id}");
    }

    let mut tx = begin_immediate(conn).await?;
    let task = get_task(&mut tx, task_id).await?;
    let depends_on = get_task(&mut tx, depends_on_id).await?;

    if task.workspace_id != depends_on.workspace_id {
        bail!(
            "error dependency-cross-workspace task_id={task_id} depends_on_task_id={depends_on_id}"
        );
    }

    let changed = sqlx::query(
        "DELETE FROM task_dependencies
         WHERE workspace_id = ? AND task_id = ? AND depends_on_task_id = ?",
    )
    .bind(&task.workspace_id)
    .bind(&task.id)
    .bind(&depends_on.id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;

    if changed {
        let workspace = workspace_for_id(&mut tx, &task.workspace_id).await?;
        insert_change(
            &mut tx,
            "task",
            &task.id,
            Some("dependencies"),
            "dependency_remove",
            json!({
                "workspace_id": &workspace.id,
                "workspace_key": &workspace.key,
                "depends_on_task_id": &depends_on.id,
            }),
            None,
        )
        .await?;
    }

    tx.commit().await?;
    Ok(DependencyOutcome {
        task,
        depends_on,
        changed,
    })
}

pub(crate) async fn dependency_path_exists(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    from_task_id: &str,
    to_task_id: &str,
) -> Result<bool> {
    let mut visited = HashSet::new();
    let mut stack = vec![from_task_id.to_string()];
    while let Some(current) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        if current == to_task_id {
            return Ok(true);
        }
        let next = sqlx::query_scalar::<_, String>(
            "SELECT depends_on_task_id
             FROM task_dependencies
             WHERE workspace_id = ? AND task_id = ?",
        )
        .bind(workspace_id)
        .bind(&current)
        .fetch_all(&mut *conn)
        .await?;
        stack.extend(next);
    }
    Ok(false)
}
