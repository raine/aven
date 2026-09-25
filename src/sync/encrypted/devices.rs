//! Listing and removing the devices that take part in sync.
//!
//! Both operations refresh verified membership from the server first. Removal
//! delegates to the engine's durable removal and rotation; rerunning it
//! resumes a retained removal instead of starting another.
use anyhow::{Result, bail, ensure};
use aven_core::db::Database;
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use super::{NOT_SET_UP, REFUSED, associated_server, explain_change_limit, is_set_up, key_store};
use crate::config::AppConfig;
use crate::peer_enrollment_http::{self, RemovalStatus};
use crate::protected_local_keys::ProtectedLocalKeyStore;
use crate::render::print_json_pretty;

const REMOVED: &str = "error sync-device-removed hint=\"another device removed this device from sync; its local tasks and images stay available here but can no longer sync\"";

#[derive(Serialize)]
struct DeviceList {
    version: u32,
    server: String,
    key_rotation_pending: bool,
    devices: Vec<DeviceEntry>,
}

#[derive(Serialize)]
struct DeviceEntry {
    device_id: String,
    label: Option<String>,
    current: bool,
    admission_sequence: u64,
}

#[derive(Serialize)]
struct RemovalReport {
    version: u32,
    device_id: String,
    state: &'static str,
    access_revoked: bool,
    key_rotation_pending: bool,
}

fn explain_revoked(error: anyhow::Error) -> anyhow::Error {
    if error.to_string() == "error enrollment-revoked" {
        error.context(REMOVED)
    } else {
        error
    }
}

struct Session {
    _guard: crate::sync::coordination::SyncProcessGuard,
    store: ProtectedLocalKeyStore,
    client: peer_enrollment_http::Client,
    server: String,
}

/// Opens an enrolled device for management under the sync coordination lock.
async fn open(database: &Database, config: &AppConfig) -> Result<Session> {
    config.ensure_sync_allowed()?;
    ensure!(is_set_up(database).await?, NOT_SET_UP);
    let store = key_store(database)?;
    let guard = crate::sync::coordination::acquire(database).await?;
    let server = associated_server(&store, database).await?;
    Ok(Session {
        _guard: guard,
        client: peer_enrollment_http::Client::new(&server)?,
        store,
        server,
    })
}

/// One device in verified membership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Device {
    pub(crate) id: [u8; 32],
    pub(crate) label: Option<String>,
    pub(crate) current: bool,
    /// Position of the admission in the membership chain; not a date or a
    /// device number.
    pub(crate) admission_sequence: u64,
}

/// Devices in sync, read from membership the server just verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceListing {
    pub(crate) server: String,
    /// Keys for future changes still need rotating after a removal.
    pub(crate) key_rotation_pending: bool,
    pub(crate) devices: Vec<Device>,
}

pub(crate) async fn load_devices(database: &Database, config: &AppConfig) -> Result<DeviceListing> {
    let Session {
        _guard,
        store,
        client,
        server,
    } = open(database, config).await?;
    let mut inputs = store.active_inputs(database, &server).await?;
    client
        .refresh_inputs(&store, database, &mut inputs)
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error enrollment-refused outcome-unknown" => error.context(REFUSED),
            _ => explain_revoked(error),
        })?;
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

pub(crate) async fn list(database: &Database, config: &AppConfig, json: bool) -> Result<()> {
    let listing = load_devices(database, config).await?;
    let report = DeviceList {
        version: 1,
        server: listing.server,
        key_rotation_pending: listing.key_rotation_pending,
        devices: listing
            .devices
            .iter()
            .map(|device| DeviceEntry {
                device_id: hex::encode(device.id),
                label: device.label.clone(),
                current: device.current,
                admission_sequence: device.admission_sequence,
            })
            .collect(),
    };
    if json {
        return print_json_pretty(&report);
    }
    print_device_table(&report.devices);
    if report.key_rotation_pending {
        println!("\nKey update still finishing.");
    }
    Ok(())
}

fn short_device_ids(devices: &[DeviceEntry]) -> Vec<String> {
    let mut length = 8;
    while length < 64 {
        let mut prefixes = devices
            .iter()
            .map(|device| &device.device_id[..length])
            .collect::<Vec<_>>();
        prefixes.sort_unstable();
        prefixes.dedup();
        if prefixes.len() == devices.len() {
            break;
        }
        length += 1;
    }
    devices
        .iter()
        .map(|device| format!("{}…", &device.device_id[..length]))
        .collect()
}

fn print_device_table(devices: &[DeviceEntry]) {
    let ids = short_device_ids(devices);
    let has_labels = devices.iter().any(|device| device.label.is_some());
    let label_width = devices
        .iter()
        .filter_map(|device| device.label.as_deref())
        .map(UnicodeWidthStr::width)
        .chain(std::iter::once("LABEL".len()))
        .max()
        .unwrap_or(0);
    let id_width = ids
        .iter()
        .map(|id| UnicodeWidthStr::width(id.as_str()))
        .chain(std::iter::once("ID".len()))
        .max()
        .unwrap_or(0);
    if has_labels {
        println!("{:<label_width$}  {:<id_width$}  STATUS", "LABEL", "ID");
    } else {
        println!("{:<id_width$}  STATUS", "ID");
    }
    for (device, id) in devices.iter().zip(ids) {
        let status = if device.current { "this device" } else { "" };
        if has_labels {
            let label = device.label.as_deref().unwrap_or("");
            let padding = label_width.saturating_sub(UnicodeWidthStr::width(label));
            println!("{label}{}  {id:<id_width$}  {status}", " ".repeat(padding));
        } else {
            println!("{id:<id_width$}  {status}");
        }
    }
}

