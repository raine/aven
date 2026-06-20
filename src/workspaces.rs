use std::path::Path;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::config::{AppConfig, WorkspaceRouteConfig};
use crate::db::insert_change;
use crate::ids::{new_id, now};
use crate::projects::normalize_key;

pub(crate) const DEFAULT_WORKSPACE_ID: &str = "0000000000000000";

static ACTIVE_WORKSPACE: OnceLock<Mutex<Option<Workspace>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Workspace {
    pub(crate) id: String,
    pub(crate) key: String,
    pub(crate) name: String,
}

pub(crate) fn set_active_workspace(workspace: Workspace) {
    *ACTIVE_WORKSPACE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("active workspace lock") = Some(workspace);
}

pub(crate) fn active_workspace() -> Workspace {
    ACTIVE_WORKSPACE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("active workspace lock")
        .clone()
        .unwrap_or_else(|| Workspace {
            id: DEFAULT_WORKSPACE_ID.to_string(),
            key: "default".to_string(),
            name: "default".to_string(),
        })
}

pub(crate) fn active_workspace_id() -> String {
    active_workspace().id
}

pub(crate) async fn ensure_default_workspace(conn: &mut SqliteConnection) -> Result<Workspace> {
    if let Some(row) = sqlx::query("SELECT id, key, name FROM workspaces WHERE id = ?")
        .bind(DEFAULT_WORKSPACE_ID)
        .fetch_optional(&mut *conn)
        .await?
    {
        return Ok(Workspace {
            id: row.get("id"),
            key: row.get("key"),
            name: row.get("name"),
        });
    }
    let id = DEFAULT_WORKSPACE_ID.to_string();
    let ts = now();
    sqlx::query("INSERT INTO workspaces(id, name, key, created_at, updated_at) VALUES (?, 'default', 'default', ?, ?)")
        .bind(&id)
        .bind(&ts)
        .bind(&ts)
        .execute(&mut *conn)
        .await?;
    Ok(Workspace {
        id,
        key: "default".to_string(),
        name: "default".to_string(),
    })
}

pub(crate) async fn list_workspaces(conn: &mut SqliteConnection) -> Result<Vec<Workspace>> {
    let rows = sqlx::query("SELECT id, key, name FROM workspaces WHERE archived = 0 ORDER BY key")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| Workspace {
            id: row.get("id"),
            key: row.get("key"),
            name: row.get("name"),
        })
        .collect())
}

pub(crate) async fn find_workspace(
    conn: &mut SqliteConnection,
    name_or_key: &str,
) -> Result<Option<Workspace>> {
    let key = normalize_key(name_or_key);
    let row = sqlx::query(
        "SELECT id, key, name FROM workspaces
         WHERE archived = 0 AND (key = ? OR lower(name) = lower(?))",
    )
    .bind(key)
    .bind(name_or_key)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|row| Workspace {
        id: row.get("id"),
        key: row.get("key"),
        name: row.get("name"),
    }))
}

pub(crate) async fn resolve_workspace(
    conn: &mut SqliteConnection,
    name_or_key: &str,
) -> Result<Workspace> {
    resolve_required_workspace(conn, name_or_key, "workspace").await
}

pub(crate) async fn resolve_required_workspace(
    conn: &mut SqliteConnection,
    name_or_key: &str,
    source: &str,
) -> Result<Workspace> {
    if let Some(workspace) = find_workspace(conn, name_or_key).await? {
        return Ok(workspace);
    }
    bail!(
        "error unknown-workspace input={} source={} hint=\"create the workspace with atm workspace create\"",
        name_or_key,
        source
    );
}

pub(crate) async fn resolve_active_workspace(
    conn: &mut SqliteConnection,
    explicit: Option<&str>,
    config: &AppConfig,
    cwd: &Path,
) -> Result<Workspace> {
    if let Some(name) = explicit {
        return resolve_required_workspace(conn, name, "--workspace").await;
    }
    if let Some(route) = longest_matching_route(cwd, &config.workspace.routes)? {
        return resolve_required_workspace(conn, &route.workspace, "workspace route").await;
    }
    if let Some(default) = config
        .workspace
        .default
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return resolve_required_workspace(conn, default, "workspace.default").await;
    }
    let workspaces = list_workspaces(conn).await?;
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
            let path = std::fs::canonicalize(path).with_context(|| {
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

pub(crate) async fn create_workspace(conn: &mut SqliteConnection, name: &str) -> Result<Workspace> {
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-workspace input={name}");
    }
    if find_workspace(conn, &key).await?.is_some() {
        bail!("error workspace-exists key={key}");
    }
    let id = new_id();
    let ts = now();
    sqlx::query("INSERT INTO workspaces(id, name, key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(name)
        .bind(&key)
        .bind(&ts)
        .bind(&ts)
        .execute(&mut *conn)
        .await?;
    insert_change(
        conn,
        "workspace",
        &id,
        None,
        "create_workspace",
        json!({ "key": key, "name": name, "created_at": ts }),
        None,
    )
    .await?;
    Ok(Workspace {
        id,
        key,
        name: name.to_string(),
    })
}

pub(crate) async fn rename_workspace(
    conn: &mut SqliteConnection,
    workspace_ref: &str,
    new_name: &str,
) -> Result<Workspace> {
    let workspace = resolve_workspace(conn, workspace_ref).await?;
    let new_key = normalize_key(new_name);
    if new_key.is_empty() {
        bail!("error invalid-workspace input={new_name}");
    }
    if new_key != workspace.key && find_workspace(conn, &new_key).await?.is_some() {
        bail!("error workspace-exists key={new_key}");
    }
    let ts = now();
    sqlx::query("UPDATE workspaces SET name = ?, key = ?, updated_at = ? WHERE id = ?")
        .bind(new_name)
        .bind(&new_key)
        .bind(&ts)
        .bind(&workspace.id)
        .execute(&mut *conn)
        .await?;
    if workspace.name != new_name {
        insert_change(
            conn,
            "workspace",
            &workspace.id,
            Some("name"),
            "set_workspace_field",
            json!({ "value": new_name }),
            None,
        )
        .await?;
    }
    if workspace.key != new_key {
        insert_change(
            conn,
            "workspace",
            &workspace.id,
            Some("key"),
            "set_workspace_field",
            json!({ "value": new_key }),
            None,
        )
        .await?;
    }
    Ok(Workspace {
        id: workspace.id,
        key: new_key,
        name: new_name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;

    #[tokio::test]
    async fn fresh_database_has_default_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_db(&dir.path().join("test.db")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let workspaces = list_workspaces(&mut conn).await.unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].key, "default");
    }
}
