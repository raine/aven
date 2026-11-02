use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use aven_core::db::Database;
use aven_core::sync::seed_claim::{Secret, StorageNotEmpty};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tracing::{info, warn};

use crate::ai::{self, AiService};
use crate::cli::{ServerArgs, ServerSetupArgs, ServerSubcommand};
use crate::http_serve::{MAX_CONNECTIONS, public_bind, serve_connections};
use crate::server_config::{self, ServerConfig, ServerPaths};
use crate::signals::shutdown_signal;

/// Setup invitations stay usable for one hour, or until a device claims storage.
const SETUP_INVITATION_SECONDS: u64 = 3600;

/// How often the server prunes unreferenced encrypted images.
const IMAGE_PRUNE_INTERVAL: Duration = Duration::from_secs(600);

const UNPREPARED_STORAGE: &str =
    "error server-storage-unprepared hint=\"run `aven server setup --url URL` first\"";
const INVALID_MEMBERSHIP: &str = "error server-membership-invalid hint=\"stored device membership failed verification; restore this path from a backup or prepare a new one with `aven server setup`\"";
const UNSUPPORTED_STORAGE: &str = "error server-storage-unsupported hint=\"this storage holds unencrypted sync history, which is no longer supported; prepare a new path with `aven server setup`\"";

pub(crate) async fn run_server(args: ServerArgs) -> Result<()> {
    let explicit_data = args.location.data.is_some();
    let paths = ServerPaths::resolve(args.location.data, args.location.config)?;
    match args.command {
        Some(ServerSubcommand::Setup(setup)) => {
            return setup_server(setup, &paths, explicit_data).await;
        }
        Some(ServerSubcommand::Ai(ai)) => return crate::commands::cmd_ai(paths, ai).await,
        None => {}
    }
    let config = ServerConfig::load(paths)?;
    serve(args.bind, args.unsafe_public_bind, &config).await
}

async fn setup_server(
    args: ServerSetupArgs,
    paths: &ServerPaths,
    explicit_data: bool,
) -> Result<()> {
    let server = super::encrypted::server_origin(&args.url)?;
    let data = &paths.data;
    let database = Database::open(data).await?;
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
        .map_err(|error| {
            if error.is::<StorageNotEmpty>() {
                error.context(UNSUPPORTED_STORAGE)
            } else {
                error
            }
        })?;
    let invitation = super::encrypted::SetupInvitation {
        server,
        setup_id,
        secret,
    };
    let invitation = invitation.encode()?;
    let wrote_config = server_config::write_template_if_missing(&paths.config_file)?;
    if args.invitation_only {
        println!("{}", invitation.as_str());
        return Ok(());
    }

    println!("================ SETUP INVITATION ================");
    println!("{}", invitation.as_str());
    println!("==================================================");
    println!("Keep this invitation private. It expires in one hour.");
    println!("Anyone with it can claim this server.");
    println!(
        "Run {sync_setup} on the device whose data should start the sync.",
        sync_setup = crate::render::stdout_command("aven sync setup")
    );
    println!("Server storage: {}", data.display());
    let template = if wrote_config {
        " (commented template)"
    } else {
        ""
    };
    println!("Server config: {}{template}", paths.config_file.display());
    let same_data = if explicit_data {
        " Pass the same --data path to aven server."
    } else {
        ""
    };
    println!(
        "Then start the server on this storage so it's reachable at {}.{same_data}",
        args.url
    );
    Ok(())
}

