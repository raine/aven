use anyhow::Result;
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};

use crate::choices::{TaskPriority, TaskStatus};
use crate::db::task_from_row;
use crate::labels::ensure_label_exists_in_workspace;
use crate::projects::resolve_existing_project_in_workspace;
use crate::queue::{now_seconds, queue_meta, queue_order};
use crate::refs::display_refs_for_tasks;
use crate::task_enrichment::load_task_enrichment;
use crate::workspaces::active_workspace_id;

use super::sorting::push_sort;
use super::{SortDirection, TaskFilters, TaskListItem, TaskQueryMode, TaskSort};

pub(crate) async fn list_task_items(
    conn: &mut SqliteConnection,
    filters: TaskFilters,
    mode: TaskQueryMode,
    sort: TaskSort,
    direction: SortDirection,
) -> Result<Vec<TaskListItem>> {
    let workspace_id = active_workspace_id();
    list_task_items_in_workspace(conn, &workspace_id, filters, mode, sort, direction).await
}

pub(crate) async fn list_task_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    filters: TaskFilters,
    mode: TaskQueryMode,
    sort: TaskSort,
    direction: SortDirection,
) -> Result<Vec<TaskListItem>> {
    if let Some(status) = filters.status.as_deref() {
        TaskStatus::parse(status)?;
    }
    for status in &filters.statuses {
        TaskStatus::parse(status)?;
    }
    let hide_done = filters.hide_done && filters.status.is_none() && filters.statuses.is_empty();
    if let Some(priority) = filters.priority.as_deref() {
        TaskPriority::parse(priority)?;
    }

    let project = if let Some(project) = filters.project.as_deref() {
        Some(resolve_existing_project_in_workspace(conn, workspace_id, project).await?)
    } else {
        None
    };
    let label = if let Some(label) = filters.label.as_deref() {
        Some(ensure_label_exists_in_workspace(conn, workspace_id, label).await?)
    } else {
        None
    };

    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at,
         t.queue_activity_at, t.deleted
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id",
    );

    let mut filters_added = 0;
    push_filter_prefix(&mut query, &mut filters_added);
    query.push("t.workspace_id = ");
    query.push_bind(workspace_id.to_string());
    if filters.deleted_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.deleted = 1");
    } else if !filters.include_deleted {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.deleted = 0");
    }
    if hide_done {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.status NOT IN ('done', 'canceled')");
    }
    if let Some(project) = project {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.project_id = ");
        query.push_bind(project.id);
    }
    if let Some(status) = filters.status {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.status = ");
        query.push_bind(status);
    }
    if !filters.statuses.is_empty() {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.status IN (");
        let mut separated = query.separated(", ");
        for status in filters.statuses {
            separated.push_bind(status);
        }
        separated.push_unseparated(")");
    }
    if let Some(priority) = filters.priority {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.priority = ");
        query.push_bind(priority);
    }
    if let Some(label) = label {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.id IN (SELECT tl.task_id FROM task_labels tl INDEXED BY idx_task_labels_workspace_label_task WHERE tl.workspace_id = ");
        query.push_bind(workspace_id);
        query.push(" AND tl.label = ");
        query.push_bind(label);
        query.push(")");
    }
    if filters.conflicts_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("EXISTS (SELECT 1 FROM conflicts c WHERE c.workspace_id = t.workspace_id AND c.task_id = t.id AND c.resolved = 0)");
    }
    if !filters.task_ids.is_empty() {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.id IN (");
        let mut separated = query.separated(", ");
        for task_id in &filters.task_ids {
            separated.push_bind(task_id);
        }
        separated.push_unseparated(")");
    }
    if filters.ready_only || filters.blocked_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.deleted = 0");
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.status NOT IN ('done', 'canceled')");
    }
    if filters.ready_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push(
                "NOT EXISTS (SELECT 1 FROM task_dependencies d
                 JOIN tasks blocker ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
                 WHERE d.workspace_id = t.workspace_id AND d.task_id = t.id
                   AND blocker.deleted = 0 AND blocker.status NOT IN ('done', 'canceled'))",
            );
    }
    if filters.blocked_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push(
                "EXISTS (SELECT 1 FROM task_dependencies d
                 JOIN tasks blocker ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
                 WHERE d.workspace_id = t.workspace_id AND d.task_id = t.id
                   AND blocker.deleted = 0 AND blocker.status NOT IN ('done', 'canceled'))",
            );
    }
    if let Some(search) = filters.search.filter(|search| !search.is_empty()) {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("(t.title LIKE ");
        query.push_bind(format!("%{search}%"));
        query.push(" OR t.description LIKE ");
        query.push_bind(format!("%{search}%"));
        query.push(")");
    }

    if mode == TaskQueryMode::Flat {
        push_sort(&mut query, sort, direction);
    }

    let rows = query.build().fetch_all(&mut *conn).await?;
    let tasks = rows
        .into_iter()
        .map(|row| task_from_row(&row))
        .collect::<Result<Vec<_>>>()?;
    let display_refs = display_refs_for_tasks(conn, &tasks).await?;
    let task_ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    let mut enrichment = load_task_enrichment(conn, workspace_id, &task_ids).await?;
    let mut items = Vec::with_capacity(tasks.len());
    let now_seconds = now_seconds();
    for task in tasks {
        let display_ref = display_refs
            .get(&task.id)
            .cloned()
            .unwrap_or_else(|| format!("{}-{}", task.project_prefix, task.id));
        let labels = enrichment
            .labels_by_task
            .remove(&task.id)
            .unwrap_or_default();
        let notes = enrichment
            .notes_by_task
            .remove(&task.id)
            .unwrap_or_default();
        let has_conflict = enrichment.conflicted_task_ids.contains(&task.id);
        let unresolved_blocker_count = *enrichment
            .unresolved_blocker_counts_by_task
            .get(&task.id)
            .unwrap_or(&0);
        let dependent_count = *enrichment
            .dependent_counts_by_task
            .get(&task.id)
            .unwrap_or(&0);
        let depends_on = enrichment
            .depends_on_by_task
            .remove(&task.id)
            .unwrap_or_default();
        let blocks = enrichment
            .blocks_by_task
            .remove(&task.id)
            .unwrap_or_default();
        let queue = queue_meta(
            &task,
            has_conflict,
            unresolved_blocker_count > 0,
            now_seconds,
        );
        items.push(TaskListItem {
            task,
            display_ref,
            labels,
            notes,
            has_conflict,
            unresolved_blocker_count,
            dependent_count,
            depends_on,
            blocks,
            queue,
        });
    }
    if mode == TaskQueryMode::RankedQueue {
        items.sort_by(|a, b| queue_order((&a.task, a.queue), (&b.task, b.queue)));
    }
    if !filters.task_ids.is_empty() {
        let order = filters.task_ids;
        items.sort_by_key(|item| {
            order
                .iter()
                .position(|task_id| task_id == &item.task.id)
                .unwrap_or(order.len())
        });
    }
    Ok(items)
}

fn push_filter_prefix(query: &mut QueryBuilder<Sqlite>, filters: &mut usize) {
    if *filters == 0 {
        query.push(" WHERE ");
    } else {
        query.push(" AND ");
    }
    *filters += 1;
}
