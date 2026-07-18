mod conflicts;
mod dependencies;
mod epics;
mod projects;
mod tasks;

pub use conflicts::{ConflictDetail, ConflictListItem, ConflictOutcome, ConflictResolutionOutcome};
pub(crate) use conflicts::{ConflictNotFoundError, ConflictValueChoice, resolve_conflict_choice};
pub use dependencies::DependencyOutcome;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use dependencies::add_task_dependency;
pub(crate) use dependencies::dependency_path_exists;
pub use epics::EpicLinkOutcome;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use epics::add_task_to_epic;
pub(crate) use epics::task_has_epic_children;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use projects::create_label_operation;
pub use projects::{
    LabelDeleteOutcome, LabelOutcome, ProjectDeleteOutcome, ProjectMetadata, ProjectOutcome,
    ProjectRenameOutcome,
};
pub(crate) use projects::{insert_project_metadata_change, set_project_metadata};
pub(crate) use tasks::update_task_labels_in_workspace;
pub use tasks::{
    NoteDeleteOutcome, NoteOutcome, TaskDraft, TaskOutcome, TaskUpdate, TaskUpdateOutcome,
};
#[cfg(any(test, feature = "test-support"))]
pub(crate) use tasks::{create_task, set_task_deleted, update_task};
