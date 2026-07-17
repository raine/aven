use std::collections::HashSet;
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path as AxumPath, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use axum::{Json, Router};
use sqlx::{SqliteConnection, SqlitePool};
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tracing::{error, info, warn};

use super::wire::{
    BlobUploadContract, ChangeRow, ChangeWire, MissingBlobsRequest, MissingBlobsResponse, PushAck,
    SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse, validate_blob_contracts,
    validate_blob_hashes, validate_pushed_change, validate_sync_request_envelope,
};
use crate::change_log::op_type;
use crate::cli::ServerArgs;
use crate::config;
use crate::db::{begin_immediate, open_db};
use crate::signals::shutdown_signal;

#[derive(Clone)]
struct ServerState {
    pool: SqlitePool,
    auth_token: Option<String>,
    blob_dir: PathBuf,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
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
    let pool = open_db(&args.data).await?;
    let blob_dir = config::resolve_blob_dir(&args.data, &config)?;
    let lifecycle_policy = config.local.attachment_lifecycle.server_policy();
    let state = ServerState {
        pool,
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
                .layer(DefaultBodyLimit::max(crate::attachments::MAX_BLOB_BYTES)),
        )
        .layer(RequestDecompressionLayer::new())
        .layer(middleware::from_fn_with_state(state.clone(), verify_auth))
        .layer(CompressionLayer::new())
        .with_state(state);
    let listener = TcpListener::bind(args.bind).await?;
    let addr = listener.local_addr()?;
    if scope == BindScope::Public {
        println!("warning public bind enabled; use TLS or a reverse proxy");
    }
    println!("listening url=http://{} scope={}", addr, scope);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
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
        Ok(response) => {
            if let Ok(mut conn) = state.pool.acquire().await
                && let Err(err) = crate::attachments::lifecycle::prune(
                    &mut conn,
                    &state.blob_dir,
                    state.lifecycle_policy,
                    true,
                    &crate::attachments::lifecycle::SystemClock,
                )
                .await
            {
                warn!(error = %err, "attachment maintenance failed");
            }
            Json(response).into_response()
        }
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
    if let Err(err) = validate_blob_contracts(&request.blobs) {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }
    match prepare_blob_uploads(&state, &request.blobs).await {
        Ok(missing) => Json(MissingBlobsResponse { missing }).into_response(),
        Err(err) if err.to_string().contains("attachment-quota-exceeded") => (
            StatusCode::INSUFFICIENT_STORAGE,
            "error attachment-quota-exceeded".to_string(),
        )
            .into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

async fn put_blob_handler(
    State(state): State<ServerState>,
    AxumPath(sha256): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(err) = validate_blob_hashes(std::slice::from_ref(&sha256)) {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }
    if crate::attachments::sha256_hex(&body) != sha256 {
        return (
            StatusCode::BAD_REQUEST,
            "error blob-hash-mismatch".to_string(),
        )
            .into_response();
    }
    let media_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if crate::attachments::validate_media_type(media_type).is_err() {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "error unsupported-attachment-media-type".to_string(),
        )
            .into_response();
    }
    let workspace_id = match headers
        .get("x-aven-workspace-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        Some(value) => value,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "error blob-workspace-required".to_string(),
            )
                .into_response();
        }
    };
    let contract = BlobUploadContract {
        workspace_id: workspace_id.to_string(),
        sha256: sha256.clone(),
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
    if let Err(err) = validate_blob_contracts(std::slice::from_ref(&contract)) {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }
    if usize::try_from(contract.byte_size).ok() != Some(body.len()) {
        return (
            StatusCode::BAD_REQUEST,
            "error blob-size-mismatch".to_string(),
        )
            .into_response();
    }
    let validated = match crate::attachments::decode::validate_image(
        body.to_vec(),
        Some(contract.media_type.clone()),
    )
    .await
    {
        Ok(validated)
            if (validated.facts.width, validated.facts.height)
                == (contract.width, contract.height) =>
        {
            validated
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "error blob-validation-failed".to_string(),
            )
                .into_response();
        }
    };
    let mut conn = match state.pool.acquire().await {
        Ok(conn) => conn,
        Err(err) => return internal_error(err).into_response(),
    };
    if let Err(err) = crate::attachments::lifecycle::prune(
        &mut conn,
        &state.blob_dir,
        state.lifecycle_policy,
        true,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await
    {
        return internal_error(err).into_response();
    }
    let reservation_id = match crate::attachments::lifecycle::reserve_upload(
        &mut conn,
        &contract.workspace_id,
        &contract.sha256,
        contract.byte_size,
        state.lifecycle_policy.quota_bytes,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await
    {
        Ok(reservation_id) => reservation_id,
        Err(err) if err.to_string().contains("attachment-quota-exceeded") => {
            return (
                StatusCode::INSUFFICIENT_STORAGE,
                "error attachment-quota-exceeded".to_string(),
            )
                .into_response();
        }
        Err(err) => return internal_error(err).into_response(),
    };
    let store_result =
        crate::attachments::storage::store_validated_blob(&mut conn, &state.blob_dir, validated)
            .await;
    if store_result.is_err()
        && let Some(reservation_id) = reservation_id
        && let Err(err) =
            crate::attachments::lifecycle::release_reservation(&mut conn, &reservation_id).await
    {
        return internal_error(err).into_response();
    }
    match store_result {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) if err.to_string().contains("blob-inventory-metadata-mismatch") => (
            StatusCode::BAD_REQUEST,
            "error blob-inventory-metadata-mismatch".to_string(),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            "error blob-validation-failed".to_string(),
        )
            .into_response(),
    }
}

