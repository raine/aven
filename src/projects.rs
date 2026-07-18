use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use aven_core::db::Database;

use crate::config::{self, AppConfig, ProjectOverrideConfig};
use crate::fuzzy::is_near;
use crate::ids::WorkspaceId;
use crate::render::{print_near_error, quote};

pub use aven_core::projects::normalize_key;
#[cfg(test)]
pub use aven_core::test_support::create_project_in_workspace;

pub async fn resolve_project_key_for_add_with_database(
    database: &Database,
    workspace_id: &WorkspaceId,
    project: &str,
) -> Result<String> {
    if let Some(project) = database.find_project(workspace_id, project).await? {
        return Ok(project.key);
    }
    let needle = normalize_key(project);
    let choices = database
        .list_projects(workspace_id, None)
        .await?
        .into_iter()
        .filter(|project| is_near(&needle, &project.key))
        .map(|project| {
            format!(
                "{} prefix={} name={}",
                project.key,
                project.prefix,
                quote(&project.name)
            )
        })
        .collect::<Vec<_>>();
    if choices.is_empty() {
        return Ok(project.to_string());
    }
    print_near_error("project", project, &choices);
    bail!("near-match project")
}

pub async fn inferred_existing_project_key_with_database(
    database: &Database,
    workspace: &crate::workspaces::Workspace,
) -> Result<Option<String>> {
    let config = AppConfig::load()?;
    let config_candidate =
        matching_project_override(&config, Some(&workspace.id), Some(&workspace.key))?;
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let root = git_root(&cwd)?;
    database
        .inferred_existing_project_key(
            &workspace.id,
            config_candidate.as_deref(),
            &cwd,
            root.as_deref(),
        )
        .await
}

pub async fn inferred_project_key_for_add_with_database(
    database: &Database,
    workspace: &crate::workspaces::Workspace,
) -> Result<Option<String>> {
    let config = AppConfig::load()?;
    if let Some(project) =
        matching_project_override(&config, Some(&workspace.id), Some(&workspace.key))?
    {
        return Ok(Some(normalize_key(&project)));
    }
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let root = git_root(&cwd)?;
    if let Some(project) = database
        .inferred_existing_project_key(&workspace.id, None, &cwd, root.as_deref())
        .await?
    {
        return Ok(Some(project));
    }
    Ok(root
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .map(|name| normalize_key(&name)))
}

pub fn project_has_config_mapping(
    workspace_id: &WorkspaceId,
    workspace_key: &str,
    project_key: &str,
) -> Result<bool> {
    let config = AppConfig::load()?;
    Ok(config.has_project_override(Some(workspace_id), Some(workspace_key), project_key))
}

fn matching_project_override(
    config: &AppConfig,
    workspace_id: Option<&WorkspaceId>,
    workspace: Option<&str>,
) -> Result<Option<String>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let root = git_root(&cwd)?.unwrap_or_else(|| cwd.clone());
    let mut best: Option<(PathMatch, bool, &ProjectOverrideConfig)> = None;
    for project_override in &config.project.overrides {
        let scoped =
            project_override.workspace_id.is_some() || project_override.workspace.is_some();
        let matches_workspace = match project_override.workspace_id.as_ref() {
            Some(id) => Some(id) == workspace_id,
            None => project_override
                .workspace
                .as_deref()
                .is_none_or(|key| Some(key) == workspace),
        };
        if !matches_workspace {
            continue;
        }
        for path in &project_override.paths {
            let Ok(path) = config::expand_tilde(path).and_then(|path| {
                fs::canonicalize(&path)
                    .with_context(|| format!("could not resolve project path {}", path.display()))
            }) else {
                continue;
            };
            if let Some(path_match) = matching_path(&cwd, &root, &path)
                && best.as_ref().is_none_or(|(best_match, best_scoped, _)| {
                    path_match.is_better_than(*best_match)
                        || path_match == *best_match && scoped && !*best_scoped
                })
            {
                best = Some((path_match, scoped, project_override));
            }
        }
    }
    Ok(best.map(|(_, _, project_override)| project_override.project.clone()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathMatch {
    kind: PathMatchKind,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PathMatchKind {
    Root,
    Cwd,
}

impl PathMatch {
    fn is_better_than(self, other: Self) -> bool {
        self.kind > other.kind || self.kind == other.kind && self.len > other.len
    }
}

fn matching_path(cwd: &Path, root: &Path, path: &Path) -> Option<PathMatch> {
    let len = path.components().count();
    if cwd.starts_with(path) {
        return Some(PathMatch {
            kind: PathMatchKind::Cwd,
            len,
        });
    }
    if root == path || root.starts_with(path) {
        return Some(PathMatch {
            kind: PathMatchKind::Root,
            len,
        });
    }
    None
}

fn git_root(path: &Path) -> Result<Option<PathBuf>> {
    let Some(root) = path
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
    else {
        return Ok(None);
    };
    let git_file = root.join(".git");
    if git_file.is_file() {
        let text = fs::read_to_string(&git_file)
            .with_context(|| format!("could not read {}", git_file.display()))?;
        if let Some(path) = text.trim().strip_prefix("gitdir:").map(str::trim) {
            let git_dir = root.join(path);
            if let Some(common_dir) = common_git_dir(&git_dir)? {
                let common_dir = fs::canonicalize(&common_dir)
                    .with_context(|| format!("could not resolve {}", common_dir.display()))?;
                if let Some(main_root) = common_dir.parent() {
                    return fs::canonicalize(main_root)
                        .map(Some)
                        .with_context(|| format!("could not resolve {}", main_root.display()));
                }
            }
        }
    }
    Ok(Some(root.to_path_buf()))
}

fn common_git_dir(git_dir: &Path) -> Result<Option<PathBuf>> {
    let common_dir_file = git_dir.join("commondir");
    if !common_dir_file.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&common_dir_file)
        .with_context(|| format!("could not read {}", common_dir_file.display()))?;
    let path = Path::new(text.trim());
    Ok(Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        git_dir.join(path)
    }))
}
