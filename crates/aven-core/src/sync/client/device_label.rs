//! This device's published label, shown by other devices.
use anyhow::Result;

use super::host::ClientHost;
use super::keys::ProtectedLocalKeyStore;
use crate::db::Database;

const MAX_CHARS: usize = 64;
const MAX_BYTES: usize = 256;

/// Publishes the host's label for this device unless it already has one.
pub(crate) async fn publish_if_missing(
    database: &Database,
    store: &ProtectedLocalKeyStore,
    host: &dyn ClientHost,
) -> Result<()> {
    let (_, server) = store
        .association(database)
        .await?
        .ok_or_else(|| anyhow::anyhow!("error sync-association-missing"))?;
    let device = store.active_inputs(database, &server).await?.device();
    if database.device_labels().await?.contains_key(&device) {
        return Ok(());
    }
    if let Some(label) = host.device_label().as_deref().and_then(clean_label) {
        database.publish_device_label(device, &label).await?;
    }
    Ok(())
}

/// Drops control characters and bounds the label for display and the wire.
pub fn clean_label(value: &str) -> Option<String> {
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
