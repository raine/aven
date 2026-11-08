//! The host's run loop: sync rounds, reminders, notification delivery and
//! attachment maintenance on one schedule, following config changes as they
//! arrive.
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use aven_core::db::Database;
use aven_core::sync::client::errors::is_access_refusal;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tracing::{debug, info, warn};

use super::executable::{self, Fingerprint};
use super::wake::{self, Listener};
use super::{CAPABILITY_REMINDERS, CAPABILITY_SYNC, HEARTBEAT_INTERVAL, Role, Seat};
use crate::config::AppConfig;
use crate::notify::{self, Deliver};
use crate::reminders;
use crate::sync::encrypted::{DaemonRound, DaemonSync, Outcome};
use crate::sync::error_explanations::{self, ErrorAction, ErrorSurface};
use crate::sync::scheduler::{ROUND_BUDGET, SyncScheduler};

pub const ATTACHMENT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How often a host that delivers looks for notifications left waiting, such
/// as those queued by a command or while another process delivered.
const DELIVERY_INTERVAL: Duration = Duration::from_secs(30);
/// How often a host checks that its wake socket file is still in place.
const WAKE_SOCKET_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Why a host stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    Shutdown,
    /// The executable it runs was replaced at this path.
    ExecutableChanged(PathBuf),
}

/// What a host's owner may want to show.
#[derive(Debug)]
pub enum HostEvent {
    Round(RoundReport),
}

/// Who delivers the notifications a host queues.
pub enum Delivery {
    /// The host does, through this service, while it holds the delivery lock.
    Here(Arc<dyn Deliver>),
    /// Another part of this process does; notified when notifications are
    /// queued or asked for.
    Elsewhere(Arc<Notify>),
}

#[derive(Debug)]
pub enum RoundReport {
    Completed(Outcome),
    NotSetUp,
    Deferred,
    VaultDeleted,
    Failed(anyhow::Error),
}

/// Everything a host runs on. `config` carries the config to follow; its
/// sender decides what each version enables.
pub struct Host {
    pub database: Database,
    pub seat: Seat,
    pub role: Role,
    pub version: String,
    pub config: watch::Receiver<AppConfig>,
    pub blob_dir: PathBuf,
    pub sync: DaemonSync,
    pub delivery: Delivery,
    /// Hears what the host did. Called on the host's task, so it must not
    /// block.
    pub events: Box<dyn FnMut(&HostEvent) + Send>,
    /// Notifying this requests a round. A notification sent during a round is
    /// kept and starts the next one.
    pub wake: Arc<Notify>,
    /// How long a round requested through `wake` waits for more requests, so
    /// a burst of local changes syncs in one round.
    pub debounce: Duration,
    /// Stops the host when this executable changes.
    pub executable: Option<Fingerprint>,
}

fn capabilities(config: &AppConfig) -> Vec<&'static str> {
    let mut capabilities = Vec::new();
    if config.automatic_sync_is_enabled() {
        capabilities.push(CAPABILITY_SYNC);
    }
    if config.reminders.enabled {
        capabilities.push(CAPABILITY_REMINDERS);
    }
    capabilities
}

/// Records a heartbeat until dropped, independent of how long a round takes.
struct Heartbeat(JoinHandle<()>);

