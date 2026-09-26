use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use super::super::wire::ChangeWire;
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
