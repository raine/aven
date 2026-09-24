use std::collections::HashMap;

use anyhow::Result;
use chrono::DateTime;
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::db::{
    recurrence_occurrence_from_row, recurrence_pause_interval_from_row, recurrence_series_from_row,
    recurrence_series_label_from_row,
};
use crate::recurrence::{RecurrenceSchedule, derive_occurrence_identity, is_slot};

use super::super::IntegrityCheck;
use super::super::validation::recurrence::recurrence_stop_boundary_valid;
use super::count_check;

pub(super) async fn recurrence_integrity_checks(
    conn: &mut SqliteConnection,
) -> Result<Vec<IntegrityCheck>> {
    let mut checks = Vec::new();
    checks.push(count_check(
        conn,
        "recurrence series projects",
        "SELECT count(*) FROM recurrence_series s LEFT JOIN projects p ON p.workspace_id = s.workspace_id AND p.id = s.project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence series label series",
        "SELECT count(*) FROM recurrence_series_labels sl LEFT JOIN recurrence_series s ON s.workspace_id = sl.workspace_id AND s.id = sl.series_id WHERE s.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence series labels",
        "SELECT count(*) FROM recurrence_series_labels sl LEFT JOIN labels l ON l.workspace_id = sl.workspace_id AND l.name = sl.label WHERE l.name IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence field versions",
        "SELECT count(*) FROM field_versions fv LEFT JOIN recurrence_series s ON s.workspace_id = fv.workspace_id AND s.id = fv.entity_id WHERE fv.entity_type = 'recurrence_series' AND s.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence lifecycle conflicts",
        "SELECT count(*) FROM conflicts c LEFT JOIN recurrence_series s ON s.workspace_id = c.workspace_id AND s.id = c.entity_id WHERE c.resolved = 0 AND c.entity_type = 'recurrence_series' AND s.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence occurrence series",
        "SELECT count(*) FROM recurrence_occurrences o LEFT JOIN recurrence_series s ON s.workspace_id = o.workspace_id AND s.id = o.series_id WHERE s.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence projected uniqueness",
        "SELECT count(*) FROM (SELECT workspace_id, series_id FROM recurrence_occurrences WHERE projection_state = 'projected' GROUP BY workspace_id, series_id HAVING count(*) > 1)",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence projection gaps",
        "SELECT count(*) FROM recurrence_series s WHERE s.state = 'active' AND s.deleted = 0 AND NOT EXISTS (SELECT 1 FROM recurrence_occurrences o WHERE o.workspace_id = s.workspace_id AND o.series_id = s.id AND o.projection_state = 'projected') AND NOT EXISTS (SELECT 1 FROM conflicts c WHERE c.workspace_id = s.workspace_id AND c.entity_type = 'recurrence_series' AND c.entity_id = s.id AND c.field = 'state' AND c.resolved = 0)",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence task links",
        "SELECT count(*) FROM recurrence_occurrences o LEFT JOIN tasks t ON t.workspace_id = o.workspace_id AND t.id = o.task_id WHERE o.task_id != '' AND t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence projected tasks",
        "SELECT count(*) FROM recurrence_occurrences o LEFT JOIN tasks t ON t.workspace_id = o.workspace_id AND t.id = o.task_id WHERE o.projection_state = 'projected' AND (o.task_id = '' OR t.id IS NULL OR t.status NOT IN ('inbox', 'backlog', 'todo', 'active'))",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence task outcomes",
        "SELECT count(*) FROM recurrence_occurrences o JOIN tasks t ON t.workspace_id = o.workspace_id AND t.id = o.task_id WHERE (o.outcome = 'completed' AND t.status != 'done') OR (o.outcome = 'skipped' AND t.status != 'canceled')",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence outcome changes",
        "SELECT count(*) FROM recurrence_occurrences o LEFT JOIN changes c ON c.change_id = o.outcome_change_id AND c.entity_type = 'recurrence_series' AND c.entity_id = o.series_id AND c.field = 'outcome' AND c.op_type = 'resolve_recurrence_occurrence' AND CASE WHEN json_valid(c.payload) THEN json_extract(c.payload, '$.slot_on') END = o.slot_on AND CASE WHEN json_valid(c.payload) THEN json_extract(c.payload, '$.outcome') END = o.outcome AND CASE WHEN json_valid(c.payload) THEN json_extract(c.payload, '$.resolved_at') END = o.resolved_at WHERE o.outcome_change_id != '' AND c.change_id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence pause series",
        "SELECT count(*) FROM recurrence_pause_intervals p LEFT JOIN recurrence_series s ON s.workspace_id = p.workspace_id AND s.id = p.series_id WHERE s.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence pause tasks",
        "SELECT count(*) FROM recurrence_pause_intervals p LEFT JOIN recurrence_occurrences o ON o.workspace_id = p.workspace_id AND o.series_id = p.series_id AND o.slot_on = p.suspended_slot_on AND o.task_id = p.suspended_task_id WHERE p.suspended_task_id != '' AND o.task_id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence pause changes",
        "SELECT count(*) FROM recurrence_pause_intervals p LEFT JOIN changes opened ON opened.change_id = p.created_by_change_id AND opened.entity_type = 'recurrence_series' AND opened.entity_id = p.series_id LEFT JOIN changes closed ON closed.change_id = p.resolved_by_change_id AND closed.entity_type = 'recurrence_series' AND closed.entity_id = p.series_id WHERE opened.change_id IS NULL OR (p.resolved_by_change_id != '' AND closed.change_id IS NULL)",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence lifecycle state",
        "SELECT count(*) FROM recurrence_series s WHERE NOT EXISTS (SELECT 1 FROM conflicts c WHERE c.workspace_id = s.workspace_id AND c.entity_type = 'recurrence_series' AND c.entity_id = s.id AND c.field = 'state' AND c.resolved = 0) AND ((s.state = 'paused') != EXISTS (SELECT 1 FROM recurrence_pause_intervals p WHERE p.workspace_id = s.workspace_id AND p.series_id = s.id AND p.resumed_at = ''))",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence pause overlaps",
        "SELECT count(*) FROM recurrence_pause_intervals a JOIN recurrence_pause_intervals b ON a.workspace_id = b.workspace_id AND a.series_id = b.series_id AND a.id < b.id WHERE a.paused_at < CASE WHEN b.resumed_at = '' THEN '9999-12-31T23:59:59Z' ELSE b.resumed_at END AND b.paused_at < CASE WHEN a.resumed_at = '' THEN '9999-12-31T23:59:59Z' ELSE a.resumed_at END",
    )
    .await?);

    checks.extend(recurrence_row_checks(conn).await?);
    Ok(checks)
}

