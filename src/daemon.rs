//! `aven daemon`: the background host that a service manager starts.
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use aven_core::db::Database;
use aven_host::host::executable::{self, Fingerprint};
use aven_host::host::{self, Delivery, Election, Exit, Host, Role, Seat};
use aven_host::sync::encrypted::DaemonSync;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
#[cfg(target_os = "macos")]
use tracing::warn;

use crate::config::AppConfig;
use crate::signals::shutdown_signal;

use aven_host::daemon::service;

pub(crate) use aven_host::daemon::wake_if_enabled;
pub use service::{
    InstalledService, ServiceInstallArgs, ServiceRepairArgs, install, reload_service_definition,
    repair, restart, status_snapshot, uninstall,
};

/// Longer than an updated app has to confirm its launch.
#[cfg(target_os = "macos")]
const UPDATE_LAUNCH_WAIT: Duration = Duration::from_secs(150);

pub struct DaemonRunArgs {
    pub db_path: PathBuf,
    pub config: AppConfig,
}

/// Runs until stopped, idle while the config gives it nothing to do, so that a
/// service manager that restarts it on exit never loops.
pub async fn run(args: DaemonRunArgs) -> Result<()> {
    let wake_addr = args.config.wake_addr()?;
    #[cfg(target_os = "macos")]
    if move_service_into_bundle().await {
        return Ok(());
    }
    let fingerprint = Fingerprint::current()?;
    #[cfg(target_os = "macos")]
    if !wait_for_update_launch(&fingerprint).await? {
        stop_for_executable(&fingerprint).await;
        return Ok(());
    }
    let mut shutdown = std::pin::pin!(shutdown_signal());
    let Some(seat) = take_seat(&args.db_path, wake_addr, &fingerprint, shutdown.as_mut()).await?
    else {
        return Ok(());
    };
    let database = Database::open(&args.db_path).await?;
    println!(
        "daemon db={} wake={} sync={} reminders={}",
        args.db_path.display(),
        wake_addr,
        args.config.automatic_sync_is_enabled(),
        args.config.reminders.enabled
    );
    let blob_dir = crate::config::resolve_blob_dir(&args.db_path, &args.config)?;
    let host = Host {
        database,
        seat,
        role: Role::Daemon,
        version: crate::update::current_version().to_string(),
        config: follow_config(&args.config, |_| {}),
        blob_dir,
        sync: DaemonSync::default(),
        delivery: Delivery::Here(crate::notification::delivery()),
        events: Box::new(|_| {}),
        wake: Arc::new(Notify::new()),
        debounce: Duration::ZERO,
        executable: Some(fingerprint.clone()),
    };
    if let Exit::ExecutableChanged(_) = host.run(shutdown).await? {
        stop_for_executable(&fingerprint).await;
    }
    Ok(())
}

