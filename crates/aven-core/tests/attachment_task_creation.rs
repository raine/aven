use std::io::Cursor;
use std::path::{Path, PathBuf};

use aven_core::attachments::{ImageOptimizationPolicy, LifecyclePolicy};
use aven_core::db::Database;
use aven_core::operations::{AttachmentAddInput, TaskAttachmentAddInput, TaskDraft, TaskUpdate};
use aven_core::query::{SortDirection, TaskFilters, TaskQueryMode, TaskSort};
use aven_core::undo::UndoContext;
use image::{DynamicImage, ImageFormat, RgbaImage};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

fn png_bytes(width: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::new(width, 1))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn attachment(attachment_id: &str, bytes: Vec<u8>) -> TaskAttachmentAddInput {
    TaskAttachmentAddInput {
        attachment_id: attachment_id.to_string(),
        input: AttachmentAddInput {
            filename: Some(format!("{attachment_id}.png")),
            alt_text: None,
            declared_media_type: Some("image/png".to_string()),
            bytes,
            optimization_policy: ImageOptimizationPolicy::Preserve,
            dedupe_existing: false,
        },
    }
}

fn draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: Some("default".to_string()),
        status: "todo".to_string(),
        priority: "none".to_string(),
        labels: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

async fn setup() -> (tempfile::TempDir, Database, SqlitePool) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("aven.sqlite");
    let database = Database::open(&path).await.unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    (temp, database, pool)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn object_path(blob_dir: &Path, sha256: &str) -> PathBuf {
    blob_dir.join("objects").join("sha256").join(sha256)
}

async fn visible_counts(pool: &SqlitePool) -> (i64, i64, i64, i64) {
    let row = sqlx::query(
        "SELECT
           (SELECT count(*) FROM tasks) AS tasks,
           (SELECT count(*) FROM task_attachments) AS attachments,
           (SELECT count(*) FROM changes) AS changes,
           (SELECT count(*) FROM field_versions) AS versions",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    (
        row.get("tasks"),
        row.get("attachments"),
        row.get("changes"),
        row.get("versions"),
    )
}

#[tokio::test]
async fn creates_task_and_attachments_as_one_core_operation() {
    let (temp, database, _) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let blob_dir = temp.path().join("blobs");

    let outcome = database
        .create_task_with_attachments(
            &workspace,
            &blob_dir,
            LifecyclePolicy::default(),
            draft("task with images"),
            vec![
                attachment("ATTACHMENT000001", png_bytes(1)),
                attachment("ATTACHMENT000002", png_bytes(2)),
            ],
        )
        .await
        .unwrap();

    assert_eq!(outcome.attachment_change_ids.len(), 2);
    let tasks = database
        .list_task_items(
            &workspace.id,
            TaskFilters::default(),
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.id, outcome.task.id);
    assert_eq!(tasks[0].attachments.len(), 2);
    assert!(tasks[0].attachments.iter().all(|item| item.has_blob));
}

