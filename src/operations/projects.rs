use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use aven_core::db::Database;

use crate::config;
use crate::config_edit::{self, ProjectPathMappingEdit};
use crate::projects::project_has_config_mapping;
use crate::types::Project;
use crate::workspaces::Workspace;

pub struct ProjectPathOutcome {
    pub project: Project,
    pub path: String,
    pub config_path: PathBuf,
}

pub struct ProjectDeleteOutcome {
    pub project: Project,
}

pub struct ProjectRenameOutcome {
    pub previous: Project,
    pub project: Project,
    pub changed: bool,
    pub config_mapping: bool,
}

pub async fn create_project_operation(
    database: &Database,
    workspace: &Workspace,
    name: &str,
    path: Option<&Path>,
) -> Result<aven_core::operations::ProjectOutcome> {
    let path = path.map(canonicalize_project_path).transpose()?;
    let outcome = database.create_project(workspace, name).await?;
    if let Some(path) = path {
        save_project_path_mapping(workspace, &outcome.project, path)?;
    }
    Ok(outcome)
}

pub async fn delete_project_operation(
    database: &Database,
    workspace: &Workspace,
    project: &str,
) -> Result<ProjectDeleteOutcome> {
    let project = database
        .resolve_existing_project(&workspace.id, project)
        .await?;
    let outcome = database.delete_project(workspace, &project.key).await?;
    Ok(ProjectDeleteOutcome {
        project: outcome.project,
    })
}

pub async fn rename_project_operation(
    database: &Database,
    workspace: &Workspace,
    project: &str,
    new_name: &str,
    prefix: Option<&str>,
) -> Result<ProjectRenameOutcome> {
    let (outcome, config_mapping) =
        database
            .rename_project_before_commit(workspace, project, new_name, prefix, |outcome| {
                if outcome.changed {
                    rename_config_project_mapping(
                        workspace,
                        &outcome.previous.key,
                        &outcome.project.key,
                    )
                } else {
                    Ok(project_has_config_mapping(
                        &workspace.id,
                        &workspace.key,
                        &outcome.previous.key,
                    )
                    .unwrap_or(false))
                }
            })
            .await?;
    Ok(ProjectRenameOutcome {
        previous: outcome.previous,
        project: outcome.project,
        changed: outcome.changed,
        config_mapping,
    })
}

pub fn rename_config_project_mapping(
    workspace: &Workspace,
    old_project: &str,
    new_project: &str,
) -> Result<bool> {
    let config_path = config::config_file_path()?;
    config_edit::rename_project_path(&config_path, &workspace.id, old_project, new_project)
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

pub async fn add_project_path_operation(
    database: &Database,
    workspace: &Workspace,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let project = database
        .resolve_existing_project(&workspace.id, project)
        .await?;
    let path = canonicalize_project_path(path)?;
    save_project_path_mapping(workspace, &project, path)
}

pub async fn remove_project_path_operation(
    database: &Database,
    workspace: &Workspace,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let project = database
        .resolve_existing_project(&workspace.id, project)
        .await?;
    let config_path = config::config_file_path()?;
    let remove_paths = project_path_remove_candidates(path);
    config_edit::remove_project_path(&config_path, &workspace.id, &project.key, &remove_paths)?;
    for path in &remove_paths {
        database
            .remove_project_path(&project.workspace_id, &project.id, path)
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

pub async fn list_project_paths_operation(
    database: &Database,
    workspace: &Workspace,
    project: Option<&str>,
) -> Result<Vec<ProjectPathOutcome>> {
    let project = if let Some(project) = project {
        Some(
            database
                .resolve_existing_project(&workspace.id, project)
                .await?,
        )
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
        let project = database
            .resolve_existing_project(&workspace.id, &project_override.project_key())
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
