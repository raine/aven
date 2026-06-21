use anyhow::{Context, Result, bail};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};
use tracing::{debug, info};

use super::wire::ChangeWire;
use crate::db::{conflict_exists, field_version, set_field_version};
use crate::ids::now;
use crate::mutation::apply_field_value_in_workspace;
use crate::projects::{find_project_in_workspace, prefix_base};
use crate::refs::get_task;
use crate::task_fields::TaskField;
use crate::workspaces::{DEFAULT_WORKSPACE_ID, ensure_default_workspace};

pub(super) async fn apply_remote_change(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    debug!(
        change_id = %change.change_id,
        op_type = %change.op_type,
        entity_type = %change.entity_type,
        entity_id = safe_entity_id(change),
        field = change.field.as_deref().unwrap_or(""),
        "applying remote change"
    );
    match change.op_type.as_str() {
        "create_workspace" => {
            let key = str_payload(&change.payload, "key")?;
            let name = str_payload(&change.payload, "name")?;
            let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
            sqlx::query(
                "INSERT OR IGNORE INTO workspaces(id, key, name, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&change.entity_id)
            .bind(key)
            .bind(name)
            .bind(&created_at)
            .bind(&created_at)
            .execute(&mut *conn)
            .await?;
        }
        "set_workspace_field" => {
            let field = change
                .field
                .as_deref()
                .context("workspace field change missing field")?;
            let value = str_payload(&change.payload, "value")?;
            let ts = now();
            match field {
                "name" => {
                    sqlx::query("UPDATE workspaces SET name = ?, updated_at = ? WHERE id = ?")
                        .bind(value)
                        .bind(&ts)
                        .bind(&change.entity_id)
                        .execute(&mut *conn)
                        .await?;
                }
                "key" => {
                    sqlx::query("UPDATE workspaces SET key = ?, updated_at = ? WHERE id = ?")
                        .bind(value)
                        .bind(&ts)
                        .bind(&change.entity_id)
                        .execute(&mut *conn)
                        .await?;
                }
                _ => bail!("error invalid-sync-change field={field}"),
            }
        }
        "create_project" => {
            let workspace_id = workspace_id_payload(conn, change).await?;
            let key = str_payload(&change.payload, "key")?;
            let name = str_payload(&change.payload, "name")?;
            let prefix = str_payload(&change.payload, "prefix")?;
            let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
            sqlx::query(
                "INSERT OR IGNORE INTO projects(workspace_id, key, name, prefix, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&workspace_id)
            .bind(key)
            .bind(name)
            .bind(prefix)
            .bind(&created_at)
            .bind(&created_at)
            .execute(&mut *conn)
            .await?;
        }
        "create_label" => {
            let workspace_id = workspace_id_payload(conn, change).await?;
            let name = str_payload(&change.payload, "name")?;
            let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
            sqlx::query(
                "INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)",
            )
            .bind(&workspace_id)
            .bind(name)
            .bind(created_at)
            .execute(&mut *conn)
            .await?;
        }
        "create_task" => apply_remote_create_task(conn, change).await?,
        "set_field" => apply_remote_set_field(conn, change, false).await?,
        "resolve_field" => apply_remote_set_field(conn, change, true).await?,
        "label_add" => {
            let workspace_id = workspace_id_payload(conn, change).await?;
            let label = str_payload(&change.payload, "label")?;
            sqlx::query(
                "INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)",
            )
            .bind(&workspace_id)
            .bind(&label)
            .bind(&change.created_at)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
            )
            .bind(&workspace_id)
            .bind(&change.entity_id)
            .bind(&label)
            .execute(&mut *conn)
            .await?;
        }
        "label_remove" => {
            let workspace_id = workspace_id_payload(conn, change).await?;
            let label = str_payload(&change.payload, "label")?;
            sqlx::query(
                "DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?",
            )
            .bind(&workspace_id)
            .bind(&change.entity_id)
            .bind(&label)
            .execute(&mut *conn)
            .await?;
        }
        "note_add" => {
            let workspace_id = workspace_id_payload(conn, change).await?;
            let note_id = str_payload(&change.payload, "note_id")?;
            let body = str_payload(&change.payload, "body")?;
            let created_at = str_payload(&change.payload, "created_at")
                .unwrap_or_else(|_| change.created_at.clone());
            sqlx::query(
                "INSERT OR IGNORE INTO notes(workspace_id, id, task_id, body, created_at, change_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&workspace_id)
            .bind(&note_id)
            .bind(&change.entity_id)
            .bind(&body)
            .bind(&created_at)
            .bind(&change.change_id)
            .execute(&mut *conn)
            .await?;
            sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
                .bind(&created_at)
                .bind(&workspace_id)
                .bind(&change.entity_id)
                .execute(&mut *conn)
                .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn apply_remote_create_task(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks WHERE workspace_id = ? AND id = ?")
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .fetch_one(&mut *conn)
        .await?
        > 0
    {
        return Ok(());
    }
    let project_key = str_payload(&change.payload, "project_key")?;
    if find_project_in_workspace(conn, &workspace_id, &project_key)
        .await?
        .is_none()
    {
        let name =
            str_payload(&change.payload, "project_name").unwrap_or_else(|_| project_key.clone());
        let prefix = str_payload(&change.payload, "project_prefix")
            .unwrap_or_else(|_| prefix_base(&project_key));
        sqlx::query(
            "INSERT OR IGNORE INTO projects(workspace_id, key, name, prefix, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&workspace_id)
        .bind(&project_key)
        .bind(&name)
        .bind(&prefix)
        .bind(&change.created_at)
        .bind(&change.created_at)
        .execute(&mut *conn)
        .await?;
    }
    let title = str_payload(&change.payload, "title")?;
    let description = str_payload(&change.payload, "description").unwrap_or_default();
    let status = str_payload(&change.payload, "status").unwrap_or_else(|_| "inbox".to_string());
    let priority = str_payload(&change.payload, "priority").unwrap_or_else(|_| "none".to_string());
    let created_at =
        str_payload(&change.payload, "created_at").unwrap_or_else(|_| change.created_at.clone());
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_key, status, priority, created_at, updated_at, queue_activity_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(&title)
    .bind(&description)
    .bind(&project_key)
    .bind(&status)
    .bind(&priority)
    .bind(&created_at)
    .bind(&change.created_at)
    .bind(&change.created_at)
    .execute(&mut *conn)
    .await?;
    if let Some(labels) = change.payload.get("labels").and_then(Value::as_array) {
        for label in labels.iter().filter_map(Value::as_str) {
            sqlx::query(
                "INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)",
            )
            .bind(&workspace_id)
            .bind(label)
            .bind(&change.created_at)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
            )
            .bind(&workspace_id)
            .bind(&change.entity_id)
            .bind(label)
            .execute(&mut *conn)
            .await?;
        }
    }
    for field in TaskField::VERSIONED {
        set_field_version(conn, &change.entity_id, field.as_str(), &change.change_id).await?;
    }
    Ok(())
}

pub(crate) async fn apply_remote_set_field(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    force: bool,
) -> Result<()> {
    let field = change
        .field
        .as_deref()
        .context("field change missing field")?;
    let value = str_payload(&change.payload, "value")?;
    if !force {
        let current = field_version(conn, &change.entity_id, field).await?;
        if current != change.base_version {
            create_conflict(conn, change, field, &value, current.as_deref()).await?;
            return Ok(());
        }
    }
    let workspace_id = workspace_id_payload(conn, change).await?;
    apply_field_value_in_workspace(conn, &workspace_id, &change.entity_id, field, &value).await?;
    set_field_version(conn, &change.entity_id, field, &change.change_id).await?;
    if force {
        sqlx::query(
            "UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
        )
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .bind(field)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn create_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    field: &str,
    remote_value: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    if conflict_exists(conn, &workspace_id, &change.entity_id, field).await? {
        return Ok(());
    }
    let local_value = current_field_value(conn, &change.entity_id, field).await?;
    let variant_a = format!(
        "v{}",
        local_change_id
            .unwrap_or("local")
            .chars()
            .take(6)
            .collect::<String>()
    );
    let variant_b = format!("v{}", change.change_id.chars().take(6).collect::<String>());
    sqlx::query(
        "INSERT OR IGNORE INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(field)
    .bind(&change.base_version)
    .bind(&local_value)
    .bind(remote_value)
    .bind(local_change_id)
    .bind(&change.change_id)
    .bind(&variant_a)
    .bind(&variant_b)
    .bind(&change.created_at)
    .execute(&mut *conn)
    .await?;
    info!(
        task_id = safe_entity_id(change),
        field = %field,
        "remote change conflict created"
    );
    Ok(())
}

async fn workspace_id_payload(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<String> {
    if let Some(workspace_id) = change.payload.get("workspace_id").and_then(Value::as_str) {
        ensure_workspace_exists(conn, workspace_id).await?;
        return Ok(workspace_id.to_string());
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

async fn ensure_workspace_exists(conn: &mut SqliteConnection, workspace_id: &str) -> Result<()> {
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?
        > 0
    {
        return Ok(());
    }
    if workspace_id == DEFAULT_WORKSPACE_ID {
        ensure_default_workspace(conn).await?;
        return Ok(());
    }
    bail!("error unknown-workspace-id id={workspace_id}")
}

async fn current_field_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
) -> Result<String> {
    let task = get_task(conn, task_id).await?;
    let task_field = TaskField::parse(field)
        .ok_or_else(|| anyhow::anyhow!("error unknown-field field={field}"))?;
    Ok(task_field.current_value(&task))
}

fn str_payload(payload: &Value, key: &str) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("payload missing {key}"))
}

fn safe_entity_id(change: &ChangeWire) -> &str {
    if change.entity_type == "task" {
        &change.entity_id
    } else {
        ""
    }
}
