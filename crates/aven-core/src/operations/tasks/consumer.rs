use super::TaskUpdate;
use super::mutation::{apply_task_update, validate_task_update};
use crate::db::{Database, begin_immediate};
use crate::ids::TaskId;
use crate::mutation::set_task_project;
use crate::refs::get_task_in_workspace;
use crate::undo::{TaskUndoSnapshot, task_snapshot};
use crate::workspaces::Workspace;
use anyhow::Result;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ConsumerTaskMutation {
    Start,
    Done,
    DetailStatus {
        status: String,
        expected_version: Option<String>,
    },
    DeletedState {
        deleted: bool,
        expected_version: Option<String>,
    },
    Snooze {
        available_at: String,
    },
    SetPriority {
        priority: String,
    },
}

pub(crate) struct ConsumerTaskMutationOutcome {
    pub field_version: Option<String>,
    pub before: TaskUndoSnapshot,
    pub after: TaskUndoSnapshot,
    pub changed: bool,
}

impl Database {
    pub(crate) async fn undo_queue_capture(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        expected: &TaskUndoSnapshot,
    ) -> Result<bool> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let current = task_snapshot(&mut tx, &workspace.id, task_id).await?;
        if current != *expected || current.deleted {
            return Err(crate::error::CoreError::generation_conflict(
                "captured task changed before undo",
            )
            .into());
        }
        let mut affected_attachment_hashes = BTreeSet::new();
        let (changed, _) = apply_task_update(
            &mut tx,
            workspace,
            task_id,
            &TaskUpdate {
                deleted: Some(true),
                ..TaskUpdate::default()
            },
            &mut affected_attachment_hashes,
        )
        .await?;
        let affected_attachment_hashes = affected_attachment_hashes.into_iter().collect::<Vec<_>>();
        crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
            &mut tx,
            &affected_attachment_hashes,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        tx.commit().await?;
        Ok(changed)
    }

    pub(crate) async fn edit_consumer_task(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        project_id: Option<&crate::ids::ProjectId>,
        update: TaskUpdate,
    ) -> Result<bool> {
        validate_task_update(&update)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
        if task.deleted {
            return Err(crate::error::CoreError::not_found("task is deleted").into());
        }
        let mut changed = false;
        if let Some(project_id) = project_id {
            let project = crate::projects::find_project_by_id_in_workspace(
                &mut tx,
                &workspace.id,
                project_id,
            )
            .await?
            .ok_or_else(|| crate::error::CoreError::not_found("selected project is unavailable"))?;
            changed |= set_task_project(&mut tx, workspace, task_id, &project).await?;
        }
        // Consumer edits retain shared workspace labels and have no undo receipt.
        let (fields_changed, _created_labels) =
            apply_task_update(&mut tx, workspace, task_id, &update, &mut BTreeSet::new()).await?;
        tx.commit().await?;
        Ok(changed || fields_changed)
    }

    pub(crate) async fn mutate_consumer_task(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        mutation: &ConsumerTaskMutation,
    ) -> Result<ConsumerTaskMutationOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        if !matches!(mutation, ConsumerTaskMutation::DeletedState { .. }) {
            let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
            if task.deleted {
                return Err(crate::error::CoreError::not_found("task is deleted").into());
            }
        }
        let before = task_snapshot(&mut tx, &workspace.id, task_id).await?;
        let open = !matches!(before.status.as_str(), "done" | "canceled");
        let update = match mutation {
            ConsumerTaskMutation::DetailStatus { status, .. } => TaskUpdate {
                status: Some(status.clone()),
                ..TaskUpdate::default()
            },
            ConsumerTaskMutation::DeletedState { deleted, .. } => TaskUpdate {
                deleted: Some(*deleted),
                ..TaskUpdate::default()
            },
            ConsumerTaskMutation::Start
                if matches!(before.status.as_str(), "inbox" | "backlog" | "todo") =>
            {
                TaskUpdate {
                    status: Some("active".to_string()),
                    ..TaskUpdate::default()
                }
            }
            ConsumerTaskMutation::Done if open => TaskUpdate {
                status: Some("done".to_string()),
                ..TaskUpdate::default()
            },
            ConsumerTaskMutation::Snooze { available_at }
                if open && before.available_at != *available_at =>
            {
                TaskUpdate {
                    available_at: Some(Some(available_at.clone())),
                    ..TaskUpdate::default()
                }
            }
            ConsumerTaskMutation::SetPriority { priority }
                if open && before.priority != *priority =>
            {
                TaskUpdate {
                    priority: Some(priority.clone()),
                    ..TaskUpdate::default()
                }
            }
            _ => {
                return Err(crate::error::CoreError::generation_conflict(
                    "quick action is stale for the task state",
                )
                .into());
            }
        };
        validate_task_update(&update)?;
        let mut affected_attachment_hashes = BTreeSet::new();
        let (changed, _) = apply_task_update(
            &mut tx,
            workspace,
            task_id,
            &update,
            &mut affected_attachment_hashes,
        )
        .await?;
        let after = task_snapshot(&mut tx, &workspace.id, task_id).await?;
        let field_version = match mutation {
            ConsumerTaskMutation::DetailStatus { .. } => {
                crate::db::field_version(&mut tx, task_id.as_str(), "status").await?
            }
            ConsumerTaskMutation::DeletedState { .. } => {
                crate::db::field_version(&mut tx, task_id.as_str(), "deleted").await?
            }
            _ => None,
        };
        let affected_attachment_hashes = affected_attachment_hashes.into_iter().collect::<Vec<_>>();
        crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
            &mut tx,
            &affected_attachment_hashes,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        tx.commit().await?;
        Ok(ConsumerTaskMutationOutcome {
            field_version,
            before,
            after,
            changed,
        })
    }

    pub(crate) async fn undo_consumer_task_mutation(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        mutation: &ConsumerTaskMutation,
        before: &TaskUndoSnapshot,
        expected: &TaskUndoSnapshot,
    ) -> Result<bool> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let current = task_snapshot(&mut tx, &workspace.id, task_id).await?;
        let expected_field_version = match mutation {
            ConsumerTaskMutation::DetailStatus {
                expected_version, ..
            } => Some((
                "status",
                expected_version,
                "task status changed after the detail action",
            )),
            ConsumerTaskMutation::DeletedState {
                expected_version, ..
            } => Some((
                "deleted",
                expected_version,
                "task deletion state changed after the action",
            )),
            _ => None,
        };
        if let Some((field, expected_version, message)) = expected_field_version
            && crate::db::field_version(&mut tx, task_id.as_str(), field).await?
                != *expected_version
        {
            return Err(crate::error::CoreError::generation_conflict(message).into());
        }
        if current != *expected {
            return Err(crate::error::CoreError::generation_conflict(
                "task changed after the quick action",
            )
            .into());
        }
        let update = match mutation {
            ConsumerTaskMutation::Start
            | ConsumerTaskMutation::Done
            | ConsumerTaskMutation::DetailStatus { .. } => TaskUpdate {
                status: Some(before.status.clone()),
                ..TaskUpdate::default()
            },
            ConsumerTaskMutation::DeletedState { .. } => TaskUpdate {
                deleted: Some(before.deleted),
                ..TaskUpdate::default()
            },
            ConsumerTaskMutation::Snooze { .. } => TaskUpdate {
                available_at: Some(
                    (!before.available_at.is_empty()).then(|| before.available_at.clone()),
                ),
                ..TaskUpdate::default()
            },
            ConsumerTaskMutation::SetPriority { .. } => TaskUpdate {
                priority: Some(before.priority.clone()),
                ..TaskUpdate::default()
            },
        };
        let mut affected_attachment_hashes = BTreeSet::new();
        let (changed, _) = apply_task_update(
            &mut tx,
            workspace,
            task_id,
            &update,
            &mut affected_attachment_hashes,
        )
        .await?;
        let affected_attachment_hashes = affected_attachment_hashes.into_iter().collect::<Vec<_>>();
        crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
            &mut tx,
            &affected_attachment_hashes,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        tx.commit().await?;
        Ok(changed)
    }
}