impl Heartbeat {
    fn start(database: Database, pid: u32) -> Self {
        Self(tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
                let now = chrono::Utc::now().timestamp();
                if let Err(error) = database.record_host_heartbeat(pid, now).await {
                    warn!(%error, "could not record the host heartbeat");
                }
            }
        }))
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Host {
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<Exit> {
        let Host {
            database,
            mut seat,
            role,
            version,
            config: mut configs,
            blob_dir,
            mut sync,
            delivery,
            mut events,
            wake,
            debounce,
            executable,
        } = self;
        // The daemon's and MCP's log lines; the app has no log to write to.
        let log = role != Role::Desktop;
        let pid = std::process::id();
        let mut config = configs.borrow_and_update().clone();
        let status = database
            .record_host_start(
                pid,
                role.as_str(),
                &version,
                &capabilities(&config),
                chrono::Utc::now().timestamp(),
            )
            .await?;
        let _heartbeat = Heartbeat::start(database.clone(), pid);
        if log {
            print_work(&config);
        }
        let mut generation = status.round_started.max(status.round_finished);
        // Set by a wake, so the next round is recorded for whoever asked.
        let mut requested = false;
        // Outlives each iteration, so a signal during a sync round still ends
        // the loop once that round returns.
        let mut shutdown = std::pin::pin!(shutdown);
        let mut configs_open = true;
        let mut wake_buf = [0_u8; 16];
        let now = Instant::now();
        let mut scheduler = SyncScheduler::new(interval(&config), now);
        let mut next_reminder = now;
        let mut next_delivery = now;
        let mut next_attachment_maintenance = now;
        let mut next_executable_check = now + executable::CHECK_INTERVAL;
        let mut next_wake_socket_check = now + WAKE_SOCKET_CHECK_INTERVAL;
        loop {
            let sync_on = config.automatic_sync_is_enabled();
            let reminders_on = config.reminders.enabled;
            tokio::select! {
                () = &mut shutdown => {
                    info!("host shutting down");
                    return Ok(Exit::Shutdown);
                }
                changed = configs.changed(), if configs_open => {
                    if changed.is_err() {
                        configs_open = false;
                        continue;
                    }
                    let next = configs.borrow_and_update().clone();
                    let now = Instant::now();
                    if interval(&next) != interval(&config) {
                        scheduler = SyncScheduler::new(interval(&next), now);
                    } else if next.automatic_sync_is_enabled() && !sync_on {
                        scheduler.wake(true, now);
                    }
                    if next.reminders.enabled != reminders_on
                        || next.reminders.digest_time != config.reminders.digest_time
                    {
                        next_reminder = now;
                    }
                    if let Err(error) = database.record_host_capabilities(pid, &capabilities(&next)).await {
                        warn!(%error, "could not record host capabilities");
                    }
                    if log {
                        println!(
                            "daemon-config-reloaded sync={} reminders={}",
                            next.automatic_sync_is_enabled(),
                            next.reminders.enabled
                        );
                        print_work(&next);
                    }
                    config = next;
                }
                retry_now = next_wake(&seat.wake, &wake, debounce, &mut wake_buf) => {
                    let now = Instant::now();
                    requested = true;
                    next_reminder = now;
                    next_delivery = now;
                    scheduler.wake(retry_now, now);
                }
                _ = sleep_until(next_executable_check), if executable.is_some() => {
                    if let Some(fingerprint) = &executable
                        && fingerprint.changed()?
                    {
                        return Ok(Exit::ExecutableChanged(fingerprint.path.clone()));
                    }
                    next_executable_check = Instant::now() + executable::CHECK_INTERVAL;
                }
                _ = sleep_until(next_wake_socket_check) => {
                    seat.wake.refresh();
                    next_wake_socket_check = Instant::now() + WAKE_SOCKET_CHECK_INTERVAL;
                }
                // Reminders wait while sync rounds have more work, so they
                // never judge a partly synced database.
                _ = sleep_until(next_reminder), if reminders_on && !scheduler.catching_up() => {
                    let check = reminders::remind(&database, &config.reminders).await;
                    if check.queued > 0 {
                        next_delivery = Instant::now();
                    }
                    next_reminder = Instant::now() + check.wait;
                }
                _ = sleep_until(next_delivery) => {
                    match &delivery {
                        Delivery::Here(service) => deliver(&database, service, log).await,
                        Delivery::Elsewhere(deliverer) => deliverer.notify_one(),
                    }
                    next_delivery = Instant::now() + DELIVERY_INTERVAL;
                }
                _ = sleep_until(next_attachment_maintenance), if sync_on => {
                    maintain_attachments(&database, &blob_dir, &config, log).await;
                    next_attachment_maintenance =
                        Instant::now() + ATTACHMENT_MAINTENANCE_INTERVAL;
                }
                _ = sleep_until(scheduler.next_sync()), if sync_on => {
                    scheduler.round_started();
                    generation += 1;
                    let recorded = std::mem::take(&mut requested);
                    if recorded
                        && let Err(error) = database.record_host_round_started(pid, generation).await
                    {
                        warn!(%error, "could not record the host round");
                    }
                    let round = sync.round(&database, &config, ROUND_BUDGET).await;
                    let report = handle_round(round, &database, &config, &mut scheduler, role).await;
                    if matches!(report, RoundReport::Completed(_)) && !scheduler.catching_up() {
                        next_reminder = Instant::now();
                    }
                    if recorded
                        && let Err(error) = database.record_host_round_finished(pid, generation).await
                    {
                        warn!(%error, "could not record the host round");
                    }
                    events(&HostEvent::Round(report));
                }
            }
        }
    }
}

