//! Listing and removing the devices that take part in sync.
//!
//! Both operations refresh verified membership from the server first. Removal
//! delegates to the engine's durable removal and rotation; rerunning it
//! resumes a retained removal instead of starting another.
use anyhow::{Result, bail};
use aven_core::db::Database;
use aven_core::sync::client::engine;
pub(crate) use aven_core::sync::client::engine::{Device, DeviceListing, Removal};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use super::{DesktopHost, driver};
use crate::config::AppConfig;
use crate::render::print_json_pretty;

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

pub(crate) async fn load_devices(database: &Database, config: &AppConfig) -> Result<DeviceListing> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::load_devices(link, database, &host))
        .await
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

/// Removes another device, or resumes its retained removal.
pub(crate) async fn remove_other_device(
    database: &Database,
    config: &AppConfig,
    target: [u8; 32],
) -> Result<Removal> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::remove_other_device(link, database, &host, target))
        .await
}

/// Continues an unfinished removal or key rotation, as an ordinary sync
/// round would. Returns whether rotation is still pending.
pub(crate) async fn finish_removal(database: &Database, config: &AppConfig) -> Result<bool> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::finish_removal(link, database, &host))
        .await
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

    fn entry(device_id: &str) -> DeviceEntry {
        DeviceEntry {
            device_id: device_id.into(),
            label: None,
            current: false,
            admission_sequence: 1,
        }
    }

    #[test]
    fn short_device_ids_extend_past_eight_only_while_prefixes_collide() {
        let distinct = [entry(&"a".repeat(64)), entry(&"b".repeat(64))];
        assert_eq!(
            short_device_ids(&distinct),
            [format!("{}…", "a".repeat(8)), format!("{}…", "b".repeat(8))]
        );
        let shared = format!("{}0", "c".repeat(10));
        let colliding = [
            entry(&format!("{shared}{}", "1".repeat(53))),
            entry(&format!("{shared}{}", "2".repeat(53))),
            entry(&"d".repeat(64)),
        ];
        assert_eq!(
            short_device_ids(&colliding),
            [
                format!("{shared}1…"),
                format!("{shared}2…"),
                format!("{}…", "d".repeat(12)),
            ]
        );
    }
}
