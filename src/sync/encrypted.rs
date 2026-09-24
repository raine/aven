//! End-to-end encrypted sync commands over the isolated encrypted transports.
//!
//! A database takes part in sync once it holds a seed genesis or enrollment
//! pin. Its server is the locator bound into protected enrollment identity;
//! configuration never redirects it. Other databases stay local until they are
//! set up or joined.
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use aven_core::db::Database;
use aven_core::sync::seed_claim::ClaimAuthentication;
use serde::Serialize;
use zeroize::Zeroizing;

use crate::cli::SetupArgs;
use crate::config::{self, AppConfig};
use crate::encrypted_tail_http::{self as tail_http, ImageTransfer, Round};
use crate::peer_enrollment_http;
use crate::protected_local_keys::{EnrollmentReadiness, ProtectedLocalKeyStore};
use crate::render::print_json_pretty;
use crate::seed_bootstrap_http;

mod devices;
pub(crate) use devices::{list as list_devices, remove as remove_device};
mod invitation;
use invitation::DeviceInvitation;
pub(super) use invitation::{SetupInvitation, server_origin};

#[cfg(test)]
mod tests;

/// Upper bound on bounded rounds in one interactive drain.
pub(crate) const ROUND_LIMIT: usize = 1000;
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

const NOT_SET_UP: &str = "error sync-not-set-up hint=\"run `aven sync setup` with an invitation from `aven server setup`, or `aven sync join` on a new database\"";

/// True when this database takes part in sync, including an interrupted
/// setup or join.
pub(crate) async fn is_set_up(database: &Database) -> Result<bool> {
    Ok(database.enrollment_pin().await?.is_some()
        || database.local_seed_genesis_commitment().await?.is_some())
}

/// Setup and joining progress that a caller may present. Stages report where
/// the engine is, not how much remains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    PreparingData,
    UploadingData,
    FinishingSetup,
    WaitingForInviter,
    DownloadingTasks,
    /// Synced tasks are installed; changes made after the snapshot are applied.
    CatchingUp,
    /// Tasks are current; images are still transferring.
    DownloadingImages,
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

/// What setup would publish from this database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SetupPreview {
    pub(crate) workspaces: usize,
    pub(crate) tasks: i64,
    /// Synced images whose bytes are missing here; they sync as unavailable.
    pub(crate) missing_images: u64,
    /// The database still names a server from earlier unencrypted sync.
    pub(crate) leaves_unencrypted_server: bool,
}

pub(crate) async fn setup_preview(database: &Database) -> Result<SetupPreview> {
    let workspaces = database.list_workspaces().await?;
    let mut tasks = 0;
    for workspace in &workspaces {
        tasks += database.workspace_task_counts(&workspace.id).await?.visible;
    }
    Ok(SetupPreview {
        workspaces: workspaces.len(),
        tasks,
        missing_images: database.missing_sync_attachment_counts().await?.count,
        leaves_unencrypted_server: database.meta("sync_server_url").await?.is_some(),
    })
}

async fn print_setup_preview(database: &Database, server: &str) -> Result<()> {
    let preview = setup_preview(database).await?;
    eprintln!("Set up sync from this database");
    eprintln!("  Database: {}", database.path().display());
    eprintln!("  Server: {server}");
    eprintln!(
        "  Workspaces: {}, tasks: {}",
        preview.workspaces, preview.tasks
    );
    if preview.missing_images > 0 {
        eprintln!(
            "  Images missing on this computer: {} (synced as unavailable)",
            preview.missing_images
        );
    }
    if preview.leaves_unencrypted_server {
        eprintln!("  This database stops using its previous unencrypted sync server.");
    }
    eprintln!(
        "Every other device starts from this data. Afterwards this database cannot use \
         backup restore or import."
    );
    Ok(())
}

/// Refuses setup when sync is disabled or this database already takes part.
pub(crate) async fn ensure_setup_available(database: &Database, config: &AppConfig) -> Result<()> {
    config.ensure_sync_allowed()?;
    ensure!(
        database.enrollment_pin().await?.is_none(),
        "error sync-already-set-up hint=\"run `aven sync` or `aven sync status`\""
    );
    Ok(())
}

pub(crate) async fn setup(database: &Database, config: &AppConfig, args: SetupArgs) -> Result<()> {
    ensure_setup_available(database, config).await?;
    let invitation = SetupInvitation::decode(&read_invitation("Setup invitation: ")?)?;
    let resuming = database.local_seed_genesis_commitment().await?.is_some();
    if !resuming {
        print_setup_preview(database, &invitation.server).await?;
        confirm_setup(args.yes)?;
    }
    let outcome = run_setup(database, config, &invitation, &|stage| {
        if stage == Stage::UploadingData {
            eprintln!("Uploading encrypted data...");
        }
    })
    .await?;
    println!("Sync set up with {}", invitation.server);
    print_outcome(&outcome);
    Ok(())
}

