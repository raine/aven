use anyhow::Result;

use crate::tui::store::{
    MutationMessage, RefreshHealth, TaskScope, UndoPresentation, types::committed_mutation_error,
};

use super::TuiStore;

impl TuiStore {
    pub(super) async fn load_latest_undo_presentation(&self) -> Result<Option<UndoPresentation>> {
        let workspace_id = &self.active_workspace.id;
        let Some(presentation) = self
            .database
            .latest_tui_undo_presentation(workspace_id)
            .await?
        else {
            return Ok(None);
        };
        let task_count = presentation.task_ids.len();
        let display_refs = self
            .database
            .display_refs_for_task_ids(workspace_id, &presentation.task_ids)
            .await?;
        let phrase = if presentation.operation_phrase() == "task creation" {
            presentation.operation_phrase().to_string()
        } else if task_count == 1 {
            match display_refs.get(&presentation.task_ids[0]) {
                Some(display_ref) => {
                    format!("{} on {display_ref}", presentation.operation_phrase())
                }
                None => format!("{} on 1 task", presentation.operation_phrase()),
            }
        } else if task_count > 1 {
            format!("{} on {task_count} tasks", presentation.operation_phrase())
        } else {
            presentation.operation_phrase().to_string()
        };
        Ok(Some(UndoPresentation {
            entry_id: presentation.id,
            phrase,
        }))
    }

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
        if self.refresh_health == RefreshHealth::Failed {
            return Ok(None);
        }
        let consumed = self.latest_undo.clone();
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
        let phrase = consumed
            .filter(|presentation| presentation.entry_id == outcome.presentation.id)
            .map(|presentation| presentation.phrase)
            .unwrap_or_else(|| outcome.presentation.operation);
        Ok(Some(MutationMessage::new(
            format!("undid {phrase}"),
            selected,
        )))
    }
}
