use anyhow::{Context, Result, bail};
use sqlx::{Row, SqliteConnection};
use tracing::info;

use crate::change_log::op_type;
use crate::db::{Database, begin_immediate, insert_change, set_field_version};
use crate::ids::{TaskId, now};
use crate::mutation::{apply_field_value_in_workspace, apply_project_id_in_workspace};
use crate::projects::{resolve_existing_project_in_workspace, resolve_project_for_stored_value};
use crate::recurrence::{RecurrenceOutcome, RecurrenceSeriesId, RecurrenceSeriesState};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::undo::{UndoCommand, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

impl Database {
    pub async fn list_conflicts(
        &self,
        workspace: &Workspace,
        project_key: Option<&str>,
        field: Option<&str>,
    ) -> Result<Vec<ConflictListItem>> {
        let mut conn = self.acquire().await?;
        list_conflicts(&mut conn, workspace, project_key, field).await
    }

    pub async fn task_conflicts(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: Option<&str>,
    ) -> Result<Vec<ConflictDetail>> {
        let mut conn = self.acquire().await?;
        task_conflicts(&mut conn, workspace, task_id, field).await
    }

    pub async fn conflict_variant_value(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: &str,
        token: &str,
    ) -> Result<String> {
        let mut conn = self.acquire().await?;
        conflict_variant_value(&mut conn, workspace, task_id, field, token).await
    }

    pub async fn resolve_conflict_with_tui_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: &str,
        value: &str,
        summary: &str,
    ) -> Result<ConflictResolutionOutcome> {
        let mut conn = self.acquire().await?;
        resolve_conflict_value(
            &mut conn,
            workspace,
            task_id,
            field,
            ResolutionValue::Explicit(value),
            Some(summary),
        )
        .await
    }

    pub async fn resolve_conflict(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        field: &str,
        value: &str,
    ) -> Result<ConflictOutcome> {
        let mut conn = self.acquire().await?;
        resolve_conflict(&mut conn, workspace, task_id, field, value).await
    }

    pub async fn recurrence_series_conflicts(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        field: Option<&str>,
    ) -> Result<Vec<ConflictDetail>> {
        let mut conn = self.acquire().await?;
        recurrence_series_conflicts(&mut conn, workspace, series_id, field).await
    }

    pub async fn recurrence_conflict_variant_value(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        field: &str,
        token: &str,
    ) -> Result<String> {
        let mut conn = self.acquire().await?;
        for detail in
            recurrence_series_conflicts(&mut conn, workspace, series_id, Some(field)).await?
        {
            if token == detail.variant_a {
                return Ok(detail.local_value);
            }
            if token == detail.variant_b {
                return Ok(detail.remote_value);
            }
        }
        bail!("error unknown-variant token={token}")
    }

    pub async fn resolve_recurrence_conflict(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        field: &str,
        value: &str,
    ) -> Result<String> {
        let mut conn = self.acquire().await?;
        resolve_recurrence_conflict(&mut conn, workspace, series_id, field, value).await
    }
}

pub struct ConflictListItem {
    pub task_id: TaskId,
    pub recurrence_series: bool,
    pub title: String,
    pub project_key: String,
    pub project_prefix: String,
    pub field: String,
    pub variant_a: String,
    pub variant_b: String,
}

