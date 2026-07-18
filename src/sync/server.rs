use std::fmt;
use std::net::IpAddr;

use anyhow::{Result, bail};
use aven_core::db::Database;
use aven_core::sync::ServerSyncPage;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
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

#[derive(Clone)]
struct ServerState {
    database: Database,
    auth_token: Option<String>,
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
    let state = ServerState {
        database,
        auth_token,
    };
    info!(
        bind = %args.bind,
        scope = %scope,
        auth_enabled,
        "sync server starting"
    );
    let app = Router::new()
        .route("/sync", post(sync_handler))
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
    match handle_sync(state, request).await {
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

fn internal_error(err: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
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
        .persist_server_sync_page(ServerSyncPage { request })
        .await
        .map_err(internal_error)?;
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
