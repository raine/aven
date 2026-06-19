use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use tokio::net::UdpSocket;
use tokio::time::{Instant, sleep_until, timeout};
use tracing::{debug, info, warn};

use crate::config::AppConfig;
use crate::db::open_db;
use crate::signals::shutdown_signal;
use crate::sync::run_sync_once;

pub struct DaemonRunArgs {
    pub db_path: PathBuf,
    pub config: AppConfig,
}

pub async fn run(args: DaemonRunArgs) -> Result<()> {
    if !args.config.sync.enabled {
        bail!("error sync-disabled hint=\"set sync.enabled = true in config.toml\"");
    }
    let server = args
        .config
        .sync
        .server_url
        .clone()
        .context("error sync-server-required hint=\"set sync.server_url in config.toml\"")?;
    let wake_addr = args.config.wake_addr()?;
    let interval_seconds = args.config.sync_interval_seconds();
    let pool = open_db(&args.db_path).await?;
    let socket = UdpSocket::bind(wake_addr).await.with_context(|| {
        format!("could not bind daemon wake address {wake_addr}; is another daemon running?")
    })?;
    info!(
        db = %args.db_path.display(),
        server = %server,
        wake_addr = %wake_addr,
        interval_seconds,
        "daemon starting"
    );
    println!(
        "daemon db={} server={} wake={}",
        args.db_path.display(),
        server,
        wake_addr
    );

    run_loop(
        pool,
        server,
        socket,
        interval_seconds,
        args.config.sync_auth_token().map(str::to_string),
    )
    .await
}

async fn run_loop(
    pool: SqlitePool,
    server: String,
    socket: UdpSocket,
    interval_seconds: u64,
    auth_token: Option<String>,
) -> Result<()> {
    let mut wake_buf = [0_u8; 16];
    let mut backoff_seconds = 1_u64;
    let mut next_sync = Instant::now();
    loop {
        tokio::select! {
            _ = shutdown_signal() => {
                info!("daemon shutting down");
                break;
            }
            result = socket.recv_from(&mut wake_buf) => {
                if let Err(err) = result {
                    warn!(error = %err, "daemon wake receive failed");
                    eprintln!("daemon wake failed: {err}");
                } else {
                    debug!("daemon wake received");
                }
                drain_wakes(&socket, &mut wake_buf);
                next_sync = Instant::now();
            }
            _ = sleep_until(next_sync) => {
                match timeout(
                    Duration::from_secs(35),
                    sync_once(&pool, &server, auth_token.as_deref()),
                )
                .await
                {
                    Ok(Ok(())) => {
                        backoff_seconds = 1;
                        next_sync = Instant::now() + Duration::from_secs(interval_seconds);
                    }
                    Ok(Err(err)) => {
                        warn!(error = %err, backoff_seconds, "daemon sync failed");
                        eprintln!("daemon sync failed: {err}");
                        next_sync = Instant::now() + Duration::from_secs(backoff_seconds);
                        backoff_seconds = (backoff_seconds * 2).min(300);
                    }
                    Err(_) => {
                        warn!(backoff_seconds, "daemon sync timed out");
                        eprintln!("daemon sync failed: timed out");
                        next_sync = Instant::now() + Duration::from_secs(backoff_seconds);
                        backoff_seconds = (backoff_seconds * 2).min(300);
                    }
                }
            }
        }
    }
    Ok(())
}

fn drain_wakes(socket: &UdpSocket, wake_buf: &mut [u8]) {
    while socket.try_recv_from(wake_buf).is_ok() {}
}

async fn sync_once(pool: &SqlitePool, server: &str, auth_token: Option<&str>) -> Result<()> {
    let mut conn = pool.acquire().await?;
    let summary = run_sync_once(&mut conn, server, auth_token).await?;
    info!(
        pushed = summary.pushed,
        pulled = summary.pulled,
        cursor = summary.cursor,
        "daemon sync completed"
    );
    println!(
        "daemon-synced pushed={} pulled={} cursor={}",
        summary.pushed, summary.pulled, summary.cursor
    );
    Ok(())
}

pub fn wake(addr: SocketAddr) {
    let bind_addr = SocketAddr::new(addr.ip(), 0);
    match std::net::UdpSocket::bind(bind_addr).and_then(|socket| socket.send_to(b"1", addr)) {
        Ok(_) => debug!(wake_addr = %addr, "daemon wake sent"),
        Err(err) => warn!(wake_addr = %addr, error = %err, "daemon wake send failed"),
    }
}
