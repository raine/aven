use std::path::Path;

use aven_core::api::{
    CreateTask, ErrorCode, Store, TaskEdit, TaskList, TaskListDirection, TaskListQuery,
    TaskListSort, TaskRecord,
};
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::{ProjectId, WorkspaceId};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};

struct Fixture {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
    store: Store,
    workspace: WorkspaceId,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("task-list.sqlite");
        let store = Store::open(&path).await.unwrap();
        let workspace = store.resolve_workspace("default").await.unwrap().id;
        Self {
            _directory: directory,
            path,
            store,
            workspace,
        }
    }

    async fn task(
        &self,
        title: &str,
        project: &str,
        status: TaskStatus,
        priority: TaskPriority,
        due_on: Option<&str>,
    ) -> TaskRecord {
        self.store
            .create_task(
                &self.workspace,
                CreateTask {
                    title: title.to_string(),
                    description: String::new(),
                    project: project.to_string(),
                    status,
                    priority,
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: due_on.map(str::to_string),
                },
            )
            .await
            .unwrap()
    }

    async fn label(&self, task: &TaskRecord, labels: &[&str]) {
        let database = Database::open(&self.path).await.unwrap();
        let workspace = database.workspace_for_id(&self.workspace).await.unwrap();
        for label in labels {
            database.create_label(&workspace, label).await.unwrap();
        }
        self.store
            .edit_task(
                &self.workspace,
                &task.id,
                TaskEdit {
                    add_labels: labels.iter().map(|label| label.to_string()).collect(),
                    ..TaskEdit::default()
                },
            )
            .await
            .unwrap();
    }

    async fn set_times(&self, task: &TaskRecord, created_at: &str, updated_at: &str) {
        let mut writer = writer(&self.path).await;
        sqlx::query("UPDATE tasks SET created_at = ?, updated_at = ? WHERE id = ?")
            .bind(created_at)
            .bind(updated_at)
            .bind(&task.id)
            .execute(&mut writer)
            .await
            .unwrap();
    }

    async fn project_id(&self, key: &str) -> ProjectId {
        self.store
            .queue_state()
            .await
            .unwrap()
            .projects
            .into_iter()
            .find(|project| project.key == key)
            .unwrap()
            .id
    }

    async fn list(&self, query: TaskListQuery) -> TaskList {
        self.store.task_list(&self.workspace, query).await.unwrap()
    }

    async fn titles(&self, query: TaskListQuery) -> Vec<String> {
        titles(&self.list(query).await)
    }
}

async fn writer(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false),
    )
    .await
    .unwrap()
}

fn titles(list: &TaskList) -> Vec<String> {
    list.rows.iter().map(|row| row.title.clone()).collect()
}

fn query(sort: TaskListSort, direction: TaskListDirection) -> TaskListQuery {
    TaskListQuery::new(sort, direction)
}

#[tokio::test]
async fn task_list_sorts_by_each_key_in_both_directions() {
    let fixture = Fixture::new().await;
    let a = fixture
        .task(
            "alpha",
            "app",
            TaskStatus::Todo,
            TaskPriority::Low,
            Some("2030-01-03"),
        )
        .await;
    let b = fixture
        .task("Bravo", "app", TaskStatus::Todo, TaskPriority::Urgent, None)
        .await;
    let c = fixture
        .task(
            "charlie",
            "app",
            TaskStatus::Todo,
            TaskPriority::None,
            Some("2030-01-01"),
        )
        .await;
    let d = fixture
        .task("delta", "app", TaskStatus::Todo, TaskPriority::High, None)
        .await;
    fixture
        .set_times(&a, "2030-01-01T00:00:00Z", "2030-01-04T00:00:00Z")
        .await;
    fixture
        .set_times(&b, "2030-01-02T00:00:00Z", "2030-01-03T00:00:00Z")
        .await;
    fixture
        .set_times(&c, "2030-01-03T00:00:00Z", "2030-01-02T00:00:00Z")
        .await;
    fixture
        .set_times(&d, "2030-01-04T00:00:00Z", "2030-01-01T00:00:00Z")
        .await;

    use TaskListDirection::{Asc, Desc};
    use TaskListSort::*;
    let cases = [
        (Created, Asc, ["alpha", "Bravo", "charlie", "delta"]),
        (Created, Desc, ["delta", "charlie", "Bravo", "alpha"]),
        (Updated, Asc, ["delta", "charlie", "Bravo", "alpha"]),
        (Updated, Desc, ["alpha", "Bravo", "charlie", "delta"]),
        (Priority, Desc, ["Bravo", "delta", "alpha", "charlie"]),
        (Priority, Asc, ["charlie", "alpha", "delta", "Bravo"]),
        (Title, Asc, ["alpha", "Bravo", "charlie", "delta"]),
        (Title, Desc, ["delta", "charlie", "Bravo", "alpha"]),
    ];
    for (sort, direction, expected) in cases {
        assert_eq!(
            fixture.titles(query(sort, direction)).await,
            expected,
            "{sort:?} {direction:?}"
        );
    }

    let due_asc = fixture.list(query(DueOn, Asc)).await;
    assert_eq!(titles(&due_asc)[..2], ["charlie", "alpha"]);
    let due_desc = fixture.list(query(DueOn, Desc)).await;
    assert_eq!(titles(&due_desc)[..2], ["alpha", "charlie"]);
    for list in [due_asc, due_desc] {
        // Undated tasks stay last in both directions, ordered by ref.
        let undated = &list.rows[2..];
        assert!(undated.iter().all(|row| row.due_on.is_none()));
        assert!(undated[0].display_ref < undated[1].display_ref);
    }
}

