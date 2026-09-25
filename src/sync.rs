use std::time::Duration;

mod coordination;
mod device_label;
pub(crate) mod encrypted;
mod server;

pub(crate) const ATTACHMENT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) use server::run_server;
