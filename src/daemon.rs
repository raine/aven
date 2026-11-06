//! `aven daemon`: the background host that a service manager starts.
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use aven_host::host::executable::{self, Fingerprint};
use aven_host::host::{self, Election, Exit, Host, Role, Seat};
use aven_host::reminders::{Notification, ReminderSender};
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

/// Posts reminders through the platform's notification service.
struct SystemNotifications;

impl ReminderSender for SystemNotifications {
    fn send(&self, notification: &Notification) -> Result<()> {
        crate::notification::send_notification(&notification.title, &notification.message)
    }
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
        println!(
            "daemon-executable-changed path={}",
            fingerprint.path.display()
        );
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
        reminders: Arc::new(SystemNotifications),
        events: Box::new(|_| {}),
        wake: Arc::new(Notify::new()),
        debounce: Duration::ZERO,
        executable: Some(fingerprint),
    };
    if let Exit::ExecutableChanged(path) = host.run(shutdown).await? {
        println!("daemon-executable-changed path={}", path.display());
    }
    Ok(())
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
                println!(
                    "daemon-executable-changed path={}",
                    fingerprint.path.display()
                );
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
/// executable changes.
pub(crate) struct EmbeddedSync {
    host: Host,
}

impl EmbeddedSync {
    /// Becomes the database's host, so a failure surfaces before the owner
    /// starts serving. Requesting a worker enables sync for it regardless of
    /// `sync.enabled`; `AVEN_SYNC_DISABLED` still refuses.
    pub(crate) async fn prepare(
        database: Database,
        config: &AppConfig,
        blob_dir: PathBuf,
    ) -> Result<Self> {
        config.ensure_sync_allowed()?;
        let wake_addr = config.wake_addr()?;
        let seat = match host::elect(database.path(), wake_addr).await? {
            Election::Won(seat) => seat,
            Election::Taken => bail!(host::host_running_message(&database).await),
            Election::WakeAddressBusy(error) => {
                return Err(error).context(host::wake_address_busy_message(wake_addr));
            }
        };
        Ok(Self {
            host: Host {
                database,
                seat,
                role: Role::Mcp,
                version: crate::update::current_version().to_string(),
                config: follow_config(config, |config| config.sync.enabled = true),
                blob_dir,
                sync: DaemonSync::default(),
                reminders: Arc::new(SystemNotifications),
                events: Box::new(|_| {}),
                wake: Arc::new(Notify::new()),
                debounce: Duration::ZERO,
                executable: None,
            },
        })
    }

    /// Notifying this requests a round. A notification sent during a round is
    /// kept and starts the next one.
    pub(crate) fn wake(&self) -> Arc<Notify> {
        self.host.wake.clone()
    }

    pub(crate) async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        self.host.run(shutdown).await.map(drop)
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