/// Delivers waiting notifications unless another process does.
async fn deliver(database: &Database, service: &Arc<dyn Deliver>, log: bool) {
    let drained = match notify::deliver_pending(database, service).await {
        Ok(drained) => drained,
        Err(error) => {
            warn!(error = %format!("{error:#}"), "notification delivery failed");
            return;
        }
    };
    if !log {
        return;
    }
    for notification in &drained.submitted {
        println!(
            "daemon-reminded title={:?} message={:?}",
            notification.title, notification.body
        );
    }
    for (notification, reason) in &drained.failed {
        println!(
            "daemon-notification-failed reason={reason:?} title={:?}",
            notification.title
        );
    }
    if drained.expired > 0 {
        println!("daemon-notifications-expired count={}", drained.expired);
    }
}

fn interval(config: &AppConfig) -> Duration {
    Duration::from_secs(config.sync_interval_seconds())
}

fn print_work(config: &AppConfig) {
    if !config.daemon_has_work() {
        println!(
            "daemon-idle hint=\"run `aven config set sync.enabled true` or `aven config set reminders.enabled true`\""
        );
    }
}

async fn handle_round(
    round: Result<DaemonRound>,
    database: &Database,
    config: &AppConfig,
    scheduler: &mut SyncScheduler,
    role: Role,
) -> RoundReport {
    let log = role != Role::Desktop;
    let now = Instant::now();
    match round {
        Ok(DaemonRound::Completed(outcome)) => {
            if log {
                println!(
                    "daemon-synced rounds={} metadata_caught_up={} images={}",
                    outcome.rounds, outcome.metadata_caught_up, outcome.images
                );
            }
            if let Err(error) = database
                .record_daemon_sync_round(chrono::Utc::now().timestamp(), None)
                .await
            {
                warn!(%error, "could not persist daemon sync health");
            }
            if role == Role::Daemon
                && let Err(error) = database
                    .record_background_sync(chrono::Utc::now().timestamp())
                    .await
            {
                warn!(%error, "could not record the background sync");
            }
            scheduler.completed(outcome.more_work_ready(), now);
            RoundReport::Completed(outcome)
        }
        Ok(DaemonRound::NotSetUp) => {
            if scheduler.not_set_up(now) && log {
                println!(
                    "daemon-sync-not-set-up hint=\"run `aven sync setup` or `aven sync join`\""
                );
            }
            RoundReport::NotSetUp
        }
        Ok(DaemonRound::Deferred) => {
            debug!("daemon sync deferred");
            scheduler.deferred(now);
            RoundReport::Deferred
        }
        Ok(DaemonRound::VaultDeleted) => {
            if scheduler.vault_deleted(now) && log {
                println!(
                    "daemon-sync-vault-deleted hint=\"this sync's vault was deleted; run `aven sync reset`, then `aven sync setup` with a new setup code\""
                );
            }
            RoundReport::VaultDeleted
        }
        Err(error) if is_access_refusal(&error) => {
            if scheduler.access_refused(now) {
                warn!(error = %error, "daemon sync refused by server");
            }
            record_failure(database, &error, config.sync_interval_seconds()).await;
            RoundReport::Failed(error)
        }
        Err(error) => {
            let retry_seconds = scheduler.failed(now);
            record_failure(database, &error, retry_seconds).await;
            warn!(error = %daemon_error(&error), retry_seconds, "daemon sync failed");
            RoundReport::Failed(error)
        }
    }
}

fn daemon_error(error: &anyhow::Error) -> String {
    match error_explanations::explain(ErrorAction::General, ErrorSurface::Cli, error) {
        Some(explanation) => format!(
            "{} [{}] Next: {} Cause: {error:#}",
            explanation.message, explanation.code, explanation.next_step
        ),
        None => error.to_string(),
    }
}

