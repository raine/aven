use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};
use tracing::info;

use crate::config;
use crate::config_edit::{self, ProjectPathMappingEdit};
use crate::db::{begin_immediate, insert_change};
use crate::ids::now;
use crate::labels::normalize_label;
use crate::projects::{
    create_project_in_workspace, normalize_key, project_has_config_mapping,
    resolve_existing_project_in_workspace,
};
use crate::types::Project;
use crate::workspaces::Workspace;

pub(crate) struct LabelDeleteOutcome {
    pub(crate) name: String,
    pub(crate) changed: bool,
}

pub(crate) struct LabelOutcome {
    pub(crate) name: String,
    pub(crate) created: bool,
    pub(crate) change_id: Option<String>,
}

pub(crate) struct ProjectOutcome {
    pub(crate) project: Project,
    pub(crate) created: bool,
    pub(crate) change_id: Option<String>,
}

pub(crate) struct ProjectPathOutcome {
    pub(crate) project: Project,
    pub(crate) path: String,
    pub(crate) config_path: PathBuf,
}

pub(crate) struct ProjectDeleteOutcome {
    pub(crate) project: Project,
    pub(crate) config_mapping: bool,
}

pub(crate) struct ProjectRenameOutcome {
    pub(crate) previous: Project,
    pub(crate) project: Project,
    pub(crate) changed: bool,
    pub(crate) config_mapping: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ProjectMetadata<'a> {
    pub(crate) key: &'a str,
    pub(crate) name: &'a str,
    pub(crate) prefix: &'a str,
}

struct ProjectPathTarget {
    workspace: Workspace,
    project: Project,
    path: PathBuf,
}
pub(crate) async fn create_label_operation(
    conn: &mut SqliteConnection,
    name: &str,
) -> Result<LabelOutcome> {
    create_label_operation_in_workspace(
        conn,
        crate::workspaces::active_workspace_id().as_str(),
        name,
    )
    .await
}

pub(crate) async fn create_label_operation_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    name: &str,
) -> Result<LabelOutcome> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
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
            insert_change(
                conn,
                "label",
                &name,
                None,
                "create_label",
                json!({
                    "workspace_id": &workspace.id,
                    "workspace_key": &workspace.key,
                    "name": name,
                    "created_at": created_at,
                }),
                None,
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

pub(crate) async fn delete_label_operation(
    conn: &mut SqliteConnection,
    name: &str,
) -> Result<LabelDeleteOutcome> {
    let workspace = crate::workspaces::active_workspace();
    let name = normalize_label(name);
    if name.is_empty() {
        bail!("error invalid-label");
    }
    let mut tx = begin_immediate(conn).await?;
    let deleted_at = now();
    let task_labels = sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND label = ?")
        .bind(&workspace.id)
        .bind(&name)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let labels = sqlx::query("DELETE FROM labels WHERE workspace_id = ? AND name = ?")
        .bind(&workspace.id)
        .bind(&name)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let changed = task_labels > 0 || labels > 0;
    if changed {
        insert_change(
            &mut tx,
            "label",
            &name,
            None,
            "label_delete",
            json!({
                "workspace_id": &workspace.id,
                "workspace_key": &workspace.key,
                "name": &name,
                "deleted_at": &deleted_at,
            }),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    if changed {
        info!("label deleted");
    }
    Ok(LabelDeleteOutcome { name, changed })
}

pub(crate) async fn create_project_operation(
    conn: &mut SqliteConnection,
    name: &str,
    path: Option<&Path>,
) -> Result<ProjectOutcome> {
    let workspace = crate::workspaces::active_workspace();
    let path = path.map(canonicalize_project_path).transpose()?;
    let outcome = create_project_in_workspace(conn, &workspace.id, name).await?;
    if let Some(path) = path {
        save_project_path_mapping(&workspace, &outcome.project, path)?;
    }
    if outcome.created {
        info!(project_key = %outcome.project.key, "project created");
    }
    Ok(ProjectOutcome {
        project: outcome.project,
        created: outcome.created,
        change_id: outcome.change_id,
    })
}

pub(crate) async fn delete_project_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project: &str,
) -> Result<ProjectDeleteOutcome> {
    let project = resolve_existing_project_in_workspace(conn, &workspace.id, project).await?;
    let config_mapping =
        project_has_config_mapping(&workspace.id, &workspace.key, &project.key).unwrap_or(false);
    let mut tx = begin_immediate(conn).await?;
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
    insert_change(
        &mut tx,
        "project",
        &project.id,
        None,
        "project_delete",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "deleted_at": &deleted_at,
        }),
        None,
    )
    .await?;
    tx.commit().await?;
    info!("project deleted");
    Ok(ProjectDeleteOutcome {
        project,
        config_mapping,
    })
}

pub(crate) async fn rename_project_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project: &str,
    new_name: &str,
    prefix: Option<&str>,
) -> Result<ProjectRenameOutcome> {
    let previous = resolve_existing_project_in_workspace(conn, &workspace.id, project).await?;
    let new_name = new_name.trim();
    let key = normalize_key(new_name);
    if key.is_empty() {
        bail!(
            "error invalid-project input={}",
            crate::render::quote(new_name)
        );
    }
    if let Some(existing) = resolve_project_key(conn, &workspace.id, &key).await?
        && existing.id != previous.id
    {
        bail!("error project-exists project={key}");
    }
    let prefix = match prefix {
        Some(prefix) => {
            let prefix = normalize_prefix(prefix)?;
            if project_prefix_exists(conn, &workspace.id, &prefix, Some(&previous.id)).await? {
                bail!("error project-prefix-exists prefix={prefix}");
            }
            prefix
        }
        None => {
            unique_project_prefix_excluding(conn, &workspace.id, &key, Some(&previous.id)).await?
        }
    };
    let changed = previous.key != key || previous.name != new_name || previous.prefix != prefix;
    let config_mapping = if !changed {
        project_has_config_mapping(&workspace.id, &workspace.key, &previous.key).unwrap_or(false)
    } else {
        false
    };
    if !changed {
        return Ok(ProjectRenameOutcome {
            project: previous.clone(),
            previous,
            changed: false,
            config_mapping,
        });
    }
    let mut tx = begin_immediate(conn).await?;
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
    let config_mapping = rename_config_project_mapping(workspace, &previous.key, &key)?;
    tx.commit().await?;
    info!(project_id = %project.id, project_key = %project.key, "project renamed");
    Ok(ProjectRenameOutcome {
        previous,
        project,
        changed: true,
        config_mapping,
    })
}

