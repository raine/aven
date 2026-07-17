use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use flate2::write::GzEncoder;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};
use tracing::info;

use super::apply::apply_remote_change;
use super::planner::{
    PendingChange, TransferBudget, TransferObject, plan_change_prefix, plan_transfers,
};
use super::wire::{
    BlobUploadContract, ChangeRow, ChangeWire, MAX_BLOB_TRANSFER_BYTES, MAX_BLOB_TRANSFER_OBJECTS,
    MAX_PULL_BATCH, MAX_PUSH_BATCH, MissingBlobsRequest, MissingBlobsResponse,
    SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse, validate_blob_contracts,
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
    pub(crate) blob_uploaded_bytes: u64,
    pub(crate) blob_downloaded: usize,
    pub(crate) blob_downloaded_bytes: u64,
    pub(crate) blob_upload_remaining: usize,
    pub(crate) blob_upload_remaining_bytes: u64,
    pub(crate) blob_download_remaining: usize,
    pub(crate) blob_download_remaining_bytes: u64,
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
    let client = SyncHttpClient::new()?;
    loop {
        let summary = run_sync_with_page_budget_using_client_and_policy(
            conn,
            &blob_dir,
            &server,
            config.sync_auth_token(),
            None,
            &client,
            config.local.attachment_lifecycle.policy(),
        )
        .await?;
        println!(
            "synced pushed={} pulled={} blob_uploaded={} blob_uploaded_bytes={} blob_downloaded={} blob_downloaded_bytes={} blob_upload_remaining={} blob_upload_remaining_bytes={} blob_download_remaining={} blob_download_remaining_bytes={} cursor={} complete={}",
            summary.pushed,
            summary.pulled,
            summary.blob_uploaded,
            summary.blob_uploaded_bytes,
            summary.blob_downloaded,
            summary.blob_downloaded_bytes,
            summary.blob_upload_remaining,
            summary.blob_upload_remaining_bytes,
            summary.blob_download_remaining,
            summary.blob_download_remaining_bytes,
            summary.cursor,
            summary.complete,
        );
        if summary.complete
            || (summary.pushed == 0
                && summary.pulled == 0
                && summary.blob_uploaded == 0
                && summary.blob_downloaded == 0)
        {
            break;
        }
    }
    Ok(())
}

