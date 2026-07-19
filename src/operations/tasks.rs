use std::collections::BTreeMap;
use std::path::Path;

use crate::ids::WorkspaceId;
use anyhow::{Result, bail};
use sqlx::SqliteConnection;
use tracing::{info, warn};

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{begin_immediate, set_field_version};
use crate::ids::{TaskId, new_id, now};
use crate::labels::resolve_labels_in_workspace;
use crate::mutation::{set_task_field, set_task_project};
use crate::projects::resolve_project_for_add_in_workspace;
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::workspaces::Workspace;

#[cfg(test)]
tokio::task_local! {
    static ATOMIC_CREATE_FAILURE_POINT: &'static str;
}

#[cfg(test)]
fn inject_atomic_create_failure(point: &'static str) -> Result<()> {
    if ATOMIC_CREATE_FAILURE_POINT
        .try_with(|configured| *configured == point)
        .unwrap_or(false)
    {
        bail!("injected atomic task create failure at {point}");
    }
    Ok(())
}

#[cfg(not(test))]
fn inject_atomic_create_failure(_point: &'static str) -> Result<()> {
    Ok(())
}

pub(crate) struct TaskDraft {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project: Option<String>,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) available_at: Option<String>,
    pub(crate) due_on: Option<String>,
    pub(crate) is_epic: bool,
}

#[derive(Debug)]
pub(crate) struct TaskOutcome {
    pub(crate) task: Task,
    pub(crate) create_change_id: Option<String>,
    pub(crate) attachment_change_ids: Vec<String>,
}

struct InsertedTask {
    id: TaskId,
    change_id: String,
    project_key: String,
    label_count: usize,
}

#[derive(Default)]
pub(crate) struct TaskUpdate {
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) available_at: Option<Option<String>>,
    pub(crate) due_on: Option<Option<String>>,
    pub(crate) is_epic: Option<bool>,
    pub(crate) add_labels: Vec<String>,
    pub(crate) remove_labels: Vec<String>,
}

pub(crate) struct TaskUpdateOutcome {
    pub(crate) task: Task,
    pub(crate) changed: bool,
}

pub(crate) struct NoteDeleteOutcome {
    #[allow(dead_code)]
    pub(crate) task_id: TaskId,
    #[allow(dead_code)]
    pub(crate) note_id: String,
    pub(crate) changed: bool,
}

pub(crate) struct NoteOutcome {
    #[allow(dead_code)]
    pub(crate) task_id: TaskId,
    pub(crate) note_id: String,
}
pub(crate) async fn create_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
) -> Result<TaskOutcome> {
    validate_task_draft(&draft)?;
    let mut tx = begin_immediate(conn).await?;
    let inserted = insert_task(&mut tx, workspace, draft).await?;
    tx.commit().await?;
    info!(
        task_id = %inserted.id,
        project_key = %inserted.project_key,
        label_count = inserted.label_count,
        "task created"
    );
    Ok(TaskOutcome {
        task: get_task_in_workspace(conn, workspace, &inserted.id).await?,
        create_change_id: Some(inserted.change_id),
        attachment_change_ids: Vec::new(),
    })
}

