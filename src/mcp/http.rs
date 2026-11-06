//! Streamable HTTP for `aven mcp --http`. Every tool is a plain request and
//! response, so requests are served statelessly as JSON, with no sessions or
//! server-initiated stream to keep or drain.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::warn;

use super::Server;
use super::tools::Tools;
use crate::cli::McpArgs;
use crate::config::AppConfig;
use crate::daemon::{Embedded, EmbeddedSync};
use crate::http_serve::{MAX_CONNECTIONS, public_bind, serve_connections};
use crate::signals::shutdown_signal;

const ENDPOINT: &str = "/mcp";
const TOKEN_VARIABLE: &str = "AVEN_MCP_TOKEN";
const MIN_TOKEN_LENGTH: usize = 32;
/// Longest shutdown waits for a sync round after requests have drained. Local
/// changes stay queued for the next start, so an unfinished round loses nothing.
const SYNC_STOP_GRACE: Duration = Duration::from_secs(3);

pub(super) async fn run(
    database: Database,
    config: AppConfig,
    workspace: Option<String>,
    blob_dir: PathBuf,
    args: McpArgs,
) -> Result<()> {
    let token = load_token(std::env::var_os(TOKEN_VARIABLE).as_deref())?;
    if public_bind(args.bind, args.unsafe_public_bind)? {
        eprintln!(
            "Warning: public or wildcard bind {} enabled without TLS. The bearer token and task data are sent in plaintext.",
            args.bind
        );
    }
    let (sync, client) = if args.sync {
        match EmbeddedSync::prepare(database.clone(), &config, blob_dir.clone()).await? {
            Embedded::Host(sync) => (Some(*sync), None),
            Embedded::Client(line) => (None, Some(line)),
        }
    } else {
        (None, None)
    };
    let sync_wake = sync.as_ref().map(EmbeddedSync::wake);
    let server = Server::new(Tools::new(database, config, workspace, blob_dir, sync_wake));
    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("could not bind MCP address {}", args.bind))?;
    let address = listener.local_addr()?;

    let (stop, stopped) = watch::channel(false);
    let stopping = wait_for_stop(stopped);
    let mut worker = tokio::spawn(async move {
        match sync {
            Some(sync) => sync.run(stopping).await,
            None => {
                stopping.await;
                Ok(())
            }
        }
    });
    println!("listening url=http://{address}{ENDPOINT}");
    if let Some(line) = client {
        println!("{line}");
    }

    let mut worker_exit = None;
    let shutdown = async {
        worker_exit = tokio::select! {
            () = shutdown_signal() => None,
            exit = &mut worker => Some(exit),
        };
        stop.send_replace(true);
    };
    serve_connections(listener, router(server, token), MAX_CONNECTIONS, shutdown).await?;
    if let Some(exit) = worker_exit {
        exit.context("MCP sync worker failed")??;
        bail!("MCP sync worker stopped unexpectedly");
    }
    match tokio::time::timeout(SYNC_STOP_GRACE, worker).await {
        Ok(exit) => exit.context("MCP sync worker failed")?,
        Err(_) => {
            warn!("sync round still running at shutdown; pending changes stay queued");
            Ok(())
        }
    }
}

async fn wait_for_stop(mut stopped: watch::Receiver<bool>) {
    let _ = stopped.wait_for(|stop| *stop).await;
}

fn router(server: Server, token: Arc<str>) -> Router {
    // Host checks defend unauthenticated local servers against DNS rebinding.
    // Here every request needs the token, and clients reach this server under
    // deployment-specific names. Browsers always identify themselves with
    // Origin on these requests, so rejecting every Origin keeps them out.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .disable_allowed_hosts()
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    Router::new()
        .route_service(ENDPOINT, service)
        .route_layer(middleware::from_fn_with_state(token, require_token))
}

async fn require_token(State(token): State<Arc<str>>, request: Request, next: Next) -> Response {
    if authorized(request.headers(), &token) {
        return next.run(request).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
    )
        .into_response()
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return false;
    };
    let Some((scheme, presented)) = value.to_str().ok().and_then(|value| value.split_once(' '))
    else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && bool::from(presented.as_bytes().ct_eq(token.as_bytes()))
}

fn load_token(value: Option<&std::ffi::OsStr>) -> Result<Arc<str>> {
    let token = value.and_then(std::ffi::OsStr::to_str).unwrap_or_default();
    if token.len() < MIN_TOKEN_LENGTH || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        bail!(
            "error mcp-token-required hint=\"set {TOKEN_VARIABLE} to at least {MIN_TOKEN_LENGTH} visible ASCII characters, for example from `openssl rand -hex 32`\""
        );
    }
    Ok(token.into())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use axum::http::HeaderValue;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn only_a_single_matching_bearer_credential_is_authorized() {
        let valid = format!("Bearer {TOKEN}");
        assert!(authorized(&headers(&[&valid]), TOKEN));
        assert!(authorized(&headers(&[&format!("bearer {TOKEN}")]), TOKEN));

        let wrong_case = format!("Bearer {}", TOKEN.to_uppercase());
        let prefix = format!("Bearer {}", &TOKEN[..31]);
        let extended = format!("Bearer {TOKEN}x");
        let double_space = format!("Bearer  {TOKEN}");
        let basic = format!("Basic {TOKEN}");
        for value in [
            wrong_case.as_str(),
            prefix.as_str(),
            extended.as_str(),
            double_space.as_str(),
            basic.as_str(),
            TOKEN,
            "Bearer",
            "",
        ] {
            assert!(!authorized(&headers(&[value]), TOKEN), "{value:?}");
        }
        assert!(!authorized(&headers(&[]), TOKEN));
        assert!(!authorized(&headers(&[&valid, &valid]), TOKEN));
    }

    #[test]
    fn startup_refuses_tokens_that_are_unset_short_or_unusable_in_a_header() {
        assert_eq!(&*load_token(Some(OsStr::new(TOKEN))).unwrap(), TOKEN);
        let spaced = format!("{TOKEN} x");
        let accented = format!("{TOKEN}é");
        for value in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new(&TOKEN[..31])),
            Some(OsStr::new(&spaced)),
            Some(OsStr::new(&accented)),
        ] {
            assert!(load_token(value).is_err(), "{value:?}");
        }
    }
}
