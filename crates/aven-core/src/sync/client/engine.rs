//! End-to-end encrypted sync operations over the isolated encrypted transports.
//!
//! A database takes part in sync once it holds a seed genesis or enrollment
//! pin. Its server is the locator bound into protected enrollment identity;
//! configuration never redirects it. Other databases stay local until they are
//! set up or joined.
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use zeroize::Zeroizing;

use super::errors::{code, has_code, is_access_refusal};
use super::exchange::Link;
use super::host::{ClientHost, key_store};
use super::keys::peer::InvitationProgress;
use super::keys::{
    EnrollmentReadiness, ProtectedLocalKeyStore, ProtectedLocalKeyStoreError,
    ProtectedLocalKeyStoreErrorKind,
};
use super::tail::{ImageTransfer, Round};
use super::{DeviceInvitation, SetupInvitation, bootstrap, coordination, enrollment, tail};
use crate::db::Database;
use crate::sync::seed_claim::{ClaimAuthentication, membership::Invitation};

mod devices;
pub use devices::{
    Device, DeviceListing, Removal, finish_removal, load_devices, remove_other_device,
};
mod reset;
pub use reset::{Reset, reset};

/// Upper bound on bounded rounds in one interactive drain. A round may append
/// a bounded run of ordinary records, but still pulls one page and transfers at
/// most one image, so this primarily bounds catch-up and image work.
pub const ROUND_LIMIT: usize = 1000;
/// Consecutive failed or unavailable image rounds before a drain stops. Each
/// such round still pulls, but a failed local head cannot advance.
const IMAGE_RETRY_ROUNDS: usize = 16;
#[cfg(not(any(test, feature = "test-support")))]
fn invitation_seconds() -> u64 {
    600
}
// CLI test workers declare short-lived invitations to exercise expiry.
#[cfg(any(test, feature = "test-support"))]
fn invitation_seconds() -> u64 {
    std::env::var("AVEN_TEST_INVITATION_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(600)
}
#[cfg(not(any(test, feature = "test-support")))]
const INVITER_POLL_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(any(test, feature = "test-support"))]
const INVITER_POLL_INTERVAL: Duration = Duration::from_millis(100);
// Two half-second waits keep the joiner responsive across the inviter's
// one-second cadence before backing off to the steady interval.
#[cfg(not(any(test, feature = "test-support")))]
const JOIN_POLL_INTERVALS: [Duration; 5] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];
#[cfg(any(test, feature = "test-support"))]
const JOIN_POLL_INTERVALS: [Duration; 4] = [
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(75),
    Duration::from_millis(100),
];

fn join_poll_interval(attempt: usize) -> Duration {
    jittered(JOIN_POLL_INTERVALS[attempt.min(JOIN_POLL_INTERVALS.len() - 1)])
}

#[cfg(not(any(test, feature = "test-support")))]
fn jittered(interval: Duration) -> Duration {
    let millis = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
    let spread = millis / 10;
    let mut random = [0_u8; 2];
    if getrandom::fill(&mut random).is_err() {
        return interval;
    }
    let offset = u64::from(u16::from_le_bytes(random)) % (spread * 2 + 1);
    Duration::from_millis(millis - spread + offset)
}

#[cfg(any(test, feature = "test-support"))]
fn jittered(interval: Duration) -> Duration {
    interval
}

pub const NOT_SET_UP: &str = "error sync-not-set-up hint=\"run `aven sync setup` with an invitation from your hosting provider or `aven server setup`, or `aven sync join` on a new database\"";

/// True when this database takes part in sync, including an interrupted
/// setup or join.
pub async fn is_set_up(database: &Database) -> Result<bool> {
    Ok(database.enrollment_pin().await?.is_some() || has_seed_setup(database).await?)
}

/// A seed pin whose claim the server refused is kept only for an exact retry;
/// it doesn't make this database take part in sync.
async fn has_seed_setup(database: &Database) -> Result<bool> {
    Ok(database.local_seed_genesis_commitment().await?.is_some()
        && !database.local_seed_claim_refused().await?)
}

/// Where this database stands in sync, from database facts alone. Reads no
/// protected keys and takes no lock, so it describes rather than authorizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalPhase {
    NotSetUp,
    /// Setup started from this database and has not bound the server yet.
    SetupIncomplete,
    /// Joining started and the synced data has not been installed yet.
    JoinIncomplete,
    SetUp,
}

pub async fn local_phase(database: &Database) -> Result<LocalPhase> {
    Ok(match database.enrollment_pin().await? {
        Some((_, _, role)) if role == "peer" => {
            if database.meta("e2ee_association").await?.is_some() {
                LocalPhase::SetUp
            } else {
                LocalPhase::JoinIncomplete
            }
        }
        Some(_) => LocalPhase::SetUp,
        None if has_seed_setup(database).await? => LocalPhase::SetupIncomplete,
        None => LocalPhase::NotSetUp,
    })
}

