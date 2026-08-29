use crate::ids::{TaskId, WorkspaceId};
use anyhow::Result;
use serde_json::Value;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};
use std::collections::HashMap;

use crate::change_log::op_type;
use crate::query::types::{RecentActionItem, RecentActionTarget};
use crate::refs::DisplayRefContext;

const RECENT_ACTION_LIMIT: i64 = 80;
const TASK_ACTIVITY_LIMIT: i64 = 8;
const SQLITE_BIND_CHUNK_SIZE: usize = 900;

pub async fn list_recent_actions_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    project_scope: Option<&str>,
) -> Result<Vec<RecentActionItem>> {
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    let series_refs = super::recurrence::SeriesRefContext::load(conn, workspace_id).await?;

    let rows = sqlx::query(
        "SELECT c.change_id, c.entity_type, c.entity_id, c.field, c.op_type, c.payload,
                c.created_at, c.server_seq,
                t.title AS task_title, t.status AS task_status, t.deleted AS task_deleted,
                p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
                rs.id AS recurrence_series_id, rs.title AS recurrence_series_title,
                related.id AS related_task_id, related_project.prefix AS related_project_prefix
         FROM changes c
         LEFT JOIN tasks t
           ON c.entity_type = 'task'
          AND t.workspace_id = ?
          AND t.id = c.entity_id
         LEFT JOIN recurrence_occurrences ro
           ON c.entity_type = 'task'
          AND ro.workspace_id = ?
          AND ro.task_id = c.entity_id
         LEFT JOIN recurrence_series rs
           ON rs.workspace_id = ?
          AND rs.id = CASE
              WHEN c.entity_type = 'recurrence_series' THEN c.entity_id
              ELSE ro.series_id
          END
         LEFT JOIN projects p
           ON p.workspace_id = ?
          AND ((c.entity_type = 'project' AND p.id = c.entity_id)
               OR p.id = t.project_id OR p.id = rs.project_id)
         LEFT JOIN tasks related
           ON related.workspace_id = ?
          AND related.id = COALESCE(
              json_extract(c.payload, '$.depends_on_task_id'),
              json_extract(c.payload, '$.related_task_id'),
              json_extract(c.payload, '$.epic_task_id')
          )
         LEFT JOIN projects related_project
           ON related_project.workspace_id = ?
          AND related_project.id = related.project_id
         WHERE json_extract(c.payload, '$.workspace_id') = ?
           AND NOT EXISTS (
             SELECT 1 FROM changes resolving
             WHERE resolving.entity_type = 'recurrence_series'
               AND resolving.op_type = 'resolve_recurrence_occurrence'
               AND (
                 json_extract(resolving.payload, '$.task_status_change_id') = c.change_id
                 OR (c.op_type = 'create_task'
                     AND json_extract(resolving.payload, '$.successor_task_id') = c.entity_id)
                 OR (c.op_type = 'project_recurrence_occurrence'
                     AND json_extract(resolving.payload, '$.successor_task_id') =
                         json_extract(c.payload, '$.task_id'))
               )
           )
           AND (? IS NULL
                OR p.key = ?
                OR json_extract(c.payload, '$.project_key') = ?)
         ORDER BY c.created_at DESC, c.local_seq DESC
         LIMIT ?",
    )
    .bind(workspace_id)
    .bind(workspace_id)
    .bind(workspace_id)
    .bind(workspace_id)
    .bind(workspace_id)
    .bind(workspace_id)
    .bind(workspace_id)
    .bind(project_scope)
    .bind(project_scope)
    .bind(project_scope)
    .bind(RECENT_ACTION_LIMIT)
    .fetch_all(&mut *conn)
    .await?;

    rows.into_iter()
        .map(|row| action_from_row(row, workspace_id, &display_refs, &series_refs))
        .collect()
}