async fn record_failure(database: &Database, error: &anyhow::Error, retry_seconds: u64) {
    let explanation = error_explanations::explain(ErrorAction::General, ErrorSurface::Tui, error);
    let code = explanation
        .map(|explanation| explanation.code.to_string())
        .or_else(|| error_explanations::codes(error).last())
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

async fn maintain_attachments(database: &Database, blob_dir: &Path, config: &AppConfig, log: bool) {
    let policy = config.local.attachment_lifecycle.policy();
    match database.prune_attachments(blob_dir, policy, true).await {
        Ok(summary) => {
            if log {
                println!(
                    "daemon-maintained eligible={} eligible_bytes={} pruned={} pruned_bytes={}",
                    summary.eligible.count,
                    summary.eligible.bytes,
                    summary.pruned.count,
                    summary.pruned.bytes,
                );
            }
        }
        Err(err) => warn!(error = %err, "attachment maintenance failed"),
    }
}

/// Waits for a wake from another process or from this one, and returns whether
/// it may bypass failure backoff.
async fn next_wake(
    listener: &Listener,
    direct_wake: &Notify,
    debounce: Duration,
    wake_buf: &mut [u8],
) -> bool {
    tokio::select! {
        () = direct_wake.notified() => {
            debug!("daemon wake notified");
            tokio::time::sleep(debounce).await;
            false
        }
        result = listener.recv(wake_buf) => {
            let mut retry_now = false;
            match result {
                Ok(len) => {
                    retry_now = &wake_buf[..len] == wake::RETRY;
                    debug!("daemon wake received");
                }
                Err(err) => warn!(error = %err, "daemon wake receive failed"),
            }
            retry_now | drain_wakes(listener, wake_buf)
        }
    }
}

fn drain_wakes(listener: &Listener, wake_buf: &mut [u8]) -> bool {
    let mut retry_now = false;
    while let Ok(len) = listener.try_recv(wake_buf) {
        retry_now |= &wake_buf[..len] == wake::RETRY;
    }
    retry_now
}

/// Follows the config file at `path`, starting from `initial`, and passes
/// each version through `adjust` first. A version that does not load is
/// skipped, keeping the one before it. Stops once every receiver is gone.
pub fn watch_config_file(
    path: PathBuf,
    initial: AppConfig,
    adjust: impl Fn(&mut AppConfig) + Send + 'static,
) -> watch::Receiver<AppConfig> {
    let mut initial = initial;
    adjust(&mut initial);
    let (sender, receiver) = watch::channel(initial);
    tokio::spawn(async move {
        let mut seen = modified(&path);
        loop {
            tokio::select! {
                () = sender.closed() => return,
                () = tokio::time::sleep(CONFIG_POLL_INTERVAL) => {}
            }
            let now = modified(&path);
            if now == seen {
                continue;
            }
            seen = now;
            match AppConfig::load_from_path(&path) {
                Ok(mut config) => {
                    adjust(&mut config);
                    sender.send_replace(config);
                }
                Err(error) => warn!(error = %format!("{error:#}"), "config change not applied"),
            }
        }
    });
    receiver
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::host::{Election, elect};

    #[tokio::test]
    async fn draining_preserves_retry_wakes_among_ordinary_wakes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let listener = Listener::bind(&db);
        for payload in [wake::CHANGED, wake::RETRY, wake::CHANGED] {
            assert!(wake::send(&db, payload));
        }
        assert!(drain_wakes(&listener, &mut [0; 16]));
        assert!(!drain_wakes(&listener, &mut [0; 16]));
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

    struct Recorder(Mutex<Vec<String>>);

    impl Deliver for Recorder {
        fn deliver(&self, request: &notify::Request) -> Result<notify::Report> {
            let mut posted = self.0.lock().unwrap();
            posted.extend(request.posts.iter().map(|post| post.title.clone()));
            Ok(notify::Report {
                authorization: notify::Authorization::Allowed,
                errors: vec![None; request.posts.len()],
                delivered: Some(Vec::new()),
            })
        }
    }

    async fn host(
        dir: &Path,
        config: AppConfig,
    ) -> (
        Host,
        watch::Sender<AppConfig>,
        Arc<Recorder>,
        sqlx::SqlitePool,
    ) {
        let db = dir.join("db.sqlite");
        let (database, pool) = crate::test_support::open_database(&db).await.unwrap();
        let Election::Won(seat) = elect(&db).await.unwrap() else {
            panic!("the host wins");
        };
        let (configs, receiver) = watch::channel(config);
        let recorder = Arc::new(Recorder(Mutex::new(Vec::new())));
        let host = Host {
            database,
            seat,
            role: Role::Daemon,
            version: "0.1.0".to_string(),
            config: receiver,
            blob_dir: dir.join("blobs"),
            sync: DaemonSync::default(),
            delivery: Delivery::Here(recorder.clone()),
            events: Box::new(|_| {}),
            wake: Arc::new(Notify::new()),
            debounce: Duration::ZERO,
            executable: None,
        };
        (host, configs, recorder, pool)
    }

    async fn wait_for(mut done: impl AsyncFnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done().await {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_host_without_work_idles_until_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let (host, _configs, recorder, _pool) = host(dir.path(), AppConfig::default()).await;
        let database = host.database.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let running = tokio::spawn(host.run(async {
            let _ = stopped.await;
        }));

        wait_for(async || database.host_status().await.unwrap().is_some()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!running.is_finished(), "an idle host keeps running");
        let status = database.host_status().await.unwrap().unwrap();
        assert!(status.capabilities.is_empty());
        assert_eq!(status.role, "daemon");
        assert_eq!(status.pid, std::process::id());
        assert!(recorder.0.lock().unwrap().is_empty());

        stop.send(()).unwrap();
        assert_eq!(running.await.unwrap().unwrap(), Exit::Shutdown);
    }

    #[tokio::test]
    async fn a_host_takes_up_config_changes_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let (host, configs, recorder, pool) = host(dir.path(), AppConfig::default()).await;
        let database = host.database.clone();
        // A task due today, so turning reminders on sends the digest.
        let mut conn = pool.acquire().await.unwrap();
        let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        crate::operations::create_task(
            &mut conn,
            &workspace,
            crate::operations::TaskDraft {
                title: "Ship".to_string(),
                description: String::new(),
                project: None,
                status: Some("todo".to_string()),
                priority: "none".to_string(),
                source: aven_core::choices::TaskSource::Unknown,
                labels: Vec::new(),
                metadata: Vec::new(),
                available_at: None,
                due_on: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
                is_epic: false,
            },
        )
        .await
        .unwrap();
        drop(conn);
        let running = tokio::spawn(host.run(std::future::pending()));
        wait_for(async || database.host_status().await.unwrap().is_some()).await;

        let mut config = AppConfig::default();
        config.reminders.enabled = true;
        config.reminders.digest_time = "00:00".to_string();
        configs.send_replace(config);
        wait_for(async || {
            database.host_status().await.unwrap().unwrap().capabilities == [CAPABILITY_REMINDERS]
        })
        .await;
        wait_for(async || !recorder.0.lock().unwrap().is_empty()).await;
        assert_eq!(*recorder.0.lock().unwrap(), ["1 due today"]);
        running.abort();
    }

    /// Two users' hosts, each on its own database in its own state
    /// directory, both run, and a round asked of one runs only there.
    #[tokio::test]
    async fn hosts_of_two_users_each_run_the_rounds_asked_of_them() {
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        // Rounds run only when asked, within the test.
        config.sync.interval_seconds = Some(3600);
        let first_home = tempfile::tempdir().unwrap();
        let second_home = tempfile::tempdir().unwrap();
        let (first, _first_configs, _, _first_pool) = host(first_home.path(), config.clone()).await;
        let (second, _second_configs, _, _second_pool) = host(second_home.path(), config).await;
        let first_db = first.database.clone();
        let second_db = second.database.clone();
        let first_run = tokio::spawn(first.run(std::future::pending()));
        let second_run = tokio::spawn(second.run(std::future::pending()));
        for database in [&first_db, &second_db] {
            wait_for(async || {
                crate::host::host_state(database)
                    .await
                    .unwrap()
                    .running()
                    .is_some_and(|status| status.capabilities == [CAPABILITY_SYNC])
            })
            .await;
        }

        let before = second_db.host_status().await.unwrap().unwrap();
        let request = crate::host::request_round(&first_db)
            .await
            .expect("the first host syncs");
        assert!(crate::host::await_round(&first_db, request, Duration::from_secs(10)).await);
        let after = second_db.host_status().await.unwrap().unwrap();
        assert_eq!(
            after.round_finished, before.round_finished,
            "the second host was not woken"
        );

        let request = crate::host::request_round(&second_db)
            .await
            .expect("the second host syncs");
        assert!(crate::host::await_round(&second_db, request, Duration::from_secs(10)).await);
        first_run.abort();
        second_run.abort();
    }
}
