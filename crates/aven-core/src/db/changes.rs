use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

use crate::error::CoreError;
use crate::ids::{new_id, now};

use super::{get_meta, set_meta};

async fn next_local_seq(conn: &mut SqliteConnection) -> Result<i64> {
    let seq = get_meta(conn, "local_seq")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?
        + 1;
    set_meta(conn, "local_seq", &seq.to_string()).await?;
    Ok(seq)
}

async fn serialize_local_change(
    conn: &mut SqliteConnection,
    op_type: &str,
    field: Option<&str>,
    payload: &Value,
) -> Result<String> {
    let protocol = crate::sync::protocol::replica_protocol(conn).await?;
    crate::sync::protocol::validate_operation(protocol, op_type, field, payload)?;
    crate::sync::wire::serialize_change_payload(payload)
}

pub(crate) async fn insert_change(
    conn: &mut SqliteConnection,
    entity_type: &str,
    entity_id: &str,
    field: Option<&str>,
    op_type: &str,
    payload: Value,
    base_version: Option<&str>,
) -> Result<String> {
    let payload = serialize_local_change(conn, op_type, field, &payload).await?;
    let change_id = new_id();
    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let local_seq = next_local_seq(conn).await?;
    let created_at = now();
    sqlx::query!(
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        change_id,
        client_id,
        local_seq,
        entity_type,
        entity_id,
        field,
        op_type,
        payload,
        base_version,
        created_at,
    )
    .execute(&mut *conn)
    .await?;
    Ok(change_id)
}

pub(crate) struct IdentifiedChange<'a> {
    pub change_id: &'a str,
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    pub field: Option<&'a str>,
    pub op_type: &'a str,
    pub payload: Value,
    pub base_version: Option<&'a str>,
    pub created_at: &'a str,
}

pub(crate) async fn insert_change_with_identity(
    conn: &mut SqliteConnection,
    change: IdentifiedChange<'_>,
) -> Result<()> {
    let IdentifiedChange {
        change_id,
        entity_type,
        entity_id,
        field,
        op_type,
        payload,
        base_version,
        created_at,
    } = change;
    let payload = serialize_local_change(conn, op_type, field, &payload).await?;
    let existing = sqlx::query(
        "SELECT entity_type, entity_id, field, op_type, payload, base_version, created_at
         FROM changes WHERE change_id = ?",
    )
    .bind(change_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(existing) = existing {
        let stored = crate::sync::wire::ChangeWire {
            change_id: change_id.into(),
            client_id: String::new(),
            local_seq: 0,
            entity_type: existing.try_get("entity_type")?,
            entity_id: existing.try_get("entity_id")?,
            field: existing.try_get("field")?,
            op_type: existing.try_get("op_type")?,
            payload: serde_json::from_str(&existing.try_get::<String, _>("payload")?)?,
            base_version: existing.try_get("base_version")?,
            created_at: existing.try_get("created_at")?,
            server_seq: None,
        };
        let incoming = crate::sync::wire::ChangeWire {
            change_id: change_id.into(),
            client_id: String::new(),
            local_seq: 0,
            entity_type: entity_type.into(),
            entity_id: entity_id.into(),
            field: field.map(str::to_owned),
            op_type: op_type.into(),
            payload: serde_json::from_str(&payload)?,
            base_version: base_version.map(str::to_owned),
            created_at: created_at.into(),
            server_seq: None,
        };
        let equal = crate::sync::canonical_equal(&stored, &incoming);
        if equal {
            return Ok(());
        }
        return Err(CoreError::generation_conflict(format!(
            "error recurrence-generation-conflict change_id={change_id}"
        ))
        .into());
    }

    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let local_seq = next_local_seq(conn).await?;
    sqlx::query(
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(change_id)
    .bind(client_id)
    .bind(local_seq)
    .bind(entity_type)
    .bind(entity_id)
    .bind(field)
    .bind(op_type)
    .bind(payload)
    .bind(base_version)
    .bind(created_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn identified_retry_uses_typed_payload_equality() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let change = || IdentifiedChange {
            change_id: "AAAAAAAAAAAAAAAA",
            entity_type: "task",
            entity_id: "BBBBBBBBBBBBBBBB",
            field: Some("title"),
            op_type: "set_field",
            base_version: None,
            created_at: "2026-09-22T00:00:00Z",
            payload: serde_json::json!({"workspace_id":"0000000000000000","workspace_key":"default","value":"x"}),
        };
        insert_change_with_identity(&mut conn, change())
            .await
            .unwrap();
        sqlx::query("UPDATE changes SET payload=? WHERE change_id='AAAAAAAAAAAAAAAA'")
            .bind(r#"{ "workspace_key": "default", "workspace_id": "0000000000000000", "value": "x" }"#)
            .execute(&mut *conn).await.unwrap();
        insert_change_with_identity(&mut conn, change())
            .await
            .unwrap();
        let mut different = change();
        different.payload["value"] = Value::String("y".into());
        assert!(
            insert_change_with_identity(&mut conn, different)
                .await
                .is_err()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM changes")
                .fetch_one(&mut *conn)
                .await
                .unwrap(),
            1
        );
    }
}