fn valid_device_prefix(text: &str) -> bool {
    (4..=64).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decode_full_device_id(text: &str) -> Result<[u8; 32]> {
    let mut device = [0; 32];
    if text.len() != 64 || hex::decode_to_slice(text, &mut device).is_err() {
        bail!("error sync-device-id-invalid");
    }
    Ok(device)
}

fn matching_devices<'a>(devices: &'a [Device], prefix: &str) -> Vec<&'a Device> {
    let prefix = prefix.to_ascii_lowercase();
    devices
        .iter()
        .filter(|device| hex::encode(device.id).starts_with(&prefix))
        .collect()
}

async fn resolve_device_id(
    database: &Database,
    config: &AppConfig,
    text: &str,
) -> Result<[u8; 32]> {
    if !valid_device_prefix(text) {
        bail!(
            "error sync-device-id-invalid hint=\"use at least 4 hexadecimal characters from `aven sync device list`\""
        );
    }
    if text.len() == 64 {
        return decode_full_device_id(text);
    }
    let listing = load_devices(database, config).await?;
    let matches = matching_devices(&listing.devices, text);
    match matches.as_slice() {
        [device] => Ok(device.id),
        [] => bail!(
            "error sync-device-not-found hint=\"no current device has that ID prefix; list devices with `aven sync device list`\""
        ),
        _ => {
            let listed = matches
                .iter()
                .map(|device| match &device.label {
                    Some(label) => format!("  {label}  {}", hex::encode(device.id)),
                    None => format!("  {}", hex::encode(device.id)),
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("error sync-device-id-ambiguous\nMatching devices:\n{listed}")
        }
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    fn device(id: [u8; 32]) -> Device {
        Device {
            id,
            label: None,
            current: false,
            admission_sequence: 0,
        }
    }

    #[test]
    fn prefixes_are_case_insensitive_and_can_be_ambiguous() {
        let mut first = [0xab; 32];
        let mut second = [0xab; 32];
        first[2] = 0x10;
        second[2] = 0x20;
        let devices = [device(first), device(second), device([0xcd; 32])];
        assert_eq!(matching_devices(&devices, "ABAB").len(), 2);
        assert_eq!(matching_devices(&devices, "abab1")[0].id, first);
        assert!(matching_devices(&devices, "ffff").is_empty());
    }
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
        // A lost reply and a refused credential look the same here.
        "error enrollment-refused outcome-unknown" => {
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
pub(crate) struct Removal {
    pub(crate) device: [u8; 32],
    /// The device is no longer in verified membership.
    pub(crate) access_revoked: bool,
    /// Keys for future changes are not rotated yet.
    pub(crate) key_rotation_pending: bool,
}

/// Removes another device, or resumes its retained removal. Refuses the
/// current device before reaching the engine.
pub(crate) async fn remove_other_device(
    database: &Database,
    config: &AppConfig,
    target: [u8; 32],
) -> Result<Removal> {
    let Session {
        _guard,
        store,
        client,
        server,
    } = open(database, config).await?;
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
            return Err(explain_removal_error(&store, database, &server, target, error).await);
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
pub(crate) async fn finish_removal(database: &Database, config: &AppConfig) -> Result<bool> {
    let Session {
        _guard,
        store,
        client,
        server,
    } = open(database, config).await?;
    client
        .finish_pending_management(&store, database)
        .await
        .map_err(|error| match error.to_string().as_str() {
            "error enrollment-refused outcome-unknown" => error.context(REFUSED),
            _ => explain_revoked(error),
        })?;
    Ok(store
        .active_inputs(database, &server)
        .await?
        .membership
        .rotation_pending())
}

pub(crate) async fn remove(
    database: &Database,
    config: &AppConfig,
    device_id: &str,
    json: bool,
) -> Result<()> {
    let target = resolve_device_id(database, config, device_id).await?;
    let removal = remove_other_device(database, config, target).await?;
    let report = RemovalReport {
        version: 1,
        device_id: hex::encode(target),
        state: if removal.key_rotation_pending {
            "pending"
        } else {
            "complete"
        },
        access_revoked: removal.access_revoked,
        key_rotation_pending: removal.key_rotation_pending,
    };
    if json {
        return print_json_pretty(&report);
    }
    println!(
        "device-removed device_id={} state={} access_revoked={}",
        report.device_id, report.state, report.access_revoked
    );
    if report.state == "pending" {
        eprintln!(
            "Keys for future changes are not rotated yet. Rerun this command, or run \
             `aven sync` on any remaining device, to finish."
        );
    } else {
        eprintln!(
            "The removed device can no longer sync. It keeps the data it already \
             downloaded."
        );
    }
    Ok(())
}
