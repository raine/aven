//! Setup, joining and device management started from the Sync dialog. The
//! controller owns in-flight work, its current stage and its latest result;
//! the dialog only presents them, so closing it cancels nothing. Quitting the
//! TUI drops the work, and the engine's durable state lets a later attempt
//! resume it. One operation runs at a time.
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use aven_core::db::Database;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::config::AppConfig;
use crate::sync::encrypted::{
    self, DeviceInvitation, DeviceListing, Removal, SetupInvitation, Stage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    /// A manual sync, run by the sync controller; recorded here only when it
    /// fails, so the dialog can explain it.
    Sync,
    Setup,
    Join,
    ListDevices,
    RemoveDevice([u8; 32]),
    /// Continues a removal or key rotation the engine retained.
    FinishRemoval,
}

impl OperationKind {
    /// Operations presented on the device page rather than the summary.
    pub(crate) fn manages_devices(self) -> bool {
        matches!(
            self,
            Self::ListDevices | Self::RemoveDevice(_) | Self::FinishRemoval
        )
    }
}

/// A running operation as the dialog and header present it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunningOperation {
    pub(crate) kind: OperationKind,
    /// The latest engine stage, or `None` before the engine reports one.
    pub(crate) stage: Option<Stage>,
    pub(crate) started_at: Instant,
}

/// How a finished sync drain left tasks and images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrainSummary {
    pub(crate) tasks_current: bool,
    pub(crate) images: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OperationResult {
    SetUp {
        server: String,
        drain: DrainSummary,
    },
    Joined {
        server: String,
        drain: DrainSummary,
    },
    Removed(Removal),
    /// A retained removal was continued; rotation may still be pending.
    RemovalFinished {
        key_rotation_pending: bool,
    },
    Failed(OperationFailure),
}

/// A failed operation. `details` is the engine's error chain, which never
/// includes invitation text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationFailure {
    pub(crate) kind: OperationKind,
    pub(crate) message: String,
    pub(crate) details: String,
}

impl OperationFailure {
    pub(crate) fn join_timed_out(&self) -> bool {
        self.kind == OperationKind::Join && self.details.contains("error sync-join-timeout")
    }

    /// The engine reported an earlier removal that must finish first.
    pub(crate) fn removal_unfinished(&self) -> bool {
        self.details.contains("error management-unfinished")
    }
}

/// The latest device list the server verified, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceSnapshot {
    pub(crate) listing: DeviceListing,
    pub(crate) checked_at: Instant,
    /// Set when the list was updated from a removal's own membership read.
    pub(crate) after_removal: bool,
}

/// What views read about operations started from the Sync dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncActivity {
    pub(crate) running: Option<RunningOperation>,
    pub(crate) last: Option<OperationResult>,
    pub(crate) devices: Option<DeviceSnapshot>,
    /// A join timed out in this session; guidance about an expired invitation
    /// stays visible while joining is resumed.
    pub(crate) join_timed_out: bool,
}

impl SyncActivity {
    /// Joining has started and synced tasks are not installed yet, so local
    /// edits would make this database refuse the installation.
    pub(crate) fn join_awaiting_tasks(&self) -> bool {
        self.running.is_some_and(|running| {
            running.kind == OperationKind::Join
                && !matches!(
                    running.stage,
                    Some(Stage::CatchingUp | Stage::DownloadingImages)
                )
        })
    }

    /// The last result, when it belongs to the device page.
    pub(crate) fn device_result(&self) -> Option<&OperationResult> {
        self.last.as_ref().filter(|result| match result {
            OperationResult::Removed(_) | OperationResult::RemovalFinished { .. } => true,
            OperationResult::Failed(failure) => failure.kind.manages_devices(),
            OperationResult::SetUp { .. } | OperationResult::Joined { .. } => false,
        })
    }
}

enum Done {
    SetUp(String, encrypted::Outcome),
    Joined(String, encrypted::Outcome),
    Devices(DeviceListing),
    Removed(Removal),
    RemovalFinished(bool),
}

