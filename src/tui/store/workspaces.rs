use anyhow::{Context, Result};

use super::{TaskFilterModifiers, TaskScope, TuiStore};
use crate::workspaces::Workspace;

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
        self.projection.workspaces.push(workspace);
        self.projection
            .workspaces
            .sort_by(|left, right| left.key.cmp(&right.key));
        self.projection.rebuild_sidebar(&self.app_config);
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
            self.projection.active_workspace = workspace.clone();
        }
        if let Some(existing) = self
            .projection
            .workspaces
            .iter_mut()
            .find(|existing| existing.id == workspace.id)
        {
            *existing = workspace.clone();
        } else {
            self.projection.workspaces.push(workspace.clone());
        }
        self.projection
            .workspaces
            .sort_by(|left, right| left.key.cmp(&right.key));

        self.projection.rebuild_sidebar(&self.app_config);
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

    pub(crate) fn take_deleted_active_workspace(&mut self) -> Option<String> {
        self.deleted_active_workspace.take()
    }

    /// Lines describing what still uses a workspace, shown before deleting it.
    pub(crate) async fn workspace_deletion_warnings(
        &self,
        workspace: &Workspace,
    ) -> Result<Vec<String>> {
        let contents = self.database.workspace_contents(&workspace.id).await?;
        let mut lines = Vec::new();
        if !contents.is_empty() {
            lines.push(format!(
                "It still has {} tasks ({} open), {} recurring schedules and {} unresolved conflicts.",
                contents.tasks, contents.open_tasks, contents.recurrences, contents.conflicts
            ));
        }
        let entries = crate::workspaces::config_entries_for_workspace(
            &self.database,
            &self.app_config,
            workspace,
        )
        .await?;
        if !entries.is_empty() {
            lines.push(format!("Config still selects it: {}.", entries.join(", ")));
        }
        Ok(lines)
    }

    /// Deletes a workspace after the user typed its key, which stands in for
    /// forcing past remaining contents and config entries.
    pub(crate) async fn delete_workspace(&mut self, workspace: &Workspace) -> Result<String> {
        let (deleted, _) = self
            .database
            .delete_workspace(&workspace.id, &[], true)
            .await?;
        self.refresh(None).await?;
        Ok(format!("deleted workspace {}", deleted.key))
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
