//! Sync work a person started, and data recovery, with no GPUI types.
//!
//! Everything here runs on a `HostMode::UserStarted` host, so it may ask for
//! Keychain access, while background rounds in the adapter never do. The
//! host's foreground helpers are terminal-mode and would deny that access from
//! a window, so the engine is called directly. Invitation text stays in
//! [`Secret`], and only explained messages leave this module, so neither an
//! invitation nor a raw error chain reaches a toast or a log.
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use aven_core::data_safety::{is_backup_archive, restore_backup_archive, write_export_file};
use aven_core::db::{self, Database};
use aven_core::sync::client::engine::{
    self, Admission, Cancellation, LocalPhase, PendingInvitation, Progress, Reset,
    SETUP_INVITATION_REQUIRED, Stage, local_phase,
};
use aven_core::sync::client::invitation::{DeviceInvitation, SetupInvitation};
use aven_core::sync::client::keys::ProtectedStorage;
use aven_host::config::{AppConfig, resolve_blob_dir};
use aven_host::sync::encrypted::{DesktopHost, HostMode, driver, run_to_completion_with};
use aven_host::sync::error_explanations::{self, ErrorAction, ErrorSurface};
use aven_host::sync::progress_text::amount_text;
use chrono::Local;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

use crate::adapter::{Reply, ReplyBody};
use crate::toast::Toast;

/// An invitation's text. It grants access to synced data, so `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(Zeroizing::new(text.into()))
    }

    pub fn text(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    Setup,
    Join,
}

/// Work that holds the sync slot, so only one runs at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    Now,
    Setup(Secret),
    ResumeSetup,
    Join {
        invitation: Option<Secret>,
        /// Joins into a new database, because this one has tasks.
        new_database: bool,
    },
    RemoveDevice {
        id: [u8; 32],
        label: String,
    },
    FinishRemoval,
    Reset {
        unfinished_setup: bool,
    },
}

impl SyncOp {
    pub fn kind(&self) -> Option<SyncKind> {
        match self {
            Self::Setup(_) | Self::ResumeSetup => Some(SyncKind::Setup),
            Self::Join { .. } => Some(SyncKind::Join),
            _ => None,
        }
    }

    fn action(&self) -> ErrorAction {
        match self.kind() {
            Some(SyncKind::Setup) => ErrorAction::Setup,
            Some(SyncKind::Join) => ErrorAction::Join,
            None => ErrorAction::General,
        }
    }

