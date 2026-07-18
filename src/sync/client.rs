use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use aven_core::db::Database;
use aven_core::sync::{SyncHttpHeader, SyncHttpResponse, SyncSession};
use tracing::info;

use crate::cli::SyncArgs;
use crate::config;

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

pub(crate) type SyncSummary = aven_core::sync::SyncSessionSummary;

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
    let mut session = SyncSession::start(
        database.clone(),
        server.to_string(),
        auth_token.map(str::to_string),
        page_budget,
    )
    .await?;
    match drive_sync_session(&mut session, server, client).await {
        Ok(summary) => Ok(summary),
        Err(error) => {
            database.record_sync_error(format!("{error:#}")).await?;
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

async fn drive_sync_session(
    session: &mut SyncSession,
    server: &str,
    client: &SyncHttpClient,
) -> Result<SyncSummary> {
    info!(server = %server, http_client_id = %client.id(), "sync client starting");
    while let Some(prepared) = session.prepare_request().await? {
        let method = reqwest::Method::from_bytes(prepared.method.as_bytes())
            .context("invalid prepared sync HTTP method")?;
        let mut request = client
            .inner
            .request(method, &prepared.url)
            .body(prepared.body.clone());
        for header in &prepared.headers {
            request = request.header(&header.name, &header.value);
        }

        let http_started = Instant::now();
        let response = request.send().await?;
        let http_ms = http_started.elapsed().as_millis();
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .map(|value| SyncHttpHeader {
                name: "content-encoding".to_string(),
                value: value.to_string(),
            })
            .into_iter()
            .collect();
        let body = response.bytes().await?.to_vec();
        let outcome = session
            .accept_response(
                &prepared.context,
                SyncHttpResponse {
                    status,
                    headers,
                    body,
                },
            )
            .await?;
        info!(
            server = %server,
            page = outcome.page,
            pushed = outcome.pushed,
            pulled = outcome.pulled,
            cursor = outcome.cursor,
            complete = outcome.complete,
            request_bytes = outcome.request_bytes,
            request_wire_bytes = outcome.request_wire_bytes,
            response_decoded_bytes = outcome.response_decoded_bytes,
            response_compression = outcome.response_compression,
            http_ms,
            apply_ms = outcome.apply_ms,
            has_more = outcome.has_more,
            local_more = outcome.local_more,
            "sync client page completed"
        );
    }

    let summary = session.summary();
    info!(
        server = %server,
        pushed = summary.pushed,
        pulled = summary.pulled,
        cursor = summary.cursor,
        complete = summary.complete,
        pages = summary.pages,
        request_bytes = summary.request_bytes,
        request_wire_bytes = summary.request_wire_bytes,
        response_decoded_bytes = summary.response_decoded_bytes,
        response_compression = summary.response_compression,
        apply_ms = summary.apply_ms,
        "sync client finished"
    );
    Ok(summary)
}
