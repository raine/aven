mod dependencies;
mod projects;
mod search;
mod sidebar;
mod sorting;
mod tasks;
mod types;

#[allow(unused_imports)]
pub(crate) use dependencies::{TaskDependencyItem, TaskDependencySummary, task_dependency_summary};
#[allow(unused_imports)]
pub(crate) use projects::{list_project_items, list_project_items_in_workspace};
#[allow(unused_imports)]
pub(crate) use search::{
    SearchMatchedField, TaskSearchQuery, TaskSearchResult, search_task_items,
    search_task_items_in_workspace,
};
#[allow(unused_imports)]
pub(crate) use sidebar::{
    sidebar_counts, sidebar_counts_for_scope_in_workspace, sidebar_counts_in_workspace,
};
#[allow(unused_imports)]
pub(crate) use tasks::{list_task_items, list_task_items_in_workspace};
#[allow(unused_imports)]
pub(crate) use types::{
    ProjectListItem, SidebarCounts, SortDirection, TaskDependencyLink, TaskFilters, TaskListItem,
    TaskNote, TaskQueryMode, TaskSort,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_conn;
    use sqlx::SqliteConnection;

    async fn seed_default_project(conn: &mut SqliteConnection) {
        sqlx::query(
            "INSERT INTO projects(id, key, name, prefix, created_at, updated_at)
             VALUES ('PROJECT000000001', 'app', 'app', 'APP', 't', 't')",
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
            "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
             VALUES (?, ?, '', 'PROJECT000000001', ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(title)
        .bind(status)
        .bind(priority)
        .bind(created_at)
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

    fn listed_titles_from_search(items: &[TaskSearchResult]) -> Vec<&str> {
        items
            .iter()
            .map(|item| item.item.task.title.as_str())
            .collect()
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
            "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 't', 't')",
        )
        .bind(crate::ids::new_id())
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

    #[allow(clippy::too_many_arguments)]
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
            "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
             VALUES (?, ?, ?, '', (SELECT id FROM projects WHERE workspace_id = ? AND key = ?), ?, ?, ?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(id)
        .bind(title)
        .bind(workspace_id)
        .bind(project_key)
        .bind(status)
        .bind(priority)
        .bind(created_at)
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

    async fn seed_workspace_conflict(
        conn: &mut SqliteConnection,
        workspace_id: &str,
        task_id: &str,
    ) {
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
            TaskQueryMode::RankedQueue,
            TaskSort::Created,
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
            TaskQueryMode::RankedQueue,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(listed_titles(&items), ["todo task"]);

        let counts = sidebar_counts(&mut conn).await.unwrap();
        assert_eq!(counts.open, 1);
        assert_eq!(counts.done, 2);
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
            TaskQueryMode::Flat,
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
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Desc,
        )
        .await
        .unwrap();
        assert_eq!(items[0].task.title, "second");

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskQueryMode::Flat,
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
            TaskQueryMode::RankedQueue,
            TaskSort::Created,
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
        insert_test_task(
            &mut conn,
            "0000000000000301",
            "labeled",
            "todo",
            "none",
            "001",
        )
        .await;
        insert_test_task(
            &mut conn,
            "0000000000000302",
            "resolved",
            "todo",
            "none",
            "002",
        )
        .await;
        insert_test_task(
            &mut conn,
            "0000000000000303",
            "plain",
            "todo",
            "none",
            "003",
        )
        .await;

        insert_test_label(&mut conn, "0000000000000301", "zeta").await;
        insert_test_label(&mut conn, "0000000000000301", "alpha").await;
        insert_test_conflict(&mut conn, "0000000000000301", false).await;
        insert_test_conflict(&mut conn, "0000000000000302", true).await;
        sqlx::query(
            "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
             VALUES (?, '0000000000000302', '0000000000000301', '003')",
        )
        .bind(crate::workspaces::active_workspace_id())
        .execute(&mut *conn)
        .await
        .unwrap();

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskQueryMode::Flat,
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
        assert_eq!(items[0].blocks[0].display_ref, "APP-0000000000000302");
        assert_eq!(items[0].blocks[0].title, "resolved");
        assert_eq!(items[1].depends_on[0].display_ref, "APP-0000000000000301");
        assert_eq!(items[1].depends_on[0].title, "labeled");
        assert!(!items[1].has_conflict);
        assert!(items[2].labels.is_empty());
    }

    #[tokio::test]
    async fn list_items_include_description_and_note_metadata() {
        let (_temp, mut conn) = test_conn().await;
        let workspace_id = crate::workspaces::active_workspace_id();
        seed_default_project(&mut conn).await;
        insert_test_task(
            &mut conn,
            "0000000000000501",
            "documented",
            "todo",
            "none",
            "001",
        )
        .await;
        sqlx::query("UPDATE tasks SET description = 'details' WHERE workspace_id = ? AND id = ?")
            .bind(&workspace_id)
            .bind("0000000000000501")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
             VALUES (?, 'note-0501-a', '0000000000000501', 'older', '001', 'change-0501-a'),
                    (?, 'note-0501-b', '0000000000000501', 'newer', '002', 'change-0501-b')",
        )
        .bind(&workspace_id)
        .bind(&workspace_id)
        .execute(&mut *conn)
        .await
        .unwrap();

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();

        assert_eq!(items[0].task.description, "details");
        assert_eq!(
            items[0]
                .notes
                .iter()
                .map(|note| note.body.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
    }

    #[tokio::test]
    async fn list_items_preserve_display_refs_with_hidden_collisions() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        insert_test_task(
            &mut conn,
            "ABCD000000000001",
            "visible",
            "todo",
            "none",
            "001",
        )
        .await;
        insert_test_task(&mut conn, "ABCD999999999999", "done", "done", "none", "002").await;

        let items = list_task_items(
            &mut conn,
            TaskFilters {
                hide_done: true,
                ..TaskFilters::default()
            },
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_ref, "APP-ABCD0");
    }

    #[tokio::test]
    async fn search_filter_matches_titles_and_descriptions() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        for (id, title, description, created_at) in [
            (
                "0000000000000501",
                "Title match needle",
                "plain body",
                "001",
            ),
            (
                "0000000000000502",
                "Body only",
                "body contains needle",
                "002",
            ),
            ("0000000000000503", "Unrelated", "plain body", "003"),
        ] {
            sqlx::query(
                "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
                 VALUES (?, ?, ?, 'PROJECT000000001', 'todo', 'none', ?, ?, ?)",
            )
            .bind(id)
            .bind(title)
            .bind(description)
            .bind(created_at)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        let items = list_task_items(
            &mut conn,
            TaskFilters {
                search: Some("needle".to_string()),
                ..TaskFilters::default()
            },
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();

        assert_eq!(listed_titles(&items), ["Title match needle", "Body only"]);
    }

    #[tokio::test]
    async fn task_search_finds_done_labels_and_notes() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;
        insert_test_task(
            &mut conn,
            "7KQ9A1X4MV2P8D6R",
            "Done release cleanup",
            "done",
            "high",
            "001",
        )
        .await;
        insert_test_task(
            &mut conn,
            "8KQ9A1X4MV2P8D6R",
            "Plain inbox",
            "inbox",
            "none",
            "002",
        )
        .await;
        insert_test_label(&mut conn, "8KQ9A1X4MV2P8D6R", "security").await;
        sqlx::query(
            "INSERT INTO notes(id, task_id, body, created_at, change_id)
             VALUES ('note-search', '7KQ9A1X4MV2P8D6R', 'contains pager rotation context', '003', 'change-search')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        let done = search_task_items(
            &mut conn,
            TaskSearchQuery {
                text: "release cleanup".to_string(),
                include_deleted: false,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(listed_titles_from_search(&done), ["Done release cleanup"]);
        assert_eq!(done[0].matched_field, SearchMatchedField::Title);

        let label = search_task_items(
            &mut conn,
            TaskSearchQuery {
                text: "security".to_string(),
                include_deleted: false,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(listed_titles_from_search(&label), ["Plain inbox"]);
        assert_eq!(label[0].matched_field, SearchMatchedField::Label);

        let note = search_task_items(
            &mut conn,
            TaskSearchQuery {
                text: "pager rotation".to_string(),
                include_deleted: false,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(listed_titles_from_search(&note), ["Done release cleanup"]);
        assert_eq!(note[0].matched_field, SearchMatchedField::Note);
        assert!(
            note[0]
                .snippet
                .as_deref()
                .is_some_and(|value| value.contains("pager rotation"))
        );
    }

    #[tokio::test]
    async fn task_search_ranks_refs_and_controls_deleted_results() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;
        insert_test_task(
            &mut conn,
            "7KQ9A1X4MV2P8D6R",
            "Needle in title",
            "todo",
            "none",
            "001",
        )
        .await;
        insert_test_task(
            &mut conn,
            "9KQ9A1X4MV2P8D6R",
            "Deleted needle",
            "todo",
            "none",
            "002",
        )
        .await;
        sqlx::query("UPDATE tasks SET deleted = 1 WHERE id = '9KQ9A1X4MV2P8D6R'")
            .execute(&mut *conn)
            .await
            .unwrap();

        let by_ref = search_task_items(
            &mut conn,
            TaskSearchQuery {
                text: "7KQ9".to_string(),
                include_deleted: false,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(by_ref[0].item.task.id, "7KQ9A1X4MV2P8D6R");
        assert_eq!(by_ref[0].matched_field, SearchMatchedField::Ref);

        let without_deleted = search_task_items(
            &mut conn,
            TaskSearchQuery {
                text: "deleted needle".to_string(),
                include_deleted: false,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert!(without_deleted.is_empty());

        let with_deleted = search_task_items(
            &mut conn,
            TaskSearchQuery {
                text: "deleted needle".to_string(),
                include_deleted: true,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(listed_titles_from_search(&with_deleted), ["Deleted needle"]);
        assert!(with_deleted[0].item.task.deleted);
    }

    #[tokio::test]
    async fn queue_sort_ranks_conflicted_tasks_ahead_of_clean_peers() {
        let (_temp, mut conn) = test_conn().await;
        seed_default_project(&mut conn).await;

        insert_test_task(
            &mut conn,
            "0000000000000401",
            "clean",
            "todo",
            "none",
            "001",
        )
        .await;
        insert_test_task(
            &mut conn,
            "0000000000000402",
            "conflicted",
            "todo",
            "none",
            "002",
        )
        .await;
        insert_test_conflict(&mut conn, "0000000000000402", false).await;

        let items = list_task_items(
            &mut conn,
            TaskFilters::default(),
            TaskQueryMode::RankedQueue,
            TaskSort::Created,
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
        let beta = crate::workspaces::create_workspace(&mut conn, "Beta")
            .await
            .unwrap();
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
            TaskQueryMode::Flat,
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
            TaskQueryMode::Flat,
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

        let alpha_counts = sidebar_counts_in_workspace(&mut conn, &alpha_id)
            .await
            .unwrap();
        assert_eq!(alpha_counts.open, 1);
        assert_eq!(alpha_counts.todo, 1);
        assert_eq!(alpha_counts.conflicts, 1);
        assert_eq!(alpha_counts.done, 0);

        let beta_counts = sidebar_counts_in_workspace(&mut conn, &beta.id)
            .await
            .unwrap();
        assert_eq!(beta_counts.open, 0);
        assert_eq!(beta_counts.done, 1);
        assert_eq!(beta_counts.conflicts, 0);
    }

    #[tokio::test]
    async fn project_filters_use_project_id_after_key_change() {
        let (_temp, mut conn) = test_conn().await;
        let outcome = crate::projects::create_project_in_workspace(
            &mut conn,
            crate::workspaces::DEFAULT_WORKSPACE_ID,
            "App",
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
             VALUES (?, 'ABCDEF0000000000', 'kept', '', ?, 'todo', 'none', 't', 't', 't')",
        )
        .bind(crate::workspaces::DEFAULT_WORKSPACE_ID)
        .bind(&outcome.project.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("UPDATE projects SET key = 'renamed-app' WHERE id = ?")
            .bind(&outcome.project.id)
            .execute(&mut *conn)
            .await
            .unwrap();

        let items = list_task_items(
            &mut conn,
            TaskFilters {
                project: Some("renamed-app".to_string()),
                ..TaskFilters::default()
            },
            TaskQueryMode::Flat,
            TaskSort::Updated,
            SortDirection::Desc,
        )
        .await
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].task.project_key, "renamed-app");
    }

    #[tokio::test]
    async fn active_workspace_wrappers_delegate_to_active_workspace() {
        let (_temp, mut conn) = test_conn().await;
        let alpha_id = crate::workspaces::DEFAULT_WORKSPACE_ID.to_string();
        let beta = crate::workspaces::create_workspace(&mut conn, "Beta")
            .await
            .unwrap();
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
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        let explicit_tasks = list_task_items_in_workspace(
            &mut conn,
            &beta.id,
            TaskFilters::default(),
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
        assert_eq!(
            listed_titles(&wrapper_tasks),
            listed_titles(&explicit_tasks)
        );
        assert_eq!(listed_titles(&wrapper_tasks), ["beta task"]);
        assert_eq!(list_project_items(&mut conn).await.unwrap()[0].key, "beta");
        assert_eq!(sidebar_counts(&mut conn).await.unwrap().open, 1);
    }
}