async fn verify_ai_auth(
    State(database): State<Database>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    if headers
        .get_all(axum::http::header::AUTHORIZATION)
        .iter()
        .count()
        != 1
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(bearer) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(crate::http_admission::bearer)
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match database.verify_enrolled_device_bearer(&bearer).await {
        Ok(()) => next.run(request).await,
        Err(error) if error.is::<aven_core::sync::seed_claim::membership::Unauthorized>() => {
            StatusCode::UNAUTHORIZED.into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Serves the seed, enrollment and tail/image routers. Each operation
/// authenticates against the stored vault. The server does not terminate TLS,
/// so non-loopback binds need an independently protected network.
async fn serve(bind: SocketAddr, unsafe_public_bind: bool, config: &ServerConfig) -> Result<()> {
    let data = config.paths.data.as_path();
    if public_bind(bind, unsafe_public_bind)? {
        eprintln!(
            "Warning: public or wildcard bind {bind} enabled without TLS. Device credentials and setup invitations are not protected by payload encryption."
        );
    }
    info!(storage = %data.display(), "sync server storage");
    info!(config = %config.paths.config_file.display(), "sync server config");
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
    let image_policy = config.attachments.policy();
    let publication_policy = aven_core::sync::bootstrap_staging::PublicationPolicy {
        workspace_quota_bytes: u64::try_from(image_policy.quota_bytes).unwrap_or(0),
    };
    tokio::spawn(prune_images(database.clone(), image_policy.grace));
    let ai_service = Arc::new(AiService::from_config(config));
    log_ai_state(config, &ai_service);
    let ai_app = Router::new()
        .route("/ai/capabilities", get(ai::capabilities_handler))
        .route(
            "/ai/completions/stream",
            post(ai::streaming_handler)
                .layer(axum::extract::DefaultBodyLimit::max(ai::MAX_REQUEST_BYTES)),
        )
        .layer(RequestDecompressionLayer::new())
        // Uploaded audio stays outside request decompression, so the handler
        // owns every rejection.
        .route("/ai/transcriptions", post(ai::transcription_handler))
        .layer(middleware::from_fn_with_state(
            database.clone(),
            verify_ai_auth,
        ))
        .layer(CompressionLayer::new())
        .with_state(ai_service);
    let app = crate::seed_bootstrap_http::router(database.clone(), publication_policy)
        .merge(crate::peer_enrollment_http::router(database.clone()))
        .merge(crate::encrypted_tail_http::router(database, image_policy))
        .merge(ai_app);
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    info!(bind = %addr, "sync server starting");
    println!("listening url=http://{addr}");
    serve_connections(listener, app, MAX_CONNECTIONS, shutdown_signal()).await
}

/// Periodically deletes the bytes of encrypted images that no live task
/// references, in small batches so request writers are not starved.
async fn prune_images(database: Database, grace: Duration) {
    const BATCH: usize = 128;
    let mut interval = tokio::time::interval(IMAGE_PRUNE_INTERVAL);
    loop {
        interval.tick().await;
        loop {
            match database.prune_encrypted_images(grace, BATCH).await {
                Ok(pruned) if pruned == BATCH => tokio::task::yield_now().await,
                Ok(_) => break,
                Err(error) => {
                    warn!(error = %error, "encrypted image prune failed");
                    break;
                }
            }
        }
    }
}

fn log_ai_state(config: &ServerConfig, service: &AiService) {
    let capabilities = service.capabilities();
    let provider = config.ai.provider.as_deref().unwrap_or("none");
    let model = config.ai.model.as_deref().unwrap_or("none");
    let status = capabilities.status.as_str();
    match capabilities.setup_hint.as_deref() {
        Some(hint) => info!(status = %status, provider = %provider, model = %model, hint, "ai"),
        None => info!(status = %status, provider = %provider, model = %model, "ai"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_errors_render_readable_codes_and_steps() {
        for raw in [UNPREPARED_STORAGE, INVALID_MEMBERSHIP, UNSUPPORTED_STORAGE] {
            let error = anyhow::anyhow!("error membership-change-limit").context(raw);
            let lines = crate::cli_error_lines(&error);
            let code = raw.split_whitespace().nth(1).unwrap();
            assert!(lines[0].starts_with("Error: "));
            assert!(lines[0].ends_with(&format!("[{code}]")));
            assert!(!lines[0].contains("hint="));
            assert!(lines[1].starts_with("Next: "));
        }
    }
}