pub(crate) async fn task_activity_for_tasks_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, Vec<RecentActionItem>>> {
    let mut activity_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok(activity_by_task);
    }
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    let series_refs = super::recurrence::SeriesRefContext::load(conn, workspace_id).await?;

    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH resolved_recurrence_changes(status_change_id, successor_task_id) AS MATERIALIZED (
                 SELECT json_extract(payload, '$.task_status_change_id'),
                        json_extract(payload, '$.successor_task_id')
                 FROM changes
                 WHERE entity_type = 'recurrence_series'
                   AND op_type = 'resolve_recurrence_occurrence'
             ), requested(task_id) AS (VALUES ",
        );
        for (index, task_id) in chunk.iter().enumerate() {
            if index > 0 {
                query.push(", ");
            }
            query.push("(");
            query.push_bind(task_id);
            query.push(")");
        }
        query.push(
            "), top_changes AS (
                 SELECT c.change_id, c.entity_type, c.entity_id, c.field, c.op_type,
                        c.payload, c.created_at, c.local_seq, c.server_seq
                 FROM requested r
                 JOIN changes c
                   ON c.rowid IN (
                   SELECT candidate.rowid
                   FROM changes candidate
                   WHERE candidate.entity_type = 'task'
                     AND candidate.entity_id = r.task_id
                     AND json_extract(candidate.payload, '$.workspace_id') = ",
        );
        query.push_bind(workspace_id);
        push_unsuppressed_task_change(&mut query, "candidate");
        query.push(
            " ORDER BY candidate.created_at DESC, candidate.local_seq DESC
                   LIMIT ",
        );
        query.push_bind(TASK_ACTIVITY_LIMIT);
        query.push(
            " )
             ), driver_changes AS (
                 SELECT c.change_id, c.entity_type, c.entity_id, c.field, c.op_type,
                        c.payload, c.created_at, c.local_seq, c.server_seq
                 FROM requested r
                 JOIN tasks t
                   ON t.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND t.id = r.task_id
                 JOIN changes c
                   ON c.entity_type = 'task' AND c.entity_id = r.task_id
                 WHERE json_extract(c.payload, '$.workspace_id') = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND c.rowid = (
                   SELECT driver.rowid
                   FROM changes driver
                   WHERE driver.entity_type = 'task'
                     AND driver.entity_id = r.task_id
                     AND json_extract(driver.payload, '$.workspace_id') = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND driver.created_at = t.queue_activity_at
                     AND (
                       driver.op_type IN ('create_task', 'note_add', 'note_edit', 'note_delete')
                       OR (driver.op_type IN ('set_field', 'resolve_field')
                           AND driver.field IN ('status', 'priority'))
                     )",
        );
        push_unsuppressed_task_change(&mut query, "driver");
        query.push(
            " ORDER BY driver.created_at DESC, driver.local_seq DESC
                   LIMIT 1
                 )
             ), selected AS (
                 SELECT * FROM top_changes
                 UNION
                 SELECT * FROM driver_changes
             )
             SELECT selected.change_id, selected.entity_type, selected.entity_id,
                    selected.field, selected.op_type, selected.payload,
                    selected.created_at, selected.server_seq,
                    t.queue_activity_at AS task_queue_activity_at,
                    t.title AS task_title, t.status AS task_status, t.deleted AS task_deleted,
                    p.key AS project_key, p.name AS project_name, p.prefix AS project_prefix,
                    rs.id AS recurrence_series_id, rs.title AS recurrence_series_title,
                    related.id AS related_task_id,
                    related_project.prefix AS related_project_prefix
             FROM selected
             JOIN tasks t
               ON t.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND t.id = selected.entity_id
             LEFT JOIN recurrence_occurrences ro
               ON ro.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND ro.task_id = selected.entity_id
             LEFT JOIN recurrence_series rs
               ON rs.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND rs.id = ro.series_id
             LEFT JOIN projects p
               ON p.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND p.id = t.project_id
             LEFT JOIN tasks related
               ON related.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND related.id = COALESCE(
                   json_extract(selected.payload, '$.depends_on_task_id'),
                   json_extract(selected.payload, '$.related_task_id'),
                   json_extract(selected.payload, '$.epic_task_id')
               )
             LEFT JOIN projects related_project
               ON related_project.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(
            " AND related_project.id = related.project_id
             ORDER BY selected.entity_id, selected.created_at DESC, selected.local_seq DESC",
        );

        for row in query.build().fetch_all(&mut *conn).await? {
            let action = action_from_row(row, workspace_id, &display_refs, &series_refs)?;
            let task_id = action.entity_id.parse()?;
            activity_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(action);
        }
    }
    Ok(activity_by_task)
}

