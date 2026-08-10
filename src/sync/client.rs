use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use aven_core::attachments::LifecyclePolicy;
use aven_core::db::Database;
use aven_core::sync::wire::MAX_BLOB_TRANSFER_BYTES;
use aven_core::sync::{
    PreparedSyncRequest, SyncHttpHeader, SyncHttpResponse, SyncRetryDecision, SyncSession,
};
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
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
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

#[derive(Debug)]
pub(crate) enum DaemonSyncOutcome {
    Completed(SyncSummary),
    Deferred,
}

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
    config.ensure_sync_allowed()?;
    let server = config::resolve_sync_server(None, config)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let client = SyncHttpClient::new()?;
    let _guard = super::coordination::acquire(database).await?;
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
    config.ensure_sync_allowed()?;
    let server = config::resolve_sync_server(args.server.as_deref(), config)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let client = SyncHttpClient::new()?;
    let _guard = super::coordination::acquire(database).await?;
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
    run_sync_session(
        database,
        blob_dir,
        server,
        auth_token,
        page_budget,
        client,
        lifecycle_policy,
    )
    .await
}

pub(crate) async fn try_run_daemon_sync_with_page_budget(
    database: &Database,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
    page_budget: usize,
    client: &SyncHttpClient,
    lifecycle_policy: LifecyclePolicy,
) -> Result<DaemonSyncOutcome> {
    let Some(_guard) = super::coordination::try_acquire(database)? else {
        return Ok(DaemonSyncOutcome::Deferred);
    };
    run_sync_session(
        database,
        blob_dir,
        server,
        auth_token,
        Some(page_budget),
        client,
        lifecycle_policy,
    )
    .await
    .map(DaemonSyncOutcome::Completed)
}

#[allow(clippy::too_many_arguments)]
async fn run_sync_session(
    database: &Database,
    blob_dir: &Path,
    server: &str,
    auth_token: Option<&str>,
    page_budget: Option<usize>,
    client: &SyncHttpClient,
    lifecycle_policy: LifecyclePolicy,
) -> Result<SyncSummary> {
    let mut session = SyncSession::start_with_attachment_storage(
        database.clone(),
        server.to_string(),
        auth_token.map(str::to_string),
        page_budget,
        blob_dir.to_path_buf(),
        lifecycle_policy,
    )
    .await?;
    drive_sync_session(&mut session, server, client).await
}

enum SendPreparedError {
    Transient(anyhow::Error),
    Terminal(anyhow::Error),
}

