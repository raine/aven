use anyhow::{Result, bail};
use sqlx::{SqliteConnection, query_scalar};

use crate::db;

use super::super::{IntegrityCheck, IntegrityReport};

pub(super) async fn report_with_connection(conn: &mut SqliteConnection) -> Result<IntegrityReport> {
    let quick_check_value: String = query_scalar("PRAGMA quick_check")
        .fetch_one(&mut *conn)
        .await?;
    let mut checks = Vec::new();
    checks.push(count_check(
        conn,
        "task projects",
        "SELECT count(*) FROM tasks t LEFT JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "project paths",
        "SELECT count(*) FROM project_paths pp LEFT JOIN projects p ON p.workspace_id = pp.workspace_id AND p.id = pp.project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "project aliases",
        "SELECT count(*) FROM project_id_aliases a LEFT JOIN projects p ON p.workspace_id = a.workspace_id AND p.id = a.local_project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "metadata field workspaces",
        "SELECT count(*) FROM metadata_fields f LEFT JOIN workspaces w ON w.id = f.workspace_id WHERE w.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "metadata aliases",
        "SELECT count(*) FROM metadata_field_id_aliases a LEFT JOIN metadata_fields f ON f.workspace_id = a.workspace_id AND f.id = a.local_field_id WHERE f.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task metadata tasks",
        "SELECT count(*) FROM task_metadata m LEFT JOIN tasks t ON t.workspace_id = m.workspace_id AND t.id = m.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task metadata fields",
        "SELECT count(*) FROM task_metadata m LEFT JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id WHERE f.id IS NULL",
    )
    .await?);
    checks.push(
        count_check(
            conn,
            "task metadata value size",
            "SELECT count(*) FROM task_metadata WHERE length(CAST(value AS BLOB)) > 4096",
        )
        .await?,
    );
    checks.push(count_check(
        conn,
        "recurrence metadata series",
        "SELECT count(*) FROM recurrence_series_metadata m LEFT JOIN recurrence_series s ON s.workspace_id = m.workspace_id AND s.id = m.series_id WHERE s.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence metadata fields",
        "SELECT count(*) FROM recurrence_series_metadata m LEFT JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id WHERE f.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence metadata value size",
        "SELECT count(*) FROM recurrence_series_metadata WHERE length(CAST(value AS BLOB)) > 4096",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence metadata aggregate limits",
        "SELECT count(*) FROM (SELECT workspace_id, series_id FROM recurrence_series_metadata GROUP BY workspace_id, series_id HAVING count(*) > 128 OR sum(length(CAST(value AS BLOB))) > 32768)",
    )
    .await?);
    let invalid_metadata_keys = sqlx::query_scalar::<_, String>("SELECT key FROM metadata_fields")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .filter(|key| match crate::metadata::normalize_metadata_key(key) {
            Ok(normalized) => normalized != key.as_str(),
            Err(_) => true,
        })
        .count();
    checks.push(IntegrityCheck {
        label: "metadata field keys",
        ok: invalid_metadata_keys == 0,
        value: invalid_metadata_keys.to_string(),
    });
    checks.push(count_check(
        conn,
        "task label tasks",
        "SELECT count(*) FROM task_labels tl LEFT JOIN tasks t ON t.workspace_id = tl.workspace_id AND t.id = tl.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task label labels",
        "SELECT count(*) FROM task_labels tl LEFT JOIN labels l ON l.workspace_id = tl.workspace_id AND l.name = tl.label WHERE l.name IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "notes",
        "SELECT count(*) FROM notes n LEFT JOIN tasks t ON t.workspace_id = n.workspace_id AND t.id = n.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "note changes",
        "SELECT count(*) FROM notes n LEFT JOIN changes c ON c.change_id = n.change_id WHERE c.change_id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "dependency tasks",
        "SELECT count(*) FROM task_dependencies d LEFT JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "dependency targets",
        "SELECT count(*) FROM task_dependencies d LEFT JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.depends_on_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "related link first endpoints",
        "SELECT count(*) FROM task_related_links r LEFT JOIN tasks t ON t.workspace_id = r.workspace_id AND t.id = r.task_a_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "related link second endpoints",
        "SELECT count(*) FROM task_related_links r LEFT JOIN tasks t ON t.workspace_id = r.workspace_id AND t.id = r.task_b_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "related link canonical pairs",
        "SELECT count(*) FROM task_related_links WHERE task_a_id >= task_b_id OR linked NOT IN (0, 1)",
    )
    .await?);
    checks.push(
        count_check(
            conn,
            "related link changes",
            "SELECT count(*) FROM task_related_links r
         WHERE NOT EXISTS (
             SELECT 1 FROM changes c
             WHERE c.change_id = r.last_change_id
               AND c.entity_type = 'task' AND c.field = 'related'
               AND c.op_type = CASE r.linked WHEN 1 THEN 'related_add' ELSE 'related_remove' END
               AND CASE WHEN json_valid(c.payload) THEN json_extract(c.payload, '$.workspace_id') END = r.workspace_id
               AND CASE WHEN json_valid(c.payload) THEN json_extract(c.payload, '$.related_task_id') END IS NOT NULL
               AND min(c.entity_id, CASE WHEN json_valid(c.payload) THEN json_extract(c.payload, '$.related_task_id') END) = r.task_a_id
               AND max(c.entity_id, CASE WHEN json_valid(c.payload) THEN json_extract(c.payload, '$.related_task_id') END) = r.task_b_id
         )",
        )
        .await?,
    );
    checks.push(count_check(
        conn,
        "epic link children",
        "SELECT count(*) FROM task_epic_links l LEFT JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.child_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "epic link parents",
        "SELECT count(*) FROM task_epic_links l LEFT JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "epic link parent flags",
        "SELECT count(*) FROM task_epic_links l JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id WHERE t.is_epic = 0",
    )
    .await?);
    checks.push(count_check(
        conn,
        "conflict tasks",
        "SELECT count(*) FROM conflicts c LEFT JOIN tasks t ON t.workspace_id = c.workspace_id AND t.id = c.entity_id WHERE c.resolved = 0 AND c.entity_type = 'task' AND t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task due dates",
        "SELECT count(*) FROM tasks WHERE due_on != '' AND (length(due_on) != 10 OR substr(due_on, 5, 1) != '-' OR substr(due_on, 8, 1) != '-' OR date(due_on) IS NULL OR strftime('%Y-%m-%d', due_on) != due_on)",
    )
    .await?);
    checks.push(count_check(
        conn,
        "field version tasks",
        "SELECT count(*) FROM field_versions fv LEFT JOIN tasks t ON t.workspace_id = fv.workspace_id AND t.id = fv.entity_id WHERE fv.entity_type = 'task' AND t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "field version changes",
        "SELECT count(*) FROM field_versions fv LEFT JOIN changes c ON c.change_id = fv.version WHERE c.change_id IS NULL AND NOT (fv.entity_type = 'task' AND EXISTS (SELECT 1 FROM recurrence_occurrences o WHERE o.workspace_id = fv.workspace_id AND o.task_id = fv.entity_id))",
    )
    .await?);
    checks.extend(super::recurrence_integrity_checks(conn).await?);
    push_meta_checks(conn, &mut checks).await?;

    Ok(IntegrityReport {
        quick_check_ok: quick_check_value == "ok",
        quick_check_value,
        checks,
    })
}

