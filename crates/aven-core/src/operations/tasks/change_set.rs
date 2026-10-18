use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Result, ensure};
use sqlx::SqliteConnection;

use super::creation::{initial_status, insert_task, validate_task_draft};
use super::mutation::{
    append_task_undo_commands, apply_task_update, update_requests_inbox_promotion,
    validate_task_update,
};
use super::notes::add_note_in_transaction;
use super::{TaskDraft, TaskUpdate};
use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{Database, begin_immediate, conflict_exists};
use crate::error::CoreError;
use crate::ids::{ProjectId, TaskId, WorkspaceId};
use crate::labels::resolve_labels_in_workspace;
use crate::projects::{find_project_by_id_in_workspace, resolve_existing_project_in_workspace};
use crate::refs::{DisplayRefContext, get_task_in_workspace};
use crate::task_fields::TaskField;
use crate::types::{Project, Task};
use crate::undo::{TaskUndoSnapshot, UndoCommand, apply_consumer_undo_commands, task_snapshot};
use crate::workspaces::{Workspace, workspace_for_id};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChangeTarget {
    Existing(TaskId),
    /// A nonempty local ID introduced by an earlier Create in the same set.
    Local(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedTaskRef {
    pub task_id: TaskId,
    pub display_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSetOutcome {
    pub created: BTreeMap<String, CreatedTaskRef>,
    pub undo_token: String,
}

pub(crate) enum ChangeSetOperation {
    Create {
        local_id: String,
        draft: TaskDraft,
    },
    Update {
        target: ChangeTarget,
        project_id: Option<ProjectId>,
        update: TaskUpdate,
    },
    AddNote {
        target: ChangeTarget,
        body: String,
    },
}

pub(crate) struct IndexedChangeSetError {
    pub operation_index: Option<usize>,
    pub error: anyhow::Error,
}

impl From<anyhow::Error> for IndexedChangeSetError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            operation_index: None,
            error,
        }
    }
}

impl IndexedChangeSetError {
    fn at(index: usize, error: anyhow::Error) -> Self {
        Self {
            operation_index: Some(index),
            error,
        }
    }
}

pub(crate) struct AppliedChangeSet {
    pub outcome: ChangeSetOutcome,
    pub note_ids: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename = "change_set")]
pub(crate) struct ChangeSetUndoToken {
    workspace_id: WorkspaceId,
    expected: BTreeMap<TaskId, TaskUndoSnapshot>,
    notes: Vec<NoteGuard>,
    groups: Vec<InverseGroup>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NoteGuard {
    task_id: TaskId,
    note_id: String,
    body: String,
    change_id: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct InverseGroup {
    task_id: TaskId,
    queue_activity_before: Option<String>,
    commands: Vec<UndoCommand>,
}

impl Database {
    pub(crate) async fn apply_consumer_change_set(
        &self,
        workspace_id: &WorkspaceId,
        ops: Vec<ChangeSetOperation>,
    ) -> std::result::Result<AppliedChangeSet, IndexedChangeSetError> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn)
            .await
            .map_err(anyhow::Error::from)?;
        let workspace = workspace_for_id(&mut tx, workspace_id).await?;
        validate_operations(&mut tx, &workspace, &ops).await?;
        let mut locals = BTreeMap::new();
        let mut token = ChangeSetUndoToken {
            workspace_id: workspace_id.clone(),
            expected: BTreeMap::new(),
            notes: Vec::new(),
            groups: Vec::new(),
        };
        for (index, op) in ops.into_iter().enumerate() {
            execute_operation(&mut tx, &workspace, op, &mut locals, &mut token)
                .await
                .map_err(|error| IndexedChangeSetError::at(index, error))?;
        }
        // Final guards are checked once, not against intermediate states of repeated edits.
        for group in &token.groups {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                token.expected.entry(group.task_id.clone())
            {
                entry.insert(task_snapshot(&mut tx, workspace_id, &group.task_id).await?);
            }
        }
        let refs = DisplayRefContext::for_workspace(&mut tx, workspace_id).await?;
        let mut created = BTreeMap::new();
        for (local_id, task_id) in locals {
            let task = get_task_in_workspace(&mut tx, &workspace, &task_id).await?;
            created.insert(
                local_id,
                CreatedTaskRef {
                    task_id,
                    display_ref: refs.display_ref(&task),
                },
            );
        }
        let undo_token = serde_json::to_string(&token).map_err(anyhow::Error::from)?;
        let note_ids = token.notes.into_iter().map(|note| note.note_id).collect();
        tx.commit().await.map_err(anyhow::Error::from)?;
        Ok(AppliedChangeSet {
            outcome: ChangeSetOutcome {
                created,
                undo_token,
            },
            note_ids,
        })
    }

