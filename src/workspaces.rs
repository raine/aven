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
    let cwd = std::fs::canonicalize(cwd).with_context(|| "could not resolve cwd")?;
    resolve_inferred_workspace(database, config, &cwd).await
}

pub(crate) async fn resolve_active_workspace_with_routing(
    database: &Database,
    explicit: Option<&str>,
    routing: &crate::routing::InvocationRouting<'_>,
) -> Result<Workspace> {
    routing.invocation_cwd()?;
    if let Some(name) = explicit {
        return database
            .resolve_required_workspace(name, "--workspace")
            .await;
    }
    resolve_inferred_workspace(database, routing.config, routing.cwd()?).await
}

async fn resolve_inferred_workspace(
    database: &Database,
    config: &AppConfig,
    cwd: &Path,
) -> Result<Workspace> {
    if let Some(route) = longest_matching_route(cwd, &config.workspace.routes)? {
        return database
            .resolve_required_workspace(&route.workspace, "workspace route")
            .await;
    }
    resolve_default_workspace(database, config).await
}

/// The workspace used when neither a command nor a directory route selects one.
pub(crate) async fn resolve_default_workspace(
    database: &Database,
    config: &AppConfig,
) -> Result<Workspace> {
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

/// Names the local configuration entries that select this workspace.
pub(crate) async fn config_entries_for_workspace(
    database: &Database,
    config: &AppConfig,
    workspace: &Workspace,
) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    let names = config
        .workspace
        .default
        .iter()
        .map(|name| ("workspace.default".to_string(), name))
        .chain(
            config
                .workspace
                .routes
                .iter()
                .enumerate()
                .map(|(index, route)| (format!("workspace.routes[{index}]"), &route.workspace)),
        );
    for (entry, name) in names {
        if database
            .find_workspace(name)
            .await?
            .is_some_and(|found| found.id == workspace.id)
        {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn longest_matching_route(
    cwd: &Path,
    routes: &[WorkspaceRouteConfig],
) -> Result<Option<WorkspaceRouteConfig>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_workspace_prefers_config_then_the_built_in_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let (database, pool) = crate::test_support::open_database(&temp.path().join("test.sqlite"))
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        ensure_default_workspace(&mut conn).await.unwrap();
        let configured = create_workspace(&mut conn, "work").await.unwrap();
        drop(conn);

        let mut config = AppConfig::default();
        let built_in = resolve_default_workspace(&database, &config).await.unwrap();
        assert_eq!(built_in.id.as_str(), DEFAULT_WORKSPACE_ID);

        config.workspace.default = Some("work".to_string());
        let selected = resolve_default_workspace(&database, &config).await.unwrap();
        assert_eq!(selected.id, configured.id);
    }
}
