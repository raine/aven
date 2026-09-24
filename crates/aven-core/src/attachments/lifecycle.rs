#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};

mod filesystem;
mod leases;
mod liveness;
mod maintenance;
mod quota;
mod report;
#[cfg(test)]
mod test_support;

pub const DEFAULT_LOCAL_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const DEFAULT_ORIGINAL_QUOTA_BYTES: i64 = 10 * 1024 * 1024 * 1024;
pub const DEFAULT_PREVIEW_QUOTA_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAINTENANCE_LIMIT: usize = 128;
pub(crate) const LEASE_TTL: Duration = Duration::from_secs(10 * 60);

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LifecyclePolicy {
    pub grace: Duration,
    pub quota_bytes: i64,
    pub preview_quota_bytes: u64,
    pub maintenance_limit: usize,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            grace: DEFAULT_LOCAL_GRACE,
            quota_bytes: DEFAULT_ORIGINAL_QUOTA_BYTES,
            preview_quota_bytes: DEFAULT_PREVIEW_QUOTA_BYTES,
            maintenance_limit: DEFAULT_MAINTENANCE_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ByteCount {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LifecycleReport {
    pub referenced: ByteCount,
    pub protected: ByteCount,
    pub grace_period: ByteCount,
    pub eligible: ByteCount,
    pub staging: ByteCount,
    pub trash: ByteCount,
    pub reservations: ByteCount,
    pub quota: ByteCount,
    pub inconsistencies: ByteCount,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneSummary {
    pub eligible: ByteCount,
    pub pruned: ByteCount,
}

fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn cutoff(now: DateTime<Utc>, grace: Duration) -> anyhow::Result<String> {
    let grace = chrono::Duration::from_std(grace)?;
    Ok(timestamp(now - grace))
}

fn trash_dir(blob_dir: &Path) -> PathBuf {
    blob_dir.join("trash")
}

fn staging_dir(blob_dir: &Path) -> PathBuf {
    blob_dir.join("objects").join("sha256")
}

pub use leases::{acquire_lease, release_lease};
pub use liveness::reconcile_liveness;
pub(crate) use liveness::reconcile_liveness_for_hashes_in_transaction;
#[cfg(test)]
pub(crate) use maintenance::reconcile_missing_objects;
pub use maintenance::{prune, prune_preview_cache};
pub use quota::{ensure_local_capacity, release_reservation};
pub use report::lifecycle_report;
