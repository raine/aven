use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail, ensure};
use sqlx::{Connection as _, Row, SqliteConnection};

use crate::ids::{new_id, now};
use crate::mutation::set_task_field;
use crate::operations::update_task_labels_in_workspace;
use crate::projects::project_has_config_mapping;
use crate::task_fields::TaskField;

static APPLYING_UNDO: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_applying_undo() -> bool {
    APPLYING_UNDO.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UndoPayload {
    pub(crate) commands: Vec<UndoCommand>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum UndoCommand {
    SetTaskField {
        task_id: String,
        field: String,
        before: String,
        after: String,
    },
    SetTaskLabels {
        task_id: String,
        before: Vec<String>,
        after: Vec<String>,
    },
    DeleteCreatedTask {
        task_id: String,
        create_change_id: Option<String>,
        expected: TaskUndoSnapshot,
    },
    DeleteCreatedNote {
        task_id: String,
        note_id: String,
        note_add_change_id: String,
    },
    DeleteCreatedProject {
        project_key: String,
        create_change_id: String,
        expected_name: String,
        expected_prefix: String,
    },
    DeleteCreatedLabel {
        label: String,
        create_change_id: String,
    },
    RestoreConflictResolution {
        task_id: String,
        field: String,
        before: String,
        after: String,
        conflict_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskUndoSnapshot {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project_key: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) deleted: bool,
    pub(crate) labels: Vec<String>,
}

pub(crate) struct UndoOutcome {
    pub(crate) summary: String,
    pub(crate) task_id: Option<String>,
    pub(crate) include_deleted: Option<bool>,
}

pub(crate) async fn task_field_value(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    field: &str,
) -> Result<String> {
    let task_field = TaskField::parse(field)
        .ok_or_else(|| anyhow::anyhow!("error unknown-field field={field}"))?;

    let row = sqlx::query(
        "SELECT title, description, project_key, status, priority, deleted
         FROM tasks WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("error task-not-found task_id={task_id}"))?;

    Ok(match task_field {
        TaskField::Title => row.get("title"),
        TaskField::Description => row.get("description"),
        TaskField::Project => row.get("project_key"),
        TaskField::Status => row.get("status"),
        TaskField::Priority => row.get("priority"),
        TaskField::Deleted => {
            if row.get::<i64, _>("deleted") != 0 {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
    })
}

pub(crate) async fn task_labels(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = ? ORDER BY label",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|row| row.get("label")).collect())
}

pub(crate) async fn task_snapshot(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<TaskUndoSnapshot> {
    let row = sqlx::query(
        "SELECT title, description, project_key, status, priority, deleted
         FROM tasks WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("error task-not-found task_id={task_id}"))?;
    let labels = task_labels(conn, workspace_id, task_id).await?;
    Ok(TaskUndoSnapshot {
        title: row.get("title"),
        description: row.get("description"),
        project_key: row.get("project_key"),
        status: row.get("status"),
        priority: row.get("priority"),
        deleted: row.get::<i64, _>("deleted") != 0,
        labels,
    })
}

pub(crate) async fn conflict_row_id(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    field: &str,
) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT id FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0
         ORDER BY id LIMIT 1",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(field)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("error conflict-not-found task_id={task_id} field={field}"))
}

pub(crate) async fn record_tui_undo(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    summary: &str,
    payload: UndoPayload,
) -> Result<()> {
    if is_applying_undo() || !undo_payload_has_effect(&payload) {
        return Ok(());
    }
    let id = new_id();
    let created_at = now();
    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM tui_undo_entries WHERE workspace_id = ?",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    let payload = serde_json::to_string(&payload)?;
    sqlx::query(
        "INSERT INTO tui_undo_entries(id, workspace_id, summary, payload_version, payload, seq, created_at)
         VALUES (?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(&id)
    .bind(workspace_id)
    .bind(summary)
    .bind(&payload)
    .bind(seq)
    .bind(&created_at)
    .execute(&mut *conn)
    .await?;
    prune_consumed_undo_entries(conn, workspace_id).await?;
    Ok(())
}

fn undo_payload_has_effect(payload: &UndoPayload) -> bool {
    payload.commands.iter().any(|command| match command {
        UndoCommand::SetTaskField { before, after, .. } => before != after,
        UndoCommand::SetTaskLabels { before, after, .. } => !label_sets_equal(before, after),
        UndoCommand::DeleteCreatedTask { .. }
        | UndoCommand::DeleteCreatedNote { .. }
        | UndoCommand::DeleteCreatedProject { .. }
        | UndoCommand::DeleteCreatedLabel { .. }
        | UndoCommand::RestoreConflictResolution { .. } => true,
    })
}

async fn prune_consumed_undo_entries(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM tui_undo_entries
         WHERE workspace_id = ? AND undone_at IS NOT NULL AND id NOT IN (
             SELECT id FROM tui_undo_entries
             WHERE workspace_id = ? AND undone_at IS NOT NULL
             ORDER BY undone_at DESC, seq DESC
             LIMIT 20
         )",
    )
    .bind(workspace_id)
    .bind(workspace_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn apply_latest_tui_undo(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Option<UndoOutcome>> {
    let mut tx = conn.begin().await?;
    let row = sqlx::query(
        "SELECT id, summary, payload FROM tui_undo_entries
         WHERE workspace_id = ? AND undone_at IS NULL
         ORDER BY seq DESC
         LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let entry_id: String = row.get("id");
    let summary: String = row.get("summary");
    let payload_text: String = row.get("payload");
    let undone_at = now();
    let claimed =
        sqlx::query("UPDATE tui_undo_entries SET undone_at = ? WHERE id = ? AND undone_at IS NULL")
            .bind(&undone_at)
            .bind(&entry_id)
            .execute(&mut *tx)
            .await?;
    ensure!(
        claimed.rows_affected() == 1,
        "error undo-entry-claim-failed id={entry_id}"
    );
    let payload: UndoPayload = serde_json::from_str(&payload_text)?;
    APPLYING_UNDO.store(true, Ordering::SeqCst);
    let apply_result = apply_undo_commands(&mut tx, workspace_id, &payload.commands).await;
    APPLYING_UNDO.store(false, Ordering::SeqCst);
    match apply_result {
        Ok(outcome) => {
            tx.commit().await?;
            Ok(Some(UndoOutcome {
                summary,
                task_id: outcome.task_id,
                include_deleted: outcome.include_deleted,
            }))
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

struct CommandOutcome {
    task_id: Option<String>,
    include_deleted: Option<bool>,
}

async fn apply_undo_commands(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    commands: &[UndoCommand],
) -> Result<CommandOutcome> {
    let mut task_id = None;
    let mut include_deleted = None;
    for command in commands {
        let outcome = apply_undo_command(conn, workspace_id, command).await?;
        if outcome.task_id.is_some() {
            task_id = outcome.task_id;
        }
        if outcome.include_deleted.is_some() {
            include_deleted = outcome.include_deleted;
        }
    }
    Ok(CommandOutcome {
        task_id,
        include_deleted,
    })
}

async fn apply_undo_command(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    command: &UndoCommand,
) -> Result<CommandOutcome> {
    match command {
        UndoCommand::SetTaskField {
            task_id,
            field,
            before,
            after,
        } => {
            let current = task_field_value(conn, workspace_id, task_id, field).await?;
            if current != *after {
                bail!("error undo-state-changed task_id={task_id} field={field}");
            }
            if before != after {
                if field == "project" && !project_exists(conn, workspace_id, before).await? {
                    bail!("error undo-state-changed task_id={task_id} field={field}");
                }
                set_task_field(conn, task_id, field, before).await?;
            }
            let include_deleted = if field == "deleted" {
                Some(before == "1")
            } else {
                None
            };
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted,
            })
        }
        UndoCommand::SetTaskLabels {
            task_id,
            before,
            after,
        } => {
            let current = task_labels(conn, workspace_id, task_id).await?;
            if !label_sets_equal(&current, after) {
                bail!("error undo-state-changed task_id={task_id} field=labels");
            }
            let (add_labels, remove_labels) = label_delta(&current, before);
            update_task_labels_in_workspace(
                conn,
                workspace_id,
                task_id,
                &add_labels,
                &remove_labels,
            )
            .await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
            })
        }
        UndoCommand::DeleteCreatedTask {
            task_id,
            create_change_id,
            expected,
        } => {
            let current = task_snapshot(conn, workspace_id, task_id).await?;
            if current != *expected {
                bail!("error undo-state-changed task_id={task_id} field=task");
            }
            if let Some(change_id) = create_change_id {
                let labels_clear = expected.labels.is_empty()
                    || labels_match_create_change(conn, change_id, &expected.labels).await?;
                if change_is_unsynced(conn, change_id).await? && labels_clear {
                    hard_delete_created_task(conn, workspace_id, task_id, change_id).await?;
                    return Ok(CommandOutcome {
                        task_id: Some(task_id.clone()),
                        include_deleted: None,
                    });
                }
            }
            set_task_field(conn, task_id, "deleted", "1").await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
            })
        }
        UndoCommand::DeleteCreatedNote {
            task_id,
            note_id,
            note_add_change_id,
        } => {
            delete_created_note(conn, workspace_id, task_id, note_id, note_add_change_id).await?;
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
            })
        }
        UndoCommand::DeleteCreatedProject {
            project_key,
            create_change_id,
            expected_name,
            expected_prefix,
        } => {
            delete_created_project(
                conn,
                workspace_id,
                project_key,
                create_change_id,
                expected_name,
                expected_prefix,
            )
            .await?;
            Ok(CommandOutcome {
                task_id: None,
                include_deleted: None,
            })
        }
        UndoCommand::DeleteCreatedLabel {
            label,
            create_change_id,
        } => {
            delete_created_label(conn, workspace_id, label, create_change_id).await?;
            Ok(CommandOutcome {
                task_id: None,
                include_deleted: None,
            })
        }
        UndoCommand::RestoreConflictResolution {
            task_id,
            field,
            before,
            after,
            conflict_id,
        } => {
            let current = task_field_value(conn, workspace_id, task_id, field).await?;
            if current != *after {
                bail!("error undo-state-changed task_id={task_id} field={field}");
            }
            set_task_field(conn, task_id, field, before).await?;
            let restored = sqlx::query(
                "UPDATE conflicts SET resolved = 0 WHERE id = ? AND workspace_id = ? AND resolved = 1",
            )
            .bind(conflict_id)
            .bind(workspace_id)
            .execute(&mut *conn)
            .await?;
            ensure!(
                restored.rows_affected() == 1,
                "error undo-state-changed task_id={task_id} field={field}"
            );
            Ok(CommandOutcome {
                task_id: Some(task_id.clone()),
                include_deleted: None,
            })
        }
    }
}

async fn project_exists(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_key: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects
         WHERE workspace_id = ? AND key = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_key)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

fn label_sets_equal(left: &[String], right: &[String]) -> bool {
    let left: BTreeSet<_> = left.iter().collect();
    let right: BTreeSet<_> = right.iter().collect();
    left == right
}

fn label_delta(current: &[String], target: &[String]) -> (Vec<String>, Vec<String>) {
    let current_set: BTreeSet<_> = current.iter().collect();
    let target_set: BTreeSet<_> = target.iter().collect();
    let add = target
        .iter()
        .filter(|label| !current_set.contains(label))
        .cloned()
        .collect();
    let remove = current
        .iter()
        .filter(|label| !target_set.contains(label))
        .cloned()
        .collect();
    (add, remove)
}

async fn change_is_unsynced(conn: &mut SqliteConnection, change_id: &str) -> Result<bool> {
    let server_seq =
        sqlx::query_scalar::<_, Option<i64>>("SELECT server_seq FROM changes WHERE change_id = ?")
            .bind(change_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(matches!(server_seq, Some(None)))
}

async fn labels_match_create_change(
    conn: &mut SqliteConnection,
    change_id: &str,
    labels: &[String],
) -> Result<bool> {
    let payload: String = sqlx::query_scalar("SELECT payload FROM changes WHERE change_id = ?")
        .bind(change_id)
        .fetch_one(&mut *conn)
        .await?;
    let payload: serde_json::Value = serde_json::from_str(&payload)?;
    let payload_labels = payload
        .get("labels")
        .and_then(|labels| labels.as_array())
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(label_sets_equal(labels, &payload_labels))
}

async fn hard_delete_created_task(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    create_change_id: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM field_versions WHERE entity_id = ?")
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM tasks WHERE workspace_id = ? AND id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(create_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn delete_created_note(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    note_id: &str,
    note_add_change_id: &str,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT change_id FROM notes WHERE workspace_id = ? AND id = ? AND task_id = ?",
    )
    .bind(workspace_id)
    .bind(note_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        bail!("error undo-state-changed task_id={task_id} field=note");
    };
    let stored_change_id: String = row.get("change_id");
    if stored_change_id != note_add_change_id {
        bail!("error undo-state-changed task_id={task_id} field=note");
    }
    if !change_is_unsynced(conn, note_add_change_id).await? {
        bail!("error undo-state-changed task_id={task_id} field=note");
    }
    sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND id = ? AND task_id = ?")
        .bind(workspace_id)
        .bind(note_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(note_add_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn delete_created_project(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_key: &str,
    create_change_id: &str,
    expected_name: &str,
    expected_prefix: &str,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT name, prefix FROM projects WHERE workspace_id = ? AND key = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_key)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        bail!("error undo-state-changed project_key={project_key}");
    };
    let name: String = row.get("name");
    let prefix: String = row.get("prefix");
    if name != expected_name || prefix != expected_prefix {
        bail!("error undo-state-changed project_key={project_key}");
    }
    if !change_is_unsynced(conn, create_change_id).await? {
        bail!("error undo-state-changed project_key={project_key}");
    }
    let task_refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND project_key = ?")
            .bind(workspace_id)
            .bind(project_key)
            .fetch_one(&mut *conn)
            .await?;
    if task_refs > 0 {
        bail!("error undo-state-changed project_key={project_key}");
    }
    let path_refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_paths WHERE workspace_id = ? AND project_key = ?",
    )
    .bind(workspace_id)
    .bind(project_key)
    .fetch_one(&mut *conn)
    .await?;
    if path_refs > 0 || project_has_config_mapping(workspace_id, project_key)? {
        bail!("error undo-state-changed project_key={project_key}");
    }
    sqlx::query("DELETE FROM projects WHERE workspace_id = ? AND key = ?")
        .bind(workspace_id)
        .bind(project_key)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(create_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn delete_created_label(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    label: &str,
    create_change_id: &str,
) -> Result<()> {
    let exists: i64 =
        sqlx::query_scalar("SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?")
            .bind(workspace_id)
            .bind(label)
            .fetch_one(&mut *conn)
            .await?;
    if exists == 0 || !change_is_unsynced(conn, create_change_id).await? {
        bail!("error undo-state-changed label={label}");
    }
    let refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE workspace_id = ? AND label = ?")
            .bind(workspace_id)
            .bind(label)
            .fetch_one(&mut *conn)
            .await?;
    if refs > 0 {
        bail!("error undo-state-changed label={label}");
    }
    sqlx::query("DELETE FROM labels WHERE workspace_id = ? AND name = ?")
        .bind(workspace_id)
        .bind(label)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ?")
        .bind(create_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
