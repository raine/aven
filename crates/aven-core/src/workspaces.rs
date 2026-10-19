use crate::ids::WorkspaceId;
use anyhow::{Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::change_log::op_type;
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

/// What a workspace still holds when it is deleted. Any nonzero count except
/// `open_tasks`, which is a subset of `tasks`, requires forcing the deletion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceContents {
    pub tasks: u32,
    pub open_tasks: u32,
    pub recurrences: u32,
    pub conflicts: u32,
}

impl WorkspaceContents {
    pub fn is_empty(&self) -> bool {
        self.tasks == 0 && self.recurrences == 0 && self.conflicts == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceOpenSummary {
    pub workspace: Workspace,
    pub open_task_count: u32,
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

    pub async fn list_workspace_open_summaries(&self) -> Result<Vec<WorkspaceOpenSummary>> {
        let mut conn = self.acquire_reader().await?;
        let rows = sqlx::query(
            "SELECT w.id, w.key, w.name,
                    COALESCE(SUM(CASE WHEN t.deleted = 0
                        AND t.status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0)
                        AS open_task_count
             FROM workspaces w
             LEFT JOIN tasks t ON t.workspace_id = w.id
             WHERE w.archived = 0
             GROUP BY w.id, w.key, w.name
             ORDER BY w.key",
        )
        .fetch_all(&mut *conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| WorkspaceOpenSummary {
                workspace: Workspace {
                    id: row.get("id"),
                    key: row.get("key"),
                    name: row.get("name"),
                },
                open_task_count: u32::try_from(row.get::<i64, _>("open_task_count"))
                    .unwrap_or(u32::MAX),
            })
            .collect())
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

    pub async fn workspace_contents(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<WorkspaceContents> {
        let mut conn = self.acquire_reader().await?;
        workspace_contents(&mut conn, workspace_id).await
    }

    /// Archives a workspace and frees its key. `config_entries` names the
    /// local configuration entries that refer to it; they and any remaining
    /// contents refuse the deletion unless `force` is set.
    pub async fn delete_workspace(
        &self,
        workspace_id: &WorkspaceId,
        config_entries: &[String],
        force: bool,
    ) -> Result<(Workspace, WorkspaceContents)> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let deleted = delete_workspace(&mut tx, workspace_id, config_entries, force).await?;
        tx.commit().await?;
        Ok(deleted)
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
    fetch_workspace(conn, workspace_id, false).await
}

async fn fetch_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    include_archived: bool,
) -> Result<Workspace> {
    let row =
        sqlx::query("SELECT id, key, name FROM workspaces WHERE id = ? AND (? OR archived = 0)")
            .bind(workspace_id)
            .bind(include_archived)
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

/// Looks up a workspace whether or not it is archived. Remote changes keep
/// applying to a deleted workspace's retained contents.
pub(crate) async fn stored_workspace_for_id(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<Workspace> {
    fetch_workspace(conn, workspace_id, true).await
}

/// Hides a deleted workspace and gives it the key `~<id>`. The key derives from
/// the ID alone so every replica agrees on it, and `normalize_key` never
/// produces `~`, so it cannot collide with a live workspace key.
pub(crate) async fn archive_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    deleted_at: &str,
) -> Result<()> {
    sqlx::query("UPDATE workspaces SET archived = 1, key = '~' || id, updated_at = ? WHERE id = ?")
        .bind(deleted_at)
        .bind(workspace_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
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

pub(crate) async fn workspace_contents(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<WorkspaceContents> {
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM tasks WHERE workspace_id = ?1 AND deleted = 0) AS tasks,
            (SELECT COUNT(*) FROM tasks WHERE workspace_id = ?1 AND deleted = 0
                AND status NOT IN ('done', 'canceled')) AS open_tasks,
            (SELECT COUNT(*) FROM recurrence_series WHERE workspace_id = ?1 AND deleted = 0
                AND state != 'stopped') AS recurrences,
            (SELECT COUNT(*) FROM conflicts WHERE workspace_id = ?1 AND resolved = 0)
                AS conflicts",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    let count = |column: &str| u32::try_from(row.get::<i64, _>(column)).unwrap_or(u32::MAX);
    Ok(WorkspaceContents {
        tasks: count("tasks"),
        open_tasks: count("open_tasks"),
        recurrences: count("recurrences"),
        conflicts: count("conflicts"),
    })
}

pub(crate) async fn delete_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    config_entries: &[String],
    force: bool,
) -> Result<(Workspace, WorkspaceContents)> {
    let protocol = crate::sync::protocol::replica_protocol(conn).await?;
    if protocol < crate::sync::protocol::required_protocol(op_type::DELETE_WORKSPACE)? {
        bail!(
            "error workspace-delete-unavailable hint=\"a later Aven release enables workspace deletion\""
        );
    }
    let workspace = workspace_for_id(conn, workspace_id).await?;
    if workspace.id.as_str() == DEFAULT_WORKSPACE_ID {
        bail!("error default-workspace-protected");
    }
    if !force && !config_entries.is_empty() {
        bail!(
            "error workspace-in-config key={} entries={} hint=\"edit the config or pass --force\"",
            workspace.key,
            config_entries.join(",")
        );
    }
    let contents = workspace_contents(conn, workspace_id).await?;
    if !force && !contents.is_empty() {
        bail!(
            "error workspace-not-empty key={} tasks={} open={} recurrences={} conflicts={} hint=\"pass --force to delete anyway\"",
            workspace.key,
            contents.tasks,
            contents.open_tasks,
            contents.recurrences,
            contents.conflicts
        );
    }
    let ts = now();
    archive_workspace(conn, workspace.id.as_str(), &ts).await?;
    insert_change(
        conn,
        "workspace",
        workspace.id.as_str(),
        None,
        op_type::DELETE_WORKSPACE,
        json!({ "deleted_at": ts }),
        None,
    )
    .await?;
    Ok((workspace, contents))
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

    async fn database() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let database = crate::test_support::open_blank_database(&dir.path().join("test.sqlite"))
            .await
            .unwrap();
        (dir, database)
    }

    fn draft(title: &str, status: &str) -> crate::operations::TaskDraft {
        crate::operations::TaskDraft {
            title: title.into(),
            description: String::new(),
            project: Some("app".into()),
            status: status.into(),
            priority: "none".into(),
            source: crate::choices::TaskSource::Cli,
            labels: vec![],
            metadata: vec![],
            available_at: None,
            due_on: None,
            is_epic: false,
        }
    }

    async fn delete_change_count(database: &Database) -> i64 {
        let mut conn = database.acquire_reader().await.unwrap();
        sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE op_type = 'delete_workspace'")
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn deletion_is_unavailable_until_the_replica_produces_protocol_19() {
        let (_dir, database) = database().await;
        let side = database.create_workspace("Side").await.unwrap();
        let error = database
            .delete_workspace(&side.id, &[], true)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("error workspace-delete-unavailable"),
            "{error}"
        );
        assert!(database.find_workspace("side").await.unwrap().is_some());
        assert_eq!(delete_change_count(&database).await, 0);
    }

    #[tokio::test]
    async fn deletion_refuses_in_order_then_archives_and_frees_the_name() {
        let (_dir, database) = database().await;
        crate::test_support::set_established_protocol(&database, 19)
            .await
            .unwrap();
        let default = database.find_workspace("default").await.unwrap().unwrap();
        let error = database
            .delete_workspace(&default.id, &["workspace.default".into()], true)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "error default-workspace-protected");

        let side = database.create_workspace("Side").await.unwrap();
        database
            .create_task(&side, draft("finished", "done"))
            .await
            .unwrap();
        let error = database
            .delete_workspace(&side.id, &["workspace.routes[0]".into()], false)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("error workspace-in-config key=side entries=workspace.routes[0] "),
            "{error}"
        );
        // Done tasks still count; projects alone do not.
        let error = database
            .delete_workspace(&side.id, &[], false)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with(
                "error workspace-not-empty key=side tasks=1 open=0 recurrences=0 conflicts=0 "
            ),
            "{error}"
        );
        assert_eq!(delete_change_count(&database).await, 0);

        let (deleted, contents) = database
            .delete_workspace(&side.id, &["workspace.routes[0]".into()], true)
            .await
            .unwrap();
        assert_eq!(deleted.key, "side");
        assert_eq!(contents.tasks, 1);
        assert!(database.find_workspace("side").await.unwrap().is_none());
        assert!(database.workspace_for_id(&side.id).await.is_err());
        let mut conn = database.acquire_reader().await.unwrap();
        let (key, name, archived, tasks): (String, String, i64, i64) = sqlx::query_as(
            "SELECT key, name, archived,
                    (SELECT COUNT(*) FROM tasks WHERE workspace_id = workspaces.id)
             FROM workspaces WHERE id = ?",
        )
        .bind(&side.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        assert_eq!(
            (key, name.as_str(), archived, tasks),
            (format!("~{}", side.id), "Side", 1, 1)
        );
        assert_eq!(delete_change_count(&database).await, 1);

        let recreated = database.create_workspace("Side").await.unwrap();
        assert_ne!(recreated.id, side.id);
        assert_eq!(recreated.key, "side");
    }

    #[tokio::test]
    async fn unresolved_conflicts_alone_keep_a_workspace_from_being_empty() {
        let (_dir, database) = database().await;
        crate::test_support::set_established_protocol(&database, 19)
            .await
            .unwrap();
        let side = database.create_workspace("Side").await.unwrap();
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(workspace_id, entity_type, entity_id, task_id, field,
                local_value, remote_value, remote_change_id, variant_a, variant_b, created_at)
             VALUES (?, 'task', 'gone', 'gone', 'title', 'a', 'b', 'c', 'a', 'b', 'now')",
        )
        .bind(&side.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        let contents = database.workspace_contents(&side.id).await.unwrap();
        assert_eq!(
            contents,
            WorkspaceContents {
                conflicts: 1,
                ..Default::default()
            }
        );
        assert!(
            database
                .delete_workspace(&side.id, &[], false)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn exported_deletion_history_imports_into_a_baseline_replica() {
        let (_dir, source) = database().await;
        crate::test_support::set_established_protocol(&source, 19)
            .await
            .unwrap();
        let side = source.create_workspace("Side").await.unwrap();
        source.delete_workspace(&side.id, &[], false).await.unwrap();
        let export = source.export_data(now()).await.unwrap();

        let (_target_dir, target) = database().await;
        target.import_data(&export).await.unwrap();
        assert!(target.find_workspace("side").await.unwrap().is_none());
        assert_eq!(delete_change_count(&target).await, 1);
        let report = target.database_integrity_report().await.unwrap();
        assert!(report.checks.iter().all(|check| check.ok), "{report:#?}");
    }
}
