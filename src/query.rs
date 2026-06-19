use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::db::{task_from_row, task_has_conflict};
use crate::labels::ensure_label_exists;
use crate::projects::resolve_existing_project;
use crate::refs::display_refs_for_tasks;
use crate::task_render::labels_for_task;
use crate::types::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskSort {
    Queue,
    Created,
    Updated,
    Priority,
    Project,
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TaskFilters {
    pub(crate) project: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) include_deleted: bool,
    pub(crate) conflicts_only: bool,
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
    pub(crate) backlog: i64,
    pub(crate) todo: i64,
    pub(crate) conflicts: i64,
}

pub(crate) async fn list_task_items(
    conn: &mut SqliteConnection,
    filters: TaskFilters,
    sort: TaskSort,
    direction: SortDirection,
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
    if filters.conflicts_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("EXISTS (SELECT 1 FROM conflicts c WHERE c.task_id = t.id AND c.resolved = 0)");
    }
    if let Some(search) = filters.search.filter(|search| !search.is_empty()) {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("(t.title LIKE ");
        query.push_bind(format!("%{search}%"));
        query.push(" OR t.description LIKE ");
        query.push_bind(format!("%{search}%"));
        query.push(")");
    }

    push_sort(&mut query, sort, direction);

    let rows = query.build().fetch_all(&mut *conn).await?;
    let tasks = rows
        .into_iter()
        .map(|row| task_from_row(&row))
        .collect::<Result<Vec<_>>>()?;
    let display_refs = display_refs_for_tasks(conn, &tasks).await?;
    let mut items = Vec::with_capacity(tasks.len());
    for task in tasks {
        let display_ref = display_refs
            .get(&task.id)
            .cloned()
            .unwrap_or_else(|| format!("{}-{}", task.project_prefix, task.id));
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
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'backlog' THEN 1 ELSE 0 END), 0) AS backlog_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'todo' THEN 1 ELSE 0 END), 0) AS todo_count,
         (SELECT COUNT(DISTINCT c.task_id)
          FROM conflicts c
          JOIN tasks t ON t.id = c.task_id
          WHERE c.resolved = 0 AND t.deleted = 0) AS conflicts_count
         FROM tasks",
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(SidebarCounts {
        all: row.get("all_count"),
        inbox: row.get("inbox_count"),
        active: row.get("active_count"),
        backlog: row.get("backlog_count"),
        todo: row.get("todo_count"),
        conflicts: row.get("conflicts_count"),
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

fn push_sort(query: &mut QueryBuilder<Sqlite>, sort: TaskSort, direction: SortDirection) {
    match (sort, direction) {
        (TaskSort::Queue, SortDirection::Asc) => query.push(
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
        (TaskSort::Queue, SortDirection::Desc) => query.push(
            " ORDER BY
              CASE t.status
                WHEN 'canceled' THEN 0
                WHEN 'done' THEN 1
                WHEN 'backlog' THEN 2
                WHEN 'inbox' THEN 3
                WHEN 'todo' THEN 4
                WHEN 'active' THEN 5
                ELSE 6
              END,
              CASE t.priority
                WHEN 'none' THEN 0
                WHEN 'low' THEN 1
                WHEN 'medium' THEN 2
                WHEN 'high' THEN 3
                WHEN 'urgent' THEN 4
                ELSE 5
              END,
              t.created_at DESC",
        ),
        (TaskSort::Created, SortDirection::Asc) => query.push(" ORDER BY t.created_at ASC"),
        (TaskSort::Created, SortDirection::Desc) => query.push(" ORDER BY t.created_at DESC"),
        (TaskSort::Updated, SortDirection::Asc) => {
            query.push(" ORDER BY t.updated_at ASC, t.created_at ASC")
        }
        (TaskSort::Updated, SortDirection::Desc) => {
            query.push(" ORDER BY t.updated_at DESC, t.created_at DESC")
        }
        (TaskSort::Priority, SortDirection::Asc) => query.push(
            " ORDER BY
              CASE t.priority
                WHEN 'urgent' THEN 0
                WHEN 'high' THEN 1
                WHEN 'medium' THEN 2
                WHEN 'low' THEN 3
                WHEN 'none' THEN 4
                ELSE 5
              END,
              t.created_at DESC",
        ),
        (TaskSort::Priority, SortDirection::Desc) => query.push(
            " ORDER BY
              CASE t.priority
                WHEN 'none' THEN 0
                WHEN 'low' THEN 1
                WHEN 'medium' THEN 2
                WHEN 'high' THEN 3
                WHEN 'urgent' THEN 4
                ELSE 5
              END,
              t.created_at DESC",
        ),
        (TaskSort::Project, SortDirection::Asc) => {
            query.push(" ORDER BY t.project_key ASC, t.created_at DESC")
        }
        (TaskSort::Project, SortDirection::Desc) => {
            query.push(" ORDER BY t.project_key DESC, t.created_at DESC")
        }
        (TaskSort::Title, SortDirection::Asc) => {
            query.push(" ORDER BY lower(t.title) ASC, t.created_at DESC")
        }
        (TaskSort::Title, SortDirection::Desc) => {
            query.push(" ORDER BY lower(t.title) DESC, t.created_at DESC")
        }
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

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Queue,
            SortDirection::Asc,
        )
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

    #[tokio::test]
    async fn priority_sort_orders_priority_then_created_at() {
        let (_temp, mut conn) = test_conn().await;
        sqlx::query(
            "INSERT INTO projects(key, name, prefix, created_at, updated_at)
             VALUES ('app', 'app', 'APP', 't', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        for (id, title, priority, created_at) in [
            ("0000000000000101", "none old", "none", "001"),
            ("0000000000000102", "urgent", "urgent", "002"),
            ("0000000000000103", "high", "high", "003"),
            ("0000000000000104", "none new", "none", "004"),
        ] {
            sqlx::query(
                "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
                 VALUES (?, ?, '', 'app', 'todo', ?, ?, ?)",
            )
            .bind(id)
            .bind(title)
            .bind(priority)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Priority,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        let titles = items
            .iter()
            .map(|item| item.task.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["urgent", "high", "none new", "none old"]);
    }

    #[tokio::test]
    async fn created_sort_respects_direction() {
        let (_temp, mut conn) = test_conn().await;
        sqlx::query(
            "INSERT INTO projects(key, name, prefix, created_at, updated_at)
             VALUES ('app', 'app', 'APP', 't', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        for (id, title, created_at) in [
            ("0000000000000201", "first", "001"),
            ("0000000000000202", "second", "002"),
        ] {
            sqlx::query(
                "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
                 VALUES (?, ?, '', 'app', 'todo', 'none', ?, ?)",
            )
            .bind(id)
            .bind(title)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Created,
            SortDirection::Desc,
        )
        .await
        .unwrap();
        assert_eq!(items[0].task.title, "second");

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(items[0].task.title, "first");
    }

    #[tokio::test]
    async fn conflicts_only_filter_returns_unresolved_conflicts() {
        let (_temp, mut conn) = test_conn().await;
        sqlx::query(
            "INSERT INTO projects(key, name, prefix, created_at, updated_at)
             VALUES ('app', 'app', 'APP', 't', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        for (id, title) in [
            ("0000000000000011", "conflicted"),
            ("0000000000000012", "clean"),
        ] {
            sqlx::query(
                "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
                 VALUES (?, ?, '', 'app', 'todo', 'none', '001', '001')",
            )
            .bind(id)
            .bind(title)
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        sqlx::query(
            "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES ('0000000000000011', 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();

        let items = list_task_items(
            &mut conn,
            TaskFilters {
                conflicts_only: true,
                ..TaskFilters::default()
            },
            TaskSort::Queue,
            SortDirection::Asc,
        )
        .await
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].task.title, "conflicted");
        assert!(items[0].has_conflict);
    }
}