async fn get_blob_handler(
    State(state): State<ServerState>,
    AxumPath(sha256): AxumPath<String>,
) -> Response {
    if let Err(err) = validate_blob_hashes(std::slice::from_ref(&sha256)) {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }
    let mut conn = match state.pool.acquire().await {
        Ok(conn) => conn,
        Err(err) => return internal_error(err).into_response(),
    };
    match blob_available(&mut conn, &state.blob_dir, &sha256).await {
        Ok(true) => {}
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => return internal_error(err).into_response(),
    }
    let lease_id = match crate::attachments::lifecycle::acquire_lease(
        &mut conn,
        &sha256,
        "transfer",
        &crate::attachments::lifecycle::SystemClock,
    )
    .await
    {
        Ok(lease_id) => lease_id,
        Err(err) => return internal_error(err).into_response(),
    };
    let path = match crate::attachments::object_path(&state.blob_dir, &sha256) {
        Ok(path) => path,
        Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    };
    let read_result = tokio::fs::read(path).await;
    if let Err(err) = crate::attachments::lifecycle::release_lease(&mut conn, &lease_id).await {
        return internal_error(err).into_response();
    }
    match read_result {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "error blob-read-failed".to_string(),
        )
            .into_response(),
    }
}

fn required_i64_header(
    headers: &HeaderMap,
    name: &str,
) -> std::result::Result<i64, (StatusCode, String)> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("error blob-header-invalid name={name}"),
            )
        })
}

fn internal_error(_err: impl std::fmt::Display) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "error internal-server-error".to_string(),
    )
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