/// Logs why the daemon stops for its executable, then waits a bounded time
/// for the replacement to be ready before the process exits. The service
/// manager starts a replaced executable again as soon as this one exits; a
/// removed one, as when Aven.app was moved to the Trash, leaves nothing to
/// restart. Waiting keeps launchd from starting an upgraded app before
/// Homebrew has carried the user's approval over to it, which would show a
/// Gatekeeper prompt for the background agent.
async fn stop_for_executable(fingerprint: &Fingerprint) {
    let event = if fingerprint.missing() {
        "daemon-executable-missing"
    } else {
        "daemon-executable-changed"
    };
    println!("{event} path={}", fingerprint.path.display());
    let deadline = Instant::now() + executable::REPLACEMENT_WAIT;
    while !fingerprint.replacement_ready() {
        if Instant::now() >= deadline {
            println!(
                "daemon-replacement-not-ready path={}",
                fingerprint.path.display()
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Follows the config file `config` was loaded from.
fn follow_config(
    config: &AppConfig,
    adjust: impl Fn(&mut AppConfig) + Send + 'static,
) -> watch::Receiver<AppConfig> {
    match crate::config::config_file_path() {
        Ok(path) => host::watch_config_file(path, config.clone(), adjust),
        Err(_) => {
            let mut config = config.clone();
            adjust(&mut config);
            watch::channel(config).1
        }
    }
}

/// Waits until this daemon may host the database. Returns `None` when it
/// should stop instead: on a shutdown signal or a replaced executable.
async fn take_seat(
    db_path: &Path,
    wake_addr: SocketAddr,
    fingerprint: &Fingerprint,
    mut shutdown: Pin<&mut impl Future<Output = ()>>,
) -> Result<Option<Seat>> {
    let mut waiting = None;
    loop {
        let (reason, retry) = match host::elect(db_path, wake_addr).await? {
            Election::Won(seat) => return Ok(Some(seat)),
            Election::Taken => ("host-running", host::LOCK_RETRY),
            Election::WakeAddressBusy(_) => ("wake-address-busy", host::OLD_DAEMON_RETRY),
        };
        if waiting != Some(reason) {
            println!(
                "daemon-waiting reason={reason} lock={} wake={wake_addr}",
                host::lock_path(db_path).display()
            );
            waiting = Some(reason);
        }
        let deadline = Instant::now() + retry;
        while Instant::now() < deadline {
            tokio::select! {
                () = shutdown.as_mut() => return Ok(None),
                () = tokio::time::sleep(executable::CHECK_INTERVAL) => {}
            }
            if fingerprint.changed()? {
                stop_for_executable(fingerprint).await;
                return Ok(None);
            }
        }
    }
}

/// A daemon started from an app that was just updated waits for the new app to
/// confirm its launch before it opens the database, so that a failed update can
/// still be undone. Returns false when the update was undone meanwhile and
/// this executable replaced.
#[cfg(target_os = "macos")]
async fn wait_for_update_launch(fingerprint: &Fingerprint) -> Result<bool> {
    let current = crate::update::current_version();
    let started = Instant::now();
    while crate::update::launch_pending(&fingerprint.path, &current) {
        if fingerprint.changed()? {
            return Ok(false);
        }
        if started.elapsed() > UPDATE_LAUNCH_WAIT {
            warn!("the updated app has not confirmed its launch; starting anyway");
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(!fingerprint.changed()?)
}

/// The host inside another long-running process, such as `aven mcp --sync`.
/// It runs reminders as the daemon would, and keeps running when the
/// executable changes. While another process hosts the database it is that
/// host's client instead, and takes over once that process stops.
pub(crate) struct EmbeddedSync {
    database: Database,
    /// The owner's config, for waking another host.
    config: AppConfig,
    /// What a host started here follows.
    host_config: watch::Receiver<AppConfig>,
    blob_dir: PathBuf,
    wake: Arc<Notify>,
    seat: Option<Seat>,
    /// Why another process hosts, as a log line naming it.
    client: Option<String>,
    retry: Duration,
}

impl EmbeddedSync {
    /// Tries to become the database's host, so a failure surfaces before the
    /// owner starts serving. Requesting a worker enables sync for it regardless
    /// of `sync.enabled`; `AVEN_SYNC_DISABLED` still refuses.
    pub(crate) async fn prepare(
        database: Database,
        config: &AppConfig,
        blob_dir: PathBuf,
    ) -> Result<Self> {
        config.ensure_sync_allowed()?;
        let host_config = follow_config(config, |config| config.sync.enabled = true);
        Self::elect(database, config.clone(), host_config, blob_dir).await
    }

    async fn elect(
        database: Database,
        config: AppConfig,
        host_config: watch::Receiver<AppConfig>,
        blob_dir: PathBuf,
    ) -> Result<Self> {
        let wake_addr = config.wake_addr()?;
        let (seat, client) = match host::elect(database.path(), wake_addr).await? {
            Election::Won(seat) => (Some(seat), None),
            Election::Taken => {
                let host = host::describe_host(&database).await;
                (
                    None,
                    Some(format!("sync-client reason=host-running host=\"{host}\"")),
                )
            }
            Election::WakeAddressBusy(_) => (
                None,
                Some(format!(
                    "sync-client reason=wake-address-busy host=\"an older Aven daemon\" wake={wake_addr}"
                )),
            ),
        };
        Ok(Self {
            database,
            config,
            host_config,
            blob_dir,
            wake: Arc::new(Notify::new()),
            seat,
            client,
            retry: host::OLD_DAEMON_RETRY,
        })
    }

    /// The log line naming the process that hosts instead of this one, if
    /// another does.
    pub(crate) fn client_line(&self) -> Option<&str> {
        self.client.as_deref()
    }

    /// Notifying this requests a round, or wakes the host this is a client
    /// of. A notification sent during a round is kept and starts the next one.
    pub(crate) fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    pub(crate) async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let mut shutdown = std::pin::pin!(shutdown);
        let seat = match self.seat {
            Some(seat) => seat,
            None => {
                let Some(seat) = Self::wait_for_seat(
                    &self.database,
                    &self.config,
                    &self.wake,
                    self.retry,
                    shutdown.as_mut(),
                )
                .await?
                else {
                    return Ok(());
                };
                println!(
                    "sync-host reason=host-stopped lock={}",
                    host::lock_path(self.database.path()).display()
                );
                seat
            }
        };
        let host = Host {
            database: self.database,
            seat,
            role: Role::Mcp,
            version: crate::update::current_version().to_string(),
            config: self.host_config,
            blob_dir: self.blob_dir,
            sync: DaemonSync::default(),
            delivery: Delivery::Here(crate::notification::delivery()),
            events: Box::new(|_| {}),
            wake: self.wake,
            debounce: Duration::ZERO,
            executable: None,
        };
        host.run(shutdown).await.map(drop)
    }

    /// Wakes the other host on every request and tries the election again
    /// every `retry`. Returns `None` on shutdown.
    async fn wait_for_seat(
        database: &Database,
        config: &AppConfig,
        wake: &Notify,
        retry: Duration,
        mut shutdown: Pin<&mut impl Future<Output = ()>>,
    ) -> Result<Option<Seat>> {
        let wake_addr = config.wake_addr()?;
        let mut next_election = Instant::now() + retry;
        loop {
            tokio::select! {
                () = shutdown.as_mut() => return Ok(None),
                () = wake.notified() => wake_if_enabled(config),
                () = tokio::time::sleep_until(next_election) => {
                    if let Election::Won(seat) = host::elect(database.path(), wake_addr).await? {
                        return Ok(Some(seat));
                    }
                    next_election = Instant::now() + retry;
                }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    #[tokio::test]
    async fn a_client_hosts_once_the_other_host_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let (database, _pool) = crate::test_support::open_database(&db).await.unwrap();
        let blob_dir = dir.path().join("blobs");
        std::fs::create_dir_all(&blob_dir).unwrap();
        let other_host = File::create(host::lock_path(&db)).unwrap();
        other_host.try_lock().unwrap();

        let config = aven_host::test_support::isolated_config();
        let host_config = watch::channel(config.clone()).1;
        let mut sync = EmbeddedSync::elect(database.clone(), config, host_config, blob_dir)
            .await
            .unwrap();
        let client = sync.client_line().expect("another process hosts");
        assert!(client.contains("reason=host-running"), "{client}");
        sync.retry = Duration::from_millis(20);
        let (stop, mut stopped) = watch::channel(false);
        let worker = tokio::spawn(sync.run(async move {
            let _ = stopped.wait_for(|stop| *stop).await;
        }));

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(database.host_status().await.unwrap(), None);
        drop(other_host);
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = database.host_status().await.unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "the client never took over");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(status.role, "mcp");
        assert_eq!(status.pid, std::process::id());
        assert!(host::lock_is_held(&db).unwrap());

        stop.send_replace(true);
        worker.await.unwrap().unwrap();
    }
}
