use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;

use crate::change_log::op_type;
use crate::db::{get_meta, set_meta};
use crate::ids::{TaskId, WorkspaceId};

const BASELINE_PREFIX: &str = "epic_membership_baseline:";
const REPAIR_KEY: &str = "epic_membership_history_repaired";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, sqlx::FromRow)]
struct Membership {
    epic_task_id: TaskId,
    created_at: String,
}

#[derive(sqlx::FromRow)]
struct Operation {
    op_type: String,
    epic_task_id: TaskId,
    created_at: String,
}

fn baseline_key(workspace_id: &str, child_id: &str) -> String {
    format!("{BASELINE_PREFIX}{workspace_id}:{child_id}")
}

pub(crate) fn parse_baseline_identity(
    key: &str,
    value: &str,
) -> Result<Option<(WorkspaceId, TaskId, TaskId)>> {
    let Some(identity) = key.strip_prefix(BASELINE_PREFIX) else {
        return Ok(None);
    };
    let (workspace_id, child_id) = identity
        .split_once(':')
        .context("invalid epic membership baseline identity")?;
    let workspace_id: WorkspaceId = workspace_id.parse()?;
    let child_id: TaskId = child_id.parse()?;
    let baseline: Membership =
        serde_json::from_str(value).context("invalid epic membership baseline")?;
    ensure!(
        child_id != baseline.epic_task_id,
        "invalid epic membership baseline self-link"
    );
    Ok(Some((workspace_id, child_id, baseline.epic_task_id)))
}

async fn validate_baseline_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    child_id: &str,
    parent_id: &TaskId,
) -> Result<()> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE workspace_id = ? AND id IN (?, ?)")
            .bind(workspace_id)
            .bind(child_id)
            .bind(parent_id)
            .fetch_one(&mut *conn)
            .await?;
    ensure!(count == 2, "invalid epic membership baseline missing task");
    Ok(())
}