/// Setup and joining progress that a caller may present: the current stage
/// and, for stages that measure it, the work completed so far in this attempt.
/// Amounts count only work this attempt performed, never item contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    pub amount: Option<Amount>,
}

impl From<Stage> for Progress {
    fn from(stage: Stage) -> Self {
        Self {
            stage,
            amount: None,
        }
    }
}

/// Completed work within one stage. Counts never decrease within a stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Amount {
    /// Encrypted bytes transferred. `total` is exact once known and does not
    /// change afterwards.
    Bytes { done: u64, total: Option<u64> },
    /// Metadata changes applied; the number still on the server is unknown.
    Changes { done: u64 },
    /// Images installed and images still waiting. Newly synced images can
    /// raise `remaining`, so it is not a fixed total.
    Images { done: u64, remaining: u64 },
}

/// Where setup or joining is. Stages report position, not how much remains;
/// [`Progress::amount`] carries measured work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
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

pub fn unix_now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

/// What setup would publish from this database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupPreview {
    pub workspaces: usize,
    pub tasks: i64,
    /// Synced images whose bytes are missing here; they sync as unavailable.
    pub missing_images: u64,
    /// The database still names a server from earlier unencrypted sync.
    pub leaves_unencrypted_server: bool,
}

pub async fn setup_preview(database: &Database, host: &dyn ClientHost) -> Result<SetupPreview> {
    let workspaces = database.list_workspaces().await?;
    let blob_dir = host.blob_dir(database)?;
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

/// Refuses setup when sync is disabled or this database already takes part.
pub async fn ensure_setup_available(database: &Database, host: &dyn ClientHost) -> Result<()> {
    host.ensure_sync_allowed()?;
    ensure!(
        database.enrollment_pin().await?.is_none(),
        "error sync-already-set-up hint=\"run `aven sync` or `aven sync status`\""
    );
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
pub async fn run_setup(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
    invitation: &SetupInvitation,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<Outcome> {
    Ok(setup(link, database, host, Some(invitation), progress)
        .await?
        .1)
}

/// The code of a [`resume_setup`] refusal that a setup invitation can
/// continue: without the setup secret the server's refusal leaves open
/// whether this claim can still be made.
pub const SETUP_INVITATION_REQUIRED: &str = "sync-setup-resume-invitation-required";

/// Resumes the setup this database started from its retained server origin,
/// setup ID and seed authority, without the setup invitation, and returns
/// the server. Only a claim the server already holds accepts the seed bearer;
/// otherwise this fails with [`SETUP_INVITATION_REQUIRED`] and changes
/// nothing.
pub async fn resume_setup(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<(String, Outcome)> {
    setup(link, database, host, None, progress).await
}

async fn setup(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
    invitation: Option<&SetupInvitation>,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<(String, Outcome)> {
    let _total = bootstrap::StageTimer::start("setup_total");
    ensure_setup_available(database, host).await?;
    let blob_dir = host.blob_dir(database)?;
    let store = {
        let _timer = bootstrap::StageTimer::start("key_store");
        key_store(host, database).await?
    };
    let _guard = coordination::acquire(database).await?;
    let (server, setup_id, secret) = match invitation {
        Some(invitation) => {
            store.bind_seed_origin(database, &invitation.server).await?;
            (
                invitation.server.clone(),
                invitation.setup_id,
                Some(&invitation.secret),
            )
        }
        None => {
            ensure!(
                local_phase(database).await? == LocalPhase::SetupIncomplete,
                "error sync-setup-not-started hint=\"rerun `aven sync setup`\""
            );
            let (server, setup_id) = store.retained_seed_setup(database).await?;
            (server, setup_id, None)
        }
    };
    let bootstrap = bootstrap::Client::new(&server, link.clone())?;
    let mut proof = None;
    // A sealed publication intent means the claim and capture are complete.
    if database.seed_publication_intent_bytes().await?.is_none() {
        progress(Stage::PreparingData.into());
        let timer = bootstrap::StageTimer::start("prepare_claim");
        let seed = store
            .prepare_seed_claim(database, setup_id)
            .await
            .map_err(explain_seed_claim_error)?;
        database.clear_local_seed_claim_refused().await?;
        drop(timer);
        let timer = bootstrap::StageTimer::start("claim");
        let bearer = ClaimAuthentication::SeedBearer(seed.bearer());
        let claim = match secret {
            Some(secret) => {
                let setup = ClaimAuthentication::SetupSecret(secret);
                match bootstrap.claim(seed.genesis(), setup).await {
                    Ok(()) => Ok(()),
                    // A response may have been lost after admission. The pinned
                    // seed bearer proves an exact retry without the invitation.
                    Err(_) => bootstrap.claim(seed.genesis(), bearer).await,
                }
            }
            None => bootstrap.claim(seed.genesis(), bearer).await,
        };
        drop(timer);
        if let Err(error) = claim {
            // Refusals are unauthenticated and may hide a committed claim, so
            // the seed authority stays for an exact retry and nothing is fenced.
            if setup_refusal(&error) {
                let fenced = database.seed_source_pin().await?.is_some();
                // Without the setup secret, a refusal can't show that the
                // claim is lost; the setup invitation may still make it.
                if secret.is_none()
                    && (!fenced || has_code(&error, "bootstrap-setup-invitation-rejected"))
                {
                    return Err(error.context(format!("error {SETUP_INVITATION_REQUIRED}")));
                }
                if !fenced {
                    database
                        .mark_local_seed_claim_refused(error.to_string().as_str())
                        .await?;
                    return Err(explain_setup_refusal(error));
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
                .package_seed_capture_validated(database, &blob_dir, setup_id, None)
                .await?;
            return Err(error.context(
                "error sync-setup-outcome-unknown hint=\"the server claim couldn't be confirmed; resume continues the same setup\"",
            ));
        }
        {
            let _timer = bootstrap::StageTimer::start("prepare_source");
            store.prepare_seed_source(database).await?;
        }
        let capture = {
            let _timer = bootstrap::StageTimer::start("capture");
            database
                .capture_local_shared_state_for_setup(&blob_dir)
                .await?
        };
        let _timer = bootstrap::StageTimer::start("package");
        proof = Some(
            store
                .package_seed_capture_validated(database, &blob_dir, setup_id, Some(capture))
                .await?,
        );
    }
    progress(Stage::UploadingData.into());
    bootstrap
        .resume_validated(&store, database, proof, &|done, total| {
            progress(Progress {
                stage: Stage::UploadingData,
                amount: Some(Amount::Bytes {
                    done,
                    total: Some(total),
                }),
            })
        })
        .await
        .map_err(explain_fenced_setup_refusal)?;
    progress(Stage::FinishingSetup.into());
    // Binds this installation's enrollment identity to the server.
    {
        let _timer = bootstrap::StageTimer::start("enrollment");
        enrollment::Client::new(&server, link.clone())?
            .refresh(&store, database)
            .await?;
    }
    let _timer = bootstrap::StageTimer::start("drain");
    let client = tail::Client::new(&server, link.clone())?;
    let outcome = drain(&client, &store, database, host, &blob_dir, ROUND_LIMIT).await?;
    Ok((server, outcome))
}

/// A created device invitation whose inviting device waits for admission.
pub struct PendingInvitation {
    server: String,
    text: Zeroizing<String>,
    handle: [u8; 32],
    vault: [u8; 32],
    deadline: Instant,
    expires_at: u64,
    resumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvitationStatus {
    pub expires_at: u64,
    pub keys_may_have_been_sent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cancellation {
    None,
    Cancelled,
    KeysMayHaveBeenSent { expires_at: u64 },
}

impl PendingInvitation {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// The server that admits the invited device.
    pub fn server(&self) -> &str {
        &self.server
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(server: &str, text: &str, expires_at: u64) -> Self {
        Self {
            server: server.to_string(),
            text: Zeroizing::new(text.to_string()),
            handle: [0; 32],
            vault: [0; 32],
            deadline: Instant::now(),
            expires_at,
            resumed: false,
        }
    }
}

impl std::fmt::Debug for PendingInvitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingInvitation([REDACTED])")
    }
}

/// Creates a device invitation, or resumes the pending one. Sync keeps
/// running while it is open.
pub async fn create_invitation(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
) -> Result<PendingInvitation> {
    host.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let store = key_store(host, database).await?;
    let _guard = coordination::acquire(database).await?;
    let Some((_, server)) = store.association(database).await? else {
        bail!("error sync-setup-incomplete hint=\"rerun `aven sync setup`\"");
    };
    // Checked before registering, so an origin the invitation text can't
    // carry never leaves an open invitation behind.
    ensure!(
        server.len() <= super::MAX_SERVER_BYTES,
        "error sync-server-url-too-long hint=\"invitations need a server origin of at most 255 bytes\""
    );
    let now = unix_now()?;
    let created = enrollment::Client::new(&server, link.clone())?
        .invite_with_status(&store, database, now + invitation_seconds())
        .await
        .map_err(|error| match code(&error).as_deref() {
            Some("withdrawal-required-unsupported") => error.context(
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
    .encode()?;
    Ok(PendingInvitation {
        server,
        text,
        handle: created.state.handle,
        vault: created.vault,
        deadline: Instant::now() + Duration::from_secs(created.state.expires_at - now),
        expires_at: created.state.expires_at,
        resumed: created.resumed,
    })
}

/// The sync server, verified member count, and open invitation.
/// Local observation only; contacts no server.
#[derive(Debug, Default)]
pub struct AssociationStatus {
    pub server: Option<String>,
    pub devices: Option<usize>,
    pub invitation: Option<InvitationStatus>,
}

pub async fn association_status(
    database: &Database,
    host: &dyn ClientHost,
) -> Result<AssociationStatus> {
    if !is_set_up(database).await? {
        return Ok(AssociationStatus::default());
    }
    let store = key_store(host, database).await?;
    let _guard = coordination::acquire(database).await?;
    read_association(database, &store).await
}

/// Like [`association_status`], but `None` without waiting while another
/// sync operation holds this database, so periodic observers stay responsive.
pub async fn try_association_status(
    database: &Database,
    host: &dyn ClientHost,
) -> Result<Option<AssociationStatus>> {
    if !is_set_up(database).await? {
        return Ok(Some(AssociationStatus::default()));
    }
    let Some(_guard) = coordination::try_acquire(database)? else {
        return Ok(None);
    };
    let store = key_store(host, database).await?;
    read_association(database, &store).await.map(Some)
}

async fn read_association(
    database: &Database,
    store: &ProtectedLocalKeyStore,
) -> Result<AssociationStatus> {
    let Some((_, server)) = store.association(database).await? else {
        return Ok(AssociationStatus::default());
    };
    let inputs = store.active_inputs(database, &server).await?;
    let invitation = store
        .open_invitation(database, &inputs)
        .await?
        .map(|state| InvitationStatus {
            expires_at: state.expires_at,
            keys_may_have_been_sent: state.keys_may_have_been_sent,
        });
    Ok(AssociationStatus {
        server: Some(server),
        devices: Some(inputs.membership.device_count()),
        invitation,
    })
}

pub async fn cancel_invitation(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
) -> Result<Cancellation> {
    host.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let store = key_store(host, database).await?;
    let _guard = coordination::acquire(database).await?;
    let Some((_, server)) = store.association(database).await? else {
        bail!("error sync-setup-incomplete hint=\"rerun `aven sync setup`\"");
    };
    let client = enrollment::Client::new(&server, link.clone())?;
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

/// How a wait for the invited device ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Admitted,
    /// Another command retired or withdrew the invitation.
    Cancelled,
}

const INVITATION_UNUSED: &str = "error sync-invitation-unused hint=\"the invitation expired unused; if keys may have been sent with it, the next `aven sync` changes keys before uploading new changes\"";

enum AdmissionPoll {
    Waiting,
    RequestPending,
    Complete(Admission),
}

/// Checks this invitation's local state, then the server mailbox, and admits
/// only once a join request is waiting, so an idle wait stays cheap.
async fn poll_admission(
    database: &Database,
    host: &dyn ClientHost,
    client: &enrollment::Client,
    invitation: &PendingInvitation,
) -> Result<AdmissionPoll> {
    let store = key_store(host, database).await?;
    let _guard = coordination::acquire(database).await?;
    match store
        .invitation_progress(database, &invitation.handle)
        .await?
    {
        InvitationProgress::Admitted => {
            return Ok(AdmissionPoll::Complete(Admission::Admitted));
        }
        InvitationProgress::Closed => {
            return Ok(AdmissionPoll::Complete(Admission::Cancelled));
        }
        InvitationProgress::Open => {}
    }
    match client
        .join_requested(invitation.vault, invitation.handle)
        .await
    {
        Ok(true) => {}
        Ok(false) => return Ok(AdmissionPoll::Waiting),
        Err(error) if busy(&error) => return Ok(AdmissionPoll::Waiting),
        Err(error) => return Err(error),
    }
    match client
        .admit_handle(&store, database, Some(invitation.handle))
        .await
    {
        Err(error) if busy(&error) => Ok(AdmissionPoll::RequestPending),
        result => Ok(if track_access_result(database, result).await? {
            AdmissionPoll::Complete(Admission::Admitted)
        } else {
            AdmissionPoll::RequestPending
        }),
    }
}

/// Polls admission until the invited device joins, the invitation is
/// cancelled, or it expires.
pub async fn await_admission(
    link: Link,
    host: &dyn ClientHost,
    database: &Database,
    invitation: &PendingInvitation,
) -> Result<Admission> {
    let client = enrollment::Client::new(&invitation.server, link.clone())?;
    let mut pending_attempt = None;
    while Instant::now() < invitation.deadline {
        match poll_admission(database, host, &client, invitation).await? {
            AdmissionPoll::Complete(admission) => return Ok(admission),
            AdmissionPoll::RequestPending => {
                pending_attempt.get_or_insert(0);
            }
            AdmissionPoll::Waiting => {}
        }
        let delay = match pending_attempt.as_mut() {
            Some(attempt) => {
                let delay = join_poll_interval(*attempt);
                *attempt = attempt.saturating_add(1);
                delay
            }
            None => jittered(INVITER_POLL_INTERVAL),
        };
        link.wait(delay).await;
    }
    bail!(INVITATION_UNUSED)
}

/// Refuses joining unless this database is fresh or already joining. The
/// fresh check reads without changing the database.
pub async fn ensure_join_available(database: &Database, host: &dyn ClientHost) -> Result<()> {
    host.ensure_sync_allowed()?;
    match local_phase(database).await? {
        LocalPhase::JoinIncomplete => Ok(()),
        LocalPhase::NotSetUp => {
            database
                .peer_target_preflight()
                .await
                .context(JOIN_REQUIRES_EMPTY)?;
            Ok(())
        }
        LocalPhase::SetupIncomplete | LocalPhase::SetUp => {
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

/// Joins sync on a fresh database, or resumes the join it started, and
/// returns the server. `invitation` is asked for only while enrollment is
/// unfinished; `None` resumes the request this database already made. With
/// `replace`, an invitation this join has not used becomes a new attempt
/// from the same device keys; earlier attempts stay able to complete.
pub async fn run_join(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
    invitation: impl FnOnce() -> Result<Option<DeviceInvitation>>,
    replace: bool,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<(String, Outcome)> {
    host.ensure_sync_allowed()?;
    let store = key_store(host, database).await?;
    let blob_dir = host.blob_dir(database)?;
    let _guard = coordination::acquire(database).await?;
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
        let client = enrollment::Client::new(&server, link.clone())?;
        let deadline = Instant::now() + Duration::from_secs(invitation_seconds());
        await_join(
            &client, &store, database, invitation, replace, deadline, progress,
        )
        .await?;
        server
    };
    progress(Stage::DownloadingTasks.into());
    enrollment::Client::new(&server, link.clone())?
        .install_reporting(&store, database, &|done, total| {
            progress(Progress {
                stage: Stage::DownloadingTasks,
                amount: Some(Amount::Bytes { done, total }),
            })
        })
        .await?;
    progress(Stage::CatchingUp.into());
    let client = tail::Client::new(&server, link.clone())?;
    let outcome = drain_reporting(
        &client,
        &store,
        database,
        host,
        &blob_dir,
        ROUND_LIMIT,
        Some(progress),
    )
    .await?;
    Ok((server, outcome))
}

/// Explains why a join request could not use this invitation. Every refusal
/// leaves the database, its device keys and earlier invitations unchanged.
fn explain_join_refusal(error: anyhow::Error) -> anyhow::Error {
    let hint = match code(&error).as_deref() {
        Some("enrollment-invitation-conflict") => {
            "error sync-join-invitation-conflict hint=\"this database started joining with another invitation; rerun with that invitation to resume, or pass a new invitation from the same inviting device with `aven sync join --new-invitation`\""
        }
        Some("enrollment-retry-context") => {
            "error sync-join-new-invitation-mismatch hint=\"a new invitation must come from the device that created the first one; run `aven sync invite` there\""
        }
        Some("enrollment-retry-unavailable") => {
            "error sync-join-new-invitation-unavailable hint=\"joining already got past admission, so a new invitation cannot be used; rerun `aven sync join` with an invitation this database already used\""
        }
        Some("enrollment-retry-limit") => {
            "error sync-join-new-invitation-limit hint=\"this database has reached its invitation limit; rerun `aven sync join` with an invitation it already used to finish if the other device accepted it, otherwise keep this database unchanged and join from a new empty database, for example `aven --db PATH sync join`\""
        }
        Some("shared-state-install") => {
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
pub async fn await_join(
    client: &enrollment::Client,
    store: &ProtectedLocalKeyStore,
    database: &Database,
    invitation: Option<Invitation>,
    replace: bool,
    deadline: Instant,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<()> {
    let peer = client
        .prepare(store, database, invitation, replace)
        .await
        .map_err(explain_join_refusal)?;
    let mut posted = client.post(&peer).await;
    progress(Stage::WaitingForInviter.into());
    let others = client.has_other_attempts(store, database).await?;
    let mut poll_attempt = 0_usize;
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
        client.link().wait(join_poll_interval(poll_attempt)).await;
        poll_attempt = poll_attempt.saturating_add(1);
    }
}

fn setup_refusal(error: &anyhow::Error) -> bool {
    matches!(
        code(error).as_deref(),
        Some(
            "bootstrap-storage-already-claimed"
                | "bootstrap-setup-invitation-rejected"
                | "bootstrap-setup-invitation-expired"
        )
    )
}

fn explain_fenced_setup_refusal(error: anyhow::Error) -> anyhow::Error {
    match code(&error).as_deref() {
        Some("bootstrap-setup-invitation-rejected" | "bootstrap-setup-invitation-expired") => error.context(
            "error sync-setup-fenced-invitation-rejected hint=\"this setup is already frozen; resume with the invitation that started setup or the newest invitation for that same server storage; if neither is available, back up this database and restore it to a new path for a local-only copy; local editing and export still work\"",
        ),
        Some("bootstrap-storage-already-claimed") => error.context(
            "error sync-setup-fenced-storage-claimed hint=\"the server reported that this storage belongs to another sync; this setup is frozen and resuming retries it; if it keeps failing, back up this database and restore it to a new path for a local-only copy; local editing and export still work\"",
        ),
        _ => error,
    }
}

fn explain_setup_refusal(error: anyhow::Error) -> anyhow::Error {
    match code(&error).as_deref() {
        Some("bootstrap-storage-already-claimed") => error.context(
            "error sync-setup-storage-already-claimed hint=\"this server already belongs to another sync; nothing here was changed; to use that sync, join it from an empty database\"",
        ),
        Some("bootstrap-setup-invitation-rejected") => error.context(
            "error sync-setup-invitation-rejected hint=\"this setup invitation expired, was replaced, or is for different storage; nothing here was changed; get the current invitation from your hosting provider, or run `aven server setup` if you host the server yourself\"",
        ),
        Some("bootstrap-setup-invitation-expired") => error.context(
            "error sync-setup-invitation-expired hint=\"this setup invitation expired; nothing here was changed; get a new invitation from your hosting provider, or run `aven server setup` again if you host the server yourself\"",
        ),
        _ => error,
    }
}

/// The enrollment server serves one exchange at a time; pollers retry.
fn busy(error: &anyhow::Error) -> bool {
    has_code(error, "enrollment-busy")
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
const CHANGE_LIMIT: &str = "error sync-device-change-limit hint=\"this sync has reached its limit on device changes, so devices can no longer be added or removed; start a new sync to keep changing devices, see https://aventasks.dev/sync/setup/#recover-from-device-loss\"";

fn explain_change_limit(error: anyhow::Error) -> anyhow::Error {
    if has_code(&error, "membership-change-limit") {
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
    match code(&error).as_deref() {
        Some("snapshot-not-installed" | "enrollment-unresolved") => {
            error.context("error sync-join-incomplete hint=\"rerun `aven sync join`\"")
        }
        Some("enrollment-unauthorized") => error.context(REFUSED),
        Some("withdrawal-rotation-required") => error.context(KEY_CHANGE_REQUIRED),
        _ => error,
    }
}

/// Changes from the server were downloaded; local changes stay queued.
const KEY_CHANGE_REQUIRED: &str = "error sync-key-change-required hint=\"an invitation expired after keys may have been sent to a device that never joined; this device downloads changes but uploads new ones only after sync changes keys; check the connection and run `aven sync` again\"";

async fn remember_access_refusal(database: &Database, error: &anyhow::Error) {
    if is_access_refusal(error)
        && let Err(state_error) = database.record_sync_access_refusal().await
    {
        tracing::warn!(
            error = %state_error,
            "could not persist sync access refusal"
        );
    }
}

/// Any authenticated server success proves current access.
async fn track_access_result<T>(database: &Database, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            database.clear_sync_access_refusal().await?;
            Ok(value)
        }
        Err(error) => {
            remember_access_refusal(database, &error).await;
            Err(error)
        }
    }
}

/// Drains up to `round_limit` rounds with the server bound during setup or
/// join. The caller holds the sync coordination lock.
async fn drain_associated(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
    round_limit: usize,
) -> Result<Outcome> {
    let store = key_store(host, database).await?;
    let blob_dir = host.blob_dir(database)?;
    let server = associated_server(&store, database).await?;
    let client = tail::Client::new(&server, link.clone())?;
    let result = drain(&client, &store, database, host, &blob_dir, round_limit)
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
pub async fn run_to_completion(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
) -> Result<Outcome> {
    host.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let _guard = coordination::acquire(database).await?;
    drain_associated(link, database, host, ROUND_LIMIT).await
}

pub enum DaemonRound {
    Completed(Outcome),
    /// Another process holds the sync coordination lock.
    Deferred,
    /// The database has not been set up or joined; it stays local.
    NotSetUp,
}

/// One bounded daemon round that never waits for another sync.
pub async fn daemon_round(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
    round_limit: usize,
) -> Result<DaemonRound> {
    host.ensure_sync_allowed()?;
    if !is_set_up(database).await? {
        return Ok(DaemonRound::NotSetUp);
    }
    let Some(_guard) = coordination::try_acquire(database)? else {
        return Ok(DaemonRound::Deferred);
    };
    drain_associated(link, database, host, round_limit)
        .await
        .map(DaemonRound::Completed)
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub version: u32,
    pub rounds: usize,
    pub metadata_caught_up: bool,
    pub images: ImageTransfer,
    pub sent_changes: usize,
    pub received_changes: usize,
    pub conflicts: usize,
    pub new_conflicts: usize,
}

impl Outcome {
    /// True when another round would likely make progress now: images are
    /// still transferring, or metadata remains while images are not blocked.
    pub fn more_work_ready(&self) -> bool {
        self.images == ImageTransfer::Pending
            || (!self.metadata_caught_up && self.images == ImageTransfer::Complete)
    }
}

/// Repeats bounded rounds until metadata is current and image work settles,
/// or until `round_limit` or the image retry bound stops it.
pub async fn drain(
    client: &tail::Client,
    store: &ProtectedLocalKeyStore,
    database: &Database,
    host: &dyn ClientHost,
    blob_dir: &Path,
    round_limit: usize,
) -> Result<Outcome> {
    drain_reporting(client, store, database, host, blob_dir, round_limit, None).await
}

async fn drain_reporting(
    client: &tail::Client,
    store: &ProtectedLocalKeyStore,
    database: &Database,
    host: &dyn ClientHost,
    blob_dir: &Path,
    round_limit: usize,
    progress: Option<&(dyn Fn(Progress) + Sync)>,
) -> Result<Outcome> {
    let conflicts_before = conflict_identities(database).await?;
    super::device_label::publish_if_missing(database, store, host).await?;
    let mut last = None;
    let mut rounds = 0;
    let mut image_retries = 0;
    let mut sent_changes = 0;
    let mut received_changes = 0;
    let mut images_progress = None;
    let lifecycle_policy = host.attachment_lifecycle_policy();
    let mut drain = Box::pin(client.start_drain(store, database)).await?;
    while rounds < round_limit {
        let round: Round = Box::pin(client.round_in_drain(
            store,
            database,
            blob_dir,
            lifecycle_policy,
            &mut drain,
        ))
        .await?;
        rounds += 1;
        sent_changes += round.sent_changes;
        received_changes += round.received_changes;
        if let Some(progress) = progress {
            images_progress = report_round(
                database,
                &round,
                received_changes,
                images_progress,
                progress,
            )
            .await?;
        }
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
        images: last.images,
        sent_changes,
        received_changes,
        conflicts: conflicts_after.len(),
        new_conflicts: conflicts_after.difference(&conflicts_before).count(),
    })
}

/// Reports catch-up or image progress after a round. Images count as done
/// when the number waiting drops, so a newly synced image never lowers the
/// count, and a failed transfer leaves it unchanged. Counting waiting images
/// costs one local query, made only while images are outstanding.
async fn report_round(
    database: &Database,
    round: &Round,
    received_changes: usize,
    images: Option<(u64, u64)>,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<Option<(u64, u64)>> {
    if !round.metadata_caught_up {
        progress(Progress {
            stage: Stage::CatchingUp,
            amount: Some(Amount::Changes {
                done: received_changes as u64,
            }),
        });
        return Ok(images);
    }
    if round.images == ImageTransfer::Complete {
        return Ok(images);
    }
    let remaining = database.encrypted_image_downloads_remaining().await?;
    let done = images_done(images, remaining);
    progress(Progress {
        stage: Stage::DownloadingImages,
        amount: Some(Amount::Images { done, remaining }),
    });
    Ok(Some((done, remaining)))
}

/// Images done after a round, given the previous `(done, remaining)` report.
fn images_done(previous: Option<(u64, u64)>, remaining: u64) -> u64 {
    previous.map_or(0, |(done, before)| done + before.saturating_sub(remaining))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncState {
    NotSetUp,
    SetupIncomplete,
    JoinIncomplete,
    KeyChangePending,
    Ready,
    AccessRefused,
}

#[derive(Serialize)]
pub struct StatusReport {
    pub version: u32,
    /// Member count from local verified membership, or unknown before enrollment.
    pub devices: Option<usize>,
    pub server: Option<String>,
    pub state: SyncState,
    pub local_changes_pending: Option<bool>,
    pub server_position: Option<i64>,
    pub initial_download_pending: Option<bool>,
    pub image_uploads_pending: Option<bool>,
    pub image_downloads_pending: Option<bool>,
    pub images_unavailable: Option<bool>,
    pub conflicts: i64,
    pub invitation: &'static str,
    pub invitation_expires_at: Option<u64>,
    pub access_refused_at: Option<String>,
}

/// Local observation only; contacts no server. Waits briefly for a running
/// sync or invitation exchange, which holds the installation exclusively.
pub async fn status_report(database: &Database, host: &dyn ClientHost) -> Result<StatusReport> {
    let mut report = StatusReport {
        version: 1,
        devices: None,
        server: None,
        state: SyncState::NotSetUp,
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
    report.state = SyncState::SetupIncomplete;
    let store = key_store(host, database).await?;
    let _guard = coordination::acquire(database).await?;
    if let Some((peer, server)) = store.association(database).await? {
        report.server = Some(server.clone());
        let enrollment_ready = !peer
            || matches!(
                store.enrollment_readiness(database).await?,
                EnrollmentReadiness::Enrolled { .. }
            );
        if enrollment_ready {
            let invitation_inputs = store.active_inputs(database, &server).await?;
            report.devices = Some(invitation_inputs.membership.device_count());
            if let Some(invitation) = store.open_invitation(database, &invitation_inputs).await? {
                report.invitation = "open";
                report.invitation_expires_at = Some(invitation.expires_at);
            }
        }
        report.state = if !enrollment_ready {
            SyncState::JoinIncomplete
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
                        SyncState::KeyChangePending
                    } else {
                        SyncState::Ready
                    }
                }
                Err(error) if peer && has_code(&error, "snapshot-not-installed") => {
                    SyncState::JoinIncomplete
                }
                Err(error) => return Err(error),
            }
        };
    }
    if report.access_refused_at.is_some()
        && !matches!(
            report.state,
            SyncState::SetupIncomplete | SyncState::JoinIncomplete
        )
    {
        report.state = SyncState::AccessRefused;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::client::errors::has_code;

    #[test]
    fn enrollment_polling_backs_off_to_a_bounded_steady_interval() {
        assert_eq!(join_poll_interval(0), Duration::from_millis(25));
        assert_eq!(join_poll_interval(1), Duration::from_millis(50));
        assert_eq!(join_poll_interval(2), Duration::from_millis(75));
        assert_eq!(join_poll_interval(3), Duration::from_millis(100));
        assert_eq!(join_poll_interval(usize::MAX), Duration::from_millis(100));
        assert_eq!(jittered(INVITER_POLL_INTERVAL), Duration::from_millis(100));
    }

    #[test]
    fn images_done_counts_only_drops_in_remaining_images() {
        assert_eq!(images_done(None, 10), 0);
        assert_eq!(images_done(Some((0, 10)), 9), 1);
        // A failed transfer leaves the count unchanged.
        assert_eq!(images_done(Some((1, 9)), 9), 1);
        // Newly synced images raise what remains, never lower what is done.
        assert_eq!(images_done(Some((1, 9)), 12), 1);
        assert_eq!(images_done(Some((1, 12)), 11), 2);
    }

    #[test]
    fn seed_claim_only_labels_a_real_setup_mismatch_as_invitation_mismatch() {
        let mismatch = anyhow::Error::new(ProtectedLocalKeyStoreError::new(
            ProtectedLocalKeyStoreErrorKind::SetupMismatch,
        ));
        let mismatch = explain_seed_claim_error(mismatch);
        assert!(
            has_code(&mismatch, "sync-setup-invitation-mismatch"),
            "{mismatch:#}"
        );
        for kind in [
            ProtectedLocalKeyStoreErrorKind::Unavailable,
            ProtectedLocalKeyStoreErrorKind::Corrupt,
        ] {
            let error = explain_seed_claim_error(anyhow::Error::new(
                ProtectedLocalKeyStoreError::new(kind),
            ));
            assert!(
                !has_code(&error, "sync-setup-invitation-mismatch"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn join_timeout_hint_covers_expiry_without_suggesting_discarding_data() {
        let hint = JOIN_TIMEOUT;
        assert!(hint.starts_with("error sync-join-timeout hint="));
        assert!(hint.contains("rerun `aven sync join`"));
        assert!(hint.contains("`aven sync join --new-invitation`"));
        for word in ["delete", "reset", "disposable", "is empty"] {
            assert!(!hint.contains(word), "{hint}");
        }
    }

    #[test]
    fn device_change_limit_hint_points_to_starting_a_new_sync() {
        let error = explain_change_limit(anyhow::anyhow!("error membership-change-limit"));
        let hint = error.to_string();
        assert!(
            hint.starts_with("error sync-device-change-limit hint="),
            "{hint}"
        );
        assert!(hint.contains("#recover-from-device-loss"), "{hint}");
        let other = explain_change_limit(anyhow::anyhow!("error membership-invalid"));
        assert_eq!(other.to_string(), "error membership-invalid");
    }
}
