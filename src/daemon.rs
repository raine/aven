use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use aven_core::db::Database;
use aven_core::sync::client::errors::is_access_refusal;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::time::{Instant, sleep_until};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use tracing::{debug, info, warn};

use crate::config::AppConfig;
use crate::signals::shutdown_signal;
use crate::sync::encrypted::{self, DaemonRound};
use aven_host::sync::scheduler::{ROUND_BUDGET, SyncScheduler};

use aven_host::daemon::service;

pub(crate) use aven_host::daemon::wake_if_enabled;
pub use service::{
    InstalledService, ServiceInstallArgs, ServiceRepairArgs, install, reload_service_definition,
    repair, restart, status_snapshot, uninstall,
};

/// Short, because an update waits for the daemon to notice its replaced
/// executable and exit before the new version opens the database.
const BINARY_CHECK_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
struct BinaryFingerprint {
    /// Path the daemon was started through, resolved again on every check so
    /// a package manager repointing a symlink counts as a change.
    launch_path: PathBuf,
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
    args.config.ensure_daemon_has_work()?;
    let wake_addr = args.config.wake_addr()?;
    #[cfg(target_os = "macos")]
    if move_service_into_bundle().await {
        return Ok(());
    }
    let database = Database::open(&args.db_path).await?;
    let socket = bind_wake_socket(wake_addr).await?;
    println!(
        "daemon db={} wake={} sync={} reminders={}",
        args.db_path.display(),
        wake_addr,
        args.config.automatic_sync_is_enabled(),
        args.config.reminders.enabled
    );
    let blob_dir = crate::config::resolve_blob_dir(&args.db_path, &args.config)?;
    let binary_fingerprint = current_binary_fingerprint()?;
    run_loop(
        database,
        &args.config,
        socket,
        &Notify::new(),
        shutdown_signal(),
        blob_dir,
        Some(binary_fingerprint),
    )
    .await
}

async fn bind_wake_socket(wake_addr: SocketAddr) -> Result<UdpSocket> {
    UdpSocket::bind(wake_addr).await.with_context(|| {
        format!(
            "could not bind daemon wake address {wake_addr}; is `aven daemon` or `aven mcp --sync` already using it?"
        )
    })
}

/// A sync worker owned by another long-running process. It keeps the daemon's
/// schedule and wake address, and leaves reminders and restarts on a changed
/// executable to the daemon.
pub(crate) struct EmbeddedSync {
    database: Database,
    config: AppConfig,
    socket: UdpSocket,
    wake: Arc<Notify>,
    blob_dir: PathBuf,
}

impl EmbeddedSync {
    /// Claims the wake address, so a failure surfaces before the owner starts
    /// serving. Requesting a worker enables sync for it regardless of
    /// `sync.enabled`; `AVEN_SYNC_DISABLED` still refuses.
    pub(crate) async fn prepare(
        database: Database,
        config: &AppConfig,
        blob_dir: PathBuf,
    ) -> Result<Self> {
        config.ensure_sync_allowed()?;
        let socket = bind_wake_socket(config.wake_addr()?).await?;
        let mut config = config.clone();
        config.sync.enabled = true;
        config.reminders.enabled = false;
        Ok(Self {
            database,
            config,
            socket,
            wake: Arc::new(Notify::new()),
            blob_dir,
        })
    }

    /// Notifying this requests a round. A notification sent during a round is
    /// kept and starts the next one.
    pub(crate) fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    pub(crate) async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        run_loop(
            self.database,
            &self.config,
            self.socket,
            &self.wake,
            shutdown,
            self.blob_dir,
            None,
        )
        .await
    }
}

