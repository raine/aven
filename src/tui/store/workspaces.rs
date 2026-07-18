use anyhow::{Context, Result};

use super::{TaskFilterModifiers, TaskScope, TuiStore};

impl TuiStore {
    pub(crate) async fn switch_workspace(
        &mut self,
        key: String,
    ) -> Result<(String, Option<usize>)> {
        let workspace = self
            .database
            .find_workspace(&key)
            .await?
            .with_context(|| format!("workspace not found: {key}"))?;
        let name = workspace.name.clone();
        let key = workspace.key.clone();
        self.active_workspace = workspace;
        self.view_state.scope = TaskScope::Workspace;
        self.view_state.filter_modifiers = TaskFilterModifiers::default();
        let selected = self.refresh(None).await?;
        Ok((format!("switched workspace to {key} ({name})"), selected))
    }
}
