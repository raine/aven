use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::ids::WorkspaceId;
use anyhow::{Result, bail};
use sqlx::SqliteConnection;
use tracing::{info, warn};

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::db::{Database, begin_immediate, set_field_version};
use crate::ids::{TaskId, new_id, now};
use crate::labels::resolve_labels_in_workspace;
use crate::mutation::{set_task_field, set_task_project};
use crate::projects::resolve_or_create_project_in_workspace;
use crate::refs::{DisplayRefContext, get_task_in_workspace};
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::undo::{
    TaskUndoSnapshot, UndoCommand, UndoContext, UndoPayload, record_tui_undo, task_snapshot,
};
use crate::workspaces::Workspace;

pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub project: Option<String>,
    pub status: String,
    pub priority: String,
    pub source: TaskSource,
    pub labels: Vec<String>,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
    pub is_epic: bool,
}

#[derive(Debug, Clone, Default)]
pub enum TaskCreationUndo {
    #[default]
    None,
    TuiTask,
    TuiEpicChild {
        epic_id: TaskId,
        epic_display_ref: String,
    },
}

#[derive(Debug, Clone)]
pub struct TaskCreationOptions {
    epic_id: Option<TaskId>,
    undo: TaskCreationUndo,
}

impl TaskCreationOptions {
    pub fn standalone(undo: TaskCreationUndo) -> Self {
        Self {
            epic_id: None,
            undo,
        }
    }

    pub fn for_epic(epic_id: TaskId, undo: TaskCreationUndo) -> Self {
        Self {
            epic_id: Some(epic_id),
            undo,
        }
    }
}

#[derive(Debug)]
pub struct TaskOutcome {
    pub task: Task,
    pub create_change_id: Option<String>,
    pub attachment_change_ids: Vec<String>,
}

struct InsertedTask {
    id: TaskId,
    change_id: String,
    project_key: String,
    label_count: usize,
}

#[derive(Debug, Clone)]
pub struct TaskLabelSelection {
    pub selected: Vec<String>,
    pub partial: Vec<String>,
}

#[derive(Clone, Default)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub cycle_priority: Option<bool>,
    pub available_at: Option<Option<String>>,
    pub due_on: Option<Option<String>>,
    pub deleted: Option<bool>,
    pub is_epic: Option<bool>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub label_selection: Option<TaskLabelSelection>,
}

pub struct TaskUpdateOutcome {
    pub task: Task,
    pub changed: bool,
}

#[derive(Debug)]
pub struct TaskMutationOutcome {
    pub task: Task,
    pub before: TaskUndoSnapshot,
    pub after: TaskUndoSnapshot,
    pub changed: bool,
}

#[derive(Debug)]
pub struct TaskMutationReport {
    pub outcomes: Vec<TaskMutationOutcome>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskMutationDetail {
    Report,
    Compact,
}

struct TaskMutationExecutionOutcome {
    task: Task,
    before: Option<TaskUndoSnapshot>,
    after: Option<TaskUndoSnapshot>,
    changed: bool,
}

impl TaskMutationReport {
    pub fn changed_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.changed)
            .count()
    }
}

pub struct NoteDeleteOutcome {
    #[allow(dead_code)]
    pub task_id: TaskId,
    #[allow(dead_code)]
    pub note_id: String,
    pub changed: bool,
}

pub struct NoteOutcome {
    #[allow(dead_code)]
    pub task_id: TaskId,
    pub note_id: String,
    pub change_id: String,
}
impl Database {
    pub async fn create_task(
        &self,
        workspace: &Workspace,
        draft: TaskDraft,
    ) -> Result<TaskOutcome> {
        self.create_task_with_undo(workspace, draft, TaskCreationUndo::None)
            .await
    }

    pub async fn create_task_with_undo(
        &self,
        workspace: &Workspace,
        draft: TaskDraft,
        undo: TaskCreationUndo,
    ) -> Result<TaskOutcome> {
        let mut conn = self.acquire().await?;
        create_task_with_epic(&mut conn, workspace, draft, None, undo).await
    }

