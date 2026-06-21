use anyhow::Result;

use crate::tui::store::MutationMessage;
use crate::undo::UndoPayload;

use super::TuiStore;

impl TuiStore {
    pub(super) async fn record_undo(&self, summary: &str, payload: UndoPayload) -> Result<()> {
        let workspace_id = self.active_workspace.id.clone();
        let mut conn = self.pool.acquire().await?;
        crate::undo::record_tui_undo(&mut conn, &workspace_id, summary, payload).await?;
        Ok(())
    }

    pub(crate) async fn undo_last(&mut self) -> Result<Option<MutationMessage>> {
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

        let selected = self.refresh(outcome.task_id.as_deref()).await?;
        Ok(Some(MutationMessage {
            message: format!("undid {}", outcome.summary),
            selected,
        }))
    }
}
