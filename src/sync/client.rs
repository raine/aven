use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use aven_core::attachments::LifecyclePolicy;
use aven_core::db::Database;
use aven_core::sync::wire::MAX_BLOB_TRANSFER_BYTES;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncRunSummary {
    pub(crate) pushed: i64,
    pub(crate) pulled: usize,
    pub(crate) cursor: i64,
    pub(crate) complete: bool,
    pub(crate) pages: usize,
}

pub(crate) async fn run_sync_to_completion(
    database: &Database,
    config: &config::AppConfig,
) -> Result<SyncRunSummary> {
    config::ensure_sync_allowed(database.path())?;
    let server = config::resolve_sync_server(None, config)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let client = SyncHttpClient::new()?;
    let mut total = SyncRunSummary {
        pushed: 0,
        pulled: 0,
        cursor: 0,
        complete: false,
        pages: 0,
    };

    loop {
        let summary = run_sync_with_page_budget_using_client_and_policy(
            database,
            &blob_dir,
            &server,
            config.sync_auth_token(),
            None,
            &client,
            config.local.attachment_lifecycle.policy(),
        )
        .await?;
        let stalled = summary.pushed == 0
            && summary.pulled == 0
            && summary.blob_uploaded == 0
            && summary.blob_downloaded == 0;
        total.pushed += summary.pushed;
        total.pulled += summary.pulled;
        total.cursor = summary.cursor;
        total.complete = summary.complete;
        total.pages += summary.pages;
        if summary.complete || stalled {
            return Ok(total);
        }
    }
}

pub(crate) async fn sync_client(
    database: &Database,
    args: SyncArgs,
    config: &config::AppConfig,
) -> Result<()> {
    config::ensure_sync_allowed(database.path())?;
    let server = config::resolve_sync_server(args.server.as_deref(), config)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let client = SyncHttpClient::new()?;
    loop {
        let summary = run_sync_with_page_budget_using_client_and_policy(
            database,
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
    database: &Database,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
    client: &SyncHttpClient,
    lifecycle_policy: LifecyclePolicy,
) -> Result<SyncSummary> {
    config::ensure_sync_allowed(database.path())?;
    let mut session = SyncSession::start_with_attachment_storage(
        database.clone(),
        server.to_string(),
        auth_token.map(str::to_string),
        page_budget,
        blob_dir.to_path_buf(),
        lifecycle_policy,
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

async fn drive_sync_session(
    session: &mut SyncSession,
    server: &str,
    client: &SyncHttpClient,
) -> Result<SyncSummary> {
    info!(server = %server, http_client_id = %client.id(), "sync client starting");
    while let Some(prepared) = session.prepare_request().await? {
        let http_started = Instant::now();
        let transport = async {
            let method = reqwest::Method::from_bytes(prepared.method.as_bytes())
                .context("invalid prepared sync HTTP method")?;
            let mut request = client
                .inner
                .request(method, &prepared.url)
                .body(prepared.body.clone());
            for header in &prepared.headers {
                request = request.header(&header.name, &header.value);
            }
            let mut response = request.send().await?;
            let status = response.status().as_u16();
            let headers = [
                reqwest::header::CONTENT_ENCODING,
                reqwest::header::CONTENT_LENGTH,
                reqwest::header::CONTENT_TYPE,
            ]
            .into_iter()
            .filter_map(|name| {
                response
                    .headers()
                    .get(&name)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| SyncHttpHeader {
                        name: name.as_str().to_string(),
                        value: value.to_string(),
                    })
            })
            .collect();
            let response_limit = usize::try_from(MAX_BLOB_TRANSFER_BYTES)
                .context("sync response limit exceeds usize")?;
            if response
                .content_length()
                .is_some_and(|length| length > MAX_BLOB_TRANSFER_BYTES)
            {
                anyhow::bail!("error sync-response-too-large");
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if body.len().saturating_add(chunk.len()) > response_limit {
                    anyhow::bail!("error sync-response-too-large");
                }
                body.extend_from_slice(&chunk);
            }
            Ok::<_, anyhow::Error>((status, headers, body))
        }
        .await;
        let (status, headers, body) = match transport {
            Ok(response) => response,
            Err(error) => {
                session
                    .fail_request(&prepared.context, format!("{error:#}"))
                    .await?;
                return Err(error);
            }
        };
        let http_ms = http_started.elapsed().as_millis();
        let outcome = match session
            .accept_response(
                &prepared.context,
                SyncHttpResponse {
                    status,
                    headers,
                    body,
                },
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                session
                    .fail_request(&prepared.context, format!("{error:#}"))
                    .await?;
                return Err(error);
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sync_client_rejects_worktree_database() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".aven/db.sqlite");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let _pool = crate::test_support::open_db(&db_path).await.unwrap();
        let database = Database::open(&db_path).await.unwrap();

        let error = sync_client(
            &database,
            SyncArgs { server: None },
            &crate::config::AppConfig::default(),
        )
        .await
        .unwrap_err();

        assert!(format!("{error:#}").contains("sync-disabled-in-worktree"));
    }
}
