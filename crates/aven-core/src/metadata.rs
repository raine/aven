use std::collections::{BTreeSet, HashMap};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::db::{
    Database, begin_immediate, conflict_exists, entity_conflict_exists, entity_field_version,
    field_version, insert_change, set_entity_field_version, set_field_version,
};
use crate::ids::{MetadataFieldId, TaskId, WorkspaceId, now};
use crate::recurrence::RecurrenceSeriesId;
use crate::types::MutableEntityType;
use crate::workspaces::Workspace;

pub(crate) const MAX_METADATA_VALUES: usize = 128;
pub(crate) const MAX_METADATA_VALUE_BYTES: usize = 4 * 1024;
pub(crate) const MAX_METADATA_TOTAL_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataField {
    pub id: MetadataFieldId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMetadataValue {
    pub field_id: MetadataFieldId,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMetadataInput {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataFieldUsage {
    pub field: MetadataField,
    pub task_count: usize,
    pub series_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolvedMetadataValue {
    pub(crate) field_id: MetadataFieldId,
    pub(crate) key: String,
    pub(crate) value: String,
}

#[derive(Serialize, Deserialize)]
struct MetadataConflictValue {
    present: bool,
    value: String,
}

pub(crate) fn encode_metadata_conflict_value(value: Option<&str>) -> Result<String> {
    Ok(serde_json::to_string(&MetadataConflictValue {
        present: value.is_some(),
        value: value.unwrap_or_default().to_string(),
    })?)
}

pub(crate) fn decode_metadata_conflict_value(value: &str) -> Result<Option<String>> {
    let value: MetadataConflictValue = serde_json::from_str(value)?;
    Ok(value.present.then_some(value.value))
}

impl Database {
    pub async fn list_metadata_fields(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<MetadataFieldUsage>> {
        let mut conn = self.acquire_reader().await?;
        list_metadata_fields_in_workspace(&mut conn, workspace_id).await
    }

    pub async fn find_metadata_field(
        &self,
        workspace_id: &WorkspaceId,
        key: &str,
    ) -> Result<Option<MetadataField>> {
        let mut conn = self.acquire_reader().await?;
        find_metadata_field_in_workspace(&mut conn, workspace_id, key).await
    }

    pub async fn task_metadata(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<Vec<TaskMetadataValue>> {
        let mut conn = self.acquire_reader().await?;
        task_metadata_in_workspace(&mut conn, workspace_id, task_id).await
    }

    pub async fn rename_metadata_field(
        &self,
        workspace: &Workspace,
        key: &str,
        new_key: &str,
    ) -> Result<MetadataField> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let field = rename_metadata_field(&mut tx, workspace, key, new_key).await?;
        tx.commit().await?;
        Ok(field)
    }
}

pub fn normalize_metadata_key(input: &str) -> Result<String> {
    let key = input.trim().to_ascii_lowercase();
    let mut bytes = key.bytes();
    let Some(first) = bytes.next() else {
        bail!("error invalid-metadata-key");
    };
    if !first.is_ascii_lowercase()
        || key.len() > 64
        || !bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
        || key.starts_with("aven.")
    {
        bail!("error invalid-metadata-key");
    }
    Ok(key)
}

pub(crate) async fn find_metadata_field_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    input: &str,
) -> Result<Option<MetadataField>> {
    let key = normalize_metadata_key(input)?;
    metadata_field_by_key(conn, workspace_id, &key).await
}

pub(crate) async fn metadata_field_by_key(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    key: &str,
) -> Result<Option<MetadataField>> {
    let row = sqlx::query(
        "SELECT id, workspace_id, key, created_at, updated_at
         FROM metadata_fields WHERE workspace_id = ? AND key = ?",
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(metadata_field_from_row))
}

pub(crate) async fn metadata_field_by_id(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    field_id: &MetadataFieldId,
) -> Result<Option<MetadataField>> {
    let row = sqlx::query(
        "SELECT id, workspace_id, key, created_at, updated_at
         FROM metadata_fields WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(field_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(metadata_field_from_row))
}

pub(crate) async fn resolve_or_create_metadata_field(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    input: &str,
) -> Result<MetadataField> {
    let key = normalize_metadata_key(input)?;
    if let Some(field) = metadata_field_by_key(conn, &workspace.id, &key).await? {
        return Ok(field);
    }

    let id = MetadataFieldId::new();
    let timestamp = now();
    sqlx::query(
        "INSERT INTO metadata_fields(id, workspace_id, key, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&workspace.id)
    .bind(&key)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&mut *conn)
    .await?;
    let change_id = insert_change(
        conn,
        "metadata_field",
        id.as_str(),
        None,
        "create_metadata_field",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "key": &key,
            "created_at": &timestamp,
        }),
        None,
    )
    .await?;
    set_entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::MetadataField,
        id.as_str(),
        "key",
        &change_id,
    )
    .await?;
    Ok(MetadataField {
        id,
        workspace_id: workspace.id.clone(),
        key,
        created_at: timestamp.clone(),
        updated_at: timestamp,
    })
}

pub(crate) async fn rename_metadata_field(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    input: &str,
    new_input: &str,
) -> Result<MetadataField> {
    let key = normalize_metadata_key(input)?;
    let new_key = normalize_metadata_key(new_input)?;
    let mut field = metadata_field_by_key(conn, &workspace.id, &key)
        .await?
        .ok_or_else(|| anyhow::anyhow!("error unknown-metadata-field"))?;
    let resolving = entity_conflict_exists(
        conn,
        &workspace.id,
        MutableEntityType::MetadataField,
        field.id.as_str(),
        "key",
    )
    .await?;
    if key == new_key && !resolving {
        return Ok(field);
    }
    if key != new_key
        && metadata_field_by_key(conn, &workspace.id, &new_key)
            .await?
            .is_some()
    {
        bail!("error metadata-field-exists");
    }
    let base = if resolving {
        None
    } else {
        entity_field_version(
            conn,
            &workspace.id,
            MutableEntityType::MetadataField,
            field.id.as_str(),
            "key",
        )
        .await?
    };
    let timestamp = now();
    sqlx::query(
        "UPDATE metadata_fields SET key = ?, updated_at = ?
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(&new_key)
    .bind(&timestamp)
    .bind(&workspace.id)
    .bind(&field.id)
    .execute(&mut *conn)
    .await?;
    let change_id = insert_change(
        conn,
        "metadata_field",
        field.id.as_str(),
        Some("key"),
        "set_metadata_field",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "key": &new_key,
            "conflict_resolution": resolving,
        }),
        base.as_deref(),
    )
    .await?;
    set_entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::MetadataField,
        field.id.as_str(),
        "key",
        &change_id,
    )
    .await?;
    if resolving {
        sqlx::query(
            "UPDATE conflicts SET resolved = 1
             WHERE workspace_id = ? AND entity_type = 'metadata_field'
               AND entity_id = ? AND field = 'key' AND resolved = 0",
        )
        .bind(&workspace.id)
        .bind(&field.id)
        .execute(&mut *conn)
        .await?;
    }
    field.key = new_key;
    field.updated_at = timestamp;
    Ok(field)
}