#[tokio::test]
async fn duplicate_content_preserves_input_order_and_reserves_one_object() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let blob_dir = temp.path().join("blobs");
    let bytes = png_bytes(2);
    let hash = sha256_hex(&bytes);
    sqlx::query("CREATE TABLE reservation_audit(sha256 TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER audit_local_reservation
         AFTER INSERT ON blob_upload_reservations
         WHEN NEW.workspace_id = '__local__'
         BEGIN INSERT INTO reservation_audit(sha256) VALUES (NEW.sha256); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    database
        .create_task_with_attachments(
            &workspace,
            &blob_dir,
            LifecyclePolicy::default(),
            draft("duplicates"),
            vec![
                attachment("ATTACHMENT000002", bytes.clone()),
                attachment("ATTACHMENT000001", bytes),
            ],
        )
        .await
        .unwrap();

    let ids: Vec<String> =
        sqlx::query_scalar("SELECT attachment_id FROM task_attachments ORDER BY created_at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ids, ["ATTACHMENT000002", "ATTACHMENT000001"]);
    let reservations: Vec<String> = sqlx::query_scalar("SELECT sha256 FROM reservation_audit")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(reservations, [hash.clone()]);
    assert!(object_path(&blob_dir, &hash).exists());
}

#[tokio::test]
async fn capacity_is_reserved_for_each_distinct_object() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let blob_dir = temp.path().join("blobs");
    let first = png_bytes(2);
    let second = png_bytes(3);
    let mut expected = vec![sha256_hex(&first), sha256_hex(&second)];
    expected.sort();
    sqlx::query("CREATE TABLE reservation_audit(sha256 TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER audit_local_reservation
         AFTER INSERT ON blob_upload_reservations
         WHEN NEW.workspace_id = '__local__'
         BEGIN INSERT INTO reservation_audit(sha256) VALUES (NEW.sha256); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    database
        .create_task_with_attachments(
            &workspace,
            &blob_dir,
            LifecyclePolicy::default(),
            draft("capacity"),
            vec![
                attachment("ATTACHMENT000001", first),
                attachment("ATTACHMENT000002", second),
            ],
        )
        .await
        .unwrap();

    let reservations: Vec<String> =
        sqlx::query_scalar("SELECT sha256 FROM reservation_audit ORDER BY sha256")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(reservations, expected);
}

#[tokio::test]
async fn database_failure_rolls_back_and_retry_reuses_attachment_id() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let blob_dir = temp.path().join("blobs");
    let bytes = png_bytes(2);
    let hash = sha256_hex(&bytes);
    sqlx::query(
        "CREATE TRIGGER fail_attachment_insert BEFORE INSERT ON task_attachments
         BEGIN SELECT RAISE(FAIL, 'injected attachment insert failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let inputs = vec![attachment("ATTACHMENT000001", bytes.clone())];

    assert!(
        database
            .create_task_with_attachments(
                &workspace,
                &blob_dir,
                LifecyclePolicy::default(),
                draft("retry"),
                inputs.clone(),
            )
            .await
            .is_err()
    );
    assert_eq!(visible_counts(&pool).await, (0, 0, 0, 0));
    assert!(!object_path(&blob_dir, &hash).exists());

    sqlx::query("DROP TRIGGER fail_attachment_insert")
        .execute(&pool)
        .await
        .unwrap();
    database
        .create_task_with_attachments(
            &workspace,
            &blob_dir,
            LifecyclePolicy::default(),
            draft("retry"),
            inputs,
        )
        .await
        .unwrap();
    let stored_id: String = sqlx::query_scalar("SELECT attachment_id FROM task_attachments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored_id, "ATTACHMENT000001");
}

#[tokio::test]
async fn commit_failure_rolls_back_database_and_staged_object() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let blob_dir = temp.path().join("blobs");
    let bytes = png_bytes(2);
    let hash = sha256_hex(&bytes);
    sqlx::query("CREATE TABLE commit_parent(id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE commit_child(
           parent_id INTEGER REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_atomic_commit AFTER INSERT ON task_attachments
         BEGIN INSERT INTO commit_child(parent_id) VALUES (1); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error = database
        .create_task_with_attachments(
            &workspace,
            &blob_dir,
            LifecyclePolicy::default(),
            draft("commit failure"),
            vec![attachment("ATTACHMENT000001", bytes)],
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("FOREIGN KEY constraint failed"));
    assert_eq!(visible_counts(&pool).await, (0, 0, 0, 0));
    assert!(!object_path(&blob_dir, &hash).exists());
}

#[tokio::test]
async fn rollback_preserves_preexisting_shared_object() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let blob_dir = temp.path().join("blobs");
    let bytes = png_bytes(2);
    let hash = sha256_hex(&bytes);
    let path = object_path(&blob_dir, &hash);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_task BEFORE INSERT ON tasks
         BEGIN SELECT RAISE(FAIL, 'injected task failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(
        database
            .create_task_with_attachments(
                &workspace,
                &blob_dir,
                LifecyclePolicy::default(),
                draft("shared"),
                vec![attachment("ATTACHMENT000001", bytes)],
            )
            .await
            .is_err()
    );

    assert!(path.exists());
    assert_eq!(visible_counts(&pool).await, (0, 0, 0, 0));
}

#[tokio::test]
async fn validation_and_staging_failures_leave_no_visible_state() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);

    assert!(
        database
            .create_task_with_attachments(
                &workspace,
                &temp.path().join("blobs"),
                LifecyclePolicy::default(),
                draft("invalid"),
                vec![attachment("ATTACHMENT000001", b"not an image".to_vec())],
            )
            .await
            .is_err()
    );
    assert_eq!(visible_counts(&pool).await, (0, 0, 0, 0));

    let blocked = temp.path().join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    assert!(
        database
            .create_task_with_attachments(
                &workspace,
                &blocked,
                LifecyclePolicy::default(),
                draft("staging"),
                vec![attachment("ATTACHMENT000001", png_bytes(1))],
            )
            .await
            .is_err()
    );
    assert_eq!(visible_counts(&pool).await, (0, 0, 0, 0));
}

#[tokio::test]
async fn task_deletion_and_restore_reconcile_attachment_liveness() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let outcome = database
        .create_task_with_attachments(
            &workspace,
            &temp.path().join("blobs"),
            LifecyclePolicy::default(),
            draft("attachment liveness"),
            vec![attachment("ATTACHMENT000001", png_bytes(1))],
        )
        .await
        .unwrap();

    database
        .set_task_deleted(&workspace, &outcome.task.id, true)
        .await
        .unwrap();
    let deleted_at: Option<String> =
        sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deleted_at.is_some());

    database
        .set_task_deleted(&workspace, &outcome.task.id, false)
        .await
        .unwrap();
    let restored_at: Option<String> =
        sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(restored_at, None);

    database
        .mutate_tasks(
            &workspace,
            vec![(
                outcome.task.id,
                TaskUpdate {
                    deleted: Some(true),
                    ..TaskUpdate::default()
                },
            )],
            UndoContext::tui("delete attachment task"),
        )
        .await
        .unwrap();
    let tui_deleted_at: Option<String> =
        sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(tui_deleted_at.is_some());
}

#[tokio::test]
async fn liveness_failure_rolls_back_task_deletion_and_undo() {
    let (temp, database, pool) = setup().await;
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let outcome = database
        .create_task_with_attachments(
            &workspace,
            &temp.path().join("blobs"),
            LifecyclePolicy::default(),
            draft("liveness rollback"),
            vec![attachment("ATTACHMENT000001", png_bytes(1))],
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_liveness_update BEFORE UPDATE OF unreferenced_at ON blob_lifecycle
         BEGIN SELECT RAISE(FAIL, 'injected liveness failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error = database
        .mutate_tasks(
            &workspace,
            vec![(
                outcome.task.id.clone(),
                TaskUpdate {
                    deleted: Some(true),
                    ..TaskUpdate::default()
                },
            )],
            UndoContext::tui("delete attachment task"),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected liveness failure"));
    let deleted: i64 = sqlx::query_scalar("SELECT deleted FROM tasks WHERE id = ?")
        .bind(&outcome.task.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(deleted, 0);
    let unreferenced_at: Option<String> =
        sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unreferenced_at, None);
    let undo_entries: i64 = sqlx::query_scalar("SELECT count(*) FROM tui_undo_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(undo_entries, 0);
}
