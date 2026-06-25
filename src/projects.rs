use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::config::{AppConfig, ProjectOverrideConfig};
use crate::db::insert_change;
use crate::fuzzy::is_near;
use crate::ids::{new_id, now};
use crate::render::{print_near_error, quote};
use crate::types::Project;
use crate::workspaces::{Workspace, active_workspace_id};

pub(crate) fn normalize_key(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[allow(dead_code)]
pub(crate) async fn resolve_project_for_add(
    conn: &mut SqliteConnection,
    project: Option<&str>,
) -> Result<Project> {
    resolve_project_for_add_in_workspace(conn, active_workspace_id().as_str(), project).await
}

pub(crate) async fn resolve_project_for_add_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project: Option<&str>,
) -> Result<Project> {
    if let Some(project) = project {
        return resolve_or_create_project(conn, workspace_id, project).await;
    }
    let config = AppConfig::load()?;
    if let Some(project) = project_from_config_override(conn, workspace_id, &config).await? {
        return Ok(project);
    }
    if let Some(project) = project_from_path_mapping(conn, workspace_id).await? {
        return Ok(project);
    }
    if let Some(root_name) = git_root_name()? {
        return resolve_or_create_project(conn, workspace_id, &root_name).await;
    }
    bail!("error project-required");
}

#[allow(dead_code)]
pub(crate) async fn resolve_existing_project(
    conn: &mut SqliteConnection,
    project: &str,
) -> Result<Project> {
    resolve_existing_project_in_workspace(conn, active_workspace_id().as_str(), project).await
}

pub(crate) async fn resolve_existing_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project: &str,
) -> Result<Project> {
    if let Some(project) = find_project_in_workspace(conn, workspace_id, project).await? {
        return Ok(project);
    }
    let choices = near_projects_in_workspace(conn, workspace_id, project).await?;
    if !choices.is_empty() {
        print_near_error("project", project, &choices);
    } else {
        eprintln!("error unknown-project input={}", project);
    }
    bail!("unknown project");
}

pub(crate) async fn inferred_project_key_for_add_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Option<String>> {
    let config = AppConfig::load()?;
    let workspace = crate::workspaces::active_workspace();
    if let Some(project) =
        matching_project_override(&config, Some(&workspace.id), Some(&workspace.key))?
    {
        return Ok(Some(normalize_key(&project)));
    }
    if let Some(project) = project_from_path_mapping(conn, workspace_id).await? {
        return Ok(Some(project.key));
    }
    Ok(git_root_name()?.map(|name| normalize_key(&name)))
}

pub(crate) async fn inferred_existing_project_key_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Option<String>> {
    let config = AppConfig::load()?;
    let workspace = crate::workspaces::active_workspace();
    if let Some(project) =
        matching_project_override(&config, Some(&workspace.id), Some(&workspace.key))?
        && let Some(project) = find_project_in_workspace(conn, workspace_id, &project).await?
    {
        return Ok(Some(project.key));
    }
    if let Some(project) = project_from_path_mapping(conn, workspace_id).await? {
        return Ok(Some(project.key));
    }
    let Some(root_name) = git_root_name()? else {
        return Ok(None);
    };
    Ok(find_project_in_workspace(conn, workspace_id, &root_name)
        .await?
        .map(|project| project.key))
}

#[allow(dead_code)]
pub(crate) async fn find_project(
    conn: &mut SqliteConnection,
    input: &str,
) -> Result<Option<Project>> {
    find_project_in_workspace(conn, active_workspace_id().as_str(), input).await
}

pub(crate) async fn find_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    input: &str,
) -> Result<Option<Project>> {
    let key = normalize_key(input);
    let row = sqlx::query(
        "SELECT id, workspace_id, key, name, prefix
         FROM projects
         WHERE workspace_id = ? AND deleted = 0 AND (key = ? OR lower(name) = lower(?))",
    )
    .bind(workspace_id)
    .bind(key)
    .bind(input)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(project_from_row))
}

#[allow(dead_code)]
pub(crate) async fn create_project(conn: &mut SqliteConnection, name: &str) -> Result<Project> {
    create_project_in_workspace(conn, active_workspace_id().as_str(), name)
        .await
        .map(|outcome| outcome.project)
}

