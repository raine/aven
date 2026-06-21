use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::db::task_from_row;
use crate::labels::ensure_label_exists_in_workspace;
use crate::projects::resolve_existing_project_in_workspace;
use crate::queue::{QueueMeta, now_seconds, queue_meta, queue_order};
use crate::refs::display_refs_for_tasks;
use crate::task_enrichment::load_task_enrichment;
use crate::types::Task;
use crate::workspaces::active_workspace_id;

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
    pub(crate) hide_done: bool,
    pub(crate) conflicts_only: bool,
    pub(crate) search: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskListItem {
    pub(crate) task: Task,
    pub(crate) display_ref: String,
    pub(crate) labels: Vec<String>,
    pub(crate) has_conflict: bool,
    pub(crate) queue: QueueMeta,
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
    pub(crate) done: i64,
}

pub(crate) async fn list_task_items(
    conn: &mut SqliteConnection,
    filters: TaskFilters,
    sort: TaskSort,
    direction: SortDirection,
) -> Result<Vec<TaskListItem>> {
    let workspace_id = active_workspace_id();
    list_task_items_in_workspace(conn, &workspace_id, filters, sort, direction).await
}

pub(crate) async fn list_task_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    filters: TaskFilters,
    sort: TaskSort,
    direction: SortDirection,
) -> Result<Vec<TaskListItem>> {
    if let Some(status) = filters.status.as_deref() {
        validate_choice("status", status, STATUSES)?;
    }
    let hide_done = filters.hide_done && filters.status.is_none();
    if let Some(priority) = filters.priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
    }

    let project_key = if let Some(project) = filters.project.as_deref() {
        Some(
            resolve_existing_project_in_workspace(conn, workspace_id, project)
                .await?
                .key,
        )
    } else {
        None
    };
    let label = if let Some(label) = filters.label.as_deref() {
        Some(ensure_label_exists_in_workspace(conn, workspace_id, label).await?)
    } else {
        None
    };

    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_key,
         p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at, t.deleted
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.key = t.project_key",
    );

    let mut filters_added = 0;
    push_filter_prefix(&mut query, &mut filters_added);
    query.push("t.workspace_id = ");
    query.push_bind(workspace_id.to_string());
    if !filters.include_deleted {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.deleted = 0");
    }
    if hide_done {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("t.status NOT IN ('done', 'canceled')");
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
        query.push("EXISTS (SELECT 1 FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id AND tl.label = ");
        query.push_bind(label);
        query.push(")");
    }
    if filters.conflicts_only {
        push_filter_prefix(&mut query, &mut filters_added);
        query.push("EXISTS (SELECT 1 FROM conflicts c WHERE c.workspace_id = t.workspace_id AND c.task_id = t.id AND c.resolved = 0)");
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
    let task_ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    let mut enrichment = load_task_enrichment(conn, &workspace_id, &task_ids).await?;
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
        let has_conflict = enrichment.conflicted_task_ids.contains(&task.id);
        let queue = queue_meta(&task, has_conflict, now_seconds);
        items.push(TaskListItem {
            task,
            display_ref,
            labels,
            has_conflict,
            queue,
        });
    }
    if sort == TaskSort::Queue {
        items.sort_by(|a, b| queue_order((&a.task, a.queue), (&b.task, b.queue)));
        if direction == SortDirection::Desc {
            items.reverse();
        }
    }
    Ok(items)
}

#[allow(dead_code)]
pub(crate) async fn list_project_items(
    conn: &mut SqliteConnection,
) -> Result<Vec<ProjectListItem>> {
    let workspace_id = active_workspace_id();
    list_project_items_in_workspace(conn, &workspace_id).await
}