/// A change observed while polling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OperationEvent {
    Stage(OperationKind, Stage),
    Finished(OperationKind, Option<OperationResult>),
}

pub(super) struct SyncOperations {
    task: Option<JoinHandle<Result<Done>>>,
    stage: Arc<Mutex<Option<Stage>>>,
    /// The submitted setup invitation, kept in memory so an interrupted setup
    /// can retry in this session without pasting it again.
    setup_invitation: Option<Zeroizing<String>>,
    pub(super) activity: SyncActivity,
}

impl SyncOperations {
    pub(super) fn new() -> Self {
        Self {
            task: None,
            stage: Arc::new(Mutex::new(None)),
            setup_invitation: None,
            activity: SyncActivity::default(),
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        self.task.is_some()
    }

    /// Shows a failure that happened outside a running operation, such as a
    /// refusal before work started or a failed manual sync.
    pub(super) fn record_refusal(&mut self, kind: OperationKind, error: &anyhow::Error) {
        self.activity.last = Some(OperationResult::Failed(super::sync_errors::failure(
            kind, error,
        )));
    }

    /// Drops a recorded failure of `kind`, such as once a later attempt
    /// succeeds, and reports whether there was one.
    pub(super) fn clear_failure(&mut self, kind: OperationKind) -> bool {
        let failed = matches!(
            &self.activity.last,
            Some(OperationResult::Failed(failure)) if failure.kind == kind
        );
        if failed {
            self.activity.last = None;
        }
        failed
    }

    pub(super) fn has_setup_invitation(&self) -> bool {
        self.setup_invitation.is_some()
    }

    /// Starts setup with a validated invitation, or resumes it with the one
    /// retained from this session when `invitation` is `None`.
    pub(super) fn start_setup(
        &mut self,
        database: &Database,
        config: &AppConfig,
        invitation: Option<Zeroizing<String>>,
    ) -> bool {
        let Some(text) = invitation.or_else(|| self.setup_invitation.clone()) else {
            return false;
        };
        if self.task.is_some() {
            return false;
        }
        self.setup_invitation = Some(text.clone());
        let database = database.clone();
        let config = config.clone();
        let stage = self.stage.clone();
        self.spawn(OperationKind::Setup, async move {
            let invitation = SetupInvitation::decode(&text)?;
            let progress = |next| *stage.lock().expect("stage lock") = Some(next);
            let outcome = encrypted::run_setup(&database, &config, &invitation, &progress).await?;
            Ok(Done::SetUp(invitation.server, outcome))
        })
    }

    /// Starts joining with a validated invitation, or resumes the request
    /// this database already made when `invitation` is `None`.
    pub(super) fn start_join(
        &mut self,
        database: &Database,
        config: &AppConfig,
        invitation: Option<Zeroizing<String>>,
        replace: bool,
    ) -> bool {
        let database = database.clone();
        let config = config.clone();
        let stage = self.stage.clone();
        self.spawn(OperationKind::Join, async move {
            let progress = |next| *stage.lock().expect("stage lock") = Some(next);
            let (server, outcome) = encrypted::run_join(
                &database,
                &config,
                move || {
                    invitation
                        .map(|text| DeviceInvitation::decode(&text))
                        .transpose()
                },
                replace,
                &progress,
            )
            .await?;
            Ok(Done::Joined(server, outcome))
        })
    }

    pub(super) fn start_list_devices(&mut self, database: &Database, config: &AppConfig) -> bool {
        let database = database.clone();
        let config = config.clone();
        self.spawn(OperationKind::ListDevices, async move {
            Ok(Done::Devices(
                encrypted::load_devices(&database, &config).await?,
            ))
        })
    }

    /// Removes another device, or resumes its retained removal.
    pub(super) fn start_remove_device(
        &mut self,
        database: &Database,
        config: &AppConfig,
        device: [u8; 32],
    ) -> bool {
        let database = database.clone();
        let config = config.clone();
        self.spawn(OperationKind::RemoveDevice(device), async move {
            Ok(Done::Removed(
                encrypted::remove_other_device(&database, &config, device).await?,
            ))
        })
    }

    pub(super) fn start_finish_removal(&mut self, database: &Database, config: &AppConfig) -> bool {
        let database = database.clone();
        let config = config.clone();
        self.spawn(OperationKind::FinishRemoval, async move {
            Ok(Done::RemovalFinished(
                encrypted::finish_removal(&database, &config).await?,
            ))
        })
    }

    fn spawn(
        &mut self,
        kind: OperationKind,
        work: impl Future<Output = Result<Done>> + Send + 'static,
    ) -> bool {
        if self.task.is_some() {
            return false;
        }
        *self.stage.lock().expect("stage lock") = None;
        self.activity.running = Some(RunningOperation {
            kind,
            stage: None,
            started_at: Instant::now(),
        });
        // A listing refreshes data without replacing the previous outcome.
        if kind != OperationKind::ListDevices {
            self.activity.last = None;
        }
        self.task = Some(tokio::spawn(work));
        true
    }

    pub(super) async fn poll(&mut self) -> Option<OperationEvent> {
        let running = self.activity.running.as_mut()?;
        if let Some(task) = self.task.take_if(|task| task.is_finished()) {
            let kind = running.kind;
            self.activity.running = None;
            let result = match task.await.context("sync operation stopped") {
                Ok(Ok(finished)) => self.finish(finished),
                Ok(Err(error)) | Err(error) => Some(OperationResult::Failed(
                    super::sync_errors::failure(kind, &error),
                )),
            };
            match &result {
                Some(OperationResult::Failed(failure)) if failure.join_timed_out() => {
                    self.activity.join_timed_out = true
                }
                Some(OperationResult::Joined { .. }) => self.activity.join_timed_out = false,
                _ => {}
            }
            if let Some(result) = &result {
                self.activity.last = Some(result.clone());
            }
            return Some(OperationEvent::Finished(kind, result));
        }
        let stage = *self.stage.lock().expect("stage lock");
        if stage == running.stage {
            return None;
        }
        running.stage = stage;
        stage.map(|stage| OperationEvent::Stage(running.kind, stage))
    }

    /// Records finished work. Listing updates the device snapshot instead
    /// of producing a result.
    fn finish(&mut self, finished: Done) -> Option<OperationResult> {
        let summary = |outcome: encrypted::Outcome| DrainSummary {
            tasks_current: outcome.metadata_caught_up,
            images: outcome.images,
        };
        Some(match finished {
            Done::SetUp(server, outcome) => {
                self.setup_invitation = None;
                OperationResult::SetUp {
                    server,
                    drain: summary(outcome),
                }
            }
            Done::Joined(server, outcome) => OperationResult::Joined {
                server,
                drain: summary(outcome),
            },
            Done::Devices(listing) => {
                self.activity.devices = Some(DeviceSnapshot {
                    listing,
                    checked_at: Instant::now(),
                    after_removal: false,
                });
                if matches!(
                    &self.activity.last,
                    Some(OperationResult::Failed(failure))
                        if failure.kind == OperationKind::ListDevices
                ) {
                    self.activity.last = None;
                }
                return None;
            }
            Done::Removed(removal) => {
                if let Some(snapshot) = &mut self.activity.devices {
                    if removal.access_revoked {
                        snapshot
                            .listing
                            .devices
                            .retain(|device| device.id != removal.device);
                    }
                    snapshot.listing.key_rotation_pending = removal.key_rotation_pending;
                    snapshot.checked_at = Instant::now();
                    snapshot.after_removal = true;
                }
                OperationResult::Removed(removal)
            }
            Done::RemovalFinished(key_rotation_pending) => {
                if let Some(snapshot) = &mut self.activity.devices {
                    snapshot.listing.key_rotation_pending = key_rotation_pending;
                }
                OperationResult::RemovalFinished {
                    key_rotation_pending,
                }
            }
        })
    }
}

impl Drop for SyncOperations {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Shortest hex prefixes, at least eight characters, that tell every listed
/// device apart. Display only; actions always use the full ID.
pub(crate) fn short_device_ids(devices: &[encrypted::Device]) -> Vec<String> {
    let full = devices
        .iter()
        .map(|device| hex::encode(device.id))
        .collect::<Vec<_>>();
    let mut length = 8;
    while length < 64 {
        let mut prefixes = full.iter().map(|id| &id[..length]).collect::<Vec<_>>();
        prefixes.sort_unstable();
        prefixes.dedup();
        if prefixes.len() == full.len() {
            break;
        }
        length += 2;
    }
    full.iter()
        .map(|id| format!("{}…", &id[..length]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::encrypted::Device;

    fn device(id: [u8; 32], current: bool) -> Device {
        Device {
            id,
            label: None,
            current,
            admission_sequence: 0,
        }
    }

    #[test]
    fn short_ids_extend_only_as_far_as_needed_to_disambiguate() {
        let first = [0xa1; 32];
        let mut second = [0xa1; 32];
        second[5] = 0xff;
        let third = [0x7f; 32];
        assert_eq!(
            short_device_ids(&[device(first, true), device(third, false)]),
            ["a1a1a1a1…", "7f7f7f7f…"]
        );
        assert_eq!(
            short_device_ids(&[device(first, true), device(second, false)]),
            ["a1a1a1a1a1a1…", "a1a1a1a1a1ff…"]
        );
        let mut last = first;
        last[31] = 0;
        let ids = short_device_ids(&[device(first, false), device(last, false)]);
        assert_ne!(ids[0], ids[1]);
    }

    async fn settle(operations: &mut SyncOperations) -> Option<OperationEvent> {
        loop {
            if let Some(event @ OperationEvent::Finished(..)) = operations.poll().await {
                return Some(event);
            }
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_join_timeout_is_remembered_for_the_session() {
        let mut operations = SyncOperations::new();
        assert!(operations.spawn(OperationKind::Join, async {
            Err(anyhow::anyhow!("error sync-join-timeout hint=\"x\""))
        }));
        settle(&mut operations).await;
        assert!(operations.activity.join_timed_out);

        // A later failure for another reason keeps the guidance.
        assert!(operations.spawn(OperationKind::Join, async {
            Err(anyhow::anyhow!("error enrollment-network outcome-unknown"))
        }));
        settle(&mut operations).await;
        assert!(operations.activity.join_timed_out);
    }

    #[test]
    fn removal_results_update_the_cached_list_without_claiming_more_than_the_engine() {
        let mut operations = SyncOperations::new();
        operations.activity.devices = Some(DeviceSnapshot {
            listing: DeviceListing {
                server: "https://sync.example.com".to_string(),
                key_rotation_pending: false,
                devices: vec![device([1; 32], true), device([2; 32], false)],
            },
            checked_at: Instant::now(),
            after_removal: false,
        });
        let result = operations.finish(Done::Removed(Removal {
            device: [2; 32],
            access_revoked: true,
            key_rotation_pending: true,
        }));
        assert!(matches!(result, Some(OperationResult::Removed(_))));
        let snapshot = operations.activity.devices.as_ref().unwrap();
        assert_eq!(snapshot.listing.devices.len(), 1);
        assert!(snapshot.listing.key_rotation_pending);
        assert!(snapshot.after_removal);

        let result = operations.finish(Done::Removed(Removal {
            device: [1; 32],
            access_revoked: false,
            key_rotation_pending: false,
        }));
        assert!(matches!(result, Some(OperationResult::Removed(_))));
        assert_eq!(
            operations
                .activity
                .devices
                .as_ref()
                .unwrap()
                .listing
                .devices
                .len(),
            1
        );
    }
}