async fn run_loop(
    database: Database,
    config: &AppConfig,
    socket: UdpSocket,
    direct_wake: &Notify,
    shutdown: impl Future<Output = ()>,
    blob_dir: PathBuf,
    binary_fingerprint: Option<BinaryFingerprint>,
) -> Result<()> {
    let interval_seconds = config.sync_interval_seconds();
    let lifecycle_policy = config.local.attachment_lifecycle.policy();
    // Outlives each iteration, so a signal during a sync round still ends the
    // loop once that round returns.
    let mut shutdown = std::pin::pin!(shutdown);
    let sync_on = config.automatic_sync_is_enabled();
    let reminders_on = config.reminders.enabled;
    let mut next_reminder = Instant::now();
    let mut wake_buf = [0_u8; 16];
    let mut scheduler = SyncScheduler::new(Duration::from_secs(interval_seconds), Instant::now());
    let mut next_attachment_maintenance = Instant::now();
    let mut next_binary_check = Instant::now() + BINARY_CHECK_INTERVAL;
    let mut sync = encrypted::DaemonSync::default();
    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("daemon shutting down");
                break;
            }
            retry_now = next_wake(&socket, direct_wake, &mut wake_buf) => {
                let now = Instant::now();
                next_reminder = now;
                scheduler.wake(retry_now, now);
            }
            _ = sleep_until(next_binary_check), if binary_fingerprint.is_some() => {
                if let Some(fingerprint) = &binary_fingerprint
                    && binary_changed(fingerprint)?
                {
                    println!("daemon-executable-changed path={}", fingerprint.path.display());
                    break;
                }
                next_binary_check = Instant::now() + BINARY_CHECK_INTERVAL;
            }
            // Reminders wait while sync rounds have more work, so they never
            // judge a partly synced database.
            _ = sleep_until(next_reminder), if reminders_on && !scheduler.catching_up() => {
                next_reminder = Instant::now() + crate::reminders::remind(&database, &config.reminders).await;
            }
            _ = sleep_until(next_attachment_maintenance), if sync_on => {
                maintain_attachments(&database, &blob_dir, lifecycle_policy).await;
                next_attachment_maintenance =
                    Instant::now() + crate::sync::ATTACHMENT_MAINTENANCE_INTERVAL;
            }
            _ = sleep_until(scheduler.next_sync()), if sync_on => {
                scheduler.round_started();
                match sync_once(&database, config, &mut sync).await {
                    Ok(DaemonRound::Completed(outcome)) => {
                        if let Err(error) = database.record_daemon_sync_round(chrono::Utc::now().timestamp(), None).await {
                            warn!(%error, "could not persist daemon sync health");
                        }
                        if scheduler.completed(outcome.more_work_ready(), Instant::now()) {
                            next_reminder = Instant::now();
                        }
                    }
                    Ok(DaemonRound::NotSetUp) => {
                        if scheduler.not_set_up(Instant::now()) {
                            println!("daemon-sync-not-set-up hint=\"run `aven sync setup` or `aven sync join`\"");
                        }
                    }
                    Ok(DaemonRound::Deferred) => {
                        debug!("daemon sync deferred");
                        scheduler.deferred(Instant::now());
                    }
                    Ok(DaemonRound::VaultDeleted) => {
                        if scheduler.vault_deleted(Instant::now()) {
                            println!("daemon-sync-vault-deleted hint=\"this sync's vault was deleted; run `aven sync reset`, then `aven sync setup` with a new setup code\"");
                        }
                    }
                    Err(err) if is_access_refusal(&err) => {
                        if scheduler.access_refused(Instant::now()) {
                            warn!(error = %err, "daemon sync refused by server");
                        }
                        record_failure(&database, &err, interval_seconds).await;
                    }
                    Err(err) => {
                        let retry_seconds = scheduler.failed(Instant::now());
                        record_failure(&database, &err, retry_seconds).await;
                        warn!(error = %daemon_error(&err), retry_seconds, "daemon sync failed");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Points the LaunchAgent at Aven.app's executable when it still runs it
/// through a symlink, then has a separate process reload the service. Returns
/// true when launchd stopped this daemon for the reload.
#[cfg(target_os = "macos")]
async fn move_service_into_bundle() -> bool {
    use std::os::unix::process::CommandExt as _;

    let program = match service::prepare_bundle_migration() {
        Ok(service::BundleMigration::Prepared { program }) => program,
        Ok(service::BundleMigration::NotNeeded) => return false,
        Ok(service::BundleMigration::Skipped(reason)) => {
            println!("daemon-bundle-migration-skipped reason={reason}");
            return false;
        }
        Err(error) => {
            warn!(error = %format!("{error:#}"), "daemon bundle migration failed");
            return false;
        }
    };
    println!("daemon-bundle-migration program={}", program.display());
    let mut command = std::process::Command::new(&program);
    command
        .args(["internal", "reload-daemon"])
        .stdin(std::process::Stdio::null());
    // Bootout ends every process in the job's process group, including a
    // reload started from it, so the reload runs in its own session.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match tokio::process::Command::from(command).spawn() {
        Ok(child) => child,
        Err(error) => {
            warn!(error = %error, "daemon reload could not start");
            return false;
        }
    };
    // If the reload fails before stopping this daemon, keep running on the
    // loaded definition; the rewritten plist applies at the next login.
    tokio::select! {
        _ = shutdown_signal() => true,
        status = child.wait() => {
            warn!(status = ?status, "daemon reload exited without stopping the daemon");
            false
        }
    }
}

fn daemon_error(error: &anyhow::Error) -> String {
    match crate::sync::error_explanations::explain(
        crate::sync::error_explanations::ErrorAction::General,
        crate::sync::error_explanations::ErrorSurface::Cli,
        error,
    ) {
        Some(explanation) => format!(
            "{} [{}] Next: {} Cause: {error:#}",
            explanation.message, explanation.code, explanation.next_step
        ),
        None => error.to_string(),
    }
}

fn current_binary_fingerprint() -> Result<BinaryFingerprint> {
    let current = std::env::current_exe().context("resolve current executable")?;
    let launch_path = std::env::args_os()
        .next()
        .map(PathBuf::from)
        .filter(|argv0| argv0.is_absolute() && same_file(argv0, &current))
        .unwrap_or(current);
    binary_fingerprint(&launch_path)
}

fn same_file(left: &Path, right: &Path) -> bool {
    matches!((left.canonicalize(), right.canonicalize()), (Ok(left), Ok(right)) if left == right)
}

fn binary_changed(initial: &BinaryFingerprint) -> Result<bool> {
    Ok(binary_fingerprint(&initial.launch_path)? != *initial)
}

fn binary_fingerprint(launch_path: &Path) -> Result<BinaryFingerprint> {
    let path = launch_path
        .canonicalize()
        .unwrap_or_else(|_| launch_path.to_path_buf());
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("read executable metadata {}", path.display()))?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    Ok(BinaryFingerprint {
        launch_path: launch_path.to_path_buf(),
        path,
        len: metadata.len(),
        modified_ns,
        #[cfg(unix)]
        dev: metadata.dev(),
        #[cfg(unix)]
        ino: metadata.ino(),
    })
}

/// Waits for a wake from another process or from this one, and returns whether
/// it may bypass failure backoff.
async fn next_wake(socket: &UdpSocket, direct_wake: &Notify, wake_buf: &mut [u8]) -> bool {
    tokio::select! {
        () = direct_wake.notified() => {
            debug!("daemon wake notified");
            false
        }
        result = socket.recv_from(wake_buf) => {
            let mut retry_now = false;
            if let Ok((len, _)) = &result {
                retry_now = &wake_buf[..*len] == b"r";
            }
            if let Err(err) = result {
                warn!(error = %err, "daemon wake receive failed");
            } else {
                debug!("daemon wake received");
            }
            retry_now | drain_wakes(socket, wake_buf)
        }
    }
}

fn drain_wakes(socket: &UdpSocket, wake_buf: &mut [u8]) -> bool {
    let mut retry_now = false;
    while let Ok((len, _)) = socket.try_recv_from(wake_buf) {
        retry_now |= &wake_buf[..len] == b"r";
    }
    retry_now
}

async fn record_failure(database: &Database, error: &anyhow::Error, retry_seconds: u64) {
    use crate::sync::error_explanations::{ErrorAction, ErrorSurface, explain};
    let explanation = explain(ErrorAction::General, ErrorSurface::Tui, error);
    let code = explanation
        .map(|explanation| explanation.code.to_string())
        .or_else(|| crate::sync::error_explanations::codes(error).last())
        .unwrap_or_else(|| "daemon-sync-failed".to_string());
    // Persist actionable explanations, not raw chains that may contain URLs or credentials.
    let message = explanation.map_or_else(
        || "Background sync failed. Choose Sync now to retry and see the cause.".to_string(),
        |explanation| explanation.combined(),
    );
    let now = chrono::Utc::now().timestamp();
    if let Err(error) = database
        .record_daemon_sync_round(now, Some((&code, &message, now + retry_seconds as i64)))
        .await
    {
        warn!(%error, "could not persist daemon sync health");
    }
}

async fn sync_once(
    database: &Database,
    config: &AppConfig,
    sync: &mut encrypted::DaemonSync,
) -> Result<DaemonRound> {
    let round = sync.round(database, config, ROUND_BUDGET).await?;
    match &round {
        DaemonRound::Completed(outcome) => {
            println!(
                "daemon-synced rounds={} metadata_caught_up={} images={}",
                outcome.rounds, outcome.metadata_caught_up, outcome.images
            );
        }
        DaemonRound::NotSetUp | DaemonRound::Deferred | DaemonRound::VaultDeleted => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn draining_preserves_retry_wakes_among_ordinary_wakes() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        for payload in [b"1", b"r", b"1"] {
            sender.send_to(payload, addr).await.unwrap();
        }
        socket.readable().await.unwrap();
        assert!(drain_wakes(&socket, &mut [0; 16]));
        assert!(!drain_wakes(&socket, &mut [0; 16]));
    }

    #[tokio::test]
    async fn failure_records_codes_and_explanations_without_raw_details() {
        let database = Database::open(Path::new(":memory:")).await.unwrap();
        record_failure(
            &database,
            &anyhow::anyhow!("error encrypted-tail-reply url=secret"),
            1,
        )
        .await;
        let state = database.daemon_sync_state().await.unwrap();
        let error = state.error.unwrap();
        assert_eq!(error.code, "encrypted-tail-reply");
        assert!(!error.message.contains("secret"));
        assert_eq!(state.next_retry_at, Some(error.since + 1));
    }

    #[test]
    fn protected_storage_daemon_errors_are_actionable() {
        let error = anyhow::Error::new(
            crate::protected_local_keys::ProtectedLocalKeyStoreError::new(
                crate::protected_local_keys::ProtectedLocalKeyStoreErrorKind::MissingAuthority,
            ),
        );
        let message = daemon_error(&error);
        assert!(message.contains("protected-key-storage-missing"));
        assert!(message.contains("Do not replace them with new keys"));
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

    #[cfg(unix)]
    #[test]
    fn binary_fingerprint_changes_when_launch_symlink_is_repointed() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("0.1.0");
        let new = dir.path().join("0.2.0");
        std::fs::write(&old, "same").unwrap();
        std::fs::write(&new, "same").unwrap();
        let link = dir.path().join("aven");
        std::os::unix::fs::symlink(&old, &link).unwrap();
        let initial = binary_fingerprint(&link).unwrap();
        assert!(!binary_changed(&initial).unwrap());

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&new, &link).unwrap();
        assert!(old.exists());
        assert!(binary_changed(&initial).unwrap());
    }
}