pub(crate) async fn create_task_with_attachments(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    blob_dir: &Path,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
    draft: TaskDraft,
    attachments: Vec<super::attachments::TaskAttachmentAddInput>,
) -> Result<TaskOutcome> {
    validate_task_draft(&draft)?;
    let mut prepared = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        prepared.push(super::attachments::prepare_task_attachment(attachment).await?);
    }

    let mut unique = BTreeMap::new();
    for attachment in &prepared {
        unique
            .entry(attachment.sha256.clone())
            .or_insert_with(|| attachment.clone());
    }

    let mut capacity_reservations = Vec::new();
    for attachment in unique.values() {
        let available: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM blob_inventory WHERE sha256 = ? AND available = 1)",
        )
        .bind(&attachment.sha256)
        .fetch_one(&mut *conn)
        .await?;
        if available {
            continue;
        }
        match crate::attachments::lifecycle::ensure_local_capacity(
            conn,
            blob_dir,
            &attachment.sha256,
            attachment.byte_size,
            lifecycle_policy,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await
        {
            Ok(Some(reservation_id)) => capacity_reservations.push(reservation_id),
            Ok(None) => {}
            Err(error) => {
                for reservation_id in capacity_reservations {
                    let _ =
                        crate::attachments::lifecycle::release_reservation(conn, &reservation_id)
                            .await;
                }
                return Err(error);
            }
        }
    }

    let mut staging_leases = Vec::with_capacity(unique.len());
    for attachment in unique.values() {
        match crate::attachments::lifecycle::acquire_lease(
            conn,
            &attachment.sha256,
            "staging",
            &crate::attachments::lifecycle::SystemClock,
        )
        .await
        {
            Ok(lease_id) => staging_leases.push(lease_id),
            Err(error) => {
                for lease_id in staging_leases {
                    let _ = crate::attachments::lifecycle::release_lease(conn, &lease_id).await;
                }
                for reservation_id in capacity_reservations {
                    let _ =
                        crate::attachments::lifecycle::release_reservation(conn, &reservation_id)
                            .await;
                }
                return Err(error);
            }
        }
    }

    let mut created_hashes = Vec::new();
    for attachment in unique.values() {
        match crate::attachments::storage::stage_blob(
            blob_dir,
            &attachment.sha256,
            &attachment.bytes,
        )
        .await
        {
            Ok(staged) => {
                if staged.byte_size != attachment.byte_size {
                    for lease_id in &staging_leases {
                        let _ = crate::attachments::lifecycle::release_lease(conn, lease_id).await;
                    }
                    cleanup_created_objects(conn, blob_dir, &created_hashes).await;
                    for reservation_id in &capacity_reservations {
                        let _ = crate::attachments::lifecycle::release_reservation(
                            conn,
                            reservation_id,
                        )
                        .await;
                    }
                    bail!("error attachment-staged-size-mismatch");
                }
                if staged.created {
                    created_hashes.push(staged.sha256);
                }
            }
            Err(error) => {
                for lease_id in &staging_leases {
                    let _ = crate::attachments::lifecycle::release_lease(conn, lease_id).await;
                }
                cleanup_created_objects(conn, blob_dir, &created_hashes).await;
                for reservation_id in capacity_reservations {
                    let _ =
                        crate::attachments::lifecycle::release_reservation(conn, &reservation_id)
                            .await;
                }
                return Err(error);
            }
        }
    }

    let database_result = async {
        let mut tx = begin_immediate(conn).await?;
        for attachment in unique.values() {
            crate::attachments::storage::upsert_inventory_available(
                &mut tx,
                &attachment.sha256,
                attachment.byte_size,
                &attachment.facts.media_type,
            )
            .await?;
        }
        let inserted = insert_task(&mut tx, workspace, draft).await?;
        let mut attachment_change_ids = Vec::with_capacity(prepared.len());
        let attachment_base = chrono::DateTime::parse_from_rfc3339(&now())?.to_utc();
        for (index, attachment) in prepared.iter().enumerate() {
            let offset = i64::try_from(index)?;
            let created_at = (attachment_base + chrono::TimeDelta::microseconds(offset))
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
            attachment_change_ids.push(
                super::attachments::insert_prepared_attachment(
                    &mut tx,
                    workspace,
                    &inserted.id,
                    attachment,
                    &created_at,
                )
                .await?,
            );
        }
        let task = get_task_in_workspace(&mut tx, workspace, &inserted.id).await?;
        inject_atomic_create_failure("commit")?;
        tx.commit().await?;
        Ok::<_, anyhow::Error>((inserted, attachment_change_ids, task))
    }
    .await;

    let (inserted, attachment_change_ids, task) = match database_result {
        Ok(value) => value,
        Err(error) => {
            for lease_id in staging_leases {
                let _ = crate::attachments::lifecycle::release_lease(conn, &lease_id).await;
            }
            cleanup_created_objects(conn, blob_dir, &created_hashes).await;
            for reservation_id in capacity_reservations {
                let _ =
                    crate::attachments::lifecycle::release_reservation(conn, &reservation_id).await;
            }
            return Err(error);
        }
    };
    for lease_id in staging_leases {
        if let Err(error) = crate::attachments::lifecycle::release_lease(conn, &lease_id).await {
            warn!(%error, "failed to release attachment staging lease");
        }
    }
    for reservation_id in capacity_reservations {
        if let Err(error) =
            crate::attachments::lifecycle::release_reservation(conn, &reservation_id).await
        {
            warn!(%error, "failed to release attachment capacity reservation");
        }
    }
    if let Err(error) = crate::attachments::lifecycle::reconcile_liveness(
        conn,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await
    {
        warn!(%error, "failed to reconcile attachment liveness");
    }
    info!(
        task_id = %inserted.id,
        project_key = %inserted.project_key,
        label_count = inserted.label_count,
        attachment_count = prepared.len(),
        "task created"
    );
    Ok(TaskOutcome {
        task,
        create_change_id: Some(inserted.change_id),
        attachment_change_ids,
    })
}