    pub async fn create_task_for_epic(
        &self,
        workspace: &Workspace,
        draft: TaskDraft,
        epic_id: &TaskId,
    ) -> Result<TaskOutcome> {
        self.create_task_for_epic_with_undo(workspace, draft, epic_id, TaskCreationUndo::None)
            .await
    }

    pub async fn create_task_for_epic_with_undo(
        &self,
        workspace: &Workspace,
        draft: TaskDraft,
        epic_id: &TaskId,
        undo: TaskCreationUndo,
    ) -> Result<TaskOutcome> {
        let mut conn = self.acquire().await?;
        create_task_with_epic(&mut conn, workspace, draft, Some(epic_id), undo).await
    }

    pub async fn create_task_with_attachments(
        &self,
        workspace: &Workspace,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        draft: TaskDraft,
        attachments: Vec<super::attachments::TaskAttachmentAddInput>,
    ) -> Result<TaskOutcome> {
        self.create_task_with_attachments_and_undo(
            workspace,
            blob_dir,
            lifecycle_policy,
            draft,
            attachments,
            TaskCreationUndo::None,
        )
        .await
    }

    pub async fn create_task_with_attachments_and_undo(
        &self,
        workspace: &Workspace,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        draft: TaskDraft,
        attachments: Vec<super::attachments::TaskAttachmentAddInput>,
        undo: TaskCreationUndo,
    ) -> Result<TaskOutcome> {
        let mut conn = self.acquire().await?;
        create_task_with_attachments_and_epic(
            &mut conn,
            workspace,
            blob_dir,
            lifecycle_policy,
            draft,
            attachments,
            TaskCreationOptions::standalone(undo),
        )
        .await
    }

    pub async fn create_task_with_attachments_for_epic(
        &self,
        workspace: &Workspace,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        draft: TaskDraft,
        attachments: Vec<super::attachments::TaskAttachmentAddInput>,
        epic_id: &TaskId,
    ) -> Result<TaskOutcome> {
        self.create_task_with_attachments_for_epic_and_undo(
            workspace,
            blob_dir,
            lifecycle_policy,
            draft,
            attachments,
            TaskCreationOptions::for_epic(epic_id.clone(), TaskCreationUndo::None),
        )
        .await
    }

    pub async fn create_task_with_attachments_for_epic_and_undo(
        &self,
        workspace: &Workspace,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        draft: TaskDraft,
        attachments: Vec<super::attachments::TaskAttachmentAddInput>,
        options: TaskCreationOptions,
    ) -> Result<TaskOutcome> {
        let mut conn = self.acquire().await?;
        create_task_with_attachments_and_epic(
            &mut conn,
            workspace,
            blob_dir,
            lifecycle_policy,
            draft,
            attachments,
            options,
        )
        .await
    }

    pub async fn update_task(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        update: TaskUpdate,
    ) -> Result<TaskUpdateOutcome> {
        let outcomes = self
            .mutate_tasks_owned(
                workspace,
                vec![(task_id.clone(), update)],
                UndoContext::None,
                TaskMutationDetail::Compact,
            )
            .await?;
        let outcome = outcomes.into_iter().next().unwrap();
        Ok(TaskUpdateOutcome {
            task: outcome.task,
            changed: outcome.changed,
        })
    }

    pub async fn update_tasks(
        &self,
        workspace: &Workspace,
        updates: Vec<(TaskId, TaskUpdate)>,
    ) -> Result<Vec<TaskUpdateOutcome>> {
        Ok(self
            .mutate_tasks_owned(
                workspace,
                updates,
                UndoContext::None,
                TaskMutationDetail::Compact,
            )
            .await?
            .into_iter()
            .map(|outcome| TaskUpdateOutcome {
                task: outcome.task,
                changed: outcome.changed,
            })
            .collect())
    }

