use anyhow::{Context, Result};

use crate::query::TaskFilters;
use crate::workspaces::find_workspace;

use super::{SidebarTarget, TuiStore};

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
        self.active_view = SidebarTarget::All;
        self.filters = TaskFilters::default();
        self.activate_workspace();
        let selected = self.refresh(None).await?;
        Ok((format!("switched workspace to {key} ({name})"), selected))
    }
}
