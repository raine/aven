//! End-to-end encrypted sync commands over the isolated encrypted transports.
//!
//! A database takes part in sync once it holds a seed genesis or enrollment
//! pin. Its server is the locator bound into protected enrollment identity;
//! configuration never redirects it. Other databases stay local until they are
//! set up or joined.
use std::collections::HashSet;
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use aven_core::db::Database;
use aven_core::sync::seed_claim::{ClaimAuthentication, membership::Invitation};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::cli::{JoinArgs, SetupArgs};
use crate::config::{self, AppConfig};
use crate::encrypted_tail_http::{self as tail_http, ImageTransfer, Round};
use crate::peer_enrollment_http;
use crate::protected_local_keys::{
    EnrollmentReadiness, ProtectedLocalKeyStore, ProtectedLocalKeyStoreError,
    ProtectedLocalKeyStoreErrorKind,
};
use crate::render::print_json_pretty;
use crate::seed_bootstrap_http;

mod devices;
pub(crate) use devices::{
    Device, DeviceListing, Removal, finish_removal, list as list_devices, load_devices,
    remove as remove_device, remove_other_device,
};
mod invitation;
#[cfg(test)]
pub(crate) use invitation::sample_invitations;
pub(super) use invitation::server_origin;
pub(crate) use invitation::{DeviceInvitation, SetupInvitation};

#[cfg(test)]
mod tests;

/// Upper bound on bounded rounds in one interactive drain. A round may append
/// a bounded run of ordinary records, but still pulls one page and transfers at
/// most one image, so this primarily bounds catch-up and image work.
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

/// Where this database stands in sync, from database facts alone. Reads no
/// protected keys and takes no lock, so it describes rather than authorizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalPhase {
    NotSetUp,
    /// Setup started from this database and has not bound the server yet.
    SetupIncomplete,
    /// Setup is fenced and the server definitely rejected its claim.
    SetupRecoveryRequired,
    /// Joining started and the synced data has not been installed yet.
    JoinIncomplete,
    SetUp,
}

pub(crate) async fn local_phase(database: &Database) -> Result<LocalPhase> {
    Ok(match database.enrollment_pin().await? {
        Some((_, _, role)) if role == "peer" => {
            if database.meta("e2ee_association").await?.is_some() {
                LocalPhase::SetUp
            } else {
                LocalPhase::JoinIncomplete
            }
        }
        Some(_) => LocalPhase::SetUp,
        None if database.local_seed_genesis_commitment().await?.is_some() => {
            if database.meta("e2ee_setup_refused").await?.is_some() {
                LocalPhase::SetupRecoveryRequired
            } else {
                LocalPhase::SetupIncomplete
            }
        }
        None => LocalPhase::NotSetUp,
    })
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

pub(crate) fn unix_now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn read_invitation(prompt: &str) -> Result<Zeroizing<String>> {
    let stdin = std::io::stdin();
    let mut text = Zeroizing::new(String::new());
    if stdin.is_terminal() {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
        #[cfg(unix)]
        let _echo = TerminalEchoGuard::disable()?;
        stdin.read_line(&mut text)?;
        eprintln!();
    } else {
        stdin.lock().take(INPUT_LIMIT).read_to_string(&mut text)?;
    }
    Ok(text)
}

#[cfg(unix)]
struct TerminalEchoGuard(libc::termios);

#[cfg(unix)]
impl TerminalEchoGuard {
    fn disable() -> Result<Self> {
        let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
        ensure!(unsafe { libc::tcgetattr(libc::STDIN_FILENO, settings.as_mut_ptr()) } == 0);
        let original = unsafe { settings.assume_init() };
        let mut hidden = original;
        hidden.c_lflag &= !libc::ECHO;
        ensure!(unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &hidden) } == 0);
        Ok(Self(original))
    }
}

#[cfg(unix)]
impl Drop for TerminalEchoGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
        }
    }
}

fn confirm_action(
    yes: bool,
    prompt: &str,
    required_error: &str,
    canceled_error: &str,
) -> Result<()> {
    if yes {
        return Ok(());
    }
    let stdin = std::io::stdin();
    ensure!(stdin.is_terminal(), "{required_error}");
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    stdin.read_line(&mut answer)?;
    ensure!(
        matches!(answer.trim(), "y" | "Y" | "yes"),
        "{canceled_error}"
    );
    Ok(())
}

