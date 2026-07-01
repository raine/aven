use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::sync::wire::ChangeWire;

use super::shared::{str_payload, workspace_id_payload};

pub(super) async fn add_epic_link(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let epic_task_id = str_payload(&change.payload, "epic_task_id")?;
    ensure_epic_tasks_can_link(conn, &workspace_id, &change.entity_id, &epic_task_id).await?;
    sqlx::query("UPDATE tasks SET is_epic = 1 WHERE workspace_id = ? AND id = ?")
        .bind(&workspace_id)
        .bind(&epic_task_id)
        .execute(&mut *conn)
        .await?;
    let existing_epic_id = sqlx::query_scalar::<_, String>(
        "SELECT epic_task_id FROM task_epic_links WHERE workspace_id = ? AND child_task_id = ?",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(existing_epic_id) = existing_epic_id {
        if existing_epic_id == epic_task_id {
            return Ok(());
        }
        if epic_task_id > existing_epic_id {
            return Ok(());
        }
        sqlx::query(
            "UPDATE task_epic_links SET epic_task_id = ?, created_at = ?
             WHERE workspace_id = ? AND child_task_id = ?",
        )
        .bind(&epic_task_id)
        .bind(
            change
                .payload
                .get("created_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&change.created_at),
        )
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .execute(&mut *conn)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT OR IGNORE INTO task_epic_links(workspace_id, child_task_id, epic_task_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(&epic_task_id)
    .bind(change.payload.get("created_at").and_then(serde_json::Value::as_str).unwrap_or(&change.created_at))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(super) async fn remove_epic_link(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let epic_task_id = str_payload(&change.payload, "epic_task_id")?;
    sqlx::query(
        "DELETE FROM task_epic_links
         WHERE workspace_id = ? AND child_task_id = ? AND epic_task_id = ?",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(&epic_task_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn ensure_epic_tasks_can_link(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    child_task_id: &str,
    epic_task_id: &str,
) -> Result<()> {
    if child_task_id == epic_task_id {
        bail!("error epic-self task_id={child_task_id}");
    }
    let rows = sqlx::query(
        "SELECT id, project_id, is_epic FROM tasks WHERE workspace_id = ? AND id IN (?, ?)",
    )
    .bind(workspace_id)
    .bind(child_task_id)
    .bind(epic_task_id)
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() != 2 {
        bail!("error epic-missing-task child_task_id={child_task_id} epic_task_id={epic_task_id}");
    }
    let mut child_project = None;
    let mut epic_project = None;
    for row in rows {
        use sqlx::Row;
        let id: String = row.get("id");
        let project_id: String = row.get("project_id");
        let is_epic = row.get::<i64, _>("is_epic") != 0;
        if id == child_task_id {
            if is_epic {
                bail!("error epic-child-is-epic child_task_id={child_task_id}");
            }
            child_project = Some(project_id);
        } else if id == epic_task_id {
            epic_project = Some(project_id);
        }
    }
    if child_project != epic_project {
        bail!("error epic-cross-project child_task_id={child_task_id} epic_task_id={epic_task_id}");
    }
    Ok(())
}
