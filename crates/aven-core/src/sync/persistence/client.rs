use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use sqlx::SqliteConnection;

use super::super::apply::apply_remote_change;
use super::{ApplySyncPage, ClientSyncPage};
use crate::change_log::op_type;
use crate::db::{Database, begin_immediate, get_meta, set_meta};
use crate::sync::wire::{
    ChangeRow, ChangeWire, MAX_PUSH_BATCH, MAX_SYNC_REQUEST_BYTES, SyncRequest,
};

impl Database {
    pub(in crate::sync) async fn ensure_plaintext_sync_available(
        &self,
        expected_generation: Option<i64>,
    ) -> Result<()> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_reader().await?;
        super::super::shared_state::ensure_no_active_local_shared_capture(&mut conn).await?;
        if let Some(expected) = expected_generation {
            anyhow::ensure!(
                sync_generation(&mut conn).await? == expected,
                "error stale-sync-page sync-generation-changed"
            );
        }
        Ok(())
    }

    pub(in crate::sync) async fn pending_sync_changes_exist(&self) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes WHERE server_seq IS NULL)")
                .fetch_one(&mut *conn)
                .await?,
        )
    }

    pub(in crate::sync) async fn pending_blob_counts(
        &self,
        known_server_blobs: &HashSet<String>,
    ) -> Result<crate::attachments::lifecycle::ByteCount> {
        let mut conn = self.acquire_reader().await?;
        let known = serde_json::to_string(known_server_blobs)?;
        let (count, bytes): (i64, i64) = sqlx::query_as(
            "WITH known(sha256) AS (
               SELECT value FROM json_each(?)
             ), pending AS (
               SELECT json_extract(payload, '$.workspace_id') AS workspace_id,
                      json_extract(payload, '$.sha256') AS sha256,
                      MAX(CAST(json_extract(payload, '$.byte_size') AS INTEGER)) AS byte_size
               FROM changes
               WHERE server_seq IS NULL AND op_type = 'attachment_add'
               GROUP BY workspace_id, sha256
             )
             SELECT COUNT(*), COALESCE(SUM(byte_size), 0)
             FROM pending LEFT JOIN known USING (sha256)
             WHERE known.sha256 IS NULL",
        )
        .bind(known)
        .fetch_one(&mut *conn)
        .await?;
        Ok(crate::attachments::lifecycle::ByteCount {
            count: u64::try_from(count)?,
            bytes: u64::try_from(bytes)?,
        })
    }

    pub(in crate::sync) async fn replica_sync_protocol(&self) -> Result<u32> {
        let mut conn = self.acquire_reader().await?;
        super::super::protocol::replica_protocol(&mut conn).await
    }

    pub(in crate::sync) async fn prepare_sync_discovery(&self, server: &str) -> Result<String> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        super::super::shared_state::adoption::ensure_unbound(&mut conn).await?;
        validate_sync_server(&mut conn, server).await?;
        super::super::protocol::replica_protocol(&mut conn).await?;
        get_meta(&mut conn, "client_id")
            .await?
            .context("missing client id")
    }

    pub(in crate::sync) async fn block_sync_protocol(&self, protocol: u32) -> Result<()> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        super::super::shared_state::adoption::ensure_unbound(&mut conn).await?;
        set_meta(&mut conn, "sync_blocked_protocol", &protocol.to_string()).await
    }

    pub async fn prepare_client_sync_page(
        &self,
        server: String,
        push_limit: usize,
        pull_limit: u32,
    ) -> Result<ClientSyncPage> {
        self.prepare_client_sync_page_at_protocol(server, push_limit, pull_limit, None)
            .await
    }

    pub(in crate::sync) async fn prepare_client_sync_page_at_protocol(
        &self,
        server: String,
        push_limit: usize,
        pull_limit: u32,
        protocol: Option<u32>,
    ) -> Result<ClientSyncPage> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        super::super::shared_state::adoption::ensure_unbound(&mut conn).await?;
        super::super::shared_state::ensure_no_active_local_shared_capture(&mut conn).await?;
        validate_sync_server(&mut conn, &server).await?;
        let behavior_protocol = super::super::protocol::replica_protocol(&mut conn).await?;
        let sync_generation = sync_generation(&mut conn).await?;
        let protocol = protocol.unwrap_or(behavior_protocol);
        super::super::protocol::validate_behavior_protocol(protocol)?;
        if protocol < behavior_protocol {
            return Err(super::super::protocol::SyncCompatibilityError {
                server_protocol: protocol,
                client_protocol: behavior_protocol,
            }
            .into());
        }
        let client_id = get_meta(&mut conn, "client_id")
            .await?
            .context("missing client id")?;
        let after = sync_cursor(&mut conn).await?;
        let changes = load_unsynced_changes(&mut conn, push_limit.min(MAX_PUSH_BATCH)).await?;
        let request = bound_push_request(
            SyncRequest {
                protocol_version: Some(protocol),
                client_id,
                after,
                pull_limit: Some(pull_limit),
                changes,
            },
            MAX_SYNC_REQUEST_BYTES,
        )?;
        for change in &request.changes {
            super::super::protocol::validate_change(protocol, change)?;
        }
        Ok(ClientSyncPage {
            behavior_protocol,
            sync_generation,
            pending: request.changes.len(),
            request,
        })
    }

    pub async fn apply_client_sync_page(&self, page: ApplySyncPage) -> Result<usize> {
        self.apply_client_sync_page_with_context(page, None).await
    }

    pub(in crate::sync) async fn apply_client_sync_page_with_context(
        &self,
        page: ApplySyncPage,
        expected_behavior: Option<u32>,
    ) -> Result<usize> {
        let protocol = page
            .request
            .protocol_version
            .context("missing selected protocol")?;
        super::super::protocol::validate_behavior_protocol(protocol)?;
        let envelope = crate::sync::wire::validate_request_at_protocol(&page.request, protocol)?;
        let request_change_ids = page
            .request
            .changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect::<Vec<_>>();
        crate::sync::wire::validate_response_at_protocol(
            protocol,
            envelope.after,
            envelope.pull_limit,
            &request_change_ids,
            &page.response,
        )?;
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        super::super::shared_state::adoption::ensure_unbound(&mut conn).await?;
        apply_sync_response(&mut conn, page, expected_behavior).await
    }
}

