use anyhow::Result;
use aven_core::db::Database;

use crate::protected_local_keys::ProtectedLocalKeyStore;

const MAX_CHARS: usize = 64;
const MAX_BYTES: usize = 256;

pub(super) async fn publish_if_missing(
    database: &Database,
    store: &ProtectedLocalKeyStore,
) -> Result<()> {
    let (_, server) = store
        .association(database)
        .await?
        .ok_or_else(|| anyhow::anyhow!("error sync-association-missing"))?;
    let device = store.active_inputs(database, &server).await?.device();
    if database.device_labels().await?.contains_key(&device) {
        return Ok(());
    }
    if let Some(label) = automatic_label() {
        database.publish_device_label(device, &label).await?;
    }
    Ok(())
}

fn clean_label(value: &str) -> Option<String> {
    let mut label = String::new();
    for character in value.trim().chars() {
        if character.is_control() {
            continue;
        }
        if label.chars().count() == MAX_CHARS || label.len() + character.len_utf8() > MAX_BYTES {
            break;
        }
        label.push(character);
    }
    (!label.is_empty()).then_some(label)
}

#[cfg(target_os = "macos")]
fn automatic_label() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
        .and_then(|label| clean_label(&label))
}

#[cfg(target_os = "linux")]
fn automatic_label() -> Option<String> {
    let mut bytes = [0_u8; 256];
    // SAFETY: `bytes` is writable for the supplied length. gethostname writes
    // at most that many bytes and does not retain the pointer.
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return None;
    }
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..length])
        .ok()
        .and_then(clean_label)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn automatic_label() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_automatic_labels_for_display_and_wire_limits() {
        assert_eq!(clean_label("  Office Mac\n"), Some("Office Mac".into()));
        assert_eq!(clean_label("bad\u{0007}name"), Some("badname".into()));
        assert_eq!(clean_label("\n"), None);
        assert_eq!(clean_label(&"x".repeat(80)).unwrap().len(), 64);
    }
}
