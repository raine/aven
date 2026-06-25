use std::time::Duration;

use anyhow::{Context, Result, bail};
use sqlx::SqliteConnection;
use tracing::info;

use super::apply::apply_remote_change;
use super::wire::{
    ChangeRow, ChangeWire, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse,
    validate_sync_protocol_version,
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
    let attempted_at = now();
    set_meta(conn, "sync_last_attempt_at", &attempted_at).await?;
    match run_sync_once_inner(conn, server, auth_token, &attempted_at).await {
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
) -> Result<SyncSummary> {
    validate_sync_server(conn, server).await?;
    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let after = get_meta(conn, "sync_cursor")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?;
    let changes = load_unsynced_changes(conn).await?;
    let pending = changes.len();
    let pushed = pending as i64;
    info!(server = %server, pending, "sync client starting");
    let url = format!("{}/sync", server.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut request = client.post(url).json(&SyncRequest {
        protocol_version: Some(SYNC_PROTOCOL_VERSION),
        client_id,
        after,
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
    validate_sync_protocol_version(SYNC_PROTOCOL_VERSION, response.protocol_version)?;
    let mut applied = 0;
    let mut tx = begin_immediate(conn).await?;
    for change in &response.changes {
        if change_exists(&mut tx, &change.change_id).await? {
            update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            continue;
        }
        apply_remote_change(&mut tx, change).await?;
        insert_wire_change(&mut tx, change).await?;
        applied += 1;
    }
    set_meta(&mut tx, "sync_cursor", &response.cursor.to_string()).await?;
    set_meta(&mut tx, "sync_last_success_at", attempted_at).await?;
    set_meta(&mut tx, "sync_last_error", "").await?;
    set_meta(&mut tx, "sync_last_pushed", &pushed.to_string()).await?;
    set_meta(&mut tx, "sync_last_pulled", &applied.to_string()).await?;
    set_meta(&mut tx, "sync_last_cursor", &response.cursor.to_string()).await?;
    tx.commit().await?;
    info!(
        server = %server,
        pushed,
        applied,
        cursor = response.cursor,
        "sync client finished"
    );
    Ok(SyncSummary {
        pushed,
        pulled: applied,
        cursor: response.cursor,
    })
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

async fn load_unsynced_changes(conn: &mut SqliteConnection) -> Result<Vec<ChangeWire>> {
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq IS NULL ORDER BY COALESCE(server_seq, local_seq), created_at"#,
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
