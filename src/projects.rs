use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::db::insert_change;
use crate::fuzzy::is_near;
use crate::ids::now;
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
        if let Some(existing) = find_project_in_workspace(conn, workspace_id, project).await? {
            return Ok(existing);
        }
        let choices = near_projects_in_workspace(conn, workspace_id, project).await?;
        if !choices.is_empty() {
            print_near_error("project", project, &choices);
            bail!("near-match project");
        }
        return create_project_in_workspace(conn, workspace_id, project)
            .await
            .map(|outcome| outcome.project);
    }
    if let Some(project) = project_from_path_mapping(conn, workspace_id).await? {
        return Ok(project);
    }
    if let Some(root_name) = git_root_name()? {
        if let Some(existing) = find_project_in_workspace(conn, workspace_id, &root_name).await? {
            return Ok(existing);
        }
        let choices = near_projects_in_workspace(conn, workspace_id, &root_name).await?;
        if !choices.is_empty() {
            print_near_error("project", &root_name, &choices);
            bail!("near-match project");
        }
        return create_project_in_workspace(conn, workspace_id, &root_name)
            .await
            .map(|outcome| outcome.project);
    }
    bail!("error project-required");
}

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
        "SELECT workspace_id, key, name, prefix
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
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-project input={}", quote(name));
    }
    if let Some(project) = find_project_in_workspace(conn, workspace_id, &key).await? {
        return Ok(ProjectCreateOutcome {
            project,
            created: false,
            change_id: None,
        });
    }
    let prefix = unique_project_prefix(conn, workspace_id, &key).await?;
    let ts = now();
    sqlx::query(
        "INSERT INTO projects(workspace_id, key, name, prefix, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(workspace_id)
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
        &key,
        None,
        "create_project",
        json!({
            "workspace_id": workspace_id,
            "workspace_key": crate::workspaces::active_workspace().key,
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
            workspace_id: workspace_id.to_string(),
            key,
            name: name.to_string(),
            prefix,
        },
        created: true,
        change_id: Some(change_id),
    })
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

async fn project_from_path_mapping(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Option<Project>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let rows = sqlx::query(
        "SELECT p.workspace_id, p.key, p.name, p.prefix, pp.path
         FROM project_paths pp
         JOIN projects p ON p.workspace_id = pp.workspace_id AND p.key = pp.project_key
         WHERE pp.workspace_id = ?
         ORDER BY length(pp.path) DESC",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let path: String = row.get("path");
        let project = project_from_row(row);
        if cwd.starts_with(Path::new(&path)) {
            return Ok(Some(project));
        }
    }
    Ok(None)
}

fn git_root_name() -> Result<Option<String>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let Some(root) = git_root(&cwd) else {
        return Ok(None);
    };
    Ok(root
        .file_name()
        .map(|name| name.to_string_lossy().to_string()))
}

fn git_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
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
    let path =
        fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))?;
    let path = path.display().to_string();
    sqlx::query(
        "INSERT OR IGNORE INTO project_paths(workspace_id, project_key, path) VALUES (?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(project_key)
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
        "SELECT workspace_id, key, name, prefix
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
        workspace_id: row.get("workspace_id"),
        key: row.get("key"),
        name: row.get("name"),
        prefix: row.get("prefix"),
    }
}