fn confirm_setup(yes: bool) -> Result<()> {
    confirm_action(
        yes,
        "Set up sync from this database?",
        "error sync-setup-confirmation-required hint=\"rerun with --yes to confirm\"",
        "error sync-setup-canceled",
    )
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

pub(crate) async fn setup_preview(database: &Database, config: &AppConfig) -> Result<SetupPreview> {
    let workspaces = database.list_workspaces().await?;
    let blob_dir = config::resolve_blob_dir(database.path(), config)?;
    let mut tasks = 0;
    for workspace in &workspaces {
        tasks += database.workspace_task_counts(&workspace.id).await?.visible;
    }
    Ok(SetupPreview {
        workspaces: workspaces.len(),
        tasks,
        missing_images: database
            .missing_setup_attachment_counts(&blob_dir)
            .await?
            .count,
        leaves_unencrypted_server: database.meta("sync_server_url").await?.is_some(),
    })
}

async fn print_setup_preview(database: &Database, config: &AppConfig, server: &str) -> Result<()> {
    let preview = setup_preview(database, config).await?;
    eprintln!("Set up sync from this database");
    eprintln!("  Database: {}", database.path().display());
    eprintln!("  Server: {server}");
    eprintln!(
        "  Workspaces: {}, non-deleted task records: {} (including scheduled and recurring occurrences)",
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
    let text = read_invitation("Setup invitation: ")?;
    let invitation = SetupInvitation::decode(&text).map_err(|error| {
        if DeviceInvitation::decode(&text).is_ok() {
            error.context("error sync-setup-invitation-device")
        } else {
            error
        }
    })?;
    let resuming = database.local_seed_genesis_commitment().await?.is_some();
    if !resuming {
        print_setup_preview(database, config, &invitation.server).await?;
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

fn explain_seed_claim_error(error: anyhow::Error) -> anyhow::Error {
    let setup_mismatch = error.chain().any(|cause| {
        cause
            .downcast_ref::<ProtectedLocalKeyStoreError>()
            .is_some_and(|error| error.kind() == ProtectedLocalKeyStoreErrorKind::SetupMismatch)
    });
    if setup_mismatch {
        error.context(
            "error sync-setup-invitation-mismatch hint=\"resume with the invitation that started setup\"",
        )
    } else {
        error
    }
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
            .map_err(explain_seed_claim_error)?;
        let setup = ClaimAuthentication::SetupSecret(&invitation.secret);
        let claim = match bootstrap.claim(seed.genesis(), setup).await {
            Ok(()) => Ok(()),
            Err(_) => {
                // A response may have been lost after admission. The pinned
                // seed bearer proves an exact retry without the invitation.
                let bearer = ClaimAuthentication::SeedBearer(seed.bearer());
                bootstrap.claim(seed.genesis(), bearer).await
            }
        };
        if let Err(error) = claim {
            if definite_setup_refusal(&error) {
                if database.seed_source_pin().await?.is_none() {
                    store.rollback_seed_claim(database, &seed).await?;
                    return Err(explain_setup_refusal(error));
                }
                if error.to_string() == "error bootstrap-storage-already-claimed" {
                    database
                        .mark_local_seed_setup_refused(error.to_string().as_str())
                        .await?;
                    return Err(error.context(
                        "error sync-setup-recovery-required hint=\"this fenced setup was definitely refused; back up this database and restore it to a new path for a local-only copy; local editing and export still work\"",
                    ));
                }
                return Err(explain_fenced_setup_refusal(error));
            }
            // An unknown outcome may already have admitted this exact claim.
            // Fence and freeze the same local snapshot so retrying is safe.
            store.prepare_seed_source(database).await?;
            database
                .capture_local_shared_state_for_setup(&blob_dir)
                .await?;
            store
                .package_seed_capture(database, &blob_dir, invitation.setup_id)
                .await?;
            return Err(error.context(
                "error sync-setup-outcome-unknown hint=\"the server claim couldn't be confirmed; resume continues the same setup\"",
            ));
        }
        store.prepare_seed_source(database).await?;
        database
            .capture_local_shared_state_for_setup(&blob_dir)
            .await?;
        store
            .package_seed_capture(database, &blob_dir, invitation.setup_id)
            .await?;
    }
    progress(Stage::UploadingData);
    if let Err(error) = bootstrap.resume(&store, database).await {
        if error.to_string() == "error bootstrap-storage-already-claimed"
            && database.seed_source_pin().await?.is_some()
        {
            database
                .mark_local_seed_setup_refused(error.to_string().as_str())
                .await?;
            return Err(error.context(
                "error sync-setup-recovery-required hint=\"this fenced setup was definitely refused; back up this database and restore it to a new path for a local-only copy; local editing and export still work\"",
            ));
        }
        return Err(error);
    }
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
    expires_at: u64,
    resumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InvitationStatus {
    pub(crate) expires_at: u64,
    pub(crate) keys_may_have_been_sent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cancellation {
    None,
    Cancelled,
    KeysMayHaveBeenSent { expires_at: u64 },
}

impl PendingInvitation {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub(crate) fn resumed(&self) -> bool {
        self.resumed
    }

    /// QR presentation of the invitation text.
    pub(crate) fn presentation(&self) -> Result<crate::pairing::PairingPresentation> {
        crate::pairing::PairingPresentation::new(&self.server, &self.text)
    }

    pub(crate) fn tui_presentation(&self) -> Result<crate::pairing::PairingPresentation> {
        crate::pairing::PairingPresentation::new_tui(&self.server, &self.text, self.expires_at)
    }
}

impl std::fmt::Debug for PendingInvitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingInvitation([REDACTED])")
    }
}

/// Creates a device invitation, or resumes the pending one. Sync keeps
/// running while it is open.
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
    let now = unix_now()?;
    let created = peer_enrollment_http::Client::new(&server)?
        .invite_with_status(&store, database, now + invitation_seconds())
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error withdrawal-required-unsupported" => error.context(
                "error sync-invitation-unresolved hint=\"keys may already have been sent with the previous invitation; invite again after that device joins, or after the invitation expires and the next `aven sync` changes keys\"",
            ),
            _ => explain_change_limit(error),
        });
    let created = track_access_result(database, created).await?;
    ensure!(
        created.state.expires_at > now,
        "error sync-invitation-unused hint=\"the open invitation has expired; run `aven sync` before creating another invitation\""
    );
    let text = DeviceInvitation {
        server: server.clone(),
        invitation: created.invitation,
    }
    .encode();
    Ok(PendingInvitation {
        server,
        text,
        deadline: Instant::now() + Duration::from_secs(created.state.expires_at - now),
        expires_at: created.state.expires_at,
        resumed: created.resumed,
    })
}

