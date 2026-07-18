use std::collections::HashSet;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};

use super::apply::apply_remote_change;
use super::wire::{ChangeRow, ChangeWire, PushAck, SyncRequest, SyncResponse};
use crate::db::{Database, begin_immediate, get_meta, set_meta};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPersistenceStatus {
    pub pinned_server: Option<String>,
    pub pending_changes: i64,
    pub conflicts: i64,
    pub sync_cursor: Option<String>,
    pub local_sequence: Option<String>,
    pub last_attempt: Option<String>,
    pub last_success: Option<String>,
    pub last_error: Option<String>,
    pub last_pushed: Option<String>,
    pub last_pulled: Option<String>,
    pub last_cursor: Option<String>,
}

#[derive(Debug)]
pub struct ClientSyncPage {
    pub request: SyncRequest,
    pub pending: usize,
}

#[derive(Debug)]
pub struct ApplySyncPage {
    pub request: SyncRequest,
    pub response: SyncResponse,
    pub attempted_at: String,
    pub previous_pushed: i64,
    pub previous_pulled: usize,
}

#[derive(Debug)]
pub struct ServerSyncPage {
    pub request: SyncRequest,
}

#[derive(Debug)]
pub struct ServerSyncResult {
    pub accepted_count: i64,
    pub push_acks: Vec<PushAck>,
    pub changes: Vec<ChangeWire>,
    pub has_more: bool,
    pub assign_ms: u128,
    pub pull_query_ms: u128,
}

impl Database {
    pub async fn sync_persistence_status(&self) -> Result<SyncPersistenceStatus> {
        let mut conn = self.acquire().await?;
        let pending_changes =
            sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await?;
        let conflicts = sqlx::query_scalar("SELECT count(*) FROM conflicts WHERE resolved = 0")
            .fetch_one(&mut *conn)
            .await?;
        Ok(SyncPersistenceStatus {
            pinned_server: get_meta(&mut conn, "sync_server_url").await?,
            pending_changes,
            conflicts,
            sync_cursor: get_meta(&mut conn, "sync_cursor").await?,
            local_sequence: get_meta(&mut conn, "local_seq").await?,
            last_attempt: get_meta(&mut conn, "sync_last_attempt_at").await?,
            last_success: get_meta(&mut conn, "sync_last_success_at").await?,
            last_error: get_meta(&mut conn, "sync_last_error").await?,
            last_pushed: get_meta(&mut conn, "sync_last_pushed").await?,
            last_pulled: get_meta(&mut conn, "sync_last_pulled").await?,
            last_cursor: get_meta(&mut conn, "sync_last_cursor").await?,
        })
    }

    pub async fn begin_sync_attempt(&self, attempted_at: String) -> Result<()> {
        let mut conn = self.acquire().await?;
        set_meta(&mut conn, "sync_last_attempt_at", &attempted_at).await
    }

    pub async fn record_sync_error(&self, error: String) -> Result<()> {
        let mut conn = self.acquire().await?;
        set_meta(&mut conn, "sync_last_error", &error).await
    }

    pub async fn prepare_client_sync_page(
        &self,
        server: String,
        push_limit: usize,
        pull_limit: u32,
    ) -> Result<ClientSyncPage> {
        let mut conn = self.acquire().await?;
        validate_sync_server(&mut conn, &server).await?;
        let client_id = get_meta(&mut conn, "client_id")
            .await?
            .context("missing client id")?;
        let after = sync_cursor(&mut conn).await?;
        let changes = load_unsynced_changes(&mut conn, push_limit).await?;
        let pending = changes.len();
        Ok(ClientSyncPage {
            request: SyncRequest {
                protocol_version: Some(super::wire::SYNC_PROTOCOL_VERSION),
                client_id,
                after,
                pull_limit: Some(pull_limit),
                changes,
            },
            pending,
        })
    }

    pub async fn apply_client_sync_page(&self, page: ApplySyncPage) -> Result<usize> {
        let envelope = super::wire::validate_sync_request_envelope(&page.request)?;
        let request_change_ids = page
            .request
            .changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect::<Vec<_>>();
        super::wire::validate_sync_response_for_request(
            envelope.after,
            envelope.pull_limit,
            &request_change_ids,
            &page.response,
        )?;
        let mut conn = self.acquire().await?;
        apply_sync_response(&mut conn, page).await
    }

    pub async fn persist_server_sync_page(&self, page: ServerSyncPage) -> Result<ServerSyncResult> {
        let envelope = super::wire::validate_sync_request_envelope(&page.request)?;
        for change in &page.request.changes {
            super::wire::validate_pushed_change(change)?;
        }
        let mut conn = self.acquire().await?;
        let assign_started = Instant::now();
        let (accepted_count, push_acks) =
            assign_server_sequences(&mut conn, page.request.changes).await?;
        let assign_ms = assign_started.elapsed().as_millis();
        let pull_query_started = Instant::now();
        let (changes, has_more) =
            load_server_changes_after(&mut conn, envelope.after, envelope.pull_limit).await?;
        let pull_query_ms = pull_query_started.elapsed().as_millis();
        Ok(ServerSyncResult {
            accepted_count,
            push_acks,
            changes,
            has_more,
            assign_ms,
            pull_query_ms,
        })
    }
}

