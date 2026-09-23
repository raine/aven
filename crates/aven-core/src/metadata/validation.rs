use std::collections::{BTreeSet, HashMap};

use anyhow::{Result, bail};
use sqlx::{Row, SqliteConnection};

use crate::ids::{TaskId, WorkspaceId};
use crate::recurrence::RecurrenceSeriesId;

use super::fields::normalize_metadata_key;
use super::{
    MAX_METADATA_TOTAL_BYTES, MAX_METADATA_VALUE_BYTES, MAX_METADATA_VALUES, TaskMetadataInput,
};

pub(crate) fn validate_metadata_update(set: &[TaskMetadataInput], remove: &[String]) -> Result<()> {
    if set.len() > MAX_METADATA_VALUES {
        bail!("error too-many-metadata-values limit={MAX_METADATA_VALUES}");
    }
    let mut keys = BTreeSet::new();
    let mut total_bytes = 0usize;
    for input in set {
        let key = normalize_metadata_key(&input.key)?;
        if !keys.insert(key) {
            bail!("error duplicate-metadata-key");
        }
        if input.value.len() > MAX_METADATA_VALUE_BYTES {
            bail!("error metadata-value-too-large limit={MAX_METADATA_VALUE_BYTES}");
        }
        total_bytes = total_bytes.saturating_add(input.value.len());
    }
    if total_bytes > MAX_METADATA_TOTAL_BYTES {
        bail!("error metadata-values-too-large limit={MAX_METADATA_TOTAL_BYTES}");
    }
    for input in remove {
        let key = normalize_metadata_key(input)?;
        if !keys.insert(key) {
            bail!("error overlapping-metadata-key");
        }
    }
    Ok(())
}

pub(super) fn validate_metadata_result_limits(
    existing: impl IntoIterator<Item = (String, String)>,
    set: &[TaskMetadataInput],
    remove: &[String],
) -> Result<()> {
    let mut values = existing.into_iter().collect::<HashMap<_, _>>();
    for key in remove {
        values.remove(&normalize_metadata_key(key)?);
    }
    for input in set {
        values.insert(normalize_metadata_key(&input.key)?, input.value.clone());
    }
    if values.len() > MAX_METADATA_VALUES {
        bail!("error too-many-metadata-values limit={MAX_METADATA_VALUES}");
    }
    let total_bytes = values.values().map(String::len).sum::<usize>();
    if total_bytes > MAX_METADATA_TOTAL_BYTES {
        bail!("error metadata-values-too-large limit={MAX_METADATA_TOTAL_BYTES}");
    }
    Ok(())
}

// Task aggregates are local admission limits, not bounds on concurrent merged state.
fn validate_task_metadata_result_limits(
    existing: impl IntoIterator<Item = (String, String)>,
    set: &[TaskMetadataInput],
    remove: &[String],
) -> Result<()> {
    let mut values = existing.into_iter().collect::<HashMap<_, _>>();
    let before_count = values.len();
    let before_bytes = values.values().map(String::len).sum::<usize>();
    for key in remove {
        values.remove(&normalize_metadata_key(key)?);
    }
    for input in set {
        values.insert(normalize_metadata_key(&input.key)?, input.value.clone());
    }
    if values.len() > MAX_METADATA_VALUES && values.len() > before_count {
        bail!("error too-many-metadata-values limit={MAX_METADATA_VALUES}");
    }
    let total_bytes = values.values().map(String::len).sum::<usize>();
    if total_bytes > MAX_METADATA_TOTAL_BYTES && total_bytes > before_bytes {
        bail!("error metadata-values-too-large limit={MAX_METADATA_TOTAL_BYTES}");
    }
    Ok(())
}

pub(crate) async fn validate_recurrence_metadata_result(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    set: &[TaskMetadataInput],
    remove: &[String],
) -> Result<()> {
    validate_metadata_update(set, remove)?;
    let rows = sqlx::query(
        "SELECT f.key, m.value FROM recurrence_series_metadata m
         JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id
         WHERE m.workspace_id = ? AND m.series_id = ?",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?;
    validate_metadata_result_limits(
        rows.into_iter()
            .map(|row| (row.get::<String, _>("key"), row.get::<String, _>("value"))),
        set,
        remove,
    )
}

pub(crate) async fn validate_task_metadata_result(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    set: &[TaskMetadataInput],
    remove: &[String],
) -> Result<()> {
    validate_metadata_update(set, remove)?;
    let rows = sqlx::query(
        "SELECT f.key, m.value FROM task_metadata m
         JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id
         WHERE m.workspace_id = ? AND m.task_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    validate_task_metadata_result_limits(
        rows.into_iter()
            .map(|row| (row.get::<String, _>("key"), row.get::<String, _>("value"))),
        set,
        remove,
    )
}

#[cfg(test)]
mod tests;
