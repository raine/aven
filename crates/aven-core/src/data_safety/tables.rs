use anyhow::Result;
use sqlx::sqlite::SqliteRow;
use sqlx::{FromRow, SqliteConnection};

pub(super) async fn scan_rows<T>(conn: &mut SqliteConnection, sql: &'static str) -> Result<Vec<T>>
where
    T: for<'r> FromRow<'r, SqliteRow> + Send + Unpin,
{
    Ok(sqlx::query_as::<_, T>(sql).fetch_all(conn).await?)
}

pub(super) async fn import_workspaces(
    tx: &mut SqliteConnection,
    rows: &[super::WorkspaceRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO workspaces(id, name, key, created_at, updated_at, archived) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.name)
        .bind(&row.key)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .bind(row.archived)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_projects(
    tx: &mut SqliteConnection,
    rows: &[super::ProjectRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at, deleted) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.workspace_id)
        .bind(&row.key)
        .bind(&row.name)
        .bind(&row.prefix)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .bind(row.deleted)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_project_id_aliases(
    tx: &mut SqliteConnection,
    rows: &[super::ProjectIdAliasRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO project_id_aliases(workspace_id, remote_project_id, local_project_id) VALUES (?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.remote_project_id)
        .bind(&row.local_project_id)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_project_paths(
    tx: &mut SqliteConnection,
    rows: &[super::ProjectPathRow],
) -> Result<()> {
    for row in rows {
        sqlx::query("INSERT INTO project_paths(workspace_id, project_id, path) VALUES (?, ?, ?)")
            .bind(&row.workspace_id)
            .bind(&row.project_id)
            .bind(&row.path)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

pub(super) async fn import_labels(
    tx: &mut SqliteConnection,
    rows: &[super::LabelRow],
) -> Result<()> {
    for row in rows {
        sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)")
            .bind(&row.workspace_id)
            .bind(&row.name)
            .bind(&row.created_at)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

pub(super) async fn import_tasks(tx: &mut SqliteConnection, rows: &[super::TaskRow]) -> Result<()> {
    for row in rows {
        let source = crate::choices::TaskSource::parse(&row.source)?;
        sqlx::query(
            "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, source, created_at, updated_at, queue_activity_at, available_at, due_on, deleted, is_epic) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.id)
        .bind(&row.title)
        .bind(&row.description)
        .bind(&row.project_id)
        .bind(&row.status)
        .bind(&row.priority)
        .bind(source.as_str())
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .bind(&row.queue_activity_at)
        .bind(&row.available_at)
        .bind(&row.due_on)
        .bind(row.deleted)
        .bind(row.is_epic)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_task_labels(
    tx: &mut SqliteConnection,
    rows: &[super::TaskLabelRow],
) -> Result<()> {
    for row in rows {
        sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
            .bind(&row.workspace_id)
            .bind(&row.task_id)
            .bind(&row.label)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

pub(super) async fn import_notes(tx: &mut SqliteConnection, rows: &[super::NoteRow]) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.id)
        .bind(&row.task_id)
        .bind(&row.body)
        .bind(&row.created_at)
        .bind(&row.change_id)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_task_dependencies(
    tx: &mut SqliteConnection,
    rows: &[super::TaskDependencyRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.task_id)
        .bind(&row.depends_on_task_id)
        .bind(&row.created_at)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_task_epic_links(
    tx: &mut SqliteConnection,
    rows: &[super::TaskEpicLinkRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO task_epic_links(workspace_id, child_task_id, epic_task_id, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.child_task_id)
        .bind(&row.epic_task_id)
        .bind(&row.created_at)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_task_attachments(
    tx: &mut SqliteConnection,
    rows: &[super::TaskAttachmentRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id, deleted, deleted_at, deleted_by_change_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.attachment_id)
        .bind(&row.task_id)
        .bind(&row.sha256)
        .bind(row.byte_size)
        .bind(&row.media_type)
        .bind(&row.filename)
        .bind(&row.alt_text)
        .bind(row.width)
        .bind(row.height)
        .bind(&row.created_at)
        .bind(&row.created_by_change_id)
        .bind(row.deleted)
        .bind(&row.deleted_at)
        .bind(&row.deleted_by_change_id)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_blob_inventory(
    tx: &mut SqliteConnection,
    rows: &[super::BlobInventoryExportRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at, last_verified_at) VALUES (?, ?, ?, 0, ?, NULL)",
        )
        .bind(&row.sha256)
        .bind(row.byte_size)
        .bind(&row.media_type)
        .bind(&row.first_seen_at)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_recurrence_series(
    tx: &mut SqliteConnection,
    rows: &[super::RecurrenceSeriesRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO recurrence_series(workspace_id, id, title, description, project_id, priority, initial_status, frequency, interval, weekdays, timezone, start_on, available_local_time, due_policy, state, stopped_at, created_at, updated_at, deleted) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.id)
        .bind(&row.title)
        .bind(&row.description)
        .bind(&row.project_id)
        .bind(&row.priority)
        .bind(&row.initial_status)
        .bind(&row.frequency)
        .bind(row.interval)
        .bind(&row.weekdays)
        .bind(&row.timezone)
        .bind(&row.start_on)
        .bind(&row.available_local_time)
        .bind(&row.due_policy)
        .bind(&row.state)
        .bind(&row.stopped_at)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .bind(row.deleted)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_recurrence_series_labels(
    tx: &mut SqliteConnection,
    rows: &[super::RecurrenceSeriesLabelRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO recurrence_series_labels(workspace_id, series_id, label) VALUES (?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.series_id)
        .bind(&row.label)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_recurrence_occurrences(
    tx: &mut SqliteConnection,
    rows: &[super::RecurrenceOccurrenceRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO recurrence_occurrences(workspace_id, series_id, slot_on, task_id, outcome, resolved_at, outcome_change_id, projection_state, archived_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.series_id)
        .bind(&row.slot_on)
        .bind(&row.task_id)
        .bind(&row.outcome)
        .bind(&row.resolved_at)
        .bind(&row.outcome_change_id)
        .bind(&row.projection_state)
        .bind(&row.archived_at)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_recurrence_pause_intervals(
    tx: &mut SqliteConnection,
    rows: &[super::RecurrencePauseIntervalRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO recurrence_pause_intervals(workspace_id, id, series_id, paused_at, resumed_at, suspended_slot_on, suspended_task_id, created_by_change_id, resolved_by_change_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.id)
        .bind(&row.series_id)
        .bind(&row.paused_at)
        .bind(&row.resumed_at)
        .bind(&row.suspended_slot_on)
        .bind(&row.suspended_task_id)
        .bind(&row.created_by_change_id)
        .bind(&row.resolved_by_change_id)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_changes(
    tx: &mut SqliteConnection,
    rows: &[super::ChangeRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.change_id)
        .bind(&row.client_id)
        .bind(row.local_seq)
        .bind(&row.entity_type)
        .bind(&row.entity_id)
        .bind(&row.field)
        .bind(&row.op_type)
        .bind(&row.payload)
        .bind(&row.base_version)
        .bind(&row.created_at)
        .bind(row.server_seq)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_field_versions(
    tx: &mut SqliteConnection,
    rows: &[super::FieldVersionRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO field_versions(workspace_id, entity_type, entity_id, field, version) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.entity_type)
        .bind(&row.entity_id)
        .bind(&row.field)
        .bind(&row.version)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn import_conflicts(
    tx: &mut SqliteConnection,
    rows: &[super::ConflictRow],
) -> Result<()> {
    for row in rows {
        sqlx::query(
            "INSERT INTO conflicts(id, workspace_id, entity_type, entity_id, task_id, field, base_version, local_value, remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.id)
        .bind(&row.workspace_id)
        .bind(&row.entity_type)
        .bind(&row.entity_id)
        .bind(&row.task_id)
        .bind(&row.field)
        .bind(&row.base_version)
        .bind(&row.local_value)
        .bind(&row.remote_value)
        .bind(&row.local_change_id)
        .bind(&row.remote_change_id)
        .bind(&row.variant_a)
        .bind(&row.variant_b)
        .bind(&row.created_at)
        .bind(row.resolved)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

#[allow(dead_code)]
pub(super) async fn import_meta(tx: &mut SqliteConnection, rows: &[super::MetaRow]) -> Result<()> {
    for row in rows {
        sqlx::query("INSERT INTO meta(key, value) VALUES (?, ?)")
            .bind(&row.key)
            .bind(&row.value)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}