pub(crate) async fn set_project_metadata(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project_id: &str,
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
        id: project_id.to_string(),
        workspace_id: workspace.id.clone(),
        key: metadata.key.to_string(),
        name: metadata.name.to_string(),
        prefix: metadata.prefix.to_string(),
    })
}

pub(crate) async fn insert_project_metadata_change(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    project_id: &str,
    metadata: ProjectMetadata<'_>,
    updated_at: &str,
) -> Result<String> {
    insert_change(
        conn,
        "project",
        project_id,
        None,
        "set_project_metadata",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "key": metadata.key,
            "name": metadata.name,
            "prefix": metadata.prefix,
            "updated_at": updated_at,
        }),
        None,
    )
    .await
}

pub(crate) fn rename_config_project_mapping(
    workspace: &Workspace,
    old_project: &str,
    new_project: &str,
) -> Result<bool> {
    let config_path = config::config_file_path()?;
    config_edit::rename_project_path(&config_path, &workspace.id, old_project, new_project)
}

async fn resolve_project_key(
    conn: &mut SqliteConnection,
    workspace_id: &str,
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
    workspace_id: &str,
    key: &str,
    exclude_project_id: Option<&str>,
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
    workspace_id: &str,
    prefix: &str,
    exclude_project_id: Option<&str>,
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
        bail!(
            "error invalid-project-prefix prefix={}",
            crate::render::quote(&prefix)
        )
    }
}

fn canonicalize_project_path(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))
}

fn project_path_remove_candidates(path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = fs::canonicalize(path) {
        paths.push(path);
    }
    let supplied = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    if !paths.iter().any(|path| path == &supplied) {
        paths.push(supplied);
    }
    paths
}

async fn resolve_project_path_target(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathTarget> {
    let workspace = crate::workspaces::active_workspace();
    let project = resolve_existing_project_in_workspace(conn, &workspace.id, project).await?;
    let path = canonicalize_project_path(path)?;
    Ok(ProjectPathTarget {
        workspace,
        project,
        path,
    })
}

fn save_project_path_mapping(
    workspace: &Workspace,
    project: &Project,
    path: PathBuf,
) -> Result<ProjectPathOutcome> {
    let config_path = config::config_file_path()?;
    config_edit::add_project_path(
        &config_path,
        ProjectPathMappingEdit {
            workspace_id: &workspace.id,
            workspace: &workspace.key,
            project: &project.key,
            path: path.clone(),
        },
    )?;
    Ok(ProjectPathOutcome {
        project: project.clone(),
        path: path.display().to_string(),
        config_path,
    })
}

pub(crate) async fn add_project_path_operation(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let target = resolve_project_path_target(conn, project, path).await?;
    save_project_path_mapping(&target.workspace, &target.project, target.path)
}

pub(crate) async fn remove_project_path_operation(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let workspace = crate::workspaces::active_workspace();
    let project = resolve_existing_project_in_workspace(conn, &workspace.id, project).await?;
    let config_path = config::config_file_path()?;
    let remove_paths = project_path_remove_candidates(path);
    config_edit::remove_project_path(&config_path, &workspace.id, &project.key, &remove_paths)?;
    for path in &remove_paths {
        sqlx::query(
            "DELETE FROM project_paths WHERE workspace_id = ? AND project_id = ? AND path = ?",
        )
        .bind(&project.workspace_id)
        .bind(&project.id)
        .bind(path.display().to_string())
        .execute(&mut *conn)
        .await?;
    }
    let path = remove_paths
        .first()
        .unwrap_or(&path.to_path_buf())
        .display()
        .to_string();
    Ok(ProjectPathOutcome {
        project,
        path,
        config_path,
    })
}

pub(crate) async fn list_project_paths_operation(
    conn: &mut SqliteConnection,
    project: Option<&str>,
) -> Result<Vec<ProjectPathOutcome>> {
    let workspace = crate::workspaces::active_workspace();
    let project = if let Some(project) = project {
        Some(resolve_existing_project_in_workspace(conn, &workspace.id, project).await?)
    } else {
        None
    };
    let project_key = project.as_ref().map(|project| project.key.as_str());
    let config_path = config::config_file_path()?;
    let config = config::AppConfig::load_from_path(&config_path)?;
    let mut paths = Vec::new();
    for project_override in config.project.overrides {
        if !project_override.matches_workspace(Some(&workspace.id), Some(&workspace.key)) {
            continue;
        }
        if project_key.is_some_and(|key| project_override.project_key() != key) {
            continue;
        }
        let project = resolve_existing_project_in_workspace(
            conn,
            &workspace.id,
            &project_override.project_key(),
        )
        .await?;
        paths.extend(
            project_override
                .paths
                .into_iter()
                .map(|path| ProjectPathOutcome {
                    project: project.clone(),
                    path: path.display().to_string(),
                    config_path: config_path.clone(),
                }),
        );
    }
    paths.sort_by(|left, right| {
        left.project
            .key
            .cmp(&right.project.key)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(paths)
}