#[allow(dead_code)]
pub(crate) async fn create_project_for_workspace(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    name: &str,
) -> Result<Project> {
    create_project_in_workspace(conn, &workspace.id, name)
        .await
        .map(|outcome| outcome.project)
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectCreateOutcome {
    pub(crate) project: Project,
    pub(crate) created: bool,
    pub(crate) change_id: Option<String>,
}

pub(crate) async fn create_project_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    name: &str,
) -> Result<ProjectCreateOutcome> {
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-project input={}", quote(name));
    }
    if let Some(project) = find_project_in_workspace(conn, &workspace.id, &key).await? {
        return Ok(ProjectCreateOutcome {
            project,
            created: false,
            change_id: None,
        });
    }
    if let Some((project, change_id)) =
        restore_deleted_project(conn, &workspace, &key, name).await?
    {
        return Ok(ProjectCreateOutcome {
            project,
            created: true,
            change_id: Some(change_id),
        });
    }
    let prefix = unique_project_prefix(conn, &workspace.id, &key).await?;
    let id = new_id();
    let ts = now();
    sqlx::query(
        "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&workspace.id)
    .bind(&key)
    .bind(name)
    .bind(&prefix)
    .bind(&ts)
    .bind(&ts)
    .execute(&mut *conn)
    .await?;
    let change_id = insert_change(
        conn,
        "project",
        &id,
        None,
        "create_project",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "key": key,
            "name": name,
            "prefix": prefix,
            "created_at": ts
        }),
        None,
    )
    .await?;
    Ok(ProjectCreateOutcome {
        project: Project {
            id,
            workspace_id: workspace.id,
            key,
            name: name.to_string(),
            prefix,
        },
        created: true,
        change_id: Some(change_id),
    })
}

async fn restore_deleted_project(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    key: &str,
    name: &str,
) -> Result<Option<(Project, String)>> {
    let Some(row) = sqlx::query(
        "SELECT id, workspace_id, key, prefix
         FROM projects
         WHERE workspace_id = ? AND key = ? AND deleted = 1",
    )
    .bind(&workspace.id)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let id: String = row.get("id");
    let workspace_id: String = row.get("workspace_id");
    let key: String = row.get("key");
    let prefix: String = row.get("prefix");
    let prefix = if prefix == id {
        unique_project_prefix(conn, &workspace_id, &key).await?
    } else {
        prefix
    };
    let ts = now();
    sqlx::query(
        "UPDATE projects SET name = ?, prefix = ?, updated_at = ?, deleted = 0
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(name)
    .bind(&prefix)
    .bind(&ts)
    .bind(&workspace_id)
    .bind(&id)
    .execute(&mut *conn)
    .await?;
    let change_id = insert_change(
        conn,
        "project",
        &id,
        None,
        "create_project",
        json!({
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
            "key": &key,
            "name": name,
            "prefix": &prefix,
            "created_at": ts
        }),
        None,
    )
    .await?;
    Ok(Some((
        Project {
            id,
            workspace_id,
            key,
            name: name.to_string(),
            prefix,
        },
        change_id,
    )))
}

async fn unique_project_prefix(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    key: &str,
) -> Result<String> {
    let base = prefix_base(key);
    let mut candidate = base.clone();
    let mut n = 2;
    while sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE workspace_id = ? AND prefix = ?",
    )
    .bind(workspace_id)
    .bind(&candidate)
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        candidate = format!("{}{}", base.chars().take(2).collect::<String>(), n);
        n += 1;
    }
    Ok(candidate)
}

pub(crate) fn prefix_base(key: &str) -> String {
    let words: Vec<&str> = key.split('-').filter(|word| !word.is_empty()).collect();
    if words.len() >= 2 {
        return words
            .iter()
            .filter_map(|word| word.chars().next())
            .take(3)
            .collect::<String>()
            .to_ascii_uppercase();
    }
    let key = words.first().copied().unwrap_or(key);
    let mut out = String::new();
    let mut chars = key.chars();
    if let Some(first) = chars.next() {
        out.push(first);
    }
    for ch in chars {
        if !"aeiou".contains(ch) {
            out.push(ch);
        }
        if out.len() >= 3 {
            break;
        }
    }
    for ch in key.chars() {
        if out.len() >= 3 {
            break;
        }
        if !out.contains(ch) {
            out.push(ch);
        }
    }
    while out.len() < 3 {
        out.push('X');
    }
    out.to_ascii_uppercase()
}

async fn resolve_or_create_project(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project: &str,
) -> Result<Project> {
    if let Some(existing) = find_project_in_workspace(conn, workspace_id, project).await? {
        return Ok(existing);
    }
    let choices = near_projects_in_workspace(conn, workspace_id, project).await?;
    if !choices.is_empty() {
        print_near_error("project", project, &choices);
        bail!("near-match project");
    }
    create_project_in_workspace(conn, workspace_id, project)
        .await
        .map(|outcome| outcome.project)
}