fn push_unsuppressed_task_change(query: &mut QueryBuilder<Sqlite>, alias: &str) {
    query.push(" AND ");
    query.push(alias);
    query.push(".change_id NOT IN (SELECT status_change_id FROM resolved_recurrence_changes WHERE status_change_id IS NOT NULL)");
    query.push(" AND (");
    query.push(alias);
    query.push(".op_type != 'create_task' OR ");
    query.push(alias);
    query.push(".entity_id NOT IN (SELECT successor_task_id FROM resolved_recurrence_changes WHERE successor_task_id IS NOT NULL))");
    query.push(" AND (");
    query.push(alias);
    query.push(".op_type != 'project_recurrence_occurrence' OR json_extract(");
    query.push(alias);
    query.push(".payload, '$.task_id') IS NULL OR json_extract(");
    query.push(alias);
    query.push(".payload, '$.task_id') NOT IN (SELECT successor_task_id FROM resolved_recurrence_changes WHERE successor_task_id IS NOT NULL))");
}

fn action_from_row(
    row: sqlx::sqlite::SqliteRow,
    workspace_id: &WorkspaceId,
    display_refs: &DisplayRefContext,
    series_refs: &super::recurrence::SeriesRefContext,
) -> Result<RecentActionItem> {
    let change_id: String = row.try_get("change_id")?;
    let entity_type: String = row.try_get("entity_type")?;
    let entity_id: String = row.try_get("entity_id")?;
    let field: Option<String> = row.try_get("field")?;
    let op_type: String = row.try_get("op_type")?;
    let payload_raw: String = row.try_get("payload")?;
    let payload: Value = serde_json::from_str(&payload_raw).unwrap_or(Value::Null);
    let created_at: String = row.try_get("created_at")?;
    let server_seq: Option<i64> = row.try_get("server_seq")?;
    let task_title: Option<String> = row.try_get("task_title")?;
    let task_status: Option<String> = row.try_get("task_status")?;
    let task_deleted: Option<i64> = row.try_get("task_deleted")?;
    let project_key: Option<String> = row.try_get("project_key")?;
    let project_name: Option<String> = row.try_get("project_name")?;
    let project_prefix: Option<String> = row.try_get("project_prefix")?;
    let recurrence_series_id: Option<crate::recurrence::RecurrenceSeriesId> =
        row.try_get("recurrence_series_id")?;
    let recurrence_series_title: Option<String> = row.try_get("recurrence_series_title")?;
    let related_task_id: Option<TaskId> = row.try_get("related_task_id")?;
    let related_project_prefix: Option<String> = row.try_get("related_project_prefix")?;

    let display_ref = if let Some(series_id) = recurrence_series_id.as_ref() {
        Some(series_refs.display_ref(series_id))
    } else if entity_type == "task" {
        let task_id = entity_id.parse()?;
        project_prefix
            .as_deref()
            .map(|prefix| display_refs.display_ref_for_id(workspace_id, prefix, &task_id))
    } else {
        None
    };
    let (verb, summary, mut detail, accent) = action_text(
        &entity_type,
        &op_type,
        field.as_deref(),
        &payload,
        task_title.as_deref(),
        project_name.as_deref(),
    );
    if matches!(
        op_type.as_str(),
        op_type::DEPENDENCY_ADD
            | op_type::DEPENDENCY_REMOVE
            | op_type::RELATED_ADD
            | op_type::RELATED_REMOVE
            | op_type::EPIC_LINK_ADD
            | op_type::EPIC_LINK_REMOVE
    ) && let (Some(task_id), Some(prefix)) =
        (related_task_id.as_ref(), related_project_prefix.as_deref())
    {
        detail = Some(display_refs.display_ref_for_id(workspace_id, prefix, task_id));
    }

    let grouped_change_count = if op_type == op_type::RESOLVE_RECURRENCE_OCCURRENCE {
        if payload_string(&payload, "successor_task_id").is_some_and(|value| !value.is_empty()) {
            4
        } else {
            2
        }
    } else {
        1
    };

    Ok(RecentActionItem {
        change_id,
        entity_type,
        entity_id,
        op_type,
        field,
        created_at,
        synced: server_seq.is_some(),
        target: RecentActionTarget {
            display_ref,
            title: recurrence_series_title.or(task_title).or(project_name),
            project_key,
            status: task_status,
            deleted: task_deleted == Some(1),
        },
        verb,
        summary,
        detail,
        accent,
        grouped_change_count,
    })
}

