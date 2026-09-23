use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageFormat, RgbaImage};

use crate::choices::TaskSource;
use crate::operations::{TaskDraft, TaskUpdate};

use super::*;

pub(super) fn package_context() -> LocalSharedStatePackageContext {
    LocalSharedStatePackageContext {
        vault_id: [0x31; 32],
        generation_id: [0x42; 32],
    }
}

pub(super) fn package_key() -> LocalSharedStatePackageKey {
    LocalSharedStatePackageKey::new([0x53; 32])
}

pub(super) fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::new(width, height))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

pub(super) async fn source_with_history() -> (tempfile::TempDir, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(&dir.path().join("source.sqlite"))
        .await
        .unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let task = database
        .create_task(
            &workspace,
            TaskDraft {
                title: "captured title".into(),
                description: "captured description".into(),
                project: Some("app".into()),
                status: "todo".into(),
                priority: "none".into(),
                source: TaskSource::Cli,
                labels: vec![],
                metadata: vec![],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap()
        .task;
    database
        .update_task(
            &workspace,
            &task.id,
            TaskUpdate {
                description: Some("history retained".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    (dir, database, task.id.to_string())
}

pub(super) async fn add_selected_images(
    dir: &Path,
    database: &Database,
    task_id: &str,
) -> (Vec<u8>, Vec<u8>, String) {
    let current = png_bytes(2, 1);
    let extra = png_bytes(1, 2);
    let current_hash = crate::attachments::storage::sha256_hex(&current);
    let extra_hash = crate::attachments::storage::sha256_hex(&extra);
    for (hash, bytes) in [(&current_hash, &current), (&extra_hash, &extra)] {
        let path = crate::attachments::storage::object_path(dir, hash).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut conn = database.acquire_writer().await.unwrap();
    crate::attachments::storage::upsert_inventory_available(
        &mut conn,
        &current_hash,
        i64::try_from(current.len()).unwrap(),
        "image/png",
    )
    .await
    .unwrap();
    crate::attachments::storage::upsert_inventory_available(
        &mut conn,
        &extra_hash,
        i64::try_from(extra.len()).unwrap(),
        "image/png",
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO blob_inventory(
             sha256, byte_size, media_type, available, first_seen_at, last_verified_at
         ) VALUES (?, 9, 'image/png', 0, '2026-09-22T01:00:00Z', NULL)",
    )
    .bind("cd".repeat(32))
    .execute(&mut *conn)
    .await
    .unwrap();
    let workspace_id: String = sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO task_attachments(
             workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
             filename, alt_text, width, height, created_at, created_by_change_id,
             deleted, deleted_at, deleted_by_change_id
         ) VALUES (?, ?, ?, ?, ?, 'image/png', 'capture.png', NULL, 2, 1,
             '2026-09-22T01:00:00Z', NULL, 0, NULL, NULL)",
    )
    .bind(workspace_id)
    .bind(crate::ids::new_id())
    .bind(task_id)
    .bind(&current_hash)
    .bind(i64::try_from(current.len()).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    (current, extra, extra_hash)
}