pub(crate) async fn list_project_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Vec<ProjectListItem>> {
    let rows = sqlx::query(
        "SELECT p.key, p.name, p.prefix,
         COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS open_count,
         COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count
         FROM projects p
         LEFT JOIN tasks t ON t.workspace_id = p.workspace_id AND t.project_key = p.key
         WHERE p.workspace_id = ? AND p.deleted = 0
         GROUP BY p.key, p.name, p.prefix
         ORDER BY p.key",
    )
    .bind(workspace_id)
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

#[allow(dead_code)]
pub(crate) async fn sidebar_counts(conn: &mut SqliteConnection) -> Result<SidebarCounts> {
    let workspace_id = active_workspace_id();
    sidebar_counts_in_workspace(conn, &workspace_id).await
}

pub(crate) async fn sidebar_counts_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<SidebarCounts> {
    let row = sqlx::query(
        "SELECT
         COALESCE(SUM(CASE WHEN deleted = 0 AND status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS all_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'active' THEN 1 ELSE 0 END), 0) AS active_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'backlog' THEN 1 ELSE 0 END), 0) AS backlog_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'todo' THEN 1 ELSE 0 END), 0) AS todo_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'done' THEN 1 ELSE 0 END), 0) AS done_count,
         (SELECT COUNT(DISTINCT c.task_id)
          FROM conflicts c
          JOIN tasks t ON t.workspace_id = c.workspace_id AND t.id = c.task_id
          WHERE c.workspace_id = ? AND c.resolved = 0 AND t.deleted = 0) AS conflicts_count
         FROM tasks
         WHERE workspace_id = ?",
    )
    .bind(&workspace_id)
    .bind(&workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(SidebarCounts {
        all: row.get("all_count"),
        inbox: row.get("inbox_count"),
        active: row.get("active_count"),
        backlog: row.get("backlog_count"),
        todo: row.get("todo_count"),
        conflicts: row.get("conflicts_count"),
        done: row.get("done_count"),
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
        (TaskSort::Queue, _) => query.push(" ORDER BY t.created_at ASC"),
        (TaskSort::Created, SortDirection::Asc) => query.push(" ORDER BY t.created_at ASC"),
        (TaskSort::Created, SortDirection::Desc) => query.push(" ORDER BY t.created_at DESC"),
        (TaskSort::Updated, SortDirection::Asc) => {
            query.push(" ORDER BY t.updated_at ASC, t.created_at ASC, t.rowid ASC")
        }
        (TaskSort::Updated, SortDirection::Desc) => {
            query.push(" ORDER BY t.updated_at DESC, t.created_at DESC, t.rowid ASC")
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
    use crate::test_support::test_conn;
    use sqlx::SqliteConnection;

    async fn seed_default_project(conn: &mut SqliteConnection) {
        sqlx::query(
            "INSERT INTO projects(key, name, prefix, created_at, updated_at)
             VALUES ('app', 'app', 'APP', 't', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    async fn insert_test_task(
        conn: &mut SqliteConnection,
        id: &str,
        title: &str,
        status: &str,
        priority: &str,
        created_at: &str,
    ) {
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

    async fn insert_test_label(conn: &mut SqliteConnection, task_id: &str, label: &str) {
        let workspace_id = crate::workspaces::active_workspace_id();
        sqlx::query(
            "INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, 't')",
        )
        .bind(&workspace_id)
        .bind(label)
        .execute(&mut *conn)
        .await
        .unwrap();

        sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
            .bind(&workspace_id)
            .bind(task_id)
            .bind(label)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    async fn insert_test_conflict(conn: &mut SqliteConnection, task_id: &str, resolved: bool) {
        let resolved = if resolved { 1_i64 } else { 0_i64 };
        sqlx::query(
            "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, ?, 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, ?)",
        )
        .bind(crate::workspaces::active_workspace_id())
        .bind(task_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .bind(resolved)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    fn listed_titles(items: &[TaskListItem]) -> Vec<&str> {
        items.iter().map(|item| item.task.title.as_str()).collect()
    }

    struct ActiveWorkspaceGuard {
        previous: crate::workspaces::Workspace,
    }

    impl ActiveWorkspaceGuard {
        fn set(workspace: crate::workspaces::Workspace) -> Self {
            let previous = crate::workspaces::active_workspace();
            crate::workspaces::set_active_workspace(workspace);
            Self { previous }
        }
    }

    impl Drop for ActiveWorkspaceGuard {
        fn drop(&mut self) {
            crate::workspaces::set_active_workspace(self.previous.clone());
        }
    }

    async fn seed_workspace_project(
        conn: &mut SqliteConnection,
        workspace_id: &str,
        key: &str,
        name: &str,
        prefix: &str,
    ) {
        sqlx::query(
            "INSERT INTO projects(workspace_id, key, name, prefix, created_at, updated_at)
             VALUES (?, ?, ?, ?, 't', 't')",
        )
        .bind(workspace_id)
        .bind(key)
        .bind(name)
        .bind(prefix)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    async fn seed_workspace_label(conn: &mut SqliteConnection, workspace_id: &str, name: &str) {
        sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, ?, 't')")
            .bind(workspace_id)
            .bind(name)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    async fn seed_workspace_task(
        conn: &mut SqliteConnection,
        workspace_id: &str,
        id: &str,
        title: &str,
        project_key: &str,
        status: &str,
        priority: &str,
        created_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO tasks(workspace_id, id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES (?, ?, ?, '', ?, ?, ?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(id)
        .bind(title)
        .bind(project_key)
        .bind(status)
        .bind(priority)
        .bind(created_at)
        .bind(created_at)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    async fn seed_workspace_task_label(
        conn: &mut SqliteConnection,
        workspace_id: &str,
        task_id: &str,
        label: &str,
    ) {
        sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
            .bind(workspace_id)
            .bind(task_id)
            .bind(label)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    async fn seed_workspace_conflict(conn: &mut SqliteConnection, workspace_id: &str, task_id: &str) {
        sqlx::query(
            "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, ?, 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn queue_sort_orders_status_then_priority_then_created_at() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

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
            insert_test_task(&mut conn, id, title, status, priority, created_at).await;
        }

        let items = list_task_items(
            &mut conn,
            TaskFilters {
                hide_done: true,
                ..TaskFilters::default()
            },
            TaskSort::Queue,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(
            listed_titles(&items),
            ["active urgent", "todo urgent", "active low", "inbox urgent"]
        );
    }

    #[tokio::test]
    async fn queue_view_hides_done_and_canceled_tasks() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        for (id, title, status) in [
            ("0000000000000111", "todo task", "todo"),
            ("0000000000000112", "done task", "done"),
            ("0000000000000113", "canceled task", "canceled"),
        ] {
            insert_test_task(&mut conn, id, title, status, "none", "001").await;
        }

        let items = list_task_items(
            &mut conn,
            TaskFilters {
                hide_done: true,
                ..TaskFilters::default()
            },
            TaskSort::Queue,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(listed_titles(&items), ["todo task"]);

        let counts = sidebar_counts(&mut conn).await.unwrap();
        assert_eq!(counts.all, 1);
        assert_eq!(counts.done, 1);
    }

    #[tokio::test]
    async fn priority_sort_orders_priority_then_created_at() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        for (id, title, priority, created_at) in [
            ("0000000000000101", "none old", "none", "001"),
            ("0000000000000102", "urgent", "urgent", "002"),
            ("0000000000000103", "high", "high", "003"),
            ("0000000000000104", "none new", "none", "004"),
        ] {
            insert_test_task(&mut conn, id, title, "todo", priority, created_at).await;
        }

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Priority,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(
            listed_titles(&items),
            ["urgent", "high", "none new", "none old"]
        );
    }

    #[tokio::test]
    async fn created_sort_respects_direction() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        for (id, title, created_at) in [
            ("0000000000000201", "first", "001"),
            ("0000000000000202", "second", "002"),
        ] {
            insert_test_task(&mut conn, id, title, "todo", "none", created_at).await;
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
        seed_default_project(&mut conn).await;

        for (id, title) in [
            ("0000000000000011", "conflicted"),
            ("0000000000000012", "clean"),
        ] {
            insert_test_task(&mut conn, id, title, "todo", "none", "001").await;
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

    #[tokio::test]
    async fn list_items_include_labels_and_unresolved_conflict_flags() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;
        insert_test_task(&mut conn, "0000000000000301", "labeled", "todo", "none", "001").await;
        insert_test_task(&mut conn, "0000000000000302", "resolved", "todo", "none", "002").await;
        insert_test_task(&mut conn, "0000000000000303", "plain", "todo", "none", "003").await;

        insert_test_label(&mut conn, "0000000000000301", "zeta").await;
        insert_test_label(&mut conn, "0000000000000301", "alpha").await;
        insert_test_conflict(&mut conn, "0000000000000301", false).await;
        insert_test_conflict(&mut conn, "0000000000000302", true).await;

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();

        assert_eq!(
            items[0].labels,
            vec!["alpha".to_string(), "zeta".to_string()]
        );
        assert!(items[0].has_conflict);
        assert!(!items[1].has_conflict);
        assert!(items[2].labels.is_empty());
    }

    #[tokio::test]
    async fn list_items_preserve_display_refs_with_hidden_collisions() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        insert_test_task(&mut conn, "ABCD000000000001", "visible", "todo", "none", "001").await;
        insert_test_task(&mut conn, "ABCD999999999999", "done", "done", "none", "002").await;

        let items = list_task_items(
            &mut conn,
            TaskFilters {
                hide_done: true,
                ..TaskFilters::default()
            },
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_ref, "APP-ABCD0");
    }

    #[tokio::test]
    async fn queue_sort_ranks_conflicted_tasks_ahead_of_clean_peers() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        insert_test_task(&mut conn, "0000000000000401", "clean", "todo", "none", "001").await;
        insert_test_task(&mut conn, "0000000000000402", "conflicted", "todo", "none", "002").await;
        insert_test_conflict(&mut conn, "0000000000000402", false).await;

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Queue,
            SortDirection::Asc,
        )
        .await
        .unwrap();

        assert_eq!(listed_titles(&items), ["conflicted", "clean"]);
    }

    #[tokio::test]
    async fn explicit_workspace_read_apis_scope_results() {
        let (_temp, mut conn) = test_conn().await;
        let alpha_id = crate::workspaces::DEFAULT_WORKSPACE_ID.to_string();
        let beta = crate::workspaces::create_workspace(&mut conn, "Beta").await.unwrap();
        seed_workspace_project(&mut conn, &alpha_id, "app", "Alpha", "ALP").await;
        seed_workspace_project(&mut conn, &beta.id, "app", "Beta", "BET").await;
        seed_workspace_label(&mut conn, &alpha_id, "shared").await;
        seed_workspace_label(&mut conn, &beta.id, "shared").await;
        seed_workspace_task(
            &mut conn,
            &alpha_id,
            "ALPHA0000000001",
            "alpha task",
            "app",
            "todo",
            "high",
            "001",
        )
        .await;
        seed_workspace_task(
            &mut conn,
            &beta.id,
            "BETA00000000001",
            "beta task",
            "app",
            "done",
            "low",
            "002",
        )
        .await;
        seed_workspace_task_label(&mut conn, &alpha_id, "ALPHA0000000001", "shared").await;
        seed_workspace_task_label(&mut conn, &beta.id, "BETA00000000001", "shared").await;
        seed_workspace_conflict(&mut conn, &alpha_id, "ALPHA0000000001").await;
        let _guard = ActiveWorkspaceGuard::set(beta.clone());

        let alpha_tasks = list_task_items_in_workspace(
            &mut conn,
            &alpha_id,
            TaskFilters {
                project: Some("app".to_string()),
                label: Some("shared".to_string()),
                conflicts_only: true,
                ..TaskFilters::default()
            },
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(listed_titles(&alpha_tasks), ["alpha task"]);
        assert_eq!(alpha_tasks[0].labels, vec!["shared".to_string()]);
        assert!(alpha_tasks[0].has_conflict);

        let beta_tasks = list_task_items_in_workspace(
            &mut conn,
            &beta.id,
            TaskFilters::default(),
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(listed_titles(&beta_tasks), ["beta task"]);

        let alpha_projects = list_project_items_in_workspace(&mut conn, &alpha_id)
            .await
            .unwrap();
        assert_eq!(alpha_projects.len(), 1);
        assert_eq!(alpha_projects[0].key, "app");
        assert_eq!(alpha_projects[0].open_count, 1);

        let beta_projects = list_project_items_in_workspace(&mut conn, &beta.id)
            .await
            .unwrap();
        assert_eq!(beta_projects.len(), 1);
        assert_eq!(beta_projects[0].key, "app");
        assert_eq!(beta_projects[0].open_count, 0);

        let alpha_counts = sidebar_counts_in_workspace(&mut conn, &alpha_id).await.unwrap();
        assert_eq!(alpha_counts.all, 1);
        assert_eq!(alpha_counts.todo, 1);
        assert_eq!(alpha_counts.conflicts, 1);
        assert_eq!(alpha_counts.done, 0);

        let beta_counts = sidebar_counts_in_workspace(&mut conn, &beta.id).await.unwrap();
        assert_eq!(beta_counts.all, 0);
        assert_eq!(beta_counts.done, 1);
        assert_eq!(beta_counts.conflicts, 0);
    }

    #[tokio::test]
    async fn active_workspace_wrappers_delegate_to_active_workspace() {
        let (_temp, mut conn) = test_conn().await;
        let alpha_id = crate::workspaces::DEFAULT_WORKSPACE_ID.to_string();
        let beta = crate::workspaces::create_workspace(&mut conn, "Beta").await.unwrap();
        seed_workspace_project(&mut conn, &alpha_id, "alpha", "Alpha", "ALP").await;
        seed_workspace_project(&mut conn, &beta.id, "beta", "Beta", "BET").await;
        seed_workspace_task(
            &mut conn,
            &alpha_id,
            "ALPHA0000000001",
            "alpha task",
            "alpha",
            "todo",
            "none",
            "001",
        )
        .await;
        seed_workspace_task(
            &mut conn,
            &beta.id,
            "BETA00000000001",
            "beta task",
            "beta",
            "todo",
            "none",
            "002",
        )
        .await;
        let _guard = ActiveWorkspaceGuard::set(beta.clone());

        let wrapper_tasks = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        let explicit_tasks = list_task_items_in_workspace(
            &mut conn,
            &beta.id,
            TaskFilters::default(),
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(listed_titles(&wrapper_tasks), listed_titles(&explicit_tasks));
        assert_eq!(listed_titles(&wrapper_tasks), ["beta task"]);
        assert_eq!(list_project_items(&mut conn).await.unwrap()[0].key, "beta");
        assert_eq!(sidebar_counts(&mut conn).await.unwrap().all, 1);
    }
}
