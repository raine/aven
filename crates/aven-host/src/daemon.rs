//! The parts of the daemon other processes reach: its service definition and
//! the wake it listens for.
use std::path::Path;

use aven_core::db::Database;
use tracing::{debug, warn};

use crate::config::AppConfig;
use crate::host::{self, ROUND_WAIT, RoundRequest, wake};

pub mod service;

pub use service::{ServiceStatus, status_snapshot};

/// A successful foreground sync can repair prompt-less key access, so after
/// one it asks a host that recorded a failure for a round now, with a wake
/// that bypasses backoff. Returns the round to wait for, when a host can
/// report it.
pub async fn retry_after_foreground_sync(
    database: &Database,
    config: &AppConfig,
) -> Option<RoundRequest> {
    if !config.sync_is_allowed() {
        return None;
    }
    match database.daemon_sync_state().await {
        Ok(state) if state.error.is_some() => host::request_round(database).await,
        Err(error) => {
            warn!(%error, "could not read daemon sync health");
            None
        }
        _ => None,
    }
}

/// Waits briefly for the requested round to finish, so a caller that shows
/// daemon health doesn't report the failure the round just replaced.
pub async fn await_daemon_retry(database: &Database, request: RoundRequest) {
    host::await_round(database, request, ROUND_WAIT).await;
}

/// Tells the host of the database at `db` about a local change. A host may
/// sync while `sync.enabled` is off, as Aven Desktop does, so only
/// `AVEN_SYNC_DISABLED` without reminders keeps the wake back.
pub fn wake_if_enabled(config: &AppConfig, db: &Path) {
    if !config.sync_is_allowed() && !config.reminders.enabled {
        return;
    }
    debug!(db = %db.display(), "waking the host after a local mutation");
    wake::send(db, wake::CHANGED);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::host::Election;

    async fn host_seat(db: &Path) -> host::Seat {
        let Election::Won(seat) = host::elect(db).await.unwrap() else {
            panic!("the host wins");
        };
        seat
    }

    fn received(seat: &host::Seat) -> Option<Vec<u8>> {
        let mut buf = [0; 16];
        seat.wake
            .try_recv(&mut buf)
            .ok()
            .map(|len| buf[..len].to_vec())
    }

    async fn receive(seat: &host::Seat) -> Vec<u8> {
        let mut buf = [0; 16];
        let len = tokio::time::timeout(Duration::from_secs(5), seat.wake.recv(&mut buf))
            .await
            .expect("a wake arrives")
            .unwrap();
        buf[..len].to_vec()
    }

    #[tokio::test]
    async fn foreground_recovery_wakes_only_when_allowed_and_error_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let database = Database::open(&db).await.unwrap();
        let seat = host_seat(&db).await;
        database
            .record_host_start(7, "daemon", "0.1.0", &[host::CAPABILITY_SYNC], now())
            .await
            .unwrap();
        let mut config = AppConfig::default();
        assert!(
            retry_after_foreground_sync(&database, &config)
                .await
                .is_none()
        );
        assert_eq!(received(&seat), None);
        database
            .record_daemon_sync_round(
                100,
                Some(("protected-key-storage-unavailable", "unlock", 400)),
            )
            .await
            .unwrap();
        config.sync.disable_override = true;
        assert!(
            retry_after_foreground_sync(&database, &config)
                .await
                .is_none()
        );
        assert_eq!(received(&seat), None);
        config.sync.disable_override = false;
        assert!(
            retry_after_foreground_sync(&database, &config)
                .await
                .is_some()
        );
        assert_eq!(receive(&seat).await, wake::RETRY);
        // Foreground success cannot claim that the daemon recovered.
        assert!(database.daemon_sync_state().await.unwrap().error.is_some());
        database.record_daemon_sync_round(200, None).await.unwrap();
        assert!(
            retry_after_foreground_sync(&database, &config)
                .await
                .is_none()
        );
        assert_eq!(received(&seat), None);
    }

    #[tokio::test]
    async fn foreground_recovery_returns_the_round_a_syncing_host_will_run() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let database = Database::open(&db).await.unwrap();
        let _seat = host_seat(&db).await;
        let config = AppConfig::default();
        database
            .record_host_start(7, "desktop", "0.1.0", &[host::CAPABILITY_SYNC], now())
            .await
            .unwrap();
        database
            .record_daemon_sync_round(100, Some(("network", "retry", 400)))
            .await
            .unwrap();

        let request = retry_after_foreground_sync(&database, &config)
            .await
            .expect("the host reports its rounds");
        database.record_host_round_started(7, 1).await.unwrap();
        database.record_host_round_finished(7, 1).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            await_daemon_retry(&database, request),
        )
        .await
        .expect("the finished round ends the wait");
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    #[tokio::test]
    async fn wake_if_enabled_wakes_the_host_of_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let other = dir.path().join("other.sqlite");
        let seat = host_seat(&db).await;
        let other_seat = host_seat(&other).await;
        let mut config = AppConfig::default();
        config.sync.enabled = true;

        wake_if_enabled(&config, &db);

        assert_eq!(receive(&seat).await, wake::CHANGED);
        assert_eq!(received(&other_seat), None);
    }

    #[tokio::test]
    async fn wake_if_enabled_wakes_a_host_that_syncs_without_sync_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let seat = host_seat(&db).await;

        wake_if_enabled(&AppConfig::default(), &db);

        assert_eq!(receive(&seat).await, wake::CHANGED);
    }

    #[tokio::test]
    async fn wake_if_enabled_skips_when_sync_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let seat = host_seat(&db).await;
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        config.sync.disable_override = true;

        wake_if_enabled(&config, &db);

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(received(&seat), None);
    }

    #[test]
    fn wake_if_enabled_without_a_host_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.sync.enabled = true;

        wake_if_enabled(&config, &dir.path().join("db.sqlite"));
    }
}
