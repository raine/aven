use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use aven_core::db::Database;
use aven_core::sync::ServerSyncPage;
use aven_core::sync::wire::{BlobUploadContract, MissingBlobsRequest, MissingBlobsResponse};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path as AxumPath, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use axum::{Json, Router};
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tracing::{error, info, warn};

use super::wire::{
    SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse, validate_pushed_change,
    validate_sync_request_envelope,
};
use crate::cli::ServerArgs;
use crate::config;
use crate::signals::shutdown_signal;

const SERVER_BLOB_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct ServerState {
    database: Database,
    auth_token: Option<String>,
    blob_dir: PathBuf,
    lifecycle_policy: aven_core::attachments::LifecyclePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindScope {
    Loopback,
    Private,
    Public,
}

impl BindScope {
    fn classify(addr: IpAddr) -> Self {
        if addr.is_loopback() {
            Self::Loopback
        } else if is_private_addr(addr) {
            Self::Private
        } else {
            Self::Public
        }
    }
}

impl fmt::Display for BindScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loopback => f.write_str("loopback"),
            Self::Private => f.write_str("private"),
            Self::Public => f.write_str("public"),
        }
    }
}

fn is_private_addr(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(addr) => {
            let octets = addr.octets();
            addr.is_private()
                || addr.is_link_local()
                || octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(addr) => addr.is_unique_local() || addr.is_unicast_link_local(),
    }
}

fn validate_bind_policy(
    scope: BindScope,
    unsafe_public_bind: bool,
    auth_token: Option<&str>,
) -> Result<()> {
    match scope {
        BindScope::Loopback => Ok(()),
        BindScope::Private => {
            if auth_token.is_none() {
                bail!(
                    "error private-bind-requires-auth hint=\"set sync.auth_token or bind to 127.0.0.1\""
                );
            }
            Ok(())
        }
        BindScope::Public => {
            if !unsafe_public_bind {
                bail!("error public-bind-requires --unsafe-public-bind");
            }
            if auth_token.is_none() {
                bail!("error sync-auth-token-required hint=\"set sync.auth_token in config.yaml\"");
            }
            Ok(())
        }
    }
}

pub(crate) async fn run_server(args: ServerArgs, config: config::AppConfig) -> Result<()> {
    let scope = BindScope::classify(args.bind.ip());
    let auth_token = config.sync_auth_token().map(str::to_string);
    let auth_enabled = auth_token.is_some();
    validate_bind_policy(scope, args.unsafe_public_bind, auth_token.as_deref())?;
    let database = Database::open(&args.data).await?;
    let blob_dir = config::resolve_blob_dir(&args.data, &config)?;
    let lifecycle_policy = config.local.attachment_lifecycle.server_policy();
    let state = ServerState {
        database,
        auth_token,
        blob_dir,
        lifecycle_policy,
    };
    info!(
        bind = %args.bind,
        scope = %scope,
        auth_enabled,
        "sync server starting"
    );
    let app = Router::new()
        .route("/sync", post(sync_handler))
        .route("/sync/blobs/missing", post(missing_blobs_handler))
        .route(
            "/sync/blobs/{sha256}",
            put(put_blob_handler)
                .get(get_blob_handler)
                .layer(DefaultBodyLimit::max(
                    aven_core::attachments::MAX_BLOB_BYTES,
                )),
        )
        .layer(RequestDecompressionLayer::new())
        .layer(middleware::from_fn_with_state(state.clone(), verify_auth))
        .layer(CompressionLayer::new())
        .with_state(state.clone());
    let listener = TcpListener::bind(args.bind).await?;
    let addr = listener.local_addr()?;
    if scope == BindScope::Public {
        println!("warning public bind enabled; use TLS or a reverse proxy");
    }
    println!("listening url=http://{} scope={}", addr, scope);
    let maintenance = spawn_blob_maintenance(state);
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    maintenance.abort();
    let _ = maintenance.await;
    result?;
    Ok(())
}

fn spawn_blob_maintenance(state: ServerState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SERVER_BLOB_MAINTENANCE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            match state
                .database
                .maintain_server_blobs(&state.blob_dir, state.lifecycle_policy)
                .await
            {
                Ok(summary) => info!(
                    eligible = summary.eligible.count,
                    eligible_bytes = summary.eligible.bytes,
                    pruned = summary.pruned.count,
                    pruned_bytes = summary.pruned.bytes,
                    "attachment maintenance completed"
                ),
                Err(err) => warn!(error = %err, "attachment maintenance failed"),
            }
        }
    })
}

