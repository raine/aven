use anyhow::Result;

use crate::tui::store::{MutationMessage, TaskScope, types::committed_mutation_error};

use super::TuiStore;

impl TuiStore {
    pub(super) async fn refresh_task_message(
        &mut self,
        task_id: &crate::ids::TaskId,
        message: impl Into<String>,
    ) -> Result<MutationMessage> {
        let selected = self
            .refresh(Some(task_id))
            .await
            .map_err(committed_mutation_error)?;
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn undo_last(
        &mut self,
        selected: Option<usize>,
    ) -> Result<Option<MutationMessage>> {
        let workspace_id = self.active_workspace.id.clone();
        let Some(outcome) = self.database.apply_latest_tui_undo(&workspace_id).await? else {
            return Ok(None);
        };

        if let Some(include_deleted) = outcome.include_deleted {
            self.view_state.filter_modifiers.include_deleted = include_deleted;
        }
        if let Some(project_rename) = &outcome.project_rename
            && self.scope_project() == Some(project_rename.after_key.as_str())
        {
            self.view_state.scope = TaskScope::Project(project_rename.before_key.clone());
        }

        let selected = if selected.is_some() {
            self.refresh(None).await.map_err(committed_mutation_error)?;
            self.restored_task_selection_at_index(selected)
        } else {
            self.refresh(outcome.task_id.as_ref())
                .await
                .map_err(committed_mutation_error)?
        };
        Ok(Some(MutationMessage::new(
            format!("undid {}", outcome.summary),
            selected,
        )))
    }
}
