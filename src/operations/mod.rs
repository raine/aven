mod config;
mod projects;

#[cfg(test)]
pub use aven_core::operations::attachment_read_items_by_task;
pub use aven_core::operations::{
    AttachmentAddInput, ConflictDetail, ConflictListItem, TaskAttachmentAddInput,
    TaskCreationOptions, TaskCreationUndo, TaskDraft, TaskOutcome, TaskUpdate,
};
#[cfg(test)]
pub use aven_core::test_support::{
    add_task_dependency, add_task_to_epic, create_label_operation, set_task_deleted, update_task,
    update_task_labels_in_workspace,
};

#[cfg(test)]
pub async fn create_task(
    conn: &mut sqlx::SqliteConnection,
    workspace: &crate::workspaces::Workspace,
    mut draft: aven_core::operations::TaskDraft,
) -> anyhow::Result<aven_core::operations::TaskOutcome> {
    let project = aven_core::test_support::resolve_or_create_project_in_workspace(
        conn,
        &workspace.id,
        draft.project.as_deref().unwrap_or("default"),
    )
    .await?;
    draft.project = Some(project.key);
    aven_core::test_support::create_task(conn, workspace, draft).await
}

pub use config::{init_config, init_config_at, show_config, show_config_paths};

pub use projects::{
    add_project_path_operation, create_project_operation, delete_project_operation,
    list_project_paths_operation, remove_project_path_operation, rename_config_project_mapping,
    rename_project_operation,
};
