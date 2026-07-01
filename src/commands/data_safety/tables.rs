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
        sqlx::query(
            "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at, deleted, is_epic) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.workspace_id)
        .bind(&row.id)
        .bind(&row.title)
        .bind(&row.description)
        .bind(&row.project_id)
        .bind(&row.status)
        .bind(&row.priority)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .bind(&row.queue_activity_at)
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
        sqlx::query("INSERT INTO field_versions(entity_id, field, version) VALUES (?, ?, ?)")
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
            "INSERT INTO conflicts(id, workspace_id, task_id, field, base_version, local_value, remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.id)
        .bind(&row.workspace_id)
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
