use anyhow::{Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};
use tracing::info;

use crate::db::{begin_immediate, insert_change, set_field_version};
use crate::mutation::{apply_field_value_in_workspace, apply_project_id_in_workspace};
use crate::projects::{resolve_existing_project_in_workspace, resolve_project_for_stored_value};
use crate::refs::get_task;
use crate::types::Task;

pub(crate) struct ConflictListItem {
    pub(crate) task_id: String,
    pub(crate) title: String,
    #[allow(dead_code)]
    pub(crate) project_key: String,
    pub(crate) project_prefix: String,
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) variant_b: String,
}

pub(crate) struct ConflictDetail {
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

pub(crate) struct ConflictOutcome {
    pub(crate) task: Task,
    pub(crate) field: String,
}
pub(crate) async fn list_conflicts(
    conn: &mut SqliteConnection,
    project_key: Option<&str>,
    field: Option<&str>,
) -> Result<Vec<ConflictListItem>> {
    let workspace_id = crate::workspaces::active_workspace_id();
    let project_id = if let Some(project) = project_key {
        Some(
            resolve_existing_project_in_workspace(conn, &workspace_id, project)
                .await?
                .id,
        )
    } else {
        None
    };
    let rows = sqlx::query(
        r#"SELECT c.task_id, c.field, c.variant_a, c.variant_b,
                 t.title, p.prefix, p.key AS project_key
                 FROM conflicts c
                 JOIN tasks t ON t.workspace_id = c.workspace_id AND t.id = c.task_id
                 JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
                 WHERE c.workspace_id = ? AND c.resolved = 0
                 AND (? IS NULL OR t.project_id = ?)
                 AND (? IS NULL OR c.field = ?)
                 ORDER BY c.created_at"#,
    )
    .bind(&workspace_id)
    .bind(&project_id)
    .bind(&project_id)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictListItem {
            task_id: row.get("task_id"),
            title: row.get("title"),
            project_key: row.get("project_key"),
            project_prefix: row.get("prefix"),
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            variant_b: row.get("variant_b"),
        })
        .collect())
}

pub(crate) async fn task_conflicts(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: Option<&str>,
) -> Result<Vec<ConflictDetail>> {
    let workspace_id = crate::workspaces::active_workspace_id();
    let rows = sqlx::query(
        r#"SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id"#,
    )
    .bind(&workspace_id)
    .bind(task_id)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictDetail {
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            local_value: row.get("local_value"),
            variant_b: row.get("variant_b"),
            remote_value: row.get("remote_value"),
        })
        .collect())
}

pub(crate) async fn conflict_variant_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    token: &str,
) -> Result<String> {
    for detail in task_conflicts(conn, task_id, Some(field)).await? {
        if token == detail.variant_a {
            return Ok(detail.local_value);
        }
        if token == detail.variant_b {
            return Ok(detail.remote_value);
        }
    }
    bail!("error unknown-variant token={token}")
}

pub(crate) async fn resolve_conflict(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<ConflictOutcome> {
    let workspace = crate::workspaces::active_workspace();
    let mut tx = begin_immediate(conn).await?;
    let result = sqlx::query(
        "UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(field)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        bail!("error conflict-not-found task_id={task_id} field={field}");
    }
    let payload = if field == crate::task_fields::TaskField::Project.as_str() {
        let project = resolve_project_for_stored_value(&mut tx, &workspace.id, value).await?;
        apply_project_id_in_workspace(&mut tx, &workspace.id, task_id, &project.id).await?;
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "value": &project.id,
            "project_id": &project.id,
            "project_key": &project.key,
            "project_name": &project.name,
            "project_prefix": &project.prefix,
        })
    } else {
        apply_field_value_in_workspace(&mut tx, &workspace.id, task_id, field, value).await?;
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "value": value,
        })
    };
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some(field),
        "resolve_field",
        payload,
        None,
    )
    .await?;
    set_field_version(&mut tx, task_id, field, &change_id).await?;
    tx.commit().await?;
    info!(task_id = %task_id, field = %field, "conflict resolved");
    Ok(ConflictOutcome {
        task: get_task(conn, task_id).await?,
        field: field.to_string(),
    })
}
