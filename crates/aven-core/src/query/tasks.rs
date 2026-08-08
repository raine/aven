use crate::ids::WorkspaceId;
use anyhow::Result;
use chrono::Local;
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};

use crate::choices::{TaskPriority, TaskStatus};
use crate::db::task_from_row;
use crate::labels::ensure_label_exists_in_workspace;
use crate::projects::resolve_existing_project_in_workspace;
use crate::queue::{now_seconds, queue_order};
use crate::refs::DisplayRefContext;

use super::fragments;
use super::hydration::{TaskHydration, build_task_list_items};
use super::sorting::push_sort;
use super::{
    SortDirection, TaskAvailabilityFilter, TaskFilters, TaskIdFilter, TaskListItem, TaskQueryMode,
    TaskSort,
};

struct TaskListRead {
    filters: TaskFilters,
    mode: TaskQueryMode,
    sort: TaskSort,
    direction: SortDirection,
    limit: Option<usize>,
    hydration: TaskHydration,
}

pub async fn list_task_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    filters: TaskFilters,
    mode: TaskQueryMode,
    sort: TaskSort,
    direction: SortDirection,
) -> Result<Vec<TaskListItem>> {
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    list_task_items_with_display_refs(
        conn,
        workspace_id,
        filters,
        mode,
        sort,
        direction,
        &display_refs,
    )
    .await
}

pub async fn list_task_items_with_display_refs(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    filters: TaskFilters,
    mode: TaskQueryMode,
    sort: TaskSort,
    direction: SortDirection,
    display_refs: &DisplayRefContext,
) -> Result<Vec<TaskListItem>> {
    query_task_items(
        conn,
        workspace_id,
        display_refs,
        TaskListRead {
            filters,
            mode,
            sort,
            direction,
            limit: None,
            hydration: TaskHydration::Detail,
        },
    )
    .await
}

pub async fn list_task_summary_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    filters: TaskFilters,
    mode: TaskQueryMode,
    sort: TaskSort,
    direction: SortDirection,
    limit: Option<usize>,
) -> Result<Vec<TaskListItem>> {
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    query_task_items(
        conn,
        workspace_id,
        &display_refs,
        TaskListRead {
            filters,
            mode,
            sort,
            direction,
            limit,
            hydration: TaskHydration::List,
        },
    )
    .await
}

