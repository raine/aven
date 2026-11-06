//! The parts of the daemon other processes reach: its service definition and
//! the wake it listens for.
use std::net::SocketAddr;

use aven_core::db::Database;
use tracing::{debug, warn};

use crate::config::AppConfig;
use crate::host::{self, ROUND_WAIT, RoundRequest};

pub mod service;

pub use service::{ServiceStatus, status_snapshot};

/// A successful foreground sync can repair prompt-less key access, so after
/// one it asks a host that recorded a failure for a round now. The `r` wake
/// bypasses backoff; ordinary mutation wakes retain the legacy `1` payload.
/// Returns the round to wait for, when a host can report it.
pub async fn retry_after_foreground_sync(
    database: &Database,
    config: &AppConfig,
) -> Option<RoundRequest> {
    if !config.sync_is_allowed() {
        return None;
    }
    match database.daemon_sync_state().await {
        Ok(state) if state.error.is_some() => {
            let addr = config.wake_addr().ok()?;
            let request = host::request_round(database, addr).await;
            if request.is_none() {
                // A daemon from before the host lock still takes the wake.
                send_wake(addr, b"r");
            }
            request
        }
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

/// Tells the host about a local change. A host may sync while `sync.enabled`
/// is off, as Aven Desktop does, so only `AVEN_SYNC_DISABLED` without
/// reminders keeps the wake back.
pub fn wake_if_enabled(config: &AppConfig) {
    if !config.sync_is_allowed() && !config.reminders.enabled {
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

pub(crate) fn send_wake(addr: SocketAddr, payload: &[u8]) {
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
    async fn foreground_recovery_wakes_only_when_allowed_and_error_recorded() {
        let database = Database::open(Path::new(":memory:")).await.unwrap();
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut config = AppConfig::default();
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
        // With no host holding the lock, the wake still reaches an old daemon.
        config.sync.disable_override = false;
        assert!(
            retry_after_foreground_sync(&database, &config)
                .await
                .is_none()
        );
        assert_eq!(socket.recv(&mut buf).unwrap(), 1);
        assert_eq!(buf[0], b'r');
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
    async fn foreground_recovery_returns_the_round_a_syncing_host_will_run() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let database = Database::open(&db).await.unwrap();
        let host::Election::Won(seat) = host::elect(&db, "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap()
        else {
            panic!("the host wins");
        };
        let mut config = AppConfig::default();
        config.daemon.wake_addr = Some(seat.wake_addr().unwrap().to_string());
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
    fn wake_if_enabled_wakes_a_host_that_syncs_without_sync_enabled() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut config = AppConfig::default();
        config.daemon.wake_addr = Some(socket.local_addr().unwrap().to_string());

        wake_if_enabled(&config);

        let mut buf = [0_u8; 1];
        assert_eq!(socket.recv(&mut buf).unwrap(), 1);
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
