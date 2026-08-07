use crate::ids::WorkspaceId;
use anyhow::{Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::db::{Database, begin_immediate, insert_change};
use crate::ids::now;
use crate::projects::normalize_key;

pub const DEFAULT_WORKSPACE_ID: &str = "0000000000000000";

pub fn default_workspace_id() -> WorkspaceId {
    DEFAULT_WORKSPACE_ID
        .parse()
        .expect("valid default workspace ID")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub key: String,
    pub name: String,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            id: DEFAULT_WORKSPACE_ID
                .parse()
                .expect("valid default workspace ID"),
            key: "default".to_string(),
            name: "default".to_string(),
        }
    }
}

impl Database {
    pub async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let mut conn = self.acquire_reader().await?;
        list_workspaces(&mut conn).await
    }

    pub async fn find_workspace(&self, name_or_key: &str) -> Result<Option<Workspace>> {
        let mut conn = self.acquire_reader().await?;
        find_workspace(&mut conn, name_or_key).await
    }

    pub async fn workspace_for_id(&self, workspace_id: &WorkspaceId) -> Result<Workspace> {
        let mut conn = self.acquire_reader().await?;
        workspace_for_id(&mut conn, workspace_id).await
    }

    pub async fn resolve_workspace(&self, name_or_key: &str) -> Result<Workspace> {
        self.resolve_required_workspace(name_or_key, "workspace")
            .await
    }

    pub async fn resolve_required_workspace(
        &self,
        name_or_key: &str,
        source: &str,
    ) -> Result<Workspace> {
        let mut conn = self.acquire_reader().await?;
        resolve_required_workspace(&mut conn, name_or_key, source).await
    }

    pub async fn create_workspace(&self, name: &str) -> Result<Workspace> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let workspace = create_workspace(&mut tx, name).await?;
        tx.commit().await?;
        Ok(workspace)
    }

    pub async fn rename_workspace(&self, workspace_ref: &str, new_name: &str) -> Result<Workspace> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let workspace = rename_workspace(&mut tx, workspace_ref, new_name).await?;
        tx.commit().await?;
        Ok(workspace)
    }
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
    let id: WorkspaceId = DEFAULT_WORKSPACE_ID
        .parse()
        .expect("valid default workspace ID");
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
    Ok(row.map(workspace_from_row))
}

pub(crate) async fn workspace_for_id(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<Workspace> {
    let row = sqlx::query("SELECT id, key, name FROM workspaces WHERE id = ? AND archived = 0")
        .bind(workspace_id)
        .fetch_optional(&mut *conn)
        .await?;
    match row {
        Some(row) => Ok(workspace_from_row(row)),
        None => Err(crate::error::CoreError::not_found(format!(
            "error unknown-workspace-id id={workspace_id}"
        ))
        .into()),
    }
}

pub(crate) async fn workspace_key_for_id(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<String> {
    Ok(workspace_for_id(conn, workspace_id).await?.key)
}

fn workspace_from_row(row: sqlx::sqlite::SqliteRow) -> Workspace {
    Workspace {
        id: row.get("id"),
        key: row.get("key"),
        name: row.get("name"),
    }
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
        "error unknown-workspace input={} source={} hint=\"create the workspace with aven workspace create\"",
        name_or_key,
        source
    );
}

pub(crate) async fn create_workspace(conn: &mut SqliteConnection, name: &str) -> Result<Workspace> {
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-workspace input={name}");
    }
    if find_workspace(conn, &key).await?.is_some() {
        bail!("error workspace-exists key={key}");
    }
    let id = WorkspaceId::new();
    let ts = now();
    sqlx::query(
        "INSERT INTO workspaces(id, name, key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
    )
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
        id.as_str(),
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
            workspace.id.as_str(),
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
            workspace.id.as_str(),
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
