use crate::ids::{ProjectId, WorkspaceId};
use crate::recurrence::RecurrenceSeriesId;

use anyhow::{Result, bail};
use sqlx::{Row, SqliteConnection};
use tracing::info;

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate};
use crate::ids::now;
use crate::projects::{
    create_project_in_workspace, ensure_project_prefix_available, normalize_key, normalize_prefix,
    resolve_existing_project_in_workspace,
};
use crate::types::Project;
use crate::undo::{UndoCommand, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

pub struct ProjectOutcome {
    pub project: Project,
    pub created: bool,
    pub change_id: Option<String>,
}

pub struct ProjectDeleteOutcome {
    pub project: Project,
    pub stopped_series_count: usize,
}

pub struct ProjectRenameOutcome {
    pub previous: Project,
    pub project: Project,
    pub changed: bool,
}

impl Database {
    pub async fn create_project(
        &self,
        workspace: &Workspace,
        name: &str,
        prefix: Option<&str>,
    ) -> Result<ProjectOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome = match create_project_operation(&mut tx, workspace, name, prefix).await {
            Ok(outcome) => outcome,
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn create_project_with_tui_undo(
        &self,
        workspace: &Workspace,
        name: &str,
    ) -> Result<ProjectOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome = create_project_operation(&mut tx, workspace, name, None).await?;
        if outcome.created {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                UndoPayload {
                    commands: vec![UndoCommand::DeleteCreatedProject {
                        project_key: outcome.project.key.clone(),
                        create_change_id: outcome.change_id.clone().unwrap_or_default(),
                        expected_name: outcome.project.name.clone(),
                        expected_prefix: outcome.project.prefix.clone(),
                    }],
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn delete_project(
        &self,
        workspace: &Workspace,
        project: &str,
    ) -> Result<ProjectDeleteOutcome> {
        let mut conn = self.acquire_writer().await?;
        delete_project_operation(&mut conn, workspace, project).await
    }

    pub async fn rename_project(
        &self,
        workspace: &Workspace,
        project: &str,
        new_name: &str,
        prefix: Option<&str>,
    ) -> Result<ProjectRenameOutcome> {
        let mut conn = self.acquire_writer().await?;
        rename_project_operation(&mut conn, workspace, project, new_name, prefix).await
    }

    pub async fn rename_project_with_tui_undo(
        &self,
        workspace: &Workspace,
        project: &str,
        new_name: &str,
        prefix: Option<&str>,
    ) -> Result<ProjectRenameOutcome> {
        let mut conn = self.acquire_writer().await?;
        let (outcome, ()) = rename_project_operation_before_commit(
            &mut conn,
            workspace,
            project,
            new_name,
            prefix,
            |_| Ok(()),
            true,
        )
        .await?;
        Ok(outcome)
    }

    pub async fn rename_project_before_commit<T, F>(
        &self,
        workspace: &Workspace,
        project: &str,
        new_name: &str,
        prefix: Option<&str>,
        before_commit: F,
    ) -> Result<(ProjectRenameOutcome, T)>
    where
        F: FnOnce(&ProjectRenameOutcome) -> Result<T>,
    {
        let mut conn = self.acquire_writer().await?;
        rename_project_operation_before_commit(
            &mut conn,
            workspace,
            project,
            new_name,
            prefix,
            before_commit,
            false,
        )
        .await
    }
}

#[derive(Clone, Copy)]
pub struct ProjectMetadata<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub prefix: &'a str,
}

pub(crate) async fn create_project_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
    prefix: Option<&str>,
) -> Result<ProjectOutcome> {
    let outcome = create_project_in_workspace(conn, &workspace.id, name, prefix).await?;
    if outcome.created {
        info!(project_key = %outcome.project.key, "project created");
    }
    Ok(ProjectOutcome {
        project: outcome.project,
        created: outcome.created,
        change_id: outcome.change_id,
    })
}

pub async fn delete_project_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project: &str,
) -> Result<ProjectDeleteOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let project = resolve_existing_project_in_workspace(&mut tx, &workspace.id, project).await?;
    let deleted_at = now();
    let series_ids = sqlx::query_scalar::<_, RecurrenceSeriesId>(
        "SELECT id FROM recurrence_series
         WHERE workspace_id = ? AND project_id = ? AND deleted = 0 AND state != 'stopped'
         ORDER BY id",
    )
    .bind(&project.workspace_id)
    .bind(&project.id)
    .fetch_all(&mut *tx)
    .await?;
    for series_id in &series_ids {
        super::recurrence::stop_recurrence_series_for_project_delete_in_transaction(
            &mut tx,
            workspace,
            series_id,
            &deleted_at,
        )
        .await?;
    }
    let task_refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND project_id = ?")
            .bind(&project.workspace_id)
            .bind(&project.id)
            .fetch_one(&mut *tx)
            .await?;
    let series_refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_series WHERE workspace_id = ? AND project_id = ?",
    )
    .bind(&project.workspace_id)
    .bind(&project.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM project_paths WHERE workspace_id = ? AND project_id = ?")
        .bind(&project.workspace_id)
        .bind(&project.id)
        .execute(&mut *tx)
        .await?;
    let deleted = if task_refs > 0 || series_refs > 0 {
        sqlx::query(
            "UPDATE projects SET deleted = 1, updated_at = ? WHERE workspace_id = ? AND key = ?",
        )
        .bind(&deleted_at)
        .bind(&project.workspace_id)
        .bind(&project.key)
        .execute(&mut *tx)
        .await?
    } else {
        sqlx::query("DELETE FROM projects WHERE workspace_id = ? AND key = ?")
            .bind(&project.workspace_id)
            .bind(&project.key)
            .execute(&mut *tx)
            .await?
    };
    if deleted.rows_affected() != 1 {
        bail!("error project-delete-race project={}", project.key);
    }
    append_change(
        &mut tx,
        ChangeEntity::Project,
        project.id.as_str(),
        None,
        op_type::PROJECT_DELETE,
        ChangePayload::workspace(workspace).set("deleted_at", deleted_at),
    )
    .await?;
    tx.commit().await?;
    info!(stopped_series_count = series_ids.len(), "project deleted");
    Ok(ProjectDeleteOutcome {
        project,
        stopped_series_count: series_ids.len(),
    })
}

