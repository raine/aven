use anyhow::{Context, Result};

use super::{TaskFilterModifiers, TaskScope, TuiStore};

impl TuiStore {
    pub(crate) async fn create_workspace(&mut self, name: String) -> Result<String> {
        let workspace = self.database.create_workspace(&name).await?;
        let key = workspace.key.clone();
        let name = workspace.name.clone();
        let display = if name == key {
            name
        } else {
            format!("{name} ({key})")
        };
        self.workspaces.push(workspace);
        self.workspaces
            .sort_by(|left, right| left.key.cmp(&right.key));
        Ok(format!("created workspace {display}"))
    }

    pub(crate) async fn rename_workspace(
        &mut self,
        workspace_ref: String,
        new_name: String,
    ) -> Result<String> {
        let workspace = self
            .database
            .rename_workspace(&workspace_ref, &new_name)
            .await?;
        let renames_active = workspace.id == self.active_workspace.id;
        if renames_active {
            self.active_workspace = workspace.clone();
        }
        if let Some(existing) = self
            .workspaces
            .iter_mut()
            .find(|existing| existing.id == workspace.id)
        {
            *existing = workspace.clone();
        } else {
            self.workspaces.push(workspace.clone());
        }
        self.workspaces
            .sort_by(|left, right| left.key.cmp(&right.key));

        if renames_active {
            Ok(format!(
                "renamed active workspace to {} ({})",
                workspace.key, workspace.name
            ))
        } else {
            Ok(format!(
                "renamed workspace to {} ({}); active workspace remains {}",
                workspace.key, workspace.name, self.active_workspace.key
            ))
        }
    }

    pub(crate) async fn switch_workspace(&mut self, key: String) -> Result<Option<usize>> {
        let workspace = self
            .database
            .find_workspace(&key)
            .await?
            .with_context(|| format!("workspace not found: {key}"))?;
        let mut view_state = self.view_state.clone();
        view_state.scope = TaskScope::Workspace;
        view_state.filter_modifiers = TaskFilterModifiers::default();
        view_state.reset_projection_origin();
        let selected = self
            .refresh_with_workspace_and_view_state(workspace, view_state)
            .await?
            .selected;
        Ok(selected)
    }
}
