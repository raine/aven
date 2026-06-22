use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Connection as _, SqliteConnection};
use tracing::info;

use crate::config;
use crate::config_edit::{self, ProjectPathMappingEdit};
use crate::db::insert_change;
use crate::ids::now;
use crate::labels::normalize_label;
use crate::projects::{
    create_project_in_workspace, project_has_config_mapping, resolve_existing_project_in_workspace,
};
use crate::types::Project;
use crate::workspaces::Workspace;

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
    let mut tx = conn.begin().await?;
    let task_refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tasks WHERE workspace_id = ? AND project_key = ? AND deleted = 0",
    )
    .bind(&project.workspace_id)
    .bind(&project.key)
    .fetch_one(&mut *tx)
    .await?;
    if task_refs > 0 {
        bail!(
            "error project-has-tasks project={} tasks={}",
            project.key,
            task_refs
        );
    }
    sqlx::query("DELETE FROM project_paths WHERE workspace_id = ? AND project_key = ?")
        .bind(&project.workspace_id)
        .bind(&project.key)
        .execute(&mut *tx)
        .await?;
    let deleted = sqlx::query("DELETE FROM projects WHERE workspace_id = ? AND key = ?")
        .bind(&project.workspace_id)
        .bind(&project.key)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() != 1 {
        bail!("error project-delete-race project={}", project.key);
    }
    tx.commit().await?;
    info!(project_key = %project.key, "project deleted");
    Ok(ProjectDeleteOutcome {
        project,
        config_mapping,
    })
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
            "DELETE FROM project_paths WHERE workspace_id = ? AND project_key = ? AND path = ?",
        )
        .bind(&project.workspace_id)
        .bind(&project.key)
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
