use crate::ids::{TaskId, WorkspaceId};
use crate::query::fragments;
use crate::query::{EpicRollup, TaskDependencyLink};
use crate::refs::DisplayRefContext;
use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};
use std::collections::HashMap;

pub(super) async fn epic_children_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, Vec<TaskDependencyLink>>> {
    let mut links = HashMap::new();
    if task_ids.is_empty() {
        return Ok(links);
    }
    for chunk in task_ids.chunks(super::SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT l.epic_task_id AS source_task_id,
                    t.id, t.title, t.status, t.priority, p.key AS project_key, p.prefix AS project_prefix,
                    CASE WHEN ",
        );
        query.push(fragments::open_task_clause("t"));
        query.push(
            " THEN 1 ELSE 0 END AS unresolved
             FROM task_epic_links l
             JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.child_task_id
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             WHERE l.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND l.epic_task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(
            ") AND t.deleted = 0 ORDER BY unresolved DESC, t.status, t.title, l.created_at, t.id",
        );

        for row in query.build().fetch_all(&mut *conn).await? {
            let source_task_id: TaskId = row.get("source_task_id");
            links.entry(source_task_id).or_insert_with(Vec::new).push(
                super::dependency_link_from_row(&row, workspace_id, display_refs),
            );
        }
    }
    Ok(links)
}

pub(super) async fn epic_rollups_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, EpicRollup>> {
    let mut rollups = HashMap::new();
    if task_ids.is_empty() {
        return Ok(rollups);
    }
    let now = crate::ids::now();
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    for chunk in task_ids.chunks(super::SQLITE_BIND_CHUNK_SIZE) {
        // CROSS JOIN pins the dependency rows as the outer loop; otherwise the
        // planner can scan every task in the workspace for each epic child.
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT l.epic_task_id,
                    COUNT(*) AS total,
                    SUM(CASE WHEN t.status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END) AS open,
                    SUM(CASE WHEN t.status = 'done' THEN 1 ELSE 0 END) AS done,
                    SUM(CASE WHEN t.status = 'canceled' THEN 1 ELSE 0 END) AS canceled,
                    SUM(CASE WHEN t.status NOT IN ('done', 'canceled') AND EXISTS (
                        SELECT 1 FROM task_dependencies d
                        CROSS JOIN tasks blocker
                          ON blocker.workspace_id = d.workspace_id
                         AND blocker.id = d.depends_on_task_id
                        WHERE d.workspace_id = t.workspace_id
                          AND d.task_id = t.id
                          AND blocker.deleted = 0
                          AND blocker.status NOT IN ('done', 'canceled')
                    ) THEN 1 ELSE 0 END) AS blocked,
                    SUM(CASE WHEN t.status NOT IN ('done', 'canceled')
                                  AND t.due_on != '' AND t.due_on < ",
        );
        query.push_bind(&today);
        query.push(
            " THEN 1 ELSE 0 END) AS overdue,
                    SUM(CASE WHEN t.status NOT IN ('done', 'canceled')
                                  AND (t.available_at = '' OR t.available_at <= ",
        );
        query.push_bind(&now);
        query.push(
            ") AND NOT EXISTS (
                        SELECT 1 FROM task_dependencies d
                        CROSS JOIN tasks blocker
                          ON blocker.workspace_id = d.workspace_id
                         AND blocker.id = d.depends_on_task_id
                        WHERE d.workspace_id = t.workspace_id
                          AND d.task_id = t.id
                          AND blocker.deleted = 0
                          AND blocker.status NOT IN ('done', 'canceled')
                    ) THEN 1 ELSE 0 END) AS ready,
                    MAX(t.updated_at) AS latest_activity_at
             FROM task_epic_links l
             JOIN tasks t
               ON t.workspace_id = l.workspace_id AND t.id = l.child_task_id
             WHERE l.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND l.epic_task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") AND t.deleted = 0 GROUP BY l.epic_task_id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let epic_id: TaskId = row.get("epic_task_id");
            rollups.insert(
                epic_id,
                EpicRollup {
                    total: usize::try_from(row.get::<i64, _>("total")).unwrap_or(usize::MAX),
                    open: usize::try_from(row.get::<i64, _>("open")).unwrap_or(usize::MAX),
                    done: usize::try_from(row.get::<i64, _>("done")).unwrap_or(usize::MAX),
                    canceled: usize::try_from(row.get::<i64, _>("canceled")).unwrap_or(usize::MAX),
                    blocked: usize::try_from(row.get::<i64, _>("blocked")).unwrap_or(usize::MAX),
                    overdue: usize::try_from(row.get::<i64, _>("overdue")).unwrap_or(usize::MAX),
                    ready: usize::try_from(row.get::<i64, _>("ready")).unwrap_or(usize::MAX),
                    latest_activity_at: row.get("latest_activity_at"),
                },
            );
        }
    }
    Ok(rollups)
}

pub(super) async fn epic_parents_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, TaskDependencyLink>> {
    let mut links = HashMap::new();
    if task_ids.is_empty() {
        return Ok(links);
    }
    for chunk in task_ids.chunks(super::SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT l.child_task_id AS source_task_id,
                    t.id, t.title, t.status, t.priority, p.key AS project_key, p.prefix AS project_prefix,
                    CASE WHEN ",
        );
        query.push(fragments::open_task_clause("t"));
        query.push(
            " THEN 1 ELSE 0 END AS unresolved
             FROM task_epic_links l
             JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             WHERE l.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND l.child_task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") AND t.deleted = 0 ORDER BY t.title, l.created_at, t.id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let source_task_id: TaskId = row.get("source_task_id");
            links.insert(
                source_task_id,
                super::dependency_link_from_row(&row, workspace_id, display_refs),
            );
        }
    }
    Ok(links)
}
