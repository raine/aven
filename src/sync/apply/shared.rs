use crate::ids::WorkspaceId;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

use crate::sync::wire::ChangeWire;
use crate::workspaces::{DEFAULT_WORKSPACE_ID, ensure_default_workspace};

pub(super) fn str_payload(payload: &Value, key: &str) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("payload missing {key}"))
}

pub(super) fn optional_str_payload(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(super) async fn workspace_id_payload(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<WorkspaceId> {
    if let Some(workspace_id) = change.payload.get("workspace_id").and_then(Value::as_str) {
        let workspace_id = workspace_id
            .parse()
            .with_context(|| "payload contains invalid workspace_id")?;
        ensure_workspace_exists(conn, &workspace_id).await?;
        return Ok(workspace_id);
    }
    let row = sqlx::query("SELECT workspace_id FROM tasks WHERE id = ?")
        .bind(&change.entity_id)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(row) = row {
        return Ok(row.get("workspace_id"));
    }
    let workspace = ensure_default_workspace(conn).await?;
    Ok(workspace.id)
}

pub(super) async fn task_field_workspace_id_payload(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<WorkspaceId> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    if change
        .payload
        .get("workspace_id")
        .and_then(Value::as_str)
        .is_none()
    {
        return Ok(workspace_id);
    }
    let task_workspace_id =
        sqlx::query_scalar::<_, WorkspaceId>("SELECT workspace_id FROM tasks WHERE id = ?")
            .bind(&change.entity_id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(task_workspace_id) = task_workspace_id
        && task_workspace_id != workspace_id
    {
        bail!(
            "error invalid-task-workspace task_id={} workspace_id={} task_workspace_id={}",
            change.entity_id,
            workspace_id,
            task_workspace_id
        );
    }
    Ok(workspace_id)
}

async fn ensure_workspace_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<()> {
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?
        > 0
    {
        return Ok(());
    }
    if workspace_id.as_str() == DEFAULT_WORKSPACE_ID {
        ensure_default_workspace(conn).await?;
        return Ok(());
    }
    bail!("error unknown-workspace-id id={workspace_id}")
}

pub(super) fn safe_entity_id(change: &ChangeWire) -> &str {
    if change.entity_type == "task" {
        &change.entity_id
    } else {
        ""
    }
}
