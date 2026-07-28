use crate::ids::WorkspaceId;
use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::choices::TaskStatus;
use crate::refs::DisplayRefContext;
use crate::types::Task;

use super::fragments;

#[derive(Debug)]
pub struct TaskDependencyItem {
    pub task: Task,
    pub display_ref: String,
    pub created_at: String,
    pub unresolved: bool,
}

#[derive(Debug)]
pub struct TaskDependencySummary {
    pub depends_on: Vec<TaskDependencyItem>,
    pub blocks: Vec<TaskDependencyItem>,
}

pub async fn task_dependency_summary(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
) -> Result<TaskDependencySummary> {
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    task_dependency_summary_with_display_refs(conn, workspace_id, task_id, &display_refs).await
}

pub async fn task_dependency_summary_with_display_refs(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    display_refs: &DisplayRefContext,
) -> Result<TaskDependencySummary> {
    let depends_on = query_dependency_items(&mut *conn, workspace_id, task_id, false, display_refs)
        .await?
        .into_iter()
        .collect::<Vec<_>>();
    let blocks = query_dependency_items(&mut *conn, workspace_id, task_id, true, display_refs)
        .await?
        .into_iter()
        .collect::<Vec<_>>();
    Ok(TaskDependencySummary { depends_on, blocks })
}

async fn query_dependency_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    blocks_only: bool,
    display_refs: &DisplayRefContext,
) -> Result<Vec<TaskDependencyItem>> {
    let rows = if blocks_only {
        sqlx::query(
            "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.source, t.created_at, t.updated_at,
         t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic, d.created_at AS dependency_created_at
         FROM task_dependencies d
         JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.task_id
         JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE d.workspace_id = ? AND d.depends_on_task_id = ?",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_all(&mut *conn)
        .await?
    } else {
        sqlx::query(
            "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.source, t.created_at, t.updated_at,
         t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic, d.created_at AS dependency_created_at
         FROM task_dependencies d
         JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.depends_on_task_id
         JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE d.workspace_id = ? AND d.task_id = ?",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_all(&mut *conn)
        .await?
    };

    let subject_is_open = if blocks_only {
        subject_task_is_open(conn, workspace_id, task_id).await?
    } else {
        true
    };
    let mut rows_tasks = rows
        .iter()
        .map(crate::db::task_from_row)
        .collect::<Result<Vec<_>>>()?;
    let mut items = rows
        .into_iter()
        .zip(rows_tasks.drain(..))
        .map(|(row, task)| {
            let created_at: String = row.get("dependency_created_at");
            let task_is_open = !task.deleted && task.status.is_open();
            let unresolved = task_is_open && (!blocks_only || subject_is_open);
            let display_ref = display_refs.display_ref(&task);
            TaskDependencyItem {
                task,
                display_ref,
                created_at,
                unresolved,
            }
        })
        .collect::<Vec<_>>();

    items.sort_by(|a, b| {
        b.unresolved.cmp(&a.unresolved).then_with(|| {
            status_order(a.task.status)
                .cmp(&status_order(b.task.status))
                .then_with(|| a.task.title.cmp(&b.task.title))
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.task.id.cmp(&b.task.id))
        })
    });
    Ok(items)
}

async fn subject_task_is_open(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
) -> Result<bool> {
    let sql = format!(
        "SELECT count(*) FROM tasks
         WHERE workspace_id = ? AND id = ? AND {}",
        fragments::open_task_clause("tasks"),
    );
    let open: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(open > 0)
}

fn status_order(status: TaskStatus) -> u8 {
    match status {
        TaskStatus::Active => 0,
        TaskStatus::Todo => 1,
        TaskStatus::Inbox => 2,
        TaskStatus::Backlog => 3,
        TaskStatus::Done => 4,
        TaskStatus::Canceled => 5,
    }
}