fn validate_task_draft(draft: &TaskDraft) -> Result<()> {
    TaskStatus::parse(&draft.status)?;
    TaskPriority::parse(&draft.priority)?;
    if let Some(available_at) = draft.available_at.as_deref() {
        crate::time_input::validate_available_at_value(available_at)?;
    }
    if let Some(due_on) = draft.due_on.as_deref() {
        crate::time_input::validate_due_on_value(due_on)?;
    }
    Ok(())
}

async fn insert_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
) -> Result<InsertedTask> {
    let status = TaskStatus::parse(&draft.status)?;
    let priority = TaskPriority::parse(&draft.priority)?;
    let id = TaskId::new();
    let available_at = draft.available_at.as_deref().unwrap_or("");
    let due_on = draft.due_on.as_deref().unwrap_or("");
    let ts = now();
    let project =
        resolve_project_for_add_in_workspace(conn, &workspace.id, draft.project.as_deref()).await?;
    let labels = resolve_labels_in_workspace(conn, &workspace.id, &draft.labels).await?;
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at, available_at, due_on, is_epic)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(&id)
    .bind(&draft.title)
    .bind(&draft.description)
    .bind(&project.id)
    .bind(status.as_str())
    .bind(priority.as_str())
    .bind(&ts)
    .bind(&ts)
    .bind(&ts)
    .bind(available_at)
    .bind(due_on)
    .bind(i64::from(draft.is_epic))
    .execute(&mut *conn)
    .await?;
    for label in &labels {
        sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(&id)
        .bind(label)
        .execute(&mut *conn)
        .await?;
    }
    let change_id = append_change(
        conn,
        ChangeEntity::Task,
        &id,
        None,
        op_type::CREATE_TASK,
        ChangePayload::workspace(workspace)
            .set("title", draft.title)
            .set("description", draft.description)
            .set("project_id", project.id.clone())
            .set("project_key", project.key.clone())
            .set("project_name", project.name.clone())
            .set("project_prefix", project.prefix.clone())
            .set("status", status.as_str())
            .set("priority", priority.as_str())
            .set("available_at", available_at)
            .set("due_on", due_on)
            .set("is_epic", if draft.is_epic { "1" } else { "0" })
            .set("labels", &labels)
            .set("created_at", ts),
    )
    .await?;
    for field in TaskField::VERSIONED {
        set_field_version(conn, &id, field.as_str(), &change_id).await?;
    }
    Ok(InsertedTask {
        id,
        change_id,
        project_key: project.key,
        label_count: labels.len(),
    })
}

async fn cleanup_created_objects(conn: &mut SqliteConnection, blob_dir: &Path, hashes: &[String]) {
    for sha256 in hashes {
        crate::attachments::storage::remove_staged_blob_if_unreferenced(conn, blob_dir, sha256)
            .await;
    }
}

