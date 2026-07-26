mod attachments;
mod conflicts;
mod dependencies;
mod epics;
mod projects;
mod tasks;

#[cfg(feature = "test-support")]
pub use attachments::attachment_read_items_by_task;
pub use attachments::{
    AttachmentAddInput, AttachmentAddOutcome, AttachmentOutcome, AttachmentReadItem,
    PreparedAttachment, TaskAttachmentAddInput,
};
pub use conflicts::{ConflictDetail, ConflictListItem, ConflictOutcome, ConflictResolutionOutcome};
pub(crate) use conflicts::{ConflictNotFoundError, ConflictValueChoice, resolve_conflict_choice};
pub use dependencies::DependencyOutcome;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use dependencies::add_task_dependency;
pub(crate) use dependencies::dependency_path_exists;
pub use epics::EpicLinkOutcome;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use epics::add_task_to_epic;
pub(crate) use epics::{
    add_task_to_epic_in_transaction, remove_task_from_epic_in_transaction,
    restore_task_to_epic_in_transaction, task_has_epic_children,
};
#[cfg(any(test, feature = "test-support"))]
pub(crate) use projects::create_label_operation;
pub use projects::{
    LabelDeleteOutcome, LabelOutcome, ProjectDeleteOutcome, ProjectMetadata, ProjectOutcome,
    ProjectRenameOutcome,
};
pub(crate) use projects::{insert_project_metadata_change, set_project_metadata};
pub(crate) use tasks::update_task_labels_in_workspace;
pub use tasks::{
    NoteDeleteOutcome, NoteOutcome, TaskCreationOptions, TaskCreationUndo, TaskDraft,
    TaskLabelSelection, TaskMutationOutcome, TaskMutationReport, TaskOutcome, TaskUpdate,
    TaskUpdateOutcome,
};
#[cfg(any(test, feature = "test-support"))]
pub(crate) use tasks::{create_task, update_task};
