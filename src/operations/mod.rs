mod attachments;
mod config;
mod conflicts;
mod dependencies;
mod epics;
mod projects;
mod tasks;

pub(crate) use attachments::{
    AttachmentAddInput, AttachmentReadItem, TaskAttachmentAddInput, add_ordered_task_attachment,
    add_task_attachment, attachment_by_id, attachment_read_items_by_task, attachments_by_task,
    delete_task_attachment,
};
pub(crate) use config::{init_config, show_config, show_config_paths};
pub(crate) use conflicts::{
    ConflictDetail, ConflictListItem, conflict_variant_value, list_conflicts, resolve_conflict,
    task_conflicts,
};
pub(crate) use dependencies::{
    add_task_dependency, dependency_path_exists, remove_task_dependency,
};
pub(crate) use epics::{add_task_to_epic, remove_task_from_epic, task_has_epic_children};
pub(crate) use projects::{
    ProjectMetadata, add_project_path_operation, create_label_operation, create_project_operation,
    delete_label_operation, delete_project_operation, insert_project_metadata_change,
    list_project_paths_operation, remove_project_path_operation, rename_config_project_mapping,
    rename_project_operation, set_project_metadata,
};
pub(crate) use tasks::{
    TaskDraft, TaskOutcome, TaskUpdate, add_note, create_task, create_task_with_attachments,
    delete_note, set_task_deleted, update_task, update_task_labels_in_workspace,
};
