//! Setup and joining started from the Sync dialog. The controller owns
//! in-flight work, its current stage and its latest result; the dialog only
//! presents them, so closing it cancels nothing. Quitting the TUI drops the
//! work, and the engine's durable state lets a later attempt resume it.
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use aven_core::db::Database;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::config::AppConfig;
use crate::sync::encrypted::{self, DeviceInvitation, SetupInvitation, Stage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    Setup,
    Join,
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
    SetUp { server: String, drain: DrainSummary },
    Joined { server: String, drain: DrainSummary },
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

/// What views read about operations started from the Sync dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncActivity {
    pub(crate) running: Option<RunningOperation>,
    pub(crate) last: Option<OperationResult>,
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
}

enum Finished {
    SetUp(String, encrypted::Outcome),
    Joined(String, encrypted::Outcome),
}

/// A change observed while polling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OperationEvent {
    Stage(OperationKind, Stage),
    Finished(OperationResult),
}

pub(super) struct SyncOperations {
    task: Option<JoinHandle<Result<Finished>>>,
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

    /// Shows a refusal that happened before any work started.
    pub(super) fn record_refusal(&mut self, kind: OperationKind, error: &anyhow::Error) {
        self.activity.last = Some(OperationResult::Failed(super::sync_errors::failure(
            kind, error,
        )));
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
        self.setup_invitation = Some(text.clone());
        let database = database.clone();
        let config = config.clone();
        let stage = self.stage.clone();
        self.spawn(OperationKind::Setup, async move {
            let invitation = SetupInvitation::decode(&text)?;
            let progress = |next| *stage.lock().expect("stage lock") = Some(next);
            let outcome = encrypted::run_setup(&database, &config, &invitation, &progress).await?;
            Ok(Finished::SetUp(invitation.server, outcome))
        })
    }

    /// Starts joining with a validated invitation, or resumes the request
    /// this database already made when `invitation` is `None`.
    pub(super) fn start_join(
        &mut self,
        database: &Database,
        config: &AppConfig,
        invitation: Option<Zeroizing<String>>,
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
                &progress,
            )
            .await?;
            Ok(Finished::Joined(server, outcome))
        })
    }

    fn spawn(
        &mut self,
        kind: OperationKind,
        work: impl Future<Output = Result<Finished>> + Send + 'static,
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
        self.activity.last = None;
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
                Ok(Err(error)) | Err(error) => {
                    OperationResult::Failed(super::sync_errors::failure(kind, &error))
                }
            };
            self.activity.last = Some(result.clone());
            return Some(OperationEvent::Finished(result));
        }
        let stage = *self.stage.lock().expect("stage lock");
        if stage == running.stage {
            return None;
        }
        running.stage = stage;
        stage.map(|stage| OperationEvent::Stage(running.kind, stage))
    }

    fn finish(&mut self, finished: Finished) -> OperationResult {
        let summary = |outcome: encrypted::Outcome| DrainSummary {
            tasks_current: outcome.metadata_caught_up,
            images: outcome.images,
        };
        match finished {
            Finished::SetUp(server, outcome) => {
                self.setup_invitation = None;
                OperationResult::SetUp {
                    server,
                    drain: summary(outcome),
                }
            }
            Finished::Joined(server, outcome) => OperationResult::Joined {
                server,
                drain: summary(outcome),
            },
        }
    }
}

impl Drop for SyncOperations {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
