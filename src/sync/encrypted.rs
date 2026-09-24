//! End-to-end encrypted sync commands over the isolated encrypted transports.
//!
//! A database uses encrypted sync once it holds a seed genesis or enrollment
//! pin. Its server is the locator bound into protected enrollment identity, so
//! configuration and `--server` never redirect it, and plaintext sync keeps
//! refusing it.
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use aven_core::db::Database;
use aven_core::sync::seed_claim::ClaimAuthentication;
use serde::Serialize;
use zeroize::Zeroizing;

use crate::cli::{SetupArgs, SyncArgs};
use crate::config::{self, AppConfig};
use crate::encrypted_tail_http::{self as tail_http, ImageTransfer, Round};
use crate::peer_enrollment_http;
use crate::protected_local_keys::{EnrollmentReadiness, ProtectedLocalKeyStore};
use crate::render::print_json_pretty;
use crate::seed_bootstrap_http;

mod invitation;
use invitation::DeviceInvitation;
pub(super) use invitation::{SetupInvitation, server_origin};

#[cfg(test)]
mod tests;

/// Upper bound on bounded rounds in one interactive drain.
const ROUND_LIMIT: usize = 1000;
/// Consecutive failed or unavailable image rounds before a drain stops. Each
/// such round still pulls, but a failed local head cannot advance.
const IMAGE_RETRY_ROUNDS: usize = 16;
#[cfg(not(test))]
fn invitation_seconds() -> u64 {
    600
}
// CLI test workers declare short-lived invitations to exercise expiry.
#[cfg(test)]
fn invitation_seconds() -> u64 {
    std::env::var("AVEN_TEST_INVITATION_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(600)
}
const INPUT_LIMIT: u64 = 8192;
#[cfg(not(test))]
const POLL_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(test)]
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// True when this database belongs to encrypted sync, including an
/// interrupted setup or join.
pub(crate) async fn is_encrypted(database: &Database) -> Result<bool> {
    Ok(database.enrollment_pin().await?.is_some()
        || database.local_seed_genesis_commitment().await?.is_some())
}

fn key_store(database: &Database) -> Result<ProtectedLocalKeyStore> {
    Ok(ProtectedLocalKeyStore::for_database(database.path())?)
}

pub(super) fn unix_now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn read_invitation(prompt: &str) -> Result<Zeroizing<String>> {
    let stdin = std::io::stdin();
    let mut text = Zeroizing::new(String::new());
    if stdin.is_terminal() {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
        stdin.read_line(&mut text)?;
    } else {
        stdin.lock().take(INPUT_LIMIT).read_to_string(&mut text)?;
    }
    Ok(text)
}

