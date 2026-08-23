use crate::ids::{ProjectId, WorkspaceId};
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use sqlx::{Row, SqliteConnection};

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{
    Database, begin_immediate, conflict_exists, field_version, insert_change, set_field_version,
    task_from_row,
};
use crate::error::CoreError;
use crate::ids::{new_id, now};
use crate::mutation::{apply_field_value_in_workspace, apply_project_id_in_workspace};
use crate::operations::{
    ProjectMetadata, insert_project_metadata_change, set_project_metadata,
    update_task_labels_in_workspace,
};
use crate::projects::resolve_project_for_stored_value;
use crate::task_fields::TaskField;
use crate::workspaces::workspace_key_for_id;

tokio::task_local! {
    static APPLYING_UNDO: ();
}

pub fn is_applying_undo() -> bool {
    APPLYING_UNDO.try_with(|_| ()).is_ok()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UndoPayload {
    pub commands: Vec<UndoCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUndoPresentation {
    pub id: String,
    pub operation: String,
    pub task_ids: Vec<crate::ids::TaskId>,
}

impl PendingUndoPresentation {
    pub fn operation_phrase(&self) -> &str {
        &self.operation
    }
}

#[derive(Debug, Clone, Default)]
pub enum UndoContext {
    #[default]
    None,
    Tui {
        summary: String,
    },
    TuiTaskMutation {
        single_summary: Option<String>,
        batch_action: String,
    },
}

impl UndoContext {
    pub fn tui(summary: impl Into<String>) -> Self {
        Self::Tui {
            summary: summary.into(),
        }
    }

    pub fn tui_task_mutation(
        single_summary: Option<String>,
        batch_action: impl Into<String>,
    ) -> Self {
        Self::TuiTaskMutation {
            single_summary,
            batch_action: batch_action.into(),
        }
    }

    pub(crate) fn task_mutation_summary(self, changed_count: usize) -> Option<String> {
        match self {
            Self::None => None,
            Self::Tui { summary } => Some(summary),
            Self::TuiTaskMutation {
                single_summary,
                batch_action,
            } => single_summary.filter(|_| changed_count == 1).or_else(|| {
                let noun = if changed_count == 1 { "task" } else { "tasks" };
                Some(format!("{batch_action} {changed_count} {noun}"))
            }),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum UndoCommand {
    SetTaskField {
        task_id: crate::ids::TaskId,
        field: String,
        before: String,
        after: String,
    },
    SetTaskLabels {
        task_id: crate::ids::TaskId,
        before: Vec<String>,
        after: Vec<String>,
    },
    SetTaskMetadata {
        task_id: crate::ids::TaskId,
        field_id: crate::ids::MetadataFieldId,
        before: Option<String>,
        after: Option<String>,
    },
    DeleteCreatedTask {
        task_id: crate::ids::TaskId,
        create_change_id: Option<String>,
        expected: TaskUndoSnapshot,
        #[serde(default)]
        attachment_ids: Vec<String>,
        #[serde(default)]
        attachment_change_ids: Vec<String>,
    },
    SetNoteBody {
        task_id: crate::ids::TaskId,
        note_id: String,
        before: String,
        after: String,
    },
    RestoreDeletedNote {
        task_id: crate::ids::TaskId,
        note_id: String,
        body: String,
        created_at: String,
    },
    DeleteCreatedNote {
        task_id: crate::ids::TaskId,
        note_id: String,
        note_add_change_id: String,
    },
    DeleteCreatedProject {
        project_key: String,
        create_change_id: String,
        expected_name: String,
        expected_prefix: String,
    },
    SetProjectMetadata {
        project_id: ProjectId,
        before_key: String,
        before_name: String,
        before_prefix: String,
        after_key: String,
        after_name: String,
        after_prefix: String,
    },
    DeleteCreatedLabel {
        label: String,
        create_change_id: String,
    },
    SetLabelName {
        before: String,
        after: String,
    },
    RestoreDeletedLabel {
        name: String,
        created_at: String,
        task_ids: Vec<crate::ids::TaskId>,
        series_ids: Vec<crate::recurrence::RecurrenceSeriesId>,
    },
    RestoreConflictResolution {
        task_id: crate::ids::TaskId,
        field: String,
        before: String,
        after: String,
        conflict_id: i64,
    },
    AddTaskDependency {
        task_id: crate::ids::TaskId,
        depends_on_task_id: crate::ids::TaskId,
    },
    RemoveTaskDependency {
        task_id: crate::ids::TaskId,
        depends_on_task_id: crate::ids::TaskId,
    },
    SetTaskRelatedLink {
        task_id: crate::ids::TaskId,
        related_task_id: crate::ids::TaskId,
        forward_change_id: String,
        linked: bool,
    },
    AddEpicChild {
        epic_id: crate::ids::TaskId,
        child_id: crate::ids::TaskId,
    },
    RemoveEpicChild {
        epic_id: crate::ids::TaskId,
        child_id: crate::ids::TaskId,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskUndoSnapshot {
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub project_key: String,
    pub status: String,
    pub priority: String,
    pub available_at: String,
    #[serde(default)]
    pub due_on: String,
    pub deleted: bool,
    #[serde(default)]
    pub is_epic: bool,
    pub labels: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

pub struct UndoOutcome {
    pub presentation: PendingUndoPresentation,
    pub task_id: Option<crate::ids::TaskId>,
    pub include_deleted: Option<bool>,
    pub project_rename: Option<ProjectRenameUndoOutcome>,
    pub label_rename: Option<LabelRenameUndoOutcome>,
}

pub struct LabelRenameUndoOutcome {
    pub before: String,
    pub after: String,
}

pub struct ProjectRenameUndoOutcome {
    pub before_key: String,
    pub after_key: String,
}

impl Database {
    pub async fn task_field_value(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &crate::ids::TaskId,
        field: &str,
    ) -> Result<String> {
        let mut conn = self.acquire_reader().await?;
        task_field_value(&mut conn, workspace_id, task_id, field).await
    }

    pub async fn task_labels(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &crate::ids::TaskId,
    ) -> Result<Vec<String>> {
        let mut conn = self.acquire_reader().await?;
        task_labels(&mut conn, workspace_id, task_id).await
    }

    pub async fn task_undo_snapshot(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &crate::ids::TaskId,
    ) -> Result<TaskUndoSnapshot> {
        let mut conn = self.acquire_reader().await?;
        task_snapshot(&mut conn, workspace_id, task_id).await
    }

    pub async fn conflict_row_id(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &crate::ids::TaskId,
        field: &str,
    ) -> Result<i64> {
        let mut conn = self.acquire_reader().await?;
        conflict_row_id(&mut conn, workspace_id, task_id, field).await
    }

    pub async fn clear_pending_tui_undo_entries(&self) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        clear_pending_tui_undo_entries(&mut conn).await
    }

    pub async fn latest_tui_undo_presentation(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<PendingUndoPresentation>> {
        let mut conn = self.acquire_reader().await?;
        latest_tui_undo_presentation(&mut conn, workspace_id).await
    }

    pub async fn apply_latest_tui_undo(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<UndoOutcome>> {
        let mut conn = self.acquire_writer().await?;
        apply_latest_tui_undo(&mut conn, workspace_id).await
    }
}

pub(crate) async fn task_field_value(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    field: &str,
) -> Result<String> {
    let task_field = TaskField::parse_or_unknown(field)?;
    task_field_value_for_field(conn, workspace_id, task_id, task_field).await
}

async fn task_field_value_for_field(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    task_field: TaskField,
) -> Result<String> {
    let row = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id, p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.source, t.created_at, t.updated_at, t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND t.id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("error task-not-found task_id={task_id}"))?;
    let task = task_from_row(&row)?;
    Ok(task_field.current_value(&task))
}

pub(crate) async fn task_labels(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = ? ORDER BY label",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|row| row.get("label")).collect())
}

pub(crate) async fn task_snapshot(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
) -> Result<TaskUndoSnapshot> {
    let row = sqlx::query(
        "SELECT t.title, t.description, t.project_id, p.key AS project_key, t.status, t.priority, t.available_at, t.due_on, t.deleted, t.is_epic
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND t.id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("error task-not-found task_id={task_id}"))?;
    let labels = task_labels(conn, workspace_id, task_id).await?;
    let metadata = sqlx::query_as::<_, (String, String)>(
        "SELECT field_id, value FROM task_metadata
         WHERE workspace_id = ? AND task_id = ? ORDER BY field_id",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    Ok(TaskUndoSnapshot {
        title: row.get("title"),
        description: row.get("description"),
        project_id: row.get("project_id"),
        project_key: row.get("project_key"),
        status: row.get("status"),
        priority: row.get("priority"),
        available_at: row.get("available_at"),
        due_on: row.get("due_on"),
        deleted: row.get::<i64, _>("deleted") != 0,
        is_epic: row.get::<i64, _>("is_epic") != 0,
        labels,
        metadata,
    })
}

pub(crate) async fn conflict_row_id(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    field: &str,
) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT id FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0
         ORDER BY id LIMIT 1",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(field)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| {
        CoreError::not_found(format!(
            "error conflict-not-found task_id={task_id} field={field}"
        ))
    })
    .map_err(Into::into)
}

pub(crate) async fn record_tui_undo(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    summary: &str,
    payload: UndoPayload,
) -> Result<()> {
    if is_applying_undo() || !undo_payload_has_effect(&payload) {
        return Ok(());
    }
    let id = new_id();
    let created_at = now();
    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM tui_undo_entries WHERE workspace_id = ?",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    let payload = serde_json::to_string(&payload)?;
    sqlx::query(
        "INSERT INTO tui_undo_entries(id, workspace_id, summary, payload_version, payload, seq, created_at)
         VALUES (?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(&id)
    .bind(workspace_id)
    .bind(summary)
    .bind(&payload)
    .bind(seq)
    .bind(&created_at)
    .execute(&mut *conn)
    .await?;
    prune_consumed_undo_entries(conn, workspace_id).await?;
    Ok(())
}

fn undo_payload_has_effect(payload: &UndoPayload) -> bool {
    payload.commands.iter().any(|command| match command {
        UndoCommand::SetTaskField { before, after, .. } => before != after,
        UndoCommand::SetTaskLabels { before, after, .. } => !label_sets_equal(before, after),
        UndoCommand::SetTaskMetadata { before, after, .. } => before != after,
        UndoCommand::SetProjectMetadata {
            before_key,
            before_name,
            before_prefix,
            after_key,
            after_name,
            after_prefix,
            ..
        } => before_key != after_key || before_name != after_name || before_prefix != after_prefix,
        UndoCommand::SetNoteBody { before, after, .. } => before != after,
        UndoCommand::SetLabelName { before, after } => before != after,
        UndoCommand::DeleteCreatedTask { .. }
        | UndoCommand::RestoreDeletedNote { .. }
        | UndoCommand::DeleteCreatedNote { .. }
        | UndoCommand::DeleteCreatedProject { .. }
        | UndoCommand::DeleteCreatedLabel { .. }
        | UndoCommand::RestoreDeletedLabel { .. }
        | UndoCommand::RestoreConflictResolution { .. }
        | UndoCommand::AddTaskDependency { .. }
        | UndoCommand::RemoveTaskDependency { .. }
        | UndoCommand::SetTaskRelatedLink { .. }
        | UndoCommand::AddEpicChild { .. }
        | UndoCommand::RemoveEpicChild { .. } => true,
    })
}

fn classify_undo_commands(id: String, commands: &[UndoCommand]) -> PendingUndoPresentation {
    let mut task_ids = BTreeSet::new();
    for command in commands {
        match command {
            UndoCommand::SetTaskField { task_id, .. }
            | UndoCommand::SetTaskLabels { task_id, .. }
            | UndoCommand::SetTaskMetadata { task_id, .. }
            | UndoCommand::DeleteCreatedTask { task_id, .. }
            | UndoCommand::SetNoteBody { task_id, .. }
            | UndoCommand::RestoreDeletedNote { task_id, .. }
            | UndoCommand::DeleteCreatedNote { task_id, .. }
            | UndoCommand::RestoreConflictResolution { task_id, .. }
            | UndoCommand::AddTaskDependency { task_id, .. }
            | UndoCommand::RemoveTaskDependency { task_id, .. }
            | UndoCommand::SetTaskRelatedLink { task_id, .. } => {
                task_ids.insert(task_id.clone());
            }
            UndoCommand::AddEpicChild { child_id, .. }
            | UndoCommand::RemoveEpicChild { child_id, .. } => {
                task_ids.insert(child_id.clone());
            }
            UndoCommand::DeleteCreatedProject { .. }
            | UndoCommand::SetProjectMetadata { .. }
            | UndoCommand::DeleteCreatedLabel { .. }
            | UndoCommand::SetLabelName { .. }
            | UndoCommand::RestoreDeletedLabel { .. } => {}
        }
    }

    let operation = if commands
        .iter()
        .any(|command| matches!(command, UndoCommand::DeleteCreatedTask { .. }))
    {
        "task creation"
    } else if commands
        .iter()
        .any(|command| matches!(command, UndoCommand::SetTaskLabels { .. }))
    {
        "label change"
    } else if let Some(command) = commands.iter().find(|command| {
        matches!(
            command,
            UndoCommand::AddEpicChild { .. } | UndoCommand::RemoveEpicChild { .. }
        )
    }) {
        match command {
            UndoCommand::AddEpicChild { .. } => "epic membership addition",
            UndoCommand::RemoveEpicChild { .. } => "epic membership removal",
            _ => unreachable!(),
        }
    } else {
        commands
            .iter()
            .find_map(undo_command_operation)
            .unwrap_or("last TUI mutation")
    };

    PendingUndoPresentation {
        id,
        operation: operation.to_string(),
        task_ids: task_ids.into_iter().collect(),
    }
}

fn undo_command_operation(command: &UndoCommand) -> Option<&'static str> {
    Some(match command {
        UndoCommand::SetTaskField { field, after, .. } => match field.as_str() {
            "title" => "title change",
            "description" => "description change",
            "project" => "project change",
            "status" => "status change",
            "priority" => "priority change",
            "available_at" => "availability change",
            "due_on" => "due date change",
            "deleted" if after == "1" => "task deletion",
            "deleted" => "task restoration",
            "is_epic" => "epic status change",
            _ => "task change",
        },
        UndoCommand::SetTaskLabels { .. } => "label change",
        UndoCommand::SetTaskMetadata { .. } => "metadata change",
        UndoCommand::DeleteCreatedTask { .. } => "task creation",
        UndoCommand::SetNoteBody { .. } => "note edit",
        UndoCommand::RestoreDeletedNote { .. } => "note deletion",
        UndoCommand::DeleteCreatedNote { .. } => "note creation",
        UndoCommand::DeleteCreatedProject { .. } => "project creation",
        UndoCommand::SetProjectMetadata { .. } => "project change",
        UndoCommand::DeleteCreatedLabel { .. } => "label creation",
        UndoCommand::SetLabelName { .. } => "label rename",
        UndoCommand::RestoreDeletedLabel { .. } => "label deletion",
        UndoCommand::RestoreConflictResolution { .. } => "conflict resolution",
        UndoCommand::AddTaskDependency { .. } => "dependency addition",
        UndoCommand::RemoveTaskDependency { .. } => "dependency removal",
        UndoCommand::SetTaskRelatedLink { linked, .. } => {
            if *linked {
                "related link addition"
            } else {
                "related link removal"
            }
        }
        UndoCommand::AddEpicChild { .. } => "epic membership addition",
        UndoCommand::RemoveEpicChild { .. } => "epic membership removal",
    })
}

fn empty_command_outcome() -> CommandOutcome {
    CommandOutcome {
        task_id: None,
        include_deleted: None,
        project_rename: None,
        label_rename: None,
    }
}

async fn prune_consumed_undo_entries(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM tui_undo_entries
         WHERE workspace_id = ? AND undone_at IS NOT NULL AND id NOT IN (
             SELECT id FROM tui_undo_entries
             WHERE workspace_id = ? AND undone_at IS NOT NULL
             ORDER BY undone_at DESC, seq DESC
             LIMIT 20
         )",
    )
    .bind(workspace_id)
    .bind(workspace_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn clear_pending_tui_undo_entries(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("DELETE FROM tui_undo_entries WHERE undone_at IS NULL")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(crate) async fn latest_tui_undo_presentation(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<Option<PendingUndoPresentation>> {
    let row = sqlx::query(
        "SELECT id, payload FROM tui_undo_entries
         WHERE workspace_id = ? AND undone_at IS NULL
         ORDER BY seq DESC
         LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id: String = row.get("id");
    let payload: String = row.get("payload");
    let payload: UndoPayload = serde_json::from_str(&payload)?;
    Ok(Some(classify_undo_commands(id, &payload.commands)))
}

pub(crate) async fn apply_latest_tui_undo(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<Option<UndoOutcome>> {
    let mut tx = begin_immediate(conn).await?;
    let row = sqlx::query(
        "SELECT id, payload FROM tui_undo_entries
         WHERE workspace_id = ? AND undone_at IS NULL
         ORDER BY seq DESC
         LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let entry_id: String = row.get("id");
    let payload_text: String = row.get("payload");
    let undone_at = now();
    let claimed =
        sqlx::query("UPDATE tui_undo_entries SET undone_at = ? WHERE id = ? AND undone_at IS NULL")
            .bind(&undone_at)
            .bind(&entry_id)
            .execute(&mut *tx)
            .await?;
    ensure!(
        claimed.rows_affected() == 1,
        "error undo-entry-claim-failed id={entry_id}"
    );
    let payload: UndoPayload = serde_json::from_str(&payload_text)?;
    let presentation = classify_undo_commands(entry_id.clone(), &payload.commands);
    let apply_result = APPLYING_UNDO
        .scope(
            (),
            apply_undo_commands(&mut tx, workspace_id, &payload.commands),
        )
        .await;
    match apply_result {
        Ok(outcome) => {
            tx.commit().await?;
            Ok(Some(UndoOutcome {
                presentation,
                task_id: outcome.task_id,
                include_deleted: outcome.include_deleted,
                project_rename: outcome.project_rename,
                label_rename: outcome.label_rename,
            }))
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

struct CommandOutcome {
    task_id: Option<crate::ids::TaskId>,
    include_deleted: Option<bool>,
    project_rename: Option<ProjectRenameUndoOutcome>,
    label_rename: Option<LabelRenameUndoOutcome>,
}

async fn apply_undo_commands(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    commands: &[UndoCommand],
) -> Result<CommandOutcome> {
    let mut task_id = None;
    let mut include_deleted = None;
    let mut project_rename = None;
    let mut label_rename = None;
    for command in commands {
        let outcome = apply_undo_command(conn, workspace_id, command).await?;
        if outcome.task_id.is_some() {
            task_id = outcome.task_id;
        }
        if outcome.include_deleted.is_some() {
            include_deleted = outcome.include_deleted;
        }
        if outcome.project_rename.is_some() {
            project_rename = outcome.project_rename;
        }
        if outcome.label_rename.is_some() {
            label_rename = outcome.label_rename;
        }
    }
    Ok(CommandOutcome {
        task_id,
        include_deleted,
        project_rename,
        label_rename,
    })
}

async fn apply_undo_command(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    command: &UndoCommand,
) -> Result<CommandOutcome> {
    match command {
        UndoCommand::SetTaskField {
            task_id,
            field,
            before,
            after,
        } => {
            let task_field = TaskField::parse_or_unknown(field)?;
            let current =
                task_field_value_for_field(conn, workspace_id, task_id, task_field).await?;
            if current != *after {
                bail!("error undo-state-changed task_id={task_id} field={field}");
            }
            if before != after {
                let recurrence_undone = if task_field == TaskField::Status {
                    crate::operations::undo_recurrence_resolution(
                        conn,
                        workspace_id,
                        task_id,
                        before,
                        after,
                    )
                    .await?
                } else {
                    false
                };
                if recurrence_undone {
                    return Ok(CommandOutcome {
                        task_id: Some(task_id.clone()),
                        include_deleted: None,
                        project_rename: None,
                        label_rename: None,
                    });
                }
                if task_field == TaskField::Project {
                    let project_id = before.parse().map_err(|_| {
                        anyhow::anyhow!("error undo-state-changed task_id={task_id} field={field}")
                    })?;
                    if !project_id_exists(conn, workspace_id, &project_id).await? {
                        bail!("error undo-state-changed task_id={task_id} field={field}");
                    }
                }
                set_task_field_in_workspace(conn, workspace_id, task_id, task_field, before)
                    .await?;
            }
            let include_deleted = if task_field == TaskField::Deleted {
                Some(before == "1")
            } else {
                None
            };
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::SetTaskLabels {
            task_id,
            before,
            after,
        } => {
            let current = task_labels(conn, workspace_id, task_id).await?;
            if !label_sets_equal(&current, after) {
                bail!("error undo-state-changed task_id={task_id} field=labels");
            }
            let (add_labels, remove_labels) = label_delta(&current, before);
            update_task_labels_in_workspace(
                conn,
                workspace_id,
                task_id,
                &add_labels,
                &remove_labels,
            )
            .await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::SetTaskMetadata {
            task_id,
            field_id,
            before,
            after,
        } => {
            let current: Option<String> = sqlx::query_scalar(
                "SELECT value FROM task_metadata
                 WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(field_id)
            .fetch_optional(&mut *conn)
            .await?;
            if current != *after {
                bail!("error undo-state-changed task_id={task_id} field=metadata");
            }
            let field = crate::metadata::metadata_field_by_id(conn, workspace_id, field_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("error metadata-field-not-found"))?;
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            if let Some(value) = before {
                crate::metadata::set_task_metadata(
                    conn,
                    &workspace,
                    task_id,
                    &crate::metadata::TaskMetadataInput {
                        key: field.key,
                        value: value.clone(),
                    },
                )
                .await?;
            } else {
                crate::metadata::remove_task_metadata(conn, &workspace, task_id, &field.key)
                    .await?;
            }
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::DeleteCreatedTask {
            task_id,
            create_change_id,
            expected,
            attachment_ids,
            attachment_change_ids,
        } => {
            let current = task_snapshot(conn, workspace_id, task_id).await?;
            if current != *expected {
                bail!("error undo-state-changed task_id={task_id} field=task");
            }
            let current_attachment_ids: Vec<String> = sqlx::query_scalar(
                "SELECT attachment_id FROM task_attachments
                 WHERE workspace_id = ? AND task_id = ? AND deleted = 0
                 ORDER BY created_at, attachment_id",
            )
            .bind(workspace_id)
            .bind(task_id)
            .fetch_all(&mut *conn)
            .await?;
            if let Some(change_id) = create_change_id {
                let labels_clear = expected.labels.is_empty()
                    || labels_match_create_change(conn, change_id, &expected.labels).await?;
                let attachment_changes_clear =
                    all_changes_unsynced(conn, attachment_change_ids).await?;
                let related_state_clear =
                    !crate::operations::task_has_related_state(conn, workspace_id, task_id).await?;
                if change_is_unsynced(conn, change_id).await?
                    && labels_clear
                    && attachment_changes_clear
                    && related_state_clear
                    && current_attachment_ids == *attachment_ids
                {
                    hard_delete_created_task(
                        conn,
                        workspace_id,
                        task_id,
                        change_id,
                        attachment_change_ids,
                    )
                    .await?;
                    return Ok(CommandOutcome {
                        task_id: Some(task_id.clone()),
                        include_deleted: None,
                        project_rename: None,
                        label_rename: None,
                    });
                }
            }
            set_task_field_in_workspace(conn, workspace_id, task_id, TaskField::Deleted, "1")
                .await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::SetNoteBody {
            task_id,
            note_id,
            before,
            after,
        } => {
            let current = sqlx::query_scalar::<_, String>(
                "SELECT body FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?",
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(note_id)
            .fetch_optional(&mut *conn)
            .await?;
            if current.as_deref() != Some(after) {
                bail!("error undo-state-changed task_id={task_id} field=notes");
            }
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            crate::operations::route_recurrence_task_field(conn, &workspace, task_id, "notes", "")
                .await?;
            let edited_at = now();
            sqlx::query(
                "UPDATE notes SET body = ? WHERE workspace_id = ? AND task_id = ? AND id = ?",
            )
            .bind(before)
            .bind(workspace_id)
            .bind(task_id)
            .bind(note_id)
            .execute(&mut *conn)
            .await?;
            crate::change_log::append_change(
                conn,
                crate::change_log::ChangeEntity::Task,
                task_id,
                Some("notes"),
                crate::change_log::op_type::NOTE_EDIT,
                crate::change_log::ChangePayload::workspace(&workspace)
                    .set("note_id", note_id)
                    .set("body", before)
                    .set("edited_at", &edited_at),
            )
            .await?;
            sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
                .bind(&edited_at)
                .bind(workspace_id)
                .bind(task_id)
                .execute(&mut *conn)
                .await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::RestoreDeletedNote {
            task_id,
            note_id,
            body,
            created_at,
        } => {
            let exists: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?",
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(note_id)
            .fetch_one(&mut *conn)
            .await?;
            if exists != 0 {
                bail!("error undo-state-changed task_id={task_id} field=notes");
            }
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            crate::operations::route_recurrence_task_field(conn, &workspace, task_id, "notes", "")
                .await?;
            let change_id = crate::change_log::append_change(
                conn,
                crate::change_log::ChangeEntity::Task,
                task_id,
                Some("notes"),
                crate::change_log::op_type::NOTE_ADD,
                crate::change_log::ChangePayload::workspace(&workspace)
                    .set("note_id", note_id)
                    .set("body", body)
                    .set("created_at", created_at),
            )
            .await?;
            sqlx::query(
                "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(workspace_id)
            .bind(note_id)
            .bind(task_id)
            .bind(body)
            .bind(created_at)
            .bind(change_id)
            .execute(&mut *conn)
            .await?;
            sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
                .bind(now())
                .bind(workspace_id)
                .bind(task_id)
                .execute(&mut *conn)
                .await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::DeleteCreatedNote {
            task_id,
            note_id,
            note_add_change_id,
        } => {
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            crate::operations::route_recurrence_task_field(conn, &workspace, task_id, "notes", "")
                .await?;
            delete_created_note(conn, workspace_id, task_id, note_id, note_add_change_id).await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::DeleteCreatedProject {
            project_key,
            create_change_id,
            expected_name,
            expected_prefix,
        } => {
            delete_created_project(
                conn,
                workspace_id,
                project_key,
                create_change_id,
                expected_name,
                expected_prefix,
            )
            .await?;
            Ok(empty_command_outcome())
        }
        UndoCommand::SetProjectMetadata {
            project_id,
            before_key,
            before_name,
            before_prefix,
            after_key,
            after_name,
            after_prefix,
        } => {
            set_project_metadata_for_undo(
                conn,
                workspace_id,
                project_id,
                ProjectMetadata {
                    key: before_key,
                    name: before_name,
                    prefix: before_prefix,
                },
                ProjectMetadata {
                    key: after_key,
                    name: after_name,
                    prefix: after_prefix,
                },
            )
            .await?;
            Ok(CommandOutcome {
                task_id: None,
                include_deleted: None,
                project_rename: Some(ProjectRenameUndoOutcome {
                    before_key: before_key.clone(),
                    after_key: after_key.clone(),
                }),
                label_rename: None,
            })
        }
        UndoCommand::DeleteCreatedLabel {
            label,
            create_change_id,
        } => {
            delete_created_label(conn, workspace_id, label, create_change_id).await?;
            Ok(empty_command_outcome())
        }
        UndoCommand::SetLabelName { before, after } => {
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            let after_exists = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?",
            )
            .bind(workspace_id)
            .bind(after)
            .fetch_one(&mut *conn)
            .await?
                == 1;
            let before_exists = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?",
            )
            .bind(workspace_id)
            .bind(before)
            .fetch_one(&mut *conn)
            .await?
                > 0;
            if !after_exists || before_exists {
                bail!("error undo-state-changed label={after}");
            }
            crate::operations::set_label_name(conn, &workspace, after, before).await?;
            Ok(CommandOutcome {
                task_id: None,
                include_deleted: None,
                project_rename: None,
                label_rename: Some(LabelRenameUndoOutcome {
                    before: before.clone(),
                    after: after.clone(),
                }),
            })
        }
        UndoCommand::RestoreDeletedLabel {
            name,
            created_at,
            task_ids,
            series_ids,
        } => {
            restore_deleted_label(conn, workspace_id, name, created_at, task_ids, series_ids)
                .await?;
            Ok(empty_command_outcome())
        }
        UndoCommand::RestoreConflictResolution {
            task_id,
            field,
            before,
            after,
            conflict_id,
        } => {
            let task_field = TaskField::parse_or_unknown(field)?;
            let current =
                task_field_value_for_field(conn, workspace_id, task_id, task_field).await?;
            if current != *after {
                bail!("error undo-state-changed task_id={task_id} field={field}");
            }
            set_task_field_in_workspace(conn, workspace_id, task_id, task_field, before).await?;
            let restored = sqlx::query(
                "UPDATE conflicts SET resolved = 0 WHERE id = ? AND workspace_id = ? AND resolved = 1",
            )
            .bind(conflict_id)
            .bind(workspace_id)
            .execute(&mut *conn)
            .await?;
            ensure!(
                restored.rows_affected() == 1,
                "error undo-state-changed task_id={task_id} field={field}"
            );
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::AddTaskDependency {
            task_id,
            depends_on_task_id,
        } => {
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            crate::operations::route_recurrence_task_field(
                conn,
                &workspace,
                task_id,
                "dependencies",
                "",
            )
            .await?;
            crate::operations::route_recurrence_task_field(
                conn,
                &workspace,
                depends_on_task_id,
                "dependencies",
                "",
            )
            .await?;
            ensure!(
                dependency_edge_exists(conn, workspace_id, task_id, depends_on_task_id).await?,
                "error undo-state-changed task_id={task_id} field=dependency"
            );
            remove_dependency_for_undo(conn, workspace_id, task_id, depends_on_task_id).await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::RemoveTaskDependency {
            task_id,
            depends_on_task_id,
        } => {
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            crate::operations::route_recurrence_task_field(
                conn,
                &workspace,
                task_id,
                "dependencies",
                "",
            )
            .await?;
            crate::operations::route_recurrence_task_field(
                conn,
                &workspace,
                depends_on_task_id,
                "dependencies",
                "",
            )
            .await?;
            ensure!(
                !dependency_edge_exists(conn, workspace_id, task_id, depends_on_task_id).await?,
                "error undo-state-changed task_id={task_id} field=dependency"
            );
            ensure!(
                !crate::operations::dependency_path_exists(
                    conn,
                    workspace_id,
                    depends_on_task_id,
                    task_id,
                )
                .await?,
                "error dependency-cycle task_id={task_id} depends_on_task_id={depends_on_task_id}"
            );
            add_dependency_for_undo(conn, workspace_id, task_id, depends_on_task_id).await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::SetTaskRelatedLink {
            task_id,
            related_task_id,
            forward_change_id,
            linked,
        } => {
            let (task_a_id, task_b_id) =
                crate::operations::canonical_related_pair(task_id, related_task_id)?;
            let current_change_id = sqlx::query_scalar::<_, String>(
                "SELECT last_change_id FROM task_related_links
                 WHERE workspace_id = ? AND task_a_id = ? AND task_b_id = ?",
            )
            .bind(workspace_id)
            .bind(task_a_id)
            .bind(task_b_id)
            .fetch_optional(&mut *conn)
            .await?;
            ensure!(
                current_change_id.as_deref() == Some(forward_change_id),
                "error undo-state-changed task_id={task_id} field=related"
            );
            let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
            crate::operations::set_task_related_link_in_transaction(
                conn,
                &workspace,
                task_id,
                related_task_id,
                !linked,
            )
            .await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::AddEpicChild { epic_id, child_id } => {
            ensure!(
                epic_edge_exists(conn, workspace_id, epic_id, child_id).await?,
                "error undo-state-changed task_id={child_id} field=epic"
            );
            set_epic_edge_for_undo(conn, workspace_id, epic_id, child_id, false).await?;
            Ok(CommandOutcome {
                task_id: Some(child_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
        UndoCommand::RemoveEpicChild { epic_id, child_id } => {
            ensure!(
                !epic_edge_exists(conn, workspace_id, epic_id, child_id).await?,
                "error undo-state-changed task_id={child_id} field=epic"
            );
            set_epic_edge_for_undo(conn, workspace_id, epic_id, child_id, true).await?;
            Ok(CommandOutcome {
                task_id: Some(child_id.clone()),
                include_deleted: None,
                project_rename: None,
                label_rename: None,
            })
        }
    }
}

async fn project_id_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    project_id: &ProjectId,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects
         WHERE workspace_id = ? AND id = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

async fn set_task_field_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    task_field: TaskField,
    value: &str,
) -> Result<()> {
    let field = task_field.as_str();
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    crate::operations::route_recurrence_task_field(conn, &workspace, task_id, field, value).await?;
    if conflict_exists(conn, workspace_id, task_id, field).await? {
        bail!(
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            task_id,
            field
        );
    }
    let base = field_version(conn, task_id, field).await?;
    let workspace_key = workspace_key_for_id(conn, workspace_id).await?;
    let payload = if task_field.is_project() {
        let project = resolve_project_for_stored_value(conn, workspace_id, value).await?;
        apply_project_id_in_workspace(conn, workspace_id, task_id, &project.id).await?;
        TaskField::project_payload(workspace_id, &workspace_key, &project)
    } else {
        apply_field_value_in_workspace(conn, workspace_id, task_id, field, value).await?;
        task_field.scalar_payload(workspace_id, &workspace_key, value)?
    };
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(field),
        "set_field",
        payload,
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, task_id, field, &change_id).await?;
    Ok(())
}

fn label_sets_equal(left: &[String], right: &[String]) -> bool {
    let left: BTreeSet<_> = left.iter().collect();
    let right: BTreeSet<_> = right.iter().collect();
    left == right
}

fn label_delta(current: &[String], target: &[String]) -> (Vec<String>, Vec<String>) {
    let current_set: BTreeSet<_> = current.iter().collect();
    let target_set: BTreeSet<_> = target.iter().collect();
    let add = target
        .iter()
        .filter(|label| !current_set.contains(label))
        .cloned()
        .collect();
    let remove = current
        .iter()
        .filter(|label| !target_set.contains(label))
        .cloned()
        .collect();
    (add, remove)
}

async fn change_is_unsynced(conn: &mut SqliteConnection, change_id: &str) -> Result<bool> {
    let server_seq =
        sqlx::query_scalar::<_, Option<i64>>("SELECT server_seq FROM changes WHERE change_id = ?")
            .bind(change_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(matches!(server_seq, Some(None)))
}

async fn all_changes_unsynced(conn: &mut SqliteConnection, change_ids: &[String]) -> Result<bool> {
    for change_id in change_ids {
        if !change_is_unsynced(conn, change_id).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn labels_match_create_change(
    conn: &mut SqliteConnection,
    change_id: &str,
    labels: &[String],
) -> Result<bool> {
    let payload: String = sqlx::query_scalar("SELECT payload FROM changes WHERE change_id = ?")
        .bind(change_id)
        .fetch_one(&mut *conn)
        .await?;
    let payload: serde_json::Value = serde_json::from_str(&payload)?;
    let payload_labels = payload
        .get("labels")
        .and_then(|labels| labels.as_array())
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(label_sets_equal(labels, &payload_labels))
}

async fn hard_delete_created_task(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    create_change_id: &str,
    attachment_change_ids: &[String],
) -> Result<()> {
    sqlx::query("DELETE FROM task_attachments WHERE workspace_id = ? AND task_id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM task_metadata WHERE workspace_id = ? AND task_id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM field_versions WHERE entity_id = ?")
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM tasks WHERE workspace_id = ? AND id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    for change_id in attachment_change_ids {
        sqlx::query("DELETE FROM changes WHERE change_id = ?")
            .bind(change_id)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(create_change_id)
        .execute(&mut *conn)
        .await?;
    crate::attachments::lifecycle::reconcile_liveness_in_transaction(
        conn,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    Ok(())
}

async fn delete_created_note(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    note_id: &str,
    note_add_change_id: &str,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT change_id FROM notes WHERE workspace_id = ? AND id = ? AND task_id = ?",
    )
    .bind(workspace_id)
    .bind(note_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        bail!("error undo-state-changed task_id={task_id} field=note");
    };
    let stored_change_id: String = row.get("change_id");
    if stored_change_id != note_add_change_id {
        bail!("error undo-state-changed task_id={task_id} field=note");
    }
    if !change_is_unsynced(conn, note_add_change_id).await? {
        bail!("error undo-state-changed task_id={task_id} field=note");
    }
    sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND id = ? AND task_id = ?")
        .bind(workspace_id)
        .bind(note_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(note_add_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn delete_created_project(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    project_key: &str,
    create_change_id: &str,
    expected_name: &str,
    expected_prefix: &str,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT id, name, prefix FROM projects WHERE workspace_id = ? AND key = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_key)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        bail!("error undo-state-changed project_key={project_key}");
    };
    let project_id: ProjectId = row.get("id");
    let name: String = row.get("name");
    let prefix: String = row.get("prefix");
    if name != expected_name || prefix != expected_prefix {
        bail!("error undo-state-changed project_key={project_key}");
    }
    if !change_is_unsynced(conn, create_change_id).await? {
        bail!("error undo-state-changed project_key={project_key}");
    }
    let task_refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND project_id = ?")
            .bind(workspace_id)
            .bind(&project_id)
            .fetch_one(&mut *conn)
            .await?;
    if task_refs > 0 {
        bail!("error undo-state-changed project_key={project_key}");
    }
    let path_refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_paths WHERE workspace_id = ? AND project_id = ?",
    )
    .bind(workspace_id)
    .bind(&project_id)
    .fetch_one(&mut *conn)
    .await?;
    if path_refs > 0 {
        bail!("error undo-state-changed project_key={project_key}");
    }
    sqlx::query("DELETE FROM projects WHERE workspace_id = ? AND key = ?")
        .bind(workspace_id)
        .bind(project_key)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(create_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn set_project_metadata_for_undo(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    project_id: &ProjectId,
    before: ProjectMetadata<'_>,
    after: ProjectMetadata<'_>,
) -> Result<()> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let row = sqlx::query(
        "SELECT key, name, prefix
         FROM projects
         WHERE workspace_id = ? AND id = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        bail!("error undo-state-changed project_id={project_id}");
    };
    let key: String = row.get("key");
    let name: String = row.get("name");
    let prefix: String = row.get("prefix");
    if key != after.key || name != after.name || prefix != after.prefix {
        bail!("error undo-state-changed project_id={project_id}");
    }
    let key_refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM projects
         WHERE workspace_id = ? AND key = ? AND id != ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(before.key)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?;
    if key_refs > 0 {
        bail!("error undo-state-changed project_id={project_id}");
    }
    let prefix_refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM projects
         WHERE workspace_id = ? AND prefix = ? AND id != ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(before.prefix)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?;
    if prefix_refs > 0 {
        bail!("error undo-state-changed project_id={project_id}");
    }
    set_project_metadata(conn, &workspace, project_id, before, false).await?;
    insert_project_metadata_change(conn, &workspace, project_id, before, &now()).await?;
    Ok(())
}

async fn delete_created_label(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    label: &str,
    create_change_id: &str,
) -> Result<()> {
    let exists: i64 =
        sqlx::query_scalar("SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?")
            .bind(workspace_id)
            .bind(label)
            .fetch_one(&mut *conn)
            .await?;
    if exists == 0 || !change_is_unsynced(conn, create_change_id).await? {
        bail!("error undo-state-changed label={label}");
    }
    let task_refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE workspace_id = ? AND label = ?")
            .bind(workspace_id)
            .bind(label)
            .fetch_one(&mut *conn)
            .await?;
    let series_refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_series_labels WHERE workspace_id = ? AND label = ?",
    )
    .bind(workspace_id)
    .bind(label)
    .fetch_one(&mut *conn)
    .await?;
    if task_refs > 0 || series_refs > 0 {
        bail!("error undo-state-changed label={label}");
    }
    sqlx::query("DELETE FROM labels WHERE workspace_id = ? AND name = ?")
        .bind(workspace_id)
        .bind(label)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(create_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn restore_deleted_label(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
    created_at: &str,
    task_ids: &[crate::ids::TaskId],
    series_ids: &[crate::recurrence::RecurrenceSeriesId],
) -> Result<()> {
    let exists: i64 =
        sqlx::query_scalar("SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?")
            .bind(workspace_id)
            .bind(name)
            .fetch_one(&mut *conn)
            .await?;
    if exists > 0 {
        bail!("error undo-state-changed label={name}");
    }
    sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)")
        .bind(workspace_id)
        .bind(name)
        .bind(created_at)
        .execute(&mut *conn)
        .await?;
    for task_id in task_ids {
        let inserted = sqlx::query(
            "INSERT INTO task_labels(workspace_id, task_id, label)
             SELECT ?, id, ? FROM tasks WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
        if inserted.rows_affected() != 1 {
            bail!("error undo-state-changed label={name} task_id={task_id}");
        }
    }
    for series_id in series_ids {
        let inserted = sqlx::query(
            "INSERT INTO recurrence_series_labels(workspace_id, series_id, label)
             SELECT ?, id, ? FROM recurrence_series WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(workspace_id)
        .bind(series_id)
        .execute(&mut *conn)
        .await?;
        if inserted.rows_affected() != 1 {
            bail!("error undo-state-changed label={name} series_id={series_id}");
        }
    }
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    append_change(
        conn,
        ChangeEntity::Label,
        name,
        None,
        op_type::LABEL_RESTORE,
        ChangePayload::workspace(&workspace)
            .set("name", name)
            .set("created_at", created_at)
            .set("task_ids", task_ids)
            .set("series_ids", series_ids)
            .set("restored_at", now()),
    )
    .await?;
    Ok(())
}

async fn dependency_task_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tasks WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await?
            > 0,
    )
}

async fn dependency_edge_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    depends_on_task_id: &crate::ids::TaskId,
) -> Result<bool> {
    ensure!(
        dependency_task_exists(conn, workspace_id, task_id).await?
            && dependency_task_exists(conn, workspace_id, depends_on_task_id).await?,
        "error undo-state-changed task_id={task_id} field=dependency"
    );
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_dependencies
         WHERE workspace_id = ? AND task_id = ? AND depends_on_task_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(depends_on_task_id)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

async fn epic_edge_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    epic_id: &crate::ids::TaskId,
    child_id: &crate::ids::TaskId,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_epic_links
         WHERE workspace_id = ? AND epic_task_id = ? AND child_task_id = ?",
    )
    .bind(workspace_id)
    .bind(epic_id)
    .bind(child_id)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

async fn set_epic_edge_for_undo(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    epic_id: &crate::ids::TaskId,
    child_id: &crate::ids::TaskId,
    linked: bool,
) -> Result<()> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let outcome = if linked {
        crate::operations::restore_task_to_epic_in_transaction(conn, &workspace, child_id, epic_id)
            .await?
    } else {
        crate::operations::remove_task_from_epic_in_transaction(conn, &workspace, child_id, epic_id)
            .await?
    };
    ensure!(
        outcome.changed,
        "error undo-state-changed task_id={child_id} field=epic"
    );
    Ok(())
}

async fn add_dependency_for_undo(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    depends_on_task_id: &crate::ids::TaskId,
) -> Result<()> {
    let created_at = now();
    sqlx::query(
        "INSERT OR IGNORE INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(depends_on_task_id)
    .bind(&created_at)
    .execute(&mut *conn)
    .await?;
    append_dependency_change(
        conn,
        workspace_id,
        task_id,
        depends_on_task_id,
        crate::change_log::op_type::DEPENDENCY_ADD,
    )
    .await
}

async fn remove_dependency_for_undo(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    depends_on_task_id: &crate::ids::TaskId,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM task_dependencies
         WHERE workspace_id = ? AND task_id = ? AND depends_on_task_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(depends_on_task_id)
    .execute(&mut *conn)
    .await?;
    append_dependency_change(
        conn,
        workspace_id,
        task_id,
        depends_on_task_id,
        crate::change_log::op_type::DEPENDENCY_REMOVE,
    )
    .await
}

async fn append_dependency_change(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    depends_on_task_id: &crate::ids::TaskId,
    op_type: &'static str,
) -> Result<()> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    crate::change_log::append_change(
        conn,
        crate::change_log::ChangeEntity::Task,
        task_id,
        Some("dependencies"),
        op_type,
        crate::change_log::ChangePayload::workspace(&workspace)
            .set("depends_on_task_id", depends_on_task_id.to_string()),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod presentation_tests {
    use super::*;

    fn set_field(task_id: &crate::ids::TaskId, field: &str) -> UndoCommand {
        UndoCommand::SetTaskField {
            task_id: task_id.clone(),
            field: field.to_string(),
            before: "before".to_string(),
            after: "after".to_string(),
        }
    }

    #[test]
    fn classifier_derives_operations_and_unique_task_scope() {
        let first = crate::ids::TaskId::new();
        let second = crate::ids::TaskId::new();
        let cases = [
            (vec![set_field(&first, "priority")], "priority change", 1),
            (
                vec![
                    set_field(&first, "status"),
                    set_field(&first, "status"),
                    set_field(&second, "status"),
                ],
                "status change",
                2,
            ),
            (
                vec![UndoCommand::SetTaskMetadata {
                    task_id: first.clone(),
                    field_id: crate::ids::MetadataFieldId::new(),
                    before: None,
                    after: Some("value".to_string()),
                }],
                "metadata change",
                1,
            ),
        ];

        for (commands, operation, task_count) in cases {
            let presentation = classify_undo_commands("entry".to_string(), &commands);
            assert_eq!(presentation.operation, operation);
            assert_eq!(presentation.task_ids.len(), task_count);
        }
    }

    #[test]
    fn classifier_describes_related_link_changes_on_the_initiating_task() {
        let task_id = crate::ids::TaskId::new();
        let related_task_id = crate::ids::TaskId::new();

        for (linked, operation) in [
            (true, "related link addition"),
            (false, "related link removal"),
        ] {
            let presentation = classify_undo_commands(
                "entry".to_string(),
                &[UndoCommand::SetTaskRelatedLink {
                    task_id: task_id.clone(),
                    related_task_id: related_task_id.clone(),
                    forward_change_id: "change".to_string(),
                    linked,
                }],
            );

            assert_eq!(presentation.operation, operation);
            assert_eq!(presentation.task_ids, vec![task_id.clone()]);
        }
    }

    #[test]
    fn classifier_uses_primary_intent_for_mixed_payloads() {
        let task_id = crate::ids::TaskId::new();
        let project_id = ProjectId::new();
        let creation = UndoCommand::DeleteCreatedTask {
            task_id: task_id.clone(),
            create_change_id: None,
            expected: TaskUndoSnapshot {
                title: "title".to_string(),
                description: String::new(),
                project_id,
                project_key: "project".to_string(),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                available_at: String::new(),
                due_on: String::new(),
                deleted: false,
                is_epic: false,
                labels: vec!["new".to_string()],
                metadata: BTreeMap::new(),
            },
            attachment_ids: Vec::new(),
            attachment_change_ids: Vec::new(),
        };
        let cleanup = UndoCommand::DeleteCreatedLabel {
            label: "new".to_string(),
            create_change_id: "change".to_string(),
        };
        let presentation =
            classify_undo_commands("entry".to_string(), &[creation, cleanup.clone()]);
        assert_eq!(presentation.operation, "task creation");

        let label_change = UndoCommand::SetTaskLabels {
            task_id,
            before: Vec::new(),
            after: vec!["new".to_string()],
        };
        let presentation = classify_undo_commands("entry".to_string(), &[label_change, cleanup]);
        assert_eq!(presentation.operation, "label change");

        let epic_id = crate::ids::TaskId::new();
        let child_id = crate::ids::TaskId::new();
        let presentation = classify_undo_commands(
            "entry".to_string(),
            &[
                set_field(&epic_id, "is_epic"),
                UndoCommand::AddEpicChild { epic_id, child_id },
            ],
        );
        assert_eq!(presentation.operation, "epic membership addition");
    }
}