pub(crate) async fn insert_initial_task_metadata(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    inputs: &[TaskMetadataInput],
    timestamp: &str,
) -> Result<Vec<ResolvedMetadataValue>> {
    let values = resolve_metadata_inputs(conn, workspace, inputs).await?;
    for value in &values {
        sqlx::query(
            "INSERT INTO task_metadata(
                 workspace_id, task_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(&value.field_id)
        .bind(&value.value)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *conn)
        .await?;
    }
    Ok(values)
}

pub(crate) async fn set_task_metadata(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    input: &TaskMetadataInput,
) -> Result<bool> {
    let field = resolve_or_create_metadata_field(conn, workspace, &input.key).await?;
    let identity = format!("metadata:{}", field.id);
    let current = sqlx::query_scalar::<_, String>(
        "SELECT value FROM task_metadata
         WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(&field.id)
    .fetch_optional(&mut *conn)
    .await?;
    if current.as_deref() == Some(input.value.as_str()) {
        return Ok(false);
    }
    if conflict_exists(conn, &workspace.id, task_id, &identity).await? {
        bail!("error conflicted-field ref={task_id} field={identity}");
    }
    let base = field_version(conn, task_id, &identity).await?;
    let timestamp = now();
    sqlx::query(
        "INSERT INTO task_metadata(
             workspace_id, task_id, field_id, value, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(workspace_id, task_id, field_id)
         DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(&field.id)
    .bind(&input.value)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&mut *conn)
    .await?;
    touch_task(conn, &workspace.id, task_id, &timestamp).await?;
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(&identity),
        "set_task_metadata",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "field_id": &field.id,
            "key": &field.key,
            "value": &input.value,
        }),
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, task_id, &identity, &change_id).await?;
    Ok(true)
}