#[tokio::test]
async fn task_list_breaks_ties_on_ref() {
    let fixture = Fixture::new().await;
    let mut tasks = Vec::new();
    for title in ["one", "two", "three", "four"] {
        let task = fixture
            .task(title, "app", TaskStatus::Todo, TaskPriority::Medium, None)
            .await;
        fixture
            .set_times(&task, "2030-01-01T00:00:00Z", "2030-01-01T00:00:00Z")
            .await;
        tasks.push(task);
    }
    for sort in [
        TaskListSort::Created,
        TaskListSort::Updated,
        TaskListSort::Priority,
        TaskListSort::DueOn,
    ] {
        for direction in [TaskListDirection::Asc, TaskListDirection::Desc] {
            let refs = fixture
                .list(query(sort, direction))
                .await
                .rows
                .into_iter()
                .map(|row| row.display_ref)
                .collect::<Vec<_>>();
            let mut sorted = refs.clone();
            sorted.sort();
            assert_eq!(refs, sorted, "{sort:?} {direction:?}");
        }
    }
}

#[tokio::test]
async fn task_list_combines_facets_and_counts_each_option() {
    let fixture = Fixture::new().await;
    let todo_copy = fixture
        .task(
            "todo copy",
            "app",
            TaskStatus::Todo,
            TaskPriority::High,
            None,
        )
        .await;
    let todo_bug = fixture
        .task("todo bug", "app", TaskStatus::Todo, TaskPriority::Low, None)
        .await;
    let active_copy = fixture
        .task(
            "active copy",
            "app",
            TaskStatus::Active,
            TaskPriority::High,
            None,
        )
        .await;
    let done_plain = fixture
        .task(
            "done plain",
            "app",
            TaskStatus::Done,
            TaskPriority::None,
            None,
        )
        .await;
    fixture.label(&todo_copy, &["copy"]).await;
    fixture.label(&todo_bug, &["bug"]).await;
    fixture.label(&active_copy, &["copy", "bug"]).await;
    let _ = done_plain;

    let mut statuses = query(TaskListSort::Title, TaskListDirection::Asc);
    statuses.statuses = vec![TaskStatus::Todo, TaskStatus::Active];
    assert_eq!(
        fixture.titles(statuses.clone()).await,
        ["active copy", "todo bug", "todo copy"]
    );

    let mut both = statuses.clone();
    both.labels = vec!["copy".to_string()];
    let list = fixture.list(both.clone()).await;
    assert_eq!(titles(&list), ["active copy", "todo copy"]);
    let status_count = |status| {
        list.statuses
            .iter()
            .find(|facet| facet.value == status)
            .unwrap()
            .count
    };
    // A status option counts tasks matching the other facets, not its own.
    assert_eq!(status_count(TaskStatus::Todo), 1);
    assert_eq!(status_count(TaskStatus::Active), 1);
    assert_eq!(status_count(TaskStatus::Done), 0);
    let label_count = |label: &str| {
        list.labels
            .iter()
            .find(|facet| facet.value == label)
            .unwrap()
            .count
    };
    assert_eq!(label_count("copy"), 2);
    assert_eq!(label_count("bug"), 2);
    assert_eq!(list.statuses.len(), 6);
    assert_eq!(list.priorities.len(), 5);

    let mut labels_or = query(TaskListSort::Title, TaskListDirection::Asc);
    labels_or.labels = vec!["copy".to_string(), "bug".to_string()];
    assert_eq!(
        fixture.titles(labels_or.clone()).await,
        ["active copy", "todo bug", "todo copy"]
    );
    labels_or.priorities = vec![TaskPriority::Low, TaskPriority::None];
    assert_eq!(fixture.titles(labels_or).await, ["todo bug"]);

    let mut missing_label = query(TaskListSort::Title, TaskListDirection::Asc);
    missing_label.labels = vec!["gone".to_string()];
    let list = fixture.list(missing_label).await;
    assert!(list.rows.is_empty());
    assert!(
        list.labels
            .iter()
            .any(|facet| facet.value == "gone" && facet.count == 0)
    );
}