/// Snapshot-only links precede the retained command history. Metadata carries
/// this baseline through database reopen and portable export/import.
pub(crate) async fn capture_snapshot_baseline(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    child_id: &str,
) -> Result<()> {
    let baseline = sqlx::query_as::<_, Membership>(
        "SELECT epic_task_id, created_at FROM task_epic_links
         WHERE workspace_id = ? AND child_task_id = ?
           AND NOT EXISTS (
             SELECT 1 FROM changes
             WHERE op_type IN ('epic_link_add', 'epic_link_remove')
               AND json_extract(payload, '$.workspace_id') = ? AND entity_id = ?
           )",
    )
    .bind(workspace_id)
    .bind(child_id)
    .bind(workspace_id)
    .bind(child_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(baseline) = baseline {
        let key = baseline_key(workspace_id, child_id);
        if get_meta(conn, &key).await?.is_none() {
            set_meta(conn, &key, &serde_json::to_string(&baseline)?).await?;
        }
    }
    Ok(())
}

fn reduce(membership: &mut Option<Membership>, operation: Operation) {
    match operation.op_type.as_str() {
        op_type::EPIC_LINK_ADD => {
            if membership
                .as_ref()
                .is_none_or(|current| operation.epic_task_id < current.epic_task_id)
            {
                *membership = Some(Membership {
                    epic_task_id: operation.epic_task_id,
                    created_at: operation.created_at,
                });
            }
        }
        op_type::EPIC_LINK_REMOVE => {
            if membership
                .as_ref()
                .is_some_and(|current| current.epic_task_id == operation.epic_task_id)
            {
                *membership = None;
            }
        }
        _ => unreachable!("membership history contains only epic operations"),
    }
}

/// Replays assigned commands in server order with the pending push order as an
/// optimistic tail. Replay preserves mutable task fields except for the final
/// parent's epic flag, which is required by its selected membership. Discarded
/// historical parents are not promoted, and task validation is not repeated.
pub(crate) async fn reconcile_child(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    child_id: &str,
) -> Result<()> {
    let operations = sqlx::query_as::<_, Operation>(
        "SELECT op_type, json_extract(payload, '$.epic_task_id') AS epic_task_id,
                COALESCE(json_extract(payload, '$.created_at'), created_at) AS created_at
         FROM changes
         WHERE op_type IN ('epic_link_add', 'epic_link_remove')
           AND json_extract(payload, '$.workspace_id') = ? AND entity_id = ?
         ORDER BY server_seq IS NULL, server_seq, local_seq, changes.created_at, change_id",
    )
    .bind(workspace_id)
    .bind(child_id)
    .fetch_all(&mut *conn)
    .await?;
    if operations.is_empty() {
        return Ok(());
    }
    let key = baseline_key(workspace_id, child_id);
    let baseline = get_meta(conn, &key).await?;
    if let Some(value) = &baseline {
        let (_, _, parent_id) = parse_baseline_identity(&key, value)?
            .context("invalid epic membership baseline identity")?;
        validate_baseline_tasks(conn, workspace_id, child_id, &parent_id).await?;
    }
    if baseline.is_none() && operations[0].op_type == op_type::EPIC_LINK_REMOVE {
        // A removal cannot establish the state preceding an incomplete history.
        // Preserve the snapshot rather than inventing an empty starting state.
        return Ok(());
    }
    let mut membership: Option<Membership> = baseline
        .map(|value| serde_json::from_str(&value).context("invalid epic membership baseline"))
        .transpose()?;
    for operation in operations {
        reduce(&mut membership, operation);
    }
    if let Some(membership) = membership {
        sqlx::query("UPDATE tasks SET is_epic = 1 WHERE workspace_id = ? AND id = ?")
            .bind(workspace_id)
            .bind(&membership.epic_task_id)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO task_epic_links(workspace_id, child_task_id, epic_task_id, created_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(workspace_id, child_task_id) DO UPDATE SET
               epic_task_id = excluded.epic_task_id, created_at = excluded.created_at",
        )
        .bind(workspace_id)
        .bind(child_id)
        .bind(membership.epic_task_id)
        .bind(membership.created_at)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query("DELETE FROM task_epic_links WHERE workspace_id = ? AND child_task_id = ?")
            .bind(workspace_id)
            .bind(child_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Repairs retained histories independently of the visible projection, including
/// children whose link is absent. The caller owns the write transaction.
pub(crate) async fn recover(conn: &mut SqliteConnection, force: bool) -> Result<()> {
    if !force && get_meta(conn, REPAIR_KEY).await?.as_deref() == Some("1") {
        return Ok(());
    }
    let baselines: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM meta WHERE key GLOB 'epic_membership_baseline:*'")
            .fetch_all(&mut *conn)
            .await?;
    for (key, value) in baselines {
        let (workspace_id, child_id, parent_id) = parse_baseline_identity(&key, &value)?
            .context("invalid epic membership baseline identity")?;
        validate_baseline_tasks(conn, workspace_id.as_str(), &child_id, &parent_id).await?;
    }
    let snapshots: Vec<(String, String)> =
        sqlx::query_as("SELECT workspace_id, child_task_id FROM task_epic_links")
            .fetch_all(&mut *conn)
            .await?;
    for (workspace_id, child_id) in snapshots {
        capture_snapshot_baseline(conn, &workspace_id, &child_id).await?;
    }
    let children: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT t.workspace_id, t.id FROM changes c
         JOIN tasks t ON t.workspace_id = json_extract(c.payload, '$.workspace_id')
           AND t.id = c.entity_id
         WHERE c.op_type IN ('epic_link_add', 'epic_link_remove')",
    )
    .fetch_all(&mut *conn)
    .await?;
    for (workspace_id, child_id) in children {
        reconcile_child(conn, &workspace_id, &child_id).await?;
    }
    set_meta(conn, REPAIR_KEY, "1").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation(add: bool, parent: &str) -> Operation {
        Operation {
            op_type: if add {
                op_type::EPIC_LINK_ADD
            } else {
                op_type::EPIC_LINK_REMOVE
            }
            .to_string(),
            epic_task_id: crate::test_support::task_id(parent),
            created_at: "t".to_string(),
        }
    }

    #[test]
    fn minimum_parent_exact_removal_and_no_resurrection() {
        let mut membership = None;
        reduce(&mut membership, operation(true, "b"));
        reduce(&mut membership, operation(true, "a"));
        reduce(&mut membership, operation(true, "c"));
        reduce(&mut membership, operation(false, "b"));
        assert_eq!(
            membership.as_ref().unwrap().epic_task_id,
            crate::test_support::task_id("a")
        );
        reduce(&mut membership, operation(false, "a"));
        assert_eq!(membership, None);
        reduce(&mut membership, operation(true, "c"));
        assert_eq!(
            membership.as_ref().unwrap().epic_task_id,
            crate::test_support::task_id("c")
        );
    }

    const CHILD: &str = "CCCC000000000001";
    const OTHER_CHILD: &str = "CCCC000000000002";
    const FIRST: &str = "AAAA000000000001";
    const SECOND: &str = "BBBB000000000001";
    const WORKSPACE: &str = "0000000000000000";

    async fn database() -> (tempfile::TempDir, crate::db::Database) {
        let temp = tempfile::tempdir().unwrap();
        let database = crate::db::Database::open(&temp.path().join("replica.sqlite"))
            .await
            .unwrap();
        let mut conn = database.acquire_writer().await.unwrap();
        let workspace = crate::workspaces::Workspace::default();
        let project = crate::projects::create_project(&mut conn, &workspace, "epics")
            .await
            .unwrap();
        for id in [CHILD, OTHER_CHILD, FIRST, SECOND] {
            sqlx::query(
                "INSERT INTO tasks(workspace_id, id, title, description, project_id,
                 status, priority, created_at, updated_at, is_epic)
                 VALUES (?, ?, 'task', '', ?, 'todo', 'none', 't', 't', ?)",
            )
            .bind(WORKSPACE)
            .bind(id)
            .bind(&project.id)
            .bind(i64::from(id == FIRST || id == SECOND))
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        drop(conn);
        (temp, database)
    }

    async fn history(
        conn: &mut SqliteConnection,
        child: &str,
        parent: &str,
        add: bool,
        sequence: i64,
    ) {
        let payload = serde_json::json!({
            "workspace_id": WORKSPACE,
            "workspace_key": "default",
            "epic_task_id": parent,
            "created_at": "t",
        });
        sqlx::query(
            "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id,
             field, op_type, payload, created_at, server_seq)
             VALUES (?, 'remote', ?, 'task', ?, 'epics', ?, ?, 't', ?)",
        )
        .bind(crate::ids::new_id())
        .bind(sequence)
        .bind(child)
        .bind(if add {
            op_type::EPIC_LINK_ADD
        } else {
            op_type::EPIC_LINK_REMOVE
        })
        .bind(payload.to_string())
        .bind(sequence)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    async fn snapshot(conn: &mut SqliteConnection, child: &str, parent: &str) {
        sqlx::query(
            "INSERT INTO task_epic_links(workspace_id, child_task_id, epic_task_id, created_at)
             VALUES (?, ?, ?, 't')",
        )
        .bind(WORKSPACE)
        .bind(child)
        .bind(parent)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    async fn parent(conn: &mut SqliteConnection, child: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT epic_task_id FROM task_epic_links WHERE workspace_id = ? AND child_task_id = ?",
        )
        .bind(WORKSPACE)
        .bind(child)
        .fetch_optional(&mut *conn)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn open_repairs_missing_and_spurious_links_without_repromoting_historical_parent() {
        let (temp, database) = database().await;
        {
            let mut conn = database.acquire_writer().await.unwrap();
            history(&mut conn, CHILD, FIRST, true, 1).await;
            history(&mut conn, CHILD, FIRST, false, 2).await;
            history(&mut conn, CHILD, SECOND, true, 3).await;
            history(&mut conn, OTHER_CHILD, FIRST, true, 4).await;
            history(&mut conn, OTHER_CHILD, FIRST, false, 5).await;
            snapshot(&mut conn, OTHER_CHILD, FIRST).await;
            sqlx::query("UPDATE tasks SET is_epic = 0 WHERE id = ?")
                .bind(FIRST)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query("DELETE FROM meta WHERE key = ?")
                .bind(REPAIR_KEY)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        drop(database);
        let database = crate::db::Database::open(&temp.path().join("replica.sqlite"))
            .await
            .unwrap();
        let mut conn = database.acquire_reader().await.unwrap();
        assert_eq!(parent(&mut conn, CHILD).await.as_deref(), Some(SECOND));
        assert_eq!(parent(&mut conn, OTHER_CHILD).await, None);
        let is_epic: bool = sqlx::query_scalar("SELECT is_epic FROM tasks WHERE id = ?")
            .bind(FIRST)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert!(!is_epic);
        assert_eq!(
            get_meta(&mut conn, REPAIR_KEY).await.unwrap().as_deref(),
            Some("1")
        );
    }

    #[tokio::test]
    async fn open_repairs_quiet_readd_after_demotion_with_valid_parent_flag() {
        let (temp, database) = database().await;
        let demotion_id;
        {
            let mut conn = database.acquire_writer().await.unwrap();
            sqlx::query("UPDATE changes SET server_seq = 1")
                .execute(&mut *conn)
                .await
                .unwrap();
            history(&mut conn, CHILD, FIRST, true, 2).await;
            history(&mut conn, CHILD, FIRST, false, 3).await;
            demotion_id = crate::db::insert_change(
                &mut conn,
                "task",
                FIRST,
                Some("is_epic"),
                op_type::SET_FIELD,
                serde_json::json!({
                    "workspace_id": WORKSPACE, "workspace_key": "default", "value": "0",
                }),
                None,
            )
            .await
            .unwrap();
            sqlx::query("UPDATE changes SET server_seq = 4 WHERE change_id = ?")
                .bind(&demotion_id)
                .execute(&mut *conn)
                .await
                .unwrap();
            crate::db::set_field_version(&mut conn, FIRST, "is_epic", &demotion_id)
                .await
                .unwrap();
            history(&mut conn, CHILD, FIRST, false, 5).await;
            history(&mut conn, CHILD, FIRST, true, 6).await;
            sqlx::query("UPDATE tasks SET is_epic = 0 WHERE id = ?")
                .bind(FIRST)
                .execute(&mut *conn)
                .await
                .unwrap();
            set_meta(&mut conn, "local_seq", "6").await.unwrap();
            set_meta(&mut conn, "sync_cursor", "6").await.unwrap();
            sqlx::query("DELETE FROM meta WHERE key = ?")
                .bind(REPAIR_KEY)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        drop(database);
        let database = crate::db::Database::open(&temp.path().join("replica.sqlite"))
            .await
            .unwrap();
        {
            let mut conn = database.acquire_reader().await.unwrap();
            assert_eq!(parent(&mut conn, CHILD).await.as_deref(), Some(FIRST));
            let is_epic: bool = sqlx::query_scalar("SELECT is_epic FROM tasks WHERE id = ?")
                .bind(FIRST)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert!(is_epic);
            assert_eq!(
                crate::db::field_version(&mut conn, FIRST, "is_epic")
                    .await
                    .unwrap(),
                Some(demotion_id),
            );
            let changes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(changes, 6);
        }
        let facts = database.sync_persistence_status().await.unwrap();
        assert_eq!(facts.pending_changes, 0);
        assert_eq!(facts.sync_cursor.as_deref(), Some("6"));
        let integrity = database.database_integrity_report().await.unwrap();
        assert!(integrity.quick_check_ok);
        assert!(
            integrity.checks.iter().all(|check| check.ok),
            "{integrity:?}"
        );
    }

    #[tokio::test]
    async fn snapshot_baseline_survives_removal_and_repeated_recovery() {
        let (_temp, database) = database().await;
        {
            let mut conn = database.acquire_writer().await.unwrap();
            snapshot(&mut conn, CHILD, FIRST).await;
        }
        database
            .remove_task_from_epic(
                &crate::workspaces::Workspace::default(),
                &CHILD.parse().unwrap(),
                &FIRST.parse().unwrap(),
            )
            .await
            .unwrap();
        let mut conn = database.acquire_writer().await.unwrap();
        assert!(
            get_meta(&mut conn, &baseline_key(WORKSPACE, CHILD))
                .await
                .unwrap()
                .is_some()
        );
        for _ in 0..2 {
            recover(&mut conn, true).await.unwrap();
            assert_eq!(parent(&mut conn, CHILD).await, None);
        }
    }

    #[tokio::test]
    async fn incomplete_history_without_baseline_preserves_snapshot() {
        let (_temp, database) = database().await;
        let mut conn = database.acquire_writer().await.unwrap();
        snapshot(&mut conn, CHILD, FIRST).await;
        history(&mut conn, CHILD, FIRST, false, 1).await;
        recover(&mut conn, true).await.unwrap();
        assert_eq!(parent(&mut conn, CHILD).await.as_deref(), Some(FIRST));
        assert_eq!(
            get_meta(&mut conn, &baseline_key(WORKSPACE, CHILD))
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn import_rejects_invalid_baseline_identity_or_missing_tasks_before_replacement() {
        let (_temp, database) = database().await;
        let mut export = database.export_data("t".to_string()).await.unwrap();
        let valid = serde_json::json!({"epic_task_id": FIRST, "created_at": "t"}).to_string();
        for (key, value) in [
            (baseline_key(WORKSPACE, CHILD), "{".to_string()),
            (baseline_key("invalid", CHILD), valid.clone()),
            (baseline_key(WORKSPACE, "invalid"), valid.clone()),
            (baseline_key(WORKSPACE, "DDDD000000000001"), valid),
            (
                baseline_key(WORKSPACE, CHILD),
                serde_json::json!({"epic_task_id": "DDDD000000000001", "created_at": "t"})
                    .to_string(),
            ),
            (
                baseline_key(WORKSPACE, CHILD),
                serde_json::json!({"epic_task_id": CHILD, "created_at": "t"}).to_string(),
            ),
        ] {
            export
                .tables
                .meta
                .push(crate::data_safety::MetaRow { key, value });
            assert!(database.validate_import_data(&export).await.is_err());
            assert!(database.import_data(&export).await.is_err());
            export.tables.meta.pop();
            let mut conn = database.acquire_reader().await.unwrap();
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(count, 4);
        }
    }

    #[tokio::test]
    async fn within_page_demotion_checks_reconciled_pending_membership() {
        use crate::sync::ApplySyncPage;
        use crate::sync::wire::{ChangeWire, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse};

        let (_temp, database) = database().await;
        {
            let mut conn = database.acquire_writer().await.unwrap();
            history(&mut conn, CHILD, FIRST, true, 1).await;
            snapshot(&mut conn, CHILD, FIRST).await;
            set_meta(&mut conn, "sync_cursor", "1").await.unwrap();
        }
        let workspace = crate::workspaces::Workspace::default();
        database
            .remove_task_from_epic(&workspace, &CHILD.parse().unwrap(), &FIRST.parse().unwrap())
            .await
            .unwrap();
        database
            .add_task_to_epic(&workspace, &CHILD.parse().unwrap(), &FIRST.parse().unwrap())
            .await
            .unwrap();
        let remove = ChangeWire {
            change_id: crate::ids::new_id(),
            client_id: "remote".to_string(),
            local_seq: 2,
            entity_type: "task".to_string(),
            entity_id: CHILD.to_string(),
            field: Some("epics".to_string()),
            op_type: op_type::EPIC_LINK_REMOVE.to_string(),
            payload: serde_json::json!({
                "workspace_id": WORKSPACE, "workspace_key": "default", "epic_task_id": FIRST,
            }),
            base_version: None,
            created_at: "2026-09-06T00:00:00Z".to_string(),
            server_seq: Some(2),
        };
        let demote = ChangeWire {
            change_id: crate::ids::new_id(),
            entity_id: FIRST.to_string(),
            field: Some("is_epic".to_string()),
            op_type: op_type::SET_FIELD.to_string(),
            payload: serde_json::json!({
                "workspace_id": WORKSPACE, "workspace_key": "default", "value": "0",
            }),
            local_seq: 3,
            server_seq: Some(3),
            ..remove.clone()
        };
        database
            .apply_client_sync_page(ApplySyncPage {
                request: SyncRequest {
                    protocol_version: Some(SYNC_PROTOCOL_VERSION),
                    client_id: "local".to_string(),
                    after: 1,
                    pull_limit: Some(100),
                    changes: Vec::new(),
                },
                response: SyncResponse {
                    protocol_version: SYNC_PROTOCOL_VERSION,
                    cursor: 3,
                    has_more: false,
                    push_acks: Vec::new(),
                    changes: vec![remove, demote],
                },
                attempted_at: "2026-09-06T00:00:00Z".to_string(),
                previous_pushed: 0,
                previous_pulled: 0,
            })
            .await
            .unwrap();
        let mut conn = database.acquire_reader().await.unwrap();
        assert_eq!(parent(&mut conn, CHILD).await.as_deref(), Some(FIRST));
        let conflicts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conflicts WHERE field = 'is_epic' AND resolved = 0",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(conflicts, 1);
        let is_epic: bool = sqlx::query_scalar("SELECT is_epic FROM tasks WHERE id = ?")
            .bind(FIRST)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert!(is_epic);
    }
}