pub(crate) async fn remove_task_metadata(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    input: &str,
) -> Result<bool> {
    let key = normalize_metadata_key(input)?;
    let Some(field) = metadata_field_by_key(conn, &workspace.id, &key).await? else {
        bail!("error unknown-metadata-field");
    };
    let identity = format!("metadata:{}", field.id);
    if conflict_exists(conn, &workspace.id, task_id, &identity).await? {
        bail!("error conflicted-field ref={task_id} field={identity}");
    }
    let base = field_version(conn, task_id, &identity).await?;
    let removed_value = sqlx::query_scalar::<_, String>(
        "SELECT value FROM task_metadata
         WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(&field.id)
    .fetch_optional(&mut *conn)
    .await?;
    let deleted = sqlx::query(
        "DELETE FROM task_metadata
         WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(&field.id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Ok(false);
    }
    let timestamp = now();
    touch_task(conn, &workspace.id, task_id, &timestamp).await?;
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(&identity),
        "remove_task_metadata",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "field_id": &field.id,
            "key": &field.key,
            "value": removed_value,
        }),
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, task_id, &identity, &change_id).await?;
    Ok(true)
}

pub(crate) async fn set_recurrence_metadata(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    input: &TaskMetadataInput,
) -> Result<bool> {
    let field = resolve_or_create_metadata_field(conn, workspace, &input.key).await?;
    let identity = format!("metadata:{}", field.id);
    let current = sqlx::query_scalar::<_, String>(
        "SELECT value FROM recurrence_series_metadata
         WHERE workspace_id = ? AND series_id = ? AND field_id = ?",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .bind(&field.id)
    .fetch_optional(&mut *conn)
    .await?;
    if current.as_deref() == Some(input.value.as_str()) {
        return Ok(false);
    }
    if entity_conflict_exists(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
    )
    .await?
    {
        bail!("error conflicted-recurrence-field field={identity}");
    }
    let base = entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
    )
    .await?;
    let timestamp = now();
    sqlx::query(
        "INSERT INTO recurrence_series_metadata(
             workspace_id, series_id, field_id, value, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(workspace_id, series_id, field_id)
         DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .bind(&field.id)
    .bind(&input.value)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE recurrence_series SET updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&timestamp)
        .bind(&workspace.id)
        .bind(series_id)
        .execute(&mut *conn)
        .await?;
    let change_id = insert_change(
        conn,
        "recurrence_series",
        series_id.as_str(),
        Some(&identity),
        "set_recurrence_metadata",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "field_id": &field.id,
            "key": &field.key,
            "value": &input.value,
        }),
        base.as_deref(),
    )
    .await?;
    set_entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
        &change_id,
    )
    .await?;
    Ok(true)
}

