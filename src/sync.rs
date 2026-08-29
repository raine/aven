use std::time::Duration;

mod client;
mod coordination;
mod server;

pub(crate) const ATTACHMENT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) use aven_core::sync::wire;

pub(crate) use client::{
    DaemonSyncOutcome, SyncHttpClient, SyncRunSummary, run_sync_to_completion, sync_client,
    try_run_daemon_sync_with_page_budget,
};
pub(crate) use server::run_server;
pub(crate) use wire::sync_server_url_is_valid;