    pub async fn mutate_tasks(
        &self,
        workspace: &Workspace,
        updates: Vec<(TaskId, TaskUpdate)>,
        undo: UndoContext,
    ) -> Result<TaskMutationReport> {
        let outcomes = self
            .mutate_tasks_owned(workspace, updates, undo, TaskMutationDetail::Report)
            .await?;
        Ok(TaskMutationReport {
            outcomes: outcomes
                .into_iter()
                .map(|outcome| TaskMutationOutcome {
                    task: outcome.task,
                    before: outcome.before.expect("report mutation before snapshot"),
                    after: outcome.after.expect("report mutation after snapshot"),
                    changed: outcome.changed,
                })
                .collect(),
        })
    }

    async fn mutate_tasks_owned(
        &self,
        workspace: &Workspace,
        updates: Vec<(TaskId, TaskUpdate)>,
        undo: UndoContext,
        detail: TaskMutationDetail,
    ) -> Result<Vec<TaskMutationExecutionOutcome>> {
        for (_, update) in &updates {
            validate_task_update(update)?;
        }
        if updates.is_empty() {
            return Ok(Vec::new());
        }
        if detail == TaskMutationDetail::Compact && !matches!(&undo, UndoContext::None) {
            bail!("compact task mutations cannot record undo");
        }

        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let mut outcomes = Vec::with_capacity(updates.len());
        let mut undo_commands = Vec::new();
        for (task_id, update) in &updates {
            let before =
                if detail == TaskMutationDetail::Report || task_update_requires_snapshot(update) {
                    Some(task_snapshot(&mut tx, &workspace.id, task_id).await?)
                } else {
                    None
                };
            let update = match before.as_ref() {
                Some(before) => materialize_task_update(update, before)?,
                None => update.clone(),
            };
            let changed = apply_task_update(&mut tx, workspace, task_id, &update).await?;
            let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
            let after = if detail == TaskMutationDetail::Report {
                Some(task_snapshot(&mut tx, &workspace.id, task_id).await?)
            } else {
                None
            };
            if let (Some(before), Some(after)) = (&before, &after) {
                append_task_undo_commands(task_id, before, after, &mut undo_commands);
            }
            outcomes.push(TaskMutationExecutionOutcome {
                task,
                changed,
                before,
                after,
            });
        }
        let changed_count = outcomes.iter().filter(|outcome| outcome.changed).count();
        if let Some(summary) = undo.task_mutation_summary(changed_count) {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &summary,
                UndoPayload {
                    commands: undo_commands,
                },
            )
            .await?;
        }
        tx.commit().await?;

        for outcome in &outcomes {
            info!(task_id = %outcome.task.id, changed = outcome.changed, "task updated");
        }
        Ok(outcomes)
    }

    pub async fn set_task_deleted(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        deleted: bool,
    ) -> Result<TaskOutcome> {
        let outcome = self
            .update_task(
                workspace,
                task_id,
                TaskUpdate {
                    deleted: Some(deleted),
                    ..TaskUpdate::default()
                },
            )
            .await?;
        Ok(TaskOutcome {
            task: outcome.task,
            create_change_id: None,
            attachment_change_ids: Vec::new(),
        })
    }

    pub async fn add_note(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        body: String,
    ) -> Result<NoteOutcome> {
        let mut conn = self.acquire().await?;
        add_note_operation(&mut conn, workspace, task_id, body, false).await
    }

    pub async fn add_note_with_tui_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        body: String,
    ) -> Result<NoteOutcome> {
        let mut conn = self.acquire().await?;
        add_note_operation(&mut conn, workspace, task_id, body, true).await
    }

    pub async fn delete_note(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        note_id: &str,
    ) -> Result<NoteDeleteOutcome> {
        let mut conn = self.acquire().await?;
        delete_note(&mut conn, workspace, task_id, note_id).await
    }
}