async fn sync_generation(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(crate::db::get_meta(conn, "sync_generation")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?)
}

async fn sync_cursor(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(crate::db::get_meta(conn, "sync_cursor")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?)
}

async fn validate_sync_server(conn: &mut SqliteConnection, server: &str) -> Result<()> {
    let normalized = server.trim_end_matches('/');
    if let Some(existing) = get_meta(conn, "sync_server_url").await? {
        if existing != normalized {
            bail!(
                "error sync-server-changed existing={} requested={} hint=\"use a fresh database for a different sync server\"",
                existing,
                normalized
            );
        }
    } else {
        set_meta(conn, "sync_server_url", normalized).await?;
    }
    Ok(())
}

pub(super) fn bound_push_request(
    mut request: SyncRequest,
    byte_limit: usize,
) -> Result<SyncRequest> {
    let changes = std::mem::take(&mut request.changes);
    // The empty array already accounts for brackets and the complete request envelope.
    let mut bytes = serde_json::to_vec(&request)?.len();
    if bytes > byte_limit {
        bail!("error sync-request-envelope-too-large limit={byte_limit}");
    }
    for change in changes.into_iter().take(MAX_PUSH_BATCH) {
        let change_bytes = serde_json::to_vec(&change)?.len();
        let separator = usize::from(!request.changes.is_empty());
        if change_bytes + separator > byte_limit - bytes {
            if request.changes.is_empty() {
                bail!(
                    "error sync-change-exceeds-request-budget local_seq={} limit={byte_limit} hint=repair-pending-change",
                    change.local_seq
                );
            }
            break;
        }
        bytes += change_bytes + separator;
        request.changes.push(change);
    }
    Ok(request)
}

async fn load_unsynced_changes(
    conn: &mut SqliteConnection,
    limit: usize,
) -> Result<Vec<ChangeWire>> {
    let limit = limit as i64;
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq IS NULL ORDER BY local_seq, created_at LIMIT ?"#,
        limit,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(ChangeRow::into_wire).collect())
}