pub struct ConflictDetail {
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

pub struct ConflictOutcome {
    pub task: Task,
    pub field: String,
}

pub struct ConflictResolutionOutcome {
    pub outcome: ConflictOutcome,
    pub before: String,
    pub after: String,
    pub conflict_id: i64,
}
pub async fn list_conflicts(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project_key: Option<&str>,
    field: Option<&str>,
) -> Result<Vec<ConflictListItem>> {
    let workspace_id = &workspace.id;
    let project_id = if let Some(project) = project_key {
        Some(
            resolve_existing_project_in_workspace(conn, workspace_id, project)
                .await?
                .id,
        )
    } else {
        None
    };
    let rows = sqlx::query(
        r#"SELECT c.entity_id AS task_id, c.entity_type, c.field, c.variant_a, c.variant_b,
                 t.title, p.prefix, p.key AS project_key
                 FROM conflicts c
                 JOIN tasks t ON c.entity_type = 'task' AND t.workspace_id = c.workspace_id AND t.id = c.entity_id
                 JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
                 WHERE c.workspace_id = ? AND c.resolved = 0
                 AND (? IS NULL OR t.project_id = ?)
                 AND (? IS NULL OR c.field = ?)
                 UNION ALL
                 SELECT c.entity_id AS task_id, c.entity_type, c.field, c.variant_a, c.variant_b,
                 s.title, p.prefix, p.key AS project_key
                 FROM conflicts c
                 JOIN recurrence_series s ON c.entity_type = 'recurrence_series'
                    AND s.workspace_id = c.workspace_id AND s.id = c.entity_id
                 JOIN projects p ON p.workspace_id = s.workspace_id AND p.id = s.project_id
                 WHERE c.workspace_id = ? AND c.resolved = 0
                 AND (? IS NULL OR s.project_id = ?)
                 AND (? IS NULL OR c.field = ?)
                 ORDER BY field"#,
    )
    .bind(workspace_id)
    .bind(&project_id)
    .bind(&project_id)
    .bind(field)
    .bind(field)
    .bind(workspace_id)
    .bind(&project_id)
    .bind(&project_id)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictListItem {
            task_id: row.get("task_id"),
            recurrence_series: row.get::<String, _>("entity_type") == "recurrence_series",
            title: row.get("title"),
            project_key: row.get("project_key"),
            project_prefix: row.get("prefix"),
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            variant_b: row.get("variant_b"),
        })
        .collect())
}

pub async fn task_conflicts(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: Option<&str>,
) -> Result<Vec<ConflictDetail>> {
    let workspace_id = &workspace.id;
    let rows = sqlx::query(
        r#"SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id"#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictDetail {
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            local_value: row.get("local_value"),
            variant_b: row.get("variant_b"),
            remote_value: row.get("remote_value"),
        })
        .collect())
}

pub async fn recurrence_series_conflicts(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    field: Option<&str>,
) -> Result<Vec<ConflictDetail>> {
    let rows = sqlx::query(
        "SELECT field, variant_a, local_value, variant_b, remote_value FROM conflicts
         WHERE workspace_id = ? AND entity_type = 'recurrence_series' AND entity_id = ?
           AND resolved = 0 AND (? IS NULL OR field = ?) ORDER BY field, id",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictDetail {
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            local_value: row.get("local_value"),
            variant_b: row.get("variant_b"),
            remote_value: row.get("remote_value"),
        })
        .collect())
}