#[cfg(any(test, feature = "test-support"))]
pub async fn create_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
) -> Result<TaskOutcome> {
    create_task_with_epic(conn, workspace, draft, None, TaskCreationUndo::None).await
}

async fn create_task_with_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
    epic_id: Option<&TaskId>,
    undo: TaskCreationUndo,
) -> Result<TaskOutcome> {
    validate_task_draft(&draft)?;
    let mut tx = begin_immediate(conn).await?;
    let inserted = insert_task(&mut tx, workspace, draft).await?;
    if let Some(epic_id) = epic_id {
        super::add_task_to_epic_in_transaction(&mut tx, workspace, &inserted.id, epic_id).await?;
    }
    let task = get_task_in_workspace(&mut tx, workspace, &inserted.id).await?;
    record_task_creation_undo(
        &mut tx,
        workspace,
        &task,
        Some(inserted.change_id.clone()),
        Vec::new(),
        Vec::new(),
        undo,
    )
    .await?;
    tx.commit().await?;
    info!(
        task_id = %inserted.id,
        project_key = %inserted.project_key,
        label_count = inserted.label_count,
        "task created"
    );
    Ok(TaskOutcome {
        task,
        create_change_id: Some(inserted.change_id),
        attachment_change_ids: Vec::new(),
    })
}