pub(crate) async fn run_sync_with_page_budget_using_client_and_policy(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
    client: &SyncHttpClient,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
) -> Result<SyncSummary> {
    let attempted_at = now();
    set_meta(conn, "sync_last_attempt_at", &attempted_at).await?;
    match run_sync_once_inner(
        conn,
        SyncRunContext {
            blob_dir,
            server,
            auth_token,
            attempted_at: &attempted_at,
            page_budget,
            client,
            lifecycle_policy,
        },
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

struct SyncRunContext<'a> {
    blob_dir: &'a Path,
    server: &'a str,
    auth_token: Option<&'a str>,
    attempted_at: &'a str,
    page_budget: Option<usize>,
    client: &'a SyncHttpClient,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
}

async fn run_sync_once_inner(
    conn: &mut SqliteConnection,
    context: SyncRunContext<'_>,
) -> Result<SyncSummary> {
    let SyncRunContext {
        blob_dir,
        server,
        auth_token,
        attempted_at,
        page_budget,
        client,
        lifecycle_policy,
    } = context;
    let mut last_response_compression: Option<String>;
    validate_sync_server(conn, server).await?;
    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let url = format!("{}/sync", server.trim_end_matches('/'));
    let mut total_pushed = 0_i64;
    let mut total_pulled = 0_usize;
    let mut total_blob_uploaded = 0_usize;
    let mut total_blob_uploaded_bytes = 0_u64;
    let mut total_blob_downloaded = 0_usize;
    let mut total_blob_downloaded_bytes = 0_u64;
    let mut transfer_budget = TransferBudget {
        objects: MAX_BLOB_TRANSFER_OBJECTS,
        bytes: MAX_BLOB_TRANSFER_BYTES,
        completed_objects: 0,
    };
    let mut cursor = sync_cursor(conn).await?;
    let mut pages = 0_usize;
    let mut last_has_more;
    let complete;
    let mut total_request_bytes = 0_usize;
    let mut total_request_wire_bytes = 0_usize;
    let mut total_response_decoded_bytes = 0_usize;
    let mut total_apply_ms = 0_u128;
    info!(server = %server, http_client_id = %client.id(), "sync client starting");

    loop {
        let pending_changes = load_unsynced_changes(conn, MAX_PUSH_BATCH).await?;
        let upload_plan = plan_upload_change_prefix(
            client,
            server,
            auth_token,
            &pending_changes,
            transfer_budget,
        )
        .await?;
        let changes = pending_changes
            .into_iter()
            .take(upload_plan.change_count)
            .collect::<Vec<_>>();
        let mut uploaded = 0_usize;
        let mut uploaded_bytes = 0_u64;
        for transfer in &upload_plan.transfers {
            let contract = attachment_blob_contract_for_hash(&changes, &transfer.sha256)?;
            upload_blob(conn, blob_dir, client, server, auth_token, &contract).await?;
            transfer_budget.consume(transfer);
            uploaded += 1;
            uploaded_bytes += transfer.byte_size;
        }
        confirm_blob_admissions(client, server, auth_token, &changes).await?;
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
        let download_outcome = download_missing_local_blobs(
            conn,
            blob_dir,
            client,
            server,
            auth_token,
            lifecycle_policy,
            transfer_budget,
        )
        .await?;
        for transfer in &download_outcome.transfers {
            transfer_budget.consume(transfer);
        }
        let downloaded = download_outcome.transfers.len();
        let downloaded_bytes = download_outcome
            .transfers
            .iter()
            .map(|transfer| transfer.byte_size)
            .sum::<u64>();
        let apply_ms = apply_started.elapsed().as_millis();
        total_pushed += pending as i64;
        total_pulled += applied;
        total_blob_uploaded += uploaded;
        total_blob_uploaded_bytes += uploaded_bytes;
        total_blob_downloaded += downloaded;
        total_blob_downloaded_bytes += downloaded_bytes;
        cursor = response.cursor;
        last_has_more = response.has_more;
        pages += 1;
        total_request_bytes += request_bytes;
        total_request_wire_bytes += request_wire_bytes;
        total_response_decoded_bytes += response_bytes;
        total_apply_ms += apply_ms;

        let local_more = unsynced_change_count(conn).await? > 0;
        let download_more = download_outcome.remaining.count > 0;
        let page_complete = !local_more && !response.has_more && !download_more;
        let budget_exhausted = page_budget.is_some_and(|budget| pages >= budget);
        let transfer_stalled = (local_more && pending == 0)
            || (!local_more && download_more && download_outcome.transfers.is_empty());
        info!(
            server = %server,
            page = pages,
            pushed = pending,
            pulled = applied,
            blob_uploaded = uploaded,
            blob_uploaded_bytes = uploaded_bytes,
            blob_downloaded = downloaded,
            blob_downloaded_bytes = downloaded_bytes,
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
        if page_complete || budget_exhausted || (transfer_stalled && !response.has_more) {
            complete = page_complete;
            break;
        }
    }

    let upload_remaining =
        missing_server_blobs_for_unsynced_changes(conn, client, server, auth_token).await?;
    let download_remaining = missing_local_blobs(conn, blob_dir).await?;
    let upload_remaining_bytes = upload_remaining
        .iter()
        .map(|blob| u64::try_from(blob.byte_size).unwrap_or(0))
        .sum::<u64>();
    let download_remaining_bytes = download_remaining
        .iter()
        .map(|blob| u64::try_from(blob.byte_size).unwrap_or(0))
        .sum::<u64>();
    let metadata_pending = unsynced_change_count(conn).await? > 0 || last_has_more;
    let complete = complete
        && !metadata_pending
        && upload_remaining.is_empty()
        && download_remaining.is_empty();

    info!(
        server = %server,
        pushed = total_pushed,
        pulled = total_pulled,
        blob_uploaded = total_blob_uploaded,
        blob_uploaded_bytes = total_blob_uploaded_bytes,
        blob_downloaded = total_blob_downloaded,
        blob_downloaded_bytes = total_blob_downloaded_bytes,
        blob_upload_remaining = upload_remaining.len(),
        blob_upload_remaining_bytes = upload_remaining_bytes,
        blob_download_remaining = download_remaining.len(),
        blob_download_remaining_bytes = download_remaining_bytes,
        metadata_pending,
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
        blob_uploaded_bytes: total_blob_uploaded_bytes,
        blob_downloaded: total_blob_downloaded,
        blob_downloaded_bytes: total_blob_downloaded_bytes,
        blob_upload_remaining: upload_remaining.len(),
        blob_upload_remaining_bytes: upload_remaining_bytes,
        blob_download_remaining: download_remaining.len(),
        blob_download_remaining_bytes: download_remaining_bytes,
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

async fn plan_upload_change_prefix(
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    changes: &[ChangeWire],
    budget: TransferBudget,
) -> Result<super::planner::ChangePrefixPlan> {
    let mut missing = HashSet::new();
    let mut contracts = HashSet::new();
    let mut pending = Vec::with_capacity(changes.len());
    for change in changes {
        let contract = attachment_blob_contract(change)?;
        if let Some(contract) = &contract
            && contracts.insert((contract.workspace_id.clone(), contract.sha256.clone()))
        {
            let response =
                request_missing_blobs(client, server, auth_token, vec![contract.clone()]).await?;
            if response.iter().any(|hash| hash == &contract.sha256) {
                missing.insert(contract.sha256.clone());
            }
        }
        let missing_blob = match contract {
            Some(contract) if missing.contains(&contract.sha256) => Some(TransferObject {
                sha256: contract.sha256,
                byte_size: u64::try_from(contract.byte_size)
                    .context("attachment bytes exceed u64")?,
            }),
            _ => None,
        };
        pending.push(PendingChange { missing_blob });
        let plan = plan_change_prefix(&pending, budget);
        if plan.change_count < pending.len() {
            return Ok(plan);
        }
    }
    Ok(plan_change_prefix(&pending, budget))
}

async fn confirm_blob_admissions(
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    changes: &[ChangeWire],
) -> Result<()> {
    let mut seen = HashSet::new();
    for change in changes {
        let Some(contract) = attachment_blob_contract(change)? else {
            continue;
        };
        if !seen.insert((contract.workspace_id.clone(), contract.sha256.clone())) {
            continue;
        }
        let missing = request_missing_blobs(client, server, auth_token, vec![contract]).await?;
        if !missing.is_empty() {
            bail!("error attachment-blob-admission-missing");
        }
    }
    Ok(())
}

fn attachment_blob_contract(change: &ChangeWire) -> Result<Option<BlobUploadContract>> {
    if change.op_type != op_type::ATTACHMENT_ADD {
        return Ok(None);
    }
    Ok(Some(BlobUploadContract {
        workspace_id: payload_string(change, "workspace_id")?,
        sha256: payload_string(change, "sha256")?,
        byte_size: payload_i64(change, "byte_size")?,
        media_type: payload_string(change, "media_type")?,
        width: payload_i64(change, "width")?,
        height: payload_i64(change, "height")?,
    }))
}

fn attachment_blob_contract_for_hash(
    changes: &[ChangeWire],
    sha256: &str,
) -> Result<BlobUploadContract> {
    for change in changes {
        if let Some(contract) = attachment_blob_contract(change)?
            && contract.sha256 == sha256
        {
            return Ok(contract);
        }
    }
    bail!("payload missing attachment blob contract")
}

fn payload_string(change: &ChangeWire, key: &str) -> Result<String> {
    change
        .payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("payload missing {key}"))
}

fn payload_i64(change: &ChangeWire, key: &str) -> Result<i64> {
    change
        .payload
        .get(key)
        .and_then(serde_json::Value::as_i64)
        .with_context(|| format!("payload missing {key}"))
}

async fn request_missing_blobs(
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    blobs: Vec<BlobUploadContract>,
) -> Result<Vec<String>> {
    if blobs.is_empty() {
        return Ok(Vec::new());
    }
    validate_blob_contracts(&blobs)?;
    let requested = blobs
        .iter()
        .map(|blob| blob.sha256.clone())
        .collect::<HashSet<_>>();
    let mut request = client
        .inner
        .post(format!(
            "{}/sync/blobs/missing",
            server.trim_end_matches('/')
        ))
        .json(&MissingBlobsRequest { blobs });
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    if response.status() == reqwest::StatusCode::INSUFFICIENT_STORAGE {
        bail!("error attachment-quota-exceeded");
    }
    if !response.status().is_success() {
        bail!(
            "error blob-admission-failed status={}",
            response.status().as_u16()
        );
    }
    let response: MissingBlobsResponse = response.json().await?;
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
    contract: &BlobUploadContract,
) -> Result<()> {
    let sha256 = &contract.sha256;
    let row = crate::attachments::blob_inventory_row(conn, sha256)
        .await?
        .filter(|row| row.available)
        .context("error attachment-blob-local-missing")?;
    let path = crate::attachments::object_path(blob_dir, sha256)?;
    let bytes = tokio::fs::read(path)
        .await
        .context("error attachment-blob-local-missing")?;
    if crate::attachments::sha256_hex(&bytes) != sha256.as_str() {
        bail!("error attachment-blob-local-invalid");
    }
    if row.byte_size != i64::try_from(bytes.len()).context("attachment bytes exceed i64")? {
        bail!("error attachment-blob-local-invalid");
    }
    let validated =
        crate::attachments::decode::validate_image(bytes.clone(), Some(row.media_type.clone()))
            .await?;
    if (validated.facts.width, validated.facts.height) != (contract.width, contract.height)
        || validated.facts.media_type != contract.media_type
        || contract.byte_size != row.byte_size
    {
        bail!("error attachment-blob-local-invalid");
    }
    let lease_id = crate::attachments::lifecycle::acquire_lease(
        conn,
        sha256,
        "transfer",
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let mut request = client
        .inner
        .put(format!(
            "{}/sync/blobs/{}",
            server.trim_end_matches('/'),
            sha256
        ))
        .header(reqwest::header::CONTENT_TYPE, contract.media_type.as_str())
        .header("x-aven-workspace-id", contract.workspace_id.as_str())
        .header("x-aven-byte-size", contract.byte_size)
        .header("x-aven-width", contract.width)
        .header("x-aven-height", contract.height)
        .body(bytes);
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let send_result = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("error blob-upload-request-failed"));
    crate::attachments::lifecycle::release_lease(conn, &lease_id).await?;
    let response = send_result?;
    if response.status() == reqwest::StatusCode::INSUFFICIENT_STORAGE {
        bail!("error attachment-quota-exceeded");
    }
    if !response.status().is_success() {
        bail!(
            "error blob-upload-failed status={}",
            response.status().as_u16()
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct MissingLocalBlob {
    sha256: String,
    byte_size: i64,
    media_type: String,
    width: Option<i64>,
    height: Option<i64>,
}

#[derive(Debug, Default)]
struct DownloadOutcome {
    transfers: Vec<TransferObject>,
    remaining: crate::attachments::lifecycle::ByteCount,
}

async fn download_missing_local_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
    budget: TransferBudget,
) -> Result<DownloadOutcome> {
    let missing = missing_local_blobs(conn, blob_dir).await?;
    let objects = missing
        .iter()
        .map(|blob| {
            Ok(TransferObject {
                sha256: blob.sha256.clone(),
                byte_size: u64::try_from(blob.byte_size).context("attachment bytes exceed u64")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let plan = plan_transfers(&objects, budget);
    let planned = plan
        .iter()
        .map(|object| object.sha256.as_str())
        .collect::<HashSet<_>>();
    let mut outcome = DownloadOutcome::default();
    for blob in missing
        .iter()
        .filter(|blob| planned.contains(blob.sha256.as_str()))
    {
        let capacity_reservation = crate::attachments::lifecycle::ensure_local_capacity(
            conn,
            blob_dir,
            &blob.sha256,
            blob.byte_size,
            lifecycle_policy,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        let download_result =
            download_blob(conn, blob_dir, client, server, auth_token, blob.clone()).await;
        if let Some(reservation_id) = capacity_reservation {
            crate::attachments::lifecycle::release_reservation(conn, &reservation_id).await?;
        }
        download_result?;
        let transfer = plan
            .iter()
            .find(|object| object.sha256 == blob.sha256)
            .context("planned attachment blob missing")?
            .clone();
        outcome.transfers.push(transfer);
    }
    let remaining = missing_local_blobs(conn, blob_dir).await?;
    outcome.remaining = crate::attachments::lifecycle::ByteCount {
        count: remaining.len() as u64,
        bytes: remaining
            .iter()
            .map(|blob| u64::try_from(blob.byte_size).unwrap_or(0))
            .sum(),
    };
    Ok(outcome)
}

async fn missing_local_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<Vec<MissingLocalBlob>> {
    let mut missing = Vec::new();
    let rows = sqlx::query(
        "SELECT ta.sha256, MAX(ta.byte_size) AS byte_size,
                MAX(ta.media_type) AS media_type, MAX(ta.width) AS width,
                MAX(ta.height) AS height, MAX(COALESCE(bi.available, 0)) AS available
         FROM task_attachments ta
         JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
         LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
         WHERE ta.deleted = 0 AND t.deleted = 0
         GROUP BY ta.sha256
         ORDER BY ta.sha256",
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let sha256: String = row.get("sha256");
        let available: i64 = row.get("available");
        let path = crate::attachments::object_path(blob_dir, &sha256)?;
        if available == 0 || !path.exists() {
            missing.push(MissingLocalBlob {
                sha256,
                byte_size: row.get("byte_size"),
                media_type: row.get("media_type"),
                width: row.get("width"),
                height: row.get("height"),
            });
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
    let mut request = client
        .inner
        .get(format!(
            "{}/sync/blobs/{}",
            server.trim_end_matches('/'),
            blob.sha256
        ))
        .header(reqwest::header::ACCEPT_ENCODING, "identity");
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("error blob-download-request-failed"))?;
    if !response.status().is_success() {
        bail!(
            "error blob-download-failed status={}",
            response.status().as_u16()
        );
    }
    let expected = u64::try_from(blob.byte_size).context("attachment bytes exceed u64")?;
    if response.content_length() != Some(expected) {
        bail!("error attachment-blob-remote-invalid");
    }
    let expected_usize = usize::try_from(expected).context("attachment bytes exceed usize")?;
    let mut bytes = Vec::with_capacity(expected_usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("error blob-download-body-invalid"))?
    {
        if bytes.len().saturating_add(chunk.len()) > expected_usize {
            bail!("error attachment-blob-remote-invalid");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != expected_usize {
        bail!("error attachment-blob-remote-invalid");
    }
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

async fn missing_server_blobs_for_unsynced_changes(
    conn: &mut SqliteConnection,
    client: &SyncHttpClient,
    server: &str,
    auth_token: Option<&str>,
) -> Result<Vec<BlobUploadContract>> {
    let changes = load_unsynced_changes(conn, i64::MAX as usize).await?;
    let mut contracts = Vec::new();
    let mut seen = HashSet::new();
    for change in &changes {
        if let Some(contract) = attachment_blob_contract(change)?
            && seen.insert((contract.workspace_id.clone(), contract.sha256.clone()))
        {
            contracts.push(contract);
        }
    }
    let mut missing = HashSet::new();
    for chunk in contracts.chunks(MAX_BLOB_TRANSFER_OBJECTS) {
        missing.extend(request_missing_blobs(client, server, auth_token, chunk.to_vec()).await?);
    }
    let mut unique_missing = HashSet::new();
    Ok(contracts
        .into_iter()
        .filter(|contract| {
            missing.contains(&contract.sha256) && unique_missing.insert(contract.sha256.clone())
        })
        .collect())
}

async fn unsynced_change_count(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE server_seq IS NULL")
            .fetch_one(&mut *conn)
            .await?,
    )
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