/// Sets up sync from this database, or resumes the setup it started. The
/// caller confirms a fresh setup first; a resumed one continues the original
/// capture and never recaptures.
pub(crate) async fn run_setup(
    database: &Database,
    config: &AppConfig,
    invitation: &SetupInvitation,
    progress: &(dyn Fn(Stage) + Sync),
) -> Result<Outcome> {
    ensure_setup_available(database, config).await?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
    let bootstrap = seed_bootstrap_http::Client::new(&invitation.server)?;
    // A sealed publication intent means the claim and capture are complete.
    if database.seed_publication_intent_bytes().await?.is_none() {
        progress(Stage::PreparingData);
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
                .map_err(|_| explain_setup_refusal(error))?;
        }
    }
    progress(Stage::UploadingData);
    bootstrap.resume(&store, database).await?;
    progress(Stage::FinishingSetup);
    // Binds this installation's enrollment identity to the server.
    peer_enrollment_http::Client::new(&invitation.server)?
        .refresh(&store, database)
        .await?;
    let client = tail_http::Client::new(&invitation.server)?;
    drain(&client, &store, database, &blob_dir, ROUND_LIMIT).await
}

/// A created device invitation whose inviting device waits for admission.
pub(crate) struct PendingInvitation {
    server: String,
    text: Zeroizing<String>,
    deadline: Instant,
}

impl PendingInvitation {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// QR presentation of the invitation text.
    pub(crate) fn presentation(&self) -> Result<crate::pairing::PairingPresentation> {
        crate::pairing::PairingPresentation::new(&self.server, &self.text)
    }
}

impl std::fmt::Debug for PendingInvitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingInvitation([REDACTED])")
    }
}

/// Creates a device invitation, or resumes the pending one. Ordinary sync on
/// this device pauses until the invitation is used or expires.
pub(crate) async fn create_invitation(
    database: &Database,
    config: &AppConfig,
) -> Result<PendingInvitation> {
    config.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
    let Some((_, server)) = store.association(database).await? else {
        bail!("error sync-setup-incomplete hint=\"rerun `aven sync setup`\"");
    };
    let invitation = peer_enrollment_http::Client::new(&server)?
        .invite(&store, database, unix_now()? + invitation_seconds())
        .await?;
    let text = DeviceInvitation {
        server: server.clone(),
        invitation,
    }
    .encode();
    Ok(PendingInvitation {
        server,
        text,
        deadline: Instant::now() + Duration::from_secs(invitation_seconds()),
    })
}