async fn query_task_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    display_refs: &DisplayRefContext,
    read: TaskListRead,
) -> Result<Vec<TaskListItem>> {
    let TaskListRead {
        filters,
        mode,
        sort,
        direction,
        limit,
        hydration,
    } = read;
    let expand_recurring = filters.expand_recurring;
    let status_filter = filters
        .status
        .as_deref()
        .map(TaskStatus::parse)
        .transpose()?;
    let status_filters = filters
        .statuses
        .iter()
        .map(|status| TaskStatus::parse(status).map_err(anyhow::Error::from))
        .collect::<Result<Vec<_>>>()?;
    let hide_done = filters.hide_done && filters.status.is_none() && filters.statuses.is_empty();
    let terminal_tasks_excluded = hide_done
        || filters.ready_only
        || filters.blocked_only
        || filters.overdue_only
        || filters.availability == TaskAvailabilityFilter::Upcoming
        || status_filter.is_some_and(|status| !status.is_terminal())
        || (!status_filters.is_empty()
            && status_filters.iter().all(|status| !status.is_terminal()));
    let limit_in_sql = limit.is_some()
        && mode == TaskQueryMode::Flat
        && matches!(&filters.task_ids, TaskIdFilter::Unrestricted)
        && (expand_recurring || terminal_tasks_excluded);
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
    let mut metadata = Vec::with_capacity(filters.metadata.len());
    for filter in &filters.metadata {
        let field =
            crate::metadata::find_metadata_field_in_workspace(conn, workspace_id, &filter.key)
                .await?
                .ok_or_else(|| anyhow::anyhow!("error unknown-metadata-field"))?;
        metadata.push((field.id, filter.value.clone()));
    }
    let mut has_metadata = Vec::with_capacity(filters.has_metadata.len());
    for key in &filters.has_metadata {
        let field = crate::metadata::find_metadata_field_in_workspace(conn, workspace_id, key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("error unknown-metadata-field"))?;
        has_metadata.push(field.id);
    }
    let mut missing_metadata = Vec::with_capacity(filters.missing_metadata.len());
    for key in &filters.missing_metadata {
        let field = crate::metadata::find_metadata_field_in_workspace(conn, workspace_id, key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("error unknown-metadata-field"))?;
        missing_metadata.push(field.id);
    }

    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.source, t.created_at, t.updated_at,
         t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id",
    );

    let mut filters_added = 0;
    push_filter_prefix(&mut query, &mut filters_added);
    query.push("t.workspace_id = ");
    query.push_bind(workspace_id.to_string());
    if matches!(&filters.task_ids, TaskIdFilter::Unrestricted) {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push(fragments::ordinary_task_clause("t"));
    }
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
    for (field_id, value) in metadata {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("EXISTS (SELECT 1 FROM task_metadata m WHERE m.workspace_id = t.workspace_id AND m.task_id = t.id AND m.field_id = ");
        query.push_bind(field_id);
        query.push(" AND m.value = ");
        query.push_bind(value);
        query.push(")");
    }
    for field_id in has_metadata {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("EXISTS (SELECT 1 FROM task_metadata m WHERE m.workspace_id = t.workspace_id AND m.task_id = t.id AND m.field_id = ");
        query.push_bind(field_id);
        query.push(")");
    }
    for field_id in missing_metadata {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("NOT EXISTS (SELECT 1 FROM task_metadata m WHERE m.workspace_id = t.workspace_id AND m.task_id = t.id AND m.field_id = ");
        query.push_bind(field_id);
        query.push(")");
    }
    push_availability_filter(&mut query, &mut filters_added, filters.availability);
    if filters.overdue_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.deleted = 0 AND t.status NOT IN ('done', 'canceled') AND t.due_on != '' AND t.due_on < ");
        query.push_bind(Local::now().date_naive().format("%Y-%m-%d").to_string());
    }
    if filters.conflicts_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("EXISTS (SELECT 1 FROM conflicts c WHERE c.workspace_id = t.workspace_id AND c.task_id = t.id AND c.resolved = 0)");
    }
    match &filters.task_ids {
        TaskIdFilter::Unrestricted => {}
        TaskIdFilter::Only(task_ids) if task_ids.is_empty() => {
            push_filter_prefix(&mut query, &mut filters_added);
            query.push("0 = 1");
        }
        TaskIdFilter::Only(task_ids) => {
            push_filter_prefix(&mut query, &mut filters_added);
            query.push("t.id IN (");
            let mut separated = query.separated(", ");
            for task_id in task_ids {
                separated.push_bind(task_id);
            }
            separated.push_unseparated(")");
        }
    }
    if filters.ready_only || filters.blocked_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push(fragments::open_task_clause("t"));
    }
    if filters.ready_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push(format!(
                "NOT EXISTS (SELECT 1 FROM task_dependencies d
                 JOIN tasks blocker ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
                 WHERE d.workspace_id = t.workspace_id AND d.task_id = t.id
                   AND {})",
                fragments::open_task_clause("blocker"),
            ));
    }
    if filters.blocked_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push(format!(
                "EXISTS (SELECT 1 FROM task_dependencies d
                 JOIN tasks blocker ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
                 WHERE d.workspace_id = t.workspace_id AND d.task_id = t.id
                   AND {})",
                fragments::open_task_clause("blocker"),
            ));
    }
    if filters.epics_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.is_epic = 1");
    }
    if filters.exclude_epics {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.is_epic = 0");
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
    if limit_in_sql {
        query.push(" LIMIT ");
        query.push_bind(i64::try_from(limit.unwrap_or(usize::MAX)).unwrap_or(i64::MAX));
    }

    let rows = query.build().fetch_all(&mut *conn).await?;
    let tasks = rows
        .into_iter()
        .map(|row| task_from_row(&row))
        .collect::<Result<Vec<_>>>()?;
    let now_seconds = now_seconds();
    let local_today = Local::now().date_naive();
    let mut items = build_task_list_items(
        conn,
        workspace_id,
        tasks,
        now_seconds,
        local_today,
        display_refs,
        hydration,
    )
    .await?;
    if !expand_recurring {
        let at = crate::ids::now_utc();
        items = super::recurrence::group_terminal_task_items(conn, workspace_id, items, at).await?;
    }
    if mode == TaskQueryMode::RankedQueue {
        items.sort_by(|a, b| queue_order((&a.task, a.queue), (&b.task, b.queue)));
    }
    if let TaskIdFilter::Only(order) = filters.task_ids {
        items.sort_by_key(|item| {
            order
                .iter()
                .position(|task_id| task_id == &item.task.id)
                .unwrap_or(order.len())
        });
    }
    if let Some(limit) = limit.filter(|_| !limit_in_sql) {
        items.truncate(limit);
    }
    Ok(items)
}

fn push_availability_filter(
    query: &mut QueryBuilder<Sqlite>,
    filters_added: &mut usize,
    availability: TaskAvailabilityFilter,
) {
    match availability {
        TaskAvailabilityFilter::All => {}
        TaskAvailabilityFilter::Available => {
            push_filter_prefix(query, filters_added);
            query.push("(t.available_at = '' OR t.available_at <= ");
            query.push_bind(crate::ids::now());
            query.push(")");
        }
        TaskAvailabilityFilter::Upcoming => {
            push_filter_prefix(query, filters_added);
            query.push("t.deleted = 0 AND t.status NOT IN ('done', 'canceled') AND t.available_at != '' AND t.available_at > ");
            query.push_bind(crate::ids::now());
        }
    }
}

fn push_filter_prefix(query: &mut QueryBuilder<Sqlite>, filters: &mut usize) {
    if *filters == 0 {
        query.push(" WHERE ");
    } else {
        query.push(" AND ");
    }
    *filters += 1;
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tests;