async fn create_task_with_attachments_and_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    blob_dir: &Path,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
    draft: TaskDraft,
    attachments: Vec<super::attachments::TaskAttachmentAddInput>,
    options: TaskCreationOptions,
) -> Result<TaskOutcome> {
    validate_task_draft(&draft)?;
    let TaskCreationOptions { epic_id, undo } = options;
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
            Ok(staged) if staged.byte_size == attachment.byte_size => {
                if staged.created {
                    created_hashes.push(staged.sha256);
                }
            }
            Ok(_) => {
                cleanup_attachment_guards(conn, &staging_leases, &capacity_reservations).await;
                cleanup_created_objects(conn, blob_dir, &created_hashes).await;
                bail!("error attachment-staged-size-mismatch");
            }
            Err(error) => {
                cleanup_attachment_guards(conn, &staging_leases, &capacity_reservations).await;
                cleanup_created_objects(conn, blob_dir, &created_hashes).await;
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
        if let Some(epic_id) = epic_id.as_ref() {
            super::add_task_to_epic_in_transaction(&mut tx, workspace, &inserted.id, epic_id)
                .await?;
        }
        let mut attachment_change_ids = Vec::with_capacity(prepared.len());
        let attachment_base = chrono::DateTime::parse_from_rfc3339(&now())?.to_utc();
        for (index, attachment) in prepared.iter().enumerate() {
            let created_at = (attachment_base
                + chrono::TimeDelta::microseconds(i64::try_from(index)?))
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
        let attachment_ids = prepared
            .iter()
            .map(|attachment| attachment.attachment_id.clone())
            .collect();
        record_task_creation_undo(
            &mut tx,
            workspace,
            &task,
            Some(inserted.change_id.clone()),
            attachment_ids,
            attachment_change_ids.clone(),
            undo,
        )
        .await?;
        tx.commit().await?;
        Ok::<_, anyhow::Error>((inserted, attachment_change_ids, task))
    }
    .await;

    let (inserted, attachment_change_ids, task) = match database_result {
        Ok(value) => value,
        Err(error) => {
            cleanup_attachment_guards(conn, &staging_leases, &capacity_reservations).await;
            cleanup_created_objects(conn, blob_dir, &created_hashes).await;
            return Err(error);
        }
    };
    cleanup_attachment_guards(conn, &staging_leases, &capacity_reservations).await;
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

async fn record_task_creation_undo(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task: &Task,
    create_change_id: Option<String>,
    attachment_ids: Vec<String>,
    attachment_change_ids: Vec<String>,
    undo: TaskCreationUndo,
) -> Result<()> {
    let (summary, command) = match undo {
        TaskCreationUndo::None => return Ok(()),
        TaskCreationUndo::TuiTask => (
            format!("task {}", task.id),
            UndoCommand::DeleteCreatedTask {
                task_id: task.id.clone(),
                create_change_id,
                expected: task_snapshot(conn, &workspace.id, &task.id).await?,
                attachment_ids,
                attachment_change_ids,
            },
        ),
        TaskCreationUndo::TuiEpicChild {
            epic_id,
            epic_display_ref,
        } => {
            let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
            let child_ref = display_refs.display_ref(task);
            (
                format!("add {child_ref} to {epic_display_ref}"),
                UndoCommand::AddEpicChild {
                    epic_id,
                    child_id: task.id.clone(),
                },
            )
        }
    };
    record_tui_undo(
        conn,
        &workspace.id,
        &summary,
        UndoPayload {
            commands: vec![command],
        },
    )
    .await
}

fn validate_task_draft(draft: &TaskDraft) -> Result<()> {
    TaskStatus::parse(&draft.status)?;
    TaskPriority::parse(&draft.priority)?;
    if let Some(available_at) = draft.available_at.as_deref() {
        crate::time_validation::validate_available_at_value(available_at)?;
    }
    if let Some(due_on) = draft.due_on.as_deref() {
        crate::time_validation::validate_due_on_value(due_on)?;
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
    let available_at = draft.available_at.as_deref().unwrap_or("");
    let due_on = draft.due_on.as_deref().unwrap_or("");
    let id = TaskId::new();
    let ts = now();
    let project = draft
        .project
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("error project-required"))?;
    let project = resolve_or_create_project_in_workspace(conn, &workspace.id, project).await?;
    let labels = resolve_labels_in_workspace(conn, &workspace.id, &draft.labels).await?;
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, source, created_at, updated_at, queue_activity_at, available_at, due_on, is_epic)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(&id)
    .bind(&draft.title)
    .bind(&draft.description)
    .bind(&project.id)
    .bind(status.as_str())
    .bind(priority.as_str())
    .bind(draft.source.as_str())
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
            .set("source", draft.source.as_str())
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

async fn cleanup_attachment_guards(
    conn: &mut SqliteConnection,
    leases: &[String],
    reservations: &[String],
) {
    for lease_id in leases {
        if let Err(error) = crate::attachments::lifecycle::release_lease(conn, lease_id).await {
            warn!(%error, "failed to release attachment staging lease");
        }
    }
    for reservation_id in reservations {
        if let Err(error) =
            crate::attachments::lifecycle::release_reservation(conn, reservation_id).await
        {
            warn!(%error, "failed to release attachment capacity reservation");
        }
    }
}

async fn cleanup_created_objects(conn: &mut SqliteConnection, blob_dir: &Path, hashes: &[String]) {
    for sha256 in hashes {
        crate::attachments::storage::remove_staged_blob_if_unreferenced(conn, blob_dir, sha256)
            .await;
    }
}

#[cfg(any(test, feature = "test-support"))]
pub async fn update_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    update: TaskUpdate,
) -> Result<TaskUpdateOutcome> {
    validate_task_update(&update)?;
    let mut tx = begin_immediate(conn).await?;
    let changed = apply_task_update(&mut tx, workspace, task_id, &update).await?;
    tx.commit().await?;
    Ok(TaskUpdateOutcome {
        task: get_task_in_workspace(conn, workspace, task_id).await?,
        changed,
    })
}

fn append_task_undo_commands(
    task_id: &TaskId,
    before: &TaskUndoSnapshot,
    after: &TaskUndoSnapshot,
    commands: &mut Vec<UndoCommand>,
) {
    let fields = [
        ("title", before.title.as_str(), after.title.as_str()),
        (
            "description",
            before.description.as_str(),
            after.description.as_str(),
        ),
        (
            "project",
            before.project_id.as_str(),
            after.project_id.as_str(),
        ),
        ("status", before.status.as_str(), after.status.as_str()),
        (
            "priority",
            before.priority.as_str(),
            after.priority.as_str(),
        ),
        (
            "available_at",
            before.available_at.as_str(),
            after.available_at.as_str(),
        ),
        ("due_on", before.due_on.as_str(), after.due_on.as_str()),
    ];
    for (field, before, after) in fields {
        if before != after {
            commands.push(UndoCommand::SetTaskField {
                task_id: task_id.clone(),
                field: field.to_string(),
                before: before.to_string(),
                after: after.to_string(),
            });
        }
    }
    let before_deleted = if before.deleted { "1" } else { "0" };
    let after_deleted = if after.deleted { "1" } else { "0" };
    if before_deleted != after_deleted {
        commands.push(UndoCommand::SetTaskField {
            task_id: task_id.clone(),
            field: "deleted".to_string(),
            before: before_deleted.to_string(),
            after: after_deleted.to_string(),
        });
    }
    let before_is_epic = if before.is_epic { "1" } else { "0" };
    let after_is_epic = if after.is_epic { "1" } else { "0" };
    if before_is_epic != after_is_epic {
        commands.push(UndoCommand::SetTaskField {
            task_id: task_id.clone(),
            field: "is_epic".to_string(),
            before: before_is_epic.to_string(),
            after: after_is_epic.to_string(),
        });
    }
    if before.labels != after.labels {
        commands.push(UndoCommand::SetTaskLabels {
            task_id: task_id.clone(),
            before: before.labels.clone(),
            after: after.labels.clone(),
        });
    }
}

fn cycled_priority(priority: TaskPriority, reverse: bool) -> TaskPriority {
    let index = TaskPriority::ALL
        .iter()
        .position(|candidate| *candidate == priority)
        .unwrap_or(0);
    let next = if reverse {
        (index + TaskPriority::ALL.len() - 1) % TaskPriority::ALL.len()
    } else {
        (index + 1) % TaskPriority::ALL.len()
    };
    TaskPriority::ALL[next]
}

fn task_update_requires_snapshot(update: &TaskUpdate) -> bool {
    update.cycle_priority.is_some() || update.label_selection.is_some()
}

fn materialize_task_update(update: &TaskUpdate, before: &TaskUndoSnapshot) -> Result<TaskUpdate> {
    let mut update = update.clone();
    if let Some(reverse) = update.cycle_priority.take() {
        let priority = TaskPriority::parse(&before.priority)?;
        update.priority = Some(cycled_priority(priority, reverse).to_string());
    }
    if let Some(selection) = update.label_selection.take() {
        let selected = selection.selected.iter().cloned().collect::<BTreeSet<_>>();
        let partial = selection.partial.iter().cloned().collect::<BTreeSet<_>>();
        update.add_labels = selection
            .selected
            .into_iter()
            .filter(|label| !before.labels.contains(label))
            .collect();
        update.remove_labels = before
            .labels
            .iter()
            .filter(|label| !selected.contains(label.as_str()) && !partial.contains(label.as_str()))
            .cloned()
            .collect();
    }
    Ok(update)
}

fn validate_task_update(update: &TaskUpdate) -> Result<()> {
    if update.priority.is_some() && update.cycle_priority.is_some() {
        bail!("error invalid-task-update priority and priority cycle are mutually exclusive");
    }
    if update.label_selection.is_some()
        && (!update.add_labels.is_empty() || !update.remove_labels.is_empty())
    {
        bail!("error invalid-task-update label selection and label deltas are mutually exclusive");
    }
    if let Some(status) = update.status.as_deref() {
        TaskStatus::parse(status)?;
    }
    if let Some(priority) = update.priority.as_deref() {
        TaskPriority::parse(priority)?;
    }
    if let Some(Some(available_at)) = update.available_at.as_ref() {
        crate::time_validation::validate_available_at_value(available_at)?;
    }
    if let Some(Some(due_on)) = update.due_on.as_ref() {
        crate::time_validation::validate_due_on_value(due_on)?;
    }
    Ok(())
}

async fn apply_task_update(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    update: &TaskUpdate,
) -> Result<bool> {
    let mut changed = false;
    if let Some(title) = update.title.as_deref() {
        changed |= update_task_field(conn, workspace, task_id, "title", title).await?;
    }
    if let Some(description) = update.description.as_deref() {
        changed |= update_task_field(conn, workspace, task_id, "description", description).await?;
    }
    if let Some(project) = update.project.as_deref() {
        let project = resolve_or_create_project_in_workspace(conn, &workspace.id, project).await?;
        changed |= set_task_project(conn, workspace, task_id, &project).await?;
    }
    if let Some(status) = update.status.as_deref() {
        changed |= update_task_field(conn, workspace, task_id, "status", status).await?;
    }
    if let Some(priority) = update.priority.as_deref() {
        changed |= update_task_field(conn, workspace, task_id, "priority", priority).await?;
    }
    if let Some(available_at) = update.available_at.as_ref() {
        changed |= update_task_field(
            conn,
            workspace,
            task_id,
            "available_at",
            available_at.as_deref().unwrap_or(""),
        )
        .await?;
    }
    if let Some(due_on) = update.due_on.as_ref() {
        changed |= update_task_field(
            conn,
            workspace,
            task_id,
            "due_on",
            due_on.as_deref().unwrap_or(""),
        )
        .await?;
    }
    if let Some(deleted) = update.deleted {
        changed |= update_task_field(
            conn,
            workspace,
            task_id,
            "deleted",
            if deleted { "1" } else { "0" },
        )
        .await?;
        crate::attachments::lifecycle::reconcile_liveness_in_transaction(
            conn,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
    }
    if let Some(is_epic) = update.is_epic {
        if !is_epic {
            let task = get_task_in_workspace(conn, workspace, task_id).await?;
            if super::epics::task_has_epic_children(conn, &task.workspace_id, task_id).await? {
                bail!("error epic-has-children task_id={task_id}");
            }
        }
        changed |= update_task_field(
            conn,
            workspace,
            task_id,
            "is_epic",
            if is_epic { "1" } else { "0" },
        )
        .await?;
    }
    changed |= update_task_labels_in_workspace(
        conn,
        &workspace.id,
        task_id,
        &update.add_labels,
        &update.remove_labels,
    )
    .await?;
    Ok(changed)
}

pub async fn update_task_field(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    value: &str,
) -> Result<bool> {
    set_task_field(conn, workspace, task_id, field, value).await
}

pub async fn update_task_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    add_labels: &[String],
    remove_labels: &[String],
) -> Result<bool> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    crate::operations::route_recurrence_task_field(conn, &workspace, task_id, "labels", "").await?;
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

pub(super) async fn add_note_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    body: String,
    tui_undo: bool,
) -> Result<NoteOutcome> {
    let note_id = new_id();
    let ts = now();
    let mut tx = begin_immediate(conn).await?;
    crate::operations::route_recurrence_task_field(&mut tx, workspace, task_id, "notes", "")
        .await?;
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
    if tui_undo {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            &format!("note {note_id}"),
            UndoPayload {
                commands: vec![UndoCommand::DeleteCreatedNote {
                    task_id: task_id.clone(),
                    note_id: note_id.clone(),
                    note_add_change_id: change_id.clone(),
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!(task_id = %task_id, note_id = %note_id, "note added");
    Ok(NoteOutcome {
        task_id: task_id.clone(),
        note_id,
        change_id,
    })
}

pub async fn delete_note(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    note_id: &str,
) -> Result<NoteDeleteOutcome> {
    let mut tx = begin_immediate(conn).await?;
    crate::operations::route_recurrence_task_field(&mut tx, workspace, task_id, "notes", "")
        .await?;
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
