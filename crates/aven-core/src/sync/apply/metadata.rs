use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sqlx::SqliteConnection;

use crate::db::{entity_field_version, field_version, set_entity_field_version, set_field_version};
use crate::ids::{MetadataFieldId, TaskId, WorkspaceId};
use crate::metadata::{
    MetadataField, encode_metadata_conflict_value, metadata_field_by_id, metadata_field_by_key,
};
use crate::sync::wire::ChangeWire;
use crate::types::MutableEntityType;

use super::shared::{str_payload, task_id, workspace_id_payload};

#[derive(Debug, Deserialize)]
pub(super) struct MetadataValuePayload {
    pub(super) field_id: MetadataFieldId,
    pub(super) key: String,
    pub(super) value: String,
}

pub(super) async fn create_field(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let remote_id: MetadataFieldId = change.entity_id.parse()?;
    let key = str_payload(&change.payload, "key")?;
    let field = ensure_remote_field(
        conn,
        &workspace_id,
        &remote_id,
        &key,
        change
            .payload
            .get("created_at")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&change.created_at),
    )
    .await?;
    if entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::MetadataField,
        field.id.as_str(),
        "key",
    )
    .await?
    .is_none()
    {
        set_entity_field_version(
            conn,
            &workspace_id,
            MutableEntityType::MetadataField,
            field.id.as_str(),
            "key",
            &change.change_id,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn set_field(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let remote_id: MetadataFieldId = change.entity_id.parse()?;
    let key = str_payload(&change.payload, "key")?;
    let field =
        ensure_remote_field(conn, &workspace_id, &remote_id, &key, &change.created_at).await?;
    let current = entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::MetadataField,
        field.id.as_str(),
        "key",
    )
    .await?;
    let version_matches = current == change.base_version
        || aliased_base_matches_current_key(
            conn,
            &workspace_id,
            &remote_id,
            change.base_version.as_deref(),
            &field.key,
        )
        .await?;
    let force = change
        .payload
        .get("conflict_resolution")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !force && !version_matches {
        create_field_conflict(
            conn,
            change,
            &workspace_id,
            &field,
            &key,
            current.as_deref(),
        )
        .await?;
        return Ok(());
    }
    if let Some(existing) = metadata_field_by_key(conn, &workspace_id, &key).await?
        && existing.id != field.id
    {
        create_field_conflict(
            conn,
            change,
            &workspace_id,
            &field,
            &key,
            current.as_deref(),
        )
        .await?;
        return Ok(());
    }
    sqlx::query(
        "UPDATE metadata_fields SET key = ?, updated_at = ?
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(&key)
    .bind(&change.created_at)
    .bind(&workspace_id)
    .bind(&field.id)
    .execute(&mut *conn)
    .await?;
    set_entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::MetadataField,
        field.id.as_str(),
        "key",
        &change.change_id,
    )
    .await?;
    if force {
        sqlx::query(
            "UPDATE conflicts SET resolved = 1
             WHERE workspace_id = ? AND entity_type = 'metadata_field'
               AND entity_id = ? AND field = 'key' AND resolved = 0",
        )
        .bind(&workspace_id)
        .bind(&field.id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub(super) async fn set_task_value(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    apply_task_value(conn, change, true).await
}

pub(super) async fn remove_task_value(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    apply_task_value(conn, change, false).await
}

pub(super) async fn set_recurrence_value(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    apply_recurrence_value(conn, change, true).await
}

pub(super) async fn remove_recurrence_value(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    apply_recurrence_value(conn, change, false).await
}

async fn apply_recurrence_value(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    present: bool,
) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let series_id: crate::recurrence::RecurrenceSeriesId = change.entity_id.parse()?;
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM recurrence_series WHERE workspace_id = ? AND id = ?",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .fetch_one(&mut *conn)
    .await?;
    if exists != 1 {
        bail!("error recurrence-series-not-found series_id={series_id}");
    }
    let remote_id: MetadataFieldId = str_payload(&change.payload, "field_id")?.parse()?;
    let key = str_payload(&change.payload, "key")?;
    let field =
        ensure_remote_field(conn, &workspace_id, &remote_id, &key, &change.created_at).await?;
    let identity = format!("metadata:{}", field.id);
    let current_version = entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
    )
    .await?;
    let force = change
        .payload
        .get("conflict_resolution")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !force && current_version != change.base_version {
        let local_value = sqlx::query_scalar::<_, String>(
            "SELECT value FROM recurrence_series_metadata
             WHERE workspace_id = ? AND series_id = ? AND field_id = ?",
        )
        .bind(&workspace_id)
        .bind(&series_id)
        .bind(&field.id)
        .fetch_optional(&mut *conn)
        .await?;
        let remote_value = if present {
            Some(str_payload(&change.payload, "value")?)
        } else {
            None
        };
        insert_conflict(
            conn,
            change,
            &workspace_id,
            "recurrence_series",
            series_id.as_str(),
            "",
            &identity,
            &encode_metadata_conflict_value(local_value.as_deref())?,
            &encode_metadata_conflict_value(remote_value.as_deref())?,
            current_version.as_deref(),
        )
        .await?;
        return Ok(());
    }
    if present {
        let value = str_payload(&change.payload, "value")?;
        let (count, bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(value AS BLOB))), 0)
             FROM recurrence_series_metadata
             WHERE workspace_id = ? AND series_id = ? AND field_id != ?",
        )
        .bind(&workspace_id)
        .bind(&series_id)
        .bind(&field.id)
        .fetch_one(&mut *conn)
        .await?;
        if count + 1 > crate::metadata::MAX_METADATA_VALUES as i64
            || bytes + value.len() as i64 > crate::metadata::MAX_METADATA_TOTAL_BYTES as i64
        {
            bail!("error invalid-sync-change recurrence-metadata-limit");
        }
        sqlx::query(
            "INSERT INTO recurrence_series_metadata(
                 workspace_id, series_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(workspace_id, series_id, field_id)
             DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(&workspace_id)
        .bind(&series_id)
        .bind(&field.id)
        .bind(value)
        .bind(&change.created_at)
        .bind(&change.created_at)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM recurrence_series_metadata
             WHERE workspace_id = ? AND series_id = ? AND field_id = ?",
        )
        .bind(&workspace_id)
        .bind(&series_id)
        .bind(&field.id)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query("UPDATE recurrence_series SET updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&change.created_at)
        .bind(&workspace_id)
        .bind(&series_id)
        .execute(&mut *conn)
        .await?;
    set_entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
        &change.change_id,
    )
    .await?;
    if force {
        sqlx::query(
            "UPDATE conflicts SET resolved = 1
             WHERE workspace_id = ? AND entity_type = 'recurrence_series'
               AND entity_id = ? AND field = ? AND resolved = 0",
        )
        .bind(&workspace_id)
        .bind(&series_id)
        .bind(&identity)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn apply_task_value(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    present: bool,
) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let task_id = task_id(change)?;
    let remote_id: MetadataFieldId = str_payload(&change.payload, "field_id")?.parse()?;
    let key = str_payload(&change.payload, "key")?;
    let field =
        ensure_remote_field(conn, &workspace_id, &remote_id, &key, &change.created_at).await?;
    let identity = format!("metadata:{}", field.id);
    let current_version = field_version(conn, &task_id, &identity).await?;
    let force = change
        .payload
        .get("conflict_resolution")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !force && current_version != change.base_version {
        let remote_value = if present {
            Some(str_payload(&change.payload, "value")?)
        } else {
            None
        };
        create_task_conflict(
            conn,
            change,
            &workspace_id,
            &task_id,
            &field,
            remote_value.as_deref(),
            current_version.as_deref(),
        )
        .await?;
        return Ok(());
    }
    if present {
        let value = str_payload(&change.payload, "value")?;
        let (count, bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(value AS BLOB))), 0)
             FROM task_metadata
             WHERE workspace_id = ? AND task_id = ? AND field_id != ?",
        )
        .bind(&workspace_id)
        .bind(&task_id)
        .bind(&field.id)
        .fetch_one(&mut *conn)
        .await?;
        if count + 1 > crate::metadata::MAX_METADATA_VALUES as i64
            || bytes + value.len() as i64 > crate::metadata::MAX_METADATA_TOTAL_BYTES as i64
        {
            bail!("error invalid-sync-change task-metadata-limit");
        }
        sqlx::query(
            "INSERT INTO task_metadata(
                 workspace_id, task_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(workspace_id, task_id, field_id)
             DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(&workspace_id)
        .bind(&task_id)
        .bind(&field.id)
        .bind(value)
        .bind(&change.created_at)
        .bind(&change.created_at)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM task_metadata
             WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
        )
        .bind(&workspace_id)
        .bind(&task_id)
        .bind(&field.id)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query("UPDATE tasks SET updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&change.created_at)
        .bind(&workspace_id)
        .bind(&task_id)
        .execute(&mut *conn)
        .await?;
    set_field_version(conn, &task_id, &identity, &change.change_id).await?;
    if force {
        sqlx::query(
            "UPDATE conflicts SET resolved = 1
             WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ?
               AND field = ? AND resolved = 0",
        )
        .bind(&workspace_id)
        .bind(&task_id)
        .bind(&identity)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub(super) async fn initial_series_values_match(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &crate::recurrence::RecurrenceSeriesId,
    change: &ChangeWire,
) -> Result<bool> {
    let values = change
        .payload
        .get("metadata")
        .cloned()
        .map(serde_json::from_value::<Vec<MetadataValuePayload>>)
        .transpose()?
        .unwrap_or_default();
    let mut expected = Vec::with_capacity(values.len());
    for value in values {
        let field = ensure_remote_field(
            conn,
            workspace_id,
            &value.field_id,
            &value.key,
            &change.created_at,
        )
        .await?;
        expected.push((field.id, value.value));
    }
    expected.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    let actual: Vec<(MetadataFieldId, String)> = sqlx::query_as(
        "SELECT field_id, value FROM recurrence_series_metadata
         WHERE workspace_id = ? AND series_id = ? ORDER BY field_id",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(actual == expected)
}

pub(super) async fn apply_initial_series_values(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &crate::recurrence::RecurrenceSeriesId,
    change: &ChangeWire,
) -> Result<()> {
    let values = change
        .payload
        .get("metadata")
        .cloned()
        .map(serde_json::from_value::<Vec<MetadataValuePayload>>)
        .transpose()?
        .unwrap_or_default();
    for value in values {
        let field = ensure_remote_field(
            conn,
            workspace_id,
            &value.field_id,
            &value.key,
            &change.created_at,
        )
        .await?;
        sqlx::query(
            "INSERT OR REPLACE INTO recurrence_series_metadata(
                 workspace_id, series_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(series_id)
        .bind(&field.id)
        .bind(value.value)
        .bind(&change.created_at)
        .bind(&change.created_at)
        .execute(&mut *conn)
        .await?;
        set_entity_field_version(
            conn,
            workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            &format!("metadata:{}", field.id),
            &change.change_id,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn apply_initial_task_values(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    change: &ChangeWire,
) -> Result<()> {
    let values = change
        .payload
        .get("metadata")
        .cloned()
        .map(serde_json::from_value::<Vec<MetadataValuePayload>>)
        .transpose()?
        .unwrap_or_default();
    for value in values {
        let field = ensure_remote_field(
            conn,
            workspace_id,
            &value.field_id,
            &value.key,
            &change.created_at,
        )
        .await?;
        sqlx::query(
            "INSERT OR REPLACE INTO task_metadata(
                 workspace_id, task_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(&field.id)
        .bind(value.value)
        .bind(&change.created_at)
        .bind(&change.created_at)
        .execute(&mut *conn)
        .await?;
        set_field_version(
            conn,
            task_id,
            &format!("metadata:{}", field.id),
            &change.change_id,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn ensure_remote_field(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    remote_id: &MetadataFieldId,
    input_key: &str,
    created_at: &str,
) -> Result<MetadataField> {
    let key = crate::metadata::normalize_metadata_key(input_key)?;
    if let Some(local_id) = field_id_alias(conn, workspace_id, remote_id).await? {
        return metadata_field_by_id(conn, workspace_id, &local_id)
            .await?
            .context("error metadata-field-alias-target-missing");
    }
    if let Some(field) = metadata_field_by_id(conn, workspace_id, remote_id).await? {
        return Ok(field);
    }
    if let Some(field) = metadata_field_by_key(conn, workspace_id, &key).await? {
        insert_field_alias(conn, workspace_id, remote_id, &field.id).await?;
        return Ok(field);
    }
    sqlx::query(
        "INSERT INTO metadata_fields(id, workspace_id, key, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(remote_id)
    .bind(workspace_id)
    .bind(&key)
    .bind(created_at)
    .bind(created_at)
    .execute(&mut *conn)
    .await?;
    Ok(MetadataField {
        id: remote_id.clone(),
        workspace_id: workspace_id.clone(),
        key,
        created_at: created_at.to_string(),
        updated_at: created_at.to_string(),
    })
}

async fn aliased_base_matches_current_key(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    remote_id: &MetadataFieldId,
    base_version: Option<&str>,
    current_key: &str,
) -> Result<bool> {
    if field_id_alias(conn, workspace_id, remote_id)
        .await?
        .is_none()
    {
        return Ok(false);
    }
    let Some(base_version) = base_version else {
        return Ok(false);
    };
    let payload: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM changes
         WHERE change_id = ? AND entity_type = 'metadata_field' AND entity_id = ?
           AND op_type IN ('create_metadata_field', 'set_metadata_field')",
    )
    .bind(base_version)
    .bind(remote_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(payload) = payload else {
        return Ok(false);
    };
    let payload: serde_json::Value = serde_json::from_str(&payload)?;
    let Some(base_key) = payload.get("key").and_then(serde_json::Value::as_str) else {
        return Ok(false);
    };
    Ok(crate::metadata::normalize_metadata_key(base_key)? == current_key)
}

async fn field_id_alias(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    remote_id: &MetadataFieldId,
) -> Result<Option<MetadataFieldId>> {
    Ok(sqlx::query_scalar(
        "SELECT local_field_id FROM metadata_field_id_aliases
         WHERE workspace_id = ? AND remote_field_id = ?",
    )
    .bind(workspace_id)
    .bind(remote_id)
    .fetch_optional(&mut *conn)
    .await?)
}

async fn insert_field_alias(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    remote_id: &MetadataFieldId,
    local_id: &MetadataFieldId,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO metadata_field_id_aliases(
             workspace_id, remote_field_id, local_field_id
         ) VALUES (?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(remote_id)
    .bind(local_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn create_task_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    field: &MetadataField,
    remote_value: Option<&str>,
    local_change_id: Option<&str>,
) -> Result<()> {
    let identity = format!("metadata:{}", field.id);
    let local_value = sqlx::query_scalar::<_, String>(
        "SELECT value FROM task_metadata
         WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(&field.id)
    .fetch_optional(&mut *conn)
    .await?;
    let local_value = encode_metadata_conflict_value(local_value.as_deref())?;
    let remote_value = encode_metadata_conflict_value(remote_value)?;
    insert_conflict(
        conn,
        change,
        workspace_id,
        "task",
        task_id.as_str(),
        task_id.as_str(),
        &identity,
        &local_value,
        &remote_value,
        local_change_id,
    )
    .await
}

async fn create_field_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    workspace_id: &WorkspaceId,
    field: &MetadataField,
    remote_key: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    insert_conflict(
        conn,
        change,
        workspace_id,
        "metadata_field",
        field.id.as_str(),
        "",
        "key",
        &field.key,
        remote_key,
        local_change_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn insert_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    workspace_id: &WorkspaceId,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    field: &str,
    local_value: &str,
    remote_value: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    let variant_a = format!(
        "v{}",
        local_change_id
            .unwrap_or("local")
            .chars()
            .take(6)
            .collect::<String>()
    );
    let variant_b = format!("v{}", change.change_id.chars().take(6).collect::<String>());
    sqlx::query(
        "INSERT OR IGNORE INTO conflicts(
             workspace_id, entity_type, entity_id, task_id, field, base_version,
             local_value, remote_value, local_change_id, remote_change_id,
             variant_a, variant_b, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(task_id)
    .bind(field)
    .bind(&change.base_version)
    .bind(local_value)
    .bind(remote_value)
    .bind(local_change_id)
    .bind(&change.change_id)
    .bind(variant_a)
    .bind(variant_b)
    .bind(&change.created_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::set_field_version;
    use crate::projects::create_project;
    use crate::test_support::test_conn;
    use crate::workspaces::Workspace;

    async fn insert_task(
        conn: &mut SqliteConnection,
        workspace: &Workspace,
    ) -> (TaskId, crate::ids::ProjectId) {
        let project = create_project(conn, workspace, "app").await.unwrap();
        let task_id: TaskId = "7KQ9A1X4MV2P8D6R".parse().unwrap();
        sqlx::query(
            "INSERT INTO tasks(
                 workspace_id, id, title, description, project_id, status, priority,
                 created_at, updated_at
             ) VALUES (?, ?, 'task', '', ?, 'inbox', 'none', '2026-08-08T00:00:00Z',
                       '2026-08-08T00:00:00Z')",
        )
        .bind(&workspace.id)
        .bind(&task_id)
        .bind(&project.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        (task_id, project.id)
    }

    #[allow(clippy::too_many_arguments)]
    fn change(
        workspace: &Workspace,
        change_id: &str,
        entity_type: &str,
        entity_id: &str,
        field: Option<String>,
        op_type: &str,
        payload: serde_json::Value,
        base_version: Option<&str>,
    ) -> ChangeWire {
        let mut payload = payload.as_object().cloned().unwrap();
        payload.insert(
            "workspace_id".to_string(),
            serde_json::to_value(&workspace.id).unwrap(),
        );
        payload.insert(
            "workspace_key".to_string(),
            serde_json::Value::String(workspace.key.clone()),
        );
        ChangeWire {
            change_id: change_id.to_string(),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            field,
            op_type: op_type.to_string(),
            payload: serde_json::Value::Object(payload),
            base_version: base_version.map(str::to_string),
            created_at: "2026-08-08T00:00:01Z".to_string(),
            server_seq: Some(1),
        }
    }

    async fn persist_change(conn: &mut SqliteConnection, change: &ChangeWire) {
        sqlx::query(
            "INSERT INTO changes(
                 change_id, client_id, local_seq, entity_type, entity_id, field,
                 op_type, payload, base_version, created_at, server_seq
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&change.change_id)
        .bind(&change.client_id)
        .bind(change.local_seq)
        .bind(&change.entity_type)
        .bind(&change.entity_id)
        .bind(&change.field)
        .bind(&change.op_type)
        .bind(change.payload.to_string())
        .bind(&change.base_version)
        .bind(&change.created_at)
        .bind(change.server_seq)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    async fn aliased_field_setup(
        conn: &mut SqliteConnection,
        workspace: &Workspace,
    ) -> (MetadataField, MetadataFieldId, ChangeWire) {
        let local = crate::metadata::resolve_or_create_metadata_field(conn, workspace, "source")
            .await
            .unwrap();
        let remote_id: MetadataFieldId = "M7K9A1X4MV2P8D6R".parse().unwrap();
        let remote_create = change(
            workspace,
            "R7K9A1X4MV2P8D6R",
            "metadata_field",
            remote_id.as_str(),
            None,
            "create_metadata_field",
            json!({"key": "source", "created_at": "2026-08-08T00:00:00Z"}),
            None,
        );
        create_field(conn, &remote_create).await.unwrap();
        persist_change(conn, &remote_create).await;
        (local, remote_id, remote_create)
    }

    #[tokio::test]
    async fn concurrent_field_creation_aliases_remote_identity() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = Workspace::default();
        let (task_id, _) = insert_task(&mut conn, &workspace).await;
        let local =
            crate::metadata::resolve_or_create_metadata_field(&mut conn, &workspace, "source")
                .await
                .unwrap();
        let remote_id: MetadataFieldId = "M7K9A1X4MV2P8D6R".parse().unwrap();
        create_field(
            &mut conn,
            &change(
                &workspace,
                "R7K9A1X4MV2P8D6R",
                "metadata_field",
                remote_id.as_str(),
                None,
                "create_metadata_field",
                json!({"key": "source", "created_at": "2026-08-08T00:00:00Z"}),
                None,
            ),
        )
        .await
        .unwrap();
        set_task_value(
            &mut conn,
            &change(
                &workspace,
                "S7K9A1X4MV2P8D6R",
                "task",
                task_id.as_str(),
                Some(format!("metadata:{remote_id}")),
                "set_task_metadata",
                json!({"field_id": remote_id, "key": "source", "value": "github"}),
                None,
            ),
        )
        .await
        .unwrap();

        let stored: (String, String) = sqlx::query_as(
            "SELECT field_id, value FROM task_metadata
             WHERE workspace_id = ? AND task_id = ?",
        )
        .bind(&workspace.id)
        .bind(&task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(stored.0, local.id.as_str());
        assert_eq!(stored.1, "github");
        assert!(
            metadata_field_by_id(&mut conn, &workspace.id, &remote_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn remote_field_rename_follows_concurrent_creation_alias() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = Workspace::default();
        let (local, remote_id, remote_create) = aliased_field_setup(&mut conn, &workspace).await;

        set_field(
            &mut conn,
            &change(
                &workspace,
                "S7K9A1X4MV2P8D6R",
                "metadata_field",
                remote_id.as_str(),
                Some("key".to_string()),
                "set_metadata_field",
                json!({"key": "origin"}),
                Some(&remote_create.change_id),
            ),
        )
        .await
        .unwrap();

        let field = metadata_field_by_id(&mut conn, &workspace.id, &local.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(field.key, "origin");
        let conflicts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conflicts
             WHERE workspace_id = ? AND entity_type = 'metadata_field' AND entity_id = ?",
        )
        .bind(&workspace.id)
        .bind(&local.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(conflicts, 0);
    }

    #[tokio::test]
    async fn aliased_field_rename_conflicts_with_a_local_rename() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = Workspace::default();
        let (local, remote_id, remote_create) = aliased_field_setup(&mut conn, &workspace).await;
        crate::metadata::rename_metadata_field(&mut conn, &workspace, "source", "local-origin")
            .await
            .unwrap();

        set_field(
            &mut conn,
            &change(
                &workspace,
                "S7K9A1X4MV2P8D6R",
                "metadata_field",
                remote_id.as_str(),
                Some("key".to_string()),
                "set_metadata_field",
                json!({"key": "remote-origin"}),
                Some(&remote_create.change_id),
            ),
        )
        .await
        .unwrap();

        let field = metadata_field_by_id(&mut conn, &workspace.id, &local.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(field.key, "local-origin");
        let conflict: (String, String) = sqlx::query_as(
            "SELECT local_value, remote_value FROM conflicts
             WHERE workspace_id = ? AND entity_type = 'metadata_field' AND entity_id = ?",
        )
        .bind(&workspace.id)
        .bind(&local.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            conflict,
            ("local-origin".to_string(), "remote-origin".to_string())
        );
    }

    #[tokio::test]
    async fn remote_field_rename_updates_one_stable_identity() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = Workspace::default();
        let remote_id: MetadataFieldId = "M7K9A1X4MV2P8D6R".parse().unwrap();
        create_field(
            &mut conn,
            &change(
                &workspace,
                "R7K9A1X4MV2P8D6R",
                "metadata_field",
                remote_id.as_str(),
                None,
                "create_metadata_field",
                json!({"key": "review_on", "created_at": "2026-08-08T00:00:00Z"}),
                None,
            ),
        )
        .await
        .unwrap();
        set_field(
            &mut conn,
            &change(
                &workspace,
                "S7K9A1X4MV2P8D6R",
                "metadata_field",
                remote_id.as_str(),
                Some("key".to_string()),
                "set_metadata_field",
                json!({"key": "review_date"}),
                Some("R7K9A1X4MV2P8D6R"),
            ),
        )
        .await
        .unwrap();

        let field = metadata_field_by_id(&mut conn, &workspace.id, &remote_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(field.key, "review_date");
        let value_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_metadata")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(value_count, 0);
    }

    #[tokio::test]
    async fn metadata_conflicts_distinguish_empty_value_from_absence() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = Workspace::default();
        let (task_id, _) = insert_task(&mut conn, &workspace).await;
        let field =
            crate::metadata::resolve_or_create_metadata_field(&mut conn, &workspace, "reviewed")
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO task_metadata(
                 workspace_id, task_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, '', '2026-08-08T00:00:00Z', '2026-08-08T00:00:00Z')",
        )
        .bind(&workspace.id)
        .bind(&task_id)
        .bind(&field.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        let identity = format!("metadata:{}", field.id);
        set_field_version(&mut conn, &task_id, &identity, "local-change")
            .await
            .unwrap();

        remove_task_value(
            &mut conn,
            &change(
                &workspace,
                "S7K9A1X4MV2P8D6R",
                "task",
                task_id.as_str(),
                Some(identity.clone()),
                "remove_task_metadata",
                json!({"field_id": field.id, "key": "reviewed"}),
                Some("different-base"),
            ),
        )
        .await
        .unwrap();

        let (local, remote): (String, String) = sqlx::query_as(
            "SELECT local_value, remote_value FROM conflicts
             WHERE workspace_id = ? AND entity_id = ? AND field = ?",
        )
        .bind(&workspace.id)
        .bind(&task_id)
        .bind(&identity)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            crate::metadata::decode_metadata_conflict_value(&local).unwrap(),
            Some(String::new())
        );
        assert_eq!(
            crate::metadata::decode_metadata_conflict_value(&remote).unwrap(),
            None
        );
        crate::operations::conflicts::resolve_conflict(
            &mut conn, &workspace, &task_id, &identity, &remote,
        )
        .await
        .unwrap();
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_metadata WHERE workspace_id = ? AND task_id = ?",
        )
        .bind(&workspace.id)
        .bind(&task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
        let operation: String = sqlx::query_scalar(
            "SELECT op_type FROM changes WHERE entity_id = ? ORDER BY local_seq DESC LIMIT 1",
        )
        .bind(&task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(operation, "remove_task_metadata");
    }
}