pub(super) fn ensure_ok(report: &IntegrityReport) -> Result<()> {
    let mut bad = vec![];
    if !report.quick_check_ok {
        bad.push("quick check");
    }
    for check in &report.checks {
        if !check.ok && check.label != "recurrence projection gaps" {
            bad.push(check.label);
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    bail!("error data-integrity-failed checks={}", bad.join(", "))
}

async fn count_check(
    conn: &mut SqliteConnection,
    label: &'static str,
    query: &'static str,
) -> Result<IntegrityCheck> {
    let count: i64 = query_scalar(query).fetch_one(&mut *conn).await?;
    Ok(IntegrityCheck {
        label,
        ok: count == 0,
        value: format!("{count} orphaned"),
    })
}

async fn push_meta_checks(
    conn: &mut SqliteConnection,
    checks: &mut Vec<IntegrityCheck>,
) -> Result<()> {
    let local_seq = db::get_meta(conn, "local_seq").await?;
    let local_seq_check = match local_seq {
        Some(raw) => match raw.parse::<i64>() {
            Ok(value) => {
                let max_seq: i64 = query_scalar("SELECT COALESCE(MAX(local_seq), 0) FROM changes")
                    .fetch_one(&mut *conn)
                    .await?;
                let ok = value >= max_seq;
                IntegrityCheck {
                    label: "meta local_seq",
                    ok,
                    value: value.to_string(),
                }
            }
            Err(error) => IntegrityCheck {
                label: "meta local_seq",
                ok: false,
                value: error.to_string(),
            },
        },
        None => IntegrityCheck {
            label: "meta local_seq",
            ok: false,
            value: "missing".to_string(),
        },
    };
    checks.push(local_seq_check);

    let sync_cursor = db::get_meta(conn, "sync_cursor").await?;
    let sync_cursor_ok = match sync_cursor {
        Some(raw) => match raw.parse::<i64>() {
            Ok(_) => IntegrityCheck {
                label: "sync cursor",
                ok: true,
                value: raw,
            },
            Err(error) => IntegrityCheck {
                label: "sync cursor",
                ok: false,
                value: error.to_string(),
            },
        },
        None => IntegrityCheck {
            label: "sync cursor",
            ok: false,
            value: "missing".to_string(),
        },
    };
    checks.push(sync_cursor_ok);
    let protocol = crate::sync::protocol::replica_protocol(conn).await;
    checks.push(IntegrityCheck {
        label: "replica sync protocol",
        ok: protocol.is_ok(),
        value: match protocol {
            Ok(value) => value.to_string(),
            Err(error) => error.to_string(),
        },
    });

    Ok(())
}
