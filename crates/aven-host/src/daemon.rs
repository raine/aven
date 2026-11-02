//! The parts of the daemon other processes reach: its service definition and
//! the wake it listens for.
use std::net::SocketAddr;
use std::time::Duration;

use aven_core::db::Database;
use aven_core::local_state::DaemonSyncState;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::config::AppConfig;

pub mod service;

pub use service::{ServiceStatus, status_snapshot};

/// How long a foreground sync waits for the daemon retry it requested.
const DAEMON_RETRY_WAIT: Duration = Duration::from_secs(5);
const DAEMON_RETRY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A successful foreground sync can repair prompt-less key access. The `r` wake
/// bypasses backoff; ordinary mutation wakes retain the legacy `1` payload.
/// Returns the daemon health that the requested retry will replace.
pub async fn retry_after_foreground_sync(
    database: &Database,
    config: &AppConfig,
) -> Option<DaemonSyncState> {
    if !config.automatic_sync_is_enabled() {
        return None;
    }
    match database.daemon_sync_state().await {
        Ok(state) if state.error.is_some() => {
            send_wake(config.wake_addr().ok()?, b"r");
            Some(state)
        }
        Err(error) => {
            warn!(%error, "could not read daemon sync health");
            None
        }
        _ => None,
    }
}

/// Waits briefly for the requested retry to record a result, so a caller that
/// shows daemon health doesn't report the failure the retry just replaced.
pub async fn await_daemon_retry(database: &Database, recorded: &DaemonSyncState) {
    await_daemon_health_change(database, recorded, DAEMON_RETRY_WAIT).await;
}

async fn await_daemon_health_change(
    database: &Database,
    recorded: &DaemonSyncState,
    wait: Duration,
) {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        tokio::time::sleep(DAEMON_RETRY_POLL_INTERVAL).await;
        match database.daemon_sync_state().await {
            Ok(state) if state != *recorded => return,
            Ok(_) => {}
            Err(error) => {
                warn!(%error, "could not read daemon sync health");
                return;
            }
        }
    }
}

pub fn wake_if_enabled(config: &AppConfig) {
    if !config.daemon_has_work() {
        return;
    }
    let Ok(addr) = config.wake_addr() else {
        return;
    };
    debug!(wake_addr = %addr, "waking daemon after local mutation");
    wake(addr);
}

fn wake(addr: SocketAddr) {
    send_wake(addr, b"1");
}

fn send_wake(addr: SocketAddr, payload: &[u8]) {
    let bind_addr = SocketAddr::new(addr.ip(), 0);
    match std::net::UdpSocket::bind(bind_addr).and_then(|socket| socket.send_to(payload, addr)) {
        Ok(_) => debug!(wake_addr = %addr, "daemon wake sent"),
        Err(err) => warn!(wake_addr = %addr, error = %err, "daemon wake send failed"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn foreground_recovery_wakes_only_when_enabled_and_error_recorded() {
        let database = Database::open(Path::new(":memory:")).await.unwrap();
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        config.daemon.wake_addr = Some(socket.local_addr().unwrap().to_string());
        let mut buf = [0; 16];
        assert!(
            retry_after_foreground_sync(&database, &config)
                .await
                .is_none()
        );
        assert!(socket.recv(&mut buf).is_err());
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
        assert!(socket.recv(&mut buf).is_err());
        config.sync.disable_override = false;
        let recorded = retry_after_foreground_sync(&database, &config).await;
        assert_eq!(socket.recv(&mut buf).unwrap(), 1);
        assert_eq!(buf[0], b'r');
        assert_eq!(recorded.unwrap().next_retry_at, Some(400));
        // Foreground success cannot claim that the daemon recovered.
        assert!(database.daemon_sync_state().await.unwrap().error.is_some());
        database.record_daemon_sync_round(200, None).await.unwrap();
        assert!(
            retry_after_foreground_sync(&database, &config)
                .await
                .is_none()
        );
        assert!(socket.recv(&mut buf).is_err());
    }

    #[tokio::test]
    async fn daemon_retry_wait_ends_when_the_retry_records_a_result() {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("db.sqlite")).await.unwrap();
        let failure = ("protected-key-storage-unavailable", "unlock", 400);

        for retry in [
            None,
            Some(("protected-key-storage-unavailable", "unlock", 500)),
        ] {
            database
                .record_daemon_sync_round(100, Some(failure))
                .await
                .unwrap();
            let recorded = database.daemon_sync_state().await.unwrap();
            let daemon = {
                let database = database.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    database.record_daemon_sync_round(200, retry).await.unwrap();
                })
            };
            tokio::time::timeout(
                Duration::from_secs(5),
                await_daemon_retry(&database, &recorded),
            )
            .await
            .expect("the recorded retry ends the wait");
            daemon.await.unwrap();
        }
    }

    #[tokio::test]
    async fn daemon_retry_wait_is_bounded_when_no_result_arrives() {
        let database = Database::open(Path::new(":memory:")).await.unwrap();
        database
            .record_daemon_sync_round(100, Some(("network", "retry", 400)))
            .await
            .unwrap();
        let recorded = database.daemon_sync_state().await.unwrap();
        let started = Instant::now();
        await_daemon_health_change(&database, &recorded, Duration::from_millis(250)).await;
        assert!(started.elapsed() >= Duration::from_millis(250));
    }

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
}