fn action_text(
    entity_type: &str,
    op_type: &str,
    field: Option<&str>,
    payload: &Value,
    task_title: Option<&str>,
    project_name: Option<&str>,
) -> (String, String, Option<String>, String) {
    match (entity_type, op_type) {
        ("recurrence_series", op_type::CREATE_RECURRENCE_SERIES) => (
            "recurrence".to_string(),
            "created recurring series".to_string(),
            payload_string(payload, "title"),
            "green".to_string(),
        ),
        ("recurrence_series", op_type::RESOLVE_RECURRENCE_OCCURRENCE) => {
            let outcome = payload_string(payload, "outcome").unwrap_or_default();
            (
                "recurrence".to_string(),
                format!("recorded recurring occurrence {outcome}"),
                payload_string(payload, "slot_on"),
                "green".to_string(),
            )
        }
        ("recurrence_series", op_type::PROJECT_RECURRENCE_OCCURRENCE) => (
            "recurrence".to_string(),
            "projected recurring occurrence".to_string(),
            payload_string(payload, "slot_on"),
            "dim".to_string(),
        ),
        ("task", op_type::CREATE_TASK) => (
            "create".to_string(),
            title_summary("created task", task_title),
            payload_string(payload, "title"),
            "green".to_string(),
        ),
        ("task", op_type::SET_FIELD) => field_action_text(field, payload, task_title),
        ("task", op_type::RESOLVE_FIELD) => resolved_field_action_text(field, payload, task_title),
        ("task", op_type::ATTACHMENT_ADD) => (
            "attachment".to_string(),
            title_summary("added attachment", task_title),
            payload_string(payload, "filename").or_else(|| payload_string(payload, "media_type")),
            "blue".to_string(),
        ),
        ("task", op_type::ATTACHMENT_DELETE) => (
            "attachment".to_string(),
            title_summary("deleted attachment", task_title),
            payload_string(payload, "filename"),
            "red".to_string(),
        ),
        ("task", op_type::SET_TASK_METADATA) => {
            let key = payload_string(payload, "key").unwrap_or_else(|| "custom".to_string());
            (
                "metadata".to_string(),
                title_summary(&format!("set {key} metadata"), task_title),
                payload_string(payload, "value"),
                "blue".to_string(),
            )
        }
        ("task", op_type::REMOVE_TASK_METADATA) => {
            let key = payload_string(payload, "key").unwrap_or_else(|| "custom".to_string());
            (
                "metadata".to_string(),
                title_summary(&format!("removed {key} metadata"), task_title),
                payload_string(payload, "value"),
                "red".to_string(),
            )
        }
        ("task", op_type::LABEL_ADD) => (
            "label".to_string(),
            title_summary("added label", task_title),
            payload_string(payload, "label"),
            "green".to_string(),
        ),
        ("task", op_type::LABEL_REMOVE) => (
            "label".to_string(),
            title_summary("removed label", task_title),
            payload_string(payload, "label"),
            "red".to_string(),
        ),
        ("task", op_type::NOTE_ADD) => (
            "note".to_string(),
            title_summary("added note", task_title),
            payload_string(payload, "body").map(|body| preview(&body, 96)),
            "blue".to_string(),
        ),
        ("task", op_type::NOTE_EDIT) => (
            "note".to_string(),
            title_summary("edited note", task_title),
            payload_string(payload, "body").map(|body| preview(&body, 96)),
            "blue".to_string(),
        ),
        ("task", op_type::NOTE_DELETE) => (
            "note".to_string(),
            title_summary("deleted note", task_title),
            payload_string(payload, "body").map(|body| preview(&body, 96)),
            "red".to_string(),
        ),
        ("task", op_type::DEPENDENCY_ADD) => (
            "blocker".to_string(),
            title_summary("added blocker", task_title),
            payload_string(payload, "depends_on_task_id"),
            "pink".to_string(),
        ),
        ("task", op_type::DEPENDENCY_REMOVE) => (
            "blocker".to_string(),
            title_summary("removed blocker", task_title),
            payload_string(payload, "depends_on_task_id"),
            "dim".to_string(),
        ),
        ("task", op_type::RELATED_ADD) => (
            "related link".to_string(),
            title_summary("added related task", task_title),
            payload_string(payload, "related_task_id"),
            "blue".to_string(),
        ),
        ("task", op_type::RELATED_REMOVE) => (
            "related link".to_string(),
            title_summary("removed related task", task_title),
            payload_string(payload, "related_task_id"),
            "dim".to_string(),
        ),
        ("task", op_type::EPIC_LINK_ADD) => (
            "epic".to_string(),
            title_summary("added to epic", task_title),
            payload_string(payload, "epic_task_id"),
            "yellow".to_string(),
        ),
        ("task", op_type::EPIC_LINK_REMOVE) => (
            "epic".to_string(),
            title_summary("removed from epic", task_title),
            payload_string(payload, "epic_task_id"),
            "dim".to_string(),
        ),
        ("project", op_type::CREATE_PROJECT) => (
            "project".to_string(),
            title_summary("created project", project_name),
            payload_string(payload, "name"),
            "green".to_string(),
        ),
        ("project", op_type::SET_PROJECT_METADATA) => (
            "project".to_string(),
            title_summary("updated project", project_name),
            payload_string(payload, "name"),
            "blue".to_string(),
        ),
        ("project", op_type::PROJECT_DELETE) => (
            "project".to_string(),
            title_summary("deleted project", project_name),
            payload_string(payload, "key"),
            "red".to_string(),
        ),
        ("label", op_type::CREATE_LABEL) => (
            "label".to_string(),
            "created label".to_string(),
            payload_string(payload, "label").or_else(|| payload_string(payload, "name")),
            "green".to_string(),
        ),
        ("label", op_type::SET_LABEL_NAME) => (
            "label".to_string(),
            "renamed label".to_string(),
            payload_string(payload, "new_name"),
            "blue".to_string(),
        ),
        ("label", op_type::LABEL_DELETE) => (
            "label".to_string(),
            "deleted label".to_string(),
            payload_string(payload, "label").or_else(|| payload_string(payload, "name")),
            "red".to_string(),
        ),
        ("label", op_type::LABEL_RESTORE) => (
            "label".to_string(),
            "restored label".to_string(),
            payload_string(payload, "name"),
            "green".to_string(),
        ),
        ("workspace", _) => (
            "workspace".to_string(),
            "updated workspace".to_string(),
            payload_string(payload, "workspace_key"),
            "blue".to_string(),
        ),
        _ => (
            "change".to_string(),
            format!("recorded {op_type}"),
            field.map(str::to_string),
            "dim".to_string(),
        ),
    }
}

