use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};
use tracing::info;

use super::apply::apply_remote_change;
use super::wire::{
    ChangeRow, ChangeWire, MAX_PULL_BATCH, MAX_PUSH_BATCH, SYNC_PROTOCOL_VERSION, SyncRequest,
    SyncResponse, validate_sync_response_for_request,
};
use crate::cli::SyncArgs;
use crate::config;
use crate::db::{begin_immediate, get_meta, set_meta};
use crate::ids::now;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SyncSummary {
    pub(crate) pushed: i64,
    pub(crate) pulled: usize,
    pub(crate) cursor: i64,
    pub(crate) complete: bool,
    pub(crate) pages: usize,
    pub(crate) request_bytes: usize,
    pub(crate) response_bytes: usize,
    pub(crate) apply_ms: u128,
}

pub(crate) async fn sync_client(
    conn: &mut SqliteConnection,
    args: SyncArgs,
    config: &config::AppConfig,
) -> Result<()> {
    let server = config::resolve_sync_server(args.server.as_deref(), config)?;
    let summary = run_sync_once(conn, &server, config.sync_auth_token()).await?;
    println!(
        "synced pushed={} pulled={} cursor={}",
        summary.pushed, summary.pulled, summary.cursor
    );
    Ok(())
}

pub(crate) async fn run_sync_once(
    conn: &mut SqliteConnection,
    server: &str,
    auth_token: Option<&str>,
) -> Result<SyncSummary> {
    run_sync_with_page_budget(conn, server, auth_token, None).await
}

pub(crate) async fn run_sync_with_page_budget(
    conn: &mut SqliteConnection,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
) -> Result<SyncSummary> {
    let attempted_at = now();
    set_meta(conn, "sync_last_attempt_at", &attempted_at).await?;
    match run_sync_once_inner(conn, server, auth_token, &attempted_at, page_budget).await {
        Ok(summary) => Ok(summary),
        Err(error) => {
            let error_text = format!("{error:#}");
            set_meta(conn, "sync_last_error", &error_text).await?;
            Err(error)
        }
    }
}

async fn run_sync_once_inner(
    conn: &mut SqliteConnection,
    server: &str,
    auth_token: Option<&str>,
    attempted_at: &str,
    page_budget: Option<usize>,
) -> Result<SyncSummary> {
    validate_sync_server(conn, server).await?;
    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let url = format!("{}/sync", server.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut total_pushed = 0_i64;
    let mut total_pulled = 0_usize;
    let mut cursor = sync_cursor(conn).await?;
    let mut pages = 0_usize;
    let complete;
    let mut total_request_bytes = 0_usize;
    let mut total_response_bytes = 0_usize;
    let mut total_apply_ms = 0_u128;
    info!(server = %server, "sync client starting");

    loop {
        let changes = load_unsynced_changes(conn, MAX_PUSH_BATCH).await?;
        let request_change_ids = changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect::<Vec<_>>();
        let pull_limit = MAX_PULL_BATCH;
        let pending = changes.len();
        let sync_request = SyncRequest {
            protocol_version: Some(SYNC_PROTOCOL_VERSION),
            client_id: client_id.clone(),
            after: cursor,
            pull_limit: Some(pull_limit),
            changes,
        };
        let request_body = serde_json::to_vec(&sync_request)?;
        let request_bytes = request_body.len();
        let mut request = client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_body);
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let http_started = Instant::now();
        let response_body = request.send().await?.error_for_status()?.bytes().await?;
        let http_ms = http_started.elapsed().as_millis();
        let response_bytes = response_body.len();
        let response: SyncResponse = serde_json::from_slice(&response_body)?;
        validate_sync_response_for_request(cursor, pull_limit, &request_change_ids, &response)?;
        let apply_started = Instant::now();
        let applied =
            apply_sync_response(conn, &response, attempted_at, total_pushed, total_pulled).await?;
        let apply_ms = apply_started.elapsed().as_millis();
        total_pushed += pending as i64;
        total_pulled += applied;
        cursor = response.cursor;
        pages += 1;
        total_request_bytes += request_bytes;
        total_response_bytes += response_bytes;
        total_apply_ms += apply_ms;

        let local_more = pending == MAX_PUSH_BATCH;
        let page_complete = !local_more && !response.has_more;
        let budget_exhausted = page_budget.is_some_and(|budget| pages >= budget);
        info!(
            server = %server,
            page = pages,
            pushed = pending,
            pulled = applied,
            cursor,
            complete = page_complete,
            request_bytes,
            response_bytes,
            http_ms,
            apply_ms,
            has_more = response.has_more,
            local_more,
            "sync client page completed"
        );
        if page_complete || budget_exhausted {
            complete = page_complete;
            break;
        }
    }

    info!(
        server = %server,
        pushed = total_pushed,
        pulled = total_pulled,
        cursor,
        complete,
        pages,
        request_bytes = total_request_bytes,
        response_bytes = total_response_bytes,
        apply_ms = total_apply_ms,
        "sync client finished"
    );
    Ok(SyncSummary {
        pushed: total_pushed,
        pulled: total_pulled,
        cursor,
        complete,
        pages,
        request_bytes: total_request_bytes,
        response_bytes: total_response_bytes,
        apply_ms: total_apply_ms,
    })
}

async fn sync_cursor(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(get_meta(conn, "sync_cursor")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?)
}

async fn apply_sync_response(
    conn: &mut SqliteConnection,
    response: &SyncResponse,
    attempted_at: &str,
    previous_pushed: i64,
    previous_pulled: usize,
) -> Result<usize> {
    let mut applied = 0;
    let mut tx = begin_immediate(conn).await?;
    update_change_server_seqs_if_missing(&mut tx, &response.push_acks).await?;
    let existing_change_ids = load_existing_change_ids(&mut tx, &response.changes).await?;
    for change in &response.changes {
        if existing_change_ids.contains(change.change_id.as_str()) {
            update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            continue;
        }
        apply_remote_change(&mut tx, change).await?;
        insert_wire_change(&mut tx, change).await?;
        applied += 1;
    }
    // Cursor metadata is committed after page apply work so apply failures roll back cursor advancement.
    let pushed = previous_pushed + response.push_acks.len() as i64;
    let pulled = previous_pulled + applied;
    set_meta(&mut tx, "sync_cursor", &response.cursor.to_string()).await?;
    set_meta(&mut tx, "sync_last_success_at", attempted_at).await?;
    set_meta(&mut tx, "sync_last_error", "").await?;
    set_meta(&mut tx, "sync_last_pushed", &pushed.to_string()).await?;
    set_meta(&mut tx, "sync_last_pulled", &pulled.to_string()).await?;
    set_meta(&mut tx, "sync_last_cursor", &response.cursor.to_string()).await?;
    tx.commit().await?;
    Ok(applied)
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

    let rows = query_builder
        .build_query_scalar::<String>()
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.into_iter().collect())
}

async fn update_change_server_seq(
    conn: &mut SqliteConnection,
    change_id: &str,
    server_seq: Option<i64>,
) -> Result<()> {
    if let Some(server_seq) = server_seq {
        sqlx::query!(
            "UPDATE changes SET server_seq = ? WHERE change_id = ?",
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
    push_acks: &[super::wire::PushAck],
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