async fn recurrence_row_checks(conn: &mut SqliteConnection) -> Result<Vec<IntegrityCheck>> {
    let series_rows = sqlx::query("SELECT * FROM recurrence_series")
        .fetch_all(&mut *conn)
        .await?;
    let mut invalid_series = 0_usize;
    let mut series_by_id = HashMap::new();
    for row in &series_rows {
        match recurrence_series_from_row(row) {
            Ok(series) => {
                series_by_id.insert(
                    (series.workspace_id.to_string(), series.id.to_string()),
                    series,
                );
            }
            Err(_) => invalid_series += 1,
        }
    }

    let label_rows = sqlx::query("SELECT * FROM recurrence_series_labels")
        .fetch_all(&mut *conn)
        .await?;
    let invalid_labels = label_rows
        .iter()
        .filter(|row| recurrence_series_label_from_row(row).is_err())
        .count();

    let occurrence_rows = sqlx::query("SELECT * FROM recurrence_occurrences")
        .fetch_all(&mut *conn)
        .await?;
    let mut final_slots = HashMap::new();
    for row in &occurrence_rows {
        if let Ok(occurrence) = recurrence_occurrence_from_row(row) {
            final_slots
                .entry((occurrence.workspace_id, occurrence.series_id))
                .and_modify(|last: &mut chrono::NaiveDate| *last = (*last).max(occurrence.slot_on))
                .or_insert(occurrence.slot_on);
        }
    }
    let mut invalid_occurrences = 0_usize;
    let mut deterministic_identity_mismatches = 0_usize;
    let mut deterministic_change_mismatches = 0_usize;
    let mut deterministic_timestamp_mismatches = 0_usize;
    let mut deterministic_field_version_mismatches = 0_usize;
    let mut off_lattice_slots = 0_usize;
    let mut creation_boundary_violations = 0_usize;
    let mut stop_boundary_violations = 0_usize;
    for row in &occurrence_rows {
        let Ok(occurrence) = recurrence_occurrence_from_row(row) else {
            invalid_occurrences += 1;
            continue;
        };
        let Some(series) = series_by_id.get(&(
            occurrence.workspace_id.to_string(),
            occurrence.series_id.to_string(),
        )) else {
            continue;
        };
        if !is_slot(&series.rule, series.start_on, occurrence.slot_on) {
            off_lattice_slots += 1;
        }
        let schedule = RecurrenceSchedule::new(
            series.rule,
            series.timezone.clone(),
            series.start_on,
            series.available_local_time,
            series.due_policy,
        );
        let created_date =
            DateTime::parse_from_rfc3339(&series.created_at)
                .ok()
                .map(|created_at| {
                    created_at
                        .with_timezone(&series.timezone.timezone())
                        .date_naive()
                });
        if created_date.is_none_or(|created_date| occurrence.slot_on < created_date) {
            creation_boundary_violations += 1;
        }
        if let Some(task_id) = &occurrence.task_id {
            match derive_occurrence_identity(
                &occurrence.workspace_id,
                &occurrence.series_id,
                &schedule,
                occurrence.slot_on,
            ) {
                Ok(identity) if identity.task_id == *task_id => {
                    let created_at: Option<String> = sqlx::query_scalar(
                        "SELECT created_at FROM tasks WHERE workspace_id = ? AND id = ?",
                    )
                    .bind(&occurrence.workspace_id)
                    .bind(task_id)
                    .fetch_optional(&mut *conn)
                    .await?;
                    if created_at.as_deref() != Some(identity.created_at.as_str()) {
                        deterministic_timestamp_mismatches += 1;
                    }
                    // Every generation of the occurrence validates in its own form, and
                    // at least one is linked to its projection.
                    let creates: Vec<(String, String)> = sqlx::query_as(
                        "SELECT change_id, payload FROM changes WHERE entity_type = 'task' AND entity_id = ? AND field IS NULL AND op_type = 'create_task' AND created_at = ? AND json_extract(payload, '$.series_id') IS NOT NULL",
                    )
                    .bind(task_id)
                    .bind(&identity.created_at)
                    .fetch_all(&mut *conn)
                    .await?;
                    let mut seeds = Vec::new();
                    let mut valid = !creates.is_empty();
                    let mut linked = false;
                    for (change_id, payload) in &creates {
                        match generation_ids(payload, &identity, occurrence.slot_on, false) {
                            Some(ids) if ids.task_change_id == *change_id => {
                                let projection: Option<String> = sqlx::query_scalar(
                                    "SELECT payload FROM changes WHERE change_id = ? AND entity_type = 'recurrence_series' AND entity_id = ? AND field = 'projection' AND op_type = 'project_recurrence_occurrence' AND created_at = ?",
                                )
                                .bind(&ids.occurrence_change_id)
                                .bind(&occurrence.series_id)
                                .bind(&identity.occurrence_link.projected_at)
                                .fetch_optional(&mut *conn)
                                .await?;
                                if let Some(projection) = projection {
                                    valid &= generation_ids(
                                        &projection,
                                        &identity,
                                        occurrence.slot_on,
                                        true,
                                    )
                                    .is_some_and(|linked| linked == ids);
                                    linked = true;
                                }
                                seeds.push(ids.task_field_version_seed);
                            }
                            _ => valid = false,
                        }
                    }
                    if !valid || !linked {
                        deterministic_change_mismatches += 1;
                    }
                    for field in crate::task_fields::TaskField::VERSIONED {
                        let version: Option<String> = sqlx::query_scalar(
                            "SELECT version FROM field_versions WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ? AND field = ?",
                        )
                        .bind(&occurrence.workspace_id)
                        .bind(task_id)
                        .bind(field.as_str())
                        .fetch_optional(&mut *conn)
                        .await?;
                        let valid = match version {
                            Some(version) if seeds.contains(&version) => true,
                            Some(version) => {
                                sqlx::query_scalar::<_, i64>(
                                    "SELECT count(*) FROM changes WHERE change_id = ?",
                                )
                                .bind(version)
                                .fetch_one(&mut *conn)
                                .await?
                                    == 1
                            }
                            None => false,
                        };
                        if !valid {
                            deterministic_field_version_mismatches += 1;
                            break;
                        }
                    }
                }
                _ => deterministic_identity_mismatches += 1,
            }
        }
        if let Some(stopped_at) = &series.stopped_at {
            let payload: Option<String> =
                sqlx::query_scalar("SELECT payload FROM changes WHERE change_id = ?")
                    .bind(&occurrence.outcome_change_id)
                    .fetch_optional(&mut *conn)
                    .await?;
            let no_successor = payload
                .and_then(|payload| serde_json::from_str::<Value>(&payload).ok())
                .is_some_and(|payload| {
                    payload.get("successor_task_id").and_then(Value::as_str) == Some("")
                });
            let final_slot = final_slots.get(&(
                occurrence.workspace_id.clone(),
                occurrence.series_id.clone(),
            ));
            if !recurrence_stop_boundary_valid(
                stopped_at,
                occurrence.resolved_at.as_deref(),
                occurrence.archived_at.as_deref(),
                final_slot == Some(&occurrence.slot_on) && no_successor,
            ) {
                stop_boundary_violations += 1;
            }
        }
    }

    let pause_rows = sqlx::query("SELECT * FROM recurrence_pause_intervals")
        .fetch_all(&mut *conn)
        .await?;
    let invalid_pauses = pause_rows
        .iter()
        .filter(|row| {
            let Ok(pause) = recurrence_pause_interval_from_row(row) else {
                return true;
            };
            let Ok(paused_at) = DateTime::parse_from_rfc3339(&pause.paused_at) else {
                return true;
            };
            pause.resumed_at.as_deref().is_some_and(
                |resumed_at| match DateTime::parse_from_rfc3339(resumed_at) {
                    Ok(resumed_at) => resumed_at <= paused_at,
                    Err(_) => true,
                },
            )
        })
        .count();

    Ok(vec![
        issue_count_check("recurrence series rows", invalid_series),
        issue_count_check("recurrence series label rows", invalid_labels),
        issue_count_check("recurrence occurrence rows", invalid_occurrences),
        issue_count_check(
            "recurrence deterministic task identity",
            deterministic_identity_mismatches,
        ),
        issue_count_check(
            "recurrence deterministic changes",
            deterministic_change_mismatches,
        ),
        issue_count_check(
            "recurrence deterministic timestamps",
            deterministic_timestamp_mismatches,
        ),
        issue_count_check(
            "recurrence deterministic field versions",
            deterministic_field_version_mismatches,
        ),
        issue_count_check("recurrence schedule slots", off_lattice_slots),
        issue_count_check(
            "recurrence creation boundaries",
            creation_boundary_violations,
        ),
        issue_count_check("recurrence pause rows", invalid_pauses),
        issue_count_check("recurrence stop boundaries", stop_boundary_violations),
    ])
}