pub(crate) async fn invitation_status(database: &Database) -> Result<Option<InvitationStatus>> {
    if !is_set_up(database).await? {
        return Ok(None);
    }
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
    let Some((_, server)) = store.association(database).await? else {
        return Ok(None);
    };
    let inputs = store.active_inputs(database, &server).await?;
    Ok(store
        .open_invitation(database, &inputs)
        .await?
        .map(|state| InvitationStatus {
            expires_at: state.expires_at,
            keys_may_have_been_sent: state.keys_may_have_been_sent,
        }))
}

pub(crate) async fn cancel_invitation(
    database: &Database,
    config: &AppConfig,
) -> Result<Cancellation> {
    config.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
    let Some((_, server)) = store.association(database).await? else {
        bail!("error sync-setup-incomplete hint=\"rerun `aven sync setup`\"");
    };
    let client = peer_enrollment_http::Client::new(&server)?;
    let mut inputs = store.active_inputs(database, &server).await?;
    let (journal, state) = store.retire_open_invitation(database, &inputs).await?;
    let Some(state) = state else {
        return Ok(Cancellation::None);
    };
    let Some(journal) = journal else {
        return Ok(Cancellation::KeysMayHaveBeenSent {
            expires_at: state.expires_at,
        });
    };
    // Local retirement is the safety boundary. Server cancellation shortens
    // the joiner's wait, but failure to deliver it cannot permit admission.
    let _ = client
        .cancel(&store, database, &mut inputs, journal.handle)
        .await;
    Ok(Cancellation::Cancelled)
}

async fn poll_admission(database: &Database, invitation: &PendingInvitation) -> Result<bool> {
    let store = key_store(database)?;
    let client = peer_enrollment_http::Client::new(&invitation.server)?;
    let _guard = super::coordination::acquire(database).await?;
    match client.admit(&store, database).await {
        Err(error) if busy(&error) => Ok(false),
        result => track_access_result(database, result).await,
    }
}

