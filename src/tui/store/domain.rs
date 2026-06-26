use anyhow::Result;

use crate::labels::list_labels_in_workspace;
use crate::operations::{
    create_label_operation, create_project_operation, delete_project_operation,
};
use crate::projects::inferred_project_key_for_add_in_workspace;
use crate::tui::store::{MutationMessage, TaskScope};
use crate::undo::UndoCommand;

use super::TuiStore;

impl TuiStore {
    pub(crate) async fn create_project(&mut self, name: String) -> Result<String> {
        let name = name.trim().to_string();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = create_project_operation(&mut conn, &name, None).await?;
        drop(conn);
        let commands = if outcome.created {
            vec![UndoCommand::DeleteCreatedProject {
                project_key: outcome.project.key.clone(),
                create_change_id: outcome.change_id.unwrap_or_default(),
                expected_name: outcome.project.name.clone(),
                expected_prefix: outcome.project.prefix.clone(),
            }]
        } else {
            Vec::new()
        };
        self.record_undo_commands(&format!("project {}", outcome.project.key), commands)
            .await?;
        self.refresh(None).await?;
        Ok(format!("created project {}", outcome.project.key))
    }

    pub(crate) async fn delete_project(&mut self, project: &str) -> Result<MutationMessage> {
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = delete_project_operation(&mut conn, &self.active_workspace, project).await?;
        drop(conn);

        if self.scope_project() == Some(outcome.project.key.as_str()) {
            self.view_state.scope = TaskScope::Workspace;
        }
        let selected = self.refresh(None).await?;
        let mut message = format!("deleted project {}", outcome.project.key);
        if outcome.config_mapping {
            message.push_str("; config path mappings were left unchanged");
        }
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn create_label(&mut self, name: String) -> Result<String> {
        let name = name.trim().to_string();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = create_label_operation(&mut conn, &name).await?;
        drop(conn);
        let commands = if outcome.created {
            vec![UndoCommand::DeleteCreatedLabel {
                label: outcome.name.clone(),
                create_change_id: outcome.change_id.unwrap_or_default(),
            }]
        } else {
            Vec::new()
        };
        self.record_undo_commands(&format!("label {}", outcome.name), commands)
            .await?;
        let mut conn = self.pool.acquire().await?;
        self.labels = list_labels_in_workspace(&mut conn, &self.active_workspace.id, None).await?;
        Ok(format!("created label {}", outcome.name))
    }

    pub(crate) async fn inferred_add_project(&self) -> Result<Option<String>> {
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        inferred_project_key_for_add_in_workspace(&mut conn, &self.active_workspace.id).await
    }
}
