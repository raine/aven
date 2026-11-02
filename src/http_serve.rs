//! Listener policy and bounded HTTP/1 serving shared by long-running Aven
//! servers.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// Open connections, including idle keep-alive ones. Further clients wait in
/// the listen backlog.
pub(crate) const MAX_CONNECTIONS: usize = 256;
/// Longest a connection may take to send one request's headers.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest graceful shutdown waits for in-flight requests.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BindScope {
    Loopback,
    Private,
    Public,
}

impl BindScope {
    fn classify(address: IpAddr) -> Self {
        if address.is_loopback() {
            Self::Loopback
        } else if is_private_address(address) {
            Self::Private
        } else {
            Self::Public
        }
    }
}

fn is_private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            address.is_private()
                || address.is_link_local()
                || octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(address) => address.is_unique_local() || address.is_unicast_link_local(),
    }
}

/// Refuses a public or wildcard bind unless acknowledged, and reports whether
/// the bind is public so the caller can warn about what it exposes.
pub(crate) fn public_bind(bind: SocketAddr, acknowledged: bool) -> Result<bool> {
    let public = BindScope::classify(bind.ip()) == BindScope::Public;
    if public && !acknowledged {
        bail!(
            "error public-bind-requires hint=\"bind a loopback or private VPN address, or pass --unsafe-public-bind for a public or wildcard address\""
        );
    }
    Ok(public)
}

/// Serves HTTP/1 with bounded connections and a header deadline, so a
/// stalled client can't hold server resources without limit.
pub(crate) async fn serve_connections(
    listener: TcpListener,
    app: axum::Router,
    max_connections: usize,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    let connections = Arc::new(Semaphore::new(max_connections));
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let permit = tokio::select! {
            permit = connections.clone().acquire_owned() => permit?,
            () = &mut shutdown => break,
        };
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => {
                    // Usually descriptor exhaustion; back off instead of spinning.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let connection = hyper::server::conn::http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(HEADER_TIMEOUT)
            .serve_connection(TokioIo::new(stream), TowerToHyperService::new(app.clone()));
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            let _ = connection.await;
            drop(permit);
        });
    }
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, graceful.shutdown()).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    #[test]
    fn classifies_private_and_vpn_bind_addresses() {
        for address in ["127.0.0.1", "::1"] {
            assert_eq!(
                BindScope::classify(address.parse::<IpAddr>().unwrap()),
                BindScope::Loopback
            );
        }
        for address in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "169.254.1.1",
            "fd00::1",
            "fe80::1",
        ] {
            assert_eq!(
                BindScope::classify(address.parse::<IpAddr>().unwrap()),
                BindScope::Private
            );
        }
        for address in [
            "0.0.0.0",
            "::",
            "1.1.1.1",
            "100.128.0.1",
            "2606:4700:4700::1111",
        ] {
            assert_eq!(
                BindScope::classify(address.parse::<IpAddr>().unwrap()),
                BindScope::Public
            );
        }
    }

    async fn read_to_end(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
            .await
            .expect("the server answers or closes")
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn incomplete_headers_time_out_and_release_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "served" }));
        let server = tokio::spawn(serve_connections(listener, app, 1, std::future::pending()));

        // The only connection slot holds a request whose headers never finish.
        let mut stalled = TcpStream::connect(address).await.unwrap();
        stalled
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .unwrap();
        let mut waiting = TcpStream::connect(address).await.unwrap();
        waiting
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        // Let the server start reading the stalled headers before time moves.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut byte = [0; 1];
        assert!(
            tokio::time::timeout(Duration::from_millis(100), waiting.read(&mut byte))
                .await
                .is_err(),
            "a second connection waits while the only slot is held"
        );

        tokio::time::pause();
        tokio::time::advance(HEADER_TIMEOUT).await;
        tokio::time::resume();
        let closed = read_to_end(&mut stalled).await;
        assert!(!closed.contains("served"), "{closed}");
        let response = read_to_end(&mut waiting).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("served"), "{response}");
        server.abort();
    }
}
