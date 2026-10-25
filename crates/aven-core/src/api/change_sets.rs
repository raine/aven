use super::{Error, ErrorCode, Store, TaskCapture, TaskEdit};
use crate::ids::{TaskId, WorkspaceId};
use crate::operations::{ChangeSetOperation, IndexedChangeSetError};

pub use crate::operations::{
    AttachmentChange, AttachmentChangeKind, ChangeSetOutcome, ChangeTarget, CreatedTaskRef,
};

/// A staged consumer mutation. Fields use the same rules as capture and edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeOp {
    Create {
        local_id: String,
        fields: TaskCapture,
    },
    Update {
        target: ChangeTarget,
        fields: TaskEdit,
    },
    AddNote {
        target: ChangeTarget,
        body: String,
    },
    /// Attaches a staged agent asset. The caller chooses `attachment_id` (a task
    /// attachment ID) and should keep it stable across retries. A task that already
    /// has a live attachment with the same bytes is reported as already present.
    AddAttachment {
        target: ChangeTarget,
        attachment_id: String,
        pin_id: String,
        filename: Option<String>,
        alt_text: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSetError {
    /// Zero-based input operation; None denotes a transaction-level failure.
    pub operation_index: Option<usize>,
    pub error: Error,
}

impl std::fmt::Display for ChangeSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(index) = self.operation_index {
            write!(f, "operation {index}: ")?;
        }
        self.error.fmt(f)
    }
}

impl std::error::Error for ChangeSetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<IndexedChangeSetError> for ChangeSetError {
    fn from(value: IndexedChangeSetError) -> Self {
        Self {
            operation_index: value.operation_index,
            error: Error::from_internal(value.error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteReceipt {
    pub note_id: String,
    pub undo_token: String,
}

impl Store {
    /// Validates the complete ordered set before writing, then commits it atomically.
    /// Local IDs must be unique and refer only to earlier creates. Empty sets succeed.
    /// The receipt needs no read-model refresh; created references reflect final projects.
    pub async fn apply_change_set(
        &self,
        workspace_id: &WorkspaceId,
        ops: Vec<ChangeOp>,
    ) -> Result<ChangeSetOutcome, ChangeSetError> {
        let ops = ops
            .into_iter()
            .enumerate()
            .map(|(index, op)| {
                let result = match op {
                    ChangeOp::Create { local_id, fields } => fields
                        .into_draft()
                        .map(|draft| ChangeSetOperation::Create { local_id, draft }),
                    ChangeOp::Update { target, fields } => {
                        fields.into_update().map(|(project_id, update)| {
                            ChangeSetOperation::Update {
                                target,
                                project_id,
                                update,
                            }
                        })
                    }
                    ChangeOp::AddNote { target, body } => {
                        Ok(ChangeSetOperation::AddNote { target, body })
                    }
                    ChangeOp::AddAttachment {
                        target,
                        attachment_id,
                        pin_id,
                        filename,
                        alt_text,
                    } => Ok(ChangeSetOperation::AddAttachment {
                        target,
                        attachment_id,
                        pin_id,
                        filename,
                        alt_text,
                    }),
                };
                result.map_err(|error| ChangeSetError {
                    operation_index: Some(index),
                    error,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.database
            .apply_consumer_change_set(workspace_id, ops)
            .await
            .map(|result| result.outcome)
            .map_err(Into::into)
    }

    /// Reverses the entire set in one transaction. Subsequent edits can invalidate
    /// the receipt; recurrence undo also retains its unsynced/untouched-successor rules.
    /// Created tasks are soft-deleted, as with undo_queue_capture. Only attachments the
    /// set inserted are deleted; one already deleted by the user is left as it is.
    pub async fn undo_change_set(&self, undo_token: &str) -> Result<(), Error> {
        let token = serde_json::from_str(undo_token).map_err(|_| {
            Error::new(
                ErrorCode::Validation,
                "invalid change set undo token".into(),
            )
        })?;
        self.database
            .undo_consumer_change_set(token)
            .await
            .map_err(Error::from_internal)
    }

    /// Adds a note verbatim and returns a conditional undo receipt without refreshing views.
    pub async fn add_note(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        body: String,
    ) -> Result<NoteReceipt, Error> {
        let result = self
            .database
            .apply_consumer_change_set(
                workspace_id,
                vec![ChangeSetOperation::AddNote {
                    target: ChangeTarget::Existing(task_id.clone()),
                    body,
                }],
            )
            .await
            .map_err(|error| Error::from_internal(error.error))?;
        Ok(NoteReceipt {
            note_id: result
                .note_ids
                .into_iter()
                .next()
                .expect("one note operation"),
            undo_token: result.outcome.undo_token,
        })
    }

    /// Accepts the receipt returned by add_note (also accepted by undo_change_set).
    pub async fn undo_note(&self, undo_token: &str) -> Result<(), Error> {
        self.undo_change_set(undo_token).await
    }
}
