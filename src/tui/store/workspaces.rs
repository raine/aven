use anyhow::{Context, Result};

use crate::workspaces::find_workspace;

use super::{TaskFilterModifiers, TaskScope, TuiStore};

impl TuiStore {
    pub(crate) async fn switch_workspace(
        &mut self,
        key: String,
    ) -> Result<(String, Option<usize>)> {
        let mut conn = self.pool.acquire().await?;
        let workspace = find_workspace(&mut conn, &key)
            .await?
            .with_context(|| format!("workspace not found: {key}"))?;
        drop(conn);
        let name = workspace.name.clone();
        let key = workspace.key.clone();
        self.active_workspace = workspace;
        self.view_state.scope = TaskScope::Workspace;
        self.view_state.filter_modifiers = TaskFilterModifiers::default();
        self.activate_workspace();
        let selected = self.refresh(None).await?;
        Ok((format!("switched workspace to {key} ({name})"), selected))
    }
}