    fn fallback(&self) -> &'static str {
        match self {
            Self::Now => "Sync couldn't finish. Try again.",
            Self::Setup(_) | Self::ResumeSetup => {
                "Setup couldn't finish. Resuming continues the same setup from where it stopped."
            }
            Self::Join { .. } => {
                "Joining couldn't finish. Resuming continues the same join from where it stopped."
            }
            Self::RemoveDevice { .. } => {
                "Removal didn't finish. Resuming continues the same removal."
            }
            Self::FinishRemoval => {
                "Securing future changes didn't finish. Try again, or sync on any remaining device."
            }
            Self::Reset { .. } => "Sync data couldn't be reset. Try again.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncRequest {
    Run(SyncOp),
    Preview,
    CheckJoin,
    LoadDevices,
    Invite,
    CancelInvitation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: [u8; 32],
    pub label: String,
    pub current: bool,
}

impl DeviceRow {
    /// The icon for the kind of device. Sync records only a label, and the
    /// iPhone and Android apps publish their platform's name as theirs.
    pub fn icon(&self) -> &'static str {
        match self.label.as_str() {
            "iPhone" | "Android" => "icons/phone.svg",
            _ => "icons/device.svg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceList {
    pub devices: Vec<DeviceRow>,
    pub removal_unfinished: bool,
}

/// How a piece of user-started work ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub op: SyncOp,
    pub result: Result<String, String>,
    /// The database a join finished or left resumable in.
    pub open: Option<PathBuf>,
    /// The work can continue only with the setup invitation pasted again.
    pub needs_invitation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEvent {
    Started(SyncOp),
    Progress {
        stage: Stage,
        amount: Option<String>,
    },
    Finished(Box<Finished>),
    Preview(Result<SetupFacts, String>),
    /// Whether this database can join, or the tasks that stop it.
    JoinCheck(Result<Option<i64>, String>),
    Devices(Result<DeviceList, String>),
    Invitation {
        text: Secret,
        expires_at: u64,
    },
    /// The invitation is gone, with what the toast says about it.
    InvitationEnded(Toast),
    /// An invitation stays open and the toast says why.
    InvitationKept(Toast),
    InvitationFailed(String),
}

pub use aven_core::sync::client::engine::SetupPreview as SetupFacts;

/// The plain-language message the terminal app shows for this error.
pub fn explain(action: ErrorAction, error: &anyhow::Error, fallback: &str) -> String {
    error_explanations::connectivity_error(error)
        .or_else(|| {
            error_explanations::explain(action, ErrorSurface::Tui, error)
                .map(|explanation| explanation.combined())
        })
        .unwrap_or_else(|| fallback.to_string())
}

const CHANGE_LIMIT: &str = "This sync has reached its limit on device changes. Start a new sync \
                            to keep changing devices; see Recover from device loss in the sync \
                            docs.";

const JOIN_TIMEOUT_EXPIRED: &str = "If the invitation expired, choose Use a new invitation and \
                                    paste a new one from the same device. The earlier invitation \
                                    still counts if the other device already added this device \
                                    with it.";

fn expiry_time(expires_at: u64) -> String {
    chrono::DateTime::from_timestamp(i64::try_from(expires_at).unwrap_or(0), 0)
        .map(|at| at.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

fn reached_change_limit(error: &anyhow::Error) -> bool {
    error_explanations::has_code(error, "sync-device-change-limit")
        || error_explanations::has_code(error, "membership-change-limit")
}

/// What user-started work needs, taken from the actor when it starts.
#[derive(Clone)]
pub struct SyncWork {
    pub database: Database,
    pub path: PathBuf,
    pub config: AppConfig,
    pub storage: Option<Arc<dyn ProtectedStorage>>,
    pub replies: mpsc::UnboundedSender<Reply>,
    pub request: u64,
}

const DEVICES_FALLBACK: &str = "Couldn't check devices with the server.";

impl SyncWork {
    fn host(&self) -> DesktopHost {
        self.host_with(&self.config)
    }

    fn host_with(&self, config: &AppConfig) -> DesktopHost {
        let host = DesktopHost::new(config, HostMode::UserStarted);
        match &self.storage {
            Some(storage) => host.with_storage(storage.clone()),
            None => host,
        }
    }

    pub fn send(&self, event: SyncEvent) {
        let _ = self.replies.send(Reply {
            request: self.request,
            body: ReplyBody::SyncEvent(event),
        });
    }

    fn progress(&self) -> impl Fn(Progress) + Sync + use<> {
        let replies = self.replies.clone();
        let request = self.request;
        move |progress| {
            let _ = replies.send(Reply {
                request,
                body: ReplyBody::SyncEvent(SyncEvent::Progress {
                    stage: progress.stage,
                    amount: progress.amount.map(amount_text),
                }),
            });
        }
    }

    /// Runs `op` and returns its result for the caller to report, once the
    /// next operation may start.
    pub async fn run(&self, op: SyncOp) -> SyncEvent {
        self.send(SyncEvent::Started(op.clone()));
        let (outcome, open) = self.perform(&op).await;
        let needs_invitation = outcome
            .as_ref()
            .is_err_and(|error| error_explanations::has_code(error, SETUP_INVITATION_REQUIRED));
        let result = outcome.map_err(|error| {
            if reached_change_limit(&error) {
                CHANGE_LIMIT.to_string()
            } else if error_explanations::has_code(&error, "sync-join-timeout") {
                format!("{} {JOIN_TIMEOUT_EXPIRED}", op.fallback())
            } else {
                explain(op.action(), &error, op.fallback())
            }
        });
        SyncEvent::Finished(Box::new(Finished {
            op,
            result,
            open,
            needs_invitation,
        }))
    }

    async fn perform(&self, op: &SyncOp) -> (Result<String>, Option<PathBuf>) {
        match op {
            SyncOp::Join {
                invitation,
                new_database: true,
            } => self.join_new(invitation.as_ref()).await,
            SyncOp::Join { invitation, .. } => (
                self.join(&self.database, &self.host(), invitation.as_ref())
                    .await,
                None,
            ),
            SyncOp::Now => (self.sync_now().await, None),
            SyncOp::Setup(invitation) => (self.setup(invitation).await, None),
            SyncOp::ResumeSetup => (self.resume_setup().await, None),
            SyncOp::RemoveDevice { id, label } => (self.remove(*id, label).await, None),
            SyncOp::FinishRemoval => (self.finish_removal().await, None),
            SyncOp::Reset { unfinished_setup } => (self.reset(*unfinished_setup).await, None),
        }
    }

    async fn sync_now(&self) -> Result<String> {
        run_to_completion_with(&self.database, &self.host()).await?;
        Ok("Sync finished".to_string())
    }

    async fn setup(&self, invitation: &Secret) -> Result<String> {
        let invitation = SetupInvitation::decode(invitation.text())?;
        let host = self.host();
        let progress = self.progress();
        driver()?
            .run(|link| engine::run_setup(link, &self.database, &host, &invitation, &progress))
            .await?;
        Ok("Sync is on".to_string())
    }

    async fn resume_setup(&self) -> Result<String> {
        let host = self.host();
        let progress = self.progress();
        driver()?
            .run(|link| engine::resume_setup(link, &self.database, &host, &progress))
            .await?;
        Ok("Sync is on".to_string())
    }

    async fn join(
        &self,
        database: &Database,
        host: &DesktopHost,
        invitation: Option<&Secret>,
    ) -> Result<String> {
        let replace =
            invitation.is_some() && local_phase(database).await? == LocalPhase::JoinIncomplete;
        let invitation = invitation
            .map(|text| DeviceInvitation::decode(text.text()))
            .transpose()?;
        let progress = self.progress();
        driver()?
            .run(|link| {
                engine::run_join(link, database, host, || Ok(invitation), replace, &progress)
            })
            .await?;
        // A joined device already uses Aven, even when the workspace it joined
        // has no tasks yet, so it opens on the Queue rather than the welcome.
        if let Err(error) = database.complete_onboarding().await {
            tracing::warn!(%error, "could not record the welcome as done");
        }
        Ok("Joined sync".to_string())
    }

    /// Joins into a new database beside this one. The current database is left
    /// alone, and the new one is discarded unless the join got far enough to
    /// resume, so a refused join leaves nothing behind.
    async fn join_new(&self, invitation: Option<&Secret>) -> (Result<String>, Option<PathBuf>) {
        let path = fresh_path(&self.path, "joined", "sqlite");
        let config = own_blobs(&self.config);
        let attempt = async {
            let database = Database::open(&path).await?;
            let result = self
                .join(&database, &self.host_with(&config), invitation)
                .await;
            let phase = local_phase(&database).await;
            database.close().await;
            anyhow::Ok((result, phase))
        };
        match attempt.await {
            Ok((result, Ok(phase))) if phase != LocalPhase::NotSetUp => {
                let done = result.map(|_| {
                    "Joined with a new database. Your earlier tasks are in Data and recovery."
                        .to_string()
                });
                (done, Some(path))
            }
            Ok((result, _)) => {
                remove_database(&path);
                (result, None)
            }
            Err(error) => {
                remove_database(&path);
                (Err(error), None)
            }
        }
    }

    async fn remove(&self, id: [u8; 32], label: &str) -> Result<String> {
        let host = self.host();
        let removal = driver()?
            .run(|link| engine::remove_other_device(link, &self.database, &host, id))
            .await?;
        Ok(
            match (removal.access_revoked, removal.key_rotation_pending) {
                (true, false) => format!("Removed {label} from sync"),
                (true, true) => format!("Access removed for {label}"),
                (false, _) => format!("Removal of {label} isn't confirmed yet"),
            },
        )
    }

    async fn finish_removal(&self) -> Result<String> {
        let host = self.host();
        let finished = driver()?
            .run(|link| engine::finish_removal(link, &self.database, &host))
            .await?;
        Ok(if finished {
            "Future changes secured"
        } else {
            "Nothing left to secure"
        }
        .to_string())
    }

    async fn reset(&self, unfinished_setup: bool) -> Result<String> {
        // The confirmation described the phase it opened in.
        if unfinished_setup != (local_phase(&self.database).await? == LocalPhase::SetupIncomplete) {
            bail!("Sync changed while this was open");
        }
        let reset = engine::reset(&self.database, &self.host(), unfinished_setup).await?;
        Ok(match reset {
            Reset::Reset if unfinished_setup => "Unfinished sync removed. Tasks kept.",
            Reset::Reset => "Sync data reset. Tasks kept.",
            Reset::NotSetUp => "Sync was not set up",
        }
        .to_string())
    }

    pub async fn preview(&self) {
        let facts = async {
            let host = self.host();
            engine::ensure_setup_available(&self.database, &host).await?;
            engine::setup_preview(&self.database, &host).await
        }
        .await;
        self.send(SyncEvent::Preview(facts.map_err(|error| {
            explain(ErrorAction::Setup, &error, "Sync setup isn't available.")
        })));
    }

    pub async fn check_join(&self) {
        let check = engine::ensure_join_available(&self.database, &self.host()).await;
        let answer = match check {
            Ok(()) => Ok(None),
            Err(error)
                if error_explanations::has_code(&error, "sync-join-requires-empty-database") =>
            {
                engine::setup_preview(&self.database, &self.host())
                    .await
                    .map(|preview| Some(preview.tasks))
                    .map_err(|error| explain(ErrorAction::Join, &error, "Joining isn't available."))
            }
            Err(error) => Err(explain(
                ErrorAction::Join,
                &error,
                "Joining isn't available.",
            )),
        };
        self.send(SyncEvent::JoinCheck(answer));
    }

    pub async fn load_devices(&self) {
        let host = self.host();
        let listing = async {
            driver()?
                .run(|link| engine::load_devices(link, &self.database, &host))
                .await
        }
        .await;
        self.send(SyncEvent::Devices(
            listing
                .map(|listing| DeviceList {
                    removal_unfinished: listing.key_rotation_pending,
                    devices: listing
                        .devices
                        .into_iter()
                        .map(|device| DeviceRow {
                            id: device.id,
                            label: device.label.unwrap_or_else(|| "Unnamed device".to_string()),
                            current: device.current,
                        })
                        .collect(),
                })
                .map_err(|error| explain(ErrorAction::General, &error, DEVICES_FALLBACK)),
        ));
    }

    /// Creates an invitation and waits for the new device, or for its end.
    pub async fn invite(&self) {
        let host = self.host();
        let created: Result<PendingInvitation> = async {
            driver()?
                .run(|link| engine::create_invitation(link, &self.database, &host))
                .await
        }
        .await;
        let invitation = match created {
            Ok(invitation) => invitation,
            Err(error) => {
                let message = if reached_change_limit(&error) {
                    CHANGE_LIMIT.to_string()
                } else {
                    explain(ErrorAction::General, &error, DEVICES_FALLBACK)
                };
                self.send(SyncEvent::InvitationFailed(message));
                return;
            }
        };
        self.send(SyncEvent::Invitation {
            text: Secret::new(invitation.text()),
            expires_at: invitation.expires_at(),
        });
        let admitted = async {
            driver()?
                .run(|link| engine::await_admission(link, &host, &self.database, &invitation))
                .await
        }
        .await;
        let ended = match admitted {
            Ok(Admission::Admitted) => Toast::done("Device added"),
            Ok(Admission::Cancelled) => Toast::done("Invitation cancelled"),
            Err(_) if self.keys_may_have_been_sent().await => {
                Toast::warn("Invitation expired").detail("The next sync changes keys")
            }
            Err(error) if error_explanations::has_code(&error, "sync-invitation-unused") => {
                Toast::info("Invitation expired unused")
            }
            Err(error) => Toast::error("The invitation ended").detail(explain(
                ErrorAction::General,
                &error,
                DEVICES_FALLBACK,
            )),
        };
        self.send(SyncEvent::InvitationEnded(ended));
    }

    async fn keys_may_have_been_sent(&self) -> bool {
        engine::association_status(&self.database, &self.host())
            .await
            .ok()
            .and_then(|status| status.invitation)
            .is_some_and(|invitation| invitation.keys_may_have_been_sent)
    }

    /// Cancels the open invitation, and says whether the wait for it can stop.
    pub async fn cancel_invitation(&self) -> bool {
        let host = self.host();
        let cancelled = async {
            driver()?
                .run(|link| engine::cancel_invitation(link, &self.database, &host))
                .await
        }
        .await;
        match cancelled {
            Ok(Cancellation::Cancelled) => {
                self.send(SyncEvent::InvitationEnded(Toast::done(
                    "Invitation cancelled",
                )));
                true
            }
            Ok(Cancellation::None) => {
                self.send(SyncEvent::InvitationEnded(Toast::info(
                    "No invitation is open",
                )));
                true
            }
            Ok(Cancellation::KeysMayHaveBeenSent { expires_at }) => {
                self.send(SyncEvent::InvitationKept(
                    Toast::warn("Keys may already have been sent").detail(format!(
                        "The invitation expires at {}, and the next sync then changes keys",
                        expiry_time(expires_at)
                    )),
                ));
                false
            }
            Err(error) => {
                self.send(SyncEvent::InvitationKept(
                    Toast::error("Could not cancel the invitation").detail(explain(
                        ErrorAction::General,
                        &error,
                        DEVICES_FALLBACK,
                    )),
                ));
                false
            }
        }
    }
}

/// A path beside `current` that nothing uses yet.
pub fn fresh_path(current: &Path, stem: &str, extension: &str) -> PathBuf {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let directory = current.parent().unwrap_or_else(|| Path::new("."));
    (0..)
        .map(|attempt| {
            let suffix = if attempt == 0 {
                String::new()
            } else {
                format!("-{attempt}")
            };
            directory.join(format!("{stem}-{stamp}{suffix}.{extension}"))
        })
        .find(|path| !path.exists())
        .expect("an unused name exists")
}

/// The config for a database other than the launch one. A configured image
/// directory belongs to that database alone.
pub fn own_blobs(config: &AppConfig) -> AppConfig {
    let mut config = config.clone();
    config.local.blob_dir = None;
    config
}

/// The config a database opens with: only the launch database shares the
/// configured image directory.
pub fn config_for(path: &Path, launch: &Path, config: &AppConfig) -> AppConfig {
    if path == launch {
        config.clone()
    } else {
        own_blobs(config)
    }
}

/// Removes a database this process just created, and its sidecars.
fn remove_database(path: &Path) {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&*name) {
            let sidecar = entry.path();
            if sidecar.is_dir() {
                let _ = std::fs::remove_dir_all(&sidecar);
            } else {
                let _ = std::fs::remove_file(&sidecar);
            }
        }
    }
}

/// Restores `archive` to a new database beside `current`, which stays as it
/// was, and returns the new path. Nothing is left behind on failure.
pub async fn restore_to_new(current: &Path, config: &AppConfig, archive: &Path) -> Result<PathBuf> {
    let path = fresh_path(current, "restored", "sqlite");
    let blob_dir = resolve_blob_dir(&path, &own_blobs(config))?;
    let restored = if is_backup_archive(archive)? {
        restore_backup_archive(&path, &blob_dir, archive).await
    } else {
        db::restore_database_file(&path, archive).await
    };
    match restored {
        Ok(_) => Ok(path),
        Err(error) => {
            remove_database(&path);
            Err(error)
        }
    }
}

/// Writes a backup archive beside `current`, including its images.
pub async fn write_backup(database: &Database, current: &Path, blob_dir: &Path) -> Result<PathBuf> {
    let path = fresh_path(current, "aven-backup", "aven-backup.tar.zst");
    database.create_backup_archive(blob_dir, &path).await?;
    Ok(path)
}

/// Writes a JSON export beside `current`, without image files.
pub async fn write_export(database: &Database, current: &Path) -> Result<PathBuf> {
    let path = fresh_path(current, "aven-export", "json");
    let export = database.export_data(aven_core::ids::now()).await?;
    let text = serde_json::to_vec(&export).context("could not serialize export")?;
    write_export_file(&path, &text)?;
    Ok(path)
}

/// Whether `path` can be opened as a database in this app.
pub fn check_database_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("Use a full path");
    }
    // SQLite's URL form would read these as a different path from the one checked.
    if path.to_string_lossy().contains(['%', '?']) {
        bail!("Use a path without % or ?");
    }
    if !path.is_file() {
        return Err(anyhow!("No database at {}", path.display()));
    }
    Ok(())
}