async fn resolve_recurrence_conflict(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    field: &str,
    value: &str,
) -> Result<String> {
    let mut tx = begin_immediate(conn).await?;
    let conflict = sqlx::query(
        "SELECT id, local_value, remote_value, remote_change_id FROM conflicts
         WHERE workspace_id = ? AND entity_type = 'recurrence_series'
           AND entity_id = ? AND field = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .bind(field)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        anyhow::anyhow!("error conflict-not-found series_id={series_id} field={field}")
    })?;
    let local: String = conflict.get("local_value");
    let remote: String = conflict.get("remote_value");
    if value != local && value != remote {
        bail!("error invalid-conflict-value value={value}");
    }
    if let Some(slot) = field.strip_prefix("outcome:") {
        resolve_recurrence_outcome_conflict(
            &mut tx,
            workspace,
            series_id,
            slot.parse()?,
            RecurrenceOutcome::parse(value)?,
        )
        .await?;
    } else if field == "state" {
        resolve_recurrence_state_conflict(
            &mut tx,
            workspace,
            series_id,
            RecurrenceSeriesState::parse(value)?,
            &conflict.get::<String, _>("remote_change_id"),
        )
        .await?;
    } else if matches!(
        field,
        "title"
            | "description"
            | "project"
            | "priority"
            | "initial_status"
            | "available_local_time"
            | "due_policy"
            | "labels"
    ) {
        resolve_recurrence_template_conflict(&mut tx, workspace, series_id, field, value).await?;
    }
    sqlx::query("UPDATE conflicts SET resolved = 1 WHERE id = ?")
        .bind(conflict.get::<i64, _>("id"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(field.to_string())
}

async fn resolve_recurrence_template_conflict(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    field: &str,
    value: &str,
) -> Result<()> {
    let updated_at = now();
    let (fields, labels, labels_changed) = if field == "labels" {
        let labels: Vec<String> = serde_json::from_str(value)?;
        sqlx::query(
            "DELETE FROM recurrence_series_labels WHERE workspace_id = ? AND series_id = ?",
        )
        .bind(&workspace.id)
        .bind(series_id)
        .execute(&mut *conn)
        .await?;
        for label in &labels {
            sqlx::query(
                "INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)",
            )
            .bind(&workspace.id)
            .bind(label)
            .bind(&updated_at)
            .execute(&mut *conn)
            .await?;
            sqlx::query("INSERT INTO recurrence_series_labels(workspace_id, series_id, label) VALUES (?, ?, ?)")
                .bind(&workspace.id).bind(series_id).bind(label).execute(&mut *conn).await?;
        }
        (Vec::<(String, String)>::new(), labels, true)
    } else {
        let query = match field {
            "title" => {
                "UPDATE recurrence_series SET title = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
            }
            "description" => {
                "UPDATE recurrence_series SET description = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
            }
            "project" => {
                "UPDATE recurrence_series SET project_id = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
            }
            "priority" => {
                "UPDATE recurrence_series SET priority = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
            }
            "initial_status" => {
                "UPDATE recurrence_series SET initial_status = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
            }
            "available_local_time" => {
                "UPDATE recurrence_series SET available_local_time = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
            }
            "due_policy" => {
                "UPDATE recurrence_series SET due_policy = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
            }
            _ => unreachable!(),
        };
        sqlx::query(query)
            .bind(value)
            .bind(&updated_at)
            .bind(&workspace.id)
            .bind(series_id)
            .execute(&mut *conn)
            .await?;
        let labels = sqlx::query_scalar("SELECT label FROM recurrence_series_labels WHERE workspace_id = ? AND series_id = ? ORDER BY label")
            .bind(&workspace.id).bind(series_id).fetch_all(&mut *conn).await?;
        (vec![(field.to_string(), value.to_string())], labels, false)
    };
    let mut base_versions = serde_json::Map::new();
    base_versions.insert(field.to_string(), serde_json::Value::Null);
    let change_id = insert_change(
        conn,
        "recurrence_series",
        series_id.as_str(),
        None,
        op_type::UPDATE_RECURRENCE_TEMPLATE,
        crate::change_log::ChangePayload::workspace(workspace)
            .set("fields", fields)
            .set("base_versions", base_versions)
            .set("labels_changed", labels_changed)
            .set("labels", labels)
            .set("updated_at", &updated_at)
            .set("conflict_resolution", true)
            .into_value(),
        None,
    )
    .await?;
    crate::db::set_entity_field_version(
        conn,
        &workspace.id,
        crate::types::MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        field,
        &change_id,
    )
    .await?;
    Ok(())
}

async fn resolve_recurrence_outcome_conflict(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    slot_on: chrono::NaiveDate,
    outcome: RecurrenceOutcome,
) -> Result<()> {
    let row = sqlx::query("SELECT task_id, resolved_at FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ? AND slot_on = ?")
        .bind(&workspace.id).bind(series_id).bind(slot_on.to_string()).fetch_one(&mut *conn).await?;
    let task_id: String = row.get("task_id");
    let resolved_at: String = row.get("resolved_at");
    let series_row = sqlx::query("SELECT workspace_id, id, title, description, project_id, priority, initial_status, frequency, interval, weekdays, timezone, start_on, available_local_time, due_policy, state, stopped_at, created_at, updated_at, deleted FROM recurrence_series WHERE workspace_id = ? AND id = ?")
        .bind(&workspace.id).bind(series_id).fetch_one(&mut *conn).await?;
    let series = crate::db::recurrence_series_from_row(&series_row)?;
    let operation = if task_id.is_empty() {
        op_type::RECORD_RECURRENCE_OUTCOME
    } else {
        let status = if outcome == RecurrenceOutcome::Completed {
            "done"
        } else {
            "canceled"
        };
        apply_field_value_in_workspace(conn, &workspace.id, &task_id.parse()?, "status", status)
            .await?;
        op_type::RESOLVE_RECURRENCE_OCCURRENCE
    };
    let change_id = insert_change(
        conn,
        "recurrence_series",
        series_id.as_str(),
        Some("outcome"),
        operation,
        crate::change_log::ChangePayload::workspace(workspace)
            .set("slot_on", slot_on.to_string())
            .set("task_id", &task_id)
            .set("outcome", outcome.as_str())
            .set(
                "task_status",
                if outcome == RecurrenceOutcome::Completed {
                    "done"
                } else {
                    "canceled"
                },
            )
            .set("resolved_at", &resolved_at)
            .set("task_status_change_id", "")
            .set("successor_task_id", "")
            .set("frequency", series.rule.frequency().as_str())
            .set("interval", series.rule.interval())
            .set("weekdays", series.rule.weekdays_set().to_string())
            .set("timezone", series.timezone.as_str())
            .set("start_on", series.start_on.to_string())
            .set(
                "available_local_time",
                series
                    .available_local_time
                    .map(|time| time.format("%H:%M:%S").to_string())
                    .unwrap_or_default(),
            )
            .set("due_policy", series.due_policy.as_str())
            .set("conflict_resolution", true)
            .into_value(),
        None,
    )
    .await?;
    sqlx::query("UPDATE recurrence_occurrences SET outcome = ?, outcome_change_id = ? WHERE workspace_id = ? AND series_id = ? AND slot_on = ?")
        .bind(outcome.as_str()).bind(change_id).bind(&workspace.id).bind(series_id).bind(slot_on.to_string()).execute(&mut *conn).await?;
    Ok(())
}

async fn resolve_recurrence_state_conflict(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    state: RecurrenceSeriesState,
    remote_change_id: &str,
) -> Result<()> {
    let local = sqlx::query(
        "SELECT state, stopped_at FROM recurrence_series WHERE workspace_id = ? AND id = ?",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .fetch_one(&mut *conn)
    .await?;
    let local_state: String = local.get("state");
    let remote_payload: Option<String> =
        sqlx::query_scalar("SELECT payload FROM changes WHERE change_id = ?")
            .bind(remote_change_id)
            .fetch_optional(&mut *conn)
            .await?;
    let remote_payload = remote_payload
        .as_deref()
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()?;
    let stopped_at = if state == RecurrenceSeriesState::Stopped {
        if local_state == "stopped" {
            local.get("stopped_at")
        } else {
            remote_payload
                .as_ref()
                .and_then(|payload| payload.get("stopped_at"))
                .and_then(serde_json::Value::as_str)
                .context("error recurrence-stop-boundary-missing")?
                .to_string()
        }
    } else {
        String::new()
    };
    let changed_at = now();
    sqlx::query("UPDATE recurrence_series SET state = ?, stopped_at = ?, updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(state.as_str()).bind(&stopped_at).bind(&changed_at).bind(&workspace.id).bind(series_id).execute(&mut *conn).await?;
    let operation = if state == RecurrenceSeriesState::Stopped {
        op_type::STOP_RECURRENCE_SERIES
    } else {
        op_type::SET_RECURRENCE_STATE
    };
    let change_id = insert_change(
        conn,
        "recurrence_series",
        series_id.as_str(),
        Some("state"),
        operation,
        crate::change_log::ChangePayload::workspace(workspace)
            .set("state", state.as_str())
            .set("stopped_at", &stopped_at)
            .set("changed_at", &changed_at)
            .set("conflict_resolution", true)
            .into_value(),
        None,
    )
    .await?;
    crate::db::set_entity_field_version(
        conn,
        &workspace.id,
        crate::types::MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        "state",
        &change_id,
    )
    .await?;
    cleanup_recurrence_projections(conn, workspace, series_id, state, &stopped_at).await?;
    Ok(())
}

pub(crate) async fn cleanup_recurrence_projections(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    state: RecurrenceSeriesState,
    stopped_at: &str,
) -> Result<()> {
    let archived_at = now();
    if state == RecurrenceSeriesState::Paused {
        let paused_at: String = sqlx::query_scalar(
            "SELECT paused_at FROM recurrence_pause_intervals
             WHERE workspace_id = ? AND series_id = ? AND resumed_at = ''
             ORDER BY paused_at DESC LIMIT 1",
        )
        .bind(&workspace.id)
        .bind(series_id)
        .fetch_one(&mut *conn)
        .await?;
        archive_projected_occurrences_at_or_after(
            conn,
            workspace,
            series_id,
            &paused_at,
            &archived_at,
        )
        .await?;
    } else if state == RecurrenceSeriesState::Stopped {
        archive_projected_occurrences_at_or_after(
            conn,
            workspace,
            series_id,
            stopped_at,
            &archived_at,
        )
        .await?;
    }
    Ok(())
}

async fn archive_projected_occurrences_at_or_after(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    boundary: &str,
    archived_at: &str,
) -> Result<()> {
    let row = sqlx::query("SELECT workspace_id, id, title, description, project_id, priority, initial_status, frequency, interval, weekdays, timezone, start_on, available_local_time, due_policy, state, stopped_at, created_at, updated_at, deleted FROM recurrence_series WHERE workspace_id = ? AND id = ?")
        .bind(&workspace.id).bind(series_id).fetch_one(&mut *conn).await?;
    let series = crate::db::recurrence_series_from_row(&row)?;
    let schedule = crate::recurrence::RecurrenceSchedule::new(
        series.rule,
        series.timezone,
        series.start_on,
        series.available_local_time,
        series.due_policy,
    );
    let rows = sqlx::query("SELECT slot_on FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ? AND projection_state = 'projected' AND outcome = ''")
        .bind(&workspace.id).bind(series_id).fetch_all(&mut *conn).await?;
    for row in rows {
        let slot: chrono::NaiveDate = row.get::<String, _>("slot_on").parse()?;
        if crate::recurrence::slot_values(&schedule, slot)?
            .boundary_at
            .as_str()
            >= boundary
        {
            sqlx::query("UPDATE recurrence_occurrences SET projection_state = 'archived', archived_at = ? WHERE workspace_id = ? AND series_id = ? AND slot_on = ? AND outcome = ''")
                .bind(archived_at).bind(&workspace.id).bind(series_id).bind(slot.to_string()).execute(&mut *conn).await?;
        }
    }
    Ok(())
}

pub async fn conflict_variant_value(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    token: &str,
) -> Result<String> {
    for detail in task_conflicts(conn, workspace, task_id, Some(field)).await? {
        if token == detail.variant_a {
            return Ok(detail.local_value);
        }
        if token == detail.variant_b {
            return Ok(detail.remote_value);
        }
    }
    bail!("error unknown-variant token={token}")
}

pub(crate) enum ConflictValueChoice {
    Local,
    Remote,
}

#[derive(Debug)]
pub(crate) struct ConflictNotFoundError {
    task_id: TaskId,
    field: &'static str,
}

impl std::fmt::Display for ConflictNotFoundError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "error conflict-not-found task_id={} field={}",
            self.task_id, self.field
        )
    }
}

