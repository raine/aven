//! Immutable operation contracts and replica compatibility policy.
use anyhow::{Context, Result, bail};
use serde_json::Value;
use sqlx::SqliteConnection;

use super::wire::{ChangeWire, SYNC_PROTOCOL_VERSION};
use crate::db::{get_meta, set_meta};

/// Ordinary releases retain this baseline, including for local-only databases.
pub const MAINTAINED_PROTOCOL_BASELINE: u32 = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncCompatibilityError {
    pub server_protocol: u32,
    pub client_protocol: u32,
}

impl std::fmt::Display for SyncCompatibilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = if self.server_protocol > self.client_protocol {
            "Update Aven on this device to continue syncing."
        } else {
            "Update your Aven sync server to continue syncing."
        };
        write!(f, "{action} Your local tasks and edits remain saved.")
    }
}
impl std::error::Error for SyncCompatibilityError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedOperationCompatibilityError {
    pub required_protocol: u32,
    pub established_protocol: u32,
}

impl std::fmt::Display for SharedOperationCompatibilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This feature requires a newer Aven sync server. Existing tasks and edits will continue syncing normally.")
    }
}
impl std::error::Error for SharedOperationCompatibilityError {}

pub fn supports_protocol(protocol: u32) -> bool {
    (MAINTAINED_PROTOCOL_BASELINE..=SYNC_PROTOCOL_VERSION).contains(&protocol)
}

pub(crate) fn validate_behavior_protocol(protocol: u32) -> Result<()> {
    if supports_protocol(protocol) {
        return Ok(());
    }
    #[cfg(test)]
    if matches!(protocol, 19 | 20) {
        return Ok(());
    }
    Err(SyncCompatibilityError {
        server_protocol: protocol,
        client_protocol: SYNC_PROTOCOL_VERSION,
    }
    .into())
}

pub(crate) async fn replica_protocol(conn: &mut SqliteConnection) -> Result<u32> {
    let protocol = get_meta(conn, "sync_established_protocol")
        .await?
        .map(|value| {
            value
                .parse::<u32>()
                .context("invalid established sync protocol")
        })
        .transpose()?
        .unwrap_or(MAINTAINED_PROTOCOL_BASELINE);
    validate_behavior_protocol(protocol)?;
    Ok(protocol)
}

pub(crate) async fn establish_protocol(conn: &mut SqliteConnection, protocol: u32) -> Result<()> {
    validate_behavior_protocol(protocol)?;
    set_meta(conn, "sync_established_protocol", &protocol.to_string()).await
}

/// These literal names are the released baseline, not aliases to extensible domain enums.
const BASELINE_OPERATIONS: &[&str] = &[
    "create_task",
    "set_field",
    "resolve_field",
    "label_add",
    "label_remove",
    "note_add",
    "note_edit",
    "note_delete",
    "dependency_add",
    "dependency_remove",
    "related_add",
    "related_remove",
    "epic_link_add",
    "epic_link_remove",
    "attachment_add",
    "attachment_delete",
    "create_project",
    "set_project_metadata",
    "project_delete",
    "create_label",
    "set_label_name",
    "label_delete",
    "label_restore",
    "create_workspace",
    "set_workspace_field",
    "create_recurrence_series",
    "update_recurrence_template",
    "project_recurrence_occurrence",
    "resolve_recurrence_occurrence",
    "set_recurrence_state",
    "create_metadata_field",
    "set_metadata_field",
    "set_task_metadata",
    "remove_task_metadata",
    "set_recurrence_metadata",
    "remove_recurrence_metadata",
    "open_recurrence_pause",
    "close_recurrence_pause",
    "stop_recurrence_series",
];
const STATUSES: &[&str] = &["inbox", "backlog", "todo", "active", "done", "canceled"];
const PRIORITIES: &[&str] = &["none", "low", "medium", "high", "urgent"];
const SOURCES: &[&str] = &["cli", "tui", "api", "ios", "android", "unknown"];
const TASK_FIELDS: &[&str] = &[
    "title",
    "description",
    "project",
    "status",
    "priority",
    "available_at",
    "due_on",
    "deleted",
    "is_epic",
];

