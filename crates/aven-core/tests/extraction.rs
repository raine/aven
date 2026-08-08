use aven_core::choices::TaskSource;
use aven_core::db::Database;
use aven_core::metadata::TaskMetadataInput;
use aven_core::operations::{TaskDraft, TaskUpdate};
use aven_core::sync::SyncSession;
use aven_core::undo::UndoContext;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use std::str::FromStr;

#[tokio::test]
async fn opens_migrates_and_mutates_through_owned_api() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();

    let workspaces = database.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    let workspace = &workspaces[0];
    let project = database
        .create_project(workspace, "Core")
        .await
        .unwrap()
        .project;

    let created = database
        .create_task(
            workspace,
            TaskDraft {
                metadata: Vec::new(),
                title: "core task".to_string(),
                description: String::new(),
                project: Some(project.key),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: TaskSource::Unknown,
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();
    let updated = database
        .update_task(
            workspace,
            &created.task.id,
            TaskUpdate {
                title: Some("updated through core".to_string()),
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    assert!(updated.changed);
    assert_eq!(created.task.source, TaskSource::Unknown);

    let export = database
        .export_data("2026-07-18T00:00:00Z".to_string())
        .await
        .unwrap();
    assert_eq!(
        export.schema_version,
        Database::latest_schema_version().expect("core has embedded migrations")
    );
    let task = export
        .tables
        .tasks
        .iter()
        .find(|task| task.id == created.task.id)
        .unwrap();
    assert_eq!(task.title, "updated through core");

    let title_change = export
        .tables
        .changes
        .iter()
        .find(|change| {
            change.entity_id == created.task.id.as_str()
                && change.field.as_deref() == Some("title")
                && change.op_type == "set_field"
        })
        .unwrap();
    let title_version = export
        .tables
        .field_versions
        .iter()
        .find(|version| version.entity_id == created.task.id.as_str() && version.field == "title")
        .unwrap();
    assert_eq!(title_version.version, title_change.change_id);
}

#[tokio::test]
async fn tui_task_mutation_uses_one_transaction_for_change_and_undo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aven.sqlite");
    let database = Database::open(&path).await.unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Core")
        .await
        .unwrap()
        .project;
    let created = database
        .create_task(
            &workspace,
            TaskDraft {
                metadata: Vec::new(),
                title: "atomic task".to_string(),
                description: String::new(),
                project: Some(project.key),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: TaskSource::Unknown,
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();

    let report = database
        .mutate_tasks(
            &workspace,
            vec![(
                created.task.id.clone(),
                TaskUpdate {
                    status: Some("todo".to_string()),
                    ..TaskUpdate::default()
                },
            )],
            UndoContext::tui("status atomic task"),
        )
        .await
        .unwrap();
    assert_eq!(report.changed_count(), 1);
    assert_eq!(report.outcomes[0].before.status, "inbox");
    assert_eq!(report.outcomes[0].after.status, "todo");

    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
    )
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_undo_insert BEFORE INSERT ON tui_undo_entries
         BEGIN SELECT RAISE(FAIL, 'injected undo failure'); END",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    drop(conn);

    let error = database
        .mutate_tasks(
            &workspace,
            vec![(
                created.task.id.clone(),
                TaskUpdate {
                    status: Some("active".to_string()),
                    ..TaskUpdate::default()
                },
            )],
            UndoContext::tui("status atomic task"),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected undo failure"));
    let persisted = database
        .task_field_value(&workspace.id, &created.task.id, "status")
        .await
        .unwrap();
    assert_eq!(persisted, "todo");
}

#[tokio::test]
async fn task_metadata_undo_tracks_field_identity_across_renames() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Core")
        .await
        .unwrap()
        .project;
    let created = database
        .create_task(
            &workspace,
            TaskDraft {
                metadata: Vec::new(),
                title: "metadata undo".to_string(),
                description: String::new(),
                project: Some(project.key),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: TaskSource::Unknown,
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();

    database
        .mutate_tasks(
            &workspace,
            vec![(
                created.task.id.clone(),
                TaskUpdate {
                    set_metadata: vec![TaskMetadataInput {
                        key: "legacy-id".to_string(),
                        value: String::new(),
                    }],
                    ..TaskUpdate::default()
                },
            )],
            UndoContext::tui("set metadata"),
        )
        .await
        .unwrap();
    database
        .rename_metadata_field(&workspace, "legacy-id", "external-id")
        .await
        .unwrap();
    database.apply_latest_tui_undo(&workspace.id).await.unwrap();
    assert!(
        database
            .task_metadata(&workspace.id, &created.task.id)
            .await
            .unwrap()
            .is_empty()
    );

    database
        .update_task(
            &workspace,
            &created.task.id,
            TaskUpdate {
                set_metadata: vec![TaskMetadataInput {
                    key: "external-id".to_string(),
                    value: String::new(),
                }],
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    database
        .mutate_tasks(
            &workspace,
            vec![(
                created.task.id.clone(),
                TaskUpdate {
                    remove_metadata: vec!["external-id".to_string()],
                    ..TaskUpdate::default()
                },
            )],
            UndoContext::tui("remove metadata"),
        )
        .await
        .unwrap();
    database
        .rename_metadata_field(&workspace, "external-id", "final-id")
        .await
        .unwrap();
    database.apply_latest_tui_undo(&workspace.id).await.unwrap();
    let metadata = database
        .task_metadata(&workspace.id, &created.task.id)
        .await
        .unwrap();
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].key, "final-id");
    assert_eq!(metadata[0].value, "");
}

#[tokio::test]
async fn label_and_project_creation_roll_back_when_change_logging_fails() {
    for (entity, create) in [("label", "Atomic Label"), ("project", "Atomic Project")] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("aven.sqlite");
        let database = Database::open(&path).await.unwrap();
        let workspace = database.list_workspaces().await.unwrap().remove(0);
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_change_insert BEFORE INSERT ON changes
             BEGIN SELECT RAISE(FAIL, 'injected change failure'); END",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        drop(conn);

        let error = if entity == "label" {
            database
                .create_label(&workspace, create)
                .await
                .err()
                .expect("label creation must fail")
        } else {
            database
                .create_project(&workspace, create)
                .await
                .err()
                .expect("project creation must fail")
        };
        assert!(error.to_string().contains("injected change failure"));

        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
        )
        .await
        .unwrap();
        let count: i64 = if entity == "label" {
            sqlx::query_scalar("SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?")
                .bind(&workspace.id)
                .bind("atomic-label")
                .fetch_one(&mut conn)
                .await
                .unwrap()
        } else {
            sqlx::query_scalar("SELECT count(*) FROM projects WHERE workspace_id = ? AND key = ?")
                .bind(&workspace.id)
                .bind("atomic-project")
                .fetch_one(&mut conn)
                .await
                .unwrap()
        };
        assert_eq!(count, 0, "{entity} row must roll back");
    }
}

#[tokio::test]
async fn workspace_mutations_roll_back_when_change_logging_fails() {
    for rename in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("aven.sqlite");
        let database = Database::open(&path).await.unwrap();
        if rename {
            database.create_workspace("Before").await.unwrap();
        }
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_change_insert BEFORE INSERT ON changes
             BEGIN SELECT RAISE(FAIL, 'injected change failure'); END",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        drop(conn);

        let error = if rename {
            database
                .rename_workspace("before", "After")
                .await
                .unwrap_err()
        } else {
            database.create_workspace("Rejected").await.unwrap_err()
        };
        assert!(error.to_string().contains("injected change failure"));

        let workspaces = database.list_workspaces().await.unwrap();
        if rename {
            let workspace = workspaces
                .iter()
                .find(|workspace| workspace.key == "before")
                .expect("original workspace remains");
            assert_eq!(workspace.name, "Before");
            assert!(workspaces.iter().all(|workspace| workspace.key != "after"));
        } else {
            assert!(
                workspaces
                    .iter()
                    .all(|workspace| workspace.key != "rejected")
            );
        }
    }
}

#[tokio::test]
async fn resolving_a_missing_project_rolls_back_when_change_logging_fails() {
    for stored_value in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("aven.sqlite");
        let database = Database::open(&path).await.unwrap();
        let workspace = database.list_workspaces().await.unwrap().remove(0);
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_change_insert BEFORE INSERT ON changes
             BEGIN SELECT RAISE(FAIL, 'injected change failure'); END",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        drop(conn);

        let error = if stored_value {
            database
                .resolve_project_for_stored_value(&workspace.id, "Rejected Project")
                .await
                .unwrap_err()
        } else {
            database
                .resolve_or_create_project(&workspace.id, "Rejected Project")
                .await
                .unwrap_err()
        };
        assert!(error.to_string().contains("injected change failure"));

        let projects = database.list_projects(&workspace.id, None).await.unwrap();
        assert!(
            projects
                .iter()
                .all(|project| project.key != "rejected-project")
        );
    }
}

#[tokio::test]
async fn database_task_field_rolls_back_when_change_logging_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aven.sqlite");
    let database = Database::open(&path).await.unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Atomic")
        .await
        .unwrap()
        .project;
    let created = database
        .create_task(
            &workspace,
            TaskDraft {
                metadata: Vec::new(),
                title: "before".to_string(),
                description: String::new(),
                project: Some(project.key),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: TaskSource::Unknown,
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
    )
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_change_insert BEFORE INSERT ON changes
         BEGIN SELECT RAISE(FAIL, 'injected change failure'); END",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    drop(conn);

    let error = database
        .set_task_field(&workspace, &created.task.id, "title", "after")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected change failure"));
    let persisted = database
        .task_field_value(&workspace.id, &created.task.id, "title")
        .await
        .unwrap();
    assert_eq!(persisted, "before");
}

#[tokio::test]
async fn task_delete_and_restore_roll_back_when_change_logging_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aven.sqlite");
    let database = Database::open(&path).await.unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Atomic")
        .await
        .unwrap()
        .project;
    let created = database
        .create_task(
            &workspace,
            TaskDraft {
                metadata: Vec::new(),
                title: "atomic deletion".to_string(),
                description: String::new(),
                project: Some(project.key),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: TaskSource::Unknown,
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();

    for (deleted, expected) in [(true, 0_i64), (false, 1_i64)] {
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_change_insert BEFORE INSERT ON changes
             BEGIN SELECT RAISE(FAIL, 'injected change failure'); END",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        drop(conn);

        let error = database
            .set_task_deleted(&workspace, &created.task.id, deleted)
            .await
            .err()
            .expect("task deletion state change must fail");
        assert!(error.to_string().contains("injected change failure"));

        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str(&path.display().to_string()).unwrap(),
        )
        .await
        .unwrap();
        let persisted: i64 = sqlx::query_scalar("SELECT deleted FROM tasks WHERE id = ?")
            .bind(&created.task.id)
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(persisted, expected);
        sqlx::query("DROP TRIGGER reject_change_insert")
            .execute(&mut conn)
            .await
            .unwrap();
        drop(conn);

        if deleted {
            database
                .set_task_deleted(&workspace, &created.task.id, true)
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn invalid_sync_initialization_records_attempt_and_error() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();

    let error =
        match SyncSession::start(database.clone(), "not a URL".to_string(), None, None).await {
            Ok(_) => panic!("invalid URL must fail sync initialization"),
            Err(error) => error,
        };

    assert_eq!(error.to_string(), "invalid sync server URL");
    let status = database.sync_persistence_status().await.unwrap();
    assert!(status.last_attempt.is_some());
    assert_eq!(
        status.last_error.as_deref(),
        Some("invalid sync server URL")
    );
}