async fn verify_auth(
    State(state): State<ServerState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Err(status) = validate_auth(&state, &headers) {
        if status == StatusCode::UNAUTHORIZED {
            warn!(
                auth_enabled = state.auth_token.is_some(),
                "sync request unauthorized"
            );
        }
        return status.into_response();
    }
    next.run(request).await
}

async fn sync_handler(
    State(state): State<ServerState>,
    Json(request): Json<SyncRequest>,
) -> Response {
    match handle_sync(state.clone(), request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => {
            if err.0.is_server_error() {
                error!(status = %err.0, error = %err.1, "sync request failed");
            } else {
                warn!(status = %err.0, error = %err.1, "sync request rejected");
            }
            err.into_response()
        }
    }
}

async fn missing_blobs_handler(
    State(state): State<ServerState>,
    Json(request): Json<MissingBlobsRequest>,
) -> Response {
    match state
        .database
        .prepare_server_blob_uploads(&state.blob_dir, state.lifecycle_policy, &request.blobs)
        .await
    {
        Ok(missing) => Json(MissingBlobsResponse { missing }).into_response(),
        Err(err) if err.to_string().contains("attachment-quota-exceeded") => (
            StatusCode::INSUFFICIENT_STORAGE,
            "error attachment-quota-exceeded".to_string(),
        )
            .into_response(),
        Err(err)
            if err.to_string().contains("invalid-sync-change")
                || err.to_string().contains("blob-batch")
                || err.to_string().contains("duplicate-blob") =>
        {
            (StatusCode::BAD_REQUEST, err.to_string()).into_response()
        }
        Err(err) => internal_error(err).into_response(),
    }
}

async fn put_blob_handler(
    State(state): State<ServerState>,
    AxumPath(sha256): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(workspace_id) = header_text(&headers, "x-aven-workspace-id") else {
        return (StatusCode::BAD_REQUEST, "error blob-workspace-required").into_response();
    };
    let Some(media_type) = header_text(&headers, "content-type") else {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "error unsupported-attachment-media-type",
        )
            .into_response();
    };
    let contract = BlobUploadContract {
        workspace_id: workspace_id.to_string(),
        sha256,
        byte_size: match required_i64_header(&headers, "x-aven-byte-size") {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        },
        media_type: media_type.to_string(),
        width: match required_i64_header(&headers, "x-aven-width") {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        },
        height: match required_i64_header(&headers, "x-aven-height") {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        },
    };
    match state
        .database
        .store_server_blob(
            &state.blob_dir,
            state.lifecycle_policy,
            &contract,
            body.to_vec(),
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) if err.to_string().contains("attachment-quota-exceeded") => (
            StatusCode::INSUFFICIENT_STORAGE,
            "error attachment-quota-exceeded",
        )
            .into_response(),
        Err(err)
            if err
                .to_string()
                .contains("unsupported-attachment-media-type") =>
        {
            (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "error unsupported-attachment-media-type".to_string(),
            )
                .into_response()
        }
        Err(err)
            if err.to_string().contains("blob-hash-or-size-mismatch")
                || err.to_string().contains("blob-inventory-metadata-mismatch")
                || err.to_string().contains("blob-validation-failed")
                || err.to_string().contains("invalid-sync-change") =>
        {
            (StatusCode::BAD_REQUEST, err.to_string()).into_response()
        }
        Err(err) => internal_error(err).into_response(),
    }
}

async fn get_blob_handler(
    State(state): State<ServerState>,
    AxumPath(sha256): AxumPath<String>,
) -> Response {
    match state
        .database
        .read_server_blob(&state.blob_dir, &sha256)
        .await
    {
        Ok(Some(blob)) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            blob.bytes,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) if err.to_string().contains("invalid") => {
            (StatusCode::BAD_REQUEST, err.to_string()).into_response()
        }
        Err(err) => internal_error(err).into_response(),
    }
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn required_i64_header(headers: &HeaderMap, name: &str) -> Result<i64, (StatusCode, String)> {
    header_text(headers, name)
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("error blob-header-invalid name={name}"),
            )
        })
}

