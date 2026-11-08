//! The background host: the one process per database that runs sync rounds,
//! reminders and attachment maintenance, whether it is the daemon, Aven
//! Desktop or `aven mcp --sync`.
//!
//! A host holds an exclusive lock on `<db>.host.lock` for its whole life and
//! listens on the database's [`wake`] socket. The lock alone decides who
//! hosts. Only the lock holder writes the database's host status row, so a stale heartbeat while the lock is held means the host stopped
//! responding, and is reported rather than taken over. The host stamps the
//! lock file's modification time when it takes the lock, which tells a host
//! that is still starting from one that never recorded its status.
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use aven_core::db::Database;
use aven_core::local_state::HostStatus;
use tokio::time::Instant;
use tracing::warn;

pub mod executable;
pub mod runtime;
pub mod wake;

pub use runtime::{Delivery, Exit, Host, HostEvent, RoundReport, watch_config_file};

/// How often a host records that it is alive.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
/// A heartbeat older than this, while the lock is held, is a fault, and so
/// is a host that took the lock this long ago without recording its status.
pub const HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(3 * 60);
/// How often Aven Desktop and `aven mcp --sync` try again to host while
/// another process holds the lock.
pub const CLIENT_RETRY: Duration = Duration::from_secs(30);
/// How often the daemon tries again while another host holds the lock.
pub const LOCK_RETRY: Duration = Duration::from_secs(5);
/// How long a caller waits for the round it asked a host for.
pub const ROUND_WAIT: Duration = Duration::from_secs(5);
const ROUND_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long an election keeps trying a lock that is held, so that a process
/// briefly probing it with [`lock_is_held`] can't make it lose.
const PROBE_GRACE: Duration = Duration::from_millis(250);
const PROBE_GRACE_POLL: Duration = Duration::from_millis(10);

pub const CAPABILITY_SYNC: &str = "sync";
pub const CAPABILITY_REMINDERS: &str = "reminders";

/// Which program hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Daemon,
    Desktop,
    Mcp,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Desktop => "desktop",
            Self::Mcp => "mcp",
        }
    }
}

/// Where the host lock of the database at `db` lives.
pub fn lock_path(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or(db.as_os_str()).to_os_string();
    name.push(".host.lock");
    db.with_file_name(name)
}

/// The host lock and wake socket of a host. Dropping it gives up both, the
/// socket first.
#[derive(Debug)]
pub struct Seat {
    pub(crate) wake: wake::Listener,
    _lock: File,
}

impl Seat {
    /// Where the host listens for wakes, if it could bind a socket.
    pub fn wake_path(&self) -> Option<&Path> {
        self.wake.path()
    }
}

#[derive(Debug)]
pub enum Election {
    Won(Seat),
    /// Another host holds the lock.
    Taken,
}

/// Tries once to become the host of the database at `db`. Takes the lock
/// first, so of several processes at most one binds the wake socket.
pub async fn elect(db: &Path) -> Result<Election> {
    let path = lock_path(db);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open host lock {}", path.display()))?;
    let deadline = Instant::now() + PROBE_GRACE;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                tokio::time::sleep(PROBE_GRACE_POLL).await;
            }
            Err(TryLockError::WouldBlock) => return Ok(Election::Taken),
            Err(TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("lock {}", path.display()));
            }
        }
    }
    if let Err(error) = lock.set_modified(SystemTime::now()) {
        warn!(%error, "could not stamp the host lock");
    }
    Ok(Election::Won(Seat {
        wake: wake::Listener::bind(db),
        _lock: lock,
    }))
}

/// How a message names the program in a host status row's `role`.
pub fn describe_role(role: &str) -> &str {
    match role {
        "daemon" => "`aven daemon`",
        "desktop" => "Aven Desktop",
        "mcp" => "`aven mcp --sync`",
        _ => "another Aven process",
    }
}

/// Names the process that last recorded itself as the database's host, such
/// as "Aven Desktop (pid 42)".
pub async fn describe_host(database: &Database) -> String {
    match database.host_status().await {
        Ok(Some(status)) => format!("{} (pid {})", describe_role(&status.role), status.pid),
        _ => "another Aven process".to_string(),
    }
}

/// What a client can tell about the host of a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostState {
    /// Nothing holds the host lock. The status is the last host's, if any.
    Stopped(Option<HostStatus>),
    /// The lock is held and the heartbeat is recent.
    Running(HostStatus),
    /// The lock was taken recently and its holder has not recorded its
    /// status yet.
    Starting,
    /// The lock is held, but the host has not recorded a heartbeat lately,
    /// or did not record its status in time. The status is the newest row,
    /// which may be an earlier host's.
    Unresponsive(Option<HostStatus>),
}

impl HostState {
    pub fn running(&self) -> Option<&HostStatus> {
        match self {
            Self::Running(status) => Some(status),
            _ => None,
        }
    }
}

/// Whether some process holds the host lock of the database at `db`.
pub fn lock_is_held(db: &Path) -> Result<bool> {
    let path = lock_path(db);
    let lock = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    match lock.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("lock {}", path.display()))
        }
    }
}

