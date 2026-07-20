use std::path::Path;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;

use crate::config::{self, AppConfig, WorkspaceRouteConfig};

#[cfg(test)]
pub use aven_core::test_support::{create_workspace, ensure_default_workspace};
pub use aven_core::workspaces::{DEFAULT_WORKSPACE_ID, Workspace};

pub async fn resolve_active_workspace_with_database(
    database: &Database,
    explicit: Option<&str>,
    config: &AppConfig,
    cwd: &Path,
) -> Result<Workspace> {
    if let Some(name) = explicit {
        return database
            .resolve_required_workspace(name, "--workspace")
            .await;
    }
    if let Some(route) = longest_matching_route(cwd, &config.workspace.routes)? {
        return database
            .resolve_required_workspace(&route.workspace, "workspace route")
            .await;
    }
    if let Some(default) = config
        .workspace
        .default
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return database
            .resolve_required_workspace(default, "workspace.default")
            .await;
    }
    let workspaces = database.list_workspaces().await?;
    if let Some(workspace) = workspaces
        .iter()
        .find(|workspace| workspace.id.as_str() == DEFAULT_WORKSPACE_ID)
    {
        return Ok(workspace.clone());
    }
    if workspaces.len() == 1 {
        return Ok(workspaces[0].clone());
    }
    bail!("error workspace-required hint=\"pass --workspace or configure workspace.default\"")
}

fn longest_matching_route(
    cwd: &Path,
    routes: &[WorkspaceRouteConfig],
) -> Result<Option<WorkspaceRouteConfig>> {
    let cwd = std::fs::canonicalize(cwd).with_context(|| "could not resolve cwd")?;
    let mut best: Option<(usize, WorkspaceRouteConfig)> = None;
    for route in routes {
        for path in &route.paths {
            let path = config::expand_tilde(path)?;
            let path = std::fs::canonicalize(&path).with_context(|| {
                format!("could not resolve workspace route path {}", path.display())
            })?;
            if cwd.starts_with(&path) {
                let len = path.components().count();
                if best.as_ref().is_none_or(|(best_len, _)| len > *best_len) {
                    best = Some((len, route.clone()));
                }
            }
        }
    }
    Ok(best.map(|(_, route)| route))
}