/// Stages of a kind's progress, in order, with how each reads now and after.
pub fn steps(kind: SyncKind) -> &'static [(Stage, &'static str, &'static str)] {
    match kind {
        SyncKind::Setup => &[
            (
                Stage::PreparingData,
                "Preparing your data",
                "Prepared your data",
            ),
            (
                Stage::UploadingData,
                "Uploading encrypted data",
                "Uploaded encrypted data",
            ),
            (Stage::FinishingSetup, "Finishing setup", "Finished setup"),
        ],
        SyncKind::Join => &[
            (
                Stage::WaitingForInviter,
                "Waiting for the other device",
                "The other device added this one",
            ),
            (
                Stage::DownloadingTasks,
                "Downloading tasks",
                "Downloaded tasks",
            ),
            (
                Stage::CatchingUp,
                "Catching up with recent changes",
                "Caught up with recent changes",
            ),
            (
                Stage::DownloadingImages,
                "Downloading images",
                "Downloaded images",
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aven_core::local_state::OnboardingStatus;
    use aven_core::sync::client::keys::FileProtectedStorage;

    use super::*;

    const INVITATION: &str = "AVEN:not-a-real-invitation";

    #[test]
    fn phones_show_a_phone_and_others_a_computer() {
        let row = |label: &str| DeviceRow {
            id: [0; 32],
            label: label.to_string(),
            current: false,
        };
        assert_eq!(row("iPhone").icon(), "icons/phone.svg");
        assert_eq!(row("Android").icon(), "icons/phone.svg");
        assert_eq!(row("Office Mac").icon(), "icons/device.svg");
        assert_eq!(row("Unnamed device").icon(), "icons/device.svg");
    }

    async fn work(path: &Path) -> (SyncWork, mpsc::UnboundedReceiver<Reply>, Database) {
        let database = Database::open(path).await.unwrap();
        let (replies, received) = mpsc::unbounded_channel();
        let storage = FileProtectedStorage::new(path.with_file_name("keys"));
        let work = SyncWork {
            database: database.clone(),
            path: path.to_path_buf(),
            config: AppConfig::default(),
            storage: Some(Arc::new(storage)),
            replies,
            request: 1,
        };
        (work, received, database)
    }

    #[test]
    fn debug_output_hides_an_invitation() {
        let secret = Secret::new(INVITATION);
        let op = SyncOp::Join {
            invitation: Some(secret.clone()),
            new_database: true,
        };
        for text in [format!("{secret:?}"), format!("{op:?}")] {
            assert!(!text.contains("not-a-real"), "{text}");
        }
    }

    #[tokio::test]
    async fn a_refused_join_leaves_no_database_behind() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        let (work, _replies, database) = work(&path).await;
        let before = database.daemon_sync_state().await.unwrap();
        let event = work
            .run(SyncOp::Join {
                invitation: Some(Secret::new(INVITATION)),
                new_database: true,
            })
            .await;
        let SyncEvent::Finished(finished) = event else {
            panic!("the operation finished");
        };
        let message = finished.result.unwrap_err();
        assert!(!message.contains("not-a-real"), "{message}");
        assert_eq!(finished.open, None);
        let names: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|name| name.contains("joined")),
            "{names:?}"
        );
        assert_eq!(local_phase(&database).await.unwrap(), LocalPhase::NotSetUp);
        assert_eq!(database.daemon_sync_state().await.unwrap(), before);
    }

    #[tokio::test]
    async fn a_refused_join_keeps_the_welcome_due() {
        let directory = tempfile::tempdir().unwrap();
        let (work, _replies, database) = work(&directory.path().join("db.sqlite")).await;
        let SyncEvent::Finished(finished) = work
            .run(SyncOp::Join {
                invitation: Some(Secret::new(INVITATION)),
                new_database: false,
            })
            .await
        else {
            panic!("the operation finished");
        };
        assert!(finished.result.is_err());
        assert_eq!(
            database.onboarding_status().await.unwrap(),
            OnboardingStatus::Due
        );
    }

    #[tokio::test]
    async fn restoring_keeps_the_current_database_and_the_archive() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        let (_work, _replies, database) = work(&path).await;
        let blobs = directory.path().join("blobs");
        let archive = write_backup(&database, &path, &blobs).await.unwrap();
        let current = std::fs::metadata(&path).unwrap().len();
        let restored = restore_to_new(&path, &AppConfig::default(), &archive)
            .await
            .unwrap();
        assert_ne!(restored, path);
        assert!(restored.is_file());
        assert!(archive.is_file());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), current);
    }

    #[tokio::test]
    async fn a_bad_archive_leaves_nothing_behind() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        let archive = directory.path().join("backup.sqlite");
        std::fs::write(&archive, "not a database").unwrap();
        assert!(
            restore_to_new(&path, &AppConfig::default(), &archive)
                .await
                .is_err()
        );
        let names = std::fs::read_dir(directory.path()).unwrap().count();
        assert_eq!(names, 1);
    }

    #[test]
    fn another_database_must_exist_at_a_full_path() {
        let directory = tempfile::tempdir().unwrap();
        assert!(check_database_path(Path::new("db.sqlite")).is_err());
        assert!(check_database_path(&directory.path().join("missing.sqlite")).is_err());
        let encoded = directory.path().join("a%2fb.sqlite");
        std::fs::write(&encoded, "").unwrap();
        assert!(check_database_path(&encoded).is_err());
    }

    #[tokio::test]
    async fn abandoning_setup_needs_an_unfinished_setup() {
        let directory = tempfile::tempdir().unwrap();
        let (work, _replies, _database) = work(&directory.path().join("db.sqlite")).await;
        let SyncEvent::Finished(finished) = work
            .run(SyncOp::Reset {
                unfinished_setup: true,
            })
            .await
        else {
            panic!("the operation finished");
        };
        assert!(finished.result.is_err());
    }
}
