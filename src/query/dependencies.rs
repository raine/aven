use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::refs::display_refs_for_tasks;
use crate::types::Task;

#[derive(Debug)]
pub(crate) struct TaskDependencyItem {
    pub(crate) task: Task,
    pub(crate) display_ref: String,
    pub(crate) created_at: String,
    pub(crate) unresolved: bool,
}

#[derive(Debug)]
pub(crate) struct TaskDependencySummary {
    pub(crate) depends_on: Vec<TaskDependencyItem>,
    pub(crate) blocks: Vec<TaskDependencyItem>,
}

pub(crate) async fn task_dependency_summary(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<TaskDependencySummary> {
    let depends_on = query_dependency_items(&mut *conn, workspace_id, task_id, false)
        .await?
        .into_iter()
        .collect::<Vec<_>>();
    let blocks = query_dependency_items(&mut *conn, workspace_id, task_id, true)
        .await?
        .into_iter()
        .collect::<Vec<_>>();
    Ok(TaskDependencySummary { depends_on, blocks })
}

async fn query_dependency_items(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    blocks_only: bool,
) -> Result<Vec<TaskDependencyItem>> {
    let rows = if blocks_only {
        sqlx::query(
            "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at,
         t.queue_activity_at, t.deleted, d.created_at AS dependency_created_at
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
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at,
         t.queue_activity_at, t.deleted, d.created_at AS dependency_created_at
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

    let mut rows_tasks = rows
        .iter()
        .map(crate::db::task_from_row)
        .collect::<Result<Vec<_>>>()?;
    let display_refs = display_refs_for_tasks(conn, &rows_tasks).await?;
    let mut items = rows
        .into_iter()
        .zip(rows_tasks.drain(..))
        .map(|(row, task)| {
            let created_at: String = row.get("dependency_created_at");
            let unresolved = !task.deleted && !matches!(task.status.as_str(), "done" | "canceled");
            let display_ref = display_refs
                .get(&task.id)
                .cloned()
                .unwrap_or_else(|| format!("{}-{}", task.project_prefix, task.id));
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
            status_order(&a.task.status)
                .cmp(&status_order(&b.task.status))
                .then_with(|| a.task.title.cmp(&b.task.title))
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.task.id.cmp(&b.task.id))
        })
    });
    Ok(items)
}

fn status_order(status: &str) -> u8 {
    match status {
        "active" => 0,
        "todo" => 1,
        "inbox" => 2,
        "backlog" => 3,
        "done" => 4,
        "canceled" => 5,
        _ => 6,
    }
}
