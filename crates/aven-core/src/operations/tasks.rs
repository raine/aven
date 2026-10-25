mod change_set;
pub(crate) use change_set::{ChangeSetOperation, IndexedChangeSetError};
pub use change_set::{ChangeSetOutcome, ChangeTarget, CreatedTaskRef};
mod attachment_creation;
mod consumer;
mod creation;
mod mutation;
mod notes;

use crate::choices::TaskSource;
use crate::ids::TaskId;
use crate::labels::CreatedLabel;
use crate::metadata::TaskMetadataInput;
use crate::types::Task;
use crate::undo::TaskUndoSnapshot;

pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub project: Option<String>,
    pub status: String,
    pub priority: String,
    pub source: TaskSource,
    pub labels: Vec<String>,
    pub metadata: Vec<TaskMetadataInput>,
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
    },
}

#[derive(Debug, Clone)]
pub struct TaskCreationOptions {
    epic_id: Option<TaskId>,
    undo: TaskCreationUndo,
    create_missing_labels: bool,
    require_existing_project: bool,
    capture_undo_snapshot: bool,
    require_existing_epic: bool,
}

impl TaskCreationOptions {
    pub fn standalone(undo: TaskCreationUndo) -> Self {
        Self {
            epic_id: None,
            undo,
            create_missing_labels: false,
            require_existing_project: false,
            capture_undo_snapshot: false,
            require_existing_epic: false,
        }
    }

    pub fn for_epic(epic_id: TaskId, undo: TaskCreationUndo) -> Self {
        Self {
            epic_id: Some(epic_id),
            undo,
            create_missing_labels: false,
            require_existing_project: false,
            capture_undo_snapshot: false,
            require_existing_epic: false,
        }
    }

    pub fn with_create_missing_labels(mut self) -> Self {
        self.create_missing_labels = true;
        self
    }

    pub(crate) fn for_consumer_epic(epic_id: Option<TaskId>) -> Self {
        Self {
            epic_id,
            undo: TaskCreationUndo::None,
            create_missing_labels: false,
            require_existing_project: true,
            capture_undo_snapshot: false,
            require_existing_epic: true,
        }
    }

    pub fn for_consumer_capture() -> Self {
        Self {
            epic_id: None,
            undo: TaskCreationUndo::None,
            create_missing_labels: true,
            require_existing_project: true,
            capture_undo_snapshot: true,
            require_existing_epic: false,
        }
    }
}

#[derive(Debug)]
pub struct TaskOutcome {
    pub task: Task,
    pub create_change_id: Option<String>,
    pub attachment_change_ids: Vec<String>,
    pub undo_snapshot: Option<TaskUndoSnapshot>,
}

struct InsertedTask {
    id: TaskId,
    change_id: String,
    project_key: String,
    label_count: usize,
    created_labels: Vec<CreatedLabel>,
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
    pub set_metadata: Vec<TaskMetadataInput>,
    pub remove_metadata: Vec<String>,
    pub require_metadata_fields: Vec<(crate::ids::MetadataFieldId, String)>,
    pub label_selection: Option<TaskLabelSelection>,
    pub create_missing_labels: bool,
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

impl TaskMutationReport {
    pub fn changed_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.changed)
            .count()
    }
}

pub struct NoteEditOutcome {
    #[allow(dead_code)]
    pub task_id: TaskId,
    #[allow(dead_code)]
    pub note_id: String,
    pub found: bool,
    pub changed: bool,
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

pub(crate) use consumer::ConsumerTaskMutation;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use creation::create_task;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use mutation::update_task;
pub(crate) use mutation::update_task_labels_in_workspace;
#[cfg(test)]
pub(in crate::operations) use notes::add_note_operation;