fn confirm_setup(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    let stdin = std::io::stdin();
    ensure!(
        stdin.is_terminal(),
        "error sync-setup-confirmation-required hint=\"rerun with --yes to confirm\""
    );
    eprint!("Set up sync from this database? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    stdin.read_line(&mut answer)?;
    ensure!(
        matches!(answer.trim(), "y" | "Y" | "yes"),
        "error sync-setup-canceled"
    );
    Ok(())
}

async fn print_setup_preview(database: &Database, server: &str) -> Result<()> {
    let workspaces = database.list_workspaces().await?;
    let mut tasks = 0;
    for workspace in &workspaces {
        tasks += database.workspace_task_counts(&workspace.id).await?.visible;
    }
    let missing = database.missing_sync_attachment_counts().await?.count;
    eprintln!("Set up sync from this database");
    eprintln!("  Database: {}", database.path().display());
    eprintln!("  Server: {server}");
    eprintln!("  Workspaces: {}, tasks: {tasks}", workspaces.len());
    if missing > 0 {
        eprintln!("  Images missing on this computer: {missing} (synced as unavailable)");
    }
    if database.meta("sync_server_url").await?.is_some() {
        eprintln!("  This database stops using its previous plaintext sync server.");
    }
    eprintln!(
        "Every other device starts from this data. Afterwards this database cannot use \
         plaintext sync, backup restore, or import."
    );
    Ok(())
}

pub(crate) async fn setup(database: &Database, config: &AppConfig, args: SetupArgs) -> Result<()> {
    config.ensure_sync_allowed()?;
    ensure!(
        database.enrollment_pin().await?.is_none(),
        "error sync-already-set-up hint=\"run `aven sync` or `aven sync status`\""
    );
    let invitation = SetupInvitation::decode(&read_invitation("Setup invitation: ")?)?;
    let resuming = database.local_seed_genesis_commitment().await?.is_some();
    if !resuming {
        print_setup_preview(database, &invitation.server).await?;
        confirm_setup(args.yes)?;
    }
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
    let bootstrap = seed_bootstrap_http::Client::new(&invitation.server)?;
    // A sealed publication intent means the claim and capture are complete.
    if database.seed_publication_intent_bytes().await?.is_none() {
        let seed = store
            .prepare_seed_claim(database, invitation.setup_id)
            .await
            .context("error sync-setup-invitation-mismatch hint=\"resume with the invitation that started setup\"")?;
        store.prepare_seed_source(database).await?;
        database
            .capture_local_shared_state_never_dispatched()
            .await?;
        store
            .package_seed_capture(database, &blob_dir, invitation.setup_id)
            .await?;
        let setup = ClaimAuthentication::SetupSecret(&invitation.secret);
        if let Err(error) = bootstrap.claim(seed.genesis(), setup).await {
            // An admitted claim stays confirmable by its own bearer after the
            // setup invitation expires.
            let bearer = ClaimAuthentication::SeedBearer(seed.bearer());
            bootstrap
                .claim(seed.genesis(), bearer)
                .await
                .map_err(|_| error)?;
        }
    }
    eprintln!("Uploading encrypted data...");
    bootstrap.resume(&store, database).await?;
    // Binds this installation's enrollment identity to the server.
    peer_enrollment_http::Client::new(&invitation.server)?
        .refresh(&store, database)
        .await?;
    let client = tail_http::Client::new(&invitation.server)?;
    let outcome = drain(&client, &store, database, &blob_dir).await?;
    println!("Sync set up with {}", invitation.server);
    print_outcome(&outcome);
    Ok(())
}

pub(crate) async fn invite(database: &Database, config: &AppConfig) -> Result<()> {
    config.ensure_sync_allowed()?;
    let store = key_store(database)?;
    let (server, invitation) = {
        let _guard = super::coordination::acquire(database).await?;
        // A pending invitation is resumed rather than refused.
        let Some((_, server)) = store.association(database).await? else {
            bail!("error sync-setup-incomplete hint=\"rerun `aven sync setup`\"");
        };
        let invitation = peer_enrollment_http::Client::new(&server)?
            .invite(&store, database, unix_now()? + invitation_seconds())
            .await?;
        (server, invitation)
    };
    let client = peer_enrollment_http::Client::new(&server)?;
    let text = DeviceInvitation { server, invitation }.encode();
    println!("{}", text.as_str());
    std::io::stdout().flush()?;
    eprintln!("Anyone with this invitation can access all your synced data and manage devices.");
    eprintln!("Run `aven sync join` on the other device. Waiting for it to join...");
    let deadline = Instant::now() + Duration::from_secs(invitation_seconds());
    while Instant::now() < deadline {
        let admitted = {
            let _guard = super::coordination::acquire(database).await?;
            match client.admit(&store, database).await {
                Err(error) if busy(&error) => false,
                result => result?,
            }
        };
        if admitted {
            println!("Device added");
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    bail!(
        "error sync-invitation-unused hint=\"sync on this device resumes when the invitation is used or has expired unused\""
    )
}

pub(crate) async fn join(database: &Database, config: &AppConfig) -> Result<()> {
    config.ensure_sync_allowed()?;
    let store = key_store(database)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let _guard = super::coordination::acquire(database).await?;
    let server = match store.association(database).await? {
        Some((true, server)) => Some(server),
        Some((false, _)) => bail!(
            "error sync-already-set-up hint=\"add devices with `aven sync invite` on this database\""
        ),
        None => {
            database.peer_target_preflight().await.context(
                "error sync-join-requires-empty-database hint=\"join with a new database, for example `aven --db PATH sync join`\"",
            )?;
            None
        }
    };
    let server = if matches!(
        store.enrollment_readiness(database).await?,
        EnrollmentReadiness::Enrolled { .. }
    ) {
        server.context("error sync-join-incomplete")?
    } else {
        let invitation = DeviceInvitation::decode(&read_invitation("Device invitation: ")?)?;
        ensure!(
            server.as_ref().is_none_or(|s| *s == invitation.server),
            "error sync-join-server-mismatch hint=\"resume with the invitation that started joining\""
        );
        let client = peer_enrollment_http::Client::new(&invitation.server)?;
        client
            .request(&store, database, Some(invitation.invitation))
            .await?;
        eprintln!("Waiting for the inviting device...");
        let deadline = Instant::now() + Duration::from_secs(invitation_seconds());
        loop {
            match client.complete(&store, database).await {
                Ok(true) => break,
                Err(error) if !busy(&error) => return Err(error),
                _ => {}
            }
            ensure!(
                Instant::now() < deadline,
                "error sync-join-timeout hint=\"keep `aven sync invite` running on the other device, then rerun `aven sync join`\""
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        invitation.server
    };
    eprintln!("Downloading synced data...");
    peer_enrollment_http::Client::new(&server)?
        .install(&store, database)
        .await?;
    let client = tail_http::Client::new(&server)?;
    let outcome = drain(&client, &store, database, &blob_dir).await?;
    println!("Joined sync with {server}");
    print_outcome(&outcome);
    Ok(())
}

/// The enrollment server serves one exchange at a time; pollers retry.
fn busy(error: &anyhow::Error) -> bool {
    error.to_string() == "error enrollment-busy"
}

async fn associated_server(store: &ProtectedLocalKeyStore, database: &Database) -> Result<String> {
    match store.association(database).await? {
        Some((peer, server)) => {
            ensure!(
                !peer
                    || matches!(
                        store.enrollment_readiness(database).await?,
                        EnrollmentReadiness::Enrolled { .. }
                    ),
                "error sync-join-incomplete hint=\"rerun `aven sync join`\""
            );
            Ok(server)
        }
        None => bail!("error sync-setup-incomplete hint=\"rerun `aven sync setup`\""),
    }
}

pub(crate) async fn sync(database: &Database, config: &AppConfig, args: &SyncArgs) -> Result<()> {
    config.ensure_sync_allowed()?;
    ensure!(
        args.server.is_none(),
        "error sync-server-fixed hint=\"encrypted sync uses the server chosen during setup\""
    );
    let store = key_store(database)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let _guard = super::coordination::acquire(database).await?;
    let server = associated_server(&store, database).await?;
    let client = tail_http::Client::new(&server)?;
    let outcome = drain(&client, &store, database, &blob_dir)
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error snapshot-not-installed" => {
                error.context("error sync-join-incomplete hint=\"rerun `aven sync join`\"")
            }
            "error enrollment-unresolved" => error.context(
                "error sync-invitation-pending hint=\"sync resumes when the invited device joins or the unused invitation expires\"",
            ),
            "error withdrawal-required-unsupported" => error.context(
                "error sync-invitation-disclosed hint=\"keys may have been sent to the invited device; sync resumes only after it joins\"",
            ),
            _ => error,
        })?;
    if args.json {
        print_json_pretty(&outcome)
    } else {
        print_outcome(&outcome);
        Ok(())
    }
}

#[derive(Serialize)]
pub(crate) struct Outcome {
    version: u32,
    pub(crate) rounds: usize,
    pub(crate) metadata_caught_up: bool,
    pub(crate) images: &'static str,
}

fn image_label(images: ImageTransfer) -> &'static str {
    match images {
        ImageTransfer::Complete => "complete",
        ImageTransfer::Pending => "pending",
        ImageTransfer::Failed => "failed",
        ImageTransfer::Unavailable => "unavailable",
    }
}

/// Repeats bounded rounds until metadata is current and image work settles,
/// or until the round or image retry bound stops it.
pub(crate) async fn drain(
    client: &tail_http::Client,
    store: &ProtectedLocalKeyStore,
    database: &Database,
    blob_dir: &Path,
) -> Result<Outcome> {
    let mut last = None;
    let mut rounds = 0;
    let mut image_retries = 0;
    while rounds < ROUND_LIMIT {
        let round: Round = client.round(store, database, blob_dir).await?;
        rounds += 1;
        last = Some(round);
        match round.images {
            ImageTransfer::Complete if round.metadata_caught_up => break,
            ImageTransfer::Complete | ImageTransfer::Pending => image_retries = 0,
            ImageTransfer::Failed | ImageTransfer::Unavailable => {
                image_retries += 1;
                if image_retries >= IMAGE_RETRY_ROUNDS {
                    break;
                }
            }
        }
    }
    let last = last.context("error sync-no-rounds")?;
    Ok(Outcome {
        version: 1,
        rounds,
        metadata_caught_up: last.metadata_caught_up,
        images: image_label(last.images),
    })
}

fn print_outcome(outcome: &Outcome) {
    if !outcome.metadata_caught_up {
        println!(
            "Sync incomplete: stopped after {} rounds. Run `aven sync` again.",
            outcome.rounds
        );
        if outcome.images == "failed" {
            println!(
                "An image transfer failed, or an image added here is missing from this \
                 computer. Changes after it wait until it uploads."
            );
        }
        return;
    }
    println!("Tasks are up to date");
    match outcome.images {
        "complete" => println!("Images are up to date"),
        "unavailable" => println!("Some images are unavailable on the server"),
        "failed" => println!("Some image transfers failed. Run `aven sync` again to retry."),
        _ => println!("Images are still transferring. Run `aven sync` again."),
    }
}

#[derive(Serialize)]
struct StatusReport {
    version: u32,
    encrypted: bool,
    server: Option<String>,
    state: &'static str,
    local_changes_pending: Option<bool>,
    server_position: Option<i64>,
    initial_download_pending: Option<bool>,
    image_uploads_pending: Option<bool>,
    image_downloads_pending: Option<bool>,
    images_unavailable: Option<bool>,
}

/// Local observation only; contacts no server. Waits briefly for a running
/// sync or invitation exchange, which holds the installation exclusively.
pub(crate) async fn status(database: &Database, json: bool) -> Result<()> {
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
    let mut report = StatusReport {
        version: 1,
        encrypted: true,
        server: None,
        state: "setup-incomplete",
        local_changes_pending: None,
        server_position: None,
        initial_download_pending: None,
        image_uploads_pending: None,
        image_downloads_pending: None,
        images_unavailable: None,
    };
    if let Some((peer, server)) = store.association(database).await? {
        report.server = Some(server.clone());
        report.state = if peer
            && !matches!(
                store.enrollment_readiness(database).await?,
                EnrollmentReadiness::Enrolled { .. }
            ) {
            "join-incomplete"
        } else {
            match store.tail_inputs(database, &server).await {
                Ok(inputs) => {
                    let state = database.encrypted_round_state(&inputs.authority).await?;
                    report.local_changes_pending = Some(!state.idle);
                    report.server_position = Some(state.cursor);
                    report.initial_download_pending = Some(state.downloads.is_none());
                    report.image_uploads_pending = Some(state.upload_pending);
                    report.image_downloads_pending = state.downloads.map(|d| d.pending);
                    report.images_unavailable = state.downloads.map(|d| d.unavailable);
                    "ready"
                }
                // Reading inputs first retires expired never-sent invitations.
                Err(error) => match store.outbound_invitation(database).await? {
                    Some(EnrollmentReadiness::UnresolvedDisclosure) => "invitation-disclosed",
                    Some(_) => "invitation-pending",
                    None if peer && error.to_string() == "error snapshot-not-installed" => {
                        "join-incomplete"
                    }
                    None => return Err(error),
                },
            }
        };
    }
    if json {
        return print_json_pretty(&report);
    }
    println!("Sync: end-to-end encrypted");
    if let Some(server) = &report.server {
        println!("Server: {server}");
    }
    match report.state {
        "setup-incomplete" => println!("State: setup incomplete. Rerun `aven sync setup`."),
        "join-incomplete" => println!("State: joining incomplete. Rerun `aven sync join`."),
        "invitation-pending" => println!(
            "State: paused until the invited device joins or the unused invitation expires."
        ),
        "invitation-disclosed" => println!(
            "State: paused until the invited device joins. Keys may have been sent to it, \
             and expiry does not withdraw them."
        ),
        _ => println!("State: ready"),
    }
    if let Some(pending) = report.local_changes_pending {
        println!(
            "Tasks: {}; server position {}",
            if pending {
                "local changes waiting to sync"
            } else {
                "no local changes waiting"
            },
            report.server_position.unwrap_or_default()
        );
        let mut images = Vec::new();
        if report.initial_download_pending == Some(true) {
            images.push("waiting for the initial task download");
        }
        if report.image_uploads_pending == Some(true) {
            images.push("uploads waiting");
        }
        if report.image_downloads_pending == Some(true) {
            images.push("downloads waiting");
        }
        if report.images_unavailable == Some(true) {
            images.push("some unavailable on the server");
        }
        if images.is_empty() {
            images.push("nothing waiting");
        }
        println!("Images: {}", images.join(", "));
        println!("Status is local. Run `aven sync` to exchange changes with the server.");
    }
    Ok(())
}
