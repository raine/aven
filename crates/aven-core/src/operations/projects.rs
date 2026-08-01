use crate::ids::{ProjectId, WorkspaceId};

use anyhow::{Result, bail};
use sqlx::{Row, SqliteConnection};
use tracing::info;

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate};
use crate::ids::now;
use crate::labels::normalize_label;
use crate::projects::{
    create_project_in_workspace, normalize_key, resolve_existing_project_in_workspace,
};
use crate::types::Project;
use crate::undo::{UndoCommand, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDeleteOutcome {
    pub name: String,
    pub changed: bool,
    pub task_count: usize,
    pub series_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRenameOutcome {
    pub previous_name: String,
    pub name: String,
    pub changed: bool,
    pub task_count: usize,
    pub series_count: usize,
}

pub struct LabelOutcome {
    pub name: String,
    pub created: bool,
    pub change_id: Option<String>,
}

pub struct ProjectOutcome {
    pub project: Project,
    pub created: bool,
    pub change_id: Option<String>,
}

pub struct ProjectDeleteOutcome {
    pub project: Project,
}

pub struct ProjectRenameOutcome {
    pub previous: Project,
    pub project: Project,
    pub changed: bool,
}

impl Database {
    pub async fn create_label(&self, workspace: &Workspace, name: &str) -> Result<LabelOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome = match create_label_operation(&mut tx, workspace, name).await {
            Ok(outcome) => outcome,
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn create_label_with_tui_undo(
        &self,
        workspace: &Workspace,
        name: &str,
    ) -> Result<LabelOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome = create_label_operation(&mut tx, workspace, name).await?;
        if outcome.created {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &format!("label {}", outcome.name),
                UndoPayload {
                    commands: vec![UndoCommand::DeleteCreatedLabel {
                        label: outcome.name.clone(),
                        create_change_id: outcome.change_id.clone().unwrap_or_default(),
                    }],
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn delete_label(
        &self,
        workspace: &Workspace,
        name: &str,
    ) -> Result<LabelDeleteOutcome> {
        let mut conn = self.acquire_writer().await?;
        delete_label_operation(&mut conn, workspace, name, false).await
    }

    pub async fn delete_label_with_tui_undo(
        &self,
        workspace: &Workspace,
        name: &str,
    ) -> Result<LabelDeleteOutcome> {
        let mut conn = self.acquire_writer().await?;
        delete_label_operation(&mut conn, workspace, name, true).await
    }

    pub async fn rename_label_with_tui_undo(
        &self,
        workspace: &Workspace,
        name: &str,
        new_name: &str,
    ) -> Result<LabelRenameOutcome> {
        let mut conn = self.acquire_writer().await?;
        rename_label_operation(&mut conn, workspace, name, new_name, true).await
    }

    pub async fn create_project(
        &self,
        workspace: &Workspace,
        name: &str,
    ) -> Result<ProjectOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome = match create_project_operation(&mut tx, workspace, name).await {
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
        let outcome = create_project_operation(&mut tx, workspace, name).await?;
        if outcome.created {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &format!("project {}", outcome.project.key),
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

pub(crate) async fn create_label_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
) -> Result<LabelOutcome> {
    let name = normalize_label(name);
    if name.is_empty() {
        bail!("error invalid-label");
    }
    let existed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?",
    )
    .bind(&workspace.id)
    .bind(&name)
    .fetch_one(&mut *conn)
    .await?
        > 0;
    let created_at = now();
    sqlx::query("INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)")
        .bind(&workspace.id)
        .bind(&name)
        .bind(&created_at)
        .execute(&mut *conn)
        .await?;
    let created = !existed;
    let change_id = if created {
        Some(
            append_change(
                conn,
                ChangeEntity::Label,
                &name,
                None,
                op_type::CREATE_LABEL,
                ChangePayload::workspace(workspace)
                    .set("name", name.clone())
                    .set("created_at", created_at),
            )
            .await?,
        )
    } else {
        None
    };
    if created {
        info!("label created");
    }
    Ok(LabelOutcome {
        name,
        created,
        change_id,
    })
}

pub async fn rename_label_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
    new_name: &str,
    tui_undo: bool,
) -> Result<LabelRenameOutcome> {
    let name = normalize_label(name);
    let new_name = normalize_label(new_name);
    if name.is_empty() || new_name.is_empty() {
        bail!("error invalid-label");
    }
    let mut tx = begin_immediate(conn).await?;
    let snapshot = label_snapshot(&mut tx, &workspace.id, &name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("error label-not-found label={name}"))?;
    if name == new_name {
        tx.commit().await?;
        return Ok(LabelRenameOutcome {
            previous_name: name.clone(),
            name,
            changed: false,
            task_count: snapshot.task_ids.len(),
            series_count: snapshot.series_ids.len(),
        });
    }
    if label_snapshot(&mut tx, &workspace.id, &new_name)
        .await?
        .is_some()
    {
        bail!("error label-exists label={new_name}");
    }
    set_label_name(&mut tx, workspace, &name, &new_name).await?;
    if tui_undo {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            &format!("label {new_name}"),
            UndoPayload {
                commands: vec![UndoCommand::SetLabelName {
                    before: name.clone(),
                    after: new_name.clone(),
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!("label renamed");
    Ok(LabelRenameOutcome {
        previous_name: name,
        name: new_name,
        changed: true,
        task_count: snapshot.task_ids.len(),
        series_count: snapshot.series_ids.len(),
    })
}

pub async fn delete_label_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
    tui_undo: bool,
) -> Result<LabelDeleteOutcome> {
    let name = normalize_label(name);
    if name.is_empty() {
        bail!("error invalid-label");
    }
    let mut tx = begin_immediate(conn).await?;
    let Some(snapshot) = label_snapshot(&mut tx, &workspace.id, &name).await? else {
        tx.commit().await?;
        return Ok(LabelDeleteOutcome {
            name,
            changed: false,
            task_count: 0,
            series_count: 0,
        });
    };
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND label = ?")
        .bind(&workspace.id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM recurrence_series_labels WHERE workspace_id = ? AND label = ?")
        .bind(&workspace.id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    let labels = sqlx::query("DELETE FROM labels WHERE workspace_id = ? AND name = ?")
        .bind(&workspace.id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    if labels.rows_affected() != 1 {
        bail!("error label-delete-race label={name}");
    }
    append_change(
        &mut tx,
        ChangeEntity::Label,
        &name,
        None,
        op_type::LABEL_DELETE,
        ChangePayload::workspace(workspace)
            .set("name", name.clone())
            .set("deleted_at", now()),
    )
    .await?;
    if tui_undo {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            &format!("label {name}"),
            UndoPayload {
                commands: vec![UndoCommand::RestoreDeletedLabel {
                    name: name.clone(),
                    created_at: snapshot.created_at.clone(),
                    task_ids: snapshot.task_ids.clone(),
                    series_ids: snapshot.series_ids.clone(),
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!("label deleted");
    Ok(LabelDeleteOutcome {
        name,
        changed: true,
        task_count: snapshot.task_ids.len(),
        series_count: snapshot.series_ids.len(),
    })
}

#[derive(Debug)]
struct LabelSnapshot {
    created_at: String,
    task_ids: Vec<crate::ids::TaskId>,
    series_ids: Vec<crate::recurrence::RecurrenceSeriesId>,
}

async fn label_snapshot(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
) -> Result<Option<LabelSnapshot>> {
    let Some(created_at) = sqlx::query_scalar::<_, String>(
        "SELECT created_at FROM labels WHERE workspace_id = ? AND name = ?",
    )
    .bind(workspace_id)
    .bind(name)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let task_ids = sqlx::query_scalar(
        "SELECT task_id FROM task_labels WHERE workspace_id = ? AND label = ? ORDER BY task_id",
    )
    .bind(workspace_id)
    .bind(name)
    .fetch_all(&mut *conn)
    .await?;
    let series_ids = sqlx::query_scalar(
        "SELECT series_id FROM recurrence_series_labels WHERE workspace_id = ? AND label = ? ORDER BY series_id",
    )
    .bind(workspace_id)
    .bind(name)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(LabelSnapshot {
        created_at,
        task_ids,
        series_ids,
    }))
}

pub(crate) async fn set_label_name(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
    new_name: &str,
) -> Result<()> {
    sqlx::query("UPDATE labels SET name = ? WHERE workspace_id = ? AND name = ?")
        .bind(new_name)
        .bind(&workspace.id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE task_labels SET label = ? WHERE workspace_id = ? AND label = ?")
        .bind(new_name)
        .bind(&workspace.id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE recurrence_series_labels SET label = ? WHERE workspace_id = ? AND label = ?",
    )
    .bind(new_name)
    .bind(&workspace.id)
    .bind(name)
    .execute(&mut *conn)
    .await?;
    append_change(
        conn,
        ChangeEntity::Label,
        name,
        None,
        op_type::SET_LABEL_NAME,
        ChangePayload::workspace(workspace)
            .set("name", name)
            .set("new_name", new_name)
            .set("renamed_at", now()),
    )
    .await?;
    Ok(())
}

pub(crate) async fn create_project_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
) -> Result<ProjectOutcome> {
    let outcome = create_project_in_workspace(conn, &workspace.id, name).await?;
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
    let task_refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND project_id = ?")
            .bind(&project.workspace_id)
            .bind(&project.id)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("DELETE FROM project_paths WHERE workspace_id = ? AND project_id = ?")
        .bind(&project.workspace_id)
        .bind(&project.id)
        .execute(&mut *tx)
        .await?;
    let deleted = if task_refs > 0 {
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
    info!("project deleted");
    Ok(ProjectDeleteOutcome { project })
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
            if project_prefix_exists(&mut tx, &workspace.id, &prefix, Some(&previous.id)).await? {
                bail!("error project-prefix-exists prefix={prefix}");
            }
            prefix
        }
        None => {
            unique_project_prefix_excluding(&mut tx, &workspace.id, &key, Some(&previous.id))
                .await?
        }
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
            &format!("project {}", outcome.project.key),
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

async fn unique_project_prefix_excluding(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    key: &str,
    exclude_project_id: Option<&ProjectId>,
) -> Result<String> {
    let base = crate::projects::prefix_base(key);
    let mut candidate = base.clone();
    let mut n = 2;
    while project_prefix_exists(conn, workspace_id, &candidate, exclude_project_id).await? {
        candidate = format!("{}{}", base.chars().take(2).collect::<String>(), n);
        n += 1;
    }
    Ok(candidate)
}

async fn project_prefix_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    prefix: &str,
    exclude_project_id: Option<&ProjectId>,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects
         WHERE workspace_id = ? AND prefix = ? AND (? IS NULL OR id != ?)",
    )
    .bind(workspace_id)
    .bind(prefix)
    .bind(exclude_project_id)
    .bind(exclude_project_id)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

fn normalize_prefix(prefix: &str) -> Result<String> {
    let prefix = prefix.trim().to_ascii_uppercase();
    if (2..=8).contains(&prefix.len()) && prefix.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        Ok(prefix)
    } else {
        bail!("error invalid-project-prefix prefix={prefix:?}")
    }
}