async fn sync_cursor(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(get_meta(conn, "sync_cursor")
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

async fn apply_sync_response(conn: &mut SqliteConnection, page: ApplySyncPage) -> Result<usize> {
    let mut applied = 0;
    let mut tx = begin_immediate(conn).await?;
    update_change_server_seqs_if_missing(&mut tx, &page.response.push_acks).await?;
    let existing_change_ids = load_existing_change_ids(&mut tx, &page.response.changes).await?;
    for change in &page.response.changes {
        if existing_change_ids.contains(change.change_id.as_str()) {
            update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            continue;
        }
        apply_remote_change(&mut tx, change).await?;
        insert_wire_change(&mut tx, change).await?;
        applied += 1;
    }
    let pushed = page.previous_pushed + page.response.push_acks.len() as i64;
    let pulled = page.previous_pulled + applied;
    set_meta(&mut tx, "sync_cursor", &page.response.cursor.to_string()).await?;
    set_meta(&mut tx, "sync_last_success_at", &page.attempted_at).await?;
    set_meta(&mut tx, "sync_last_error", "").await?;
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

async fn load_existing_change_ids(
    conn: &mut SqliteConnection,
    changes: &[ChangeWire],
) -> Result<HashSet<String>> {
    if changes.is_empty() {
        return Ok(HashSet::new());
    }
    let mut query_builder =
        QueryBuilder::<Sqlite>::new("SELECT change_id FROM changes WHERE change_id IN (");
    let mut separated = query_builder.separated(", ");
    for change in changes {
        separated.push_bind(&change.change_id);
    }
    separated.push_unseparated(")");
    Ok(query_builder
        .build_query_scalar::<String>()
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect())
}

async fn update_change_server_seq(
    conn: &mut SqliteConnection,
    change_id: &str,
    server_seq: Option<i64>,
) -> Result<()> {
    if let Some(server_seq) = server_seq {
        sqlx::query!(
            "UPDATE changes SET server_seq = ? WHERE change_id = ? AND server_seq IS NULL",
            server_seq,
            change_id,
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn update_change_server_seqs_if_missing(
    conn: &mut SqliteConnection,
    push_acks: &[PushAck],
) -> Result<()> {
    if push_acks.is_empty() {
        return Ok(());
    }
    let mut query_builder = QueryBuilder::<Sqlite>::new("WITH updates(change_id, server_seq) AS (");
    query_builder.push_values(push_acks, |mut row, ack| {
        row.push_bind(&ack.change_id).push_bind(ack.server_seq);
    });
    query_builder.push(
        ") UPDATE changes
         SET server_seq = (
             SELECT updates.server_seq
             FROM updates
             WHERE updates.change_id = changes.change_id
         )
         WHERE server_seq IS NULL
           AND change_id IN (SELECT change_id FROM updates)",
    );
    query_builder.build().execute(&mut *conn).await?;
    Ok(())
}

async fn insert_wire_change(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let payload = change.payload.to_string();
    sqlx::query!(
        "INSERT OR IGNORE INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at, server_seq)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        change.change_id,
        change.client_id,
        change.local_seq,
        change.entity_type,
        change.entity_id,
        change.field,
        change.op_type,
        payload,
        change.base_version,
        change.created_at,
        change.server_seq,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn assign_server_sequences(
    conn: &mut SqliteConnection,
    changes: Vec<ChangeWire>,
) -> Result<(i64, Vec<PushAck>)> {
    if changes.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mut tx = begin_immediate(conn).await?;
    let mut next_server_seq = next_available_server_seq(&mut tx).await?;
    let mut accepted_count = 0_i64;
    let mut push_acks = Vec::with_capacity(changes.len());
    for change in changes {
        let existing_server_seq = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT server_seq FROM changes WHERE change_id = ?",
        )
        .bind(&change.change_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let server_seq = if let Some(server_seq) = existing_server_seq {
            server_seq
        } else {
            let server_seq = next_server_seq;
            next_server_seq += 1;
            let payload = change.payload.to_string();
            sqlx::query(
                "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
                 op_type, payload, base_version, created_at, server_seq)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&change.change_id)
            .bind(&change.client_id)
            .bind(change.local_seq)
            .bind(&change.entity_type)
            .bind(&change.entity_id)
            .bind(&change.field)
            .bind(&change.op_type)
            .bind(payload)
            .bind(&change.base_version)
            .bind(&change.created_at)
            .bind(server_seq)
            .execute(&mut *tx)
            .await?;
            accepted_count += 1;
            server_seq
        };
        push_acks.push(PushAck {
            change_id: change.change_id,
            server_seq,
        });
    }
    tx.commit().await?;
    Ok((accepted_count, push_acks))
}

async fn next_available_server_seq(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(server_seq), 0) + 1 AS "seq!: i64" FROM changes"#
    )
    .fetch_one(&mut *conn)
    .await?)
}

async fn load_server_changes_after(
    conn: &mut SqliteConnection,
    after: i64,
    pull_limit: u32,
) -> Result<(Vec<ChangeWire>, bool)> {
    let fetch_limit = i64::from(pull_limit) + 1;
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq > ? ORDER BY server_seq LIMIT ?"#,
        after,
        fetch_limit,
    )
    .fetch_all(&mut *conn)
    .await?;
    let has_more = rows.len() > pull_limit as usize;
    let changes = rows
        .into_iter()
        .take(pull_limit as usize)
        .map(ChangeRow::into_wire)
        .collect();
    Ok((changes, has_more))
}
