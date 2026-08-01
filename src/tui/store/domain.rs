use std::path::Path;

use anyhow::Result;

use crate::operations::{
    add_project_path_operation, list_project_paths_operation, remove_project_path_operation,
    rename_config_project_mapping,
};
use crate::projects::{inferred_project_key_for_add_with_database, project_has_config_mapping};
use crate::tui::store::{MutationMessage, TaskScope};

use super::TuiStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectCreationResult {
    pub(crate) project_key: String,
}

fn usage_count(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

impl TuiStore {
    pub(crate) async fn create_project(&mut self, name: String) -> Result<ProjectCreationResult> {
        let name = name.trim().to_string();
        let outcome = self
            .database
            .create_project_with_tui_undo(&self.active_workspace, &name)
            .await?;
        let project_key = outcome.project.key;
        self.wake_after_mutation();
        self.refresh(None).await?;
        Ok(ProjectCreationResult { project_key })
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
        self.wake_after_mutation();

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
        self.wake_after_mutation();
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
        self.wake_after_mutation();
        self.labels = self
            .database
            .list_labels(&self.active_workspace.id, None)
            .await?;
        Ok(format!("created label {}", outcome.name))
    }

    pub(crate) async fn label_usage(&self) -> Result<Vec<aven_core::labels::LabelUsage>> {
        self.database.label_usage(&self.active_workspace.id).await
    }

    pub(crate) async fn rename_label(
        &mut self,
        label: &str,
        new_name: String,
    ) -> Result<MutationMessage> {
        let outcome = self
            .database
            .rename_label_with_tui_undo(&self.active_workspace, label, &new_name)
            .await?;
        let mut view_state = self.view_state.clone();
        if view_state.filter_modifiers.label.as_deref() == Some(outcome.previous_name.as_str()) {
            view_state.filter_modifiers.label = Some(outcome.name.clone());
        }
        let selected = self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected;
        let message = if outcome.changed {
            format!(
                "renamed label {} to {} on {} and {}",
                outcome.previous_name,
                outcome.name,
                usage_count(outcome.task_count, "task", "tasks"),
                usage_count(outcome.series_count, "recurring series", "recurring series")
            )
        } else {
            format!("renamed label {} changed=none", outcome.name)
        };
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn delete_label(&mut self, label: &str) -> Result<MutationMessage> {
        let outcome = self
            .database
            .delete_label_with_tui_undo(&self.active_workspace, label)
            .await?;
        let mut view_state = self.view_state.clone();
        if view_state.filter_modifiers.label.as_deref() == Some(outcome.name.as_str()) {
            view_state.filter_modifiers.label = None;
        }
        let selected = self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected;
        let message = format!(
            "deleted label {} from {} and {}",
            outcome.name,
            usage_count(outcome.task_count, "task", "tasks"),
            usage_count(outcome.series_count, "recurring series", "recurring series")
        );
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn inferred_add_project(&self) -> Result<Option<String>> {
        inferred_project_key_for_add_with_database(&self.database, &self.active_workspace).await
    }

    pub(crate) async fn project_paths(
        &self,
        project: &str,
    ) -> Result<Vec<crate::operations::ProjectPathOutcome>> {
        list_project_paths_operation(&self.database, &self.active_workspace, Some(project)).await
    }

    pub(crate) async fn add_project_path(&self, project: &str, path: &Path) -> Result<String> {
        let outcome =
            add_project_path_operation(&self.database, &self.active_workspace, project, path)
                .await?;
        Ok(format!("added project path for {}", outcome.project.key))
    }

    pub(crate) async fn remove_project_path(&self, project: &str, path: &Path) -> Result<String> {
        let outcome =
            remove_project_path_operation(&self.database, &self.active_workspace, project, path)
                .await?;
        Ok(format!("removed project path for {}", outcome.project.key))
    }
}
