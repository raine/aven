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
        self.wake_after_mutation();

        let mut view_state = self.view_state.clone();
        if let Some(include_deleted) = outcome.include_deleted {
            view_state.filter_modifiers.include_deleted = include_deleted;
        }
        if let Some(project_rename) = &outcome.project_rename
            && self.scope_project() == Some(project_rename.after_key.as_str())
        {
            view_state.scope = TaskScope::Project(project_rename.before_key.clone());
        }
        if let Some(label_rename) = &outcome.label_rename
            && view_state.filter_modifiers.label.as_deref() == Some(label_rename.after.as_str())
        {
            view_state.filter_modifiers.label = Some(label_rename.before.clone());
        }

        let selected = if selected.is_some() {
            self.refresh_with_view_state(view_state, None)
                .await
                .map_err(committed_mutation_error)?;
            self.restored_task_selection_at_index(selected)
        } else {
            self.refresh_with_view_state(view_state, outcome.task_id.as_ref())
                .await
                .map_err(committed_mutation_error)?
                .selected
        };
        Ok(Some(MutationMessage::new(
            format!("undid {}", outcome.summary),
            selected,
        )))
    }
}
