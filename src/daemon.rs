use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use tokio::net::UdpSocket;
use tokio::time::{Instant, sleep_until, timeout};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use tracing::{debug, info, warn};

use crate::config::AppConfig;
use crate::db::open_db;
use crate::signals::shutdown_signal;
use crate::sync::SyncHttpClient;
use crate::sync::wire::{DAEMON_INCOMPLETE_RESCHEDULE_MS, DAEMON_SYNC_PAGE_BUDGET};

mod service;

pub use service::{
    ServiceInstallArgs, ServiceRepairArgs, install, repair, restart, status_snapshot, uninstall,
};

const BINARY_CHECK_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
struct BinaryFingerprint {
    path: PathBuf,
    len: u64,
    modified_ns: Option<u128>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

pub struct DaemonRunArgs {
    pub db_path: PathBuf,
    pub config: AppConfig,
}

pub async fn run(args: DaemonRunArgs) -> Result<()> {
    if !args.config.sync.enabled {
        bail!("error sync-disabled hint=\"set sync.enabled = true in config.yaml\"");
    }
    let server = args
        .config
        .sync
        .server_url
        .clone()
        .context("error sync-server-required hint=\"set sync.server_url in config.yaml\"")?;
    let wake_addr = args.config.wake_addr()?;
    let interval_seconds = args.config.sync_interval_seconds();
    let blob_dir = crate::config::resolve_blob_dir(&args.db_path, &args.config)?;
    let pool = open_db(&args.db_path).await?;
    let socket = UdpSocket::bind(wake_addr).await.with_context(|| {
        format!("could not bind daemon wake address {wake_addr}; is another daemon running?")
    })?;
    info!(
        db = %args.db_path.display(),
        server = %server,
        wake_addr = %wake_addr,
        interval_seconds,
        "daemon starting"
    );
    println!(
        "daemon db={} server={} wake={}",
        args.db_path.display(),
        server,
        wake_addr
    );

    let binary_fingerprint = current_binary_fingerprint()?;
    let client = SyncHttpClient::new().context("build daemon sync HTTP client")?;
    info!(server = %server, http_client_id = %client.id(), "daemon sync client ready");
    run_loop(DaemonLoop {
        pool,
        blob_dir,
        lifecycle_policy: args.config.local.attachment_lifecycle.policy(),
        server,
        socket,
        interval_seconds,
        auth_token: args.config.sync_auth_token().map(str::to_string),
        client,
        binary_fingerprint,
    })
    .await
}

struct DaemonLoop {
    pool: SqlitePool,
    blob_dir: PathBuf,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
    server: String,
    socket: UdpSocket,
    interval_seconds: u64,
    auth_token: Option<String>,
    client: SyncHttpClient,
    binary_fingerprint: BinaryFingerprint,
}

async fn run_loop(args: DaemonLoop) -> Result<()> {
    let DaemonLoop {
        pool,
        blob_dir,
        lifecycle_policy,
        server,
        socket,
        interval_seconds,
        auth_token,
        client,
        binary_fingerprint,
    } = args;
    let mut wake_buf = [0_u8; 16];
    let mut backoff_seconds = 1_u64;
    let mut next_sync = Instant::now();
    let mut next_binary_check = Instant::now() + BINARY_CHECK_INTERVAL;
    loop {
        tokio::select! {
            _ = shutdown_signal() => {
                info!("daemon shutting down");
                break;
            }
            result = socket.recv_from(&mut wake_buf) => {
                if let Err(err) = result {
                    warn!(error = %err, "daemon wake receive failed");
                    eprintln!("daemon wake failed: {err}");
                } else {
                    debug!("daemon wake received");
                }
                drain_wakes(&socket, &mut wake_buf);
                next_sync = Instant::now();
            }
            _ = sleep_until(next_binary_check) => {
                if binary_changed(&binary_fingerprint)? {
                    info!(path = %binary_fingerprint.path.display(), "daemon executable changed");
                    println!("daemon-executable-changed path={}", binary_fingerprint.path.display());
                    break;
                }
                next_binary_check = Instant::now() + BINARY_CHECK_INTERVAL;
            }
            _ = sleep_until(next_sync) => {
                match timeout(
                    Duration::from_secs(35),
                    sync_once(
                        &pool,
                        &blob_dir,
                        lifecycle_policy,
                        &server,
                        auth_token.as_deref(),
                        &client,
                    ),
                )
                .await
                {
                    Ok(Ok(summary)) => {
                        backoff_seconds = 1;
                        next_sync = if summary.complete {
                            Instant::now() + Duration::from_secs(interval_seconds)
                        } else {
                            Instant::now() + Duration::from_millis(DAEMON_INCOMPLETE_RESCHEDULE_MS)
                        };
                    }
                    Ok(Err(err)) => {
                        warn!(error = %err, backoff_seconds, "daemon sync failed");
                        eprintln!("daemon sync failed: {err}");
                        next_sync = Instant::now() + Duration::from_secs(backoff_seconds);
                        backoff_seconds = (backoff_seconds * 2).min(300);
                    }
                    Err(_) => {
                        warn!(backoff_seconds, "daemon sync timed out");
                        eprintln!("daemon sync failed: timed out");
                        next_sync = Instant::now() + Duration::from_secs(backoff_seconds);
                        backoff_seconds = (backoff_seconds * 2).min(300);
                    }
                }
            }
        }
    }
    Ok(())
}

fn current_binary_fingerprint() -> Result<BinaryFingerprint> {
    let path = std::env::current_exe().context("resolve current executable")?;
    binary_fingerprint(&path)
}

fn binary_changed(initial: &BinaryFingerprint) -> Result<bool> {
    Ok(binary_fingerprint(&initial.path)? != *initial)
}

fn binary_fingerprint(path: &Path) -> Result<BinaryFingerprint> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("read executable metadata {}", path.display()))?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    Ok(BinaryFingerprint {
        path,
        len: metadata.len(),
        modified_ns,
        #[cfg(unix)]
        dev: metadata.dev(),
        #[cfg(unix)]
        ino: metadata.ino(),
    })
}

