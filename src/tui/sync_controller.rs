use std::time::Instant;

use anyhow::{Context, Result};
use aven_core::db::Database;
use tokio::task::JoinHandle;

use crate::config::AppConfig;
use crate::sync::encrypted::{self, LocalPhase, Outcome};
use crate::tui::app::{App, Notification};

pub(super) struct SyncController {
    task: Option<JoinHandle<Result<Outcome>>>,
    started_at: Option<Instant>,
}

impl SyncController {
    pub(super) fn new() -> Self {
        Self {
            task: None,
            started_at: None,
        }
    }

    pub(super) fn start(&mut self, database: &Database, config: &AppConfig) -> Result<bool> {
        if self.task.is_some() {
            return Ok(false);
        }
        config.ensure_sync_allowed()?;
        let database = database.clone();
        let config = config.clone();
        self.task = Some(tokio::spawn(async move {
            encrypted::run_to_completion(&database, &config).await
        }));
        self.started_at = Some(Instant::now());
        Ok(true)
    }

    pub(super) fn work_pending(&self) -> bool {
        self.task.is_some()
    }

    /// When the running sync started; `None` while idle.
    pub(super) fn started_at(&self) -> Option<Instant> {
        self.task.as_ref().and(self.started_at)
    }

    pub(super) async fn poll(&mut self) -> Option<Result<Outcome>> {
        if !self.task.as_ref().is_some_and(JoinHandle::is_finished) {
            return None;
        }
        let task = self.task.take().expect("finished sync task");
        Some(match task.await {
            Ok(result) => result,
            Err(error) => Err(error).context("manual sync task stopped"),
        })
    }
}

impl App {
    pub(super) fn begin_sync(&mut self) {
        if !self.store.sync_status.set_up {
            self.set_error("sync unavailable: set up or join sync from :sync first");
            return;
        }
        if self.sync_ops.work_pending() {
            self.set_info("sync is busy; open :sync to follow its progress");
            return;
        }
        match self.store.sync_status.phase {
            LocalPhase::SetupIncomplete => {
                self.set_warning("setup is unfinished; resume it from :sync");
                return;
            }
            LocalPhase::SetupRecoveryRequired => {
                self.set_warning("setup was refused; open :sync for recovery steps");
                return;
            }
            LocalPhase::JoinIncomplete => {
                self.set_warning("joining is unfinished; resume it from :sync");
                return;
            }
            LocalPhase::NotSetUp | LocalPhase::SetUp => {}
        }
        match self
            .sync
            .start(&self.store.database(), self.intake.config())
        {
            Ok(true) => self.notification = Some(Notification::loading("syncing")),
            Ok(false) => self.set_info("sync already in progress"),
            Err(error) => {
                let message = crate::tui::sync_errors::failure(
                    crate::tui::sync_operations::OperationKind::Sync,
                    &error,
                )
                .message;
                self.set_error(format!("sync unavailable: {message}"));
            }
        }
    }

    pub(super) async fn poll_sync(&mut self) -> Result<bool> {
        let Some(result) = self.sync.poll().await else {
            return Ok(false);
        };
        if matches!(self.notification, Some(Notification::Loading { .. })) {
            self.notification = None;
        }
        match result {
            Ok(result) => {
                self.sync_ops
                    .clear_failure(crate::tui::sync_operations::OperationKind::Sync);
                let refresh_error = self.refresh().await.err();
                match refresh_error {
                    Some(error) => {
                        self.set_warning(format!("sync finished but refresh failed: {error:#}"));
                    }
                    None if result.metadata_caught_up && result.images == "complete" => {
                        self.set_success("sync complete")
                    }
                    None if result.metadata_caught_up => {
                        self.set_warning(format!("tasks synced; images {}", result.images))
                    }
                    None => self.set_warning(format!(
                        "sync stopped with pending work after {} rounds; images {}",
                        result.rounds, result.images
                    )),
                }
            }
            Err(error) => {
                let refresh_error = self.refresh().await.err();
                // The dialog keeps the engine error as details.
                self.sync_ops
                    .record_refusal(crate::tui::sync_operations::OperationKind::Sync, &error);
                let message = crate::tui::sync_errors::failure(
                    crate::tui::sync_operations::OperationKind::Sync,
                    &error,
                )
                .message;
                let message = match refresh_error {
                    Some(refresh_error) => {
                        format!("sync failed: {message} Refresh failed: {refresh_error:#}")
                    }
                    None => format!("sync failed: {message} Open :sync for details."),
                };
                self.set_error(message);
            }
        }
        Ok(true)
    }
}

impl Drop for SyncController {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