async fn apply_server_blob_reference(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    match change.op_type.as_str() {
        op_type::ATTACHMENT_ADD => {
            let workspace_id = change
                .payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .context("payload missing workspace_id")?;
            let attachment_id = change
                .payload
                .get("attachment_id")
                .and_then(serde_json::Value::as_str)
                .context("payload missing attachment_id")?;
            let sha256 = change
                .payload
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .context("payload missing sha256")?;
            let byte_size = change
                .payload
                .get("byte_size")
                .and_then(serde_json::Value::as_i64)
                .context("payload missing byte_size")?;
            sqlx::query(
                "INSERT INTO server_blob_references(
                   workspace_id, attachment_id, task_id, sha256, byte_size, deleted
                 ) VALUES (?, ?, ?, ?, ?, 0)
                 ON CONFLICT(workspace_id, attachment_id) DO UPDATE SET
                   task_id = excluded.task_id,
                   sha256 = excluded.sha256,
                   byte_size = excluded.byte_size,
                   deleted = 0",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .bind(&change.entity_id)
            .bind(sha256)
            .bind(byte_size)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "DELETE FROM blob_upload_reservations WHERE workspace_id = ? AND sha256 = ?",
            )
            .bind(workspace_id)
            .bind(sha256)
            .execute(&mut *conn)
            .await?;
        }
        op_type::ATTACHMENT_DELETE => {
            let workspace_id = change
                .payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .context("payload missing workspace_id")?;
            let attachment_id = change
                .payload
                .get("attachment_id")
                .and_then(serde_json::Value::as_str)
                .context("payload missing attachment_id")?;
            sqlx::query(
                "UPDATE server_blob_references SET deleted = 1
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .execute(&mut *conn)
            .await?;
        }
        op_type::SET_FIELD if change.field.as_deref() == Some("deleted") => {
            let workspace_id = change
                .payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .context("payload missing workspace_id")?;
            let deleted = change
                .payload
                .get("value")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value == "1");
            sqlx::query(
                "INSERT INTO server_task_tombstones(workspace_id, task_id, deleted)
                 VALUES (?, ?, ?)
                 ON CONFLICT(workspace_id, task_id) DO UPDATE SET deleted = excluded.deleted",
            )
            .bind(workspace_id)
            .bind(&change.entity_id)
            .bind(i64::from(deleted))
            .execute(&mut *conn)
            .await?;
        }
        _ => {}
    }
    Ok(())
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

    let mut conn = state.pool.acquire().await.map_err(internal_error)?;
    let assign_started = Instant::now();
    let (accepted_count, push_acks) = if request.changes.is_empty() {
        (0_i64, Vec::new())
    } else {
        let mut tx = begin_immediate(&mut conn).await.map_err(internal_error)?;
        ensure_attachment_blobs_present(&mut tx, &state.blob_dir, &request.changes)
            .await
            .map_err(invalid_sync_change)?;
        let mut next_server_seq = next_available_server_seq(&mut tx)
            .await
            .map_err(internal_error)?;
        let mut accepted_count = 0_i64;
        let mut push_acks = Vec::with_capacity(request.changes.len());
        for change in request.changes {
            let existing_server_seq = sqlx::query_scalar!(
                r#"SELECT server_seq AS "server_seq?: i64" FROM changes WHERE change_id = ?"#,
                change.change_id
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_error)?
            .flatten();
            let server_seq = if let Some(server_seq) = existing_server_seq {
                server_seq
            } else {
                let server_seq = next_server_seq;
                next_server_seq += 1;
                let payload = change.payload.to_string();
                sqlx::query!(
                    "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
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
                    server_seq,
                )
                .execute(&mut *tx)
                .await
                .map_err(internal_error)?;
                apply_server_blob_reference(&mut tx, &change)
                    .await
                    .map_err(internal_error)?;
                accepted_count += 1;
                server_seq
            };
            push_acks.push(PushAck {
                change_id: change.change_id,
                server_seq,
            });
        }
        tx.commit().await.map_err(internal_error)?;
        (accepted_count, push_acks)
    };
    let assign_ms = assign_started.elapsed().as_millis();
    let pull_query_started = Instant::now();
    let (changes, has_more) = load_server_changes_after(&mut conn, after, pull_limit)
        .await
        .map_err(internal_error)?;
    let pull_query_ms = pull_query_started.elapsed().as_millis();
    let cursor = changes
        .last()
        .and_then(|change| change.server_seq)
        .unwrap_or(after);
    info!(
        client_id = %client_id,
        after,
        incoming = change_count,
        accepted = accepted_count,
        returned = changes.len(),
        cursor,
        has_more,
        assign_ms,
        pull_query_ms,
        "sync request completed"
    );
    Ok(SyncResponse {
        protocol_version: SYNC_PROTOCOL_VERSION,
        cursor,
        has_more,
        push_acks,
        changes,
    })
}

async fn next_available_server_seq(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(server_seq), 0) + 1 AS "seq!: i64" FROM changes"#
    )
    .fetch_one(&mut *conn)
    .await?)
}

