use std::collections::HashMap;

use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::choices::{TaskPriority, TaskStatus};
use crate::ids::{TaskId, WorkspaceId};
use crate::refs::DisplayRefContext;

const RELATED_BIND_CHUNK_SIZE: usize = 449;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRelatedLink {
    pub task_id: TaskId,
    pub display_ref: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub deleted: bool,
    pub linked_at: String,
}

pub(crate) async fn related_links_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, Vec<TaskRelatedLink>>> {
    let mut links_by_task = HashMap::new();
    for chunk in task_ids.chunks(RELATED_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT r.task_a_id AS source_task_id,
                    t.id, t.title, t.status, t.priority, t.deleted,
                    p.prefix AS project_prefix, c.created_at AS linked_at
             FROM task_related_links r
             JOIN tasks t ON t.workspace_id = r.workspace_id AND t.id = r.task_b_id
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             JOIN changes c ON c.change_id = r.last_change_id
             WHERE r.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND r.task_a_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(
            ") AND r.linked = 1 UNION ALL SELECT r.task_b_id AS source_task_id,
                    t.id, t.title, t.status, t.priority, t.deleted,
                    p.prefix AS project_prefix, c.created_at AS linked_at
             FROM task_related_links r
             JOIN tasks t ON t.workspace_id = r.workspace_id AND t.id = r.task_a_id
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             JOIN changes c ON c.change_id = r.last_change_id
             WHERE r.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND r.task_b_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") AND r.linked = 1");

        for row in query.build().fetch_all(&mut *conn).await? {
            let source_task_id: TaskId = row.get("source_task_id");
            let related_id: TaskId = row.get("id");
            let project_prefix: String = row.get("project_prefix");
            links_by_task
                .entry(source_task_id)
                .or_insert_with(Vec::new)
                .push(TaskRelatedLink {
                    display_ref: display_refs.display_ref_for_id(
                        workspace_id,
                        &project_prefix,
                        &related_id,
                    ),
                    task_id: related_id,
                    title: row.get("title"),
                    status: TaskStatus::parse(&row.get::<String, _>("status"))?,
                    priority: TaskPriority::parse(&row.get::<String, _>("priority"))?,
                    deleted: row.get::<i64, _>("deleted") != 0,
                    linked_at: row.get("linked_at"),
                });
        }
    }

    for links in links_by_task.values_mut() {
        links.sort_by(|left, right| {
            left.deleted
                .cmp(&right.deleted)
                .then_with(|| {
                    super::status_order(left.status).cmp(&super::status_order(right.status))
                })
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.linked_at.cmp(&right.linked_at))
                .then_with(|| left.task_id.cmp(&right.task_id))
        });
    }
    Ok(links_by_task)
}

pub(crate) async fn task_related_links(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    display_refs: Option<&DisplayRefContext>,
) -> Result<Vec<TaskRelatedLink>> {
    let owned_refs;
    let display_refs = match display_refs {
        Some(refs) => refs,
        None => {
            owned_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
            &owned_refs
        }
    };
    Ok(related_links_for_tasks(
        conn,
        workspace_id,
        std::slice::from_ref(task_id),
        display_refs,
    )
    .await?
    .remove(task_id)
    .unwrap_or_default())
}
