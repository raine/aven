use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::db::{task_from_row, task_has_conflict};
use crate::labels::ensure_label_exists;
use crate::projects::resolve_existing_project;
use crate::refs::display_ref;
use crate::task_render::labels_for_task;
use crate::types::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskSort {
    Queue,
    Created,
    Updated,
    Project,
    Title,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TaskFilters {
    pub(crate) project: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) include_deleted: bool,
    pub(crate) search: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskListItem {
    pub(crate) task: Task,
    pub(crate) display_ref: String,
    pub(crate) labels: Vec<String>,
    pub(crate) has_conflict: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectListItem {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
    pub(crate) open_count: i64,
    pub(crate) inbox_count: i64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SidebarCounts {
    pub(crate) all: i64,
    pub(crate) inbox: i64,
    pub(crate) active: i64,
    pub(crate) todo: i64,
}

pub(crate) async fn list_task_items(
    conn: &mut SqliteConnection,
    filters: TaskFilters,
    sort: TaskSort,
) -> Result<Vec<TaskListItem>> {
    if let Some(status) = filters.status.as_deref() {
        validate_choice("status", status, STATUSES)?;
    }
    if let Some(priority) = filters.priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
    }

    let project_key = if let Some(project) = filters.project.as_deref() {
        Some(resolve_existing_project(conn, project).await?.key)
    } else {
        None
    };
    let label = if let Some(label) = filters.label.as_deref() {
        Some(ensure_label_exists(conn, label).await?)
    } else {
        None
    };

    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT t.id, t.title, t.description, t.project_key, p.prefix, t.status, t.priority,
         t.created_at, t.updated_at, t.deleted
         FROM tasks t JOIN projects p ON p.key = t.project_key",
    );

    let mut filters_added = 0;
    if !filters.include_deleted {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.deleted = 0");
    }
    if let Some(project_key) = project_key {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.project_key = ");
        query.push_bind(project_key);
    }
    if let Some(status) = filters.status {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.status = ");
        query.push_bind(status);
    }
    if let Some(priority) = filters.priority {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.priority = ");
        query.push_bind(priority);
    }
    if let Some(label) = label {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("EXISTS (SELECT 1 FROM task_labels tl WHERE tl.task_id = t.id AND tl.label = ");
        query.push_bind(label);
        query.push(")");
    }
    if let Some(search) = filters.search.filter(|search| !search.is_empty()) {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("(t.title LIKE ");
        query.push_bind(format!("%{search}%"));
        query.push(" OR t.description LIKE ");
        query.push_bind(format!("%{search}%"));
        query.push(")");
    }

    push_sort(&mut query, sort);

    let rows = query.build().fetch_all(&mut *conn).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let task = task_from_row(&row)?;
        let display_ref = display_ref(conn, &task).await?;
        let labels = labels_for_task(conn, &task.id).await?;
        let has_conflict = task_has_conflict(conn, &task.id).await?;
        items.push(TaskListItem {
            task,
            display_ref,
            labels,
            has_conflict,
        });
    }
    Ok(items)
}

pub(crate) async fn list_project_items(
    conn: &mut SqliteConnection,
) -> Result<Vec<ProjectListItem>> {
    let rows = sqlx::query(
        "SELECT p.key, p.name, p.prefix,
         COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS open_count,
         COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count
         FROM projects p
         LEFT JOIN tasks t ON t.project_key = p.key
         WHERE p.deleted = 0
         GROUP BY p.key, p.name, p.prefix
         ORDER BY p.key",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ProjectListItem {
            key: row.get("key"),
            name: row.get("name"),
            prefix: row.get("prefix"),
            open_count: row.get("open_count"),
            inbox_count: row.get("inbox_count"),
        })
        .collect())
}

pub(crate) async fn sidebar_counts(conn: &mut SqliteConnection) -> Result<SidebarCounts> {
    let row = sqlx::query(
        "SELECT
         COALESCE(SUM(CASE WHEN deleted = 0 THEN 1 ELSE 0 END), 0) AS all_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'active' THEN 1 ELSE 0 END), 0) AS active_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'todo' THEN 1 ELSE 0 END), 0) AS todo_count
         FROM tasks",
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(SidebarCounts {
        all: row.get("all_count"),
        inbox: row.get("inbox_count"),
        active: row.get("active_count"),
        todo: row.get("todo_count"),
    })
}

fn push_filter_prefix(query: &mut QueryBuilder<Sqlite>, filters: &mut usize) {
    if *filters == 0 {
        query.push(" WHERE ");
    } else {
        query.push(" AND ");
    }
    *filters += 1;
}

fn push_sort(query: &mut QueryBuilder<Sqlite>, sort: TaskSort) {
    match sort {
        TaskSort::Queue => query.push(
            " ORDER BY
              CASE t.status
                WHEN 'active' THEN 0
                WHEN 'todo' THEN 1
                WHEN 'inbox' THEN 2
                WHEN 'backlog' THEN 3
                WHEN 'done' THEN 4
                WHEN 'canceled' THEN 5
                ELSE 6
              END,
              CASE t.priority
                WHEN 'urgent' THEN 0
                WHEN 'high' THEN 1
                WHEN 'medium' THEN 2
                WHEN 'low' THEN 3
                WHEN 'none' THEN 4
                ELSE 5
              END,
              t.created_at ASC",
        ),
        TaskSort::Created => query.push(" ORDER BY t.created_at DESC"),
        TaskSort::Updated => query.push(" ORDER BY t.updated_at DESC, t.created_at DESC"),
        TaskSort::Project => query.push(" ORDER BY t.project_key, t.created_at DESC"),
        TaskSort::Title => query.push(" ORDER BY lower(t.title), t.created_at DESC"),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;

    async fn test_conn() -> (tempfile::TempDir, sqlx::pool::PoolConnection<Sqlite>) {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let conn = pool.acquire().await.unwrap();
        (temp, conn)
    }

    #[tokio::test]
    async fn queue_sort_orders_status_then_priority_then_created_at() {
        let (_temp, mut conn) = test_conn().await;
        sqlx::query(
            "INSERT INTO projects(key, name, prefix, created_at, updated_at)
             VALUES ('app', 'app', 'APP', 't', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        for (id, title, status, priority, created_at) in [
            ("0000000000000001", "done urgent", "done", "urgent", "001"),
            ("0000000000000002", "inbox urgent", "inbox", "urgent", "002"),
            ("0000000000000003", "active low", "active", "low", "003"),
            ("0000000000000004", "todo urgent", "todo", "urgent", "004"),
            (
                "0000000000000005",
                "active urgent",
                "active",
                "urgent",
                "005",
            ),
        ] {
            sqlx::query(
                "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
                 VALUES (?, ?, '', 'app', ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(title)
            .bind(status)
            .bind(priority)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        let items = list_task_items(&mut conn, TaskFilters::default(), TaskSort::Queue)
            .await
            .unwrap();
        let titles = items
            .iter()
            .map(|item| item.task.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            [
                "active urgent",
                "active low",
                "todo urgent",
                "inbox urgent",
                "done urgent"
            ]
        );
    }
}