fn resolved_field_action_text(
    field: Option<&str>,
    payload: &Value,
    task_title: Option<&str>,
) -> (String, String, Option<String>, String) {
    let value = payload_string(payload, "project_key").or_else(|| payload_string(payload, "value"));
    let field_label = match field.unwrap_or_default() {
        "available_at" => "availability",
        "due_on" => "due date",
        "is_epic" => "epic designation",
        "" => "field",
        other => other,
    };
    let summary = value
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| format!("resolved {field_label} conflict to {value}"))
        .unwrap_or_else(|| format!("resolved {field_label} conflict"));
    (
        "conflict".to_string(),
        title_summary(&summary, task_title),
        value.filter(|value| !value.is_empty()),
        "blue".to_string(),
    )
}

fn field_action_text(
    field: Option<&str>,
    payload: &Value,
    task_title: Option<&str>,
) -> (String, String, Option<String>, String) {
    let value = payload_string(payload, "value");
    match field.unwrap_or_default() {
        "title" => (
            "title".to_string(),
            "renamed task".to_string(),
            value,
            "blue".to_string(),
        ),
        "description" => {
            let summary = if value.as_deref().is_some_and(str::is_empty) {
                "cleared description"
            } else {
                "edited description"
            };
            (
                "details".to_string(),
                title_summary(summary, task_title),
                value.map(|description| preview(&description, 96)),
                "blue".to_string(),
            )
        }
        "status" => (
            "status".to_string(),
            title_summary(
                &value
                    .as_deref()
                    .map(|status| format!("changed status to {status}"))
                    .unwrap_or_else(|| "changed status".to_string()),
                task_title,
            ),
            value,
            "green".to_string(),
        ),
        "priority" => (
            "priority".to_string(),
            title_summary(
                &value
                    .as_deref()
                    .map(|priority| format!("changed priority to {priority}"))
                    .unwrap_or_else(|| "changed priority".to_string()),
                task_title,
            ),
            value,
            "yellow".to_string(),
        ),
        "project" => (
            "project".to_string(),
            title_summary("moved task", task_title),
            payload_string(payload, "project_key").or(value),
            "pink".to_string(),
        ),
        "deleted" => {
            let deleted = value.as_deref() == Some("1") || value.as_deref() == Some("true");
            (
                "delete".to_string(),
                title_summary(
                    if deleted {
                        "deleted task"
                    } else {
                        "restored task"
                    },
                    task_title,
                ),
                None,
                if deleted { "red" } else { "green" }.to_string(),
            )
        }
        "available_at" => {
            let summary = if value.as_deref().is_none_or(str::is_empty) {
                "cleared availability"
            } else {
                "changed availability"
            };
            (
                "availability".to_string(),
                title_summary(summary, task_title),
                value.filter(|value| !value.is_empty()),
                "blue".to_string(),
            )
        }
        "due_on" => {
            let summary = if value.as_deref().is_none_or(str::is_empty) {
                "cleared due date"
            } else {
                "changed due date"
            };
            (
                "due date".to_string(),
                title_summary(summary, task_title),
                value.filter(|value| !value.is_empty()),
                "blue".to_string(),
            )
        }
        "is_epic" => {
            let marked = value.as_deref() == Some("1");
            (
                "epic".to_string(),
                title_summary(
                    if marked {
                        "marked as epic"
                    } else {
                        "removed epic designation"
                    },
                    task_title,
                ),
                None,
                "yellow".to_string(),
            )
        }
        other => (
            "field".to_string(),
            title_summary(&format!("changed {}", other.replace('_', " ")), task_title),
            value,
            "blue".to_string(),
        ),
    }
}