pub(crate) async fn update_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    update: TaskUpdate,
) -> Result<TaskUpdateOutcome> {
    if let Some(status) = update.status.as_deref() {
        TaskStatus::parse(status)?;
    }
    if let Some(priority) = update.priority.as_deref() {
        TaskPriority::parse(priority)?;
    }
    if let Some(Some(available_at)) = update.available_at.as_ref() {
        crate::time_input::validate_available_at_value(available_at)?;
    }
    if let Some(Some(due_on)) = update.due_on.as_ref() {
        crate::time_input::validate_due_on_value(due_on)?;
    }
    let mut changed = false;
    let mut tx = begin_immediate(conn).await?;
    if let Some(title) = update.title {
        changed |= update_task_field(&mut tx, workspace, task_id, "title", &title).await?;
    }
    if let Some(description) = update.description {
        changed |=
            update_task_field(&mut tx, workspace, task_id, "description", &description).await?;
    }
    if let Some(project) = update.project {
        let project =
            resolve_project_for_add_in_workspace(&mut tx, &workspace.id, Some(&project)).await?;
        changed |= set_task_project(&mut tx, workspace, task_id, &project).await?;
    }
    if let Some(status) = update.status {
        changed |= update_task_field(&mut tx, workspace, task_id, "status", &status).await?;
    }
    if let Some(priority) = update.priority {
        changed |= update_task_field(&mut tx, workspace, task_id, "priority", &priority).await?;
    }
    if let Some(available_at) = update.available_at {
        changed |= update_task_field(
            &mut tx,
            workspace,
            task_id,
            "available_at",
            available_at.as_deref().unwrap_or(""),
        )
        .await?;
    }
    if let Some(due_on) = update.due_on {
        changed |= update_task_field(
            &mut tx,
            workspace,
            task_id,
            "due_on",
            due_on.as_deref().unwrap_or(""),
        )
        .await?;
    }
    if let Some(is_epic) = update.is_epic {
        if !is_epic {
            let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
            if super::epics::task_has_epic_children(&mut tx, &task.workspace_id, task_id).await? {
                bail!("error epic-has-children task_id={task_id}");
            }
        }
        changed |= update_task_field(
            &mut tx,
            workspace,
            task_id,
            "is_epic",
            if is_epic { "1" } else { "0" },
        )
        .await?;
    }
    if update_task_labels_in_workspace(
        &mut tx,
        &workspace.id,
        task_id,
        &update.add_labels,
        &update.remove_labels,
    )
    .await?
    {
        changed = true;
    }
    tx.commit().await?;
    info!(task_id = %task_id, changed, "task updated");
    Ok(TaskUpdateOutcome {
        task: get_task_in_workspace(conn, workspace, task_id).await?,
        changed,
    })
}

pub(crate) async fn update_task_field(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    value: &str,
) -> Result<bool> {
    set_task_field(conn, workspace, task_id, field, value).await
}

pub(crate) async fn update_task_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    add_labels: &[String],
    remove_labels: &[String],
) -> Result<bool> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let mut changed = false;
    for label in resolve_labels_in_workspace(conn, &workspace.id, add_labels).await? {
        let rows_affected = sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if rows_affected > 0 {
            append_change(
                conn,
                ChangeEntity::Task,
                task_id,
                Some("labels"),
                op_type::LABEL_ADD,
                ChangePayload::workspace(&workspace).set("label", label),
            )
            .await?;
            changed = true;
        }
    }
    for label in resolve_labels_in_workspace(conn, &workspace.id, remove_labels).await? {
        let rows_affected = sqlx::query(
            "DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if rows_affected > 0 {
            append_change(
                conn,
                ChangeEntity::Task,
                task_id,
                Some("labels"),
                op_type::LABEL_REMOVE,
                ChangePayload::workspace(&workspace).set("label", label),
            )
            .await?;
            changed = true;
        }
    }
    if changed {
        info!(
            task_id = %task_id,
            added = add_labels.len(),
            removed = remove_labels.len(),
            "task labels changed"
        );
    }
    Ok(changed)
}

