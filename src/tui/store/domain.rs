use anyhow::Result;

use crate::operations::rename_config_project_mapping;
use crate::projects::{inferred_project_key_for_add_with_database, project_has_config_mapping};
use crate::tui::store::{MutationMessage, TaskScope};

use super::TuiStore;

impl TuiStore {
    pub(crate) async fn create_project(&mut self, name: String) -> Result<String> {
        let name = name.trim().to_string();
        let outcome = self
            .database
            .create_project_with_tui_undo(&self.active_workspace, &name)
            .await?;
        self.refresh(None).await?;
        Ok(format!("created project {}", outcome.project.key))
    }

    pub(crate) async fn delete_project(&mut self, project: &str) -> Result<MutationMessage> {
        let project = self
            .database
            .resolve_existing_project(&self.active_workspace.id, project)
            .await?;
        let config_mapping = project_has_config_mapping(
            &self.active_workspace.id,
            &self.active_workspace.key,
            &project.key,
        )
        .unwrap_or(false);
        let outcome = self
            .database
            .delete_project(&self.active_workspace, &project.key)
            .await?;

        let mut view_state = self.view_state.clone();
        if self.scope_project() == Some(outcome.project.key.as_str()) {
            view_state.scope = TaskScope::Workspace;
        }
        let selected = self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected;
        let mut message = format!("deleted project {}", outcome.project.key);
        if config_mapping {
            message.push_str("; config path mappings were left unchanged");
        }
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn rename_project(
        &mut self,
        project: &str,
        new_name: String,
    ) -> Result<MutationMessage> {
        let outcome = self
            .database
            .rename_project_with_tui_undo(&self.active_workspace, project, &new_name, None)
            .await?;
        let config_mapping = if outcome.changed {
            rename_config_project_mapping(
                &self.active_workspace,
                &outcome.previous.key,
                &outcome.project.key,
            )?
        } else {
            project_has_config_mapping(
                &self.active_workspace.id,
                &self.active_workspace.key,
                &outcome.previous.key,
            )
            .unwrap_or(false)
        };
        let mut view_state = self.view_state.clone();
        if self.scope_project() == Some(outcome.previous.key.as_str()) {
            view_state.scope = TaskScope::Project(outcome.project.key.clone());
        }
        let selected = self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected;
        let mut message = format!(
            "renamed project {} prefix={}",
            outcome.project.key, outcome.project.prefix
        );
        if !outcome.changed {
            message = format!("renamed project {} changed=none", outcome.project.key);
        } else if config_mapping {
            message.push_str("; updated config path mappings");
        }
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn create_label(&mut self, name: String) -> Result<String> {
        let name = name.trim().to_string();
        let outcome = self
            .database
            .create_label_with_tui_undo(&self.active_workspace, &name)
            .await?;
        self.labels = self
            .database
            .list_labels(&self.active_workspace.id, None)
            .await?;
        Ok(format!("created label {}", outcome.name))
    }

    pub(crate) async fn inferred_add_project(&self) -> Result<Option<String>> {
        inferred_project_key_for_add_with_database(&self.database, &self.active_workspace).await
    }
}
