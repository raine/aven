use std::time::Duration;

pub(crate) mod encrypted;
pub(crate) use aven_host::sync::{error_explanations, progress_text};
mod server;

pub(crate) const ATTACHMENT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) use server::run_server;