pub(super) async fn apply_sync_response(
    conn: &mut SqliteConnection,
    page: ApplySyncPage,
    expected_behavior: Option<u32>,
) -> Result<usize> {
    let mut applied = 0;
    let mut tx = begin_immediate(conn).await?;
    super::super::shared_state::ensure_no_active_local_shared_capture(&mut tx).await?;
    let current_generation = sync_generation(&mut tx).await?;
    if current_generation != page.sync_generation {
        bail!(
            "error stale-sync-page sync-generation-changed expected={} prepared={}",
            current_generation,
            page.sync_generation
        );
    }
    let current_behavior = super::super::protocol::replica_protocol(&mut tx).await?;
    if expected_behavior.is_some_and(|expected| expected != current_behavior) {
        bail!("error stale-sync-page replica-protocol-changed");
    }
    let selected = page
        .request
        .protocol_version
        .context("missing selected protocol")?;
    if selected < current_behavior {
        bail!("error stale-sync-page replica-protocol-regressed");
    }
    super::super::protocol::establish_protocol(&mut tx, selected).await?;
    let current_cursor = sync_cursor(&mut tx).await?;
    if current_cursor != page.request.after {
        bail!(
            "error stale-sync-page expected_cursor={} request_cursor={}",
            current_cursor,
            page.request.after
        );
    }
    super::update_change_server_seqs_if_missing(&mut tx, &page.response.push_acks).await?;
    super::reconcile_acknowledged_epic_memberships(&mut tx, &page.response.push_acks).await?;
    let existing_change_ids =
        super::load_existing_change_ids(&mut tx, &page.response.changes).await?;
    let mut affected_series = HashSet::new();
    let mut affected_attachment_hashes = HashSet::new();
    for change in &page.response.changes {
        if existing_change_ids.contains(change.change_id.as_str()) {
            super::verify_existing_change(&mut tx, change).await?;
            super::update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            super::reconcile_epic_change(&mut tx, change).await?;
            continue;
        }
        super::collect_attachment_liveness_hashes(&mut tx, change, &mut affected_attachment_hashes)
            .await?;
        if super::is_epic_change(change) {
            let workspace_id = super::epic_change_workspace(change)?;
            crate::epic_membership::capture_snapshot_baseline(
                &mut tx,
                workspace_id,
                &change.entity_id,
            )
            .await?;
        }
        let related_mutation = matches!(
            change.op_type.as_str(),
            op_type::RELATED_ADD | op_type::RELATED_REMOVE
        );
        if related_mutation {
            super::insert_wire_change(&mut tx, change).await?;
        }
        apply_remote_change(&mut tx, change).await?;
        if change.entity_type == "recurrence_series" {
            let workspace_id = change
                .payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .context("recurrence change missing workspace_id")?;
            affected_series.insert((workspace_id.to_string(), change.entity_id.clone()));
        } else if let Some(series_id) = change
            .payload
            .get("series_id")
            .and_then(serde_json::Value::as_str)
        {
            let workspace_id = change
                .payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .context("recurrence task change missing workspace_id")?;
            affected_series.insert((workspace_id.to_string(), series_id.to_string()));
        }
        if !related_mutation {
            super::insert_wire_change(&mut tx, change).await?;
        }
        super::reconcile_epic_change(&mut tx, change).await?;
        applied += 1;
    }
    for (workspace_id, series_id) in affected_series {
        let workspace_id: crate::ids::WorkspaceId = workspace_id.parse()?;
        let series_id: crate::recurrence::RecurrenceSeriesId = series_id.parse()?;
        let workspace = crate::workspaces::workspace_for_id(&mut tx, &workspace_id).await?;
        let at =
            chrono::DateTime::parse_from_rfc3339(&page.attempted_at)?.with_timezone(&chrono::Utc);
        crate::operations::recurrence::reconcile_recurrence_series_in_transaction(
            &mut tx, &workspace, &series_id, at,
        )
        .await?;
    }
    let affected_attachment_hashes = affected_attachment_hashes.into_iter().collect::<Vec<_>>();
    crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
        &mut tx,
        &affected_attachment_hashes,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let pushed = page.previous_pushed + page.response.push_acks.len() as i64;
    let pulled = page.previous_pulled + applied;
    set_meta(&mut tx, "sync_cursor", &page.response.cursor.to_string()).await?;
    set_meta(&mut tx, "sync_last_success_at", &page.attempted_at).await?;
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes WHERE server_seq IS NULL)")
            .fetch_one(&mut *tx)
            .await?;
    let caught_up = !page.response.has_more && !pending;
    set_meta(
        &mut tx,
        "sync_metadata_caught_up",
        if caught_up { "1" } else { "0" },
    )
    .await?;
    if caught_up {
        set_meta(&mut tx, "sync_metadata_confirmed_at", &crate::ids::now()).await?;
    }

    set_meta(&mut tx, "sync_last_error", "").await?;
    set_meta(&mut tx, "sync_blocked_protocol", "").await?;
    set_meta(&mut tx, "sync_last_pushed", &pushed.to_string()).await?;
    set_meta(&mut tx, "sync_last_pulled", &pulled.to_string()).await?;
    set_meta(
        &mut tx,
        "sync_last_cursor",
        &page.response.cursor.to_string(),
    )
    .await?;
    tx.commit().await?;
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::super::ApplySyncPage;
    use super::*;
    use crate::change_log::op_type;
    use crate::db::set_meta;
    use crate::sync::wire::{
        ChangeWire, MAX_PUSH_BATCH, MAX_SYNC_REQUEST_BYTES, PushAck, SYNC_PROTOCOL_VERSION,
        SyncRequest, SyncResponse,
    };
    use serde_json::json;

    fn budget_request() -> SyncRequest {
        SyncRequest {
            protocol_version: Some(SYNC_PROTOCOL_VERSION),
            client_id: "budget-client\"\n".to_string(),
            after: i64::MAX,
            pull_limit: Some(crate::sync::wire::MAX_PULL_BATCH),
            changes: (0..3)
                .map(|index| ChangeWire {
                    change_id: format!("AAAAAAAAAAAAAAA{index}"),
                    client_id: "budget-client".to_string(),
                    local_seq: index + 1,
                    entity_type: "task".to_string(),
                    entity_id: "BBBBBBBBBBBBBBB0".to_string(),
                    field: Some("description".to_string()),
                    op_type: op_type::SET_FIELD.to_string(),
                    payload: json!({
                        "workspace_id": "0000000000000000",
                        "workspace_key": "default",
                        "value": "quoted \"text\"\n雪".repeat(8),
                    }),
                    base_version: None,
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    server_seq: None,
                })
                .collect(),
        }
    }

    #[test]
    fn push_byte_budget_selects_exact_ordered_prefix() {
        let request = budget_request();
        for change in &request.changes {
            crate::sync::wire::validate_pushed_change(change).unwrap();
        }
        let mut prefix = request.clone();
        prefix.changes.truncate(2);
        let limit = serde_json::to_vec(&prefix).unwrap().len();
        let selected = bound_push_request(request.clone(), limit).unwrap();
        assert_eq!(
            serde_json::to_vec(&selected).unwrap(),
            serde_json::to_vec(&prefix).unwrap()
        );
        let selected = bound_push_request(request, limit - 1).unwrap();
        assert_eq!(selected.changes.len(), 1);
        assert_eq!(selected.changes[0].local_seq, 1);
        assert!(serde_json::to_vec(&selected).unwrap().len() < limit);
    }

    #[test]
    fn push_byte_budget_rejects_unfit_first_change_and_envelope() {
        let mut request = budget_request();
        request.changes.truncate(1);
        let limit = serde_json::to_vec(&request).unwrap().len();
        assert_eq!(
            bound_push_request(request.clone(), limit)
                .unwrap()
                .changes
                .len(),
            1
        );
        assert!(
            bound_push_request(request.clone(), limit - 1)
                .unwrap_err()
                .to_string()
                .contains("sync-change-exceeds-request-budget")
        );
        request.changes.clear();
        let limit = serde_json::to_vec(&request).unwrap().len();
        assert!(
            bound_push_request(request.clone(), limit)
                .unwrap()
                .changes
                .is_empty()
        );
        assert!(
            bound_push_request(request, limit - 1)
                .unwrap_err()
                .to_string()
                .contains("sync-request-envelope-too-large")
        );
    }

    #[test]
    fn push_byte_budget_does_not_skip_a_change_that_does_not_fit() {
        let mut request = budget_request();
        let mut prefix = request.clone();
        prefix.changes.truncate(2);
        let limit = serde_json::to_vec(&prefix).unwrap().len();
        request.changes[1].payload["value"] = json!("middle".repeat(limit));
        let selected = bound_push_request(request, limit).unwrap();
        assert_eq!(selected.changes.len(), 1);
        assert_eq!(selected.changes[0].local_seq, 1);
    }

    #[test]
    fn push_byte_budget_preserves_count_bound() {
        let mut request = budget_request();
        request.changes = vec![request.changes[0].clone(); MAX_PUSH_BATCH + 1];
        let selected = bound_push_request(request, MAX_SYNC_REQUEST_BYTES).unwrap();
        assert_eq!(selected.changes.len(), MAX_PUSH_BATCH);
    }

    #[tokio::test]
    async fn related_comparison_observes_push_acknowledgement_first() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace = crate::workspaces::Workspace::default();
        let project = crate::projects::create_project(&mut conn, &workspace, "related-ack")
            .await
            .unwrap();
        let task_id: crate::ids::TaskId = "AAAA000000000001".parse().unwrap();
        let related_task_id: crate::ids::TaskId = "BBBB000000000002".parse().unwrap();
        for id in [&task_id, &related_task_id] {
            sqlx::query(
                "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
                 VALUES (?, ?, 'task', '', ?, 'todo', 'none', 't', 't')",
            )
            .bind(id)
            .bind(&workspace.id)
            .bind(&project.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        let local = crate::operations::set_task_related_link_in_transaction(
            &mut conn,
            &workspace,
            &task_id,
            &related_task_id,
            true,
        )
        .await
        .unwrap();
        let local_change_id = local.change_id.unwrap();
        set_meta(&mut conn, "sync_cursor", "3").await.unwrap();

        let remote = ChangeWire {
            change_id: "CCCC000000000003".to_string(),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: task_id.to_string(),
            field: Some("related".to_string()),
            op_type: op_type::RELATED_REMOVE.to_string(),
            payload: json!({
                "workspace_id": workspace.id,
                "workspace_key": workspace.key,
                "related_task_id": related_task_id,
            }),
            base_version: None,
            created_at: "2026-08-22T00:00:00Z".to_string(),
            server_seq: Some(4),
        };
        let page = ApplySyncPage {
            sync_generation: 0,
            request: SyncRequest {
                protocol_version: Some(SYNC_PROTOCOL_VERSION),
                client_id: "local".to_string(),
                after: 3,
                pull_limit: Some(100),
                changes: Vec::new(),
            },
            response: SyncResponse {
                protocol_version: SYNC_PROTOCOL_VERSION,
                cursor: 4,
                has_more: false,
                push_acks: vec![PushAck {
                    change_id: local_change_id.clone(),
                    server_seq: 5,
                }],
                changes: vec![remote],
            },
            attempted_at: "2026-08-22T00:00:01Z".to_string(),
            previous_pushed: 0,
            previous_pulled: 0,
        };

        apply_sync_response(&mut conn, page, None).await.unwrap();

        let state: (i64, String) = sqlx::query_as(
            "SELECT linked, last_change_id FROM task_related_links
             WHERE workspace_id = ? AND task_a_id = ? AND task_b_id = ?",
        )
        .bind(&workspace.id)
        .bind(&task_id)
        .bind(&related_task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(state, (1, local_change_id.clone()));
        let acknowledged: Option<i64> =
            sqlx::query_scalar("SELECT server_seq FROM changes WHERE change_id = ?")
                .bind(local_change_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(acknowledged, Some(5));
    }
}
