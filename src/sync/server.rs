use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use aven_core::sync::seed_claim::Secret;
use tokio::net::TcpListener;
use tracing::info;

use crate::cli::{ServerArgs, ServerSetupArgs, ServerSubcommand};
use crate::config;
use crate::signals::shutdown_signal;

/// Setup invitations stay usable for one hour, or until a device claims storage.
const SETUP_INVITATION_SECONDS: u64 = 3600;

const UNPREPARED_STORAGE: &str =
    "error server-storage-unprepared hint=\"run `aven server setup --data PATH --url URL` first\"";
const INVALID_MEMBERSHIP: &str = "error server-membership-invalid hint=\"stored device membership failed verification; restore this path from a backup or prepare a new one with `aven server setup`\"";
const UNSUPPORTED_STORAGE: &str = "error server-storage-unsupported hint=\"this storage holds unencrypted sync history, which is no longer supported; prepare a new path with `aven server setup`\"";

pub(crate) async fn run_server(args: ServerArgs, config: config::AppConfig) -> Result<()> {
    if let Some(ServerSubcommand::Setup(setup)) = args.command {
        return setup_server(setup).await;
    }
    let data = args.data.context("error server-data-required")?;
    serve(args.bind, &data, &config).await
}

async fn setup_server(args: ServerSetupArgs) -> Result<()> {
    let server = super::encrypted::server_origin(&args.url)?;
    let database = Database::open(&args.data).await?;
    let mut fresh_id = [0; 32];
    getrandom::fill(&mut fresh_id).map_err(|_| anyhow::anyhow!("error server-setup-entropy"))?;
    let secret = Secret::generate()?;
    let setup_id = database
        .issue_e2ee_server_setup(
            &secret,
            fresh_id,
            super::encrypted::unix_now()? + SETUP_INVITATION_SECONDS,
        )
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error e2ee-server-storage-not-empty" => error.context(UNSUPPORTED_STORAGE),
            _ => error,
        })?;
    let invitation = super::encrypted::SetupInvitation {
        server,
        setup_id,
        secret,
    };
    println!("{}", invitation.encode().as_str());
    eprintln!("Anyone with this invitation can claim this server. It expires in one hour.");
    eprintln!("Run `aven sync setup` on the device whose data should start the sync.");
    let port = url::Url::parse(&args.url)
        .ok()
        .and_then(|url| url.port_or_known_default())
        .unwrap_or(3554);
    eprintln!(
        "Then start the server: aven server --data {} --bind 127.0.0.1:{port}",
        args.data.display()
    );
    Ok(())
}

/// Serves the seed, enrollment and tail/image routers. Each operation
/// authenticates against the stored vault; TLS belongs in a reverse proxy, so
/// only loopback binds are accepted.
async fn serve(bind: SocketAddr, data: &Path, config: &config::AppConfig) -> Result<()> {
    if !bind.ip().is_loopback() {
        bail!("error server-bind-loopback hint=\"bind 127.0.0.1 behind a TLS reverse proxy\"");
    }
    if !data.exists() {
        bail!(UNPREPARED_STORAGE);
    }
    let database = Database::open(data).await?;
    if !database.is_e2ee_server_storage().await? {
        if database.has_change_history().await? {
            bail!(UNSUPPORTED_STORAGE);
        }
        bail!(UNPREPARED_STORAGE);
    }
    database
        .verify_membership_history()
        .await
        .map_err(|error| error.context(INVALID_MEMBERSHIP))?;
    let app = crate::seed_bootstrap_http::router(database.clone(), None, Default::default())
        .merge(crate::peer_enrollment_http::router(database.clone()))
        .merge(crate::encrypted_tail_http::router_with_policy(
            database,
            config.local.attachment_lifecycle.server_policy(),
        ));
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    info!(bind = %addr, "sync server starting");
    println!("listening url=http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
