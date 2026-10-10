use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use aven_core::db::Database;
use aven_core::sync::seed_claim::{Secret, StorageNotEmpty};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::cli::{ServerArgs, ServerSetupArgs, ServerSubcommand};
use crate::config;
use crate::signals::shutdown_signal;

/// Setup invitations stay usable for one hour, or until a device claims storage.
const SETUP_INVITATION_SECONDS: u64 = 3600;

/// Open connections, including idle keep-alive ones. Further clients wait in
/// the listen backlog.
const MAX_CONNECTIONS: usize = 256;
/// How often the server prunes unreferenced encrypted images.
const IMAGE_PRUNE_INTERVAL: Duration = Duration::from_secs(600);
/// Longest a connection may take to send one request's headers.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest graceful shutdown waits for in-flight requests.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

const UNPREPARED_STORAGE: &str =
    "error server-storage-unprepared hint=\"run `aven server setup --url URL` first\"";
const INVALID_MEMBERSHIP: &str = "error server-membership-invalid hint=\"stored device membership failed verification; restore this path from a backup or prepare a new one with `aven server setup`\"";
const UNSUPPORTED_STORAGE: &str = "error server-storage-unsupported hint=\"this storage holds unencrypted sync history, which is no longer supported; prepare a new path with `aven server setup`\"";

pub(crate) async fn run_server(args: ServerArgs, config: config::AppConfig) -> Result<()> {
    if let Some(ServerSubcommand::Setup(setup)) = args.command {
        return setup_server(setup).await;
    }
    let data = server_data_path(args.data)?;
    serve(args.bind, args.unsafe_public_bind, &data, &config).await
}

fn server_data_path(flag: Option<PathBuf>) -> Result<PathBuf> {
    match flag {
        Some(path) => Ok(path),
        None => config::default_server_data_path(),
    }
}

async fn setup_server(args: ServerSetupArgs) -> Result<()> {
    let server = super::encrypted::server_origin(&args.url)?;
    let explicit_data = args.data.is_some();
    let data = server_data_path(args.data)?;
    let database = Database::open(&data).await?;
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
        .map_err(|error| {
            if error.is::<StorageNotEmpty>() {
                error.context(UNSUPPORTED_STORAGE)
            } else {
                error
            }
        })?;
    let invitation = super::encrypted::SetupInvitation {
        server,
        setup_id,
        secret,
    };
    let invitation = invitation.encode()?;
    if args.invitation_only {
        println!("{}", invitation.as_str());
        return Ok(());
    }

    println!("================ SETUP INVITATION ================");
    println!("{}", invitation.as_str());
    println!("==================================================");
    println!("Keep this invitation private. It expires in one hour.");
    println!("Anyone with it can claim this server.");
    println!("Run `aven sync setup` on the device whose data should start the sync.");
    println!("Server storage: {}", data.display());
    let same_data = if explicit_data {
        " Pass the same --data path to aven server."
    } else {
        ""
    };
    println!(
        "Then start the server on this storage so it's reachable at {}.{same_data}",
        args.url
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BindScope {
    Loopback,
    Private,
    Public,
}

impl BindScope {
    fn classify(address: IpAddr) -> Self {
        if address.is_loopback() {
            Self::Loopback
        } else if is_private_address(address) {
            Self::Private
        } else {
            Self::Public
        }
    }
}

fn is_private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            address.is_private()
                || address.is_link_local()
                || octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(address) => address.is_unique_local() || address.is_unicast_link_local(),
    }
}

/// Serves the seed, enrollment and tail/image routers. Each operation
/// authenticates against the stored vault. The server does not terminate TLS,
/// so non-loopback binds need an independently protected network.
async fn serve(
    bind: SocketAddr,
    unsafe_public_bind: bool,
    data: &Path,
    config: &config::AppConfig,
) -> Result<()> {
    let scope = BindScope::classify(bind.ip());
    if scope == BindScope::Public && !unsafe_public_bind {
        bail!(
            "error public-bind-requires hint=\"bind a loopback or private VPN address, or pass --unsafe-public-bind for a public or wildcard address\""
        );
    }
    if scope == BindScope::Public {
        eprintln!(
            "Warning: public or wildcard bind {bind} enabled without TLS. Device credentials and setup invitations are not protected by payload encryption."
        );
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
    let image_policy = config.local.attachment_lifecycle.server_policy();
    let publication_policy = aven_core::sync::bootstrap_staging::PublicationPolicy {
        workspace_quota_bytes: u64::try_from(image_policy.quota_bytes).unwrap_or(0),
    };
    tokio::spawn(prune_images(database.clone(), image_policy.grace));
    let app = crate::seed_bootstrap_http::router(database.clone(), publication_policy)
        .merge(crate::peer_enrollment_http::router(database.clone()))
        .merge(crate::encrypted_tail_http::router(database, image_policy));
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    info!(bind = %addr, "sync server starting");
    println!("listening url=http://{addr}");
    serve_connections(listener, app, MAX_CONNECTIONS, shutdown_signal()).await
}

/// Periodically deletes the bytes of encrypted images that no live task
/// references, in small batches so request writers are not starved.
async fn prune_images(database: Database, grace: Duration) {
    const BATCH: usize = 128;
    let mut interval = tokio::time::interval(IMAGE_PRUNE_INTERVAL);
    loop {
        interval.tick().await;
        loop {
            match database.prune_encrypted_images(grace, BATCH).await {
                Ok(pruned) if pruned == BATCH => tokio::task::yield_now().await,
                Ok(_) => break,
                Err(error) => {
                    warn!(error = %error, "encrypted image prune failed");
                    break;
                }
            }
        }
    }
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
    use super::BindScope;
    use std::net::IpAddr;

    #[test]
    fn classifies_private_and_vpn_bind_addresses() {
        for address in ["127.0.0.1", "::1"] {
            assert_eq!(
                BindScope::classify(address.parse::<IpAddr>().unwrap()),
                BindScope::Loopback
            );
        }
        for address in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "169.254.1.1",
            "fd00::1",
            "fe80::1",
        ] {
            assert_eq!(
                BindScope::classify(address.parse::<IpAddr>().unwrap()),
                BindScope::Private
            );
        }
        for address in [
            "0.0.0.0",
            "::",
            "1.1.1.1",
            "100.128.0.1",
            "2606:4700:4700::1111",
        ] {
            assert_eq!(
                BindScope::classify(address.parse::<IpAddr>().unwrap()),
                BindScope::Public
            );
        }
    }

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