pub async fn rename_project_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project: &str,
    new_name: &str,
    prefix: Option<&str>,
) -> Result<ProjectRenameOutcome> {
    let (outcome, ()) = rename_project_operation_before_commit(
        conn,
        workspace,
        project,
        new_name,
        prefix,
        |_| Ok(()),
        false,
    )
    .await?;
    Ok(outcome)
}

async fn rename_project_operation_before_commit<T, F>(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project: &str,
    new_name: &str,
    prefix: Option<&str>,
    before_commit: F,
    tui_undo: bool,
) -> Result<(ProjectRenameOutcome, T)>
where
    F: FnOnce(&ProjectRenameOutcome) -> Result<T>,
{
    let mut tx = begin_immediate(conn).await?;
    let previous = resolve_existing_project_in_workspace(&mut tx, &workspace.id, project).await?;
    let new_name = new_name.trim();
    let key = normalize_key(new_name);
    if key.is_empty() {
        bail!("error invalid-project input={new_name:?}");
    }
    if let Some(existing) = resolve_project_key(&mut tx, &workspace.id, &key).await?
        && existing.id != previous.id
    {
        bail!("error project-exists project={key}");
    }
    let prefix = match prefix {
        Some(prefix) => {
            let prefix = normalize_prefix(prefix)?;
            ensure_project_prefix_available(&mut tx, &workspace.id, &prefix, Some(&previous.id))
                .await?;
            prefix
        }
        None => previous.prefix.clone(),
    };
    let changed = previous.key != key || previous.name != new_name || previous.prefix != prefix;
    if !changed {
        let outcome = ProjectRenameOutcome {
            project: previous.clone(),
            previous,
            changed: false,
        };
        let value = before_commit(&outcome)?;
        tx.commit().await?;
        return Ok((outcome, value));
    }
    let project = set_project_metadata(
        &mut tx,
        workspace,
        &previous.id,
        ProjectMetadata {
            key: &key,
            name: new_name,
            prefix: &prefix,
        },
        true,
    )
    .await?;
    let outcome = ProjectRenameOutcome {
        previous,
        project,
        changed: true,
    };
    let value = before_commit(&outcome)?;
    if tui_undo {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            UndoPayload {
                commands: vec![UndoCommand::SetProjectMetadata {
                    project_id: outcome.project.id.clone(),
                    before_key: outcome.previous.key.clone(),
                    before_name: outcome.previous.name.clone(),
                    before_prefix: outcome.previous.prefix.clone(),
                    after_key: outcome.project.key.clone(),
                    after_name: outcome.project.name.clone(),
                    after_prefix: outcome.project.prefix.clone(),
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!(project_id = %outcome.project.id, project_key = %outcome.project.key, "project renamed");
    Ok((outcome, value))
}

pub async fn set_project_metadata(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project_id: &ProjectId,
    metadata: ProjectMetadata<'_>,
    record_change: bool,
) -> Result<Project> {
    let ts = now();
    let updated = sqlx::query(
        "UPDATE projects SET key = ?, name = ?, prefix = ?, updated_at = ?
         WHERE workspace_id = ? AND id = ? AND deleted = 0",
    )
    .bind(metadata.key)
    .bind(metadata.name)
    .bind(metadata.prefix)
    .bind(&ts)
    .bind(&workspace.id)
    .bind(project_id)
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() != 1 {
        bail!("error project-metadata-target-missing project_id={project_id}");
    }
    if record_change {
        insert_project_metadata_change(conn, workspace, project_id, metadata, &ts).await?;
    }
    Ok(Project {
        id: project_id.clone(),
        workspace_id: workspace.id.clone(),
        key: metadata.key.to_string(),
        name: metadata.name.to_string(),
        prefix: metadata.prefix.to_string(),
    })
}

pub async fn insert_project_metadata_change(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project_id: &ProjectId,
    metadata: ProjectMetadata<'_>,
    updated_at: &str,
) -> Result<String> {
    append_change(
        conn,
        ChangeEntity::Project,
        project_id.as_str(),
        None,
        op_type::SET_PROJECT_METADATA,
        ChangePayload::workspace(workspace)
            .set("key", metadata.key)
            .set("name", metadata.name)
            .set("prefix", metadata.prefix)
            .set("updated_at", updated_at),
    )
    .await
}

async fn resolve_project_key(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    key: &str,
) -> Result<Option<Project>> {
    let row = sqlx::query(
        "SELECT id, workspace_id, key, name, prefix
         FROM projects
         WHERE workspace_id = ? AND key = ?",
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|row| Project {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        key: row.get("key"),
        name: row.get("name"),
        prefix: row.get("prefix"),
    }))
}

#[cfg(test)]
mod tests;