pub async fn host_state(database: &Database) -> Result<HostState> {
    let status = database.host_status().await?;
    if !lock_is_held(database.path())? {
        return Ok(HostState::Stopped(status));
    }
    let taken_at = lock_taken_at(database.path());
    Ok(classify(status, taken_at, chrono::Utc::now().timestamp()))
}

/// When the current holder took the host lock of the database at `db`, in
/// Unix seconds.
fn lock_taken_at(db: &Path) -> Option<i64> {
    let modified = std::fs::metadata(lock_path(db)).ok()?.modified().ok()?;
    let since_epoch = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    i64::try_from(since_epoch.as_secs()).ok()
}

fn classify(status: Option<HostStatus>, taken_at: Option<i64>, now: i64) -> HostState {
    let stale_after = HEARTBEAT_STALE_AFTER.as_secs() as i64;
    // A row recorded before the lock was taken is an earlier host's.
    let current = |status: &HostStatus| taken_at.is_none_or(|taken| status.started_at >= taken);
    match status {
        Some(status) if current(&status) => {
            if now - status.heartbeat_at <= stale_after {
                HostState::Running(status)
            } else {
                HostState::Unresponsive(Some(status))
            }
        }
        status => match taken_at {
            Some(taken) if now - taken <= stale_after => HostState::Starting,
            _ => HostState::Unresponsive(status),
        },
    }
}

/// A round requested from the running host, and the generation it must pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundRequest {
    after: i64,
}

/// Wakes the running host for a round that bypasses its failure backoff.
/// Returns `None` when no responsive host syncs this database.
pub async fn request_round(database: &Database) -> Option<RoundRequest> {
    let state = match host_state(database).await {
        Ok(state) => state,
        Err(error) => {
            warn!(error = %format!("{error:#}"), "could not read the background host");
            return None;
        }
    };
    let status = state.running()?;
    if !status
        .capabilities
        .iter()
        .any(|capability| capability == CAPABILITY_SYNC)
    {
        return None;
    }
    let request = RoundRequest {
        after: status.round_started,
    };
    wake::send(database.path(), wake::RETRY);
    Some(request)
}