async fn project_from_path_mapping(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Option<Project>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let root = git_root(&cwd)?.unwrap_or_else(|| cwd.clone());
    let rows = sqlx::query(
        "SELECT p.id, p.workspace_id, p.key, p.name, p.prefix, pp.path
         FROM project_paths pp
         JOIN projects p ON p.workspace_id = pp.workspace_id AND p.id = pp.project_id
         WHERE pp.workspace_id = ? AND p.deleted = 0
         ORDER BY length(pp.path) DESC",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut best: Option<(PathMatch, Project)> = None;
    for row in rows {
        let path: String = row.get("path");
        let project = project_from_row(row);
        let path = Path::new(&path);
        if let Some(path_match) = matching_path(&cwd, &root, path)
            && best
                .as_ref()
                .is_none_or(|(best_match, _)| path_match.is_better_than(*best_match))
        {
            best = Some((path_match, project));
        }
    }
    Ok(best.map(|(_, project)| project))
}

async fn project_from_config_override(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    config: &AppConfig,
) -> Result<Option<Project>> {
    let workspace = crate::workspaces::active_workspace();
    let Some(project) =
        matching_project_override(config, Some(&workspace.id), Some(&workspace.key))?
    else {
        return Ok(None);
    };
    resolve_or_create_project(conn, workspace_id, &project)
        .await
        .map(Some)
}

pub(crate) fn project_has_config_mapping(
    workspace_id: &str,
    workspace_key: &str,
    project_key: &str,
) -> Result<bool> {
    let config = AppConfig::load()?;
    Ok(project_has_config_mapping_in_config(
        &config,
        workspace_id,
        workspace_key,
        project_key,
    ))
}

fn project_has_config_mapping_in_config(
    config: &AppConfig,
    workspace_id: &str,
    workspace_key: &str,
    project_key: &str,
) -> bool {
    config.has_project_override(Some(workspace_id), Some(workspace_key), project_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProjectConfig;

    #[test]
    fn config_mapping_detection_uses_supplied_workspace_key() {
        let config = AppConfig {
            project: ProjectConfig {
                overrides: vec![ProjectOverrideConfig {
                    workspace_id: None,
                    workspace: Some("client-work".to_string()),
                    project: "Mobile App".to_string(),
                    paths: Vec::new(),
                }],
            },
            ..AppConfig::default()
        };

        assert!(project_has_config_mapping_in_config(
            &config,
            "workspace-id",
            "client-work",
            "mobile-app",
        ));
        assert!(!project_has_config_mapping_in_config(
            &config,
            "workspace-id",
            "default",
            "mobile-app",
        ));
    }

    #[test]
    fn config_mapping_detection_preserves_unscoped_overrides() {
        let config = AppConfig {
            project: ProjectConfig {
                overrides: vec![ProjectOverrideConfig {
                    workspace_id: None,
                    workspace: None,
                    project: "Mobile App".to_string(),
                    paths: Vec::new(),
                }],
            },
            ..AppConfig::default()
        };

        assert!(project_has_config_mapping_in_config(
            &config,
            "workspace-id",
            "client-work",
            "mobile-app",
        ));
    }
}

fn matching_project_override(
    config: &AppConfig,
    workspace_id: Option<&str>,
    workspace: Option<&str>,
) -> Result<Option<String>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let root = git_root(&cwd)?.unwrap_or_else(|| cwd.clone());
    let mut best: Option<(PathMatch, bool, &ProjectOverrideConfig)> = None;
    for project_override in &config.project.overrides {
        let scoped =
            project_override.workspace_id.is_some() || project_override.workspace.is_some();
        let matches_workspace = match project_override.workspace_id.as_deref() {
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
            let Ok(path) = fs::canonicalize(path) else {
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

fn git_root_name() -> Result<Option<String>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let Some(root) = git_root(&cwd)? else {
        return Ok(None);
    };
    Ok(root
        .file_name()
        .map(|name| name.to_string_lossy().to_string()))
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
    let common_dir = if path.is_absolute() {
        path.to_path_buf()
    } else {
        git_dir.join(path)
    };
    Ok(Some(common_dir))
}

#[allow(dead_code)]
pub(crate) async fn add_project_path(
    conn: &mut SqliteConnection,
    project_key: &str,
    path: &Path,
) -> Result<()> {
    add_project_path_in_workspace(conn, active_workspace_id().as_str(), project_key, path).await
}

pub(crate) async fn add_project_path_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_key: &str,
    path: &Path,
) -> Result<()> {
    let project = resolve_existing_project_in_workspace(conn, workspace_id, project_key).await?;
    let path =
        fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))?;
    let path = path.display().to_string();
    sqlx::query(
        "INSERT OR IGNORE INTO project_paths(workspace_id, project_id, path) VALUES (?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(&project.id)
    .bind(path)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn near_projects_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    input: &str,
) -> Result<Vec<String>> {
    let needle = normalize_key(input);
    let projects = list_projects_in_workspace(conn, workspace_id, None).await?;
    Ok(projects
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
        .collect())
}

pub(crate) async fn list_projects(
    conn: &mut SqliteConnection,
    search: Option<&str>,
) -> Result<Vec<Project>> {
    list_projects_in_workspace(conn, active_workspace_id().as_str(), search).await
}

pub(crate) async fn list_projects_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    search: Option<&str>,
) -> Result<Vec<Project>> {
    let search = search.map(normalize_key);
    let rows = sqlx::query(
        "SELECT id, workspace_id, key, name, prefix
         FROM projects
         WHERE workspace_id = ? AND deleted = 0
         ORDER BY key",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    let projects = rows.into_iter().map(project_from_row).collect::<Vec<_>>();
    Ok(projects
        .into_iter()
        .filter(|project| {
            search.as_deref().is_none_or(|search| {
                project.key.contains(search) || project.name.to_lowercase().contains(search)
            })
        })
        .collect())
}

fn project_from_row(row: sqlx::sqlite::SqliteRow) -> Project {
    Project {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        key: row.get("key"),
        name: row.get("name"),
        prefix: row.get("prefix"),
    }
}
