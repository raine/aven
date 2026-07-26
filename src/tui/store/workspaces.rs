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
        let mut view_state = self.view_state.clone();
        view_state.scope = TaskScope::Workspace;
        view_state.filter_modifiers = TaskFilterModifiers::default();
        let selected = self
            .refresh_with_workspace_and_view_state(workspace, view_state)
            .await?
            .selected;
        Ok((format!("switched workspace to {key} ({name})"), selected))
    }
}
