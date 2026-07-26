use crate::ids::{ProjectId, WorkspaceId};
use anyhow::{Result, bail, ensure};
use sqlx::SqliteConnection;
use tracing::{debug, info};

use crate::change_log::op_type;
use crate::choices::TaskPriority;
use crate::db::{
    Database, begin_immediate, conflict_exists, field_version, insert_change, set_field_version,
    task_from_row,
};
use crate::ids::now;
use crate::projects::resolve_or_create_project_in_workspace;
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::{Project, Task};
use crate::workspaces::Workspace;

#[derive(Debug)]
pub(crate) struct OpenConflictError {
    task_id: crate::ids::TaskId,
    field: &'static str,
}

impl std::fmt::Display for OpenConflictError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            self.task_id, self.field
        )
    }
}

impl std::error::Error for OpenConflictError {}

impl Database {
    pub async fn set_task_status(
        &self,
        workspace: &Workspace,
        task: &Task,
        status: &str,
    ) -> Result<Task> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let task = set_status(&mut tx, workspace, task, status).await?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn set_task_priority(
        &self,
        workspace: &Workspace,
        task: &Task,
        priority: &str,
    ) -> Result<Task> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let task = set_priority(&mut tx, workspace, task, priority).await?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn cycle_task_priority(
        &self,
        workspace: &Workspace,
        task: &Task,
        reverse: bool,
    ) -> Result<Task> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let current = get_task_in_workspace(&mut tx, workspace, &task.id).await?;
        let task = cycle_priority(&mut tx, workspace, &current, reverse).await?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn set_task_deleted_state(
        &self,
        workspace: &Workspace,
        task: &Task,
        deleted: bool,
    ) -> Result<Task> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let task = set_deleted(&mut tx, workspace, task, deleted).await?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn set_task_field(
        &self,
        workspace: &Workspace,
        task_id: &crate::ids::TaskId,
        field: &str,
        value: &str,
    ) -> Result<bool> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let changed = set_task_field(&mut tx, workspace, task_id, field, value).await?;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn set_task_project(
        &self,
        workspace: &Workspace,
        task_id: &crate::ids::TaskId,
        project: &Project,
    ) -> Result<bool> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let changed = set_task_project(&mut tx, workspace, task_id, project).await?;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn set_task_fields(
        &self,
        workspace: &Workspace,
        updates: &[(crate::ids::TaskId, String, String)],
    ) -> Result<Vec<bool>> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let mut outcomes = Vec::with_capacity(updates.len());
        for (task_id, field, value) in updates {
            outcomes.push(set_task_field(&mut tx, workspace, task_id, field, value).await?);
        }
        tx.commit().await?;
        Ok(outcomes)
    }

    pub async fn cycle_task_priorities(
        &self,
        workspace: &Workspace,
        tasks: &[Task],
        reverse: bool,
    ) -> Result<Vec<Task>> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let mut outcomes = Vec::with_capacity(tasks.len());
        for task in tasks {
            outcomes.push(cycle_priority(&mut tx, workspace, task, reverse).await?);
        }
        tx.commit().await?;
        Ok(outcomes)
    }
}

pub(crate) async fn set_status(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task: &Task,
    status: &str,
) -> Result<Task> {
    set_task_field(conn, workspace, &task.id, "status", status).await?;
    get_task_in_workspace(conn, workspace, &task.id).await
}

pub(crate) async fn set_priority(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task: &Task,
    priority: &str,
) -> Result<Task> {
    set_task_field(conn, workspace, &task.id, "priority", priority).await?;
    get_task_in_workspace(conn, workspace, &task.id).await
}

pub(crate) async fn cycle_priority(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task: &Task,
    reverse: bool,
) -> Result<Task> {
    let index = TaskPriority::ALL
        .iter()
        .position(|priority| *priority == task.priority)
        .unwrap_or(0);
    let next = if reverse {
        (index + TaskPriority::ALL.len() - 1) % TaskPriority::ALL.len()
    } else {
        (index + 1) % TaskPriority::ALL.len()
    };
    set_priority(conn, workspace, task, TaskPriority::ALL[next].as_str()).await
}

pub(crate) async fn set_deleted(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task: &Task,
    deleted: bool,
) -> Result<Task> {
    set_task_field(
        conn,
        workspace,
        &task.id,
        "deleted",
        if deleted { "1" } else { "0" },
    )
    .await?;
    get_task_in_workspace(conn, workspace, &task.id).await
}

pub(crate) async fn set_task_field(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    value: &str,
) -> Result<bool> {
    let task_field = TaskField::parse_or_unknown(field)?;
    if task_field.is_project() {
        let project = resolve_or_create_project_in_workspace(conn, &workspace.id, value).await?;
        set_task_project(conn, workspace, task_id, &project).await
    } else {
        set_task_scalar_field(conn, workspace, task_id, task_field, value).await
    }
}