fn drain_wakes(socket: &UdpSocket, wake_buf: &mut [u8]) {
    while socket.try_recv_from(wake_buf).is_ok() {}
}

async fn sync_once(
    pool: &SqlitePool,
    blob_dir: &Path,
    lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
    server: &str,
    auth_token: Option<&str>,
    client: &SyncHttpClient,
) -> Result<crate::sync::SyncSummary> {
    let mut conn = pool.acquire().await?;
    let summary = crate::sync::run_sync_with_page_budget_using_client_and_policy(
        &mut conn,
        blob_dir,
        server,
        auth_token,
        Some(DAEMON_SYNC_PAGE_BUDGET),
        client,
        lifecycle_policy,
    )
    .await?;
    if let Err(err) = crate::attachments::lifecycle::prune(
        &mut conn,
        blob_dir,
        lifecycle_policy,
        true,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await
    {
        warn!(error = %err, "attachment maintenance failed");
    }
    info!(
        pushed = summary.pushed,
        pulled = summary.pulled,
        blob_uploaded = summary.blob_uploaded,
        blob_downloaded = summary.blob_downloaded,
        cursor = summary.cursor,
        complete = summary.complete,
        pages = summary.pages,
        request_bytes = summary.request_bytes,
        request_wire_bytes = summary.request_wire_bytes,
        response_decoded_bytes = summary.response_decoded_bytes,
        response_compression = summary.response_compression,
        apply_ms = summary.apply_ms,
        "daemon sync completed"
    );
    println!(
        "daemon-synced pushed={} pulled={} blob_uploaded={} blob_downloaded={} cursor={} complete={} pages={}",
        summary.pushed,
        summary.pulled,
        summary.blob_uploaded,
        summary.blob_downloaded,
        summary.cursor,
        summary.complete,
        summary.pages
    );
    Ok(summary)
}

pub(crate) fn wake_if_enabled(config: &AppConfig) {
    if !config.sync.enabled {
        return;
    }
    let Ok(addr) = config.wake_addr() else {
        return;
    };
    debug!(wake_addr = %addr, "waking daemon after local mutation");
    wake(addr);
}

fn wake(addr: SocketAddr) {
    let bind_addr = SocketAddr::new(addr.ip(), 0);
    match std::net::UdpSocket::bind(bind_addr).and_then(|socket| socket.send_to(b"1", addr)) {
        Ok(_) => debug!(wake_addr = %addr, "daemon wake sent"),
        Err(err) => warn!(wake_addr = %addr, error = %err, "daemon wake send failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_if_enabled_sends_to_configured_address() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        config.daemon.wake_addr = Some(socket.local_addr().unwrap().to_string());

        wake_if_enabled(&config);

        let mut buf = [0_u8; 1];
        assert_eq!(socket.recv(&mut buf).unwrap(), 1);
        assert_eq!(buf, [b'1']);
    }

    #[test]
    fn wake_if_enabled_skips_when_sync_is_disabled() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(25)))
            .unwrap();
        let mut config = AppConfig::default();
        config.daemon.wake_addr = Some(socket.local_addr().unwrap().to_string());

        wake_if_enabled(&config);

        let mut buf = [0_u8; 1];
        let error = socket.recv(&mut buf).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn wake_if_enabled_skips_invalid_address() {
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        config.daemon.wake_addr = Some("not-an-address".to_string());

        wake_if_enabled(&config);
    }

    #[test]
    fn binary_fingerprint_changes_when_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aven");
        std::fs::write(&path, "one").unwrap();
        let initial = binary_fingerprint(&path).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(&path, "two-two").unwrap();
        assert!(binary_changed(&initial).unwrap());
    }
}
