use crate::ids::WorkspaceId;
use anyhow::Result;
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

use crate::change_log::op_type;
use crate::query::types::{RecentActionItem, RecentActionTarget};
use crate::refs::DisplayRefContext;

const RECENT_ACTION_LIMIT: i64 = 80;

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
                rs.id AS recurrence_series_id, rs.title AS recurrence_series_title
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
    let (verb, summary, detail, accent) = action_text(
        &entity_type,
        &op_type,
        field.as_deref(),
        &payload,
        task_title.as_deref(),
        project_name.as_deref(),
    );

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
            payload_string(payload, "note_id"),
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
        "description" => (
            "details".to_string(),
            title_summary("edited description", task_title),
            None,
            "blue".to_string(),
        ),
        "status" => (
            "status".to_string(),
            title_summary("changed status", task_title),
            value,
            "green".to_string(),
        ),
        "priority" => (
            "priority".to_string(),
            title_summary("changed priority", task_title),
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
        "is_epic" => (
            "epic".to_string(),
            title_summary("changed epic flag", task_title),
            value,
            "yellow".to_string(),
        ),
        other => (
            "field".to_string(),
            title_summary(&format!("changed {other}"), task_title),
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
