use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::DateTime;
use sqlx::{SqliteConnection, query_scalar};

use crate::attachments::storage::{object_path, sha256_hex};
use crate::attachments::validation::SUPPORTED_MEDIA_TYPES;
use crate::db::{
    recurrence_occurrence_from_row, recurrence_pause_interval_from_row, recurrence_series_from_row,
    recurrence_series_label_from_row,
};
use crate::recurrence::{RecurrenceSchedule, derive_occurrence_identity, is_slot, slot_values};

use super::IntegrityCheck;

type AttachmentImageMetadataRow = (String, i64, String, Option<i64>, Option<i64>);

pub(crate) async fn recurrence_integrity_checks(
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
        "recurrence corrected slots",
        "SELECT count(*) FROM recurrence_occurrences WHERE projection_state = 'corrected' AND (task_id != '' OR outcome = '' OR resolved_at = '' OR outcome_change_id = '' OR archived_at != '')",
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
    let mut invalid_occurrences = 0_usize;
    let mut deterministic_identity_mismatches = 0_usize;
    let mut off_lattice_slots = 0_usize;
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
        if let Some(task_id) = &occurrence.task_id {
            match derive_occurrence_identity(
                &occurrence.workspace_id,
                &occurrence.series_id,
                &schedule,
                occurrence.slot_on,
            ) {
                Ok(identity) if identity.task_id == *task_id => {}
                _ => deterministic_identity_mismatches += 1,
            }
        }
        if let Some(stopped_at) = &series.stopped_at {
            let boundary = slot_values(&schedule, occurrence.slot_on)
                .ok()
                .and_then(|slot| DateTime::parse_from_rfc3339(&slot.boundary_at).ok());
            let stopped = DateTime::parse_from_rfc3339(stopped_at).ok();
            if boundary
                .zip(stopped)
                .is_none_or(|(boundary, stopped)| boundary > stopped)
            {
                stop_boundary_violations += 1;
            }
        }
    }

    let pause_rows = sqlx::query("SELECT * FROM recurrence_pause_intervals")
        .fetch_all(&mut *conn)
        .await?;
    let invalid_pauses = pause_rows
        .iter()
        .filter(|row| recurrence_pause_interval_from_row(row).is_err())
        .count();

    Ok(vec![
        issue_count_check("recurrence series rows", invalid_series),
        issue_count_check("recurrence series label rows", invalid_labels),
        issue_count_check("recurrence occurrence rows", invalid_occurrences),
        issue_count_check(
            "recurrence deterministic task identity",
            deterministic_identity_mismatches,
        ),
        issue_count_check("recurrence schedule slots", off_lattice_slots),
        issue_count_check("recurrence pause rows", invalid_pauses),
        issue_count_check("recurrence stop boundaries", stop_boundary_violations),
    ])
}

fn issue_count_check(label: &'static str, count: usize) -> IntegrityCheck {
    IntegrityCheck {
        label,
        ok: count == 0,
        value: format!("{count} invalid"),
    }
}