pub(crate) async fn set_task_project(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    project: &Project,
) -> Result<bool> {
    let field = TaskField::Project.as_str();
    let current = current_task(conn, &workspace.id, task_id).await?;
    if current.project_id == project.id {
        return Ok(false);
    }
    if conflict_exists(conn, &workspace.id, task_id, field).await? {
        return Err(anyhow::Error::new(OpenConflictError {
            task_id: task_id.clone(),
            field,
        }));
    }
    debug!(task_id = %task_id, field = %field, "task field mutation started");
    let base = field_version(conn, task_id, field).await?;
    apply_project_id_in_workspace(conn, &workspace.id, task_id, &project.id).await?;
    let payload = TaskField::project_payload(&workspace.id, &workspace.key, project);
    finish_task_field_change(conn, task_id, field, payload, base.as_deref()).await?;
    Ok(true)
}

async fn set_task_scalar_field(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    task_field: TaskField,
    value: &str,
) -> Result<bool> {
    task_field.validate_value(value)?;

    let field = task_field.as_str();
    let current = current_task(conn, &workspace.id, task_id).await?;
    if task_field.current_value(&current) == value {
        return Ok(false);
    }
    if conflict_exists(conn, &workspace.id, task_id, field).await? {
        return Err(anyhow::Error::new(OpenConflictError {
            task_id: task_id.clone(),
            field,
        }));
    }
    debug!(task_id = %task_id, field = %field, "task field mutation started");
    let base = field_version(conn, task_id, field).await?;
    apply_scalar_field_value_in_workspace(conn, &workspace.id, task_id, task_field, value).await?;
    let payload = task_field.scalar_payload(&workspace.id, &workspace.key, value)?;
    finish_task_field_change(conn, task_id, field, payload, base.as_deref()).await?;
    Ok(true)
}

async fn finish_task_field_change(
    conn: &mut SqliteConnection,
    task_id: &crate::ids::TaskId,
    field: &str,
    payload: serde_json::Value,
    base: Option<&str>,
) -> Result<()> {
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(field),
        op_type::SET_FIELD,
        payload,
        base,
    )
    .await?;
    set_field_version(conn, task_id, field, &change_id).await?;
    info!(
        task_id = %task_id,
        field = %field,
        change_id = %change_id,
        "task field mutated"
    );
    Ok(())
}

async fn current_task(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
) -> Result<Task> {
    let row = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
                p.key AS project_key, p.prefix AS project_prefix, t.status,
                t.priority, t.created_at, t.updated_at, t.queue_activity_at,
                t.available_at, t.due_on, t.deleted, t.is_epic
         FROM tasks t
         JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND t.id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| {
        anyhow::anyhow!(
            "error task-not-found task_id={} workspace_id={}",
            task_id,
            workspace_id
        )
    })?;
    task_from_row(&row)
}

#[allow(dead_code)]
pub async fn apply_field_value(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    field: &str,
    value: &str,
) -> Result<()> {
    apply_field_value_in_workspace(conn, workspace_id, task_id, field, value).await
}

pub async fn apply_project_id_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    project_id: &ProjectId,
) -> Result<()> {
    let project_exists = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE workspace_id = ? AND id = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?
        > 0;
    if !project_exists {
        bail!("error unknown-project-id id={project_id}");
    }
    let ts = now();
    let rows_affected = sqlx::query(
        "UPDATE tasks SET project_id = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
    )
    .bind(project_id)
    .bind(&ts)
    .bind(workspace_id)
    .bind(task_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    ensure!(
        rows_affected == 1,
        "error task-not-found task_id={} workspace_id={}",
        task_id,
        workspace_id
    );
    Ok(())
}

pub async fn apply_field_value_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    field: &str,
    value: &str,
) -> Result<()> {
    let task_field = TaskField::parse_or_unknown(field)?;
    apply_scalar_field_value_in_workspace(conn, workspace_id, task_id, task_field, value).await
}

async fn apply_scalar_field_value_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    task_field: TaskField,
    value: &str,
) -> Result<()> {
    task_field.validate_value(value)?;

    let ts = now();
    let activity_at = if task_field.updates_queue_activity() {
        ts.as_str()
    } else {
        ""
    };
    let deleted_value = i64::from(value == "1");
    let epic_value = i64::from(value == "1");
    let rows_affected = match task_field {
        TaskField::Title => sqlx::query(
            "UPDATE tasks SET title = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Description => sqlx::query(
            "UPDATE tasks SET description = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Project => bail!("error project-update-requires-project-id"),
        TaskField::Status => sqlx::query(
            "UPDATE tasks SET status = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Priority => sqlx::query(
            "UPDATE tasks SET priority = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::AvailableAt => sqlx::query(
            "UPDATE tasks SET available_at = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::DueOn => sqlx::query(
            "UPDATE tasks SET due_on = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Deleted => sqlx::query(
            "UPDATE tasks SET deleted = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(deleted_value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::IsEpic => sqlx::query(
            "UPDATE tasks SET is_epic = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
        )
        .bind(epic_value)
        .bind(&ts)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
    };
    ensure!(
        rows_affected == 1,
        "error task-not-found task_id={} workspace_id={}",
        task_id,
        workspace_id
    );
    Ok(())
}
