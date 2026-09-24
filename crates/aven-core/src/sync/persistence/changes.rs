#[cfg(any(test, feature = "test-support"))]
use std::collections::HashSet;

#[cfg(any(test, feature = "test-support"))]
use anyhow::bail;
use anyhow::{Context, Result};
use sqlx::SqliteConnection;
#[cfg(any(test, feature = "test-support"))]
use sqlx::{QueryBuilder, Sqlite};

use super::super::wire::ChangeWire;
#[cfg(any(test, feature = "test-support"))]
use super::super::wire::PushAck;
use crate::change_log::op_type;

pub(in crate::sync) fn is_epic_change(change: &ChangeWire) -> bool {
    matches!(
        change.op_type.as_str(),
        op_type::EPIC_LINK_ADD | op_type::EPIC_LINK_REMOVE
    )
}

pub(in crate::sync) fn epic_change_workspace(change: &ChangeWire) -> Result<&str> {
    change.payload["workspace_id"]
        .as_str()
        .context("epic change missing workspace_id")
}

pub(in crate::sync) async fn reconcile_epic_change(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    if is_epic_change(change) {
        crate::epic_membership::reconcile_child(
            conn,
            epic_change_workspace(change)?,
            &change.entity_id,
        )
        .await?;
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub(in crate::sync) async fn reconcile_acknowledged_epic_memberships(
    conn: &mut SqliteConnection,
    acknowledgements: &[PushAck],
) -> Result<()> {
    if acknowledgements.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT DISTINCT json_extract(payload, '$.workspace_id'), entity_id FROM changes
         WHERE op_type IN ('epic_link_add', 'epic_link_remove') AND change_id IN (",
    );
    let mut ids = query.separated(", ");
    for acknowledgement in acknowledgements {
        ids.push_bind(&acknowledgement.change_id);
    }
    ids.push_unseparated(")");
    let children = query
        .build_query_as::<(String, String)>()
        .fetch_all(&mut *conn)
        .await?;
    for (workspace_id, child_id) in children {
        crate::epic_membership::reconcile_child(conn, &workspace_id, &child_id).await?;
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub(in crate::sync) async fn load_existing_change_ids(
    conn: &mut SqliteConnection,
    changes: &[ChangeWire],
) -> Result<HashSet<String>> {
    if changes.is_empty() {
        return Ok(HashSet::new());
    }
    let mut query_builder =
        QueryBuilder::<Sqlite>::new("SELECT change_id FROM changes WHERE change_id IN (");
    let mut separated = query_builder.separated(", ");
    for change in changes {
        separated.push_bind(&change.change_id);
    }
    separated.push_unseparated(")");
    Ok(query_builder
        .build_query_scalar::<String>()
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect())
}

#[cfg(any(test, feature = "test-support"))]
pub(in crate::sync) async fn verify_existing_change(
    conn: &mut SqliteConnection,
    incoming: &ChangeWire,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT entity_type, entity_id, field, op_type, payload, base_version, created_at
         FROM changes WHERE change_id = ?",
    )
    .bind(&incoming.change_id)
    .fetch_one(&mut *conn)
    .await?;
    use sqlx::Row;
    let stored_payload: String = row.try_get("payload")?;
    let stored_payload: serde_json::Value = serde_json::from_str(&stored_payload)?;
    let stored = ChangeWire {
        change_id: incoming.change_id.clone(),
        client_id: String::new(),
        local_seq: 0,
        entity_type: row.try_get("entity_type")?,
        entity_id: row.try_get("entity_id")?,
        field: row.try_get("field")?,
        op_type: row.try_get("op_type")?,
        payload: stored_payload,
        base_version: row.try_get("base_version")?,
        created_at: row.try_get("created_at")?,
        server_seq: None,
    };
    let equal = canonical_equal(&stored, incoming);
    if !equal {
        bail!(
            "error sync-change-id-payload-mismatch change_id={}",
            incoming.change_id
        );
    }
    Ok(())
}

pub(in crate::sync) async fn update_change_server_seq(
    conn: &mut SqliteConnection,
    change_id: &str,
    server_seq: Option<i64>,
) -> Result<()> {
    if let Some(server_seq) = server_seq {
        sqlx::query!(
            "UPDATE changes SET server_seq = ? WHERE change_id = ? AND server_seq IS NULL",
            server_seq,
            change_id,
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub(in crate::sync) async fn update_change_server_seqs_if_missing(
    conn: &mut SqliteConnection,
    push_acks: &[PushAck],
) -> Result<()> {
    if push_acks.is_empty() {
        return Ok(());
    }
    let mut query_builder = QueryBuilder::<Sqlite>::new("WITH updates(change_id, server_seq) AS (");
    query_builder.push_values(push_acks, |mut row, ack| {
        row.push_bind(&ack.change_id).push_bind(ack.server_seq);
    });
    query_builder.push(
        ") UPDATE changes
         SET server_seq = (
             SELECT updates.server_seq
             FROM updates
             WHERE updates.change_id = changes.change_id
         )
         WHERE server_seq IS NULL
           AND change_id IN (SELECT change_id FROM updates)",
    );
    query_builder.build().execute(&mut *conn).await?;
    Ok(())
}

pub(in crate::sync) async fn insert_wire_change(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let payload = change.payload.to_string();
    sqlx::query!(
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at, server_seq)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        change.change_id,
        change.client_id,
        change.local_seq,
        change.entity_type,
        change.entity_id,
        change.field,
        change.op_type,
        payload,
        change.base_version,
        change.created_at,
        change.server_seq,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Canonical domain meaning excludes transport rank and originating provenance.
pub(crate) fn canonical_equal(a: &ChangeWire, b: &ChangeWire) -> bool {
    a.change_id == b.change_id
        && a.entity_type == b.entity_type
        && a.entity_id == b.entity_id
        && a.field == b.field
        && a.op_type == b.op_type
        && a.payload == b.payload
        && a.base_version == b.base_version
        && a.created_at == b.created_at
}