/// Polls admission until the invited device joins or the invitation expires.
pub(crate) async fn await_admission(
    database: &Database,
    invitation: &PendingInvitation,
) -> Result<()> {
    let store = key_store(database)?;
    let client = peer_enrollment_http::Client::new(&invitation.server)?;
    while Instant::now() < invitation.deadline {
        let admitted = {
            let _guard = super::coordination::acquire(database).await?;
            match client.admit(&store, database).await {
                Err(error) if busy(&error) => false,
                result => result?,
            }
        };
        if admitted {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    bail!(
        "error sync-invitation-unused hint=\"sync on this device stays paused until the invitation is used; after it expires, the next `aven sync` ends it, rotating keys if they may have been sent\""
    )
}

pub(crate) async fn invite(database: &Database, config: &AppConfig) -> Result<()> {
    let invitation = create_invitation(database, config).await?;
    println!("{}", invitation.text());
    std::io::stdout().flush()?;
    print_invitation_qr(&invitation);
    eprintln!("Anyone with this invitation can access all your synced data and manage devices.");
    eprintln!("Run `aven sync join` on the other device. Waiting for it to join...");
    await_admission(database, &invitation).await?;
    println!("Device added");
    Ok(())
}

/// Shows the invitation QR on an interactive standard error; standard output
/// keeps only the invitation text for scripts.
fn print_invitation_qr(invitation: &PendingInvitation) {
    let stderr_is_terminal = std::io::stderr().is_terminal();
    if !stderr_is_terminal {
        return;
    }
    let (columns, styled) = crate::pairing::output_options(
        stderr_is_terminal,
        std::env::var_os("NO_COLOR").is_some(),
        || crossterm::terminal::size().ok().map(|(columns, _)| columns),
    );
    match invitation.presentation().and_then(|presentation| {
        crate::pairing::render_terminal_qr(presentation.qr(), columns, styled)
    }) {
        Ok(qr) => {
            eprint!("{qr}");
            eprintln!("Scan this code on the other device, or paste the invitation.");
        }
        Err(error) => eprintln!("QR code unavailable: {error:#}"),
    }
}

const ALREADY_SET_UP: &str =
    "error sync-already-set-up hint=\"add devices with `aven sync invite` on this database\"";
const JOIN_REQUIRES_EMPTY: &str = "error sync-join-requires-empty-database hint=\"join with a new database, for example `aven --db PATH sync join`\"";

pub(crate) async fn join(database: &Database, config: &AppConfig) -> Result<()> {
    let (server, outcome) = run_join(
        database,
        config,
        || DeviceInvitation::decode(&read_invitation("Device invitation: ")?).map(Some),
        &|stage| match stage {
            Stage::WaitingForInviter => eprintln!("Waiting for the inviting device..."),
            Stage::DownloadingTasks => eprintln!("Downloading synced data..."),
            _ => {}
        },
    )
    .await?;
    println!("Joined sync with {server}");
    print_outcome(&outcome);
    Ok(())
}

/// Joins sync on a fresh database, or resumes the join it started, and
/// returns the server. `invitation` is asked for only while enrollment is
/// unfinished; `None` resumes the request this database already made.
pub(crate) async fn run_join(
    database: &Database,
    config: &AppConfig,
    invitation: impl FnOnce() -> Result<Option<DeviceInvitation>>,
    progress: &(dyn Fn(Stage) + Sync),
) -> Result<(String, Outcome)> {
    config.ensure_sync_allowed()?;
    let store = key_store(database)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let _guard = super::coordination::acquire(database).await?;
    let server = match store.association(database).await? {
        Some((true, server)) => Some(server),
        Some((false, _)) => bail!(ALREADY_SET_UP),
        None => {
            database
                .peer_target_preflight()
                .await
                .context(JOIN_REQUIRES_EMPTY)?;
            None
        }
    };
    let server = if matches!(
        store.enrollment_readiness(database).await?,
        EnrollmentReadiness::Enrolled { .. }
    ) {
        server.context("error sync-join-incomplete")?
    } else {
        let invitation = invitation()?;
        ensure!(
            server
                .as_ref()
                .zip(invitation.as_ref())
                .is_none_or(|(server, invitation)| *server == invitation.server),
            "error sync-join-server-mismatch hint=\"resume with the invitation that started joining\""
        );
        let (server, invitation) = match invitation {
            Some(invitation) => (invitation.server, Some(invitation.invitation)),
            None => (server.context("error sync-join-invitation-required")?, None),
        };
        let client = peer_enrollment_http::Client::new(&server)?;
        client.request(&store, database, invitation).await?;
        progress(Stage::WaitingForInviter);
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
        server
    };
    progress(Stage::DownloadingTasks);
    peer_enrollment_http::Client::new(&server)?
        .install(&store, database)
        .await?;
    progress(Stage::CatchingUp);
    let client = tail_http::Client::new(&server)?;
    let outcome = drain_reporting(
        &client,
        &store,
        database,
        &blob_dir,
        ROUND_LIMIT,
        &mut |round| {
            if round.metadata_caught_up && round.images != ImageTransfer::Complete {
                progress(Stage::DownloadingImages);
            }
        },
    )
    .await?;
    Ok((server, outcome))
}

/// A refused claim may come from a replaced or expired setup invitation, but a
/// refusal does not prove which; the hint names both possibilities.
fn explain_setup_refusal(error: anyhow::Error) -> anyhow::Error {
    if !error.to_string().starts_with("error bootstrap-refused") {
        return error;
    }
    error.context(
        "error sync-setup-refused hint=\"the server refused this setup invitation; if `aven server setup` was run again or the invitation is over an hour old, rerun setup with the newest invitation, otherwise check the server and retry\"",
    )
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

/// A refusal alone proves neither a server failure nor removal of this device.
const REFUSED: &str = "error sync-server-refused hint=\"the server refused this request; it may have failed, or another device may have removed this device from sync, which leaves local tasks and images available here; retry later, and check `aven sync device list` on another device\"";

/// Explains engine refusals that ordinary rounds report while a join or
/// invitation is unfinished.
fn explain_round_error(error: anyhow::Error) -> anyhow::Error {
    match error.to_string().as_str() {
        "error snapshot-not-installed" => {
            error.context("error sync-join-incomplete hint=\"rerun `aven sync join`\"")
        }
        "error enrollment-unresolved" => error.context(
            "error sync-invitation-pending hint=\"sync resumes when the invited device joins, or with the next sync after the unused invitation expires\"",
        ),
        "error enrollment-refused outcome-unknown" => error.context(REFUSED),
        "error withdrawal-required-unsupported" => error.context(
            "error sync-invitation-disclosed hint=\"keys may have been sent to the invited device; sync resumes after it joins, or once the next sync after expiry rotates keys\"",
        ),
        _ => error,
    }
}

/// Drains up to `round_limit` rounds with the server bound during setup or
/// join. The caller holds the sync coordination lock.
async fn drain_associated(
    database: &Database,
    config: &AppConfig,
    round_limit: usize,
) -> Result<Outcome> {
    let store = key_store(database)?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let server = associated_server(&store, database).await?;
    let client = tail_http::Client::new(&server)?;
    drain(&client, &store, database, &blob_dir, round_limit)
        .await
        .map_err(explain_round_error)
}

/// Runs one interactive drain, waiting briefly for another sync to finish.
pub(crate) async fn run_to_completion(database: &Database, config: &AppConfig) -> Result<Outcome> {
    config.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let _guard = super::coordination::acquire(database).await?;
    drain_associated(database, config, ROUND_LIMIT).await
}

pub(crate) async fn sync(database: &Database, config: &AppConfig, json: bool) -> Result<()> {
    let outcome = run_to_completion(database, config).await?;
    if json {
        print_json_pretty(&outcome)
    } else {
        print_outcome(&outcome);
        Ok(())
    }
}

pub(crate) enum DaemonRound {
    Completed(Outcome),
    /// Another process holds the sync coordination lock.
    Deferred,
    /// The database has not been set up or joined; it stays local.
    NotSetUp,
}

/// One bounded daemon round that never waits for another sync.
pub(crate) async fn daemon_round(
    database: &Database,
    config: &AppConfig,
    round_limit: usize,
) -> Result<DaemonRound> {
    config.ensure_sync_allowed()?;
    if !is_set_up(database).await? {
        return Ok(DaemonRound::NotSetUp);
    }
    let Some(_guard) = super::coordination::try_acquire(database)? else {
        return Ok(DaemonRound::Deferred);
    };
    drain_associated(database, config, round_limit)
        .await
        .map(DaemonRound::Completed)
}

#[derive(Debug, Serialize)]
pub(crate) struct Outcome {
    version: u32,
    pub(crate) rounds: usize,
    pub(crate) metadata_caught_up: bool,
    pub(crate) images: &'static str,
}

impl Outcome {
    /// True when another round would likely make progress now: images are
    /// still transferring, or metadata remains while images are not blocked.
    pub(crate) fn more_work_ready(&self) -> bool {
        self.images == "pending" || (!self.metadata_caught_up && self.images == "complete")
    }
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
/// or until `round_limit` or the image retry bound stops it.
pub(crate) async fn drain(
    client: &tail_http::Client,
    store: &ProtectedLocalKeyStore,
    database: &Database,
    blob_dir: &Path,
    round_limit: usize,
) -> Result<Outcome> {
    drain_reporting(client, store, database, blob_dir, round_limit, &mut |_| {}).await
}

async fn drain_reporting(
    client: &tail_http::Client,
    store: &ProtectedLocalKeyStore,
    database: &Database,
    blob_dir: &Path,
    round_limit: usize,
    on_round: &mut (dyn FnMut(&Round) + Send),
) -> Result<Outcome> {
    let mut last = None;
    let mut rounds = 0;
    let mut image_retries = 0;
    while rounds < round_limit {
        let round: Round = client.round(store, database, blob_dir).await?;
        on_round(&round);
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
    let mut report = StatusReport {
        version: 1,
        server: None,
        state: "not-set-up",
        local_changes_pending: None,
        server_position: None,
        initial_download_pending: None,
        image_uploads_pending: None,
        image_downloads_pending: None,
        images_unavailable: None,
    };
    if !is_set_up(database).await? {
        if json {
            return print_json_pretty(&report);
        }
        println!("Sync: not set up; this database is local only");
        println!(
            "Run `aven sync setup` with an invitation from `aven server setup`, or \
             `aven sync join` on a new database."
        );
        return Ok(());
    }
    report.state = "setup-incomplete";
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
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
            "State: paused until the invited device joins. After the unused invitation \
             expires, the next `aven sync` ends it."
        ),
        "invitation-disclosed" => println!(
            "State: paused until the invited device joins. Keys may have been sent to it. \
             After the invitation expires, `aven sync` ends it by rotating keys; data the \
             device may already hold stays readable to it."
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
