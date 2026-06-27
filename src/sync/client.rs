use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sqlx::SqliteConnection;
use tracing::info;

use super::apply::apply_remote_change;
use super::wire::{
    ChangeRow, ChangeWire, MAX_PULL_BATCH, MAX_PUSH_BATCH, SYNC_PROTOCOL_VERSION, SyncRequest,
    SyncResponse, validate_sync_protocol_version,
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
    info!(server = %server, "sync client starting");

    loop {
        let changes = load_unsynced_changes(conn, MAX_PUSH_BATCH).await?;
        let request_change_ids = changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect::<Vec<_>>();
        let pending = changes.len();
        let mut request = client.post(&url).json(&SyncRequest {
            protocol_version: Some(SYNC_PROTOCOL_VERSION),
            client_id: client_id.clone(),
            after: cursor,
            pull_limit: Some(MAX_PULL_BATCH),
            changes,
        });
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await?
            .error_for_status()?
            .json::<SyncResponse>()
            .await?;
        validate_sync_response(cursor, &request_change_ids, &response)?;
        let applied =
            apply_sync_response(conn, &response, attempted_at, total_pushed, total_pulled).await?;
        total_pushed += pending as i64;
        total_pulled += applied;
        cursor = response.cursor;
        pages += 1;

        let local_more = pending == MAX_PUSH_BATCH;
        let page_complete = !local_more && !response.has_more;
        let budget_exhausted = page_budget.is_some_and(|budget| pages >= budget);
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
        "sync client finished"
    );
    Ok(SyncSummary {
        pushed: total_pushed,
        pulled: total_pulled,
        cursor,
        complete,
        pages,
    })
}

async fn sync_cursor(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(get_meta(conn, "sync_cursor")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?)
}

fn validate_sync_response(
    after: i64,
    request_change_ids: &[String],
    response: &SyncResponse,
) -> Result<()> {
    validate_sync_protocol_version(SYNC_PROTOCOL_VERSION, response.protocol_version)?;
    if response.changes.len() > MAX_PULL_BATCH as usize {
        bail!(
            "error invalid-sync-response pull-too-large limit={} got={}",
            MAX_PULL_BATCH,
            response.changes.len()
        );
    }
    if response.cursor < after {
        bail!(
            "error invalid-sync-response cursor-regressed after={} cursor={}",
            after,
            response.cursor
        );
    }
    validate_push_acks(request_change_ids, response)?;
    validate_pull_page(after, response)?;
    validate_push_pull_overlap(response)?;
    Ok(())
}

fn validate_push_acks(request_change_ids: &[String], response: &SyncResponse) -> Result<()> {
    if response.push_acks.len() != request_change_ids.len() {
        bail!(
            "error invalid-sync-response push-ack-count expected={} got={}",
            request_change_ids.len(),
            response.push_acks.len()
        );
    }
    let expected = request_change_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::with_capacity(response.push_acks.len());
    for ack in &response.push_acks {
        if !expected.contains(ack.change_id.as_str()) {
            bail!(
                "error invalid-sync-response unexpected-push-ack change_id={}",
                ack.change_id
            );
        }
        if ack.server_seq <= 0 {
            bail!(
                "error invalid-sync-response push-ack-server-seq change_id={} server_seq={}",
                ack.change_id,
                ack.server_seq
            );
        }
        if !seen.insert(&ack.change_id) {
            bail!(
                "error invalid-sync-response duplicate-push-ack change_id={}",
                ack.change_id
            );
        }
    }
    Ok(())
}

fn validate_pull_page(after: i64, response: &SyncResponse) -> Result<()> {
    let mut previous = after;
    let mut change_ids = HashSet::with_capacity(response.changes.len());
    for change in &response.changes {
        if !change_ids.insert(&change.change_id) {
            bail!(
                "error invalid-sync-response duplicate-pull-change change_id={}",
                change.change_id
            );
        }
        let server_seq = change.server_seq.with_context(|| {
            format!(
                "error invalid-sync-response missing-server-seq change_id={}",
                change.change_id
            )
        })?;
        if server_seq <= previous {
            bail!(
                "error invalid-sync-response server-seq-order previous={} server_seq={}",
                previous,
                server_seq
            );
        }
        previous = server_seq;
    }
    let expected_cursor = response
        .changes
        .last()
        .and_then(|change| change.server_seq)
        .unwrap_or(after);
    if response.cursor != expected_cursor {
        bail!(
            "error invalid-sync-response cursor-mismatch expected={} got={}",
            expected_cursor,
            response.cursor
        );
    }
    if response.has_more && response.changes.len() < MAX_PULL_BATCH as usize {
        bail!(
            "error invalid-sync-response has-more-short-page returned={} limit={}",
            response.changes.len(),
            MAX_PULL_BATCH
        );
    }
    Ok(())
}

fn validate_push_pull_overlap(response: &SyncResponse) -> Result<()> {
    let acked = response
        .push_acks
        .iter()
        .map(|ack| (ack.change_id.as_str(), ack.server_seq))
        .collect::<std::collections::HashMap<_, _>>();
    for change in &response.changes {
        if let Some(acked_server_seq) = acked.get(change.change_id.as_str()) {
            let Some(pull_server_seq) = change.server_seq else {
                continue;
            };
            if *acked_server_seq != pull_server_seq {
                bail!(
                    "error invalid-sync-response push-pull-server-seq-mismatch change_id={} ack={} pull={}",
                    change.change_id,
                    acked_server_seq,
                    pull_server_seq
                );
            }
        }
    }
    Ok(())
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
    for ack in &response.push_acks {
        update_change_server_seq_if_missing(&mut tx, &ack.change_id, ack.server_seq).await?;
    }
    for change in &response.changes {
        if change_exists(&mut tx, &change.change_id).await? {
            update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            continue;
        }
        apply_remote_change(&mut tx, change).await?;
        insert_wire_change(&mut tx, change).await?;
        applied += 1;
    }
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

async fn change_exists(conn: &mut SqliteConnection, change_id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!: i64" FROM changes WHERE change_id = ?"#,
        change_id
    )
    .fetch_one(&mut *conn)
    .await?
        > 0)
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

async fn update_change_server_seq_if_missing(
    conn: &mut SqliteConnection,
    change_id: &str,
    server_seq: i64,
) -> Result<()> {
    sqlx::query!(
        "UPDATE changes SET server_seq = ? WHERE change_id = ? AND server_seq IS NULL",
        server_seq,
        change_id,
    )
    .execute(&mut *conn)
    .await?;
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