async fn prepare_blob_uploads(
    state: &ServerState,
    blobs: &[BlobUploadContract],
) -> Result<Vec<String>> {
    let mut conn = state.pool.acquire().await?;
    crate::attachments::lifecycle::prune(
        &mut conn,
        &state.blob_dir,
        state.lifecycle_policy,
        true,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let mut missing = Vec::new();
    let mut missing_hashes = HashSet::new();
    for blob in blobs {
        if blob_available(&mut conn, &state.blob_dir, &blob.sha256).await? {
            crate::attachments::lifecycle::reserve_upload(
                &mut conn,
                &blob.workspace_id,
                &blob.sha256,
                blob.byte_size,
                state.lifecycle_policy.quota_bytes,
                &crate::attachments::lifecycle::SystemClock,
            )
            .await?;
        } else if missing_hashes.insert(blob.sha256.as_str()) {
            missing.push(blob.sha256.clone());
        }
    }
    Ok(missing)
}

async fn blob_available(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    sha256: &str,
) -> Result<bool> {
    let Some(row) = crate::attachments::blob_inventory_row(conn, sha256).await? else {
        return Ok(false);
    };
    if !row.available {
        return Ok(false);
    }
    Ok(crate::attachments::object_path(blob_dir, sha256)?.exists())
}

async fn ensure_attachment_blobs_present(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    changes: &[ChangeWire],
) -> Result<()> {
    for change in changes {
        if change.op_type != op_type::ATTACHMENT_ADD {
            continue;
        }
        let sha256 = change
            .payload
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .context("error attachment-blob-missing")?;
        let byte_size = change
            .payload
            .get("byte_size")
            .and_then(serde_json::Value::as_i64)
            .context("error attachment-blob-missing")?;
        let media_type = change
            .payload
            .get("media_type")
            .and_then(serde_json::Value::as_str)
            .context("error attachment-blob-missing")?;
        let width = change
            .payload
            .get("width")
            .and_then(serde_json::Value::as_i64)
            .context("error attachment-blob-missing")?;
        let height = change
            .payload
            .get("height")
            .and_then(serde_json::Value::as_i64)
            .context("error attachment-blob-missing")?;
        ensure_attachment_blob_matches(
            conn, blob_dir, sha256, byte_size, media_type, width, height,
        )
        .await?;
        let workspace_id = change
            .payload
            .get("workspace_id")
            .and_then(serde_json::Value::as_str)
            .context("error attachment-blob-missing")?;
        let admitted: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM server_blob_references sbr
               LEFT JOIN server_task_tombstones st
                 ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
               WHERE sbr.workspace_id = ? AND sbr.sha256 = ? AND sbr.deleted = 0
                 AND COALESCE(st.deleted, 0) = 0
             ) OR EXISTS(
               SELECT 1 FROM blob_upload_reservations
               WHERE workspace_id = ? AND sha256 = ? AND byte_size = ? AND expires_at > ?
             )",
        )
        .bind(workspace_id)
        .bind(sha256)
        .bind(workspace_id)
        .bind(sha256)
        .bind(byte_size)
        .bind(crate::ids::now())
        .fetch_one(&mut *conn)
        .await?;
        if !admitted {
            bail!("error attachment-blob-unreserved");
        }
    }
    Ok(())
}

async fn ensure_attachment_blob_matches(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    sha256: &str,
    byte_size: i64,
    media_type: &str,
    width: i64,
    height: i64,
) -> Result<()> {
    let Some(row) = crate::attachments::blob_inventory_row(conn, sha256).await? else {
        bail!("error attachment-blob-missing");
    };
    if !row.available || !crate::attachments::object_path(blob_dir, sha256)?.exists() {
        bail!("error attachment-blob-missing");
    }
    if row.byte_size != byte_size || row.media_type != media_type {
        bail!("error blob-inventory-metadata-mismatch");
    }
    let path = crate::attachments::object_path(blob_dir, sha256)?;
    let bytes = tokio::fs::read(path).await?;
    let validated =
        crate::attachments::decode::validate_image(bytes, Some(media_type.to_string())).await?;
    if (validated.facts.width, validated.facts.height) != (width, height) {
        bail!("error blob-inventory-metadata-mismatch");
    }
    Ok(())
}

async fn load_server_changes_after(
    conn: &mut SqliteConnection,
    after: i64,
    pull_limit: u32,
) -> Result<(Vec<ChangeWire>, bool)> {
    let fetch_limit = i64::from(pull_limit) + 1;
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq > ? ORDER BY server_seq LIMIT ?"#,
        after,
        fetch_limit,
    )
    .fetch_all(&mut *conn)
    .await?;
    let has_more = rows.len() > pull_limit as usize;
    let changes = rows
        .into_iter()
        .take(pull_limit as usize)
        .map(ChangeRow::into_wire)
        .collect();
    Ok((changes, has_more))
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