fn generation_ids(
    payload: &str,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
    slot_on: chrono::NaiveDate,
    projection: bool,
) -> Option<crate::recurrence::RecurrenceProposalIds> {
    let payload = serde_json::from_str::<Value>(payload).ok()?;
    identity.stored_generation_ids(&payload, slot_on, projection)
}

fn issue_count_check(label: &'static str, count: usize) -> IntegrityCheck {
    IntegrityCheck {
        label,
        ok: count == 0,
        value: format!("{count} invalid"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::projects::create_project;
    use crate::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSeriesId, TimeZoneId};
    use crate::test_support::test_conn;
    use crate::workspaces::{Workspace, default_workspace_id};

    async fn insert_series_fixture(
        conn: &mut SqliteConnection,
    ) -> (RecurrenceSeriesId, RecurrenceSchedule, String) {
        let project = create_project(conn, &Workspace::default(), "app")
            .await
            .unwrap();
        let series_id: RecurrenceSeriesId = "7KQ9A1X4MV2P8D6R".parse().unwrap();
        let schedule = RecurrenceSchedule::new(
            RecurrenceRule::daily(),
            "UTC".parse::<TimeZoneId>().unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            None,
            RecurrenceDuePolicy::SameDay,
        );
        sqlx::query(
            "INSERT INTO recurrence_series(
                workspace_id, id, title, description, project_id, priority, initial_status,
                frequency, interval, weekdays, timezone, start_on, available_local_time,
                due_policy, state, created_at, updated_at
             ) VALUES (?, ?, 'journal', '', ?, 'none', 'todo', 'daily', 1, '', 'UTC',
                       '2026-07-20', '', 'same_day', 'active', '2026-07-20T00:00:00Z',
                       '2026-07-20T00:00:00Z')",
        )
        .bind(default_workspace_id())
        .bind(series_id.to_string())
        .bind(&project.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        (series_id, schedule, project.id.to_string())
    }

    async fn insert_occurrence_task(
        conn: &mut SqliteConnection,
        series_id: &RecurrenceSeriesId,
        schedule: &RecurrenceSchedule,
        project_id: &str,
        slot_on: NaiveDate,
        projection_state: &str,
        status: &str,
    ) -> String {
        let identity =
            derive_occurrence_identity(&default_workspace_id(), series_id, schedule, slot_on)
                .unwrap();
        sqlx::query(
            "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES (?, ?, 'journal', '', ?, ?, 'none', ?, ?)",
        )
        .bind(&identity.task_id)
        .bind(default_workspace_id())
        .bind(project_id)
        .bind(status)
        .bind(&identity.created_at)
        .bind(&identity.updated_at)
        .execute(&mut *conn)
        .await
        .unwrap();
        let archived_at = if projection_state == "archived" {
            "2026-07-22T00:00:00Z"
        } else {
            ""
        };
        sqlx::query(
            "INSERT INTO recurrence_occurrences(
                workspace_id, series_id, slot_on, task_id, projection_state, archived_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(default_workspace_id())
        .bind(series_id.to_string())
        .bind(slot_on.to_string())
        .bind(&identity.task_id)
        .bind(projection_state)
        .bind(archived_at)
        .execute(&mut *conn)
        .await
        .unwrap();
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(local_seq), 0) + 1 FROM changes WHERE client_id = 'integrity-test'",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let identity_payload = serde_json::json!({
            "task_id": identity.task_id.as_str(),
            "series_id": series_id.as_str(),
            "slot_on": slot_on.to_string(),
            "task_change_id": identity.task_change_id,
            "occurrence_change_id": identity.occurrence_change_id,
            "task_field_version_seed": identity.field_version_seeds.task,
            "occurrence_field_version_seed": identity.field_version_seeds.occurrence,
        });
        let mut projection_payload = identity_payload.clone();
        projection_payload["projected_at"] =
            serde_json::Value::String(identity.occurrence_link.projected_at.clone());
        sqlx::query(
            "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, created_at)
             VALUES (?, 'integrity-test', ?, 'task', ?, NULL, 'create_task', ?, ?),
                    (?, 'integrity-test', ?, 'recurrence_series', ?, 'projection', 'project_recurrence_occurrence', ?, ?)",
        )
        .bind(&identity.task_change_id)
        .bind(next_seq)
        .bind(&identity.task_id)
        .bind(identity_payload.to_string())
        .bind(&identity.created_at)
        .bind(&identity.occurrence_change_id)
        .bind(next_seq + 1)
        .bind(series_id.to_string())
        .bind(projection_payload.to_string())
        .bind(&identity.occurrence_link.projected_at)
        .execute(&mut *conn)
        .await
        .unwrap();
        for field in crate::task_fields::TaskField::VERSIONED {
            sqlx::query(
                "INSERT INTO field_versions(workspace_id, entity_type, entity_id, field, version)
                 VALUES (?, 'task', ?, ?, ?)",
            )
            .bind(default_workspace_id())
            .bind(&identity.task_id)
            .bind(field.as_str())
            .bind(&identity.field_version_seeds.task)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        identity.task_id.to_string()
    }

    fn check<'a>(checks: &'a [IntegrityCheck], label: &str) -> &'a IntegrityCheck {
        checks.iter().find(|check| check.label == label).unwrap()
    }

    #[tokio::test]
    async fn healthy_recurrence_rows_pass_integrity_checks() {
        let (_temp, mut conn) = test_conn().await;
        let (series_id, schedule, project_id) = insert_series_fixture(&mut conn).await;
        insert_occurrence_task(
            &mut conn,
            &series_id,
            &schedule,
            &project_id,
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            "projected",
            "todo",
        )
        .await;

        let checks = recurrence_integrity_checks(&mut conn).await.unwrap();
        assert!(checks.iter().all(|check| check.ok), "{checks:#?}");
    }

    #[tokio::test]
    async fn stopped_series_may_keep_a_future_projected_occurrence() {
        let (_temp, mut conn) = test_conn().await;
        let (series_id, schedule, project_id) = insert_series_fixture(&mut conn).await;
        insert_occurrence_task(
            &mut conn,
            &series_id,
            &schedule,
            &project_id,
            NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
            "projected",
            "todo",
        )
        .await;
        sqlx::query(
            "UPDATE recurrence_series SET state = 'stopped', stopped_at = '2026-07-20T12:00:00Z' WHERE id = ?",
        )
        .bind(series_id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();

        let checks = recurrence_integrity_checks(&mut conn).await.unwrap();
        assert!(
            check(&checks, "recurrence stop boundaries").ok,
            "{checks:#?}"
        );
    }

    #[tokio::test]
    async fn recurrence_integrity_checks_report_projection_outcome_pause_and_stop_corruption() {
        let (_temp, mut conn) = test_conn().await;
        let (series_id, schedule, project_id) = insert_series_fixture(&mut conn).await;
        let projected_task = insert_occurrence_task(
            &mut conn,
            &series_id,
            &schedule,
            &project_id,
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            "projected",
            "todo",
        )
        .await;

        sqlx::query("DROP INDEX idx_recurrence_occurrences_active_projection")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO recurrence_occurrences(workspace_id, series_id, slot_on, task_id, projection_state)
             VALUES (?, ?, '2026-07-21', '7KQ9A1X4MV2P8D6T', 'projected')",
        )
        .bind(default_workspace_id())
        .bind(series_id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("UPDATE tasks SET status = 'done' WHERE id = ?")
            .bind(&projected_task)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO recurrence_occurrences(
                workspace_id, series_id, slot_on, task_id, projection_state, archived_at
             ) VALUES (?, ?, '2026-07-18', '7KQ9A1X4MV2P8D6V', 'archived', 't')",
        )
        .bind(default_workspace_id())
        .bind(series_id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO recurrence_pause_intervals(
                workspace_id, id, series_id, paused_at, resumed_at,
                created_by_change_id, resolved_by_change_id
             ) VALUES
                (?, 'pause-a', ?, '2026-07-20T01:00:00Z', '2026-07-20T05:00:00Z', 'a', 'ar'),
                (?, 'pause-b', ?, '2026-07-20T04:00:00Z', '2026-07-20T06:00:00Z', 'b', 'br')",
        )
        .bind(default_workspace_id())
        .bind(series_id.to_string())
        .bind(default_workspace_id())
        .bind(series_id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE recurrence_series SET state = 'stopped', stopped_at = '2026-07-20T12:00:00Z' WHERE id = ?",
        )
        .bind(series_id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
        insert_occurrence_task(
            &mut conn,
            &series_id,
            &schedule,
            &project_id,
            NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
            "archived",
            "todo",
        )
        .await;
        sqlx::query(
            "UPDATE recurrence_occurrences
             SET projection_state = 'resolved', outcome = 'completed', resolved_at = 't',
                 outcome_change_id = 'outcome-change', archived_at = ''
             WHERE series_id = ? AND slot_on = '2026-07-22'",
        )
        .bind(series_id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();

        let checks = recurrence_integrity_checks(&mut conn).await.unwrap();
        for label in [
            "recurrence projected uniqueness",
            "recurrence task links",
            "recurrence projected tasks",
            "recurrence task outcomes",
            "recurrence deterministic task identity",
            "recurrence schedule slots",
            "recurrence pause overlaps",
            "recurrence stop boundaries",
        ] {
            assert!(!check(&checks, label).ok, "{label} unexpectedly passed");
        }
    }
}