async fn send_prepared(
    client: &SyncHttpClient,
    prepared: &PreparedSyncRequest,
) -> std::result::Result<SyncHttpResponse, SendPreparedError> {
    let method = reqwest::Method::from_bytes(prepared.method.as_bytes()).map_err(|error| {
        SendPreparedError::Terminal(anyhow::Error::new(error).context("invalid sync HTTP method"))
    })?;
    let mut request = client
        .inner
        .request(method, &prepared.url)
        .timeout(Duration::from_millis(prepared.timeout.attempt_ms))
        .body(prepared.body.clone());
    for header in &prepared.headers {
        request = request.header(&header.name, &header.value);
    }
    let mut response = request.send().await.map_err(|error| {
        if error.is_builder() {
            SendPreparedError::Terminal(anyhow::Error::new(error))
        } else {
            SendPreparedError::Transient(anyhow::Error::new(error))
        }
    })?;
    let status = response.status().as_u16();
    let headers = [
        reqwest::header::CONTENT_ENCODING,
        reqwest::header::CONTENT_LENGTH,
        reqwest::header::CONTENT_TYPE,
        reqwest::header::RETRY_AFTER,
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
    let response_limit = usize::try_from(MAX_BLOB_TRANSFER_BYTES).map_err(|error| {
        SendPreparedError::Terminal(
            anyhow::Error::new(error).context("sync response limit exceeds usize"),
        )
    })?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BLOB_TRANSFER_BYTES)
    {
        return Err(SendPreparedError::Terminal(anyhow::anyhow!(
            "error sync-response-too-large"
        )));
    }
    let inactivity = Duration::from_millis(prepared.timeout.inactivity_ms);
    let mut body = Vec::new();
    loop {
        let chunk = tokio::time::timeout(inactivity, response.chunk())
            .await
            .map_err(|_| SendPreparedError::Transient(anyhow::anyhow!("sync response stalled")))?
            .map_err(|error| SendPreparedError::Transient(anyhow::Error::new(error)))?;
        let Some(chunk) = chunk else {
            break;
        };
        if body.len().saturating_add(chunk.len()) > response_limit {
            return Err(SendPreparedError::Terminal(anyhow::anyhow!(
                "error sync-response-too-large"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(SyncHttpResponse {
        status,
        headers,
        body,
    })
}

async fn drive_sync_session(
    session: &mut SyncSession,
    server: &str,
    client: &SyncHttpClient,
) -> Result<SyncSummary> {
    info!(server = %server, http_client_id = %client.id(), "sync client starting");
    loop {
        let prepared = match session.prepare_request().await {
            Ok(Some(prepared)) => prepared,
            Ok(None) => break,
            Err(error) => {
                session.fail("sync request preparation failed").await?;
                return Err(error);
            }
        };
        let http_started = Instant::now();
        let response = loop {
            match send_prepared(client, &prepared).await {
                Ok(response) if !(200..300).contains(&response.status) => {
                    match session.register_http_failure(
                        &prepared.context,
                        response.status,
                        &response.headers,
                    )? {
                        SyncRetryDecision::RetryAfter { delay_ms } => {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        SyncRetryDecision::Stop => break response,
                    }
                }
                Ok(response) => break response,
                Err(SendPreparedError::Transient(error)) => {
                    match session.register_transport_failure(&prepared.context)? {
                        SyncRetryDecision::RetryAfter { delay_ms } => {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        SyncRetryDecision::Stop => {
                            session
                                .fail_request(&prepared.context, "sync transport failed")
                                .await?;
                            return Err(error);
                        }
                    }
                }
                Err(SendPreparedError::Terminal(error)) => {
                    session
                        .fail_request(&prepared.context, "sync response rejected")
                        .await?;
                    return Err(error);
                }
            }
        };
        let http_ms = http_started.elapsed().as_millis();
        let outcome = match session.accept_response(&prepared.context, response).await {
            Ok(outcome) => outcome,
            Err(error) => {
                session
                    .fail_request(&prepared.context, "sync response rejected")
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderValue, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;

    use super::*;

    #[derive(Clone)]
    struct ScriptState {
        attempts: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<Vec<u8>>>>,
        first_status: StatusCode,
        response_delay: Duration,
    }

    async fn scripted_sync(State(state): State<ScriptState>, body: Bytes) -> Response {
        state.bodies.lock().unwrap().push(body.to_vec());
        let attempt = state.attempts.fetch_add(1, Ordering::SeqCst);
        if !state.response_delay.is_zero() {
            tokio::time::sleep(state.response_delay).await;
        }
        if attempt == 0 && state.first_status != StatusCode::OK {
            let mut response = state.first_status.into_response();
            response
                .headers_mut()
                .insert("retry-after", HeaderValue::from_static("0"));
            return response;
        }
        (
            StatusCode::OK,
            format!(
                "{{\"protocol_version\":{},\"cursor\":0,\"has_more\":false,\"push_acks\":[],\"changes\":[]}}",
                aven_core::sync::wire::SYNC_PROTOCOL_VERSION
            ),
        )
            .into_response()
    }

    async fn scripted_server(state: ScriptState) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/sync", post(scripted_sync))
                    .with_state(state),
            )
            .await
            .unwrap();
        });
        format!("http://{address}")
    }

    fn script(first_status: StatusCode, response_delay: Duration) -> ScriptState {
        ScriptState {
            attempts: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
            first_status,
            response_delay,
        }
    }

    #[tokio::test]
    async fn retryable_status_replays_the_same_request() {
        let state = script(StatusCode::SERVICE_UNAVAILABLE, Duration::ZERO);
        let server = scripted_server(state.clone()).await;
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("aven.sqlite"))
            .await
            .unwrap();
        let mut session = SyncSession::start(database, server.clone(), None, None)
            .await
            .unwrap();

        let summary = drive_sync_session(&mut session, &server, &SyncHttpClient::new().unwrap())
            .await
            .unwrap();

        assert!(summary.complete);
        assert_eq!(state.attempts.load(Ordering::SeqCst), 2);
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
    }

    #[tokio::test]
    async fn non_retryable_status_is_sent_once_and_records_a_safe_error() {
        let state = script(StatusCode::BAD_REQUEST, Duration::ZERO);
        let server = scripted_server(state.clone()).await;
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("aven.sqlite"))
            .await
            .unwrap();
        let mut session = SyncSession::start(database.clone(), server.clone(), None, None)
            .await
            .unwrap();

        assert!(
            drive_sync_session(&mut session, &server, &SyncHttpClient::new().unwrap())
                .await
                .is_err()
        );
        assert_eq!(state.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            database
                .sync_persistence_status()
                .await
                .unwrap()
                .last_error
                .as_deref(),
            Some("sync response rejected")
        );
    }

    #[tokio::test]
    async fn request_timeout_is_a_transient_transport_failure() {
        let state = script(StatusCode::OK, Duration::from_millis(100));
        let server = scripted_server(state).await;
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("aven.sqlite"))
            .await
            .unwrap();
        let mut session = SyncSession::start(database, server, None, None)
            .await
            .unwrap();
        let mut prepared = session.prepare_request().await.unwrap().unwrap();
        prepared.timeout.attempt_ms = 10;

        assert!(matches!(
            send_prepared(&SyncHttpClient::new().unwrap(), &prepared).await,
            Err(SendPreparedError::Transient(_))
        ));
    }
}
