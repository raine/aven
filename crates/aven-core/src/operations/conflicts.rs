use anyhow::{Result, bail};
use sqlx::{Row, SqliteConnection};
use tracing::info;

use crate::change_log::op_type;
use crate::db::{Database, begin_immediate, insert_change, set_field_version};
use crate::ids::TaskId;
use crate::mutation::{apply_field_value_in_workspace, apply_project_id_in_workspace};
use crate::projects::{resolve_existing_project_in_workspace, resolve_project_for_stored_value};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::undo::{UndoCommand, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

impl Database {
    pub async fn list_conflicts(
        &self,
        workspace: &Workspace,
        project_key: Option<&str>,
        field: Option<&str>,
    ) -> Result<Vec<ConflictListItem>> {
        let mut conn = self.acquire().await?;
        list_conflicts(&mut conn, workspace, project_key, field).await
    }

    pub async fn task_conflicts(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: Option<&str>,
    ) -> Result<Vec<ConflictDetail>> {
        let mut conn = self.acquire().await?;
        task_conflicts(&mut conn, workspace, task_id, field).await
    }

    pub async fn conflict_variant_value(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: &str,
        token: &str,
    ) -> Result<String> {
        let mut conn = self.acquire().await?;
        conflict_variant_value(&mut conn, workspace, task_id, field, token).await
    }

    pub async fn resolve_conflict_with_tui_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: &str,
        value: &str,
        summary: &str,
    ) -> Result<ConflictResolutionOutcome> {
        let mut conn = self.acquire().await?;
        resolve_conflict_value(
            &mut conn,
            workspace,
            task_id,
            field,
            ResolutionValue::Explicit(value),
            Some(summary),
        )
        .await
    }

    pub async fn resolve_conflict(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: &str,
        value: &str,
    ) -> Result<ConflictOutcome> {
        let mut conn = self.acquire().await?;
        resolve_conflict(&mut conn, workspace, task_id, field, value).await
    }
}

pub struct ConflictListItem {
    pub task_id: TaskId,
    pub title: String,
    pub project_key: String,
    pub project_prefix: String,
    pub field: String,
    pub variant_a: String,
    pub variant_b: String,
}

pub struct ConflictDetail {
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

pub struct ConflictOutcome {
    pub task: Task,
    pub field: String,
}

pub struct ConflictResolutionOutcome {
    pub outcome: ConflictOutcome,
    pub before: String,
    pub after: String,
    pub conflict_id: i64,
}
pub async fn list_conflicts(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project_key: Option<&str>,
    field: Option<&str>,
) -> Result<Vec<ConflictListItem>> {
    let workspace_id = &workspace.id;
    let project_id = if let Some(project) = project_key {
        Some(
            resolve_existing_project_in_workspace(conn, workspace_id, project)
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
    .bind(workspace_id)
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

pub async fn task_conflicts(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: Option<&str>,
) -> Result<Vec<ConflictDetail>> {
    let workspace_id = &workspace.id;
    let rows = sqlx::query(
        r#"SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id"#,
    )
    .bind(workspace_id)
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

pub async fn conflict_variant_value(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    token: &str,
) -> Result<String> {
    for detail in task_conflicts(conn, workspace, task_id, Some(field)).await? {
        if token == detail.variant_a {
            return Ok(detail.local_value);
        }
        if token == detail.variant_b {
            return Ok(detail.remote_value);
        }
    }
    bail!("error unknown-variant token={token}")
}

pub(crate) enum ConflictValueChoice {
    Local,
    Remote,
}

#[derive(Debug)]
pub(crate) struct ConflictNotFoundError {
    task_id: TaskId,
    field: &'static str,
}

impl std::fmt::Display for ConflictNotFoundError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "error conflict-not-found task_id={} field={}",
            self.task_id, self.field
        )
    }
}

impl std::error::Error for ConflictNotFoundError {}

pub async fn resolve_conflict(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    value: &str,
) -> Result<ConflictOutcome> {
    Ok(resolve_conflict_value(
        conn,
        workspace,
        task_id,
        field,
        ResolutionValue::Explicit(value),
        None,
    )
    .await?
    .outcome)
}

pub(crate) async fn resolve_conflict_choice(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    choice: ConflictValueChoice,
) -> Result<ConflictOutcome> {
    Ok(resolve_conflict_value(
        conn,
        workspace,
        task_id,
        field,
        ResolutionValue::Choice(choice),
        None,
    )
    .await?
    .outcome)
}

enum ResolutionValue<'a> {
    Explicit(&'a str),
    Choice(ConflictValueChoice),
}

async fn resolve_conflict_value(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    resolution: ResolutionValue<'_>,
    tui_summary: Option<&str>,
) -> Result<ConflictResolutionOutcome> {
    let task_field = TaskField::parse_or_unknown(field)?;
    let field = task_field.as_str();
    let mut tx = begin_immediate(conn).await?;
    let before = crate::undo::task_field_value(&mut tx, &workspace.id, task_id, field).await?;
    let conflict_id = crate::undo::conflict_row_id(&mut tx, &workspace.id, task_id, field).await?;
    let value = match resolution {
        ResolutionValue::Explicit(value) => value.to_string(),
        ResolutionValue::Choice(choice) => {
            let values = sqlx::query_as::<_, (String, String)>(
                "SELECT local_value, remote_value FROM conflicts
                 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
            )
            .bind(&workspace.id)
            .bind(task_id)
            .bind(field)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                anyhow::Error::new(ConflictNotFoundError {
                    task_id: task_id.clone(),
                    field,
                })
            })?;
            match choice {
                ConflictValueChoice::Local => values.0,
                ConflictValueChoice::Remote => values.1,
            }
        }
    };
    if task_field == TaskField::IsEpic
        && value == "0"
        && crate::operations::task_has_epic_children(&mut tx, &workspace.id, task_id).await?
    {
        bail!("error epic-has-children task_id={task_id}");
    }
    let result = sqlx::query(
        "UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(field)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(anyhow::Error::new(ConflictNotFoundError {
            task_id: task_id.clone(),
            field,
        }));
    }
    let payload = if task_field.is_project() {
        let project = resolve_project_for_stored_value(&mut tx, &workspace.id, &value).await?;
        apply_project_id_in_workspace(&mut tx, &workspace.id, task_id, &project.id).await?;
        TaskField::project_payload(&workspace.id, &workspace.key, &project)
    } else {
        apply_field_value_in_workspace(&mut tx, &workspace.id, task_id, field, &value).await?;
        task_field.scalar_payload(&workspace.id, &workspace.key, &value)?
    };
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some(field),
        op_type::RESOLVE_FIELD,
        payload,
        None,
    )
    .await?;
    set_field_version(&mut tx, task_id, field, &change_id).await?;
    let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
    let after = crate::undo::task_field_value(&mut tx, &workspace.id, task_id, field).await?;
    if let Some(summary) = tui_summary {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            summary,
            UndoPayload {
                commands: vec![UndoCommand::RestoreConflictResolution {
                    task_id: task_id.clone(),
                    field: field.to_string(),
                    before: before.clone(),
                    after: after.clone(),
                    conflict_id,
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!(task_id = %task_id, field = %field, "conflict resolved");
    Ok(ConflictResolutionOutcome {
        outcome: ConflictOutcome {
            task,
            field: field.to_string(),
        },
        before,
        after,
        conflict_id,
    })
}