pub(crate) async fn attachment_integrity_checks(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    deep: bool,
) -> Result<Vec<IntegrityCheck>> {
    let mut checks = Vec::new();
    checks.push(count_check(
        conn,
        "attachment inventory",
        "SELECT count(*) FROM task_attachments ta LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256 WHERE bi.sha256 IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment tasks",
        "SELECT count(*) FROM task_attachments ta LEFT JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment changes",
        "SELECT count(*) FROM task_attachments ta LEFT JOIN changes c ON c.change_id = ta.created_by_change_id WHERE ta.created_by_change_id IS NOT NULL AND c.change_id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment inventory metadata",
        "SELECT count(*) FROM task_attachments ta JOIN blob_inventory bi ON bi.sha256 = ta.sha256 WHERE bi.byte_size != ta.byte_size OR bi.media_type != ta.media_type",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment media types",
        "SELECT count(*) FROM blob_inventory WHERE media_type NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')",
    )
    .await?);
    checks.push(
        count_check(
            conn,
            "attachment sizes",
            "SELECT count(*) FROM blob_inventory WHERE byte_size <= 0",
        )
        .await?,
    );
    checks.push(missing_blob_check(conn, blob_dir).await?);
    if deep {
        checks.push(mismatched_blob_check(conn, blob_dir).await?);
    }
    checks.push(orphan_blob_check(conn, blob_dir).await?);
    Ok(checks)
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

async fn missing_blob_check(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<IntegrityCheck> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT sha256 FROM blob_inventory WHERE available = 1")
            .fetch_all(&mut *conn)
            .await?;
    let count = rows
        .iter()
        .filter(|sha| object_path(blob_dir, sha).is_ok_and(|path| !path.exists()))
        .count();
    Ok(IntegrityCheck {
        label: "attachment objects",
        ok: count == 0,
        value: format!("{count} missing"),
    })
}

async fn mismatched_blob_check(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<IntegrityCheck> {
    let inventory: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT sha256, byte_size, media_type FROM blob_inventory WHERE available = 1",
    )
    .fetch_all(&mut *conn)
    .await?;
    let attachments: Vec<AttachmentImageMetadataRow> =
        sqlx::query_as("SELECT sha256, byte_size, media_type, width, height FROM task_attachments")
            .fetch_all(&mut *conn)
            .await?;
    let blob_dir = blob_dir.to_path_buf();
    let count = crate::attachments::blocking::run(move || {
        let mut count = 0_usize;
        let mut facts_by_hash = HashMap::new();
        for (sha, byte_size, media_type) in inventory {
            if !SUPPORTED_MEDIA_TYPES.contains(&media_type.as_str()) {
                continue;
            }
            let Ok(path) = object_path(&blob_dir, &sha) else {
                count += 1;
                continue;
            };
            if !path.exists() {
                continue;
            }
            let bytes = fs::read(&path).context("error attachment-object-read")?;
            if sha256_hex(&bytes) != sha || i64::try_from(bytes.len()).unwrap_or(-1) != byte_size {
                count += 1;
                continue;
            }
            let Ok(validated) =
                crate::attachments::decode::validate_image_blocking(bytes, Some(&media_type))
            else {
                count += 1;
                continue;
            };
            facts_by_hash.insert(sha, validated.facts);
        }
        for (sha, _byte_size, media_type, width, height) in attachments {
            let Some(facts) = facts_by_hash.get(&sha) else {
                continue;
            };
            if facts.media_type != media_type
                || (width, height) != (Some(facts.width), Some(facts.height))
            {
                count += 1;
            }
        }
        Ok(count)
    })
    .await?;
    Ok(IntegrityCheck {
        label: "attachment object hashes",
        ok: count == 0,
        value: format!("{count} mismatched"),
    })
}

async fn orphan_blob_check(conn: &mut SqliteConnection, blob_dir: &Path) -> Result<IntegrityCheck> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT sha256 FROM blob_inventory")
        .fetch_all(&mut *conn)
        .await?;
    let known = rows.into_iter().collect::<HashSet<_>>();
    let object_dir = blob_dir.join("objects").join("sha256");
    let mut count = 0_usize;
    if object_dir.exists() {
        for entry in
            fs::read_dir(&object_dir).context("could not read attachment object directory")?
        {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && let Some(name) = entry.file_name().to_str()
                && !known.contains(name)
            {
                count += 1;
            }
        }
    }
    Ok(IntegrityCheck {
        label: "attachment orphan objects",
        ok: count == 0,
        value: format!("{count} orphaned"),
    })
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
                workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                outcome_change_id, projection_state
             ) VALUES (?, ?, '2026-07-19', '7KQ9A1X4MV2P8D6V', 'completed', 't', 'c', 'corrected')",
        )
        .bind(default_workspace_id())
        .bind(series_id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO recurrence_occurrences(
                workspace_id, series_id, slot_on, outcome, resolved_at,
                outcome_change_id, projection_state
             ) VALUES (?, ?, '2026-07-18', 'skipped', 't', 'off-lattice', 'corrected')",
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
            "recurrence corrected slots",
            "recurrence deterministic task identity",
            "recurrence schedule slots",
            "recurrence pause overlaps",
            "recurrence stop boundaries",
        ] {
            assert!(!check(&checks, label).ok, "{label} unexpectedly passed");
        }
    }
}
