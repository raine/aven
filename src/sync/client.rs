use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use aven_core::db::Database;
use aven_core::sync::ApplySyncPage;
use flate2::write::GzEncoder;
use tracing::info;

use super::wire::{
    MAX_PULL_BATCH, MAX_PUSH_BATCH, SyncResponse, validate_sync_response_for_request,
};
use crate::cli::SyncArgs;
use crate::config;
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
    database: &Database,
    args: SyncArgs,
    config: &config::AppConfig,
) -> Result<()> {
    let server = config::resolve_sync_server(args.server.as_deref(), config)?;
    let summary = run_sync_once(database, &server, config.sync_auth_token()).await?;
    println!(
        "synced pushed={} pulled={} cursor={}",
        summary.pushed, summary.pulled, summary.cursor
    );
    Ok(())
}

pub(crate) async fn run_sync_once(
    database: &Database,
    server: &str,
    auth_token: Option<&str>,
) -> Result<SyncSummary> {
    run_sync_with_page_budget(database, server, auth_token, None).await
}

pub(crate) async fn run_sync_with_page_budget_using_client(
    database: &Database,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
    client: &SyncHttpClient,
) -> Result<SyncSummary> {
    let attempted_at = now();
    database.begin_sync_attempt(attempted_at.clone()).await?;
    match run_sync_once_inner(
        database,
        server,
        auth_token,
        attempted_at,
        page_budget,
        client,
    )
    .await
    {
        Ok(summary) => Ok(summary),
        Err(error) => {
            let error_text = format!("{error:#}");
            database.record_sync_error(error_text).await?;
            Err(error)
        }
    }
}

pub(crate) async fn run_sync_with_page_budget(
    database: &Database,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
) -> Result<SyncSummary> {
    let client = SyncHttpClient::new()?;
    run_sync_with_page_budget_using_client(database, server, auth_token, page_budget, &client).await
}

async fn run_sync_once_inner(
    database: &Database,
    server: &str,
    auth_token: Option<&str>,
    attempted_at: String,
    page_budget: Option<usize>,
    client: &SyncHttpClient,
) -> Result<SyncSummary> {
    let mut last_response_compression: Option<String>;
    let url = format!("{}/sync", server.trim_end_matches('/'));
    let mut total_pushed = 0_i64;
    let mut total_pulled = 0_usize;
    let mut cursor: i64;
    let mut pages = 0_usize;
    let complete;
    let mut total_request_bytes = 0_usize;
    let mut total_request_wire_bytes = 0_usize;
    let mut total_response_decoded_bytes = 0_usize;
    let mut total_apply_ms = 0_u128;
    info!(server = %server, http_client_id = %client.id(), "sync client starting");

    loop {
        let page = database
            .prepare_client_sync_page(server.to_string(), MAX_PUSH_BATCH, MAX_PULL_BATCH)
            .await?;
        let request_change_ids = page
            .request
            .changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect::<Vec<_>>();
        let pending = page.pending;
        let request_cursor = page.request.after;
        let request_body = serde_json::to_vec(&page.request)?;
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
        validate_sync_response_for_request(
            request_cursor,
            MAX_PULL_BATCH,
            &request_change_ids,
            &response,
        )?;
        let has_more = response.has_more;
        cursor = response.cursor;
        let apply_started = Instant::now();
        let applied = database
            .apply_client_sync_page(ApplySyncPage {
                request: page.request,
                response,
                attempted_at: attempted_at.clone(),
                previous_pushed: total_pushed,
                previous_pulled: total_pulled,
            })
            .await?;
        let apply_ms = apply_started.elapsed().as_millis();
        total_pushed += pending as i64;
        total_pulled += applied;
        pages += 1;
        total_request_bytes += request_bytes;
        total_request_wire_bytes += request_wire_bytes;
        total_response_decoded_bytes += response_bytes;
        total_apply_ms += apply_ms;

        let local_more = pending == MAX_PUSH_BATCH;
        let page_complete = !local_more && !has_more;
        let budget_exhausted = page_budget.is_some_and(|budget| pages >= budget);
        info!(
            server = %server,
            page = pages,
            pushed = pending,
            pulled = applied,
            cursor,
            complete = page_complete,
            request_bytes,
            request_wire_bytes,
            response_decoded_bytes = response_bytes,
            response_compression = last_response_compression.as_deref().unwrap_or("none"),
            http_ms,
            apply_ms,
            has_more,
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
        request_wire_bytes = total_request_wire_bytes,
        response_decoded_bytes = total_response_decoded_bytes,
        response_compression = last_response_compression.as_deref().unwrap_or("none"),
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
        request_wire_bytes: total_request_wire_bytes,
        response_decoded_bytes: total_response_decoded_bytes,
        response_compression: last_response_compression.unwrap_or_else(|| "none".to_string()),
        apply_ms: total_apply_ms,
    })
}
