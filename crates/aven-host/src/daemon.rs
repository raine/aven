//! The parts of the daemon other processes reach: its service definition and
//! the wake it listens for.
use std::net::SocketAddr;

use aven_core::db::Database;
use tracing::{debug, warn};

use crate::config::AppConfig;

pub mod service;

pub use service::{ServiceStatus, status_snapshot};

/// A successful foreground sync can repair prompt-less key access. The `r` wake
/// bypasses backoff; ordinary mutation wakes retain the legacy `1` payload.
pub async fn retry_after_foreground_sync(database: &Database, config: &AppConfig) {
    if !config.automatic_sync_is_enabled() {
        return;
    }
    match database.daemon_sync_state().await {
        Ok(state) if state.error.is_some() => {
            if let Ok(addr) = config.wake_addr() {
                send_wake(addr, b"r");
            }
        }
        Err(error) => warn!(%error, "could not read daemon sync health"),
        _ => {}
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
        retry_after_foreground_sync(&database, &config).await;
        assert!(socket.recv(&mut buf).is_err());
        database
            .record_daemon_sync_round(
                100,
                Some(("protected-key-storage-unavailable", "unlock", 400)),
            )
            .await
            .unwrap();
        config.sync.disable_override = true;
        retry_after_foreground_sync(&database, &config).await;
        assert!(socket.recv(&mut buf).is_err());
        config.sync.disable_override = false;
        retry_after_foreground_sync(&database, &config).await;
        assert_eq!(socket.recv(&mut buf).unwrap(), 1);
        assert_eq!(buf[0], b'r');
        // Foreground success cannot claim that the daemon recovered.
        assert!(database.daemon_sync_state().await.unwrap().error.is_some());
        database.record_daemon_sync_round(200, None).await.unwrap();
        retry_after_foreground_sync(&database, &config).await;
        assert!(socket.recv(&mut buf).is_err());
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
