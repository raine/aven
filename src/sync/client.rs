use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use flate2::write::GzEncoder;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};
use tracing::info;

use super::apply::apply_remote_change;
use super::wire::{
    ChangeRow, ChangeWire, MAX_BLOB_TRANSFER_BATCH, MAX_PULL_BATCH, MAX_PUSH_BATCH,
    MissingBlobsRequest, MissingBlobsResponse, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse,
    validate_blob_hashes, validate_sync_response_for_request,
};
use crate::change_log::op_type;
use crate::cli::SyncArgs;
use crate::config;
use crate::db::{begin_immediate, get_meta, set_meta};
use crate::ids::now;

const GZIP_THRESHOLD: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct SyncHttpClient {
    pub(crate) inner: reqwest::Client,
    // The id identifies this process-local HTTP client instance in sync logs.
    id: String,
}

impl SyncHttpClient {
    pub(crate) fn new() -> Result<Self> {
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("build sync HTTP client")?;
        let id = format!("sc-{}", crate::ids::new_id());
        Ok(Self { inner, id })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SyncSummary {
    pub(crate) pushed: i64,
    pub(crate) pulled: usize,
    pub(crate) blob_uploaded: usize,
    pub(crate) blob_downloaded: usize,
    pub(crate) cursor: i64,
    pub(crate) complete: bool,
    pub(crate) pages: usize,
    pub(crate) request_bytes: usize,
    pub(crate) request_wire_bytes: usize,
    pub(crate) response_decoded_bytes: usize,
    pub(crate) response_compression: String,
    pub(crate) apply_ms: u128,
}

fn gzip_encode(body: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(body)
        .context("gzip encode sync request body")?;
    encoder.finish().context("finish sync request compression")
}

pub(crate) async fn sync_client(
    conn: &mut SqliteConnection,
    db_path: &Path,
    args: SyncArgs,
    config: &config::AppConfig,
) -> Result<()> {
    let server = config::resolve_sync_server(args.server.as_deref(), config)?;
    let blob_dir = config::resolve_blob_dir(db_path, config)?;
    let summary = run_sync_once(conn, &blob_dir, &server, config.sync_auth_token()).await?;
    println!(
        "synced pushed={} pulled={} blob_uploaded={} blob_downloaded={} cursor={}",
        summary.pushed,
        summary.pulled,
        summary.blob_uploaded,
        summary.blob_downloaded,
        summary.cursor
    );
    Ok(())
}

pub(crate) async fn run_sync_once(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
) -> Result<SyncSummary> {
    run_sync_with_page_budget(conn, blob_dir, server, auth_token, None).await
}

pub(crate) async fn run_sync_with_page_budget_using_client(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
    client: &SyncHttpClient,
) -> Result<SyncSummary> {
    let attempted_at = now();
    set_meta(conn, "sync_last_attempt_at", &attempted_at).await?;
    match run_sync_once_inner(
        conn,
        blob_dir,
        server,
        auth_token,
        &attempted_at,
        page_budget,
        client,
    )
    .await
    {
        Ok(summary) => Ok(summary),
        Err(error) => {
            let error_text = format!("{error:#}");
            set_meta(conn, "sync_last_error", &error_text).await?;
            Err(error)
        }
    }
}

pub(crate) async fn run_sync_with_page_budget(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
) -> Result<SyncSummary> {
    let client = SyncHttpClient::new()?;
    run_sync_with_page_budget_using_client(conn, blob_dir, server, auth_token, page_budget, &client)
        .await
}

async fn run_sync_once_inner(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
    attempted_at: &str,
    page_budget: Option<usize>,
    client: &SyncHttpClient,
) -> Result<SyncSummary> {
    let mut last_response_compression: Option<String>;
    validate_sync_server(conn, server).await?;
    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let url = format!("{}/sync", server.trim_end_matches('/'));
    let mut total_pushed = 0_i64;
    let mut total_pulled = 0_usize;
    let mut total_blob_uploaded = 0_usize;
    let mut total_blob_downloaded = 0_usize;
    let mut cursor = sync_cursor(conn).await?;
    let mut pages = 0_usize;
    let complete;
    let mut total_request_bytes = 0_usize;
    let mut total_request_wire_bytes = 0_usize;
    let mut total_response_decoded_bytes = 0_usize;
    let mut total_apply_ms = 0_u128;
    info!(server = %server, http_client_id = %client.id(), "sync client starting");

    loop {
        let changes = load_unsynced_changes(conn, MAX_PUSH_BATCH).await?;
        let uploaded =
            upload_missing_blobs_for_changes(conn, blob_dir, client, server, auth_token, &changes)
                .await?;
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
        let (wire_body, request_wire_bytes) = if request_bytes > GZIP_THRESHOLD {
            let compressed = gzip_encode(&request_body)?;
            let wire = compressed.len();
            (compressed, wire)
        } else {
            (request_body, request_bytes)
        };
        let mut request = client
            .inner
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(wire_body);
        if request_wire_bytes != request_bytes {
            request = request.header(reqwest::header::CONTENT_ENCODING, "gzip");
        }
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let http_started = Instant::now();
        let response = request.send().await?.error_for_status()?;
        let http_ms = http_started.elapsed().as_millis();
        last_response_compression = Some(
            response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("none")
                .to_string(),
        );
        let response_body = response.bytes().await?;
        let response_bytes = response_body.len();
        let response: SyncResponse = serde_json::from_slice(&response_body)?;
        validate_sync_response_for_request(cursor, pull_limit, &request_change_ids, &response)?;
        let apply_started = Instant::now();
        let applied =
            apply_sync_response(conn, &response, attempted_at, total_pushed, total_pulled).await?;
        let downloaded =
            download_missing_local_blobs(conn, blob_dir, client, server, auth_token).await?;
        let apply_ms = apply_started.elapsed().as_millis();
        total_pushed += pending as i64;
        total_pulled += applied;
        total_blob_uploaded += uploaded;
        total_blob_downloaded += downloaded;
        cursor = response.cursor;
        pages += 1;
        total_request_bytes += request_bytes;
        total_request_wire_bytes += request_wire_bytes;
        total_response_decoded_bytes += response_bytes;
        total_apply_ms += apply_ms;

        let local_more = pending == MAX_PUSH_BATCH;
        let blob_more = downloaded == MAX_BLOB_TRANSFER_BATCH;
        let page_complete = !local_more && !response.has_more && !blob_more;
        let budget_exhausted = page_budget.is_some_and(|budget| pages >= budget);
        info!(
            server = %server,
            page = pages,
            pushed = pending,
            pulled = applied,
            blob_uploaded = uploaded,
            blob_downloaded = downloaded,
            cursor,
            complete = page_complete,
            request_bytes,
            request_wire_bytes,
            response_decoded_bytes = response_bytes,
            response_compression = last_response_compression.as_deref().unwrap_or("none"),
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
        blob_uploaded = total_blob_uploaded,
        blob_downloaded = total_blob_downloaded,
        cursor,
        complete,
        pages,
        request_bytes = total_request_bytes,
        request_wire_bytes = total_request_wire_bytes,
        response_decoded_bytes = total_response_decoded_bytes,
        response_compression = last_response_compression.as_deref().unwrap_or("none"),
        apply_ms = total_apply_ms,
        "sync client finished"
    );
    Ok(SyncSummary {
        pushed: total_pushed,
        pulled: total_pulled,
        blob_uploaded: total_blob_uploaded,
        blob_downloaded: total_blob_downloaded,
        cursor,
        complete,
        pages,
        request_bytes: total_request_bytes,
        request_wire_bytes: total_request_wire_bytes,
        response_decoded_bytes: total_response_decoded_bytes,
        response_compression: last_response_compression.unwrap_or_else(|| "none".to_string()),
        apply_ms: total_apply_ms,
    })
}

async fn upload_missing_blobs_for_changes(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    changes: &[ChangeWire],
) -> Result<usize> {
    let hashes = attachment_add_hashes(changes)?;
    let mut uploaded = 0_usize;
    for chunk in hashes.chunks(MAX_BLOB_TRANSFER_BATCH) {
        let missing = request_missing_blobs(client, server, auth_token, chunk.to_vec()).await?;
        for sha256 in missing {
            upload_blob(conn, blob_dir, client, server, auth_token, &sha256).await?;
            uploaded += 1;
        }
    }
    Ok(uploaded)
}

fn attachment_add_hashes(changes: &[ChangeWire]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut hashes = Vec::new();
    for change in changes {
        if change.op_type != op_type::ATTACHMENT_ADD {
            continue;
        }
        let sha256 = change
            .payload
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .context("payload missing sha256")?
            .to_string();
        if seen.insert(sha256.clone()) {
            hashes.push(sha256);
        }
    }
    Ok(hashes)
}

async fn request_missing_blobs(
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    hashes: Vec<String>,
) -> Result<Vec<String>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    validate_blob_hashes(&hashes)?;
    let requested = hashes.iter().cloned().collect::<HashSet<_>>();
    let mut request = client
        .inner
        .post(format!(
            "{}/sync/blobs/missing",
            server.trim_end_matches('/')
        ))
        .json(&MissingBlobsRequest { hashes });
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let response: MissingBlobsResponse = request.send().await?.error_for_status()?.json().await?;
    validate_blob_hashes(&response.missing)?;
    for hash in &response.missing {
        if !requested.contains(hash) {
            bail!("error invalid-blob-missing-response");
        }
    }
    Ok(response.missing)
}

async fn upload_blob(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    sha256: &str,
) -> Result<()> {
    let row = crate::attachments::blob_inventory_row(conn, sha256)
        .await?
        .filter(|row| row.available)
        .context("error attachment-blob-local-missing")?;
    let path = crate::attachments::object_path(blob_dir, sha256)?;
    let bytes = tokio::fs::read(path)
        .await
        .context("error attachment-blob-local-missing")?;
    if crate::attachments::sha256_hex(&bytes) != sha256 {
        bail!("error attachment-blob-local-invalid");
    }
    if row.byte_size != i64::try_from(bytes.len()).context("attachment bytes exceed i64")? {
        bail!("error attachment-blob-local-invalid");
    }
    let validated =
        crate::attachments::decode::validate_image(bytes.clone(), Some(row.media_type.clone()))
            .await?;
    let dimensions: Option<(Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT width, height FROM task_attachments WHERE sha256 = ? LIMIT 1")
            .bind(sha256)
            .fetch_optional(&mut *conn)
            .await?;
    if dimensions.is_some_and(|dimensions| {
        dimensions != (Some(validated.facts.width), Some(validated.facts.height))
    }) {
        bail!("error attachment-blob-local-invalid");
    }
    let mut request = client
        .inner
        .put(format!(
            "{}/sync/blobs/{}",
            server.trim_end_matches('/'),
            sha256
        ))
        .header(reqwest::header::CONTENT_TYPE, row.media_type.as_str())
        .body(bytes);
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    request.send().await?.error_for_status()?;
    Ok(())
}

#[derive(Debug)]
struct MissingLocalBlob {
    sha256: String,
    byte_size: i64,
    media_type: String,
    width: Option<i64>,
    height: Option<i64>,
}

async fn download_missing_local_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
) -> Result<usize> {
    let missing = missing_local_blobs(conn, blob_dir).await?;
    let mut downloaded = 0_usize;
    for blob in missing {
        download_blob(conn, blob_dir, client, server, auth_token, blob).await?;
        downloaded += 1;
    }
    Ok(downloaded)
}

async fn missing_local_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<Vec<MissingLocalBlob>> {
    let mut missing = Vec::new();
    let rows = sqlx::query(
        "SELECT DISTINCT ta.sha256, ta.byte_size, ta.media_type, ta.width, ta.height
         FROM task_attachments ta
         LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
         WHERE ta.deleted = 0 AND COALESCE(bi.available, 0) = 0
         LIMIT ?",
    )
    .bind(MAX_BLOB_TRANSFER_BATCH as i64)
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let sha256: String = row.get("sha256");
        let path = crate::attachments::object_path(blob_dir, &sha256)?;
        if !path.exists() {
            missing.push(MissingLocalBlob {
                sha256,
                byte_size: row.get("byte_size"),
                media_type: row.get("media_type"),
                width: row.get("width"),
                height: row.get("height"),
            });
        }
    }
    if missing.len() < MAX_BLOB_TRANSFER_BATCH {
        let remaining = MAX_BLOB_TRANSFER_BATCH - missing.len();
        let rows = sqlx::query(
            "SELECT DISTINCT ta.sha256, ta.byte_size, ta.media_type, ta.width, ta.height
             FROM task_attachments ta
             JOIN blob_inventory bi ON bi.sha256 = ta.sha256
             WHERE ta.deleted = 0 AND bi.available = 1
             LIMIT ?",
        )
        .bind(remaining as i64)
        .fetch_all(&mut *conn)
        .await?;
        for row in rows {
            let sha256: String = row.get("sha256");
            if missing.iter().any(|blob| blob.sha256 == sha256) {
                continue;
            }
            let path = crate::attachments::object_path(blob_dir, &sha256)?;
            if !path.exists() {
                missing.push(MissingLocalBlob {
                    sha256,
                    byte_size: row.get("byte_size"),
                    media_type: row.get("media_type"),
                    width: row.get("width"),
                    height: row.get("height"),
                });
            }
        }
    }
    Ok(missing)
}

async fn download_blob(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    blob: MissingLocalBlob,
) -> Result<()> {
    let mut request = client.inner.get(format!(
        "{}/sync/blobs/{}",
        server.trim_end_matches('/'),
        blob.sha256
    ));
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let bytes = request.send().await?.error_for_status()?.bytes().await?;
    if crate::attachments::sha256_hex(&bytes) != blob.sha256 {
        bail!("error attachment-blob-remote-invalid");
    }
    if blob.byte_size != i64::try_from(bytes.len()).context("attachment bytes exceed i64")? {
        bail!("error attachment-blob-remote-invalid");
    }
    let validated =
        crate::attachments::decode::validate_image(bytes.to_vec(), Some(blob.media_type.clone()))
            .await
            .context("error attachment-blob-remote-invalid")?;
    if (blob.width, blob.height) != (Some(validated.facts.width), Some(validated.facts.height)) {
        bail!("error attachment-blob-remote-invalid");
    }
    crate::attachments::storage::store_validated_blob(conn, blob_dir, validated).await?;
    Ok(())
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
