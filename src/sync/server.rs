use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use aven_core::sync::seed_claim::Secret;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tracing::info;

use crate::cli::{ServerArgs, ServerSetupArgs, ServerSubcommand};
use crate::config;
use crate::signals::shutdown_signal;

/// Setup invitations stay usable for one hour, or until a device claims storage.
const SETUP_INVITATION_SECONDS: u64 = 3600;

/// Open connections, including idle keep-alive ones. Further clients wait in
/// the listen backlog.
const MAX_CONNECTIONS: usize = 256;
/// Longest a connection may take to send one request's headers.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest graceful shutdown waits for in-flight requests.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

const UNPREPARED_STORAGE: &str =
    "error server-storage-unprepared hint=\"run `aven server setup --data PATH --url URL` first\"";
const INVALID_MEMBERSHIP: &str = "error server-membership-invalid hint=\"stored device membership failed verification; restore this path from a backup or prepare a new one with `aven server setup`\"";
const UNSUPPORTED_STORAGE: &str = "error server-storage-unsupported hint=\"this storage holds unencrypted sync history, which is no longer supported; prepare a new path with `aven server setup`\"";

pub(crate) async fn run_server(args: ServerArgs, config: config::AppConfig) -> Result<()> {
    if let Some(ServerSubcommand::Setup(setup)) = args.command {
        return setup_server(setup).await;
    }
    let data = args.data.context("error server-data-required")?;
    serve(args.bind, &data, &config).await
}

async fn setup_server(args: ServerSetupArgs) -> Result<()> {
    let server = super::encrypted::server_origin(&args.url)?;
    let database = Database::open(&args.data).await?;
    let mut fresh_id = [0; 32];
    getrandom::fill(&mut fresh_id).map_err(|_| anyhow::anyhow!("error server-setup-entropy"))?;
    let secret = Secret::generate()?;
    let setup_id = database
        .issue_e2ee_server_setup(
            &secret,
            fresh_id,
            super::encrypted::unix_now()? + SETUP_INVITATION_SECONDS,
        )
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error e2ee-server-storage-not-empty" => error.context(UNSUPPORTED_STORAGE),
            _ => error,
        })?;
    let invitation = super::encrypted::SetupInvitation {
        server,
        setup_id,
        secret,
    };
    println!("{}", invitation.encode().as_str());
    eprintln!("Anyone with this invitation can claim this server. It expires in one hour.");
    eprintln!("Run `aven sync setup` on the device whose data should start the sync.");
    let port = url::Url::parse(&args.url)
        .ok()
        .and_then(|url| url.port_or_known_default())
        .unwrap_or(3746);
    eprintln!(
        "Then start the server: aven server --data {} --bind 127.0.0.1:{port}",
        args.data.display()
    );
    Ok(())
}

/// Serves the seed, enrollment and tail/image routers. Each operation
/// authenticates against the stored vault; TLS belongs in a reverse proxy, so
/// only loopback binds are accepted.
async fn serve(bind: SocketAddr, data: &Path, config: &config::AppConfig) -> Result<()> {
    if !bind.ip().is_loopback() {
        bail!("error server-bind-loopback hint=\"bind 127.0.0.1 behind a TLS reverse proxy\"");
    }
    if !data.exists() {
        bail!(UNPREPARED_STORAGE);
    }
    let database = Database::open(data).await?;
    if !database.is_e2ee_server_storage().await? {
        if database.has_change_history().await? {
            bail!(UNSUPPORTED_STORAGE);
        }
        bail!(UNPREPARED_STORAGE);
    }
    database
        .verify_membership_history()
        .await
        .map_err(|error| error.context(INVALID_MEMBERSHIP))?;
    let app = crate::seed_bootstrap_http::router(database.clone(), None, Default::default())
        .merge(crate::peer_enrollment_http::router(database.clone()))
        .merge(crate::encrypted_tail_http::router_with_policy(
            database,
            config.local.attachment_lifecycle.server_policy(),
        ));
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    info!(bind = %addr, "sync server starting");
    println!("listening url=http://{addr}");
    serve_connections(listener, app, MAX_CONNECTIONS, shutdown_signal()).await
}

/// Serves HTTP/1 with bounded connections and a header deadline, so a
/// stalled client can't hold server resources without limit.
async fn serve_connections(
    listener: TcpListener,
    app: axum::Router,
    max_connections: usize,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    let connections = Arc::new(Semaphore::new(max_connections));
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let permit = tokio::select! {
            permit = connections.clone().acquire_owned() => permit?,
            () = &mut shutdown => break,
        };
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => {
                    // Usually descriptor exhaustion; back off instead of spinning.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let connection = hyper::server::conn::http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(HEADER_TIMEOUT)
            .serve_connection(TokioIo::new(stream), TowerToHyperService::new(app.clone()));
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            let _ = connection.await;
            drop(permit);
        });
    }
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, graceful.shutdown()).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn read_to_end(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
            .await
            .expect("the server answers or closes")
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn incomplete_headers_time_out_and_release_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "served" }));
        let server = tokio::spawn(serve_connections(listener, app, 1, std::future::pending()));

        // The only connection slot holds a request whose headers never finish.
        let mut stalled = TcpStream::connect(address).await.unwrap();
        stalled
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .unwrap();
        let mut waiting = TcpStream::connect(address).await.unwrap();
        waiting
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        // Let the server start reading the stalled headers before time moves.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut byte = [0; 1];
        assert!(
            tokio::time::timeout(Duration::from_millis(100), waiting.read(&mut byte))
                .await
                .is_err(),
            "a second connection waits while the only slot is held"
        );

        tokio::time::pause();
        tokio::time::advance(HEADER_TIMEOUT).await;
        tokio::time::resume();
        let closed = read_to_end(&mut stalled).await;
        assert!(!closed.contains("served"), "{closed}");
        let response = read_to_end(&mut waiting).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("served"), "{response}");
        server.abort();
    }
}