/// Polls admission until the invited device joins or the invitation expires.
pub(crate) async fn await_admission(
    database: &Database,
    invitation: &PendingInvitation,
) -> Result<()> {
    while Instant::now() < invitation.deadline {
        if poll_admission(database, invitation).await? {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    bail!(
        "error sync-invitation-unused hint=\"the invitation expired unused; if keys may have been sent with it, the next `aven sync` changes keys before uploading new changes\""
    )
}

async fn await_admission_until_interrupt(
    database: &Database,
    invitation: &PendingInvitation,
) -> Result<bool> {
    // The listener runs during each network poll, but cancellation is acted on
    // only after that poll releases the coordination lock.
    let mut interrupt = tokio::spawn(tokio::signal::ctrl_c());
    while Instant::now() < invitation.deadline {
        match poll_admission(database, invitation).await {
            Ok(true) => {
                interrupt.abort();
                return Ok(true);
            }
            Ok(false) => {}
            Err(error) => {
                interrupt.abort();
                return Err(error);
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = &mut interrupt => return Ok(false),
        }
    }
    interrupt.abort();
    bail!(
        "error sync-invitation-unused hint=\"the invitation expired unused; if keys may have been sent with it, the next `aven sync` changes keys before uploading new changes\""
    )
}

pub(crate) async fn invite(database: &Database, config: &AppConfig) -> Result<()> {
    let invitation = create_invitation(database, config).await?;
    if invitation.resumed() {
        eprintln!(
            "Resuming the open invitation; it expires in {}.",
            format_duration(invitation.expires_at().saturating_sub(unix_now()?))
        );
    }
    println!("{}", invitation.text());
    std::io::stdout().flush()?;
    print_invitation_qr(&invitation);
    eprintln!("Anyone with this invitation can access all your synced data and manage devices.");
    eprintln!("Run `aven sync join` on the other device. Waiting for it to join...");
    if await_admission_until_interrupt(database, &invitation).await? {
        eprintln!("Device added");
        return Ok(());
    }
    let cancellation = tokio::select! {
        result = cancel_invitation(database, config) => result?,
        _ = tokio::signal::ctrl_c() => return Ok(()),
    };
    match cancellation {
        Cancellation::Cancelled => eprintln!("Invitation cancelled."),
        Cancellation::KeysMayHaveBeenSent { expires_at } => eprintln!(
            "Keys may already have been sent. The invitation remains open until {}; the next sync then changes keys.",
            format_expiry(expires_at)
        ),
        Cancellation::None => eprintln!("No invitation is open."),
    }
    Ok(())
}

fn format_duration(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    if minutes == 1 {
        "1 minute".to_string()
    } else {
        format!("{minutes} minutes")
    }
}

pub(crate) fn format_expiry(expires_at: u64) -> String {
    chrono::DateTime::from_timestamp(expires_at as i64, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|| expires_at.to_string())
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

/// Refuses joining unless this database is fresh or already joining. The
/// fresh check reads without changing the database.
pub(crate) async fn ensure_join_available(database: &Database, config: &AppConfig) -> Result<()> {
    config.ensure_sync_allowed()?;
    match local_phase(database).await? {
        LocalPhase::JoinIncomplete => Ok(()),
        LocalPhase::NotSetUp => {
            database
                .peer_target_preflight()
                .await
                .context(JOIN_REQUIRES_EMPTY)?;
            Ok(())
        }
        LocalPhase::SetupIncomplete | LocalPhase::SetupRecoveryRequired | LocalPhase::SetUp => {
            bail!(ALREADY_SET_UP)
        }
    }
}

/// A timeout proves neither expiry nor non-admission, and expiry alone does
/// not stop an admission committed before it, so the hint is conditional on
/// expiry and a replacement keeps the earlier invitation's admission usable.
const JOIN_TIMEOUT: &str = "error sync-join-timeout hint=\"the other device did not add this device in time; keep `aven sync invite` running there and rerun `aven sync join`. If the invitation expired, run `aven sync invite` again on the same device and pass the new invitation to `aven sync join --new-invitation`; an admission from the earlier invitation still completes the join\"";

const ALREADY_SET_UP: &str =
    "error sync-already-set-up hint=\"add devices with `aven sync invite` on this database\"";
const JOIN_REQUIRES_EMPTY: &str = "error sync-join-requires-empty-database hint=\"join with a new database, for example `aven --db PATH sync join`\"";

pub(crate) async fn join(database: &Database, config: &AppConfig, args: JoinArgs) -> Result<()> {
    ensure_join_available(database, config).await?;
    let resuming = local_phase(database).await? == LocalPhase::JoinIncomplete;
    let invitation = if resuming && !args.new_invitation {
        None
    } else {
        let text = read_invitation("Device invitation: ")?;
        ensure!(
            !text.trim().is_empty(),
            "error sync-join-invitation-required hint=\"paste the invitation from `aven sync invite`\""
        );
        let invitation = DeviceInvitation::decode(&text).map_err(|error| {
            if SetupInvitation::decode(&text).is_ok() {
                error.context("error sync-device-invitation-setup")
            } else {
                error
            }
        })?;
        eprintln!("Server: {}", invitation.server);
        confirm_action(
            args.yes,
            "Join this sync?",
            "error sync-join-confirmation-required hint=\"rerun with --yes to confirm\"",
            "error sync-join-canceled",
        )?;
        Some(invitation)
    };
    let (server, outcome) = run_join(
        database,
        config,
        || Ok(invitation),
        args.new_invitation,
        &|stage| match stage {
            Stage::WaitingForInviter => eprintln!("Waiting for the inviting device..."),
            Stage::DownloadingTasks => eprintln!("Downloading synced data..."),
            _ => {}
        },
    )
    .await
    .context("error sync-join-command")?;
    println!("Joined sync with {server}");
    print_outcome(&outcome);
    Ok(())
}

/// Joins sync on a fresh database, or resumes the join it started, and
/// returns the server. `invitation` is asked for only while enrollment is
/// unfinished; `None` resumes the request this database already made. With
/// `replace`, an invitation this join has not used becomes a new attempt
/// from the same device keys; earlier attempts stay able to complete.
pub(crate) async fn run_join(
    database: &Database,
    config: &AppConfig,
    invitation: impl FnOnce() -> Result<Option<DeviceInvitation>>,
    replace: bool,
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
            "error sync-join-server-mismatch hint=\"this database started joining with another server; use an invitation for that server\""
        );
        let (server, invitation) = match invitation {
            Some(invitation) => (invitation.server, Some(invitation.invitation)),
            None => (server.context("error sync-join-invitation-required")?, None),
        };
        let client = peer_enrollment_http::Client::new(&server)?;
        let deadline = Instant::now() + Duration::from_secs(invitation_seconds());
        await_join(
            &client, &store, database, invitation, replace, deadline, progress,
        )
        .await?;
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

/// Explains why a join request could not use this invitation. Every refusal
/// leaves the database, its device keys and earlier invitations unchanged.
fn explain_join_refusal(error: anyhow::Error) -> anyhow::Error {
    let hint = match error.to_string().as_str() {
        "error enrollment-invitation-conflict" => {
            "error sync-join-invitation-conflict hint=\"this database started joining with another invitation; rerun with that invitation to resume, or pass a new invitation from the same inviting device with `aven sync join --new-invitation`\""
        }
        "error enrollment-retry-context" => {
            "error sync-join-new-invitation-mismatch hint=\"a new invitation must come from the device that created the first one; run `aven sync invite` there\""
        }
        "error enrollment-retry-unavailable" => {
            "error sync-join-new-invitation-unavailable hint=\"joining already got past admission, so a new invitation cannot be used; rerun `aven sync join` with an invitation this database already used\""
        }
        "error enrollment-retry-limit" => {
            "error sync-join-new-invitation-limit hint=\"this database has reached its invitation limit; rerun `aven sync join` with an invitation it already used to finish if the other device accepted it, otherwise keep this database unchanged and join from a new empty database, for example `aven --db PATH sync join`\""
        }
        _ if error.to_string().starts_with("error shared-state-install") => {
            "error sync-join-target-not-empty hint=\"data was added to this database while joining, so it cannot finish joining; keep it unchanged and join from a new empty database, for example `aven --db PATH sync join`\""
        }
        _ => return error,
    };
    error.context(hint)
}

/// Requests admission and waits until a retained attempt completes or the
/// deadline passes. Local preparation failures are final. A busy post is
/// retried; a refused one ends waiting only when no earlier attempt could
/// still complete, since a refusal proves nothing about the others.
pub(crate) async fn await_join(
    client: &peer_enrollment_http::Client,
    store: &ProtectedLocalKeyStore,
    database: &Database,
    invitation: Option<Invitation>,
    replace: bool,
    deadline: Instant,
    progress: &(dyn Fn(Stage) + Sync),
) -> Result<()> {
    let peer = client
        .prepare(store, database, invitation, replace)
        .await
        .map_err(explain_join_refusal)?;
    let mut posted = client.post(&peer).await;
    progress(Stage::WaitingForInviter);
    let others = client.has_other_attempts(store, database).await?;
    loop {
        if posted.as_ref().is_err_and(busy) {
            posted = client.post(&peer).await;
        }
        match client.complete(store, database).await {
            Ok(true) => return Ok(()),
            Err(error) if !busy(&error) => return Err(error),
            _ => {}
        }
        let refused = posted.as_ref().is_err_and(|error| !busy(error));
        if (refused && !others) || Instant::now() >= deadline {
            return Err(match posted {
                Err(error) if refused && !others => error,
                Err(error) => error.context(JOIN_TIMEOUT),
                Ok(()) => anyhow::anyhow!(JOIN_TIMEOUT),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn definite_setup_refusal(error: &anyhow::Error) -> bool {
    matches!(
        error.to_string().as_str(),
        "error bootstrap-storage-already-claimed" | "error bootstrap-setup-invitation-rejected"
    )
}

fn explain_fenced_setup_refusal(error: anyhow::Error) -> anyhow::Error {
    match error.to_string().as_str() {
        "error bootstrap-setup-invitation-rejected" => error.context(
            "error sync-setup-fenced-invitation-rejected hint=\"this setup is already frozen; resume with the invitation that started setup or the newest invitation for that same server storage; if neither is available, back up this database and restore it to a new path for a local-only copy; local editing and export still work\"",
        ),
        _ => error,
    }
}

fn explain_setup_refusal(error: anyhow::Error) -> anyhow::Error {
    match error.to_string().as_str() {
        "error bootstrap-storage-already-claimed" => error.context(
            "error sync-setup-storage-already-claimed hint=\"this server already belongs to another sync; nothing here was changed; to use that sync, join it from an empty database\"",
        ),
        "error bootstrap-setup-invitation-rejected" => error.context(
            "error sync-setup-invitation-rejected hint=\"this setup invitation expired, was replaced, or is for different storage; nothing here was changed; run `aven server setup` for this unclaimed storage and try its current invitation\"",
        ),
        _ => error,
    }
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

/// The vault's lifetime budget of signed membership changes is spent.
const CHANGE_LIMIT: &str = "error sync-device-change-limit hint=\"this sync has reached its limit on device changes, so devices can no longer be added or removed; start a new sync to keep changing devices, see https://aventasks.dev/sync/#recover-from-device-loss\"";

fn explain_change_limit(error: anyhow::Error) -> anyhow::Error {
    if error
        .chain()
        .any(|cause| cause.to_string() == "error membership-change-limit")
    {
        error.context(CHANGE_LIMIT)
    } else {
        error
    }
}

/// A refusal alone proves neither a server failure nor removal of this device.
const REFUSED: &str = "error sync-server-refused hint=\"the server refused this request; it may have failed, or another device may have removed this device from sync, which leaves local tasks and images available here; retry later, and check `aven sync device list` on another device\"";

/// Explains engine refusals that ordinary rounds report while a join is
/// unfinished or new changes wait for a key change.
fn explain_round_error(error: anyhow::Error) -> anyhow::Error {
    match error.to_string().as_str() {
        "error snapshot-not-installed" | "error enrollment-unresolved" => {
            error.context("error sync-join-incomplete hint=\"rerun `aven sync join`\"")
        }
        "error enrollment-refused outcome-unknown" => error.context(REFUSED),
        "error withdrawal-rotation-required" => error.context(KEY_CHANGE_REQUIRED),
        _ => error,
    }
}

/// Changes from the server were downloaded; local changes stay queued.
const KEY_CHANGE_REQUIRED: &str = "error sync-key-change-required hint=\"an invitation expired after keys may have been sent to a device that never joined; this device downloads changes but uploads new ones only after sync changes keys; check the connection and run `aven sync` again\"";

async fn remember_access_refusal(database: &Database, error: &anyhow::Error) {
    if crate::sync::error_explanations::is_access_refusal(error)
        && let Err(state_error) = database.record_sync_access_refusal().await
    {
        tracing::warn!(
            error = %state_error,
            "could not persist sync access refusal"
        );
    }
}

async fn track_access_result<T>(database: &Database, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            remember_access_refusal(database, &error).await;
            Err(error)
        }
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
    let result = drain(&client, &store, database, &blob_dir, round_limit)
        .await
        .map_err(explain_round_error);
    match result {
        Ok(outcome) => {
            database.clear_sync_access_refusal().await?;
            Ok(outcome)
        }
        Err(error) => {
            remember_access_refusal(database, &error).await;
            Err(error)
        }
    }
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
    pub(crate) sent_changes: usize,
    pub(crate) received_changes: usize,
    pub(crate) conflicts: usize,
    pub(crate) new_conflicts: usize,
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
    let conflicts_before = conflict_identities(database).await?;
    super::device_label::publish_if_missing(database, store).await?;
    let mut last = None;
    let mut rounds = 0;
    let mut image_retries = 0;
    let mut sent_changes = 0;
    let mut received_changes = 0;
    let mut drain = Box::pin(client.start_drain(store, database)).await?;
    while rounds < round_limit {
        let round: Round =
            Box::pin(client.round_in_drain(store, database, blob_dir, &mut drain)).await?;
        on_round(&round);
        rounds += 1;
        sent_changes += round.sent_changes;
        received_changes += round.received_changes;
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
    if last.publishing_blocked {
        return Err(drain.publishing_blocked_error());
    }
    let conflicts_after = conflict_identities(database).await?;
    Ok(Outcome {
        version: 1,
        rounds,
        metadata_caught_up: last.metadata_caught_up,
        images: image_label(last.images),
        sent_changes,
        received_changes,
        conflicts: conflicts_after.len(),
        new_conflicts: conflicts_after.difference(&conflicts_before).count(),
    })
}

async fn conflict_identities(database: &Database) -> Result<HashSet<String>> {
    let mut identities = HashSet::new();
    for workspace in database.list_workspaces().await? {
        for conflict in database.list_conflicts(&workspace, None, None).await? {
            let (first, second) = if conflict.variant_a <= conflict.variant_b {
                (conflict.variant_a, conflict.variant_b)
            } else {
                (conflict.variant_b, conflict.variant_a)
            };
            identities.insert(format!(
                "{}:{}:{}:{}:{}:{}",
                workspace.id,
                conflict.recurrence_series,
                conflict.task_id,
                conflict.field,
                first,
                second
            ));
        }
    }
    Ok(identities)
}

fn print_outcome(outcome: &Outcome) {
    if !outcome.metadata_caught_up {
        println!(
            "Sync incomplete: sent {}, received {}; stopped after {} rounds. Run `aven sync` again.",
            outcome.sent_changes, outcome.received_changes, outcome.rounds
        );
        print_conflict_outcome(outcome);
        if outcome.images == "failed" {
            println!(
                "An image transfer failed, or an image added here is missing from this \
                 computer. Changes after it wait until it uploads."
            );
        }
        return;
    }
    if outcome.sent_changes == 0 && outcome.received_changes == 0 {
        println!("Tasks were already up to date");
    } else {
        println!(
            "Tasks: sent {}, received {}",
            outcome.sent_changes, outcome.received_changes
        );
    }
    print_conflict_outcome(outcome);
    match outcome.images {
        "complete" => println!("Images are up to date"),
        "unavailable" => println!("Some images are unavailable on the server"),
        "failed" => println!("Some image transfers failed. Run `aven sync` again to retry."),
        _ => println!("Images are still transferring. Run `aven sync` again."),
    }
}

fn print_conflict_outcome(outcome: &Outcome) {
    if outcome.conflicts == 0 {
        return;
    }
    let qualifier = if outcome.new_conflicts > 0 {
        format!(" ({} new)", outcome.new_conflicts)
    } else {
        String::new()
    };
    println!(
        "{} conflict{} need{} a decision{qualifier}. Run `aven conflict list`.",
        outcome.conflicts,
        if outcome.conflicts == 1 { "" } else { "s" },
        if outcome.conflicts == 1 { "s" } else { "" },
    );
}

#[derive(Serialize)]
pub(crate) struct StatusReport {
    version: u32,
    pub(crate) server: Option<String>,
    pub(crate) state: &'static str,
    local_changes_pending: Option<bool>,
    server_position: Option<i64>,
    initial_download_pending: Option<bool>,
    image_uploads_pending: Option<bool>,
    image_downloads_pending: Option<bool>,
    images_unavailable: Option<bool>,
    conflicts: i64,
    invitation: &'static str,
    invitation_expires_at: Option<u64>,
    access_refused_at: Option<String>,
}

/// Local observation only; contacts no server. Waits briefly for a running
/// sync or invitation exchange, which holds the installation exclusively.
pub(crate) async fn status_report(database: &Database) -> Result<StatusReport> {
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
        conflicts: database.unresolved_conflict_count().await?,
        invitation: "none",
        invitation_expires_at: None,
        access_refused_at: database
            .sync_access_refusal()
            .await?
            .map(|refusal| refusal.at),
    };
    if !is_set_up(database).await? {
        return Ok(report);
    }
    report.state = match local_phase(database).await? {
        LocalPhase::SetupRecoveryRequired => "setup-recovery-required",
        _ => "setup-incomplete",
    };
    let store = key_store(database)?;
    let _guard = super::coordination::acquire(database).await?;
    if report.state != "setup-recovery-required"
        && let Some((peer, server)) = store.association(database).await?
    {
        report.server = Some(server.clone());
        let enrollment_ready = !peer
            || matches!(
                store.enrollment_readiness(database).await?,
                EnrollmentReadiness::Enrolled { .. }
            );
        if enrollment_ready {
            let invitation_inputs = store.active_inputs(database, &server).await?;
            if let Some(invitation) = store.open_invitation(database, &invitation_inputs).await? {
                report.invitation = "open";
                report.invitation_expires_at = Some(invitation.expires_at);
            }
        }
        report.state = if !enrollment_ready {
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
                    if inputs.publishing_blocked()? {
                        "key-change-pending"
                    } else {
                        "ready"
                    }
                }
                Err(error) if peer && error.to_string() == "error snapshot-not-installed" => {
                    "join-incomplete"
                }
                Err(error) => return Err(error),
            }
        };
    }
    if report.access_refused_at.is_some()
        && !matches!(report.state, "setup-incomplete" | "join-incomplete")
    {
        report.state = "access-refused";
    }
    Ok(report)
}

pub(crate) fn status_state_words(state: &str) -> &'static str {
    match state {
        "not-set-up" => "not set up",
        "setup-incomplete" => "setup incomplete",
        "setup-recovery-required" => "setup recovery required",
        "join-incomplete" => "joining incomplete",
        "key-change-pending" => "key change pending",
        "access-refused" => "access unconfirmed",
        _ => "ready",
    }
}

pub(crate) async fn status(database: &Database, json: bool) -> Result<()> {
    let report = status_report(database).await?;
    if json {
        return print_json_pretty(&report);
    }
    if report.state == "not-set-up" {
        println!("Sync: not set up; this database is local only");
        println!(
            "Run `aven sync setup` with an invitation from `aven server setup`, or \
             `aven sync join` on a new database."
        );
        return Ok(());
    }
    println!("Sync: end-to-end encrypted");
    if let Some(server) = &report.server {
        println!("Server: {server}");
    }
    match report.state {
        "setup-incomplete" => println!("State: setup incomplete. Rerun `aven sync setup`."),
        "setup-recovery-required" => println!(
            "State: this setup was refused and cannot resume. Local editing and export still work. \
             Back up this database, then restore it to a new path for a local-only copy."
        ),
        "join-incomplete" => println!(
            "State: joining incomplete. Rerun `aven sync join`; if its invitation expired, \
             pass a new one from the same device with `aven sync join --new-invitation`."
        ),
        "key-change-pending" => println!(
            "State: an invitation expired after keys may have been sent to a device that \
             never joined. The next `aven sync` changes keys before uploading new changes; \
             data that device may already hold stays readable to it."
        ),
        "access-refused" => println!(
            "State: access unconfirmed. The server refused this device. It may have been \
             removed from sync; check from another device. Local tasks and images stay here."
        ),
        _ => println!("State: ready"),
    }
    if let Some(expires_at) = report.invitation_expires_at {
        println!("Invitation: open, expires {}", format_expiry(expires_at));
    } else {
        println!("Invitation: none");
    }
    if report.conflicts > 0 {
        println!(
            "Conflicts: {} need a decision. Run `aven conflict list`.",
            report.conflicts
        );
    } else {
        println!("Conflicts: none");
    }
    if let Some(pending) = report.local_changes_pending {
        println!(
            "Tasks: {}",
            if pending {
                "local changes waiting to sync"
            } else {
                "no local changes waiting"
            }
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