fn internal_error(err: impl std::fmt::Display) -> (StatusCode, String) {
    error!(error = %err, "sync server request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "error internal-server-error".to_string(),
    )
}

fn server_sync_error(err: anyhow::Error) -> (StatusCode, String) {
    let message = err.to_string();
    if message.contains("attachment-blob") || message.contains("blob-inventory-metadata-mismatch") {
        (StatusCode::BAD_REQUEST, message)
    } else {
        internal_error(err)
    }
}

fn invalid_sync_change(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, err.to_string())
}

fn validate_auth(state: &ServerState, headers: &HeaderMap) -> std::result::Result<(), StatusCode> {
    let Some(expected) = state.auth_token.as_deref() else {
        return Ok(());
    };
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected);
    if authorized {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn handle_sync(
    state: ServerState,
    request: SyncRequest,
) -> std::result::Result<SyncResponse, (StatusCode, String)> {
    let envelope = validate_sync_request_envelope(&request).map_err(invalid_sync_change)?;
    for change in &request.changes {
        validate_pushed_change(change).map_err(invalid_sync_change)?;
    }

    let client_id = request.client_id.clone();
    let after = envelope.after;
    let pull_limit = envelope.pull_limit;
    let change_count = envelope.push_count;
    info!(client_id = %client_id, after, change_count, pull_limit, "sync request received");

    let persisted = state
        .database
        .persist_server_sync_page_with_blobs(ServerSyncPage { request }, &state.blob_dir)
        .await
        .map_err(server_sync_error)?;
    let cursor = persisted
        .changes
        .last()
        .and_then(|change| change.server_seq)
        .unwrap_or(after);
    info!(
        client_id = %client_id,
        after,
        incoming = change_count,
        accepted = persisted.accepted_count,
        returned = persisted.changes.len(),
        cursor,
        has_more = persisted.has_more,
        blob_prepare_ms = persisted.blob_prepare_ms,
        assign_ms = persisted.assign_ms,
        pull_query_ms = persisted.pull_query_ms,
        "sync request completed"
    );
    Ok(SyncResponse {
        protocol_version: SYNC_PROTOCOL_VERSION,
        cursor,
        has_more: persisted.has_more,
        push_acks: persisted.push_acks,
        changes: persisted.changes,
    })
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{BindScope, validate_bind_policy};

    #[test]
    fn classifies_bind_scope() {
        for addr in ["127.0.0.1", "::1"] {
            assert_eq!(
                BindScope::classify(addr.parse::<IpAddr>().unwrap()),
                BindScope::Loopback
            );
        }
        for addr in [
            "10.0.0.5",
            "172.16.0.5",
            "192.168.1.5",
            "100.64.0.5",
            "100.127.255.5",
            "169.254.1.5",
            "fd00::1",
            "fe80::1",
        ] {
            assert_eq!(
                BindScope::classify(addr.parse::<IpAddr>().unwrap()),
                BindScope::Private
            );
        }
        for addr in ["8.8.8.8", "100.128.0.1", "1.1.1.1", "2001:4860:4860::8888"] {
            assert_eq!(
                BindScope::classify(addr.parse::<IpAddr>().unwrap()),
                BindScope::Public
            );
        }
    }

    #[test]
    fn bind_policy_enforces_guardrails() {
        assert!(validate_bind_policy(BindScope::Loopback, false, None).is_ok());
        assert!(validate_bind_policy(BindScope::Private, false, Some("secret")).is_ok());
        assert!(validate_bind_policy(BindScope::Public, true, Some("secret")).is_ok());

        assert_eq!(
            validate_bind_policy(BindScope::Private, false, None)
                .unwrap_err()
                .to_string(),
            "error private-bind-requires-auth hint=\"set sync.auth_token or bind to 127.0.0.1\""
        );
        assert_eq!(
            validate_bind_policy(BindScope::Public, false, Some("secret"))
                .unwrap_err()
                .to_string(),
            "error public-bind-requires --unsafe-public-bind"
        );
        assert_eq!(
            validate_bind_policy(BindScope::Public, true, None)
                .unwrap_err()
                .to_string(),
            "error sync-auth-token-required hint=\"set sync.auth_token in config.yaml\""
        );
    }
}