pub(crate) fn validate_operation(
    protocol: u32,
    op: &str,
    field: Option<&str>,
    payload: &Value,
) -> Result<()> {
    validate_behavior_protocol(protocol)?;
    let required = required_protocol(op)?;
    if required > protocol {
        return Err(SharedOperationCompatibilityError {
            required_protocol: required,
            established_protocol: protocol,
        }
        .into());
    }
    if op == "set_workspace_field" {
        registered(
            field.context("error invalid-sync-change field missing")?,
            &["key", "name"],
        )?;
    }
    if op == "set_metadata_field" {
        registered(
            field.context("error invalid-sync-change field missing")?,
            &["key"],
        )?;
    }
    if op == "attachment_add"
        && let Some(value) = payload.get("media_type").and_then(Value::as_str)
    {
        registered(
            value,
            &["image/png", "image/jpeg", "image/gif", "image/webp"],
        )?;
    }
    if matches!(op, "set_field" | "resolve_field") {
        let field = field.context("error invalid-sync-change field missing")?;
        registered(field, TASK_FIELDS)?;
        if let Some(value) = payload.get("value").and_then(Value::as_str) {
            validate_value(field, value)?;
        }
    }
    if matches!(
        op,
        "create_task"
            | "create_recurrence_series"
            | "project_recurrence_occurrence"
            | "resolve_recurrence_occurrence"
            | "set_recurrence_state"
            | "stop_recurrence_series"
            | "update_recurrence_template"
    ) {
        for key in [
            "status",
            "initial_status",
            "priority",
            "source",
            "frequency",
            "state",
            "outcome",
            "due_policy",
        ] {
            if let Some(value) = payload.get(key).and_then(Value::as_str) {
                validate_value(key, value)?;
            }
        }
        if let Some(fields) = payload.get("fields").and_then(Value::as_array) {
            for pair in fields {
                if let (Some(field), Some(value)) = (
                    pair.get(0).and_then(Value::as_str),
                    pair.get(1).and_then(Value::as_str),
                ) {
                    registered(
                        field,
                        &[
                            "title",
                            "description",
                            "project",
                            "priority",
                            "initial_status",
                            "available_local_time",
                            "due_policy",
                        ],
                    )?;
                    validate_value(field, value)?;
                }
            }
        }
    }
    Ok(())
}

fn required_protocol(op: &str) -> Result<u32> {
    #[cfg(test)]
    match op {
        "test_protocol_19" => return Ok(19),
        "test_protocol_20" => return Ok(20),
        _ => {}
    }
    if !BASELINE_OPERATIONS.contains(&op) {
        bail!("error invalid-sync-change op_type={op} unregistered-operation");
    }
    Ok(MAINTAINED_PROTOCOL_BASELINE)
}

fn registered(value: &str, values: &[&str]) -> Result<()> {
    if !values.contains(&value) {
        bail!("error invalid-sync-change unregistered-value");
    }
    Ok(())
}

fn validate_value(field: &str, value: &str) -> Result<()> {
    match field {
        "status" | "initial_status" => registered(value, STATUSES),
        "priority" => registered(value, PRIORITIES),
        "source" => {
            crate::choices::TaskSource::parse(value)?;
            registered(value, SOURCES)
        }
        "frequency" => registered(value, &["daily", "weekly", "monthly", "yearly"]),
        "state" => registered(value, &["active", "paused", "stopped"]),
        "outcome" => registered(value, &["completed", "skipped"]),
        "due_policy" => registered(value, &["none", "same_day"]),
        _ => Ok(()),
    }
}

pub(crate) fn validate_change(protocol: u32, change: &ChangeWire) -> Result<()> {
    validate_operation(
        protocol,
        &change.op_type,
        change.field.as_deref(),
        &change.payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_vocabulary_is_explicit_and_complete() {
        let source = include_str!("../change_log.rs");
        let operations = source
            .lines()
            .filter_map(|line| {
                let (_, value) = line
                    .trim()
                    .strip_prefix("pub const ")?
                    .split_once(": &str = \"")?;
                value.strip_suffix("\";")
            })
            .collect::<Vec<_>>();
        assert_eq!(operations, BASELINE_OPERATIONS);
        assert_eq!(crate::choices::STATUSES, STATUSES);
        assert_eq!(crate::choices::PRIORITIES, PRIORITIES);
        assert_eq!(crate::choices::TASK_SOURCES, SOURCES);
        let mut fields = crate::task_fields::TaskField::VERSIONED
            .map(|field| field.as_str())
            .to_vec();
        fields.sort();
        let mut frozen = TASK_FIELDS.to_vec();
        frozen.sort();
        assert_eq!(fields, frozen);
    }

    #[tokio::test]
    async fn unsupported_persisted_mode_fails_closed_on_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("replica.sqlite");
        let database = crate::db::Database::open(&path).await.unwrap();
        let mut conn = database.acquire_writer().await.unwrap();
        set_meta(&mut conn, "sync_established_protocol", "999")
            .await
            .unwrap();
        drop(conn);
        drop(database);
        assert!(crate::db::Database::open(&path).await.is_err());
    }
}
