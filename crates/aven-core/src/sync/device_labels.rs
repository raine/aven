use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde_json::json;
use sqlx::Row;

use crate::change_log::op_type;
use crate::db::{Database, insert_change};

pub const MAX_DEVICE_LABEL_CHARS: usize = 64;
pub const MAX_DEVICE_LABEL_BYTES: usize = 256;

pub(crate) fn validate_device_label(label: &str) -> Result<()> {
    ensure!(
        !label.is_empty()
            && label.chars().count() <= MAX_DEVICE_LABEL_CHARS
            && label.len() <= MAX_DEVICE_LABEL_BYTES
            && !label.chars().any(char::is_control),
        "error invalid-sync-change device-label"
    );
    Ok(())
}

impl Database {
    /// Publishes this installation's automatic label once. Device labels are
    /// display metadata; the membership device ID remains the identity.
    pub async fn publish_device_label(&self, device: [u8; 32], label: &str) -> Result<()> {
        validate_device_label(label)?;
        let device_id = hex::encode(device);
        let mut conn = self.acquire_writer().await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM changes
                 WHERE op_type = 'publish_device_label' AND entity_id = ?
             )",
        )
        .bind(&device_id)
        .fetch_one(&mut *conn)
        .await?;
        if exists {
            return Ok(());
        }
        insert_change(
            &mut conn,
            "device",
            &device_id,
            None,
            op_type::PUBLISH_DEVICE_LABEL,
            json!({ "label": label }),
            None,
        )
        .await?;
        Ok(())
    }

    /// Reads effective labels from encrypted retained history. Callers pair
    /// these with verified membership, so removed devices are not displayed.
    pub async fn device_labels(&self) -> Result<BTreeMap<[u8; 32], String>> {
        let mut conn = self.acquire_reader().await?;
        let rows = sqlx::query(
            "SELECT entity_id, payload
             FROM changes
             WHERE op_type = 'publish_device_label'
             ORDER BY (server_seq IS NULL), server_seq, local_seq",
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut labels = BTreeMap::new();
        for row in rows {
            let device_id: String = row.try_get("entity_id")?;
            let payload: String = row.try_get("payload")?;
            let Ok(bytes) = hex::decode(&device_id) else {
                continue;
            };
            let Ok(device) = <[u8; 32]>::try_from(bytes) else {
                continue;
            };
            let Some(label) = serde_json::from_str::<serde_json::Value>(&payload)?
                .get("label")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
            else {
                continue;
            };
            labels.entry(device).or_insert(label);
        }
        Ok(labels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_once_and_reads_the_label() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("aven.sqlite"))
            .await
            .unwrap();
        let device = [0xab; 32];
        database
            .publish_device_label(device, "Office Mac")
            .await
            .unwrap();
        database
            .publish_device_label(device, "Renamed Mac")
            .await
            .unwrap();
        assert_eq!(
            database.device_labels().await.unwrap()[&device],
            "Office Mac"
        );
    }

    #[test]
    fn rejects_unsafe_or_oversized_labels() {
        assert!(validate_device_label("Linux laptop").is_ok());
        assert!(validate_device_label("").is_err());
        assert!(validate_device_label("bad\nname").is_err());
        assert!(validate_device_label(&"x".repeat(65)).is_err());
    }
}