impl std::error::Error for ConflictNotFoundError {}

pub async fn resolve_conflict(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    value: &str,
) -> Result<ConflictOutcome> {
    Ok(resolve_conflict_value(
        conn,
        workspace,
        task_id,
        field,
        ResolutionValue::Explicit(value),
        None,
    )
    .await?
    .outcome)
}

pub(crate) async fn resolve_conflict_choice(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    choice: ConflictValueChoice,
) -> Result<ConflictOutcome> {
    Ok(resolve_conflict_value(
        conn,
        workspace,
        task_id,
        field,
        ResolutionValue::Choice(choice),
        None,
    )
    .await?
    .outcome)
}

enum ResolutionValue<'a> {
    Explicit(&'a str),
    Choice(ConflictValueChoice),
}

async fn resolve_conflict_value(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    resolution: ResolutionValue<'_>,
    tui_summary: Option<&str>,
) -> Result<ConflictResolutionOutcome> {
    let task_field = TaskField::parse_or_unknown(field)?;
    let field = task_field.as_str();
    let mut tx = begin_immediate(conn).await?;
    let before = crate::undo::task_field_value(&mut tx, &workspace.id, task_id, field).await?;
    let conflict_id = crate::undo::conflict_row_id(&mut tx, &workspace.id, task_id, field).await?;
    let value = match resolution {
        ResolutionValue::Explicit(value) => value.to_string(),
        ResolutionValue::Choice(choice) => {
            let values = sqlx::query_as::<_, (String, String)>(
                "SELECT local_value, remote_value FROM conflicts
                 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
            )
            .bind(&workspace.id)
            .bind(task_id)
            .bind(field)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                anyhow::Error::new(ConflictNotFoundError {
                    task_id: task_id.clone(),
                    field,
                })
            })?;
            match choice {
                ConflictValueChoice::Local => values.0,
                ConflictValueChoice::Remote => values.1,
            }
        }
    };
    if task_field == TaskField::IsEpic
        && value == "0"
        && crate::operations::task_has_epic_children(&mut tx, &workspace.id, task_id).await?
    {
        bail!("error epic-has-children task_id={task_id}");
    }
    let result = sqlx::query(
        "UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(field)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(anyhow::Error::new(ConflictNotFoundError {
            task_id: task_id.clone(),
            field,
        }));
    }
    let payload = if task_field.is_project() {
        let project = resolve_project_for_stored_value(&mut tx, &workspace.id, &value).await?;
        apply_project_id_in_workspace(&mut tx, &workspace.id, task_id, &project.id).await?;
        TaskField::project_payload(&workspace.id, &workspace.key, &project)
    } else {
        apply_field_value_in_workspace(&mut tx, &workspace.id, task_id, field, &value).await?;
        task_field.scalar_payload(&workspace.id, &workspace.key, &value)?
    };
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some(field),
        op_type::RESOLVE_FIELD,
        payload,
        None,
    )
    .await?;
    set_field_version(&mut tx, task_id, field, &change_id).await?;
    let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
    let after = crate::undo::task_field_value(&mut tx, &workspace.id, task_id, field).await?;
    if let Some(summary) = tui_summary {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            summary,
            UndoPayload {
                commands: vec![UndoCommand::RestoreConflictResolution {
                    task_id: task_id.clone(),
                    field: field.to_string(),
                    before: before.clone(),
                    after: after.clone(),
                    conflict_id,
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!(task_id = %task_id, field = %field, "conflict resolved");
    Ok(ConflictResolutionOutcome {
        outcome: ConflictOutcome {
            task,
            field: field.to_string(),
        },
        before,
        after,
        conflict_id,
    })
}
