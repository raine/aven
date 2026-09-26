//! Listing and removing the devices that take part in sync.
//!
//! Both operations refresh verified membership from the server first. Removal
//! delegates to the engine's durable removal and rotation; rerunning it
//! resumes a retained removal instead of starting another.
use anyhow::{Result, bail, ensure};

use super::{
    NOT_SET_UP, REFUSED, associated_server, explain_change_limit, is_set_up, track_access_result,
};
use crate::db::Database;
use crate::sync::client::coordination;
use crate::sync::client::enrollment::{self, RemovalStatus};
use crate::sync::client::exchange::Link;
use crate::sync::client::host::{ClientHost, key_store};
use crate::sync::client::keys::ProtectedLocalKeyStore;

const REMOVED: &str = "error sync-device-removed hint=\"another device removed this device from sync; its local tasks and images stay available here but can no longer sync\"";

fn explain_revoked(error: anyhow::Error) -> anyhow::Error {
    if error.to_string() == "error enrollment-revoked" {
        error.context(REMOVED)
    } else {
        error
    }
}

struct Session {
    _guard: coordination::SyncProcessGuard,
    store: ProtectedLocalKeyStore,
    client: enrollment::Client,
    server: String,
}

/// Opens an enrolled device for management under the sync coordination lock.
async fn open(link: Link, database: &Database, host: &dyn ClientHost) -> Result<Session> {
    host.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let store = key_store(host, database).await?;
    let guard = coordination::acquire(database).await?;
    let server = associated_server(&store, database).await?;
    Ok(Session {
        _guard: guard,
        client: enrollment::Client::new(&server, link)?,
        store,
        server,
    })
}

/// One device in verified membership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: [u8; 32],
    pub label: Option<String>,
    pub current: bool,
    /// Position of the admission in the membership chain; not a date or a
    /// device number.
    pub admission_sequence: u64,
}

/// Devices in sync, read from membership the server just verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceListing {
    pub server: String,
    /// Keys for future changes still need rotating after a removal.
    pub key_rotation_pending: bool,
    pub devices: Vec<Device>,
}

pub async fn load_devices(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
) -> Result<DeviceListing> {
    let Session {
        _guard,
        store,
        client,
        server,
    } = open(link, database, host).await?;
    let mut inputs = store.active_inputs(database, &server).await?;
    let refreshed = client
        .refresh_inputs(&store, database, &mut inputs)
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error enrollment-unauthorized" => error.context(REFUSED),
            _ => explain_revoked(error),
        });
    track_access_result(database, refreshed).await?;
    let current = inputs.device();
    let labels = database.device_labels().await?;
    Ok(DeviceListing {
        server,
        key_rotation_pending: inputs.membership.rotation_pending(),
        devices: inputs
            .membership
            .admissions()
            .map(|(device, sequence)| Device {
                id: device,
                label: labels.get(&device).cloned(),
                current: device == current,
                admission_sequence: sequence,
            })
            .collect(),
    })
}

/// Explains engine refusals that do not name their cause. The engine has
/// already refreshed verified membership when it refuses a target.
async fn explain_removal_error(
    store: &ProtectedLocalKeyStore,
    database: &Database,
    server: &str,
    target: [u8; 32],
    error: anyhow::Error,
) -> anyhow::Error {
    let hint = match error.to_string().as_str() {
        "error enrollment-revoked" => return error.context(REMOVED),
        // Preparing a removal refuses a target outside verified membership.
        "error membership-signer" | "error membership-invalid" => {
            match store.active_inputs(database, server).await {
                Ok(inputs) if !inputs.membership.has_device(target) => {
                    "error sync-device-not-found hint=\"the device is not in sync; list devices with `aven sync device list`\""
                }
                _ => return error,
            }
        }
        "error membership-change-limit" => return explain_change_limit(error),
        "error management-unfinished" => {
            "error sync-device-removal-unfinished hint=\"an earlier device removal from this device is unfinished; run `aven sync` to finish it, then retry\""
        }
        "error enrollment-unauthorized" => {
            "error sync-device-removal-incomplete hint=\"rerun the same command; it resumes this removal. If the server keeps refusing, another device may have removed this device from sync\""
        }
        _ => {
            "error sync-device-removal-incomplete hint=\"rerun the same command; it resumes this removal\""
        }
    };
    error.context(hint)
}

/// Result of removing another device, read back from verified membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Removal {
    pub device: [u8; 32],
    /// The device is no longer in verified membership.
    pub access_revoked: bool,
    /// Keys for future changes are not rotated yet.
    pub key_rotation_pending: bool,
}

/// Removes another device, or resumes its retained removal. Refuses the
/// current device before reaching the engine.
pub async fn remove_other_device(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
    target: [u8; 32],
) -> Result<Removal> {
    let Session {
        _guard,
        store,
        client,
        server,
    } = open(link, database, host).await?;
    // Leaving from this device would need a separate contract for its own
    // retired credential and remaining local sync state.
    let current = store.active_inputs(database, &server).await?.device();
    ensure!(
        target != current,
        "error sync-device-current hint=\"this is the current device; remove it from another device in sync\""
    );
    let status = match client.remove_device(&store, database, target).await {
        Ok(status) => status,
        Err(error) => {
            let error = explain_removal_error(&store, database, &server, target, error).await;
            return track_access_result(database, Err(error)).await;
        }
    };
    let key_rotation_pending = match status {
        RemovalStatus::Complete => false,
        RemovalStatus::Pending => true,
        RemovalStatus::SelfRevoked => bail!("error management-self-revoke"),
    };
    let membership = store.active_inputs(database, &server).await?.membership;
    Ok(Removal {
        device: target,
        access_revoked: !membership.has_device(target),
        key_rotation_pending,
    })
}

/// Continues an unfinished removal or key rotation retained by the engine, as
/// an ordinary sync round would. Returns whether rotation is still pending.
pub async fn finish_removal(
    link: Link,
    database: &Database,
    host: &dyn ClientHost,
) -> Result<bool> {
    let Session {
        _guard,
        store,
        client,
        server,
    } = open(link, database, host).await?;
    let finished = client
        .finish_pending_management(&store, database)
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error enrollment-unauthorized" => error.context(REFUSED),
            _ => explain_revoked(error),
        });
    track_access_result(database, finished).await?;
    Ok(store
        .active_inputs(database, &server)
        .await?
        .membership
        .rotation_pending())
}