pub(crate) async fn remove_recurrence_metadata(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    key: &str,
) -> Result<bool> {
    let Some(field) =
        metadata_field_by_key(conn, &workspace.id, &normalize_metadata_key(key)?).await?
    else {
        return Ok(false);
    };
    let identity = format!("metadata:{}", field.id);
    if entity_conflict_exists(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
    )
    .await?
    {
        bail!("error conflicted-recurrence-field field={identity}");
    }
    let removed = sqlx::query(
        "DELETE FROM recurrence_series_metadata
         WHERE workspace_id = ? AND series_id = ? AND field_id = ?",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .bind(&field.id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if removed == 0 {
        return Ok(false);
    }
    let base = entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
    )
    .await?;
    let timestamp = now();
    sqlx::query("UPDATE recurrence_series SET updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&timestamp)
        .bind(&workspace.id)
        .bind(series_id)
        .execute(&mut *conn)
        .await?;
    let change_id = insert_change(
        conn,
        "recurrence_series",
        series_id.as_str(),
        Some(&identity),
        "remove_recurrence_metadata",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "field_id": &field.id,
            "key": &field.key,
        }),
        base.as_deref(),
    )
    .await?;
    set_entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        &identity,
        &change_id,
    )
    .await?;
    Ok(true)
}

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

fn validate_metadata_result_limits(
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
    validate_metadata_result_limits(
        rows.into_iter()
            .map(|row| (row.get::<String, _>("key"), row.get::<String, _>("value"))),
        set,
        remove,
    )
}

pub(crate) async fn resolve_metadata_inputs(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    inputs: &[TaskMetadataInput],
) -> Result<Vec<ResolvedMetadataValue>> {
    validate_metadata_update(inputs, &[])?;
    let mut values = Vec::with_capacity(inputs.len());
    for input in inputs {
        let field = resolve_or_create_metadata_field(conn, workspace, &input.key).await?;
        values.push(ResolvedMetadataValue {
            field_id: field.id,
            key: field.key,
            value: input.value.clone(),
        });
    }
    Ok(values)
}

async fn touch_task(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    timestamp: &str,
) -> Result<()> {
    let affected = sqlx::query("UPDATE tasks SET updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(timestamp)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if affected != 1 {
        bail!("error task-not-found task_id={task_id}");
    }
    Ok(())
}

pub(crate) async fn task_metadata_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
) -> Result<Vec<TaskMetadataValue>> {
    let rows = sqlx::query(
        "SELECT m.field_id, f.key, m.value
         FROM task_metadata m
         JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id
         WHERE m.workspace_id = ? AND m.task_id = ?
         ORDER BY f.key",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| TaskMetadataValue {
            field_id: row.get("field_id"),
            key: row.get("key"),
            value: row.get("value"),
        })
        .collect())
}

pub(crate) async fn metadata_by_task_ids(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, Vec<TaskMetadataValue>>> {
    let mut values = HashMap::new();
    for chunk in task_ids.chunks(900) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT m.task_id, m.field_id, f.key, m.value
             FROM task_metadata m
             JOIN metadata_fields f
               ON f.workspace_id = m.workspace_id AND f.id = m.field_id
             WHERE m.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND m.task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY m.task_id, f.key");
        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: TaskId = row.get("task_id");
            values
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(TaskMetadataValue {
                    field_id: row.get("field_id"),
                    key: row.get("key"),
                    value: row.get("value"),
                });
        }
    }
    Ok(values)
}

