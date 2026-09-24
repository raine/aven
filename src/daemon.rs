use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use aven_core::db::Database;
use tokio::net::UdpSocket;
use tokio::time::{Instant, sleep_until};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use tracing::{debug, info, warn};

use crate::config::AppConfig;
use crate::signals::shutdown_signal;
use crate::sync::encrypted::{self, DaemonRound};

mod service;

pub use service::{
    ServiceInstallArgs, ServiceRepairArgs, ServiceStatus, install, repair, restart,
    status_snapshot, uninstall,
};

const BINARY_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const DAEMON_CONTENTION_RESCHEDULE: Duration = Duration::from_secs(1);
/// Bounded sync rounds per daemon wake.
const DAEMON_ROUND_BUDGET: usize = 8;
/// Delay before continuing a wake that stopped at its round budget.
const DAEMON_INCOMPLETE_RESCHEDULE: Duration = Duration::from_millis(100);

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
    args.config.ensure_automatic_sync_enabled()?;
    let wake_addr = args.config.wake_addr()?;
    let interval_seconds = args.config.sync_interval_seconds();
    let database = Database::open(&args.db_path).await?;
    let socket = UdpSocket::bind(wake_addr).await.with_context(|| {
        format!("could not bind daemon wake address {wake_addr}; is another daemon running?")
    })?;
    info!(
        db = %args.db_path.display(),
        wake_addr = %wake_addr,
        interval_seconds,
        "daemon starting"
    );
    println!("daemon db={} wake={}", args.db_path.display(), wake_addr);

    let blob_dir = crate::config::resolve_blob_dir(&args.db_path, &args.config)?;
    let lifecycle_policy = args.config.local.attachment_lifecycle.policy();
    let binary_fingerprint = current_binary_fingerprint()?;
    run_loop(
        database,
        &args.config,
        socket,
        interval_seconds,
        blob_dir,
        lifecycle_policy,
        binary_fingerprint,
    )
    .await
}

async fn run_loop(
    database: Database,
    config: &AppConfig,
    socket: UdpSocket,
    interval_seconds: u64,
    blob_dir: PathBuf,
    lifecycle_policy: aven_core::attachments::LifecyclePolicy,
    binary_fingerprint: BinaryFingerprint,
) -> Result<()> {
    let mut wake_buf = [0_u8; 16];
    let mut backoff_seconds = 1_u64;
    let mut next_sync = Instant::now();
    let mut retry_not_before = None;
    let mut awaiting_setup = false;
    let mut next_attachment_maintenance = Instant::now();
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
                let now = Instant::now();
                if retry_not_before.is_none_or(|deadline| now >= deadline) {
                    next_sync = now;
                }
            }
            _ = sleep_until(next_binary_check) => {
                if binary_changed(&binary_fingerprint)? {
                    info!(path = %binary_fingerprint.path.display(), "daemon executable changed");
                    println!("daemon-executable-changed path={}", binary_fingerprint.path.display());
                    break;
                }
                next_binary_check = Instant::now() + BINARY_CHECK_INTERVAL;
            }
            _ = sleep_until(next_attachment_maintenance) => {
                maintain_attachments(&database, &blob_dir, lifecycle_policy).await;
                next_attachment_maintenance =
                    Instant::now() + crate::sync::ATTACHMENT_MAINTENANCE_INTERVAL;
            }
            _ = sleep_until(next_sync) => {
                retry_not_before = None;
                match sync_once(&database, config).await {
                    Ok(DaemonRound::Completed(outcome)) => {
                        backoff_seconds = 1;
                        awaiting_setup = false;
                        next_sync = if outcome.more_work_ready() {
                            Instant::now() + DAEMON_INCOMPLETE_RESCHEDULE
                        } else {
                            Instant::now() + Duration::from_secs(interval_seconds)
                        };
                    }
                    Ok(DaemonRound::NotSetUp) => {
                        if !awaiting_setup {
                            awaiting_setup = true;
                            info!("daemon sync waiting for setup");
                            println!("daemon-sync-not-set-up hint=\"run `aven sync setup` or `aven sync join`\"");
                        }
                        backoff_seconds = 1;
                        next_sync = Instant::now() + Duration::from_secs(interval_seconds);
                    }
                    Ok(DaemonRound::Deferred) => {
                        debug!("daemon sync deferred");
                        next_sync = Instant::now() + DAEMON_CONTENTION_RESCHEDULE;
                    }
                    Err(err) => {
                        let retry_seconds = backoff_seconds;
                        backoff_seconds = (backoff_seconds * 2).min(300);
                        next_sync = Instant::now() + Duration::from_secs(retry_seconds);
                        retry_not_before = Some(next_sync);
                        warn!(error = %err, retry_seconds, "daemon sync failed");
                        eprintln!("daemon sync failed: {err}");
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

async fn sync_once(database: &Database, config: &AppConfig) -> Result<DaemonRound> {
    let round = encrypted::daemon_round(database, config, DAEMON_ROUND_BUDGET).await?;
    match &round {
        DaemonRound::Completed(outcome) => {
            info!(
                rounds = outcome.rounds,
                metadata_caught_up = outcome.metadata_caught_up,
                images = outcome.images,
                "daemon sync completed"
            );
            println!(
                "daemon-synced rounds={} metadata_caught_up={} images={}",
                outcome.rounds, outcome.metadata_caught_up, outcome.images
            );
        }
        DaemonRound::NotSetUp | DaemonRound::Deferred => {}
    }
    Ok(round)
}

async fn maintain_attachments(
    database: &Database,
    blob_dir: &Path,
    lifecycle_policy: aven_core::attachments::LifecyclePolicy,
) {
    match database
        .prune_attachments(blob_dir, lifecycle_policy, true)
        .await
    {
        Ok(summary) => {
            info!(
                eligible = summary.eligible.count,
                eligible_bytes = summary.eligible.bytes,
                pruned = summary.pruned.count,
                pruned_bytes = summary.pruned.bytes,
                "attachment maintenance completed"
            );
            println!(
                "daemon-maintained eligible={} eligible_bytes={} pruned={} pruned_bytes={}",
                summary.eligible.count,
                summary.eligible.bytes,
                summary.pruned.count,
                summary.pruned.bytes,
            );
        }
        Err(err) => warn!(error = %err, "attachment maintenance failed"),
    }
}

pub(crate) fn wake_if_enabled(config: &AppConfig) {
    if !config.automatic_sync_is_enabled() {
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
        config.sync.enabled = true;
        config.sync.disable_override = true;
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
