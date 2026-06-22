use anyhow::Result;

use crate::tui::store::MutationMessage;
use crate::undo::{UndoCommand, UndoPayload};

use super::TuiStore;

impl TuiStore {
    pub(super) async fn record_undo(&self, summary: &str, payload: UndoPayload) -> Result<()> {
        let workspace_id = self.active_workspace.id.clone();
        let mut conn = self.pool.acquire().await?;
        crate::undo::record_tui_undo(&mut conn, &workspace_id, summary, payload).await?;
        Ok(())
    }

    pub(super) async fn record_undo_commands(
        &self,
        summary: &str,
        commands: Vec<UndoCommand>,
    ) -> Result<()> {
        self.record_undo(summary, UndoPayload { commands }).await
    }

    pub(super) async fn refresh_task_message(
        &mut self,
        task_id: &str,
        message: impl Into<String>,
    ) -> Result<MutationMessage> {
        let selected = self.refresh(Some(task_id)).await?;
        Ok(MutationMessage::new(message, selected))
    }

    pub(super) async fn refresh_index_message(
        &mut self,
        selected: Option<usize>,
        message: impl Into<String>,
    ) -> Result<MutationMessage> {
        self.refresh(None).await?;
        let selected = self.restored_task_selection_at_index(selected);
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn undo_last(
        &mut self,
        selected: Option<usize>,
    ) -> Result<Option<MutationMessage>> {
        self.activate_workspace();
        let workspace_id = self.active_workspace.id.clone();
        let mut conn = self.pool.acquire().await?;
        let Some(outcome) = crate::undo::apply_latest_tui_undo(&mut conn, &workspace_id).await?
        else {
            return Ok(None);
        };
        drop(conn);

        if let Some(include_deleted) = outcome.include_deleted {
            self.filters.include_deleted = include_deleted;
        }

        let selected = if selected.is_some() {
            self.refresh(None).await?;
            self.restored_task_selection_at_index(selected)
        } else {
            self.refresh(outcome.task_id.as_deref()).await?
        };
        Ok(Some(MutationMessage::new(
            format!("undid {}", outcome.summary),
            selected,
        )))
    }
}