/// Waits up to `wait` for the first round the host started after `request`
/// to finish. Returns whether it did.
pub async fn await_round(database: &Database, request: RoundRequest, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        match database.host_status().await {
            Ok(Some(status)) if status.round_finished > request.after => return true,
            Ok(_) => {}
            Err(error) => {
                warn!(%error, "could not read the background host");
                return false;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(ROUND_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(heartbeat_at: i64) -> HostStatus {
        HostStatus {
            pid: 1,
            role: "daemon".to_string(),
            version: "0.1.0".to_string(),
            capabilities: vec![CAPABILITY_SYNC.to_string()],
            started_at: 0,
            heartbeat_at,
            round_started: 0,
            round_finished: 0,
        }
    }

    #[test]
    fn the_lock_sits_beside_the_database() {
        assert_eq!(
            lock_path(Path::new("/data/aven.sqlite")),
            Path::new("/data/aven.sqlite.host.lock")
        );
    }

    #[tokio::test]
    async fn a_brief_status_probe_does_not_make_a_starting_host_lose() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        std::fs::write(lock_path(&db), "").unwrap();
        // The shared lock lock_is_held takes, held for longer than a probe.
        let probe = File::open(lock_path(&db)).unwrap();
        probe.try_lock_shared().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(probe);
        });

        let election = elect(&db).await.unwrap();
        release.join().unwrap();
        assert!(matches!(election, Election::Won(_)), "{election:?}");
    }

    #[tokio::test]
    async fn of_two_hosts_only_the_first_wins_until_it_stops() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        assert!(!lock_is_held(&db).unwrap());

        let Election::Won(seat) = elect(&db).await.unwrap() else {
            panic!("the first host wins");
        };
        assert_eq!(seat.wake_path(), Some(wake::paths(&db)[0].as_path()));
        assert!(lock_is_held(&db).unwrap());
        assert!(matches!(elect(&db).await.unwrap(), Election::Taken));

        drop(seat);
        assert!(!lock_is_held(&db).unwrap());
        assert!(matches!(elect(&db).await.unwrap(), Election::Won(_)));
    }

    /// Two users each have their own database in their own state directory,
    /// so each one's host wins its own election and hears only its own wakes.
    #[tokio::test]
    async fn hosts_of_two_users_host_and_wake_independently() {
        let first_home = tempfile::tempdir().unwrap();
        let second_home = tempfile::tempdir().unwrap();
        let first_db = first_home.path().join("aven.sqlite");
        let second_db = second_home.path().join("aven.sqlite");

        let Election::Won(first) = elect(&first_db).await.unwrap() else {
            panic!("the first user's host wins");
        };
        let Election::Won(second) = elect(&second_db).await.unwrap() else {
            panic!("the second user's host wins too");
        };
        assert_ne!(first.wake_path(), second.wake_path());
        assert!(first.wake_path().unwrap().starts_with(first_home.path()));
        assert!(second.wake_path().unwrap().starts_with(second_home.path()));

        let mut buf = [0; 4];
        assert!(wake::send(&second_db, wake::RETRY));
        let len = tokio::time::timeout(Duration::from_secs(5), second.wake.recv(&mut buf))
            .await
            .expect("the second host hears its wake")
            .unwrap();
        assert_eq!(&buf[..len], wake::RETRY);
        assert!(
            first.wake.try_recv(&mut buf).is_err(),
            "nothing for the first"
        );

        assert!(wake::send(&first_db, wake::CHANGED));
        let len = tokio::time::timeout(Duration::from_secs(5), first.wake.recv(&mut buf))
            .await
            .expect("the first host hears its wake")
            .unwrap();
        assert_eq!(&buf[..len], wake::CHANGED);
        assert!(
            second.wake.try_recv(&mut buf).is_err(),
            "nothing for the second"
        );
    }

    #[test]
    fn a_held_lock_with_a_stale_heartbeat_is_unresponsive() {
        let stale = HEARTBEAT_STALE_AFTER.as_secs() as i64;
        assert!(matches!(
            classify(Some(status(1_000)), Some(0), 1_000 + stale),
            HostState::Running(_)
        ));
        assert!(matches!(
            classify(Some(status(1_000)), Some(0), 1_001 + stale),
            HostState::Unresponsive(Some(_))
        ));
        assert!(matches!(
            classify(Some(status(1_000)), None, 1_001 + stale),
            HostState::Unresponsive(Some(_))
        ));
        assert_eq!(classify(None, None, 0), HostState::Unresponsive(None));
    }

    #[test]
    fn a_host_that_has_not_recorded_its_status_is_starting_for_a_while() {
        let stale = HEARTBEAT_STALE_AFTER.as_secs() as i64;
        assert_eq!(classify(None, Some(1_000), 1_000), HostState::Starting);
        assert_eq!(
            classify(None, Some(1_000), 1_000 + stale),
            HostState::Starting
        );
        assert_eq!(
            classify(None, Some(1_000), 1_001 + stale),
            HostState::Unresponsive(None)
        );
        // An earlier host's row, however fresh, does not speak for the new one.
        let earlier = status(999);
        assert_eq!(
            classify(Some(earlier.clone()), Some(1_000), 1_000),
            HostState::Starting
        );
        assert_eq!(
            classify(Some(earlier.clone()), Some(1_000), 1_001 + stale),
            HostState::Unresponsive(Some(earlier))
        );
        let mut current = status(1_000);
        current.started_at = 1_000;
        assert!(matches!(
            classify(Some(current), Some(1_000), 1_000),
            HostState::Running(_)
        ));
    }

    #[tokio::test]
    async fn host_state_reads_the_lock_and_the_status_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let database = Database::open(&db).await.unwrap();
        assert_eq!(
            host_state(&database).await.unwrap(),
            HostState::Stopped(None)
        );

        let Election::Won(seat) = elect(&db).await.unwrap() else {
            panic!("the host wins");
        };
        assert_eq!(host_state(&database).await.unwrap(), HostState::Starting);
        let now = chrono::Utc::now().timestamp();
        database
            .record_host_start(7, "daemon", "0.1.0", &[CAPABILITY_SYNC], now)
            .await
            .unwrap();
        assert!(matches!(
            host_state(&database).await.unwrap(),
            HostState::Running(status) if status.pid == 7
        ));

        drop(seat);
        assert!(matches!(
            host_state(&database).await.unwrap(),
            HostState::Stopped(Some(status)) if status.pid == 7
        ));
    }

    #[tokio::test]
    async fn a_requested_round_waits_for_a_round_started_after_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let database = Database::open(&db).await.unwrap();
        let Election::Won(seat) = elect(&db).await.unwrap() else {
            panic!("the host wins");
        };
        let now = chrono::Utc::now().timestamp();
        database
            .record_host_start(7, "daemon", "0.1.0", &[CAPABILITY_SYNC], now)
            .await
            .unwrap();
        // A round in progress when the request arrives does not count.
        database.record_host_round_started(7, 4).await.unwrap();

        let request = request_round(&database).await.unwrap();
        let mut buf = [0; 4];
        let len = seat.wake.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], wake::RETRY);
        database.record_host_round_finished(7, 4).await.unwrap();
        assert!(!await_round(&database, request, Duration::from_millis(200)).await);

        let host = {
            let database = database.clone();
            tokio::spawn(async move {
                database.record_host_round_started(7, 5).await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                database.record_host_round_finished(7, 5).await.unwrap();
            })
        };
        assert!(await_round(&database, request, Duration::from_secs(5)).await);
        host.await.unwrap();
    }

    #[tokio::test]
    async fn no_round_is_requested_from_a_host_that_does_not_sync() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let database = Database::open(&db).await.unwrap();
        assert_eq!(request_round(&database).await, None);

        let Election::Won(_seat) = elect(&db).await.unwrap() else {
            panic!("the host wins");
        };
        let now = chrono::Utc::now().timestamp();
        database
            .record_host_start(7, "daemon", "0.1.0", &[CAPABILITY_REMINDERS], now)
            .await
            .unwrap();
        assert_eq!(request_round(&database).await, None);
    }
}