    pub(crate) async fn undo_consumer_change_set(&self, token: ChangeSetUndoToken) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        workspace_for_id(&mut tx, &token.workspace_id).await?;
        for (task_id, expected) in &token.expected {
            let current = task_snapshot(&mut tx, &token.workspace_id, task_id).await?;
            ensure!(
                current == *expected,
                CoreError::generation_conflict("task changed after the change set")
            );
        }
        for note in &token.notes {
            let current: Option<(String, String)> = sqlx::query_as(
                "SELECT body, change_id FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?",
            ).bind(&token.workspace_id).bind(&note.task_id).bind(&note.note_id)
                .fetch_optional(&mut *tx).await?;
            ensure!(
                current
                    .as_ref()
                    .is_some_and(|(body, change)| body == &note.body && change == &note.change_id),
                CoreError::generation_conflict("note changed after the change set")
            );
        }
        for group in token.groups.into_iter().rev() {
            apply_consumer_undo_commands(&mut tx, &token.workspace_id, &group.commands).await?;
            if let Some(activity) = group.queue_activity_before {
                sqlx::query(
                    "UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?",
                )
                .bind(activity)
                .bind(&token.workspace_id)
                .bind(&group.task_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn validate_operations(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    ops: &[ChangeSetOperation],
) -> std::result::Result<(), IndexedChangeSetError> {
    let mut tasks = HashMap::<ChangeTarget, Task>::new();
    let at = crate::ids::now_utc();
    for (index, op) in ops.iter().enumerate() {
        let result: Result<()> = async {
            match op {
                ChangeSetOperation::Create { local_id, draft } => {
                    ensure!(
                        !local_id.trim().is_empty(),
                        CoreError::validation("local ID must not be empty")
                    );
                    let target = ChangeTarget::Local(local_id.clone());
                    ensure!(
                        !tasks.contains_key(&target),
                        CoreError::validation("duplicate local ID")
                    );
                    validate_task_draft(draft)?;
                    let project = resolve_existing_project_in_workspace(
                        conn,
                        &workspace.id,
                        draft
                            .project
                            .as_deref()
                            .expect("capture requires a project"),
                    )
                    .await?;
                    resolve_labels_in_workspace(conn, &workspace.id, &draft.labels).await?;
                    let (status, priority) = initial_status(draft)?;
                    tasks.insert(
                        target,
                        Task {
                            id: TaskId::new(),
                            workspace_id: workspace.id.clone(),
                            title: draft.title.clone(),
                            description: draft.description.clone(),
                            project_id: project.id,
                            project_key: project.key,
                            project_prefix: project.prefix,
                            status,
                            priority,
                            source: draft.source,
                            created_at: String::new(),
                            updated_at: String::new(),
                            queue_activity_at: String::new(),
                            available_at: draft.available_at.clone(),
                            due_on: draft.due_on.clone(),
                            deleted: false,
                            is_epic: false,
                        },
                    );
                }
                ChangeSetOperation::Update {
                    target,
                    project_id,
                    update,
                } => {
                    let task = effective_task(conn, workspace, target, &mut tasks).await?;
                    validate_task_update(update)?;
                    let project = match project_id {
                        Some(id) => Some(require_project(conn, workspace, id).await?),
                        None => None,
                    };
                    let fields = consumer_update_fields(task, project.as_ref(), update);
                    let target_status = fields
                        .iter()
                        .find(|(field, _)| *field == TaskField::Status)
                        .map(|(_, value)| TaskStatus::parse(value))
                        .transpose()?;
                    if matches!(target, ChangeTarget::Existing(_)) {
                        crate::operations::recurrence::validate_consumer_recurrence_mutation(
                            conn,
                            workspace,
                            &task.id,
                            task.status,
                            target_status,
                            at,
                        )
                        .await?;
                    }
                    for (field, value) in fields {
                        if field.current_value(task) != value
                            && matches!(target, ChangeTarget::Existing(_))
                        {
                            ensure!(
                                !conflict_exists(conn, &workspace.id, &task.id, field.as_str())
                                    .await?,
                                CoreError::open_conflict(format!(
                                    "error conflicted-field ref={} field={}",
                                    task.id,
                                    field.as_str()
                                ))
                            );
                        }
                        set_effective_field(task, field, value)?;
                    }
                    resolve_labels_in_workspace(conn, &workspace.id, &update.add_labels).await?;
                    resolve_labels_in_workspace(conn, &workspace.id, &update.remove_labels).await?;
                    if matches!(target, ChangeTarget::Existing(_)) {
                        crate::metadata::validate_task_metadata_result(
                            conn,
                            &workspace.id,
                            &task.id,
                            &[],
                            &[],
                        )
                        .await?;
                    }
                }
                ChangeSetOperation::AddNote { target, .. } => {
                    let task = effective_task(conn, workspace, target, &mut tasks).await?;
                    if matches!(target, ChangeTarget::Existing(_)) {
                        crate::operations::recurrence::validate_consumer_recurrence_mutation(
                            conn,
                            workspace,
                            &task.id,
                            task.status,
                            None,
                            at,
                        )
                        .await?;
                    }
                }
            }
            Ok(())
        }
        .await;
        result.map_err(|error| IndexedChangeSetError::at(index, error))?;
    }
    Ok(())
}

async fn effective_task<'a>(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    target: &ChangeTarget,
    tasks: &'a mut HashMap<ChangeTarget, Task>,
) -> Result<&'a mut Task> {
    if !tasks.contains_key(target) {
        let ChangeTarget::Existing(task_id) = target else {
            return Err(
                CoreError::validation("local target must refer to an earlier create").into(),
            );
        };
        let task = get_task_in_workspace(conn, workspace, task_id).await?;
        ensure!(!task.deleted, CoreError::not_found("task is deleted"));
        tasks.insert(target.clone(), task);
    }
    Ok(tasks.get_mut(target).expect("effective task loaded"))
}

async fn require_project(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    id: &ProjectId,
) -> Result<Project> {
    find_project_by_id_in_workspace(conn, &workspace.id, id)
        .await?
        .ok_or_else(|| CoreError::not_found("selected project is unavailable").into())
}

fn consumer_update_fields(
    task: &Task,
    project: Option<&Project>,
    update: &TaskUpdate,
) -> Vec<(TaskField, String)> {
    let mut fields = Vec::new();
    if let Some(project) = project {
        fields.push((TaskField::Project, project.id.to_string()));
    }
    if let Some(value) = &update.title {
        fields.push((TaskField::Title, value.clone()));
    }
    if let Some(value) = &update.description {
        fields.push((TaskField::Description, value.clone()));
    }
    let promote = task.status == TaskStatus::Inbox && update_requests_inbox_promotion(update);
    if promote {
        fields.push((TaskField::Status, "todo".into()));
    } else if let Some(value) = &update.status {
        fields.push((TaskField::Status, value.clone()));
    }
    if let Some(value) = &update.priority {
        fields.push((TaskField::Priority, value.clone()));
    }
    if let Some(value) = &update.available_at {
        fields.push((TaskField::AvailableAt, value.clone().unwrap_or_default()));
    }
    if let Some(value) = &update.due_on {
        fields.push((TaskField::DueOn, value.clone().unwrap_or_default()));
    }
    fields
}

fn set_effective_field(task: &mut Task, field: TaskField, value: String) -> Result<()> {
    match field {
        TaskField::Title => task.title = value,
        TaskField::Description => task.description = value,
        TaskField::Project => task.project_id = value.parse()?,
        TaskField::Status => task.status = TaskStatus::parse(&value)?,
        TaskField::Priority => task.priority = TaskPriority::parse(&value)?,
        TaskField::AvailableAt => task.available_at = (!value.is_empty()).then_some(value),
        TaskField::DueOn => task.due_on = (!value.is_empty()).then_some(value),
        TaskField::Deleted | TaskField::IsEpic => {
            unreachable!("consumer edit does not expose these fields")
        }
    }
    Ok(())
}

fn resolve_target(target: ChangeTarget, locals: &BTreeMap<String, TaskId>) -> TaskId {
    match target {
        ChangeTarget::Existing(id) => id,
        ChangeTarget::Local(local) => locals.get(&local).expect("local target validated").clone(),
    }
}

async fn execute_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    op: ChangeSetOperation,
    locals: &mut BTreeMap<String, TaskId>,
    token: &mut ChangeSetUndoToken,
) -> Result<()> {
    let group = match op {
        ChangeSetOperation::Create { local_id, draft } => {
            let inserted = insert_task(conn, workspace, draft, false, true).await?;
            let expected = task_snapshot(conn, &workspace.id, &inserted.id).await?;
            locals.insert(local_id, inserted.id.clone());
            InverseGroup {
                task_id: inserted.id.clone(),
                queue_activity_before: None,
                commands: vec![UndoCommand::DeleteCreatedTask {
                    task_id: inserted.id,
                    create_change_id: None,
                    expected,
                    attachment_ids: Vec::new(),
                    attachment_change_ids: Vec::new(),
                }],
            }
        }
        ChangeSetOperation::Update {
            target,
            project_id,
            update,
        } => {
            let task_id = resolve_target(target, locals);
            let before = task_snapshot(conn, &workspace.id, &task_id).await?;
            if let Some(id) = project_id {
                let project = require_project(conn, workspace, &id).await?;
                crate::mutation::set_task_project(conn, workspace, &task_id, &project).await?;
            }
            apply_task_update(conn, workspace, &task_id, &update, &mut BTreeSet::new()).await?;
            let after = task_snapshot(conn, &workspace.id, &task_id).await?;
            let mut commands = Vec::new();
            append_task_undo_commands(&task_id, &before, &after, &mut commands);
            InverseGroup {
                task_id,
                queue_activity_before: Some(before.queue_activity_at),
                commands,
            }
        }
        ChangeSetOperation::AddNote { target, body } => {
            let task_id = resolve_target(target, locals);
            let before = get_task_in_workspace(conn, workspace, &task_id).await?;
            let note = add_note_in_transaction(conn, workspace, &task_id, body.clone()).await?;
            token.notes.push(NoteGuard {
                task_id: task_id.clone(),
                note_id: note.note_id.clone(),
                body,
                change_id: note.change_id.clone(),
            });
            InverseGroup {
                task_id: task_id.clone(),
                queue_activity_before: Some(before.queue_activity_at),
                commands: vec![UndoCommand::DeleteCreatedNote {
                    task_id,
                    note_id: note.note_id,
                    note_add_change_id: note.change_id,
                    restoration_change_ids: Vec::new(),
                }],
            }
        }
    };
    if !group.commands.is_empty() {
        token.groups.push(group);
    }
    Ok(())
}
