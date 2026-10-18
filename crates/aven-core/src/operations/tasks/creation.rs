use super::{InsertedTask, TaskCreationOptions, TaskCreationUndo, TaskDraft, TaskOutcome};
use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{Database, begin_immediate, set_field_version};
use crate::ids::{TaskId, now};
use crate::labels::CreatedLabel;
use crate::labels::{resolve_labels_in_workspace, resolve_or_create_labels_in_workspace};
use crate::projects::{
    resolve_existing_project_in_workspace, resolve_or_create_project_in_workspace,
};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::undo::{UndoCommand, UndoPayload, record_tui_undo, task_snapshot};
use crate::workspaces::Workspace;
use anyhow::Result;
use sqlx::SqliteConnection;
use tracing::info;

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
        self.create_task_with_options(workspace, draft, TaskCreationOptions::standalone(undo))
            .await
    }

    pub async fn create_task_with_options(
        &self,
        workspace: &Workspace,
        draft: TaskDraft,
        options: TaskCreationOptions,
    ) -> Result<TaskOutcome> {
        let mut conn = self.acquire_writer().await?;
        create_task_with_epic(&mut conn, workspace, draft, options).await
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
        let mut conn = self.acquire_writer().await?;
        create_task_with_epic(
            &mut conn,
            workspace,
            draft,
            TaskCreationOptions::for_epic(epic_id.clone(), undo),
        )
        .await
    }
}

#[cfg(any(test, feature = "test-support"))]
pub async fn create_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
) -> Result<TaskOutcome> {
    create_task_with_epic(
        conn,
        workspace,
        draft,
        TaskCreationOptions::standalone(TaskCreationUndo::None),
    )
    .await
}

async fn create_task_with_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
    options: TaskCreationOptions,
) -> Result<TaskOutcome> {
    validate_task_draft(&draft)?;
    let TaskCreationOptions {
        epic_id,
        undo,
        create_missing_labels,
        require_existing_project,
        capture_undo_snapshot,
        require_existing_epic,
    } = options;
    let mut tx = begin_immediate(conn).await?;
    if require_existing_epic && let Some(epic_id) = &epic_id {
        crate::operations::epics::require_consumer_epic(&mut tx, workspace, epic_id).await?;
    }
    let inserted = insert_task(
        &mut tx,
        workspace,
        draft,
        create_missing_labels,
        require_existing_project,
    )
    .await?;
    if let Some(epic_id) = epic_id {
        crate::operations::add_task_to_epic_in_transaction(
            &mut tx,
            workspace,
            &inserted.id,
            &epic_id,
        )
        .await?;
    }
    let task = get_task_in_workspace(&mut tx, workspace, &inserted.id).await?;
    record_task_creation_undo(
        &mut tx,
        workspace,
        &task,
        &inserted,
        Vec::new(),
        Vec::new(),
        undo,
    )
    .await?;
    let undo_snapshot = if capture_undo_snapshot {
        Some(task_snapshot(&mut tx, &workspace.id, &task.id).await?)
    } else {
        None
    };
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
        undo_snapshot,
    })
}

pub(super) async fn record_task_creation_undo(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task: &Task,
    inserted: &InsertedTask,
    attachment_ids: Vec<String>,
    attachment_change_ids: Vec<String>,
    undo: TaskCreationUndo,
) -> Result<()> {
    let commands = match undo {
        TaskCreationUndo::None => return Ok(()),
        TaskCreationUndo::TuiTask => {
            let mut commands = vec![UndoCommand::DeleteCreatedTask {
                task_id: task.id.clone(),
                create_change_id: Some(inserted.change_id.clone()),
                expected: task_snapshot(conn, &workspace.id, &task.id).await?,
                attachment_ids,
                attachment_change_ids,
            }];
            append_created_label_undo_commands(&mut commands, &inserted.created_labels);
            commands
        }
        TaskCreationUndo::TuiEpicChild { epic_id } => vec![UndoCommand::AddEpicChild {
            epic_id,
            child_id: task.id.clone(),
        }],
    };
    record_tui_undo(conn, &workspace.id, UndoPayload { commands }).await
}

pub(super) fn append_created_label_undo_commands(
    commands: &mut Vec<UndoCommand>,
    created_labels: &[CreatedLabel],
) {
    commands.extend(
        created_labels
            .iter()
            .map(|label| UndoCommand::DeleteCreatedLabel {
                label: label.name.clone(),
                create_change_id: label.change_id.clone(),
            }),
    );
}

pub(super) fn validate_task_draft(draft: &TaskDraft) -> Result<()> {
    TaskStatus::parse(&draft.status)?;
    TaskPriority::parse(&draft.priority)?;
    crate::metadata::validate_metadata_update(&draft.metadata, &[])?;
    if let Some(available_at) = draft.available_at.as_deref() {
        crate::time_validation::validate_available_at_value(available_at)?;
    }
    if let Some(due_on) = draft.due_on.as_deref() {
        crate::time_validation::validate_due_on_value(due_on)?;
    }
    Ok(())
}

pub(super) fn initial_status(draft: &TaskDraft) -> Result<(TaskStatus, TaskPriority)> {
    let status = TaskStatus::parse(&draft.status)?;
    let priority = TaskPriority::parse(&draft.priority)?;
    let status = if status == TaskStatus::Inbox && priority.promotes_inbox_to_todo() {
        TaskStatus::Todo
    } else {
        status
    };
    Ok((status, priority))
}

pub(super) async fn insert_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: TaskDraft,
    create_missing_labels: bool,
    require_existing_project: bool,
) -> Result<InsertedTask> {
    let (status, priority) = initial_status(&draft)?;
    let available_at = draft.available_at.as_deref().unwrap_or("");
    let due_on = draft.due_on.as_deref().unwrap_or("");
    let id = TaskId::new();
    let ts = now();
    let project = draft
        .project
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("error project-required"))?;
    let project = if require_existing_project {
        resolve_existing_project_in_workspace(conn, &workspace.id, project).await?
    } else {
        resolve_or_create_project_in_workspace(conn, &workspace.id, project).await?
    };
    let (labels, created_labels) = if create_missing_labels {
        let resolution =
            resolve_or_create_labels_in_workspace(conn, workspace, &draft.labels).await?;
        (resolution.names, resolution.created)
    } else {
        (
            resolve_labels_in_workspace(conn, &workspace.id, &draft.labels).await?,
            Vec::new(),
        )
    };
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
    let metadata =
        crate::metadata::insert_initial_task_metadata(conn, workspace, &id, &draft.metadata, &ts)
            .await?;
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
            .set("metadata", &metadata)
            .set("created_at", ts),
    )
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO task_workspace_history(task_id, workspace_id)
         VALUES (?, ?)",
    )
    .bind(&id)
    .bind(&workspace.id)
    .execute(&mut *conn)
    .await?;
    for field in TaskField::VERSIONED {
        set_field_version(conn, &id, field.as_str(), &change_id).await?;
    }
    for value in &metadata {
        set_field_version(
            conn,
            &id,
            &format!("metadata:{}", value.field_id),
            &change_id,
        )
        .await?;
    }
    Ok(InsertedTask {
        id,
        change_id,
        project_key: project.key,
        label_count: labels.len(),
        created_labels,
    })
}
