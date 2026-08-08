use anyhow::{Context, Result};
use aven_core::db::Database;
use tokio::task::JoinHandle;

use crate::config::{self, AppConfig};
use crate::sync::SyncRunSummary;
use crate::tui::app::{App, Notification};

pub(super) struct SyncController {
    task: Option<JoinHandle<Result<SyncRunSummary>>>,
}

impl SyncController {
    pub(super) fn new() -> Self {
        Self { task: None }
    }

    pub(super) fn start(&mut self, database: &Database, config: &AppConfig) -> Result<bool> {
        if self.task.is_some() {
            return Ok(false);
        }
        config.ensure_sync_allowed()?;
        config::resolve_sync_server(None, config)?;
        let database = database.clone();
        let config = config.clone();
        self.task = Some(tokio::spawn(async move {
            crate::sync::run_sync_to_completion(&database, &config).await
        }));
        Ok(true)
    }

    pub(super) fn work_pending(&self) -> bool {
        self.task.is_some()
    }

    pub(super) async fn poll(&mut self) -> Option<Result<SyncRunSummary>> {
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
        match self
            .sync
            .start(&self.store.database(), self.intake.config())
        {
            Ok(true) => self.notification = Some(Notification::loading("syncing")),
            Ok(false) => self.set_info("sync already in progress"),
            Err(error) => self.set_error(format!("sync unavailable: {error:#}")),
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
                let refresh_error = self.refresh().await.err();
                match refresh_error {
                    Some(error) => {
                        self.set_warning(format!("sync finished but refresh failed: {error:#}"));
                    }
                    None if result.complete => self.set_success(format!(
                        "sync complete pushed={} pulled={} cursor={} pages={}",
                        result.pushed, result.pulled, result.cursor, result.pages
                    )),
                    None => self.set_warning(format!(
                        "sync stopped with pending work pushed={} pulled={} cursor={} pages={}",
                        result.pushed, result.pulled, result.cursor, result.pages
                    )),
                }
            }
            Err(error) => {
                let refresh_error = self.refresh().await.err();
                let message = match refresh_error {
                    Some(refresh_error) => {
                        format!("sync failed: {error:#}; refresh failed: {refresh_error:#}")
                    }
                    None => format!("sync failed: {error:#}"),
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