fn title_summary(prefix: &str, title: Option<&str>) -> String {
    title
        .filter(|title| !title.is_empty())
        .map(|title| format!("{prefix}: {title}"))
        .unwrap_or_else(|| prefix.to_string())
}

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    let value = payload.get(key)?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if value.is_null() {
        None
    } else {
        Some(value.to_string())
    }
}

fn preview(value: &str, limit: usize) -> String {
    let trimmed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() <= limit {
        return trimmed;
    }
    let mut out = trimmed
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn task_activity_is_scoped_and_bounded_per_task() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace = crate::test_support::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        let project = crate::test_support::resolve_or_create_project_in_workspace(
            &mut conn,
            &workspace.id,
            "App",
        )
        .await
        .unwrap();
        let first = crate::test_support::create_task(
            &mut conn,
            &workspace,
            crate::operations::TaskDraft {
                title: "first task".to_string(),
                description: String::new(),
                project: Some(project.key.clone()),
                status: "todo".to_string(),
                priority: "none".to_string(),
                source: crate::choices::TaskSource::Tui,
                labels: Vec::new(),
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();
        let second = crate::test_support::create_task(
            &mut conn,
            &workspace,
            crate::operations::TaskDraft {
                title: "second task".to_string(),
                description: String::new(),
                project: Some(project.key),
                status: "todo".to_string(),
                priority: "none".to_string(),
                source: crate::choices::TaskSource::Tui,
                labels: Vec::new(),
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();
        let queue_activity = crate::test_support::update_task(
            &mut conn,
            &workspace,
            &first.task.id,
            crate::operations::TaskUpdate {
                priority: Some("low".to_string()),
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap()
        .task
        .queue_activity_at;
        for index in 0..10 {
            crate::test_support::update_task(
                &mut conn,
                &workspace,
                &first.task.id,
                crate::operations::TaskUpdate {
                    title: Some(format!("first task {index}")),
                    ..crate::operations::TaskUpdate::default()
                },
            )
            .await
            .unwrap();
        }
        let suppressed_change_id = crate::db::insert_change(
            &mut conn,
            "task",
            &first.task.id,
            Some("title"),
            op_type::SET_FIELD,
            json!({
                "workspace_id": workspace.id,
                "value": "suppressed"
            }),
            None,
        )
        .await
        .unwrap();
        crate::db::insert_change(
            &mut conn,
            "recurrence_series",
            "series",
            None,
            op_type::RESOLVE_RECURRENCE_OCCURRENCE,
            json!({
                "workspace_id": workspace.id,
                "task_status_change_id": suppressed_change_id
            }),
            None,
        )
        .await
        .unwrap();
        crate::test_support::update_task(
            &mut conn,
            &workspace,
            &first.task.id,
            crate::operations::TaskUpdate {
                add_labels: vec!["backend".to_string()],
                create_missing_labels: true,
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        crate::test_support::update_task(
            &mut conn,
            &workspace,
            &first.task.id,
            crate::operations::TaskUpdate {
                set_metadata: vec![crate::metadata::TaskMetadataInput {
                    key: "customer".to_string(),
                    value: "Acme".to_string(),
                }],
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        crate::test_support::update_task(
            &mut conn,
            &workspace,
            &first.task.id,
            crate::operations::TaskUpdate {
                remove_metadata: vec!["customer".to_string()],
                ..crate::operations::TaskUpdate::default()
            },
        )
        .await
        .unwrap();

        let activity = task_activity_for_tasks_in_workspace(
            &mut conn,
            &workspace.id,
            &[first.task.id.clone(), second.task.id.clone()],
        )
        .await
        .unwrap();

        assert_eq!(
            activity[&first.task.id].len(),
            TASK_ACTIVITY_LIMIT as usize + 1
        );
        assert_eq!(activity[&second.task.id].len(), 1);
        assert!(
            activity[&first.task.id]
                .iter()
                .all(|action| action.entity_id == first.task.id.to_string())
        );
        assert!(
            activity[&first.task.id]
                .iter()
                .all(|action| action.change_id != suppressed_change_id)
        );
        assert!(activity[&first.task.id].iter().any(|action| {
            action.field.as_deref() == Some("priority") && action.created_at == queue_activity
        }));
        assert!(activity[&first.task.id].iter().any(|action| {
            action.op_type == op_type::LABEL_ADD && action.detail.as_deref() == Some("backend")
        }));
        assert!(activity[&first.task.id].iter().any(|action| {
            action.op_type == op_type::REMOVE_TASK_METADATA
                && action.summary.starts_with("removed customer metadata")
                && action.detail.as_deref() == Some("Acme")
        }));
    }

    #[test]
    fn scalar_activity_summaries_include_the_new_value() {
        let status = field_action_text(Some("status"), &json!({"value": "done"}), Some("subject"));
        let priority =
            field_action_text(Some("priority"), &json!({"value": "high"}), Some("subject"));

        assert_eq!(status.1, "changed status to done: subject");
        assert_eq!(status.2.as_deref(), Some("done"));
        assert_eq!(priority.1, "changed priority to high: subject");
        assert_eq!(priority.2.as_deref(), Some("high"));
    }

    #[test]
    fn activity_presentations_retain_distinguishing_values() {
        let cases = [
            (
                op_type::LABEL_ADD,
                None,
                json!({"label": "backend"}),
                "added label: subject",
                Some("backend"),
            ),
            (
                op_type::ATTACHMENT_ADD,
                Some("attachments"),
                json!({"filename": "mockup.png"}),
                "added attachment: subject",
                Some("mockup.png"),
            ),
            (
                op_type::SET_TASK_METADATA,
                Some("metadata:field-id"),
                json!({"key": "customer", "value": "Acme"}),
                "set customer metadata: subject",
                Some("Acme"),
            ),
            (
                op_type::NOTE_DELETE,
                Some("notes"),
                json!({"body": "Confirmed the refresh race"}),
                "deleted note: subject",
                Some("Confirmed the refresh race"),
            ),
        ];

        for (operation, field, payload, expected_summary, expected_detail) in cases {
            let action = action_text("task", operation, field, &payload, Some("subject"), None);
            assert_eq!(action.1, expected_summary);
            assert_eq!(action.2.as_deref(), expected_detail);
        }

        let due = field_action_text(
            Some("due_on"),
            &json!({"value": "2026-08-28"}),
            Some("subject"),
        );
        assert_eq!(due.1, "changed due date: subject");
        assert_eq!(due.2.as_deref(), Some("2026-08-28"));

        let epic = field_action_text(Some("is_epic"), &json!({"value": "1"}), Some("subject"));
        assert_eq!(epic.1, "marked as epic: subject");
        assert_eq!(epic.2, None);

        let resolution = action_text(
            "task",
            op_type::RESOLVE_FIELD,
            Some("status"),
            &json!({"value": "todo"}),
            Some("subject"),
            None,
        );
        assert_eq!(resolution.1, "resolved status conflict to todo: subject");
    }

    #[test]
    fn related_mutations_use_related_link_activity_vocabulary() {
        let payload = json!({"related_task_id": "BBBB000000000002"});
        let added = action_text(
            "task",
            op_type::RELATED_ADD,
            Some("related"),
            &payload,
            Some("subject"),
            None,
        );
        assert_eq!(
            added,
            (
                "related link".to_string(),
                "added related task: subject".to_string(),
                Some("BBBB000000000002".to_string()),
                "blue".to_string(),
            )
        );

        let removed = action_text(
            "task",
            op_type::RELATED_REMOVE,
            Some("related"),
            &payload,
            Some("subject"),
            None,
        );
        assert_eq!(
            removed,
            (
                "related link".to_string(),
                "removed related task: subject".to_string(),
                Some("BBBB000000000002".to_string()),
                "dim".to_string(),
            )
        );
    }
}