pub(crate) async fn set_task_deleted(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    deleted: bool,
) -> Result<TaskOutcome> {
    set_task_field(
        conn,
        workspace,
        task_id,
        "deleted",
        if deleted { "1" } else { "0" },
    )
    .await?;
    crate::attachments::lifecycle::reconcile_liveness(
        conn,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    info!(task_id = %task_id, deleted, "task deleted flag changed");
    Ok(TaskOutcome {
        task: get_task_in_workspace(conn, workspace, task_id).await?,
        create_change_id: None,
        attachment_change_ids: Vec::new(),
    })
}

pub(crate) async fn add_note(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    body: String,
) -> Result<NoteOutcome> {
    let note_id = new_id();
    let ts = now();
    let mut tx = begin_immediate(conn).await?;
    let change_id = append_change(
        &mut tx,
        ChangeEntity::Task,
        task_id,
        Some("notes"),
        op_type::NOTE_ADD,
        ChangePayload::workspace(workspace)
            .set("note_id", &note_id)
            .set("body", &body)
            .set("created_at", &ts),
    )
    .await?;
    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(&note_id)
    .bind(task_id)
    .bind(&body)
    .bind(&ts)
    .bind(&change_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&ts)
        .bind(&workspace.id)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    info!(task_id = %task_id, note_id = %note_id, "note added");
    Ok(NoteOutcome {
        task_id: task_id.clone(),
        note_id,
    })
}

pub(crate) async fn delete_note(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    note_id: &str,
) -> Result<NoteDeleteOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let deleted_at = now();
    let deleted =
        sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?")
            .bind(&workspace.id)
            .bind(task_id)
            .bind(note_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if deleted > 0 {
        sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
            .bind(&deleted_at)
            .bind(&workspace.id)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        append_change(
            &mut tx,
            ChangeEntity::Task,
            task_id,
            Some("notes"),
            op_type::NOTE_DELETE,
            ChangePayload::workspace(workspace)
                .set("note_id", note_id)
                .set("deleted_at", deleted_at),
        )
        .await?;
    }
    tx.commit().await?;
    if deleted > 0 {
        info!(task_id = %task_id, note_id = %note_id, "note deleted");
    }
    Ok(NoteDeleteOutcome {
        task_id: task_id.clone(),
        note_id: note_id.to_string(),
        changed: deleted > 0,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, RgbaImage};

    use super::*;
    use crate::attachments::optimization::ImageOptimizationPolicy;
    use crate::attachments::storage::{object_path, sha256_hex};
    use crate::operations::{AttachmentAddInput, TaskAttachmentAddInput};

    fn task_draft(title: &str) -> TaskDraft {
        TaskDraft {
            title: title.to_string(),
            description: "description".to_string(),
            project: None,
            status: "inbox".to_string(),
            priority: "none".to_string(),
            labels: Vec::new(),
            available_at: None,
            due_on: None,
            is_epic: false,
        }
    }

    fn png_bytes(width: u32) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::new(width, 1))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn attachment(id: &str, bytes: Vec<u8>) -> TaskAttachmentAddInput {
        TaskAttachmentAddInput {
            attachment_id: id.to_string(),
            input: AttachmentAddInput {
                filename: Some(format!("{id}.png")),
                alt_text: None,
                declared_media_type: Some("image/png".to_string()),
                bytes,
                optimization_policy: ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        }
    }

    async fn setup() -> (tempfile::TempDir, sqlx::SqlitePool, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        (dir, pool, Workspace::default())
    }

    async fn counts(conn: &mut SqliteConnection) -> (i64, i64, i64, i64) {
        let tasks = sqlx::query_scalar("SELECT count(*) FROM tasks")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let attachments = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let changes = sqlx::query_scalar("SELECT count(*) FROM changes")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let versions = sqlx::query_scalar("SELECT count(*) FROM field_versions")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        (tasks, attachments, changes, versions)
    }

    #[tokio::test]
    async fn creates_zero_one_and_multiple_attachments_atomically() {
        for attachment_count in 0..=3 {
            let (dir, pool, workspace) = setup().await;
            let blob_dir = dir.path().join("blobs");
            let inputs = (0..attachment_count)
                .map(|index| attachment(&format!("ATTACHMENT{index:06}"), png_bytes(index + 1)))
                .collect();
            let mut conn = pool.acquire().await.unwrap();
            let outcome = create_task_with_attachments(
                &mut conn,
                &workspace,
                &blob_dir,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                task_draft("atomic"),
                inputs,
            )
            .await
            .unwrap();
            assert_eq!(
                outcome.attachment_change_ids.len(),
                attachment_count as usize
            );
            let (tasks, attachments, _changes, versions) = counts(&mut conn).await;
            assert_eq!(tasks, 1);
            assert_eq!(attachments, i64::from(attachment_count));
            let task_changes: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM changes WHERE op_type IN ('create_task', 'attachment_add')",
            )
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            assert_eq!(task_changes, 1 + i64::from(attachment_count));
            assert_eq!(versions, TaskField::VERSIONED.len() as i64);
        }
    }

    #[tokio::test]
    async fn duplicate_content_keeps_attachment_order_and_uses_one_object() {
        let (dir, pool, workspace) = setup().await;
        let blob_dir = dir.path().join("blobs");
        let bytes = png_bytes(2);
        let hash = sha256_hex(&bytes);
        let mut conn = pool.acquire().await.unwrap();
        create_task_with_attachments(
            &mut conn,
            &workspace,
            &blob_dir,
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            task_draft("duplicates"),
            vec![
                attachment("ATTACHMENT000002", bytes.clone()),
                attachment("ATTACHMENT000001", bytes),
            ],
        )
        .await
        .unwrap();

        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT attachment_id FROM task_attachments ORDER BY created_at, attachment_id",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(ids, vec!["ATTACHMENT000002", "ATTACHMENT000001"]);
        assert!(object_path(&blob_dir, &hash).unwrap().exists());
        assert_eq!(
            std::fs::read_dir(blob_dir.join("objects/sha256"))
                .unwrap()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn multi_attachment_capacity_is_reserved_per_hash() {
        let (dir, pool, workspace) = setup().await;
        let blob_dir = dir.path().join("blobs");
        let first = png_bytes(2);
        let second = png_bytes(3);
        let first_hash = sha256_hex(&first);
        let second_hash = sha256_hex(&second);
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TABLE reservation_audit(sha256 TEXT NOT NULL, byte_size INTEGER NOT NULL)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER audit_local_reservation
             AFTER INSERT ON blob_upload_reservations
             WHEN NEW.workspace_id = '__local__'
             BEGIN
               INSERT INTO reservation_audit(sha256, byte_size)
               VALUES (NEW.sha256, NEW.byte_size);
             END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        create_task_with_attachments(
            &mut conn,
            &workspace,
            &blob_dir,
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            task_draft("capacity"),
            vec![
                attachment("ATTACHMENT000001", first.clone()),
                attachment("ATTACHMENT000002", second.clone()),
            ],
        )
        .await
        .unwrap();

        let reservations: Vec<(String, i64)> =
            sqlx::query_as("SELECT sha256, byte_size FROM reservation_audit ORDER BY sha256")
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        let mut expected = vec![
            (first_hash, i64::try_from(first.len()).unwrap()),
            (second_hash, i64::try_from(second.len()).unwrap()),
        ];
        expected.sort();
        assert_eq!(reservations, expected);
    }

    #[tokio::test]
    async fn committed_create_succeeds_when_maintenance_cleanup_fails() {
        let (dir, pool, workspace) = setup().await;
        let blob_dir = dir.path().join("blobs");
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TRIGGER block_lease_release BEFORE DELETE ON blob_leases
             BEGIN SELECT RAISE(FAIL, 'lease release'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER block_reservation_release
             BEFORE DELETE ON blob_upload_reservations
             BEGIN SELECT RAISE(FAIL, 'reservation release'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER block_liveness_reconcile BEFORE INSERT ON blob_lifecycle
             BEGIN SELECT RAISE(FAIL, 'liveness reconcile'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        let outcome = create_task_with_attachments(
            &mut conn,
            &workspace,
            &blob_dir,
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            task_draft("committed"),
            vec![attachment("ATTACHMENT000001", png_bytes(2))],
        )
        .await
        .unwrap();

        assert_eq!(outcome.task.title, "committed");
        assert_eq!(counts(&mut conn).await.0, 1);
    }

    #[tokio::test]
    async fn database_failure_rolls_back_and_retry_reuses_stable_ids() {
        let (dir, pool, workspace) = setup().await;
        let blob_dir = dir.path().join("blobs");
        let bytes = png_bytes(2);
        let hash = sha256_hex(&bytes);
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_attachment_insert BEFORE INSERT ON task_attachments
             BEGIN SELECT RAISE(FAIL, 'injected attachment insert failure'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let inputs = vec![attachment("ATTACHMENT000001", bytes.clone())];
        assert!(
            create_task_with_attachments(
                &mut conn,
                &workspace,
                &blob_dir,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                task_draft("retry"),
                inputs.clone(),
            )
            .await
            .is_err()
        );
        assert_eq!(counts(&mut conn).await, (0, 0, 0, 0));
        assert!(!object_path(&blob_dir, &hash).unwrap().exists());

        sqlx::query("DROP TRIGGER fail_attachment_insert")
            .execute(&mut *conn)
            .await
            .unwrap();
        create_task_with_attachments(
            &mut conn,
            &workspace,
            &blob_dir,
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            task_draft("retry"),
            inputs,
        )
        .await
        .unwrap();
        assert_eq!(counts(&mut conn).await.0, 1);
        let stored_id: String = sqlx::query_scalar("SELECT attachment_id FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(stored_id, "ATTACHMENT000001");
    }

    #[tokio::test]
    async fn failures_at_database_write_tables_leave_no_visible_state() {
        let cases = [
            (
                "blob_inventory",
                "inventory",
                "CREATE TRIGGER injected BEFORE INSERT ON blob_inventory BEGIN SELECT RAISE(FAIL, 'inventory'); END",
            ),
            (
                "tasks",
                "task",
                "CREATE TRIGGER injected BEFORE INSERT ON tasks BEGIN SELECT RAISE(FAIL, 'task'); END",
            ),
            (
                "create_change",
                "create change",
                "CREATE TRIGGER injected BEFORE INSERT ON changes WHEN NEW.op_type = 'create_task' BEGIN SELECT RAISE(FAIL, 'create change'); END",
            ),
            (
                "field_versions",
                "version",
                "CREATE TRIGGER injected BEFORE INSERT ON field_versions BEGIN SELECT RAISE(FAIL, 'version'); END",
            ),
            (
                "attachment_change",
                "attachment change",
                "CREATE TRIGGER injected BEFORE INSERT ON changes WHEN NEW.op_type = 'attachment_add' BEGIN SELECT RAISE(FAIL, 'attachment change'); END",
            ),
            (
                "task_attachments",
                "attachment",
                "CREATE TRIGGER injected BEFORE INSERT ON task_attachments BEGIN SELECT RAISE(FAIL, 'attachment'); END",
            ),
        ];
        for (name, expected_error, trigger) in cases {
            let (dir, pool, workspace) = setup().await;
            let blob_dir = dir.path().join("blobs");
            let mut conn = pool.acquire().await.unwrap();
            sqlx::query(trigger).execute(&mut *conn).await.unwrap();
            let error = create_task_with_attachments(
                &mut conn,
                &workspace,
                &blob_dir,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                task_draft(name),
                vec![attachment("ATTACHMENT000001", png_bytes(1))],
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(expected_error));
            assert_eq!(counts(&mut conn).await, (0, 0, 0, 0), "{name}");
        }
    }

    #[tokio::test]
    async fn injected_commit_failure_rolls_back_database_and_created_object() {
        let (dir, pool, workspace) = setup().await;
        let blob_dir = dir.path().join("blobs");
        let bytes = png_bytes(2);
        let hash = sha256_hex(&bytes);
        let mut conn = pool.acquire().await.unwrap();
        let result = ATOMIC_CREATE_FAILURE_POINT
            .scope(
                "commit",
                create_task_with_attachments(
                    &mut conn,
                    &workspace,
                    &blob_dir,
                    crate::attachments::lifecycle::LifecyclePolicy::default(),
                    task_draft("commit failure"),
                    vec![attachment("ATTACHMENT000001", bytes)],
                ),
            )
            .await;
        assert!(result.unwrap_err().to_string().contains("commit"));
        assert_eq!(counts(&mut conn).await, (0, 0, 0, 0));
        assert!(!object_path(&blob_dir, &hash).unwrap().exists());
    }

    #[tokio::test]
    async fn rollback_preserves_preexisting_shared_object() {
        let (dir, pool, workspace) = setup().await;
        let blob_dir = dir.path().join("blobs");
        let bytes = png_bytes(2);
        let hash = sha256_hex(&bytes);
        crate::attachments::storage::stage_blob(&blob_dir, &hash, &bytes)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_task BEFORE INSERT ON tasks
             BEGIN SELECT RAISE(FAIL, 'injected task failure'); END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        assert!(
            create_task_with_attachments(
                &mut conn,
                &workspace,
                &blob_dir,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                task_draft("shared"),
                vec![attachment("ATTACHMENT000001", bytes)],
            )
            .await
            .is_err()
        );
        assert!(object_path(&blob_dir, &hash).unwrap().exists());
        assert_eq!(counts(&mut conn).await, (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn validation_and_staging_failures_create_no_database_state() {
        let (dir, pool, workspace) = setup().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            create_task_with_attachments(
                &mut conn,
                &workspace,
                &dir.path().join("blobs"),
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                task_draft("invalid"),
                vec![attachment("ATTACHMENT000001", b"not an image".to_vec())],
            )
            .await
            .is_err()
        );
        assert_eq!(counts(&mut conn).await, (0, 0, 0, 0));

        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"file").unwrap();
        assert!(
            create_task_with_attachments(
                &mut conn,
                &workspace,
                &blocked,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                task_draft("staging"),
                vec![attachment("ATTACHMENT000001", png_bytes(1))],
            )
            .await
            .is_err()
        );
        assert_eq!(counts(&mut conn).await, (0, 0, 0, 0));
    }
}