#[tokio::test]
async fn task_list_scopes_to_project_and_workspace() {
    let fixture = Fixture::new().await;
    fixture
        .task(
            "app task",
            "app",
            TaskStatus::Todo,
            TaskPriority::None,
            None,
        )
        .await;
    fixture
        .task(
            "docs task",
            "docs",
            TaskStatus::Todo,
            TaskPriority::None,
            None,
        )
        .await;
    let database = Database::open(&fixture.path).await.unwrap();
    let other = database.create_workspace("Other").await.unwrap();
    let foreign = database
        .resolve_or_create_project(&other.id, "Foreign")
        .await
        .unwrap();
    fixture
        .store
        .create_task(
            &other.id,
            CreateTask {
                title: "foreign task".to_string(),
                description: String::new(),
                project: "Foreign".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();

    let mut app = query(TaskListSort::Title, TaskListDirection::Asc);
    app.project_id = Some(fixture.project_id("app").await);
    assert_eq!(fixture.titles(app).await, ["app task"]);
    assert_eq!(
        fixture
            .titles(query(TaskListSort::Title, TaskListDirection::Asc))
            .await,
        ["app task", "docs task"]
    );

    let mut cross = query(TaskListSort::Title, TaskListDirection::Asc);
    cross.project_id = Some(foreign.id);
    let error = fixture
        .store
        .task_list(&fixture.workspace, cross)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn task_list_text_uses_search_matching_with_filters() {
    let fixture = Fixture::new().await;
    let note = fixture
        .task(
            "Pager rotation",
            "app",
            TaskStatus::Todo,
            TaskPriority::None,
            None,
        )
        .await;
    fixture
        .task(
            "Pager docs",
            "docs",
            TaskStatus::Todo,
            TaskPriority::None,
            None,
        )
        .await;
    fixture
        .task(
            "Pager done",
            "app",
            TaskStatus::Done,
            TaskPriority::None,
            None,
        )
        .await;
    fixture
        .task(
            "Unrelated",
            "app",
            TaskStatus::Todo,
            TaskPriority::None,
            None,
        )
        .await;

    let mut text = query(TaskListSort::Title, TaskListDirection::Asc);
    text.text = "  pager ".to_string();
    assert_eq!(
        fixture.titles(text.clone()).await,
        ["Pager docs", "Pager done", "Pager rotation"]
    );
    text.project_id = Some(fixture.project_id("app").await);
    text.statuses = vec![TaskStatus::Todo];
    assert_eq!(fixture.titles(text.clone()).await, ["Pager rotation"]);

    let mut by_ref = query(TaskListSort::Title, TaskListDirection::Asc);
    by_ref.text = fixture.list(text).await.rows[0].display_ref.clone();
    let rows = fixture.list(by_ref).await.rows;
    assert!(rows.iter().any(|row| row.id == note.id));

    let mut too_long = query(TaskListSort::Title, TaskListDirection::Asc);
    too_long.text = "x".repeat(257);
    let error = fixture
        .store
        .task_list(&fixture.workspace, too_long)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Validation);
}

#[tokio::test]
async fn task_list_orders_creation_separately_from_edits() {
    let fixture = Fixture::new().await;
    let old = fixture
        .task(
            "old task",
            "app",
            TaskStatus::Todo,
            TaskPriority::None,
            None,
        )
        .await;
    fixture
        .set_times(&old, "2020-01-01T00:00:00Z", "2020-01-01T00:00:00Z")
        .await;
    let new = fixture
        .task(
            "new task",
            "app",
            TaskStatus::Todo,
            TaskPriority::None,
            None,
        )
        .await;
    fixture
        .set_times(&new, "2021-01-01T00:00:00Z", "2021-01-01T00:00:00Z")
        .await;
    fixture
        .store
        .edit_task(
            &fixture.workspace,
            &old.id,
            TaskEdit {
                title: Some("old task edited".to_string()),
                ..TaskEdit::default()
            },
        )
        .await
        .unwrap();

    let created = fixture
        .list(query(TaskListSort::Created, TaskListDirection::Desc))
        .await;
    assert_eq!(titles(&created), ["new task", "old task edited"]);
    assert_eq!(created.rows[1].created_at, "2020-01-01T00:00:00Z");
    assert!(created.rows[1].updated_at > created.rows[0].updated_at);
    assert_eq!(
        fixture
            .titles(query(TaskListSort::Updated, TaskListDirection::Desc))
            .await,
        ["old task edited", "new task"]
    );
}

#[tokio::test]
async fn task_list_limit_keeps_total() {
    let fixture = Fixture::new().await;
    for (index, title) in ["a", "b", "c"].into_iter().enumerate() {
        let task = fixture
            .task(title, "app", TaskStatus::Todo, TaskPriority::None, None)
            .await;
        let at = format!("2030-01-0{}T00:00:00Z", index + 1);
        fixture.set_times(&task, &at, &at).await;
    }
    let mut newest = query(TaskListSort::Created, TaskListDirection::Desc);
    newest.limit = Some(2);
    let list = fixture.list(newest).await;
    assert_eq!(titles(&list), ["c", "b"]);
    assert_eq!(list.total, 3);
}