async fn list_metadata_fields_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<Vec<MetadataFieldUsage>> {
    let rows = sqlx::query(
        "SELECT f.id, f.workspace_id, f.key, f.created_at, f.updated_at,
                (SELECT count(*) FROM task_metadata m
                 WHERE m.workspace_id = f.workspace_id AND m.field_id = f.id) AS task_count,
                (SELECT count(*) FROM recurrence_series_metadata m
                 WHERE m.workspace_id = f.workspace_id AND m.field_id = f.id) AS series_count
         FROM metadata_fields f
         WHERE f.workspace_id = ?
         ORDER BY f.key",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| MetadataFieldUsage {
            field: MetadataField {
                id: row.get("id"),
                workspace_id: row.get("workspace_id"),
                key: row.get("key"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            },
            task_count: usize::try_from(row.get::<i64, _>("task_count")).unwrap_or(usize::MAX),
            series_count: usize::try_from(row.get::<i64, _>("series_count")).unwrap_or(usize::MAX),
        })
        .collect())
}

fn metadata_field_from_row(row: sqlx::sqlite::SqliteRow) -> MetadataField {
    MetadataField {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        key: row.get("key"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::begin_immediate;
    use crate::test_support::{ensure_default_workspace, test_conn};

    #[test]
    fn metadata_keys_have_stable_normalization() {
        assert_eq!(normalize_metadata_key(" Max_Turns ").unwrap(), "max_turns");
        assert_eq!(normalize_metadata_key("source.id").unwrap(), "source.id");
        assert!(normalize_metadata_key("1source").is_err());
        assert!(normalize_metadata_key("source id").is_err());
        assert!(normalize_metadata_key("aven.internal").is_err());
    }

    #[test]
    fn metadata_result_limits_include_existing_value_bytes() {
        let mut existing = (0..7)
            .map(|index| (format!("key_{index}"), "x".repeat(MAX_METADATA_VALUE_BYTES)))
            .collect::<Vec<_>>();
        existing.push((
            "key_7".to_string(),
            "x".repeat(MAX_METADATA_VALUE_BYTES - 1),
        ));
        let set = [TaskMetadataInput {
            key: "key_8".to_string(),
            value: "é".to_string(),
        }];

        let error = validate_metadata_result_limits(existing, &set, &[]).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("error metadata-values-too-large limit={MAX_METADATA_TOTAL_BYTES}")
        );
    }

    #[test]
    fn metadata_result_limits_apply_removals_before_sets() {
        let existing = (0..8)
            .map(|index| (format!("key_{index}"), "x".repeat(MAX_METADATA_VALUE_BYTES)))
            .collect::<Vec<_>>();
        let set = [TaskMetadataInput {
            key: "replacement".to_string(),
            value: "y".repeat(MAX_METADATA_VALUE_BYTES),
        }];

        validate_metadata_result_limits(existing, &set, &[" KEY_0 ".to_string()]).unwrap();
    }

    #[test]
    fn metadata_result_limits_distinguish_replacement_from_insertion_at_count_limit() {
        let existing = (0..MAX_METADATA_VALUES)
            .map(|index| {
                (
                    format!("key_{index}"),
                    "x".repeat(MAX_METADATA_TOTAL_BYTES / MAX_METADATA_VALUES),
                )
            })
            .collect::<Vec<_>>();
        let replacement = [TaskMetadataInput {
            key: "KEY_0".to_string(),
            value: "replacement".to_string(),
        }];
        validate_metadata_result_limits(existing.clone(), &replacement, &[]).unwrap();

        let insertion = [TaskMetadataInput {
            key: "extra".to_string(),
            value: "x".to_string(),
        }];
        let error = validate_metadata_result_limits(existing, &insertion, &[]).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("error too-many-metadata-values limit={MAX_METADATA_VALUES}")
        );
    }

    #[tokio::test]
    async fn repeated_key_resolution_reuses_stable_field_identity() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = ensure_default_workspace(&mut conn).await.unwrap();
        let mut tx = begin_immediate(&mut conn).await.unwrap();
        let first = resolve_or_create_metadata_field(&mut tx, &workspace, "max_turns")
            .await
            .unwrap();
        let second = resolve_or_create_metadata_field(&mut tx, &workspace, "MAX_TURNS")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(first.id, second.id);
        let changes: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM changes WHERE op_type = 'create_metadata_field'",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(changes, 1);
    }
}
