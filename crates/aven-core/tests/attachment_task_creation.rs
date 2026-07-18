use std::io::Cursor;

use aven_core::attachments::ImageOptimizationPolicy;
use aven_core::db::Database;
use aven_core::operations::{AttachmentAddInput, TaskAttachmentAddInput, TaskDraft};
use aven_core::query::{SortDirection, TaskFilters, TaskQueryMode, TaskSort};
use image::{DynamicImage, ImageFormat, RgbaImage};

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

#[tokio::test]
async fn creates_task_and_attachments_as_one_core_operation() {
    let temp = tempfile::tempdir().unwrap();
    let database = Database::open(&temp.path().join("aven.sqlite"))
        .await
        .unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let blob_dir = temp.path().join("blobs");

    let outcome = database
        .create_task_with_attachments(
            &workspace,
            &blob_dir,
            aven_core::attachments::LifecyclePolicy::default(),
            TaskDraft {
                title: "task with images".to_string(),
                description: String::new(),
                project: Some("default".to_string()),
                status: "todo".to_string(),
                priority: "none".to_string(),
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
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
